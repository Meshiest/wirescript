//! `src/wirescript/analyze/cycle.ts`.
//!
//! Every cycle in the wire graph must pass through a tick-crossing
//! barrier (BufferTicks/BufferSeconds/QueueTicks/QueueSeconds/EdgeDetector)
//! per the design plan. We run Tarjan's SCC and for every non-trivial
//! component verify a barrier is present.

use std::collections::{HashMap, HashSet};

use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::{Module, Node, NodeId};

const BARRIER_CLASSES: &[&str] = &[
    "BrickComponentType_WireGraphPseudo_BufferTicks",
    "BrickComponentType_WireGraphPseudo_BufferSeconds",
    "BrickComponentType_WireGraphPseudo_QueueTicks",
    "BrickComponentType_WireGraphPseudo_QueueSeconds",
];

#[derive(Debug, Default)]
pub struct CycleResult {
    /// SCCs with more than one node (or a self-loop), in discovery order.
    pub strongly_connected: Vec<Vec<NodeId>>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Recursively analyse a module and every chip sub-module.
pub fn analyze_cycles(root: &Module) -> CycleResult {
    let mut out = CycleResult::default();
    analyze_module(root, &mut out);
    for child_module in root.chips.values() {
        let sub = analyze_cycles(child_module);
        out.diagnostics.extend(sub.diagnostics);
        out.strongly_connected.extend(sub.strongly_connected);
    }
    out
}

fn analyze_module(m: &Module, out: &mut CycleResult) {
    let adj = build_adjacency(m);
    for scc in tarjan(&adj) {
        if scc.len() == 1 && !has_self_loop(&adj, &scc[0]) {
            continue;
        }
        let has_barrier = scc.iter().any(|id| {
            m.nodes
                .get(id)
                .map(|n| BARRIER_CLASSES.contains(&n.gate_class))
                .unwrap_or(false)
        });
        if !has_barrier {
            out.diagnostics.push(emit_cycle_diagnostic(m, &scc));
        }
        out.strongly_connected.push(scc);
    }
}

fn build_adjacency(m: &Module) -> HashMap<NodeId, Vec<NodeId>> {
    let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for id in m.nodes.keys() {
        adj.insert(*id, Vec::new());
    }
    for w in &m.wires {
        adj.entry(w.source.node_id)
            .or_default()
            .push(w.target.node_id);
    }
    adj
}

fn has_self_loop(adj: &HashMap<NodeId, Vec<NodeId>>, id: &NodeId) -> bool {
    adj.get(id)
        .map(|v| v.iter().any(|t| t == id))
        .unwrap_or(false)
}

fn emit_cycle_diagnostic(m: &Module, scc: &[NodeId]) -> Diagnostic {
    let anchor = earliest_source_node(m, scc);
    let range = anchor
        .map(|n| n.source_range.clone())
        .unwrap_or_else(|| SourceRange::new("<ir>", Default::default(), Default::default()));
    Diagnostic {
        severity: Severity::Error,
        code: "WS005".to_string(),
        message:
            "cycle in the wire graph has no barrier (Buffer/Queue/EdgeDetector required to break it)"
                .to_string(),
        range,
    }
}

fn earliest_source_node<'a>(m: &'a Module, ids: &[NodeId]) -> Option<&'a Node> {
    let mut earliest: Option<&Node> = None;
    for id in ids {
        let Some(n) = m.nodes.get(id) else { continue };
        match earliest {
            None => earliest = Some(n),
            Some(e) if n.source_range.start.offset < e.source_range.start.offset => {
                earliest = Some(n)
            }
            _ => {}
        }
    }
    earliest
}

// ---------- Tarjan's SCC ----------

fn tarjan(adj: &HashMap<NodeId, Vec<NodeId>>) -> Vec<Vec<NodeId>> {
    let mut state = TarjanState {
        idx: HashMap::new(),
        low: HashMap::new(),
        on_stack: HashSet::new(),
        stack: Vec::new(),
        counter: 0,
        result: Vec::new(),
    };
    // Visit in deterministic order so test output is stable.
    let mut keys: Vec<&NodeId> = adj.keys().collect();
    keys.sort_by_key(|id| id.0);
    for v in keys {
        if !state.idx.contains_key(v) {
            strongconnect(adj, v, &mut state);
        }
    }
    state.result
}

struct TarjanState {
    idx: HashMap<NodeId, i32>,
    low: HashMap<NodeId, i32>,
    on_stack: HashSet<NodeId>,
    stack: Vec<NodeId>,
    counter: i32,
    result: Vec<Vec<NodeId>>,
}

fn strongconnect(adj: &HashMap<NodeId, Vec<NodeId>>, v: &NodeId, s: &mut TarjanState) {
    s.idx.insert(*v, s.counter);
    s.low.insert(*v, s.counter);
    s.counter += 1;
    s.stack.push(*v);
    s.on_stack.insert(*v);

    let neighbors: Vec<NodeId> = adj.get(v).cloned().unwrap_or_default();
    for w in &neighbors {
        if !s.idx.contains_key(w) {
            strongconnect(adj, w, s);
            let lw = s.low[w];
            let lv = s.low[v];
            s.low.insert(*v, lv.min(lw));
        } else if s.on_stack.contains(w) {
            let iw = s.idx[w];
            let lv = s.low[v];
            s.low.insert(*v, lv.min(iw));
        }
    }

    if s.low[v] == s.idx[v] {
        let mut scc = Vec::new();
        loop {
            let w = s.stack.pop().expect("stack must contain v");
            s.on_stack.remove(&w);
            let is_v = w == *v;
            scc.push(w);
            if is_v {
                break;
            }
        }
        s.result.push(scc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Module, Node, NodeKind, PortRef, Wire};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn gate(class: &'static str) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class: class,
            properties: Arc::new(HashMap::new()),
            ports: Arc::new(crate::GateIO::default()),
            source_range: SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        }
    }

    fn wire_between(src: NodeId, dst: NodeId) -> Wire {
        Wire {
            source: PortRef {
                node_id: src,
                port: crate::ir::port_registry::WirePort::from_name("Output"),
            },
            target: PortRef {
                node_id: dst,
                port: crate::ir::port_registry::WirePort::from_name("Input"),
            },
        }
    }

    #[test]
    fn no_cycles_no_diags() {
        let mut m = Module::new("clean");
        let a = gate("X");
        let b = gate("Y");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(wire_between(a_id, b_id));
        let r = analyze_cycles(&m);
        assert!(r.strongly_connected.is_empty());
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn cycle_without_barrier_diags() {
        let mut m = Module::new("loop");
        let a = gate("X");
        let b = gate("Y");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(wire_between(a_id, b_id));
        m.add_wire(wire_between(b_id, a_id));
        let r = analyze_cycles(&m);
        assert_eq!(r.strongly_connected.len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].code, "WS005");
    }

    #[test]
    fn buffer_breaks_cycle() {
        let mut m = Module::new("buffered");
        let a = gate("X");
        let buf = gate("BrickComponentType_WireGraphPseudo_BufferTicks");
        let a_id = a.id;
        let buf_id = buf.id;
        m.add_node(a);
        m.add_node(buf);
        m.add_wire(wire_between(a_id, buf_id));
        m.add_wire(wire_between(buf_id, a_id));
        let r = analyze_cycles(&m);
        assert_eq!(r.strongly_connected.len(), 1);
        assert!(r.diagnostics.is_empty(), "barrier should suppress the diag");
    }
}
