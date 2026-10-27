//! Pre-pass that moves `chip_id`-tagged nodes into child modules.

use crate::collections::{HashMap, HashSet};
use crate::intern::sym;
use crate::ir::port_registry::WirePort;
use crate::ir::{Module, NodeId, NodeKind, PortRef, Wire, gate_class as gc};

/// Pre-pass: move nodes tagged with `chip_id` into child Modules so the chip emit path
/// handles them. Cross-boundary wires stay in the parent module; the brdb writer's `add_wire`
/// creates remote wire sources when source and target are on different grids.
pub fn partition_anon_chips(module: &mut Module) {
    partition_one(module);
    // A named chip's instance module is partitioned too: an anon `chip { }` written inside
    // one tags its nodes there, not at the root, and emit skips a `Chip` node that has no
    // `chips` entry. Sorted, since the intern order of `_anon_{id}` names decides emitted wire
    // counts and Sym numbering.
    let mut nested: Vec<NodeId> = module.chips.keys().copied().collect();
    nested.sort_unstable();
    for id in nested {
        if let Some(child) = module.chips.get_mut(&id) {
            partition_anon_chips(child);
        }
    }
}

fn partition_one(module: &mut Module) {
    let layout_port = WirePort::Layout;

    // node -> owning anon chip, from the chip_id tags (one scan of nodes).
    let assignment: HashMap<NodeId, NodeId> = module
        .nodes
        .iter()
        .filter_map(|(id, n)| n.chip_id.map(|c| (*id, c)))
        .collect();
    // Sorted Vec, not a std HashSet: iteration order decides the intern order of the
    // `_anon_{id}` module names, and so emitted wire counts and Sym numbering.
    let mut chip_ids: Vec<NodeId> = assignment.values().copied().collect();
    chip_ids.sort_unstable();
    chip_ids.dedup();

    // Anon chips with an entirely empty functional body (e.g. `@label("...") chip on trigger { }`)
    // tag no descendant nodes, so the loop below never sees them, and emit skips a `Chip` node
    // with no `module.chips` entry along with its `@label`/`@closed`/doc annotations. Give them
    // an empty child module. Named chip instances already have a populated module (see
    // `lower/call.rs`).
    let empty_annotated: Vec<NodeId> = module
        .nodes
        .iter()
        .filter(|(id, n)| {
            n.kind == NodeKind::Chip
                && chip_ids.binary_search(id).is_err()
                && !module.chips.contains_key(id)
                && (n.properties.contains_key(&*sym::NAME_LABEL)
                    || n.properties.contains_key(&*sym::CHIP_CLOSED)
                    || n.properties.contains_key(&*sym::DOC_TEXT)
                    || n.properties.contains_key(&*sym::BRICK_COLOR)
                    || n.properties.contains_key(&*sym::BRICK_MATERIAL))
        })
        .map(|(id, _)| *id)
        .collect();
    for id in empty_annotated {
        module.chips.insert(id, Module::new(&format!("_anon_{id}")));
    }

    if chip_ids.is_empty() {
        return;
    }

    // Parent-side Literal nodes we clone into chips (below); cleaned up after.
    let mut cloned_literal_sources: HashSet<NodeId> = HashSet::default();

    // Child module per anon chip; tagged nodes move into their chip's child.
    let mut children: HashMap<NodeId, Module> = chip_ids
        .iter()
        .map(|c| (*c, Module::new(&format!("_anon_{c}"))))
        .collect();
    for (&nid, &cid) in &assignment {
        if let Some(mut node) = module.nodes.remove(&nid) {
            node.chip_id = None;
            children
                .get_mut(&cid)
                .expect("child module for tagged node")
                .nodes
                .insert(nid, node);
        }
    }

    // Map each chip-instance boundary pin (MicrochipInput/Output of a called `chip`'s module)
    // to the anon chip its instance node is tagged to. A wire targeting such a pin belongs
    // inside that anon chip's module: a root wire can route one grid in, but an exec pulse
    // cannot cross into an instance grid nested inside another anon chip.
    let mut pin_chip: HashMap<NodeId, NodeId> = HashMap::default();
    for (chip_node, inst) in &module.chips {
        if let Some(&cid) = assignment.get(chip_node) {
            for pin in inst.inputs.iter().chain(inst.outputs.iter()) {
                pin_chip.insert(*pin, cid);
            }
        }
    }
    let chip_of = |id: &NodeId| assignment.get(id).or_else(|| pin_chip.get(id)).copied();

    // Partition wires in one pass: internal wires go to their chip's child; cross-boundary
    // wires stay in the parent as remote wires, with Layout edges keeping the chips inline in
    // the DAG (chip->chip, chip->inner-node and inner-node->chip as the wire crosses).
    let layout_edge = |a: NodeId, b: NodeId| Wire {
        source: PortRef {
            node_id: a,
            port: layout_port,
        },
        target: PortRef {
            node_id: b,
            port: layout_port,
        },
    };
    let mut parent_wires: Vec<Wire> = Vec::with_capacity(module.wires.len());
    let mut seen_layout_edges: HashSet<(NodeId, NodeId)> = HashSet::default();
    // Dedupe of Literal nodes cloned into a target module, per (target, literal)
    // where the target is `Some(chip)` or `None` for the parent.
    let mut literal_clones: HashMap<(Option<NodeId>, NodeId), NodeId> = HashMap::default();
    for w in std::mem::take(&mut module.wires) {
        let src_chip = chip_of(&w.source.node_id);
        let tgt_chip = chip_of(&w.target.node_id);
        // A `_Literal` source feeding a target in a different module cannot be a wire: `_Literal`
        // is a compiler placeholder that emit inlines into its same-module consumers, so a
        // cross-module wire leaves the far end reading the port default (0). Clone the literal into
        // the target's module and keep the wire internal there. Vars cross a boundary via a Ref
        // port; a literal has none. Covers parent->child, chip->chip and chip->parent.
        if src_chip != tgt_chip {
            let src_is_literal = match src_chip {
                Some(a) => &children[&a].nodes,
                None => &module.nodes,
            }
            .get(&w.source.node_id)
            .is_some_and(|n| n.gate_class == gc::LITERAL);
            if src_is_literal {
                let key = (tgt_chip, w.source.node_id);
                let clone_id = match literal_clones.get(&key) {
                    Some(&id) => id,
                    None => {
                        let mut cl = match src_chip {
                            Some(a) => children[&a].nodes[&w.source.node_id].clone(),
                            None => module.nodes[&w.source.node_id].clone(),
                        };
                        let nid = NodeId::fresh();
                        cl.id = nid;
                        cl.chip_id = None;
                        match tgt_chip {
                            Some(b) => {
                                children
                                    .get_mut(&b)
                                    .expect("chip module")
                                    .nodes
                                    .insert(nid, cl);
                            }
                            None => {
                                module.nodes.insert(nid, cl);
                            }
                        }
                        cloned_literal_sources.insert(w.source.node_id);
                        literal_clones.insert(key, nid);
                        nid
                    }
                };
                let mut w2 = w;
                w2.source.node_id = clone_id;
                match tgt_chip {
                    Some(b) => children.get_mut(&b).expect("chip module").wires.push(w2),
                    None => parent_wires.push(w2),
                }
                continue;
            }
        }
        match (src_chip, tgt_chip) {
            (Some(a), Some(b)) if a == b => {
                children.get_mut(&a).expect("chip module").wires.push(w);
            }
            (None, None) => parent_wires.push(w),
            (Some(a), None) => {
                if seen_layout_edges.insert((a, w.target.node_id)) {
                    parent_wires.push(layout_edge(a, w.target.node_id));
                }
                parent_wires.push(w);
            }
            (None, Some(b)) => {
                if seen_layout_edges.insert((w.source.node_id, b)) {
                    parent_wires.push(layout_edge(w.source.node_id, b));
                }
                parent_wires.push(w);
            }
            (Some(a), Some(b)) => {
                if seen_layout_edges.insert((a, b)) {
                    parent_wires.push(layout_edge(a, b));
                }
                if seen_layout_edges.insert((a, w.target.node_id)) {
                    parent_wires.push(layout_edge(a, w.target.node_id));
                }
                if seen_layout_edges.insert((w.source.node_id, b)) {
                    parent_wires.push(layout_edge(w.source.node_id, b));
                }
                parent_wires.push(w);
            }
        }
    }
    module.wires = parent_wires;
    for (cid, child) in children {
        module.chips.insert(cid, child);
    }

    // Drop parent-side Literal nodes that were fully cloned into chips and now
    // have no remaining parent consumer, so they don't emit as stray gates.
    let sources: HashSet<NodeId> = module.wires.iter().map(|w| w.source.node_id).collect();
    for lit_id in cloned_literal_sources {
        if !sources.contains(&lit_id) {
            module.nodes.remove(&lit_id);
        }
    }

    // Re-nest inner chip modules: a child module holding a Chip node whose module sits in this
    // module's `chips` map takes that module into its own `chips` map so emit can find it.
    // Moving the deepest first keeps each parent at this level until its own children have
    // moved into it.
    let mut parent_of: HashMap<NodeId, NodeId> = HashMap::default();
    for (parent_id, child_mod) in &module.chips {
        for (nid, n) in &child_mod.nodes {
            if n.kind == NodeKind::Chip && module.chips.contains_key(nid) {
                parent_of.insert(*nid, *parent_id);
            }
        }
    }
    let depth = |mut id: NodeId| {
        let mut d = 0usize;
        while let Some(&p) = parent_of.get(&id) {
            d += 1;
            id = p;
        }
        d
    };
    let mut moves: Vec<(usize, NodeId, NodeId)> = parent_of
        .iter()
        .map(|(&inner, &parent)| (depth(inner), inner, parent))
        .collect();
    moves.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, inner_id, parent_id) in moves {
        let inner_child = module.chips.remove(&inner_id).expect("inner chip module");
        module
            .chips
            .get_mut(&parent_id)
            .expect("parent chip module still at this level")
            .chips
            .insert(inner_id, inner_child);
    }
}
