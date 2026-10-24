//! `@flat` — the chip tree is inlined onto one module before boundary pins.

use crate::ir::{Module, NodeId, NodeKind, gate_class as gc};

use super::*;

fn total_nodes(m: &Module) -> usize {
    m.nodes_deep().count()
}

fn total_wires(m: &Module) -> usize {
    m.wires_deep().count()
}

/// Chip modules anywhere below `m`.
fn total_chips(m: &Module) -> usize {
    m.modules_deep().len() - 1
}

/// Every gate class present anywhere in the tree, with its count.
fn class_census(m: &Module) -> std::collections::BTreeMap<&'static str, usize> {
    let mut out = std::collections::BTreeMap::new();
    for n in m.nodes_deep() {
        *out.entry(n.gate_class).or_insert(0) += 1;
    }
    out
}

// Two named-chip instances plus a nested chip and an anonymous chip — the
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

/// Flattening must MOVE the work, not drop it: every computing gate the
/// nested build produced is still there afterwards. The chip shells go, and
/// the three rerouter classes are excluded because the nested build's
/// boundary-pin pass adds pins the flat build has no walls to need.
#[test]
fn flat_keeps_every_gate_the_nested_build_had() {
    for src in [NESTED, DEEP_CHAIN] {
        let plain = compile_ok_auto(src);
        let flattened = compile_ok_auto(&flat(src));

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
        let shells: Vec<NodeId> = flattened
            .module
            .nodes_deep()
            .filter(|n| n.gate_class == gc::MICROCHIP)
            .map(|n| n.id)
            .collect();
        assert!(shells.is_empty(), "microchip nodes survived: {shells:?}");
    }
}

/// `NodeKind::Chip` nodes anywhere in the tree.
fn chip_node_count(m: &Module) -> usize {
    m.nodes_deep().filter(|n| n.kind == NodeKind::Chip).count()
}

/// The anon-chip partition is skipped under `@flat`, so no node keeps a
/// `chip_id` pointing at a shell that no longer exists.
#[test]
fn flat_clears_every_chip_id_tag() {
    let flattened = compile_ok_auto(&flat(NESTED));
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
    let flattened = compile_ok_auto(&flat(NESTED));
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
        let flattened = compile_ok_auto(&flat(src));
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
    let flattened = compile_ok_auto(&flat(NESTED));
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
    let plain = compile_ok_auto(DEEP_CHAIN);
    assert!(
        total_chips(&plain.module) >= 3,
        "fixture must nest at least three deep, got {}",
        total_chips(&plain.module)
    );
    let flattened = compile_ok_auto(&flat(DEEP_CHAIN));
    assert_eq!(total_chips(&flattened.module), 0);
    // The deepest body's own gates are in the one module now.
    assert!(
        total_nodes(&flattened.module)
            >= total_nodes(&plain.module) - chip_node_count(&plain.module),
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
    let flattened = compile_ok_auto(&flat(src));
    let m = &flattened.module;

    let bodies = m
        .scopes
        .values()
        .filter(|s| matches!(&s.kind, crate::ir::ScopeKind::ChipBody { name } if name == "Twice"))
        .count();
    assert_eq!(
        bodies, 2,
        "one ChipBody scope per instance; scopes: {:?}",
        m.scopes
    );

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
    let flattened = compile_ok_auto(&flat(NESTED));
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
    let plain = compile_ok_auto(NESTED);
    let flattened = compile_ok_auto(&flat(NESTED));
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
        // The program without `@flat`: its chip tree, for `flatten_chips` to flatten.
        let mut r =
            crate::compile::front_end(src, "test", None, FoldMode::Auto, &crate::resolve::FsLoader)
                .lowered;
        assert!(!r.module.chips.is_empty(), "nothing to flatten");
        let mut diags = Vec::new();
        let stats = crate::lower::flatten::flatten_chips(&mut r.module, &mut diags);
        assert!(r.module.chips.is_empty());
        assert!(diags.is_empty(), "no loop plane here to refuse: {diags:?}");
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
        let r = compile_ok_auto(&src);
        assert_eq!(r.module.chips.len(), 0, "{prefix:?} left a child module");
    }
}
