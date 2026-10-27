//! Synthesized microchip boundary pins.
//!
//! Runs after fold/materialize/partition, before layout: every wire whose
//! endpoints live in different modules of the chip tree is rewired through
//! MicrochipInput/Output rerouter pins, one per wall crossed, so external
//! references are visible (and labeled) at each chip boundary.
//!
//! Wires the pass leaves alone:
//! - `gc::LITERAL` sources: literals are inlined into the target's data, and a
//!   rerouter cannot hold inlined data.
//! - `WirePort::Layout` synthetic edges.
//!
//! `VarRef`/`ArrayVarRef` crossings are rewired too: a ref leaves a pin on
//! `RER_Output` and enters one on `RER_Input`, the only two rerouter ports
//! (see `normalize_pin_ports`). `scope_captures` is recomputed bottom-up at the end.
//!
//! A crossing whose deep endpoint is already a pin at its own wall (a declared
//! chip param/output, or a pin from an earlier run) reuses that pin, which keeps
//! the pass idempotent. Between sibling modules the middle segment is a direct
//! `MicrochipOutput` -> `MicrochipInput` wire held in the LCA module, the shape
//! lowering gives a declared chip-call wire.

use std::sync::Arc;

use crate::collections::{HashMap, HashSet};
use crate::intern::{Sym, sym};
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, Type, Wire};

/// Chip-node path from the root module; the root itself is `[]`.
type ModPath = Vec<NodeId>;

pub(crate) fn synthesize_boundary_pins(root: &mut Module) {
    // Before anything is rewired: a crossing that already lands on a pin is
    // reused as-is by `rewire`, so a wrong port on one would be carried into
    // the new wiring instead of being corrected.
    normalize_pin_ports(root);

    let mut owner: HashMap<NodeId, ModPath> = HashMap::default();
    root.walk(&mut |path, m| {
        for id in m.nodes.keys() {
            owner.insert(*id, path.to_vec());
        }
    });

    let mut crossings: Vec<Wire> = Vec::new();
    extract_crossings(root, &owner, &mut crossings);
    if crossings.is_empty() {
        return;
    }

    // (owning module, feeding PortRef) -> pin node id. Keying on the feeder
    // dedupes: N consumers of one external source share one pin per wall.
    let mut pins: HashMap<(ModPath, PortRef), NodeId> = HashMap::default();
    for wire in crossings {
        rewire(root, wire, &mut owner, &mut pins);
    }

    root.walk_mut(&mut |_, m| assign_pin_labels(m));
    refresh_scope_captures(root);
}

/// Put every wire endpoint that lands on a boundary pin onto that pin's own
/// rerouter port: a value LEAVES a pin on `RER_Output` and ENTERS one on
/// `RER_Input`.
///
/// A pin component has only those two ports, so any other port name on one
/// does not exist in game. Lowering produces such wires when a pin stands in for
/// a storage gate (a chip's array/Map/ref param read through `ArrayVarRef`,
/// `MapVarRef` or `VarRef`). `flatten::merge_child` and emit's `build_port_index`
/// apply the same remap. `WirePort::Layout` edges keep their port. Idempotent.
fn normalize_pin_ports(root: &mut Module) {
    // Tree-wide, because a parent's wire can name a pin owned by a child chip
    // (the shape of a declared chip param).
    let pins = root.node_ids_where(|n| matches!(n.kind, NodeKind::Input | NodeKind::Output));
    if pins.is_empty() {
        return;
    }
    root.walk_mut(&mut |_, m| {
        for w in &mut m.wires {
            if w.source.port != WirePort::Layout && pins.contains(&w.source.node_id) {
                w.source.port = WirePort::RerOutput;
            }
            if w.target.port != WirePort::Layout && pins.contains(&w.target.node_id) {
                w.target.port = WirePort::RerInput;
            }
        }
    });
}

/// Rewiring changes which external node ids each module's wires reference;
/// recompute captures children-first (a parent's list folds in its
/// children's).
fn refresh_scope_captures(m: &mut Module) {
    let mut kids: Vec<NodeId> = m.chips.keys().copied().collect();
    kids.sort();
    for k in kids {
        refresh_scope_captures(m.chips.get_mut(&k).unwrap());
    }
    m.scope_captures = crate::lower::call::compute_scope_captures(m);
}

/// Remove every rewirable crossing from all wire lists, in deterministic
/// (module DFS, list order) order, and collect them. Two phases because the
/// literal-source check needs a read-only view while `retain` holds the
/// mutable borrow.
fn extract_crossings(root: &mut Module, owner: &HashMap<NodeId, ModPath>, out: &mut Vec<Wire>) {
    let literal_ids = root.node_ids_where(|n| n.gate_class == gc::LITERAL);
    // A call's definition wire runs straight from the `fn` shell, through any
    // wall (see `function::is_definition_wire`).
    let shells = root.node_ids_where(|n| n.gate_class == gc::MICROCHIP_FUNCTION_DEFINITION);
    root.walk_mut(&mut |_, m| {
        m.wires.retain(|w| {
            if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
                return true;
            }
            if literal_ids.contains(&w.source.node_id) {
                return true;
            }
            if shells.contains(&w.source.node_id) && w.source.port == WirePort::Definition {
                return true;
            }
            let (Some(sp), Some(tp)) = (owner.get(&w.source.node_id), owner.get(&w.target.node_id))
            else {
                return true;
            };
            if sp == tp {
                return true;
            }
            out.push(*w);
            false
        });
    });
}

fn rewire(
    root: &mut Module,
    wire: Wire,
    owner: &mut HashMap<NodeId, ModPath>,
    pins: &mut HashMap<(ModPath, PortRef), NodeId>,
) {
    let sp = owner[&wire.source.node_id].clone();
    let tp = owner[&wire.target.node_id].clone();
    let lca = sp.iter().zip(tp.iter()).take_while(|(a, b)| a == b).count();

    let ty = source_port_type(root, &sp, wire.source).unwrap_or(Type::Any);
    let label = derive_label(root, &sp, wire.source.node_id);

    let mut feeder = wire.source;
    let mut feeder_path = sp.clone();
    // Upward legs: each module from the source's up to, not including, the LCA.
    // The deepest wall (mpath == sp) reuses `wire.source` when it is already
    // a MicrochipOutput there.
    for k in (lca + 1..=sp.len()).rev() {
        let mpath = sp[..k].to_vec();
        if mpath == sp
            && root
                .at(&mpath)
                .nodes
                .get(&wire.source.node_id)
                .is_some_and(|n| n.gate_class == gc::MICROCHIP_OUTPUT)
        {
            feeder_path = mpath;
            continue;
        }
        let (pin, is_new) = pin_for(
            root,
            owner,
            pins,
            &mpath,
            feeder,
            NodeKind::Output,
            &ty,
            &label,
        );
        // Only a new pin needs a feeding wire; a reused one already has it, and
        // a second would fan two wires into the same (pin, RerInput).
        if is_new {
            push_wire_at_lca(
                root,
                &feeder_path,
                &mpath,
                feeder,
                PortRef {
                    node_id: pin,
                    port: WirePort::RerInput,
                },
            );
        }
        feeder = PortRef {
            node_id: pin,
            port: WirePort::RerOutput,
        };
        feeder_path = mpath;
    }

    // Downward legs: each module from just below the LCA down to the target's.
    // The deepest wall (mpath == tp) reuses `wire.target` when it is already a
    // MicrochipInput there.
    for k in lca + 1..=tp.len() {
        let mpath = tp[..k].to_vec();
        if mpath == tp
            && root
                .at(&mpath)
                .nodes
                .get(&wire.target.node_id)
                .is_some_and(|n| n.gate_class == gc::MICROCHIP_INPUT)
        {
            break;
        }
        let (pin, is_new) = pin_for(
            root,
            owner,
            pins,
            &mpath,
            feeder,
            NodeKind::Input,
            &ty,
            &label,
        );
        if is_new {
            push_wire_at_lca(
                root,
                &feeder_path,
                &mpath,
                feeder,
                PortRef {
                    node_id: pin,
                    port: WirePort::RerInput,
                },
            );
        }
        feeder = PortRef {
            node_id: pin,
            port: WirePort::RerOutput,
        };
        feeder_path = mpath;
    }
    push_wire_at_lca(root, &feeder_path, &tp, feeder, wire.target);
}

/// A new wire lives in the LCA module of its endpoints: the parent for a
/// parent<->child-pin hop, the module itself for a local hop.
fn push_wire_at_lca(
    root: &mut Module,
    a: &[NodeId],
    b: &[NodeId],
    source: PortRef,
    target: PortRef,
) {
    let lca = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    root.at_mut(&a[..lca.min(a.len())])
        .wires
        .push(Wire { source, target });
}

/// The pin for `(mpath, feeder)` and whether it was just created. `false` means
/// an earlier crossing created and fed it, so the caller must not feed it again.
fn pin_for(
    root: &mut Module,
    owner: &mut HashMap<NodeId, ModPath>,
    pins: &mut HashMap<(ModPath, PortRef), NodeId>,
    mpath: &ModPath,
    feeder: PortRef,
    kind: NodeKind,
    ty: &Type,
    label: &str,
) -> (NodeId, bool) {
    if let Some(id) = pins.get(&(mpath.clone(), feeder)) {
        return (*id, false);
    }
    let id = NodeId::fresh();
    let gate_class = if kind == NodeKind::Input {
        gc::MICROCHIP_INPUT
    } else {
        gc::MICROCHIP_OUTPUT
    };
    let mut props = HashMap::default();
    props.insert(*sym::PORT_LABEL, Literal::String(label.to_string()));
    let node = Node::new(
        id,
        kind,
        gate_class,
        GateIO::pin(ty.clone()),
        props,
        None,
        Some("boundary_pin"),
    );
    let m = root.at_mut(mpath);
    m.nodes.insert(id, node);
    if kind == NodeKind::Input {
        m.inputs.push(id);
    } else {
        m.outputs.push(id);
    }
    m.template_key = None;
    owner.insert(id, mpath.clone());
    pins.insert((mpath.clone(), feeder), id);
    (id, true)
}

fn source_port_type(root: &Module, sp: &[NodeId], src: PortRef) -> Option<Type> {
    root.at(sp).nodes.get(&src.node_id)?.output_type(src.port)
}

/// The source node's identifier: `NAME_LABEL` (a declared name), then
/// `PORT_LABEL` (an upstream pin's label), then `BINDING_NAME` (a `let`), else
/// empty, which `assign_pin_labels` replaces. `Node.note` and the gate class are
/// internal strings and must not be used as a player-visible label.
fn derive_label(root: &Module, sp: &[NodeId], src: NodeId) -> String {
    let Some(n) = root.at(sp).nodes.get(&src) else {
        return String::new();
    };
    let get_str = |k: Sym| match n.properties.get(&k) {
        Some(Literal::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    get_str(*sym::NAME_LABEL)
        .or_else(|| get_str(*sym::PORT_LABEL))
        .or_else(|| get_str(*sym::BINDING_NAME))
        .unwrap_or_default()
}

/// Finalize each synthesized pin's `PORT_LABEL` in one module: an unnamed pin gets
/// `ext1`, `ext2` and so on, and a collision gets the lowest free numeric suffix.
/// Pins go in `NodeId` (creation) order so output is stable. Declared ports count
/// as taken, so a `score` param and a captured outer `score` get distinct labels.
fn assign_pin_labels(m: &mut Module) {
    let mut pin_ids: Vec<NodeId> = m
        .nodes
        .iter()
        .filter(|(_, n)| n.note == Some("boundary_pin"))
        .map(|(id, _)| *id)
        .collect();
    pin_ids.sort();

    let mut used: HashSet<String> = m
        .nodes
        .values()
        .filter(|n| {
            matches!(n.kind, NodeKind::Input | NodeKind::Output) && n.note != Some("boundary_pin")
        })
        .filter_map(|n| match n.properties.get(&*sym::PORT_LABEL) {
            Some(Literal::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
        .collect();
    let mut next_ext: u32 = 1;
    for id in pin_ids {
        let current = match m.nodes[&id].properties.get(&*sym::PORT_LABEL) {
            Some(Literal::String(s)) => s.clone(),
            _ => String::new(),
        };
        let base = if current.is_empty() {
            loop {
                let candidate = format!("ext{next_ext}");
                next_ext += 1;
                if !used.contains(&candidate) {
                    break candidate;
                }
            }
        } else {
            current
        };
        let label = if used.contains(&base) {
            let mut n = 2;
            loop {
                let candidate = format!("{base}{n}");
                if !used.contains(&candidate) {
                    break candidate;
                }
                n += 1;
            }
        } else {
            base
        };
        used.insert(label.clone());
        let node = m.nodes.get_mut(&id).unwrap();
        Arc::make_mut(&mut node.properties).insert(*sym::PORT_LABEL, Literal::String(label));
    }
}
