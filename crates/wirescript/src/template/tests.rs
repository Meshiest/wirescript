use std::sync::Arc;

use super::*;
use crate::collections::HashSet;
use crate::ir::port_registry::WirePort;
use crate::ir::{GateIO, NodeKind, PortRef, PortSpec, Type, Wire};

const MC_INPUT: &str = "BrickComponentType_Internal_MicrochipInput";
const MC_OUTPUT: &str = "BrickComponentType_Internal_MicrochipOutput";
const MC_CHIP: &str = "Component_Internal_Microchip";
const ADD_GATE: &str = "BrickComponentType_Expr_MathAdd";

/// A `Node` for a hand-built test module: no properties, no chain, root
/// scope, default ports. Anything a test actually varies is written as a
/// struct update over this, so a new `Node` field lands here once.
fn test_node(id: NodeId, kind: NodeKind, gate_class: &'static str) -> Node {
    Node {
        id,
        kind,
        gate_class,
        properties: Arc::new(HashMap::default()),
        ports: Arc::new(GateIO::default()),
        source_range: Default::default(),
        chip_id: None,
        chain_id: None,
        scope_id: 0,
        note: None,
    }
}

/// A wire from `a`'s port `pa` to `b`'s port `pb`.
fn wire(a: NodeId, pa: WirePort, b: NodeId, pb: WirePort) -> Wire {
    Wire {
        source: PortRef {
            node_id: a,
            port: pa,
        },
        target: PortRef {
            node_id: b,
            port: pb,
        },
    }
}

/// Rerouter ports: one `RER_Input`, one `RER_Output`, both int.
fn rerouter_ports() -> Arc<GateIO> {
    use crate::intern::sym;
    Arc::new(GateIO {
        inputs: vec![PortSpec::new(*sym::RER_INPUT, Type::Int)],
        outputs: vec![PortSpec::new(*sym::RER_OUTPUT, Type::Int)],
    })
}

/// Build a minimal module:
///
/// ```text
///  in_a (MicrochipInput)  ─┐
///                          ├-> add (Gate) -> out_r (MicrochipOutput)
///  in_b (MicrochipInput)  ─┘
/// ```
fn simple_add_module() -> Module {
    use crate::intern::sym;
    let mut m = Module::new("simple_add");

    let in_a_id = NodeId::fresh();
    let in_b_id = NodeId::fresh();
    let add_id = NodeId::fresh();
    let out_r_id = NodeId::fresh();

    let in_a = Node {
        ports: rerouter_ports(),
        ..test_node(in_a_id, NodeKind::Input, MC_INPUT)
    };
    let in_b = Node {
        ports: rerouter_ports(),
        ..test_node(in_b_id, NodeKind::Input, MC_INPUT)
    };
    let add = Node {
        ports: Arc::new(GateIO {
            inputs: vec![PortSpec::new(*sym::INPUT_A, Type::Int)],
            outputs: vec![
                PortSpec::new(*sym::INPUT_B, Type::Int),
                PortSpec::new(*sym::OUTPUT, Type::Int),
            ],
        }),
        ..test_node(add_id, NodeKind::Gate, ADD_GATE)
    };
    let out_r = Node {
        ports: rerouter_ports(),
        ..test_node(out_r_id, NodeKind::Output, MC_OUTPUT)
    };

    m.nodes.insert(in_a_id, in_a);
    m.nodes.insert(in_b_id, in_b);
    m.nodes.insert(add_id, add);
    m.nodes.insert(out_r_id, out_r);

    m.inputs.push(in_a_id);
    m.inputs.push(in_b_id);
    m.outputs.push(out_r_id);

    m.wires
        .push(wire(in_a_id, WirePort::RerOutput, add_id, WirePort::InputA));
    m.wires
        .push(wire(in_b_id, WirePort::RerOutput, add_id, WirePort::InputB));
    m.wires
        .push(wire(add_id, WirePort::Output, out_r_id, WirePort::RerInput));

    m
}

#[test]
fn template_from_module_preserves_structure() {
    let m = simple_add_module();
    let node_count = m.nodes.len();
    let wire_count = m.wires.len();
    let input_count = m.inputs.len();
    let output_count = m.outputs.len();

    let t = CompiledTemplate::from_module(m);
    assert_eq!(t.module.nodes.len(), node_count);
    assert_eq!(t.module.wires.len(), wire_count);
    assert_eq!(t.module.inputs.len(), input_count);
    assert_eq!(t.module.outputs.len(), output_count);
    assert!(t.bounds.is_none());
    assert!(t.placements.is_none());
    assert!(t.external_refs.is_empty());
}

#[test]
fn instantiate_preserves_wire_integrity() {
    let t = CompiledTemplate::from_module(simple_add_module());
    let inst = t.instantiate("check", &HashMap::default());

    for wire in &inst.wires {
        assert!(
            inst.nodes.contains_key(&wire.source.node_id),
            "wire source {} not found in nodes",
            wire.source.node_id
        );
        assert!(
            inst.nodes.contains_key(&wire.target.node_id),
            "wire target {} not found in nodes",
            wire.target.node_id
        );
    }
}

#[test]
fn instantiate_captures_not_remapped_as_internal() {
    let m = simple_add_module();

    // Designate the first input as an external ref.
    let in_a_id = m.inputs[0];
    let mut t = CompiledTemplate::from_module(m);
    t.external_refs.push(("in_a".to_string(), in_a_id));

    // Remove in_a from the internal node map so it is treated as purely external.
    t.module.nodes.remove(&in_a_id);

    let caller_id = NodeId::fresh();
    let mut captures = HashMap::default();
    captures.insert("in_a".to_string(), caller_id);

    let inst = t.instantiate("ext_test", &captures);

    // Internal nodes should have fresh IDs, not the original template IDs.
    for &id in inst.nodes.keys() {
        assert_ne!(
            id, in_a_id,
            "external placeholder should not appear as internal node"
        );
    }

    // The placeholder's outgoing wire leaves the caller's node, and no wire
    // names the placeholder.
    assert!(
        inst.wires
            .iter()
            .any(|w| w.source.node_id == caller_id && w.source.port == WirePort::RerOutput),
        "external ref wire should use caller_id: {:?}",
        inst.wires
    );
    assert!(
        inst.wires
            .iter()
            .all(|w| w.source.node_id != in_a_id && w.target.node_id != in_a_id),
        "the external placeholder leaked into a wire: {:?}",
        inst.wires
    );
}

#[test]
fn instantiate_50_times_all_disjoint() {
    let t = CompiledTemplate::from_module(simple_add_module());
    let caps = HashMap::default();
    let mut all_ids: HashSet<NodeId> = HashSet::default();

    for i in 0..50 {
        let inst = t.instantiate(&format!("stress{i}"), &caps);
        for &id in inst.nodes.keys() {
            assert!(
                all_ids.insert(id),
                "collision: node '{}' appeared in multiple instances",
                id
            );
        }
    }
}

fn collect_all_ids(m: &Module) -> HashSet<NodeId> {
    let mut ids: HashSet<NodeId> = m.nodes.keys().cloned().collect();
    for child_module in m.chips.values() {
        ids.extend(collect_all_ids(child_module));
    }
    ids
}

/// The `chips` map keys at every level of `m`.
fn collect_chip_keys(m: &Module) -> HashSet<NodeId> {
    let mut keys: HashSet<NodeId> = m.chips.keys().cloned().collect();
    for child in m.chips.values() {
        keys.extend(collect_chip_keys(child));
    }
    keys
}

#[test]
fn instantiate_three_level_nesting() {
    // leaf: simple_add_module (4 nodes)
    let leaf = simple_add_module();
    let leaf_chip_key = NodeId::fresh();
    let mid_gate_id = NodeId::fresh();

    // mid: 1 gate + 1 chip containing leaf
    let mut mid = Module::new("mid");
    mid.nodes.insert(
        mid_gate_id,
        test_node(mid_gate_id, NodeKind::Gate, ADD_GATE),
    );
    mid.nodes.insert(
        leaf_chip_key,
        test_node(leaf_chip_key, NodeKind::Chip, MC_CHIP),
    );
    mid.chips.insert(leaf_chip_key, leaf);

    let mid_chip_key = NodeId::fresh();
    let top_gate_id = NodeId::fresh();

    // top: 1 gate + 1 chip containing mid
    let mut top = Module::new("top");
    top.nodes.insert(
        top_gate_id,
        test_node(top_gate_id, NodeKind::Gate, ADD_GATE),
    );
    top.nodes.insert(
        mid_chip_key,
        test_node(mid_chip_key, NodeKind::Chip, MC_CHIP),
    );
    top.chips.insert(mid_chip_key, mid);

    let t = CompiledTemplate::from_module(top);
    let caps = HashMap::default();

    let inst0 = t.instantiate("top0", &caps);
    let inst1 = t.instantiate("top1", &caps);

    let ids0 = collect_all_ids(&inst0);
    let ids1 = collect_all_ids(&inst1);
    assert!(
        ids0.is_disjoint(&ids1),
        "three-level instances share IDs: {:?}",
        ids0.intersection(&ids1).collect::<Vec<_>>()
    );
    // The chip map keys are remapped along with the chip nodes.
    let keys0 = collect_chip_keys(&inst0);
    let keys1 = collect_chip_keys(&inst1);
    assert_eq!(keys0.len(), 2, "{keys0:?}");
    assert!(
        keys0.is_disjoint(&keys1),
        "chip keys overlap between instances"
    );
    assert!(
        !keys0.contains(&mid_chip_key) && !keys0.contains(&leaf_chip_key),
        "a chip key kept its template id"
    );

    // Nesting structure: inst0 should have exactly 1 child chip (mid),
    // and that child chip should itself have exactly 1 child chip (leaf).
    assert_eq!(inst0.chips.len(), 1, "top should have 1 child chip");
    let mid_inst = inst0.chips.values().next().unwrap();
    assert_eq!(mid_inst.chips.len(), 1, "mid should have 1 child chip");
}

#[test]
fn instantiate_empty_template() {
    let m = Module::new("empty");
    let t = CompiledTemplate::from_module(m);
    let inst = t.instantiate("e0", &HashMap::default());

    assert_eq!(inst.nodes.len(), 0, "expected 0 nodes");
    assert_eq!(inst.wires.len(), 0, "expected 0 wires");
    assert_eq!(inst.chips.len(), 0, "expected 0 chips");
}

#[test]
fn instantiate_no_captures_no_params() {
    let gate_id = NodeId::fresh();

    let mut m = Module::new("solo");
    m.nodes
        .insert(gate_id, test_node(gate_id, NodeKind::Gate, ADD_GATE));

    let t = CompiledTemplate::from_module(m);
    let inst = t.instantiate("nc0", &HashMap::default());

    assert_eq!(inst.nodes.len(), 1, "expected exactly 1 node");
    let new_id = *inst.nodes.keys().next().unwrap();
    assert_ne!(new_id, gate_id, "instantiated node must have a new ID");
}

#[test]
fn instantiate_multiple_captures_same_call() {
    use crate::intern::sym;
    let gate_id = NodeId::fresh();

    let ext_a_id = NodeId::fresh();
    let ext_b_id = NodeId::fresh();
    let ext_c_id = NodeId::fresh();

    let real_a = NodeId::fresh();
    let real_b = NodeId::fresh();
    let real_c = NodeId::fresh();

    let mut m = Module::new("multi_cap");

    m.nodes.insert(
        gate_id,
        Node {
            ports: Arc::new(GateIO {
                inputs: vec![
                    PortSpec::new(*sym::INPUT_A, Type::Int),
                    PortSpec::new(*sym::INPUT_B, Type::Int),
                ],
                outputs: vec![PortSpec::new(*sym::OUTPUT, Type::Int)],
            }),
            ..test_node(gate_id, NodeKind::Gate, ADD_GATE)
        },
    );

    m.wires
        .push(wire(ext_a_id, WirePort::InputA, gate_id, WirePort::InputA));
    m.wires
        .push(wire(ext_b_id, WirePort::InputB, gate_id, WirePort::InputB));
    // ext_c -> gate.Output (unusual, but exercises the path)
    m.wires
        .push(wire(ext_c_id, WirePort::Output, gate_id, WirePort::Output));

    let mut t = CompiledTemplate::from_module(m);
    t.external_refs.push(("ext_a".to_string(), ext_a_id));
    t.external_refs.push(("ext_b".to_string(), ext_b_id));
    t.external_refs.push(("ext_c".to_string(), ext_c_id));

    let mut captures = HashMap::default();
    captures.insert("ext_a".to_string(), real_a);
    captures.insert("ext_b".to_string(), real_b);
    captures.insert("ext_c".to_string(), real_c);

    let inst = t.instantiate("mc0", &captures);

    let wire_sources: HashSet<NodeId> = inst.wires.iter().map(|w| w.source.node_id).collect();

    assert!(
        wire_sources.contains(&real_a),
        "real_node_a not found as wire source; sources: {:?}",
        wire_sources.iter().collect::<Vec<_>>()
    );
    assert!(
        wire_sources.contains(&real_b),
        "real_node_b not found as wire source"
    );
    assert!(
        wire_sources.contains(&real_c),
        "real_node_c not found as wire source"
    );

    for w in &inst.wires {
        assert_ne!(
            w.source.node_id, ext_a_id,
            "ext_a placeholder leaked into wires"
        );
        assert_ne!(
            w.source.node_id, ext_b_id,
            "ext_b placeholder leaked into wires"
        );
        assert_ne!(
            w.source.node_id, ext_c_id,
            "ext_c placeholder leaked into wires"
        );
    }
}

#[test]
fn instantiate_boundary_wire_into_child_is_per_instance() {
    // A parent module with a gate wired into a CHILD chip's input node: the
    // wire lives in the PARENT wire list but its target is a CHILD node (how
    // a chip call wires an argument into a nested chip). Two instances must
    // NOT converge on the same child node — that fan-in collision is what
    // made the game drop every second-instance boundary wire.
    let gate_id = NodeId::fresh();
    let chip_key = NodeId::fresh();
    let child_input_id = NodeId::fresh();

    // Child module: one MicrochipInput node.
    let mut child = Module::new("child");
    child.nodes.insert(
        child_input_id,
        Node {
            ports: rerouter_ports(),
            ..test_node(child_input_id, NodeKind::Input, MC_INPUT)
        },
    );
    child.inputs.push(child_input_id);

    // Parent: a gate + the chip node, plus a wire gate.Output ->
    // child_input.RER_Input (target is the CHILD node, not a parent node).
    let mut parent = Module::new("parent");
    parent
        .nodes
        .insert(gate_id, test_node(gate_id, NodeKind::Gate, ADD_GATE));
    parent
        .nodes
        .insert(chip_key, test_node(chip_key, NodeKind::Chip, MC_CHIP));
    parent.chips.insert(chip_key, child);
    parent.wires.push(wire(
        gate_id,
        WirePort::Output,
        child_input_id,
        WirePort::RerInput,
    ));

    let t = CompiledTemplate::from_module(parent);
    let caps = HashMap::default();
    let inst0 = t.instantiate("i0", &caps);
    let inst1 = t.instantiate("i1", &caps);

    let boundary_target = |m: &Module| -> NodeId {
        m.wires
            .iter()
            .find(|w| w.target.port == WirePort::RerInput)
            .expect("boundary wire present")
            .target
            .node_id
    };
    let t0 = boundary_target(&inst0);
    let t1 = boundary_target(&inst1);
    assert_ne!(
        t0, t1,
        "two instances' boundary wires target the SAME child node (fan-in collision)"
    );

    // Each instance's boundary target must be a real node in THAT instance's
    // freshly-stamped child (not the template's original id).
    let child_has = |m: &Module, id: NodeId| m.chips.values().any(|c| c.nodes.contains_key(&id));
    assert!(
        child_has(&inst0, t0),
        "inst0 boundary target not in its child"
    );
    assert!(
        child_has(&inst1, t1),
        "inst1 boundary target not in its child"
    );
    assert_ne!(t0, child_input_id, "boundary target leaked the template id");
}
