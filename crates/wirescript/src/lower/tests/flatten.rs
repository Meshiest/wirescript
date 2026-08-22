//! `@flat` — the chip tree is inlined onto one module before boundary pins.

use crate::ir::{Module, NodeId, NodeKind, gate_class as gc};
use crate::template_cache::TemplateCache;

use super::*;

/// Lower `src` under the production `Auto` fold mode, so a module-level
/// `@flat` in `src` reaches the pipeline exactly as it would in a real
/// compile.
fn lowered(src: &str) -> LowerResult {
    let parsed = crate::parser::parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = crate::typecheck::typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    let r = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: std::sync::Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
        fold_mode: FoldMode::Auto,
        ce_slots: &crate::typecheck::CeSlotMap::default(),
    });
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    r
}

fn total_nodes(m: &Module) -> usize {
    m.nodes.len() + m.chips.values().map(total_nodes).sum::<usize>()
}

fn total_wires(m: &Module) -> usize {
    m.wires.len() + m.chips.values().map(total_wires).sum::<usize>()
}

fn total_chips(m: &Module) -> usize {
    m.chips.len() + m.chips.values().map(total_chips).sum::<usize>()
}

/// Every gate class present anywhere in the tree, with its count.
fn class_census(m: &Module) -> std::collections::BTreeMap<&'static str, usize> {
    let mut out = std::collections::BTreeMap::new();
    fn walk(m: &Module, out: &mut std::collections::BTreeMap<&'static str, usize>) {
        for n in m.nodes.values() {
            *out.entry(n.gate_class).or_insert(0) += 1;
        }
        for c in m.chips.values() {
            walk(c, out);
        }
    }
    walk(m, &mut out);
    out
}

/// Two named-chip instances plus a nested chip and an anonymous chip — the
/// shapes that put nodes in a child module or tag them for one.
const NESTED: &str = "\
chip Inner(a: int) -> (r: int) {
  return a * 2
}
chip Outer(x: int) -> (y: int) {
  let m = Inner(x)
  return m + 1
}

var seed: int = 3
in go: exec
out total: int

chip {
  var scratch: int = 0
}

on go {
  emit total = Outer(seed) + Outer(seed + 1)
}
";

fn flat(src: &str) -> String {
    format!("@flat\n\n{src}")
}

#[test]
fn flat_collapses_the_whole_chip_tree_into_one_module() {
    let plain = lowered(NESTED);
    let flattened = lowered(&flat(NESTED));

    assert!(
        total_chips(&plain.module) > 0,
        "the fixture must actually build child modules, else this proves nothing"
    );
    assert_eq!(
        flattened.module.chips.len(),
        0,
        "@flat must leave no child module"
    );
    assert_eq!(total_chips(&flattened.module), 0);
}

#[test]
fn flat_emits_no_microchip_node() {
    let flattened = lowered(&flat(NESTED));
    let chips: Vec<NodeId> = flattened
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.kind == NodeKind::Chip || n.gate_class == gc::MICROCHIP)
        .map(|(id, _)| *id)
        .collect();
    assert!(chips.is_empty(), "microchip nodes survived: {chips:?}");
}

/// Flattening must MOVE the work, not drop it: every computing gate the
/// nested build produced is still there afterwards. The chip shells go, and
/// the three rerouter classes are excluded because the nested build's
/// boundary-pin pass adds pins the flat build has no walls to need.
#[test]
fn flat_keeps_every_gate_the_nested_build_had() {
    for src in [NESTED, DEEP_CHAIN] {
        let plain = lowered(src);
        let flattened = lowered(&flat(src));

        let strip = |m: &Module| {
            let mut c = class_census(m);
            for k in [
                gc::MICROCHIP,
                gc::MICROCHIP_ALT,
                gc::MICROCHIP_INPUT,
                gc::MICROCHIP_OUTPUT,
            ] {
                c.remove(k);
            }
            c
        };
        assert_eq!(
            strip(&flattened.module),
            strip(&plain.module),
            "gate census changed under @flat"
        );
        assert!(
            chip_node_count(&plain.module) > 0,
            "fixture built no microchip node, so its removal proves nothing"
        );
        assert_eq!(chip_node_count(&flattened.module), 0);
    }
}

/// `NodeKind::Chip` nodes anywhere in the tree.
fn chip_node_count(m: &Module) -> usize {
    m.nodes.values().filter(|n| n.kind == NodeKind::Chip).count()
        + m.chips.values().map(chip_node_count).sum::<usize>()
}

/// The anon-chip partition is skipped under `@flat`, so no node keeps a
/// `chip_id` pointing at a shell that no longer exists.
#[test]
fn flat_clears_every_chip_id_tag() {
    let flattened = lowered(&flat(NESTED));
    let tagged: Vec<NodeId> = flattened
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.chip_id.is_some())
        .map(|(id, _)| *id)
        .collect();
    assert!(tagged.is_empty(), "chip_id tags survived: {tagged:?}");
}

/// `partition_anon_chips` inserts synthetic `WirePort::Layout` edges to keep
/// partitioned chips inline in the DAG. Flat never partitions, so none exist
/// — and any that a chip shell was an endpoint of are gone with it.
#[test]
fn flat_leaves_no_layout_edges() {
    let flattened = lowered(&flat(NESTED));
    let layout_edges = flattened
        .module
        .wires
        .iter()
        .filter(|w| {
            w.source.port == crate::ir::port_registry::WirePort::Layout
                || w.target.port == crate::ir::port_registry::WirePort::Layout
        })
        .count();
    assert_eq!(layout_edges, 0);
}

/// Every wire endpoint must name a node that exists in the one module. A
/// dangling endpoint is exactly the silent miscompile flattening could
/// cause, and nothing downstream reports it.
#[test]
fn flat_leaves_no_wire_pointing_outside_the_module() {
    for src in [NESTED, DEEP_CHAIN] {
        let flattened = lowered(&flat(src));
        let m = &flattened.module;
        for w in &m.wires {
            assert!(
                m.nodes.contains_key(&w.source.node_id),
                "wire source {} is not a node in the flattened module",
                w.source.node_id
            );
            assert!(
                m.nodes.contains_key(&w.target.node_id),
                "wire target {} is not a node in the flattened module",
                w.target.node_id
            );
        }
        assert!(
            m.scope_captures.is_empty(),
            "a single module captures nothing: {:?}",
            m.scope_captures
        );
    }
}

/// With the tree already flat, the boundary pin pass has nothing to do.
/// Asserted, not assumed — if it ever did fire it would add
/// MicrochipInput/Output rerouters for wires that no longer cross anything.
#[test]
fn boundary_pins_is_a_no_op_on_a_flattened_module() {
    let flattened = lowered(&flat(NESTED));
    assert!(
        flattened.module.chips.is_empty(),
        "precondition: the module under test must actually be flat"
    );
    let mut after = flattened.module.clone();
    crate::lower::boundary_pins::synthesize_boundary_pins(&mut after);

    assert_eq!(after.nodes.len(), flattened.module.nodes.len());
    assert_eq!(after.wires.len(), flattened.module.wires.len());
    assert!(
        after.nodes.values().all(|n| n.note != Some("boundary_pin")),
        "boundary pins were synthesized on an already-flat module"
    );
}

/// Chips three deep, so the merge has to run more than one level and the
/// deepest body still lands in the root.
const DEEP_CHAIN: &str = "\
chip L3(a: int) -> (r: int) { return a + 3 }
chip L2(a: int) -> (r: int) { return L3(a) + 2 }
chip L1(a: int) -> (r: int) { return L2(a) + 1 }

var seed: int = 0
in go: exec
out total: int

on go {
  emit total = L1(seed)
}
";

#[test]
fn flat_reaches_the_deepest_chip() {
    let plain = lowered(DEEP_CHAIN);
    assert!(
        total_chips(&plain.module) >= 3,
        "fixture must nest at least three deep, got {}",
        total_chips(&plain.module)
    );
    let flattened = lowered(&flat(DEEP_CHAIN));
    assert_eq!(total_chips(&flattened.module), 0);
    // The deepest body's own gates are in the one module now.
    assert!(
        total_nodes(&flattened.module) >= total_nodes(&plain.module) - chip_node_count(&plain.module),
        "nodes went missing on the way up"
    );
    assert!(total_wires(&flattened.module) > 0);
}

/// Two instances of one chip come from the same template and carry the same
/// scope ids. Merging both into one module must not let the second overwrite
/// the first's scope table entries, or the two bodies would be reported as
/// one region.
#[test]
fn two_instances_of_one_chip_get_distinct_scopes() {
    let src = "\
chip Twice(a: int) -> (r: int) {
  let doubled = a * 2
  return doubled
}

var seed: int = 1
in go: exec
out total: int

on go {
  emit total = Twice(seed) + Twice(seed + 1)
}
";
    let flattened = lowered(&flat(src));
    let m = &flattened.module;

    let bodies = m
        .scopes
        .values()
        .filter(|s| matches!(&s.kind, crate::ir::ScopeKind::ChipBody { name } if name == "Twice"))
        .count();
    assert_eq!(bodies, 2, "one ChipBody scope per instance; scopes: {:?}", m.scopes);

    // Every node's scope must resolve in this module's table — an id left
    // over from a child module would silently re-home the node onto the
    // root region.
    for n in m.nodes.values() {
        assert!(
            m.scopes.contains_key(&n.scope_id),
            "node {} references scope {} which is not in the merged table",
            n.id,
            n.scope_id
        );
    }
}

/// A merged chip body's `ChipBody` scope hangs off the scope the chip node
/// sat in, so the region tree still shows it nested where the chip was.
#[test]
fn merged_chip_body_scopes_stay_parented() {
    let flattened = lowered(&flat(NESTED));
    let m = &flattened.module;
    let bodies: Vec<_> = m
        .scopes
        .iter()
        .filter(|(_, s)| matches!(s.kind, crate::ir::ScopeKind::ChipBody { .. }))
        .collect();
    assert!(!bodies.is_empty(), "no chip bodies merged");
    for (id, info) in bodies {
        let parent = info
            .parent
            .unwrap_or_else(|| panic!("merged chip body {id} kept parent: None"));
        assert!(
            m.scopes.contains_key(&parent),
            "merged chip body {id} parented onto missing scope {parent}"
        );
    }
}

/// Declared chip `in`/`out` pins are kept as ordinary rerouters, but must
/// NOT join the host's `inputs`/`outputs` — those drive the outer rerouter
/// bricks for the program's own top-level ports.
#[test]
fn merged_pins_stay_out_of_the_hosts_port_lists() {
    let plain = lowered(NESTED);
    let flattened = lowered(&flat(NESTED));
    assert_eq!(
        flattened.module.inputs.len(),
        plain.module.inputs.len(),
        "top-level input ports changed"
    );
    assert_eq!(
        flattened.module.outputs.len(),
        plain.module.outputs.len(),
        "top-level output ports changed"
    );

    // The chip pins are still present as nodes, just unlisted.
    let pin_nodes = flattened
        .module
        .nodes
        .values()
        .filter(|n| matches!(n.kind, NodeKind::Input | NodeKind::Output))
        .count();
    assert!(
        pin_nodes > flattened.module.inputs.len() + flattened.module.outputs.len(),
        "merged chip pins were dropped, not kept"
    );
}

/// Ordinary lowering wires straight to a chip's pin nodes, so the
/// `(chip node, port label)` form never survives to be dropped.
#[test]
fn flat_drops_no_chip_port_wire_on_real_programs() {
    for src in [NESTED, DEEP_CHAIN] {
        let parsed = crate::parser::parse(&flat(src), "test");
        let tc = crate::typecheck::typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
        let mut r = lower(LowerInput {
            ast: &crate::ast::Script {
                flat: false,
                ..parsed.ast.clone()
            },
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: std::sync::Arc::new(TemplateCache::new()),
            doc_comments: &parsed.doc_comments,
            fold_mode: FoldMode::Auto,
            ce_slots: &crate::typecheck::CeSlotMap::default(),
        });
        assert!(!r.module.chips.is_empty(), "nothing to flatten");
        let stats = crate::lower::flatten::flatten_chips(&mut r.module);
        assert!(r.module.chips.is_empty());
        assert_eq!(
            stats.dropped_chip_port_wires, 0,
            "flatten dropped a chip-port wire it could not resolve"
        );
    }
}

/// `@flat` composes with `@layout(...)` and with `@fold` — it is a separate
/// switch, not part of the layout choice.
#[test]
fn flat_composes_with_the_other_module_annotations() {
    for prefix in [
        "@flat\n",
        "@flat\n@layout(\"cube\")\n",
        "@flat\n@layout(\"code\")\n",
        "@layout(\"code\")\n@flat\n",
        "@fold\n@flat\n",
    ] {
        let src = format!("{prefix}\n{NESTED}");
        let parsed = crate::parser::parse(&src, "test");
        assert!(parsed.ast.flat, "{prefix:?} did not set flat");
        let r = lowered(&src);
        assert_eq!(r.module.chips.len(), 0, "{prefix:?} left a child module");
    }
}

/// Without `@flat` nothing changes — the chip tree is still built.
#[test]
fn omitting_flat_keeps_the_chip_tree() {
    let plain = lowered(NESTED);
    assert!(plain.module.chips.len() > 0);
}
