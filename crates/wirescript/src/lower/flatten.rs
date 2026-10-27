//! `@flat`: inline every chip body onto one grid.
//!
//! Runs in place of [`crate::emit::partition_anon_chips`], just before
//! [`super::boundary_pins`]. Deepest chip first, each child module's nodes,
//! wires and scopes move into the module that instantiates it and the chip node
//! is dropped, leaving one [`Module`] with an empty `chips` map. Boundary pins,
//! layout and emit then see a single grid with no microchip bricks.
//!
//! `partition_anon_chips` is skipped, not undone: running it and merging back
//! would turn its per-chip literal clones into duplicate constant gates and
//! leave its `WirePort::Layout` edges behind.
//!
//! Declared chip `in`/`out` pins stay as ordinary same-grid rerouters, keeping
//! the wiring shape. They are not added to the host's `inputs`/`outputs`, which
//! drive the program's own top-level rerouter bricks. `@label` and `@closed` on
//! a chip have nothing to apply to.

use crate::collections::{HashMap, HashSet};
use crate::diagnostic::Diagnostic;
use crate::intern::sym;
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{Literal, Module, NodeId, NodeKind, PortRef, ROOT_SCOPE_ID, ScopeId, ScopeInfo};

/// What flattening had to discard, for callers that want to assert it found
/// nothing surprising.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FlattenStats {
    /// Wires dropped because they still address a chip instance after the
    /// merge (a port label the merged body has no pin for). Ordinary lowering
    /// never produces these, so non-zero means an unexpected wire shape.
    pub dropped_chip_port_wires: usize,
}

/// Inline every chip in `module`, recursively, leaving one flat module.
///
/// A loop's plane refuses instead (WS080): flattening it would compile a
/// different program. Refusal runs once, after every chip has merged up, so
/// a plane nested under a named chip is reported exactly once.
pub(crate) fn flatten_chips(
    module: &mut Module,
    diagnostics: &mut Vec<Diagnostic>,
) -> FlattenStats {
    let mut stats = FlattenStats::default();
    flatten_into(module);
    drop_chip_nodes(module, &mut stats, diagnostics);
    module.template_key = None;
    module.scope_captures = super::call::compute_scope_captures(module);
    stats
}

fn flatten_into(module: &mut Module) {
    // Deterministic order: `chips` is a HashMap, and the scope ids handed out
    // by `merge_child` depend on the order children are merged in.
    let mut chip_ids: Vec<NodeId> = module.chips.keys().copied().collect();
    chip_ids.sort_unstable();

    for chip_id in chip_ids {
        let mut child = module.chips.remove(&chip_id).expect("chip module");
        // Deepest first, so `child` is already flat when it is merged and
        // every node lands in its final home in one hop.
        flatten_into(&mut child);
        merge_child(module, chip_id, child);
    }
    module.chips.clear();
}

/// Move `child`'s contents into `parent`, which instantiates it at
/// `chip_id`.
fn merge_child(parent: &mut Module, chip_id: NodeId, child: Module) {
    // Scopes. Scope ids are per module (each restarts at `ROOT_SCOPE_ID`, and
    // template instances copy their table), so every merged scope gets a fresh
    // host id. The child's tree is kept, its root re-parented onto the chip
    // node's scope, so the region tree still nests the body where the chip was.
    let host_scope = parent
        .nodes
        .get(&chip_id)
        .map(|n| n.scope_id)
        .filter(|s| parent.scopes.contains_key(s))
        .unwrap_or(ROOT_SCOPE_ID);
    let first_scope = parent.scopes.keys().copied().max().unwrap_or(ROOT_SCOPE_ID) + 1;
    let mut old_scopes: Vec<ScopeId> = child.scopes.keys().copied().collect();
    old_scopes.sort_unstable();
    let scope_map: HashMap<ScopeId, ScopeId> =
        old_scopes.iter().copied().zip(first_scope..).collect();
    for old in &old_scopes {
        let info = &child.scopes[old];
        // `parent: None` marks the child's root, which hangs off the chip
        // node's scope; any other unmapped parent falls back there too.
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

    // Wires addressing the instance by port label. Emit's `build_port_index`
    // resolves a `(chip node, port label)` end to the child pin it names; the
    // chip node is about to go away, so substitute the pin here.
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

    // Nodes. `NodeId`s are globally unique, so they move without renumbering;
    // the assert catches a clobber, which would silently drop a gate.
    for (id, mut node) in child.nodes {
        // `chip_id` still names the right shell after the move;
        // `drop_chip_nodes` clears it once on the fully merged module.
        node.scope_id = scope_map.get(&node.scope_id).copied().unwrap_or(host_scope);
        let clobbered = parent.nodes.insert(id, node);
        assert!(
            clobbered.is_none(),
            "flatten: node {id} exists in both a chip body and the module hosting it"
        );
    }
    parent.wires.extend(child.wires);
}

/// Remove every `NodeKind::Chip` node (an empty shell by now), the wires that
/// reference one, and the `chip_id` tags that point at one.
///
/// A loop's plane shell is refused (WS080) instead of dropped: it and its
/// still chip_id-tagged body are left alone, since merging them would compile
/// a different program.
fn drop_chip_nodes(
    module: &mut Module,
    stats: &mut FlattenStats,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut shells: HashSet<NodeId> = HashSet::default();
    let mut refused: HashSet<NodeId> = HashSet::default();
    for (id, n) in &module.nodes {
        if n.kind != NodeKind::Chip {
            continue;
        }
        if gc::is_plane_shell(n.gate_class) {
            refused.insert(*id);
            diagnostics.push(Diagnostic::error(
                "WS080",
                "`@flat` cannot flatten this `for` loop's plane. A loop's plane is an \
                 execution boundary, not a visual grouping, so merging its body onto the \
                 outer grid would compile a different program. Remove `@flat` from the file.",
                n.source_range.clone(),
            ));
            continue;
        }
        shells.insert(*id);
    }

    for node in module.nodes.values_mut() {
        if let Some(cid) = node.chip_id
            && !refused.contains(&cid)
        {
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
