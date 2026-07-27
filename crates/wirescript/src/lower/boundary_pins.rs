//! Synthesized microchip boundary pins.
//!
//! Runs after fold/materialize/partition, before layout: every wire whose
//! endpoints live in different modules of the chip tree is rewired through
//! MicrochipInput/Output rerouter pins, one per wall crossed, so external
//! references are visible (and labeled) at each chip boundary.
//!
//! Exclusions — wires the pass must NOT touch:
//! - `gc::LITERAL` sources: literals are delivered by inlining into the
//!   target's data, not by routed wires; a rerouter cannot hold inlined data.
//! - `WirePort::Layout` synthetic edges.
//!
//! `VarRef`/`ArrayVarRef` crossings ARE rewired — emit traces variable
//! bindings through rerouter pins (declared ref/array chip params already
//! route this way, see call.rs build_chip_module's non-captured ref/array
//! handling). Rewiring changes which external node ids each module's wires
//! reference, so scope_captures is recomputed bottom-up at the end.
//!
//! A crossing whose deep endpoint is ALREADY a MicrochipInput/Output pin at
//! its own wall (a declared chip param/output from ordinary lowering, or a
//! pin this pass itself added on an earlier call) is reused rather than
//! wrapped in a redundant second pin — this is what makes the pass
//! idempotent and keeps it from double-pinning normal declared chip I/O.
//! When the source and target modules are genuine siblings (neither owns
//! the other, i.e. neither is the crossing's LCA), the middle segment is a
//! direct MicrochipOutput → MicrochipInput wire held in the LCA module —
//! the exact shape ordinary lowering already gives a declared pin→pin
//! chip-call wire, so declared chip I/O keeps its shape class and the pass
//! stays idempotent over it.

use std::sync::Arc;

use crate::collections::{HashMap, HashSet};
use crate::intern::sym;
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::ir::{
    GateIO, Literal, Module, Node, NodeId, NodeKind, PortRef, PortSpec, ROOT_SCOPE_ID, Type, Wire,
};

/// Chip-node path from the root module; the root itself is `[]`.
type ModPath = Vec<NodeId>;

pub fn synthesize_boundary_pins(root: &mut Module) {
    let mut owner: HashMap<NodeId, ModPath> = HashMap::default();
    index_owners(root, &mut Vec::new(), &mut owner);

    let mut crossings: Vec<Wire> = Vec::new();
    extract_crossings(root, &owner, &mut crossings);
    if crossings.is_empty() {
        return;
    }

    // (owning module, feeding PortRef) → pin node id. Keying on the feeder
    // dedupes: N consumers of one external source share one pin per wall.
    let mut pins: HashMap<(ModPath, PortRef), NodeId> = HashMap::default();
    for wire in crossings {
        rewire(root, wire, &mut owner, &mut pins);
    }

    assign_pin_labels(root);
    refresh_scope_captures(root);
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

fn index_owners(m: &Module, path: &mut ModPath, owner: &mut HashMap<NodeId, ModPath>) {
    for id in m.nodes.keys() {
        owner.insert(*id, path.clone());
    }
    let mut kids: Vec<NodeId> = m.chips.keys().copied().collect();
    kids.sort();
    for k in kids {
        path.push(k);
        index_owners(&m.chips[&k], path, owner);
        path.pop();
    }
}

/// Remove every rewirable crossing from all wire lists, in deterministic
/// (module DFS, list order) order, and collect them. Two phases because the
/// literal-source check needs a read-only view while `retain` holds the
/// mutable borrow.
fn extract_crossings(root: &mut Module, owner: &HashMap<NodeId, ModPath>, out: &mut Vec<Wire>) {
    let mut literal_ids: HashSet<NodeId> = Default::default();
    collect_literals(root, &mut literal_ids);
    fn walk(
        m: &mut Module,
        owner: &HashMap<NodeId, ModPath>,
        literal_ids: &HashSet<NodeId>,
        out: &mut Vec<Wire>,
    ) {
        m.wires.retain(|w| {
            if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
                return true;
            }
            if literal_ids.contains(&w.source.node_id) {
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
        let mut kids: Vec<NodeId> = m.chips.keys().copied().collect();
        kids.sort();
        for k in kids {
            walk(m.chips.get_mut(&k).unwrap(), owner, literal_ids, out);
        }
    }
    walk(root, owner, &literal_ids, out);
}

fn collect_literals(m: &Module, out: &mut HashSet<NodeId>) {
    for (id, n) in &m.nodes {
        if n.gate_class == gc::LITERAL {
            out.insert(*id);
        }
    }
    for c in m.chips.values() {
        collect_literals(c, out);
    }
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
    // Upward legs: each module from the source's, up to (not incl.) the LCA.
    // The deepest wall (mpath == sp) is skipped — reusing `wire.source`
    // itself — when it's already a MicrochipOutput there (a declared chip
    // output, or a pin an earlier run of this pass already added).
    for k in (lca + 1..=sp.len()).rev() {
        let mpath = sp[..k].to_vec();
        if mpath == sp
            && module_at(root, &mpath)
                .nodes
                .get(&wire.source.node_id)
                .is_some_and(|n| n.gate_class == gc::MICROCHIP_OUTPUT)
        {
            feeder_path = mpath;
            continue;
        }
        let (pin, is_new) = pin_for(root, owner, pins, &mpath, feeder, NodeKind::Output, &ty, &label);
        // `pin_for` dedupes on (mpath, feeder): a pin reused by another
        // crossing already has its feeding wire from that earlier call, so
        // only a freshly created pin needs one now — otherwise two
        // crossings sharing a feeder would both target the same
        // (pin, RerInput) tuple.
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

    // Downward legs: each module from just below the LCA down to the
    // target's. The deepest wall (mpath == tp) is skipped — reusing
    // `wire.target` itself — when it's already a MicrochipInput there.
    for k in lca + 1..=tp.len() {
        let mpath = tp[..k].to_vec();
        if mpath == tp
            && module_at(root, &mpath)
                .nodes
                .get(&wire.target.node_id)
                .is_some_and(|n| n.gate_class == gc::MICROCHIP_INPUT)
        {
            break;
        }
        let (pin, is_new) = pin_for(root, owner, pins, &mpath, feeder, NodeKind::Input, &ty, &label);
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

fn module_at<'a>(root: &'a Module, path: &[NodeId]) -> &'a Module {
    let mut m = root;
    for c in path {
        m = &m.chips[c];
    }
    m
}
fn module_at_mut<'a>(root: &'a mut Module, path: &[NodeId]) -> &'a mut Module {
    let mut m = root;
    for c in path {
        m = m.chips.get_mut(c).unwrap();
    }
    m
}

/// New wires live in the LCA module of their endpoints — the parent for a
/// parent<->child-pin hop, the module itself for a local hop.
fn push_wire_at_lca(root: &mut Module, a: &[NodeId], b: &[NodeId], source: PortRef, target: PortRef) {
    let lca = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    module_at_mut(root, &a[..lca.min(a.len())])
        .wires
        .push(Wire { source, target });
}

/// Returns the pin's id and whether it was just created (`false` means an
/// earlier crossing in this same pass run already created and fed it, via
/// the `(mpath, feeder)` dedup key — the caller must not feed it again).
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
    let node = Node {
        id,
        kind,
        gate_class,
        properties: Arc::new(props),
        ports: Arc::new(GateIO {
            inputs: vec![PortSpec {
                name: *sym::RER_INPUT,
                ty: ty.clone(),
            }],
            outputs: vec![PortSpec {
                name: *sym::RER_OUTPUT,
                ty: ty.clone(),
            }],
        }),
        source_range: Default::default(),
        chip_id: None,
        chain_id: None,
        scope_id: ROOT_SCOPE_ID,
        note: Some("boundary_pin"),
    };
    let m = module_at_mut(root, mpath);
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
    module_at(root, sp)
        .nodes
        .get(&src.node_id)?
        .ports
        .outputs
        .iter()
        .find(|p| WirePort::from_name(crate::intern::resolve(p.name)) == src.port)
        .map(|p| p.ty.clone())
}

/// The referenced identifier, when the source node has one — `NAME_LABEL`
/// (a var/array/param's own declared name) first, then `PORT_LABEL` (an
/// upstream pin's already-resolved label). Returns an empty string when
/// neither is present; callers must not fall back to `Node.note` or the
/// gate class, since both are internal implementation strings, not
/// something a player would recognize in-game. `assign_pin_labels` turns
/// any empty result into a synthetic name after all pins for this module
/// have been created.
fn derive_label(root: &Module, sp: &[NodeId], src: NodeId) -> String {
    let Some(n) = module_at(root, sp).nodes.get(&src) else {
        return String::new();
    };
    let get_str = |k: crate::intern::Sym| match n.properties.get(&k) {
        Some(Literal::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    get_str(*sym::NAME_LABEL)
        .or_else(|| get_str(*sym::PORT_LABEL))
        .unwrap_or_default()
}

/// Finalizes every synthesized pin's `PORT_LABEL` for one module: pins
/// `derive_label` couldn't name (empty label) get a short synthetic name
/// (`ext1`, `ext2`, …), and any remaining collision — two pins in the same
/// module landing on the same label, synthetic or identifier-derived — is
/// resolved by appending the lowest free numeric suffix. Assignment order is
/// each pin's `NodeId`, which reflects creation order (`pin_for` calls
/// `NodeId::fresh()` in the crossing-processing order `extract_crossings`
/// fixed), so runs over the same input are stable.
///
/// The taken-name set starts from the module's DECLARED ports, not just the
/// pins: a chip with a `score` parameter that also reads an outer `score`
/// would otherwise put two ports named `score` on the same edge.
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
            matches!(n.kind, NodeKind::Input | NodeKind::Output)
                && n.note != Some("boundary_pin")
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

    let mut kids: Vec<NodeId> = m.chips.keys().copied().collect();
    kids.sort();
    for k in kids {
        assign_pin_labels(m.chips.get_mut(&k).unwrap());
    }
}
