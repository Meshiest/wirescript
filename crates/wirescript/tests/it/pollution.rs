//! Pollution tests: verify cross-module isolation after lowering.
//!
//! Bug classes targeted:
//! - Cloned chip modules sharing internal node IDs (wire target collisions).
//! - Inline mod template cache not remapping chip child module IDs.
//!
//! Run with: `cargo test -p wirescript --test it pollution::`

use std::collections::{HashMap, HashSet};

use wirescript::{
    FoldMode,
    ir::{Module, NodeId},
    layout::layout,
    resolve::FsLoader,
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
        let child_path = format!("{path}/{chip_id}");
        result.extend(collect_ids_by_chip(child_module, &child_path));
    }
    result
}

/// Lowering produced no hard error. A pollution test that starts from a broken
/// program proves nothing about isolation, so every one of them checks this
/// before looking at the module graph.
fn assert_no_errors(lr: &wirescript::LowerResult) {
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != wirescript::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        lr.diagnostics
    );
}

/// The front end over a source string (no file imports).
fn lower_source(src: &str) -> wirescript::LowerResult {
    // Node-ID isolation tests use constant chip args as minimal fixtures;
    // force folding off so the fold pass can't elide the chip instances.
    wirescript::front_end(src, "test.ws", None, FoldMode::ForceOff, &FsLoader).lowered
}

/// Every chip sub-module in a module tree, nested ones included.
fn chip_module_count(module: &Module) -> usize {
    module
        .chips
        .values()
        .map(|c| 1 + chip_module_count(c))
        .sum()
}

// ---------- node ID isolation ----------

/// Programs whose chip instances must not share a node id, each with the
/// chip-instance count it lowers to (`None` where the count is not the point).
/// Mods inline, so every counted instance sits directly in the root module.
const ISOLATION_CASES: &[(&str, &str, Option<usize>)] = &[
    (
        "sibling chip calls each get their own node ids",
        r"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
let r1 = ALU(1, 2)
let r2 = ALU(3, 4)
let r3 = ALU(5, 6)
out total = r1.r + r2.r + r3.r
",
        Some(3),
    ),
    (
        "chips inside inlined mods are not reused verbatim across call sites (template cache)",
        r"
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
",
        Some(3),
    ),
    (
        "a chip reached through a returning mod, called three times",
        r"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
mod compute(x: int, y: int) -> (r: int) {
    let sum = ALU(x, y)
    return sum.r
}
let r1 = compute(1, 2)
let r2 = compute(3, 4)
let r3 = compute(5, 6)
out total = r1 + r2 + r3
",
        None,
    ),
    (
        "a chip inside an inlined mod, one instance per call in the root module",
        r"
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
",
        Some(4),
    ),
];

/// No node ID may appear in two modules anywhere in the tree, so wires cannot
/// accidentally cross-target another chip instance.
#[test]
fn no_node_id_appears_in_two_modules() {
    let mut failures = Vec::new();
    for (label, src, chips) in ISOLATION_CASES {
        let lr = lower_source(src);
        assert_no_errors(&lr);
        if let Some(want) = chips {
            let (deep, root) = (chip_module_count(&lr.module), lr.module.chips.len());
            if (deep, root) != (*want, *want) {
                failures.push(format!("{label}: expected {want} chip instances in the root, got {root} ({deep} in the tree)"));
            }
        }
        let mut seen: HashMap<NodeId, String> = HashMap::new();
        for (path, id) in collect_ids_by_chip(&lr.module, "root") {
            if let Some(prev_path) = seen.insert(id, path.clone()) {
                failures.push(format!(
                    "{label}: node ID '{id}' appears in {prev_path} and {path}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------- placement isolation ----------

/// Within a chip's interior module, no two spawnable nodes may share the
/// same (x, y, z) placement.
#[test]
fn no_placement_overlap_within_chip() {
    let src = r"
chip Big(a: int, b: int, c: int) -> (r: int) {
    let ab = a + b
    let bc = b + c
    let abc = ab + bc
    out r = abc
}
let r = Big(1, 2, 3)
out result = r.r
";
    let lr = lower_source(src);
    assert_no_errors(&lr);

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

// ---------- wire integrity ----------

/// For every wire in every module (recursively), both the source and target
/// node IDs must exist either in that module's own nodes or as a chip node
/// key (whose body is a child module), i.e. no dangling wire endpoints.
#[test]
fn all_wire_endpoints_resolve_to_existing_nodes() {
    let src = r"
chip ALU(a: int, b: int) -> (r: int) { out r = a + b }
let r1 = ALU(1, 2)
let r2 = ALU(3, 4)
let r3 = ALU(5, 6)
out total = r1.r + r2.r + r3.r
";
    let lr = lower_source(src);
    assert_no_errors(&lr);

    fn check_wires(module: &Module, path: &str) {
        for wire in &module.wires {
            let src_id = wire.source.node_id;
            let tgt_id = wire.target.node_id;

            // A valid endpoint is either a node in this module, or a chip
            // node key (which is itself a node of kind Chip).
            let in_child = |id: &wirescript::ir::NodeId| -> bool {
                module.chips.values().any(|c| c.nodes.contains_key(id))
            };
            let src_ok = module.nodes.contains_key(&src_id)
                || module.chips.contains_key(&src_id)
                || in_child(&src_id);
            let tgt_ok = module.nodes.contains_key(&tgt_id)
                || module.chips.contains_key(&tgt_id)
                || in_child(&tgt_id);

            assert!(
                src_ok,
                "in module '{path}': wire source '{src_id}' does not resolve to any node or chip"
            );
            assert!(
                tgt_ok,
                "in module '{path}': wire target '{tgt_id}' does not resolve to any node or chip"
            );
        }

        for (&chip_id, child_module) in &module.chips {
            let child_path = format!("{path}/{chip_id}");
            check_wires(child_module, &child_path);
        }
    }

    check_wires(&lr.module, "root");
}
