//! The wire-graph view of a module: who produces a value, who consumes it,
//! and which nodes sit on a line's exec spine.

use super::*;

pub(super) struct Adjacency {
    /// node -> nodes that consume its output (wire targets).
    pub(super) consumers: HashMap<NodeId, Vec<NodeId>>,
    /// node -> nodes that produce into it (wire sources).
    pub(super) producers: HashMap<NodeId, Vec<NodeId>>,
    /// `producers` minus every wire landing on an exec input — the operand
    /// graph, without the sequencing graph. See [`line_groups`].
    pub(super) value_producers: HashMap<NodeId, Vec<NodeId>>,
}

/// True when the node has an exec input at all — the node-side reading of
/// the same port test [`targets_exec`] applies to a wire.
pub(super) fn takes_exec_input(node: &Node) -> bool {
    node.ports.inputs.iter().any(|p| p.ty == Type::Exec)
}

/// True when a node belongs to its line's EXEC SPINE: it takes an exec
/// input AND nothing on its own line reads the value it produces, so it is
/// the statement's sink rather than a step in an expression.
///
/// `in_line_consumers` is the node's in-line value consumers — the entry
/// indices `line_groups` derives from `value_producers` restricted to the
/// line's own entries.
///
/// The second clause is what keeps a variable read out of the spine. An
/// `Exec_Var_Get` inside an expression takes an exec input too, but its
/// value feeds an operator on the same line, so it belongs to the value
/// flow: it stays in the depth columns and stays horizontal. A `Var_Set`,
/// `PrintToConsole` or `ArrayVar_Push` reads nothing back in line and heads
/// its statement on the left.
///
/// The column pin and the rotation decision both read this, so they cannot
/// drift apart.
pub(super) fn on_exec_spine(node: &Node, in_line_consumers: &[usize]) -> bool {
    takes_exec_input(node) && in_line_consumers.is_empty()
}

/// True for the gates that form a line's exec spine and so face down.
///
/// Deliberately narrower than [`on_exec_spine`] alone: a `Chip` node can
/// also take an exec input, but the microchip shell is emitted through a
/// separate path that hardcodes its 1×1 offsets, and its interior is a
/// distinct grid entity carrying its own transform. Rotating it there
/// would put layout and emit out of step — the one failure this whole
/// mechanism exists to avoid — for a brick with no facing to speak of.
pub(super) fn is_spine_exec_gate(node: &Node, in_line_consumers: &[usize]) -> bool {
    node.kind == NodeKind::Gate && on_exec_spine(node, in_line_consumers)
}

/// True when `target` names an exec input on its node — an exec-chain edge
/// rather than an operand edge.
pub(super) fn targets_exec(module: &Module, target: &PortRef) -> bool {
    module
        .nodes
        .get(&target.node_id)
        .is_some_and(|n| {
            n.ports.inputs.iter().any(|p| {
                p.ty == Type::Exec && crate::intern::resolve(p.name) == target.port.as_str()
            })
        })
}

pub(super) fn build_adjacency(module: &Module) -> Adjacency {
    let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    let mut producers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    let mut value_producers: HashMap<NodeId, Vec<NodeId>> = HashMap::default();
    for w in &module.wires {
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
        consumers
            .entry(w.source.node_id)
            .or_default()
            .push(w.target.node_id);
        producers
            .entry(w.target.node_id)
            .or_default()
            .push(w.source.node_id);
        if !targets_exec(module, &w.target) {
            value_producers
                .entry(w.target.node_id)
                .or_default()
                .push(w.source.node_id);
        }
    }
    Adjacency {
        consumers,
        producers,
        value_producers,
    }
}

/// BFS from a homeless node over the wire graph — consumers before
/// producers, both sorted by `(start.offset, id)` — stopping at the
/// first node with a literal line. Traverses through other homeless
/// nodes transitively; a visited set guards against cycles.
pub(super) fn adopt_line(
    start: NodeId,
    module: &Module,
    adjacency: &Adjacency,
    literal_line: &HashMap<NodeId, i32>,
) -> Option<i32> {
    let offset_of = |id: &NodeId| {
        module
            .nodes
            .get(id)
            .map(|n| n.source_range.start.offset)
            .unwrap_or(usize::MAX)
    };

    let mut visited: HashSet<NodeId> = HashSet::default();
    visited.insert(start);
    let mut queue: VecDeque<NodeId> = VecDeque::new();
    queue.push_back(start);

    while let Some(cur) = queue.pop_front() {
        let mut neighbors: Vec<NodeId> = Vec::new();
        if let Some(cs) = adjacency.consumers.get(&cur) {
            let mut v = cs.clone();
            v.sort_by_key(|id| (offset_of(id), *id));
            neighbors.extend(v);
        }
        if let Some(ps) = adjacency.producers.get(&cur) {
            let mut v = ps.clone();
            v.sort_by_key(|id| (offset_of(id), *id));
            neighbors.extend(v);
        }
        for nb in neighbors {
            if !visited.insert(nb) {
                continue;
            }
            if let Some(&line) = literal_line.get(&nb) {
                return Some(line);
            }
            queue.push_back(nb);
        }
    }
    None
}
