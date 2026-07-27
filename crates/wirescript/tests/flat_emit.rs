//! End-to-end test for `@flat`: a real `compile_to_world()` call must produce
//! a world with no child brick grid and no microchip brick, under every
//! layout engine, and the resulting single grid must still satisfy the two
//! invariants the game enforces silently — no overlapping bricks (the game
//! DROPS them at load) and no two wires into one target port (the game
//! rejects the wire at load).

use wirescript::compile::{CompileInput, FoldMode, compile_to_world};

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
