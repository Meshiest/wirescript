//! Wire-end resolution and the gutter-bus lane bricks.

use super::*;

/// One rerouter brick per [`crate::layout::BusNode`], in `BusNodeId` order —
/// the gutter lanes that carry a value to its consumers in place of one long
/// diagonal wire each. Returns each lane node's brick id, indexed by
/// `BusNodeId`, for the wire pass to address.
///
/// Positions are min-corner like every other brick the layout hands over, so
/// the brick centre sits a rerouter half-extent (1) in on both axes.
pub(super) fn emit_bus(
    bricks: &mut Vec<brdb::Brick>,
    bus: &BusLayout,
    module: &Module,
    wire_target_index: &HashMap<(NodeId, WirePort), NodeId>,
) -> Vec<usize> {
    let mut brick_ids = Vec::with_capacity(bus.nodes.len());
    for node in &bus.nodes {
        let (mut brick, brick_id) = brdb::Brick {
            asset: BrickType::from("B_1x1_Reroute_Node"),
            position: Position {
                x: node.x + 1,
                y: node.y + 1,
                z: node.z,
            },
            rotation: match node.rotation {
                NodeRotation::Deg0 => brdb::Rotation::Deg0,
                NodeRotation::Deg90 => brdb::Rotation::Deg90,
                NodeRotation::Deg180 => brdb::Rotation::Deg180,
                NodeRotation::Deg270 => brdb::Rotation::Deg270,
            },
            color: node
                .color_of
                .map(|id| match module.nodes.get(&id) {
                    Some(n) => color_for_node(n, module, wire_target_index),
                    // The allocator named a node this module doesn't own, so
                    // the lane can't mirror its colour — say so rather than
                    // leaving a silently default-coloured lane to explain.
                    None => {
                        eprintln!(
                            "[bus] colour source {id} is not a node of module {}; using the default",
                            resolve(module.name)
                        );
                        Color::default()
                    }
                })
                .unwrap_or_default(),
            ..Default::default()
        }
        .with_id_split();
        // Saved worlds use the component's instance name; the rerouter's
        // data struct is empty.
        brick.add_component_box(Box::new(LiteralComponent::new(gc::REROUTER)));
        bricks.push(brick);
        brick_ids.push(brick_id);
    }
    brick_ids
}

/// Pre-build index: (chip_node_id, port_name) → (brick_id, component_class, remapped_port)
pub(super) fn build_port_index(
    module: &Module,
    node_brick_ids: &HashMap<NodeId, usize>,
) -> HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)> {
    let port_label_sym = *sym::PORT_LABEL;
    let mut idx = HashMap::default();
    for (chip_nid, node) in &module.nodes {
        if node.kind != NodeKind::Chip {
            continue;
        }
        let child = match module.chips.get(chip_nid) {
            Some(c) => c,
            None => continue,
        };
        for (child_nid, child_node) in &child.nodes {
            let is_output = child_node.kind == NodeKind::Output;
            let is_input = child_node.kind == NodeKind::Input;
            if !is_output && !is_input {
                continue;
            }
            let label: &'static str =
                match child_node.properties.get(&port_label_sym).and_then(|l| {
                    if let Literal::String(s) = l {
                        Some(resolve(crate::intern::intern(s)))
                    } else {
                        None
                    }
                }) {
                    Some(l) => l,
                    None => continue,
                };
            if label.is_empty() {
                continue;
            }
            if let Some(&brick_id) = node_brick_ids.get(child_nid) {
                let class: &'static str = if is_output {
                    "BrickComponentType_Internal_MicrochipOutput"
                } else {
                    "BrickComponentType_Internal_MicrochipInput"
                };
                let remap_port: &'static str = if is_output { "RER_Output" } else { "RER_Input" };
                idx.insert((*chip_nid, label), (brick_id, class, remap_port));
            }
        }
    }
    idx
}

pub(super) fn is_spawnable(kind: NodeKind, gate_class: &str) -> bool {
    if gate_class == gc::LITERAL || gate_class == gc::UNSUPPORTED {
        return false;
    }
    matches!(
        kind,
        NodeKind::Gate | NodeKind::Event | NodeKind::Input | NodeKind::Output
    )
}

/// Resolve one wire end — an IR node plus the port on it — to the emitted
/// brick port, applying the chip-port remap in `port_index` when the node is
/// a chip and the port one of its declared pins.
pub(super) fn resolve_wire_end(
    node_id: NodeId,
    port_idx: WirePort,
    node_brick_ids: &HashMap<NodeId, usize>,
    class_index: &HashMap<NodeId, &'static str>,
    port_index: &HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)>,
) -> Result<BrdbWirePort, EmitError> {
    let port_str: &'static str = port_idx.as_str();
    if let Some(&(brick_id, cls, remapped)) = port_index.get(&(node_id, port_str)) {
        return Ok(BrdbWirePort {
            brick_id,
            component_type: BString::Static(cls),
            port_name: BString::Static(remapped),
        });
    }
    let brick_id = *node_brick_ids
        .get(&node_id)
        .ok_or_else(|| EmitError::UnknownWireNode(node_id.to_string()))?;
    let cls = *class_index
        .get(&node_id)
        .ok_or_else(|| EmitError::UnknownWireNode(node_id.to_string()))?;
    Ok(BrdbWirePort {
        brick_id,
        component_type: BString::Static(cls),
        port_name: BString::Static(port_str),
    })
}

pub(super) fn wire_to_connection_indexed(
    w: &Wire,
    node_brick_ids: &HashMap<NodeId, usize>,
    class_index: &HashMap<NodeId, &'static str>,
    port_index: &HashMap<(NodeId, &'static str), (usize, &'static str, &'static str)>,
) -> Result<WireConnection, EmitError> {
    Ok(WireConnection {
        source: resolve_wire_end(
            w.source.node_id,
            w.source.port,
            node_brick_ids,
            class_index,
            port_index,
        )?,
        target: resolve_wire_end(
            w.target.node_id,
            w.target.port,
            node_brick_ids,
            class_index,
            port_index,
        )?,
    })
}
