//! `@flat` — inline every chip body onto one grid.
//!
//! Runs in place of [`crate::emit::partition_anon_chips`], just before
//! [`super::boundary_pins`]. Working deepest-chip-first, each child module's
//! nodes, wires and scopes move into the module that instantiates it and the
//! chip node itself is dropped, until the whole program is a single
//! [`Module`] with an empty `chips` map.
//!
//! Everything downstream then does the right thing untouched: with no child
//! modules, `synthesize_boundary_pins` finds no wire whose endpoints live in
//! different modules and returns without synthesizing a pin; layout lays out
//! one module; and emit's chip pass has no `NodeKind::Chip` node to build a
//! child brick grid for. The result has no microchip bricks and no child
//! grids, and every wire that used to cross a chip wall is an ordinary
//! same-grid wire.
//!
//! It replaces `partition_anon_chips` rather than running after it because
//! that pass exists to MOVE nodes into child modules — the exact thing being
//! undone here. Running it first and merging back would round-trip its
//! parent-side literal clones (one clone per anon chip, the original then
//! deleted) into a pile of duplicate constant gates, and leave its synthetic
//! `WirePort::Layout` edges to clean up. Skipping it leaves an anonymous
//! chip's members where they already are — in the module that declares them,
//! which is what flat means — with only the (bodyless) shell node to drop.
//!
//! Declared chip `in`/`out` pins are KEPT. After the merge each is an
//! ordinary rerouter with one inbound wire and its consumers, sitting on the
//! same grid as both; keeping them preserves the wiring structure exactly,
//! where bypassing them would be an optimization with its own fan-in risk.
//! They are deliberately NOT added to the host module's `inputs`/`outputs`
//! lists: those drive the outer rerouter bricks for the program's own
//! top-level `in`/`out`, and a chip pin is not one of those.
//!
//! `@label` and `@closed` on a chip describe a microchip brick that no
//! longer exists, so under `@flat` they simply have nothing to apply to.

use crate::collections::{HashMap, HashSet};
use crate::intern::sym;
use crate::ir::port_registry::WirePort;
use crate::ir::{Literal, Module, NodeId, NodeKind, PortRef, ROOT_SCOPE_ID, ScopeId, ScopeInfo};

/// What flattening had to discard, for callers that want to assert it found
/// nothing surprising.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlattenStats {
    /// Wires still addressing a chip instance by a port label after the
    /// merge — i.e. naming a pin the merged body does not have. Ordinary
    /// lowering wires straight to the child's pin nodes and never produces
    /// this, so a non-zero count is a wire shape this pass does not
    /// understand; the wires are dropped.
    pub dropped_chip_port_wires: usize,
}

/// Inline every chip in `module`, recursively, leaving one flat module.
pub fn flatten_chips(module: &mut Module) -> FlattenStats {
    let mut stats = FlattenStats::default();
    flatten_into(module, &mut stats);
    module.scope_captures = super::call::compute_scope_captures(module);
    stats
}

fn flatten_into(module: &mut Module, stats: &mut FlattenStats) {
    // Deterministic order: `chips` is a HashMap, and the scope ids handed out
    // by `merge_child` depend on the order children are merged in.
    let mut chip_ids: Vec<NodeId> = module.chips.keys().copied().collect();
    chip_ids.sort_unstable();

    for chip_id in chip_ids {
        let mut child = module.chips.remove(&chip_id).expect("chip module");
        // Deepest first, so `child` is already flat when it is merged and
        // every node lands in its final home in one hop.
        flatten_into(&mut child, stats);
        merge_child(module, chip_id, child);
    }
    module.chips.clear();

    // Every `NodeKind::Chip` node is bodyless by now: a named instance had
    // its body merged above, and an anonymous chip never had one — its
    // members are tagged with `chip_id` and left in place. Drop the shells
    // and the tags.
    drop_chip_nodes(module, stats);

    // The module is no longer a verbatim copy of any chip template.
    module.template_key = None;
}

/// Move `child`'s contents into `parent`, which instantiates it at
/// `chip_id`.
fn merge_child(parent: &mut Module, chip_id: NodeId, child: Module) {
    // ── Scopes ──
    // Child scope ids are NOT unique across modules: every module seeds
    // `ROOT_SCOPE_ID`, each chip body starts its counter over at
    // `ROOT_SCOPE_ID + 1`, and a template instance copies its source's scope
    // table verbatim, so two instances of one chip carry identical ids. Every
    // merged scope therefore gets a fresh id in the host.
    //
    // The child's scope TREE is preserved rather than collapsed onto the
    // host's root: its root scope (a `ChipBody`) is re-parented onto whatever
    // scope the chip node itself sat in, so the region tree still shows the
    // chip's body as a group nested where the chip was. Collapsing would
    // have been simpler but throws that structure away for nothing — nothing
    // downstream requires a flat scope table.
    let host_scope = parent
        .nodes
        .get(&chip_id)
        .map(|n| n.scope_id)
        .filter(|s| parent.scopes.contains_key(s))
        .unwrap_or(ROOT_SCOPE_ID);
    let mut next_scope = parent.scopes.keys().copied().max().unwrap_or(ROOT_SCOPE_ID) + 1;
    let mut old_scopes: Vec<ScopeId> = child.scopes.keys().copied().collect();
    old_scopes.sort_unstable();
    let mut scope_map: HashMap<ScopeId, ScopeId> = HashMap::default();
    for old in &old_scopes {
        scope_map.insert(*old, next_scope);
        next_scope += 1;
    }
    for old in &old_scopes {
        let info = &child.scopes[old];
        // `parent: None` marks the child's own root; it hangs off the scope
        // the chip node lived in. Any other unmapped parent falls back there
        // too rather than dangling.
        let new_parent = info
            .parent
            .and_then(|p| scope_map.get(&p).copied())
            .unwrap_or(host_scope);
        parent.scopes.insert(
            scope_map[old],
            ScopeInfo {
                kind: info.kind.clone(),
                source_range: info.source_range.clone(),
                parent: Some(new_parent),
            },
        );
    }

    // ── Wires addressing the instance by port label ──
    // `emit::build_port_index` resolves a `(chip node, port label)` wire end
    // to the child pin that label names. Once the pin is a sibling node here
    // that indirection has nothing left to resolve, so do the substitution
    // now, before the chip node goes away.
    let mut by_label: HashMap<String, (NodeId, bool)> = HashMap::default();
    for (pin_id, pin) in &child.nodes {
        let is_output = match pin.kind {
            NodeKind::Output => true,
            NodeKind::Input => false,
            _ => continue,
        };
        if let Some(Literal::String(label)) = pin.properties.get(&*sym::PORT_LABEL)
            && !label.is_empty()
        {
            by_label.insert(label.clone(), (*pin_id, is_output));
        }
    }
    if !by_label.is_empty() {
        let resolve_end = |p: PortRef| -> PortRef {
            if p.node_id != chip_id || p.port == WirePort::Layout {
                return p;
            }
            match by_label.get(p.port.as_str()) {
                Some(&(pin, is_output)) => PortRef {
                    node_id: pin,
                    // Same remap `build_port_index` applies: the value
                    // leaves an output pin on RER_Output and enters an
                    // input pin on RER_Input.
                    port: if is_output {
                        WirePort::RerOutput
                    } else {
                        WirePort::RerInput
                    },
                },
                None => p,
            }
        };
        for w in &mut parent.wires {
            w.source = resolve_end(w.source);
            w.target = resolve_end(w.target);
        }
    }

    // ── Nodes ──
    // `NodeId`s are globally fresh (`NodeId::fresh()` off one process-wide
    // counter, and template instantiation re-stamps every copied node), so a
    // move needs no renumbering — but assert it rather than trust it, since a
    // silent clobber here would drop a gate with no diagnostic.
    for (id, mut node) in child.nodes {
        node.chip_id = None;
        node.scope_id = scope_map
            .get(&node.scope_id)
            .copied()
            .unwrap_or(host_scope);
        let clobbered = parent.nodes.insert(id, node);
        assert!(
            clobbered.is_none(),
            "flatten: node {id} exists in both a chip body and the module hosting it"
        );
    }
    parent.wires.extend(child.wires);
}

/// Remove every `NodeKind::Chip` node — each is an empty shell by the time
/// this runs — along with the wires that reference one and the `chip_id`
/// tags that pointed at one.
fn drop_chip_nodes(module: &mut Module, stats: &mut FlattenStats) {
    let shells: HashSet<NodeId> = module
        .nodes
        .iter()
        .filter(|(_, n)| n.kind == NodeKind::Chip)
        .map(|(id, _)| *id)
        .collect();

    for node in module.nodes.values_mut() {
        if node.chip_id.is_some() {
            node.chip_id = None;
        }
    }

    if shells.is_empty() {
        return;
    }
    module.nodes.retain(|id, _| !shells.contains(id));
    module.inputs.retain(|id| !shells.contains(id));
    module.outputs.retain(|id| !shells.contains(id));
    module.wires.retain(|w| {
        let touches = shells.contains(&w.source.node_id) || shells.contains(&w.target.node_id);
        if touches && w.source.port != WirePort::Layout && w.target.port != WirePort::Layout {
            stats.dropped_chip_port_wires += 1;
        }
        !touches
    });
}
