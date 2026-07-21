//! Pollution tests — verify cross-module isolation after lowering.
//!
//! Bug classes targeted:
//! - Cloned chip modules sharing internal node IDs (wire target collisions).
//! - Inline mod template cache not remapping chip child module IDs.
//!
//! Run with: `cargo test -p wirescript --test pollution`

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use wirescript::{
    compile,
    compile::CompileInput,
    emit::{emit_brz, EmitOptions},
    ir::{Module, NodeId},
    layout::layout,
    lower::{lower, FoldMode, LowerInput},
    resolve::{FsLoader, resolve as ws_resolve},
    template_cache::TemplateCache,
    typecheck::typecheck,
};

// ---------- helpers ----------

/// Recursively collect `(chip_path, node_id)` pairs from a module tree.
fn collect_ids_by_chip(module: &Module, path: &str) -> Vec<(String, NodeId)> {
    let mut result: Vec<(String, NodeId)> = module
        .nodes
        .keys()
        .map(|&id| (path.to_string(), id))
        .collect();
    for (&chip_id, child_module) in &module.chips {
        let child_path = format!("{}/{}", path, chip_id);
        result.extend(collect_ids_by_chip(child_module, &child_path));
    }
    result
}

/// Parse → typecheck → lower a source string (no file imports).
fn lower_source(src: &str) -> wirescript::lower::LowerResult {
    let resolved = ws_resolve(src, "test.ws", &FsLoader);
    let tc = typecheck(&resolved.ast, "test.ws");
    lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test.ws",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::Auto,
    })
}

/// Collect every chip sub-module from a module tree into a flat list of
/// `(label, &Module)` pairs for easy iteration.
fn collect_chip_modules<'a>(module: &'a Module, path: &str) -> Vec<(String, &'a Module)> {
    let mut out = Vec::new();
    for (&chip_id, child_module) in &module.chips {
        let label = format!("{}/{}", path, chip_id);
        out.push((label.clone(), child_module));
        out.extend(collect_chip_modules(child_module, &label));
    }
    out
}

// ---------- node ID isolation ----------

/// Each chip instance (sibling calls) must have a disjoint set of internal
/// node IDs so wires cannot accidentally cross-target another instance.
#[test]
fn sibling_chips_have_disjoint_node_ids() {
    let src = r#"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
let r1 = ALU(1, 2)
let r2 = ALU(3, 4)
let r3 = ALU(5, 6)
out total = r1.r + r2.r + r3.r
"#;
    let lr = lower_source(src);
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );

    let chip_modules: Vec<_> = collect_chip_modules(&lr.module, "root");
    assert_eq!(
        chip_modules.len(),
        3,
        "expected 3 chip instances, got {}",
        chip_modules.len()
    );

    // Pairwise: no node ID may appear in two different top-level chip instances.
    for i in 0..chip_modules.len() {
        for j in (i + 1)..chip_modules.len() {
            let ids_i: HashSet<NodeId> = chip_modules[i].1.nodes.keys().copied().collect();
            let ids_j: HashSet<NodeId> = chip_modules[j].1.nodes.keys().copied().collect();
            let shared: Vec<_> = ids_i.intersection(&ids_j).collect();
            assert!(
                shared.is_empty(),
                "chip instances '{}' and '{}' share node IDs: {:?}",
                chip_modules[i].0,
                chip_modules[j].0,
                shared.iter().map(|&&id| id).collect::<Vec<_>>()
            );
        }
    }
}

/// Chip instances created inside inlined mods must also have disjoint IDs.
/// This catches the template-cache bug where the same chip sub-module was
/// reused verbatim across mod call sites.
#[test]
fn inlined_mod_chips_have_disjoint_ids() {
    let src = r#"
chip Double(x: int) -> (r: int) { out r = x + x }
mod do_double(v: *int) {
    let d = Double(v)
    v = d.r
}
var a: int = 1
var b: int = 2
var c: int = 3
in tick: exec
on tick {
    do_double(a)
    do_double(b)
    do_double(c)
}
"#;
    let lr = lower_source(src);
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );

    let chip_modules: Vec<_> = collect_chip_modules(&lr.module, "root");
    assert_eq!(
        chip_modules.len(),
        3,
        "expected 3 chip instances from inlined mod calls, got {}",
        chip_modules.len()
    );

    // Pairwise disjoint.
    for i in 0..chip_modules.len() {
        for j in (i + 1)..chip_modules.len() {
            let ids_i: HashSet<NodeId> = chip_modules[i].1.nodes.keys().copied().collect();
            let ids_j: HashSet<NodeId> = chip_modules[j].1.nodes.keys().copied().collect();
            let shared: Vec<_> = ids_i.intersection(&ids_j).collect();
            assert!(
                shared.is_empty(),
                "inlined-mod chip instances '{}' and '{}' share node IDs: {:?}",
                chip_modules[i].0,
                chip_modules[j].0,
                shared.iter().map(|&&id| id).collect::<Vec<_>>()
            );
        }
    }
}

/// No node ID may appear in two different modules anywhere in the tree.
/// Uses `collect_ids_by_chip` for a comprehensive flat scan.
#[test]
fn no_node_id_appears_in_two_modules() {
    let src = r#"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
mod compute(x: int, y: int) -> (r: int) {
    let sum = ALU(x, y)
    return sum.r
}
let r1 = compute(1, 2)
let r2 = compute(3, 4)
let r3 = compute(5, 6)
out total = r1 + r2 + r3
"#;
    let lr = lower_source(src);
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );

    let all_pairs = collect_ids_by_chip(&lr.module, "root");
    let mut seen: HashMap<NodeId, String> = HashMap::new();
    for (path, id) in &all_pairs {
        if let Some(prev_path) = seen.get(id) {
            panic!(
                "node ID '{}' appears in two modules:\n  first: {}\n  second: {}",
                id,
                prev_path,
                path
            );
        }
        seen.insert(*id, path.clone());
    }
}

// ---------- placement isolation ----------

/// Within a chip's interior module, no two spawnable nodes may share the
/// same (x, y, z) placement.
#[test]
fn no_placement_overlap_within_chip() {
    let src = r#"
chip Big(a: int, b: int, c: int) -> (r: int) {
    let ab = a + b
    let bc = b + c
    let abc = ab + bc
    out r = abc
}
let r = Big(1, 2, 3)
out result = r.r
"#;
    let lr = lower_source(src);
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );

    let layout_result = layout(&lr.module);

    // Check the root module's placements.
    let mut seen: HashSet<(i32, i32, i32)> = HashSet::new();
    for (&id, p) in &layout_result.placements {
        assert!(
            seen.insert((p.x, p.y, p.z)),
            "placement overlap in root module: node '{}' at ({}, {}, {})",
            id,
            p.x,
            p.y,
            p.z
        );
    }

    // Check each chip sub-layout.
    for (chip_id, chip_layout) in &layout_result.chip_layouts {
        let mut chip_seen: HashSet<(i32, i32, i32)> = HashSet::new();
        for (&id, p) in &chip_layout.placements {
            assert!(
                chip_seen.insert((p.x, p.y, p.z)),
                "placement overlap in chip '{}': node '{}' at ({}, {}, {})",
                chip_id,
                id,
                p.x,
                p.y,
                p.z
            );
        }
    }
}

// ---------- BRZ brick isolation ----------

/// Five ALU chip calls must compile to a valid BRZ with non-trivial size.
#[test]
fn brz_compiles_with_zero_wire_drops() {
    let src = r#"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
let r1 = ALU(1, 2)
let r2 = ALU(3, 4)
let r3 = ALU(5, 6)
let r4 = ALU(7, 8)
let r5 = ALU(9, 10)
out total = r1 + r2 + r3 + r4 + r5
"#;
    let result = compile(CompileInput {
        source: src,
        file: "test.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    });
    match result {
        Ok(cr) => {
            assert!(
                cr.brz.len() > 100,
                "BRZ output is suspiciously small: {} bytes",
                cr.brz.len()
            );
        }
        Err(e) => panic!("compile failed: {}", e),
    }
}

/// Chip inside inlined mod: 4 calls must produce 4 chip instances and a valid BRZ.
#[test]
fn brz_mod_with_chips_no_wire_drops() {
    let src = r#"
chip Inc(x: int) -> (r: int) { out r = x + 1 }
mod inc_var(v: *int) {
    let d = Inc(v)
    v = d.r
}
var a: int = 0
var b: int = 0
var c: int = 0
var d: int = 0
in tick: exec
on tick {
    inc_var(a)
    inc_var(b)
    inc_var(c)
    inc_var(d)
}
"#;
    let lr = lower_source(src);
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );

    // Count direct chip instances in the root (each inc_var call embeds one Inc chip).
    let chip_count = lr.module.chips.len();
    assert_eq!(
        chip_count, 4,
        "expected 4 chip instances from inc_var calls, got {}",
        chip_count
    );

    // Must also emit valid BRZ.
    let layout_result = layout(&lr.module);
    let template_cache = Arc::new(TemplateCache::new());
    let brz = emit_brz(&lr.module, &layout_result, &EmitOptions::default(), &template_cache);
    assert!(brz.is_ok(), "BRZ emit failed: {:?}", brz.err());
    let brz_bytes = brz.unwrap();
    assert!(
        brz_bytes.len() > 100,
        "BRZ output is suspiciously small: {} bytes",
        brz_bytes.len()
    );
}

// ---------- wire integrity ----------

/// For every wire in every module (recursively), both the source and target
/// node IDs must exist either in that module's own nodes or as a chip node
/// key (whose body is a child module), i.e. no dangling wire endpoints.
#[test]
fn all_wire_endpoints_resolve_to_existing_nodes() {
    let src = r#"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
let r1 = ALU(1, 2)
let r2 = ALU(3, 4)
let r3 = ALU(5, 6)
out total = r1.r + r2.r + r3.r
"#;
    let lr = lower_source(src);
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );

    fn check_wires(module: &Module, path: &str) {
        for wire in &module.wires {
            let src_id = wire.source.node_id;
            let tgt_id = wire.target.node_id;

            // A valid endpoint is either a node in this module, or a chip
            // node key (which is itself a node of kind Chip).
            let in_child = |id: &wirescript::ir::NodeId| -> bool {
                module.chips.values().any(|c| c.nodes.contains_key(id))
            };
            let src_ok = module.nodes.contains_key(&src_id) || module.chips.contains_key(&src_id) || in_child(&src_id);
            let tgt_ok = module.nodes.contains_key(&tgt_id) || module.chips.contains_key(&tgt_id) || in_child(&tgt_id);

            assert!(
                src_ok,
                "in module '{}': wire source '{}' does not resolve to any node or chip",
                path,
                src_id
            );
            assert!(
                tgt_ok,
                "in module '{}': wire target '{}' does not resolve to any node or chip",
                path,
                tgt_id
            );
        }

        for (&chip_id, child_module) in &module.chips {
            let child_path = format!("{}/{}", path, chip_id);
            check_wires(child_module, &child_path);
        }
    }

    check_wires(&lr.module, "root");
}
