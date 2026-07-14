//! `src/wirescript/analyze/cycle.ts`.
//!
//! Every cycle in the wire graph must pass through a tick-crossing
//! barrier (BufferTicks/BufferSeconds/QueueTicks/QueueSeconds/EdgeDetector)
//! per the design plan. We run Tarjan's SCC and for every non-trivial
//! component verify a barrier is present.

use std::collections::{HashMap, HashSet};

use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::port_registry::WirePort;
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
            out.diagnostics.push(emit_cycle_diagnostic(m, &adj, &scc));
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
        // `_Layout` wires are cosmetic row/inline placement hints, not signal flow —
        // emit.rs skips them when writing real connections (see its Pass 3), so they
        // must not count toward wire-graph cycles either. Left in, a chip container's
        // layout edges to the nodes it holds form loops that raise false WS005 errors.
        if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
            continue;
        }
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

fn emit_cycle_diagnostic(
    m: &Module,
    adj: &HashMap<NodeId, Vec<NodeId>>,
    scc: &[NodeId],
) -> Diagnostic {
    let anchor = earliest_source_node(m, scc);
    let range = anchor
        .map(|n| n.source_range.clone())
        .unwrap_or_else(|| SourceRange::new("<ir>", Default::default(), Default::default()));

    // Trace a representative loop through the component so the message shows the
    // actual back-edge path — each gate's class + source location + debug note, and
    // the wire PORTS on every hop (so exec edges and data edges, and exactly which
    // pin closes the loop, are visible). This is what turns an un-actionable WS005
    // into a fixable one.
    let loop_path = find_cycle_path(adj, scc);
    let mut trace = String::new();
    for (i, id) in loop_path.iter().enumerate() {
        let arrow = if i == 0 {
            String::new()
        } else {
            edge_label(m, loop_path[i - 1], *id)
        };
        match m.nodes.get(id) {
            Some(n) => {
                let note = n.note.map(|s| format!(" \"{s}\"")).unwrap_or_default();
                trace.push_str(&format!(
                    "\n    {arrow}{}{note} @ {}:{}:{}",
                    short_class(n.gate_class),
                    n.source_range.file,
                    n.source_range.start.line,
                    n.source_range.start.col,
                ));
            }
            None => trace.push_str(&format!("\n    {arrow}<unknown node {}>", id.0)),
        }
    }
    // Close the loop back to the first node so the cycle reads unambiguously.
    if let (Some(&last), Some(&first)) = (loop_path.last(), loop_path.first()) {
        if let Some(fnode) = m.nodes.get(&first) {
            trace.push_str(&format!(
                "\n    {}(back to {})",
                edge_label(m, last, first),
                short_class(fnode.gate_class),
            ));
        }
    }
    let extra = scc.len().saturating_sub(loop_path.len());
    let more = if extra > 0 {
        format!("\n    ({extra} more gate(s) share this strongly-connected component)")
    } else {
        String::new()
    };

    Diagnostic {
        severity: Severity::Error,
        code: "WS005".to_string(),
        message: format!(
            "cycle in the wire graph has no barrier (Buffer/Queue/EdgeDetector required to break it) — \
             {} gate(s) in the loop:{trace}{more}",
            loop_path.len(),
        ),
        range,
    }
}

/// Recover a representative simple cycle inside an SCC. Because the component is
/// strongly connected, greedily following the first in-component out-edge from any
/// member is guaranteed to revisit a node, and the segment from that first visit is
/// a cycle. Returns the node ids in loop order (the last edges back to the first);
/// a single-node vec for a self-loop.
fn find_cycle_path(adj: &HashMap<NodeId, Vec<NodeId>>, scc: &[NodeId]) -> Vec<NodeId> {
    if scc.len() == 1 {
        return vec![scc[0]]; // self-loop
    }
    let members: HashSet<NodeId> = scc.iter().copied().collect();
    let mut walk: Vec<NodeId> = Vec::new();
    let mut seen_at: HashMap<NodeId, usize> = HashMap::new();
    let mut cur = scc[0];
    loop {
        if let Some(&p) = seen_at.get(&cur) {
            return walk[p..].to_vec();
        }
        seen_at.insert(cur, walk.len());
        walk.push(cur);
        match adj
            .get(&cur)
            .and_then(|ns| ns.iter().copied().find(|w| members.contains(w)))
        {
            Some(w) => cur = w,
            None => return walk, // defensive: a real SCC member always has an in-SCC edge
        }
    }
}

/// Label the edge `src -> dst` with a representative wire's ports, e.g.
/// `--[ExecOut -> ExecIn]--> ` (exec) vs `--[Value -> B]--> ` (data). Falls back to
/// a plain `-> ` when no direct wire is found (shouldn't happen along a real path).
fn edge_label(m: &Module, src: NodeId, dst: NodeId) -> String {
    match m
        .wires
        .iter()
        .find(|w| w.source.node_id == src && w.target.node_id == dst)
    {
        Some(w) => format!("--[{} -> {}]--> ", w.source.port.as_str(), w.target.port.as_str()),
        None => "-> ".to_string(),
    }
}

/// Strip the noisy `BrickComponentType_*` prefix so the trace reads `Math_Add`
/// instead of `BrickComponentType_WireGraph_Math_Add`.
fn short_class(class: &str) -> &str {
    for prefix in [
        "BrickComponentType_WireGraphPseudo_",
        "BrickComponentType_WireGraph_",
        "BrickComponentType_Internal_",
        "BrickComponentType_",
    ] {
        if let Some(rest) = class.strip_prefix(prefix) {
            return rest;
        }
    }
    class
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
    fn cycle_diag_lists_loop_members() {
        // A → B → A: the WS005 message must name both gates and show the path so
        // the loop is actually diagnosable.
        let mut m = Module::new("loop");
        let a = gate("Alpha");
        let b = gate("Beta");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(wire_between(a_id, b_id));
        m.add_wire(wire_between(b_id, a_id));
        let r = analyze_cycles(&m);
        assert_eq!(r.diagnostics.len(), 1);
        let msg = &r.diagnostics[0].message;
        assert!(msg.contains("Alpha"), "should name the gate: {msg}");
        assert!(msg.contains("Beta"), "should name the gate: {msg}");
        assert!(msg.contains("->"), "should show the loop path: {msg}");
        assert!(msg.contains("2 gate(s) in the loop"), "should count the loop: {msg}");
    }

    fn layout_wire(src: NodeId, dst: NodeId) -> Wire {
        Wire {
            source: PortRef {
                node_id: src,
                port: crate::ir::port_registry::WirePort::Layout,
            },
            target: PortRef {
                node_id: dst,
                port: crate::ir::port_registry::WirePort::Layout,
            },
        }
    }

    #[test]
    fn layout_wires_are_not_signal_cycles() {
        // A loop made only of `_Layout` placement wires is cosmetic, not signal flow,
        // and must NOT raise WS005 (regression: chip-inline layout edges did).
        let mut m = Module::new("layout-loop");
        let a = gate("Alpha");
        let b = gate("Beta");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(layout_wire(a_id, b_id));
        m.add_wire(layout_wire(b_id, a_id));
        let r = analyze_cycles(&m);
        assert!(
            r.diagnostics.is_empty(),
            "layout wires must not trip the cycle check: {:?}",
            r.diagnostics
        );
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
