//! Snapshot tests for the block-aware layout pass.
//!
//! Each fixture `<name>.ws` in `tests/fixtures/` is compiled through
//! parse → typecheck → lower → layout, then dumped to a stable text
//! format and compared against `<name>.layout.snap`. Set `BLESS=1`
//! when running `cargo test` to regenerate the goldens after a
//! deliberate change.
//!
//! The dump format lists `bounds`, then each placement as
//! `id x y z kind scope` sorted by node id. Nested chip layouts are
//! indented under a `chip <node_id>` header.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wirescript::ir::{Module, NodeId};
use wirescript::lower::{LowerInput, lower};
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
    let tc = typecheck(&parsed.ast, name);
    assert!(
        tc.diagnostics.is_empty(),
        "typecheck diagnostics in {}: {:?}",
        name,
        tc.diagnostics
    );
    let lr = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: name,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
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
                n.note.map(|s| s.to_string()).unwrap_or_else(|| {
                    n.gate_class.to_string()
                })
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

fn check_snapshot(name: &str) {
    let dir = fixtures_dir();
    let ws_path = dir.join(format!("{}.ws", name));
    let snap_path = dir.join(format!("{}.layout.snap", name));

    let src = fs::read_to_string(&ws_path)
        .unwrap_or_else(|e| panic!("read {}: {}", ws_path.display(), e));
    let module = compile_to_module(&src, name);
    let layout_out = layout(&module);
    let actual = dump_layout(&layout_out, &module, 0);

    if std::env::var("BLESS").is_ok() {
        fs::write(&snap_path, &actual).unwrap();
        eprintln!("blessed {}", snap_path.display());
        return;
    }

    let expected = match fs::read_to_string(&snap_path) {
        Ok(s) => s,
        Err(_) => {
            panic!(
                "missing snapshot {}; run `BLESS=1 cargo test -p wirescript \
                 --test layout_snapshots {}` to create it",
                snap_path.display(),
                name
            );
        }
    };

    if expected != actual {
        eprintln!("=== EXPECTED ===\n{}", expected);
        eprintln!("=== ACTUAL ===\n{}", actual);
        panic!(
            "snapshot mismatch for {}: run `BLESS=1 cargo test ...` to update",
            name
        );
    }
}

/// Guard against accidental layout non-determinism: run layout twice
/// and assert the dump is byte-identical.
fn check_determinism(name: &str) {
    let src = fs::read_to_string(fixtures_dir().join(format!("{}.ws", name))).unwrap();
    let m = compile_to_module(&src, name);
    let a = dump_layout(&layout(&m), &m, 0);
    let b = dump_layout(&layout(&m), &m, 0);
    assert_eq!(a, b, "layout of {} is non-deterministic", name);
}

/// No two placements may share the same (x, y, z).
fn check_no_overlap(name: &str) {
    use std::collections::HashSet;
    let src = fs::read_to_string(fixtures_dir().join(format!("{}.ws", name))).unwrap();
    let m = compile_to_module(&src, name);
    let l = layout(&m);
    walk_no_overlap(&l, name);

    fn walk_no_overlap(l: &LayoutResult, name: &str) {
        let mut seen: HashSet<(i32, i32, i32)> = HashSet::new();
        for (id, p) in &l.placements {
            assert!(
                seen.insert((p.x, p.y, p.z)),
                "overlap in {}: node {:?} at ({}, {}, {})",
                name,
                id,
                p.x,
                p.y,
                p.z
            );
        }
        for sub in l.chip_layouts.values() {
            walk_no_overlap(sub, name);
        }
    }
}

fn _assert_path_exists(p: &Path) {
    assert!(p.exists(), "missing: {}", p.display());
}

macro_rules! fixture_tests {
    ($($name:ident),* $(,)?) => {
        $(
            mod $name {
                #[test] fn snapshot() { super::check_snapshot(stringify!($name)); }
                #[test] fn deterministic() { super::check_determinism(stringify!($name)); }
                #[test] fn no_overlap() { super::check_no_overlap(stringify!($name)); }
            }
        )*
    }
}

fixture_tests!(if_handler, buffer_loop, nested_ifs, own_and_child);
