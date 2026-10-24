//! Snapshot tests for the block-aware layout pass.
//!
//! Each fixture `<name>.ws` in `tests/fixtures/` is compiled through
//! parse -> typecheck -> lower -> layout, then dumped to a stable text
//! format and compared against `<name>.layout.snap`. Set `BLESS=1`
//! when running `cargo test` to regenerate the goldens after a
//! deliberate change. The same compile also checks layout determinism
//! and that no two placements overlap.
//!
//! The dump format lists `bounds`, then each placement as
//! `id x y z kind scope` sorted by node id. Nested chip layouts are
//! indented under a `chip <node_id>` header.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use wirescript::ir::{Module, NodeId};
use wirescript::lower::{FoldMode, LowerInput, lower};
use wirescript::parser::parse;
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::typecheck;
use wirescript::{LayoutResult, layout};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn compile_to_module(src: &str, name: &str) -> Module {
    let parsed = parse(src, name);
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diagnostics in {}: {:?}",
        name,
        parsed.diagnostics
    );
    let tc = typecheck(
        &parsed.ast,
        name,
        &wirescript::typecheck::CeSlotMap::default(),
    );
    assert!(
        tc.diagnostics.is_empty(),
        "typecheck diagnostics in {}: {:?}",
        name,
        tc.diagnostics
    );
    // Unfolded: goldens under tests/fixtures/*.layout.snap predate the
    // certified fold pass and encode the pre-fold gate shape.
    let lr = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        resolutions: &tc.resolutions,
        file: name,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
        fold_mode: FoldMode::ForceOff,
        ce_slots: &wirescript::typecheck::CeSlotMap::default(),
    });
    lr.module
}

fn dump_layout(l: &LayoutResult, m: &Module, indent: usize) -> String {
    let pad = "  ".repeat(indent);
    let mut out = String::new();
    out.push_str(&format!(
        "{}bounds: min=({},{},{}) max=({},{},{})\n",
        pad,
        l.bounds_min.x,
        l.bounds_min.y,
        l.bounds_min.z,
        l.bounds_max.x,
        l.bounds_max.y,
        l.bounds_max.z,
    ));
    // Use the node's debug note (deterministic path) for display, not the
    // numeric NodeId (which changes across runs).
    let node_label = |id: &NodeId| -> String {
        m.nodes
            .get(id)
            .map(|n| {
                n.note
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| n.gate_class.to_string())
            })
            .unwrap_or_else(|| "?".to_string())
    };
    let mut ids: Vec<&NodeId> = l.placements.keys().collect();
    ids.sort_by(|a, b| {
        let la = node_label(a);
        let lb = node_label(b);
        la.cmp(&lb).then_with(|| {
            let pa = &l.placements[a];
            let pb = &l.placements[b];
            (pa.x, pa.y, pa.z).cmp(&(pb.x, pb.y, pb.z))
        })
    });
    for id in ids {
        let p = &l.placements[id];
        let kind = m
            .nodes
            .get(id)
            .map(|n| format!("{:?}", n.kind))
            .unwrap_or_else(|| "?".into());
        let scope = m
            .nodes
            .get(id)
            .map(|n| format!("s{}", n.scope_id))
            .unwrap_or_else(|| "s?".into());
        out.push_str(&format!(
            "{}  {:<40} ({:>5},{:>5},{:>3}) {:<10} {}\n",
            pad,
            node_label(id),
            p.x,
            p.y,
            p.z,
            kind,
            scope,
        ));
    }
    let mut chip_ids: Vec<&NodeId> = l.chip_layouts.keys().collect();
    chip_ids.sort_by_key(|id| node_label(id));
    for chip_id in chip_ids {
        out.push_str(&format!("{}chip {}:\n", pad, node_label(chip_id)));
        if let Some(chip_mod) = m.chips.get(chip_id) {
            out.push_str(&dump_layout(&l.chip_layouts[chip_id], chip_mod, indent + 1));
        }
    }
    out
}

/// Compile fixture `name` once and check its layout: it matches the golden
/// `<name>.layout.snap` (rewritten instead when `BLESS` is set), a second
/// layout of the same module dumps byte-identically, and no two placements
/// share an (x, y, z). Every failing check is reported.
fn check_fixture(name: &str) {
    let dir = fixtures_dir();
    let ws_path = dir.join(format!("{}.ws", name));
    let snap_path = dir.join(format!("{}.layout.snap", name));

    let src = fs::read_to_string(&ws_path)
        .unwrap_or_else(|e| panic!("read {}: {}", ws_path.display(), e));
    let module = compile_to_module(&src, name);
    let layout_out = layout(&module);
    let actual = dump_layout(&layout_out, &module, 0);
    let mut failures = Vec::new();

    if std::env::var("BLESS").is_ok() {
        fs::write(&snap_path, &actual).unwrap();
        eprintln!("blessed {}", snap_path.display());
    } else {
        match fs::read_to_string(&snap_path) {
            Ok(expected) if expected == actual => {}
            Ok(expected) => {
                eprintln!("=== EXPECTED ===\n{}", expected);
                eprintln!("=== ACTUAL ===\n{}", actual);
                failures.push(format!(
                    "snapshot mismatch: run `BLESS=1 cargo test -p wirescript --test it layout_snapshots::{name}` to update"
                ));
            }
            Err(_) => failures.push(format!(
                "missing snapshot {}; run `BLESS=1 cargo test -p wirescript --test it layout_snapshots::{name}` to create it",
                snap_path.display()
            )),
        }
    }

    if dump_layout(&layout(&module), &module, 0) != actual {
        failures.push("layout is non-deterministic".to_string());
    }

    fn overlaps(l: &LayoutResult, out: &mut Vec<String>) {
        let mut seen: HashSet<(i32, i32, i32)> = HashSet::new();
        for (id, p) in &l.placements {
            if !seen.insert((p.x, p.y, p.z)) {
                out.push(format!(
                    "overlap: node {:?} at ({}, {}, {})",
                    id, p.x, p.y, p.z
                ));
            }
        }
        for sub in l.chip_layouts.values() {
            overlaps(sub, out);
        }
    }
    overlaps(&layout_out, &mut failures);

    assert!(failures.is_empty(), "{name}:\n{}", failures.join("\n"));
}

macro_rules! fixture_tests {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                check_fixture(stringify!($name));
            }
        )*
    }
}

fixture_tests!(if_handler, buffer_loop, nested_ifs, own_and_child);
