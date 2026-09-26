//! End-to-end test for `@flat`: a real `compile_to_world()` call must produce
//! a world with no child brick grid and no microchip brick, under every
//! layout engine, and the resulting single grid must still satisfy the two
//! invariants the game enforces silently — no overlapping bricks (the game
//! DROPS them at load) and no two wires into one target port (the game
//! rejects the wire at load).

use wirescript::compile::{CompileError, CompileInput, FoldMode, compile_to_world, diagnostics_only};
use wirescript::diagnostic::Severity;

/// Two named chips (one nested inside the other, and one instantiated
/// twice — so template instantiation is exercised), an anonymous chip, a
/// chip that owns its own handler, and a chip reading an outer variable.
/// Between them these cover every route a node takes into a child module.
const BODY: &str = r#"chip Double(a: int) -> (r: int) {
  return a * 2
}

chip Scale(x: int) -> (y: int) {
  let d = Double(x)
  return d + offset
}

var offset: int = 1
var total: int = 0
in tick: exec
out sum: int

chip {
  var scratch: int = 0
}

chip Watch(t: exec) {
  var seen: int = 0
  on t { seen = seen + total }
}

let watcher = Watch(tick)

on tick {
  total = Scale(offset) + Scale(offset + 1)
  emit sum = total
}
"#;

struct Shape {
    grids: usize,
    root_bricks: usize,
    wires: usize,
    overlaps: usize,
    fan_in: usize,
}

fn build(prefix: &str) -> Shape {
    let source = format!("{prefix}{BODY}");
    let r = compile_to_world(
        CompileInput {
            source: &source,
            file: "flat_emit.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        wirescript::EmitOptions::default(),
    )
    .unwrap_or_else(|e| panic!("compile {prefix:?}: {e:?}"));

    let mut overlaps = 0;
    for bricks in std::iter::once(&r.world.bricks).chain(r.world.grids.iter().map(|(_, b)| b)) {
        overlaps += overlapping_pairs(bricks);
    }

    let mut seen: std::collections::HashMap<(usize, String, String), usize> =
        std::collections::HashMap::new();
    let mut fan_in = 0;
    for w in &r.world.wires {
        let key = (
            w.target.brick_id,
            w.target.component_type.to_string(),
            w.target.port_name.to_string(),
        );
        let n = seen.entry(key).or_insert(0);
        *n += 1;
        if *n == 2 {
            fan_in += 1;
        }
    }

    Shape {
        grids: r.world.grids.len(),
        root_bricks: r.world.bricks.len(),
        wires: r.world.wires.len(),
        overlaps,
        fan_in,
    }
}

/// Brick pairs whose world-space AABBs intersect. Same scan as the
/// `check_overlaps` example, which is the acceptance gate for this feature.
fn overlapping_pairs(bricks: &[brdb::Brick]) -> usize {
    let boxes: Vec<(brdb::Position, brdb::Position)> = bricks
        .iter()
        .map(|b| {
            let (lo, hi) = b.local_bounds();
            (b.position + lo, b.position + hi)
        })
        .collect();
    let mut n = 0;
    for i in 0..boxes.len() {
        for j in (i + 1)..boxes.len() {
            let (alo, ahi) = &boxes[i];
            let (blo, bhi) = &boxes[j];
            let separated = ahi.x <= blo.x
                || bhi.x <= alo.x
                || ahi.y <= blo.y
                || bhi.y <= alo.y
                || ahi.z <= blo.z
                || bhi.z <= alo.z;
            if !separated {
                n += 1;
            }
        }
    }
    n
}

/// `world.grids` holds the child brick grids. Without `@flat` this program
/// builds several; with it, the whole thing has to fit on the one grid the
/// root module already owns.
#[test]
fn flat_leaves_no_child_grid_under_any_layout() {
    let nested = build("");
    assert!(
        nested.grids > 1,
        "the fixture must build child grids without @flat, else this proves nothing (got {})",
        nested.grids
    );

    for prefix in [
        "@flat\n\n",
        "@flat\n@layout(\"cube\")\n\n",
        "@flat\n@layout(\"code\")\n\n",
    ] {
        let s = build(prefix);
        assert_eq!(
            s.grids, 1,
            "{prefix:?} left {} brick grids; @flat must collapse to the one root grid",
            s.grids
        );
        assert!(s.root_bricks > 0 && s.wires > 0, "{prefix:?} emitted nothing");
    }
}

/// The two failures the game reports as nothing at all.
#[test]
fn flat_emits_no_overlaps_and_no_fan_in_under_any_layout() {
    for prefix in [
        "",
        "@flat\n\n",
        "@flat\n@layout(\"cube\")\n\n",
        "@flat\n@layout(\"code\")\n\n",
    ] {
        let s = build(prefix);
        assert_eq!(s.overlaps, 0, "{prefix:?} emitted overlapping bricks");
        assert_eq!(s.fan_in, 0, "{prefix:?} emitted fan-in wire targets");
    }
}

/// A microchip brick is what links a parent grid to a child one, and every
/// such pairing is registered. With no child grids there must be exactly one
/// — the root shell every program is emitted inside.
#[test]
fn flat_emits_one_microchip_brick_for_the_root_shell_only() {
    let source = format!("@flat\n\n{BODY}");
    let r = compile_to_world(
        CompileInput {
            source: &source,
            file: "flat_emit.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        wirescript::EmitOptions::default(),
    )
    .expect("compile");

    assert_eq!(
        r.world.microchip_links.len(),
        1,
        "expected only the root shell's brick<->grid link"
    );

    // The root module's own bricks live in a grid, not in `world.bricks` —
    // that outer list holds only the shell the whole program sits inside. A
    // chip's microchip brick is pushed onto the bricks of the module that
    // instantiates it, so both lists have to be counted. Match the asset
    // exactly: the chip I/O gates are named `..._MicrochipInput`/`Output`
    // and are ordinary gate bricks, not links to anywhere.
    let microchips = std::iter::once(&r.world.bricks)
        .chain(r.world.grids.iter().map(|(_, b)| b))
        .flat_map(|bricks| bricks.iter())
        .filter(|b| b.asset == brdb::assets::bricks::B_MICROCHIP)
        .count();
    assert_eq!(
        microchips, 1,
        "expected only the root shell's microchip brick"
    );
}

/// Compiling the same source twice must give the identical shape — the merge
/// walks HashMaps, and an unsorted walk would hand out scope ids (and, before
/// that, node order) differently run to run.
#[test]
fn flat_is_deterministic() {
    let a = build("@flat\n\n");
    let b = build("@flat\n\n");
    assert_eq!(a.grids, b.grids);
    assert_eq!(a.root_bricks, b.root_bricks);
    assert_eq!(a.wires, b.wires);
}

/// A `for` loop's plane is an execution boundary, not a visual grouping:
/// flattening it onto the root grid would strand the plane-only
/// `CallableEntry_*` gate there, which the game rejects at load ("Brick not
/// allowed on this board"). `@flat` over any `for` must refuse (WS080)
/// instead of silently building that broken save.
const FLAT_OVER_A_LOOP: &str = "@flat\n\nvar t: int = 0\non ReadBrickGrid() {\n  for i in 0..5 { t = t + i }\n}\n";

/// `just check` (`diagnostics_only`) must report WS080 for this program.
#[test]
fn flat_over_a_loop_is_ws080_in_diagnostics_only() {
    let diags = diagnostics_only(CompileInput {
        source: FLAT_OVER_A_LOOP,
        file: "flat_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    });
    assert!(
        diags
            .iter()
            .any(|d| d.code == "WS080" && d.severity == Severity::Error),
        "expected an WS080 error, got {diags:?}"
    );
}

/// The refusal must actually stop the bad emit, not just add a warning next
/// to it: `compile_to_world` has to fail outright, before layout/emit ever
/// run, so no `.brdb`/`.brz` with the stranded gate can be produced.
#[test]
fn flat_over_a_loop_fails_compile_to_world() {
    let result = compile_to_world(
        CompileInput {
            source: FLAT_OVER_A_LOOP,
            file: "flat_emit.ws",
            module_name: None,
            fold_mode: FoldMode::Auto,
        },
        wirescript::EmitOptions::default(),
    );
    let Err(CompileError::HasErrors(diags)) = result else {
        panic!("`@flat` over a `for` loop must fail to compile with HasErrors, not silently emit");
    };
    assert!(
        diags.iter().any(|d| d.code == "WS080"),
        "expected WS080 among the compile errors, got {diags:?}"
    );
}

/// Control: the identical program with `@flat` removed compiles cleanly, so
/// WS080 is specifically about the `@flat` + `for` combination, not about the
/// loop itself.
#[test]
fn the_same_loop_without_flat_compiles_clean() {
    let source = FLAT_OVER_A_LOOP.replacen("@flat\n\n", "", 1);
    let diags = diagnostics_only(CompileInput {
        source: &source,
        file: "flat_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    });
    assert!(
        diags.iter().all(|d| d.severity != Severity::Error),
        "the loop alone (no @flat) must compile clean: {diags:?}"
    );
}

/// A loop nested inside a named chip still merges into the chip's own module
/// before the chip itself merges into the root, so refusing it once, on the
/// root, is what proves it isn't refused again on the way up.
fn ws080_count(source: &str) -> usize {
    diagnostics_only(CompileInput {
        source,
        file: "flat_emit.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .into_iter()
    .filter(|d| d.code == "WS080")
    .count()
}

#[test]
fn flat_over_a_loop_inside_a_named_chip_is_ws080_exactly_once() {
    let src = "@flat\n\nchip Looper(go: exec) {\n  var t: int = 0\n  \
               on go { for i in 0..3 { t = t + i } }\n}\n\n\
               in start: exec\nlet l = Looper(start)\n";
    assert_eq!(
        ws080_count(src),
        1,
        "one loop, nested one level under a named chip, must be refused once"
    );
}

/// Two instances of one chip build two distinct loop planes (template
/// instantiation re-stamps every node), so the refusal count must scale with
/// the number of planes, not with how many times each one merges upward.
#[test]
fn two_instances_of_a_chip_with_a_loop_are_ws080_twice() {
    let src = "@flat\n\nchip Looper(go: exec) {\n  var t: int = 0\n  \
               on go { for i in 0..3 { t = t + i } }\n}\n\n\
               in start: exec\nlet l1 = Looper(start)\nlet l2 = Looper(start)\n";
    assert_eq!(
        ws080_count(src),
        2,
        "two loop instances must be refused exactly twice, not four times"
    );
}
