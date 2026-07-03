//! Per-region DAG layout.
//!
//! Takes one leaf `Region` plus the wires whose endpoints both sit
//! inside it and assigns local `(dx, dy)` coordinates:
//!   1. Build a `DiGraphMap` over the region's own nodes.
//!   2. Tarjan SCC → for each non-trivial SCC, drop one feedback edge.
//!      Preference: a `Type::Exec` edge whose target node `is_buffer()`;
//!      fallback: the edge with the latest source position (any kind),
//!      logged via the returned diagnostic list.
//!   3. Weakly-connected-components split so disconnected subgraphs
//!      never overlap at (0, 0).
//!   4. Per-WCC: topological sort + longest-path depth → `dx` level.
//!   5. Within each level, sort by `(source_range.start, node_id)` → `dy`.
//!
//! This module intentionally does NOT know about scope kinds (handler
//! headers, chip I/O, loop feedback columns). Those are layered on top
//! by [`crate::layout::compose`] in Phase 5.

use std::collections::{HashMap, HashSet};

use petgraph::algo::tarjan_scc;
use petgraph::graphmap::DiGraphMap;
use petgraph::unionfind::UnionFind;

use crate::ir::{NodeId, Wire};

use super::region::Region;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LocalPlacement {
    pub dx: i32,
    pub dy: i32,
}

#[derive(Debug)]
pub struct RegionLayout {
    pub local: HashMap<NodeId, LocalPlacement>,
    /// (width, height) in cell units; `width = max(dx) + 1`, same for height.
    pub bbox: (i32, i32),
    /// Edges dropped from the DAG for ordering purposes. Still emitted by
    /// the brick emitter — these are just *not* used when assigning
    /// longest-path levels.
    pub feedback_edges: Vec<(NodeId, NodeId)>,
    /// Non-fatal warnings raised during layout (e.g., a non-trivial SCC
    /// with no `is_buffer()` target — likely a typecheck bug).
    pub warnings: Vec<String>,
}

/// Lay out a leaf region. `wires` should be the full module's wire list;
/// this function filters to the subset with both endpoints inside
/// `region.own_nodes`.
pub fn layout_leaf(region: &Region<'_>, wires: &[Wire]) -> RegionLayout {
    // Index the region's nodes for O(1) lookup during cycle breaking
    // (we need `is_buffer()` and source-range comparisons by node id).
    let node_ids: Vec<&NodeId> = region.own_nodes.iter().map(|n| &n.id).collect();
    let node_set: HashSet<&NodeId> = node_ids.iter().copied().collect();
    let node_by_id: HashMap<&NodeId, &crate::ir::Node> =
        region.own_nodes.iter().map(|n| (&n.id, *n)).collect();

    // Restrict wires to edges fully inside this region. Preserve input
    // order; we'll use wire source-range tiebreaks later.
    let in_scope: Vec<&Wire> = wires
        .iter()
        .filter(|w| node_set.contains(&w.source.node_id) && node_set.contains(&w.target.node_id))
        .collect();

    // Build the graph.
    let mut g: DiGraphMap<&NodeId, ()> = DiGraphMap::new();
    for id in &node_ids {
        g.add_node(id);
    }
    for w in &in_scope {
        // Self-loops on a single node are treated as their own feedback
        // edge during SCC processing below.
        g.add_edge(&w.source.node_id, &w.target.node_id, ());
    }

    // SCC + feedback-edge selection. Drop exactly one edge per non-trivial SCC.
    let mut feedback_edges: Vec<(NodeId, NodeId)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let sccs = tarjan_scc(&g);
    for scc in &sccs {
        let has_cycle = scc.len() > 1
            || scc
                .first()
                .map(|n| g.contains_edge(*n, *n))
                .unwrap_or(false);
        if !has_cycle {
            continue;
        }

        let scc_set: HashSet<&NodeId> = scc.iter().copied().collect();

        // Indices into `in_scope` for edges entirely inside this SCC.
        let cand: Vec<usize> = in_scope
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                scc_set.contains(&w.source.node_id) && scc_set.contains(&w.target.node_id)
            })
            .map(|(i, _)| i)
            .collect();
        if cand.is_empty() {
            continue;
        }

        // Prefer edges whose target is a buffer (the feedback edge of a
        // one-tick delay). Otherwise, take any edge.
        let preferred: Vec<usize> = cand
            .iter()
            .copied()
            .filter(|&i| {
                let w = in_scope[i];
                node_by_id
                    .get(&w.target.node_id)
                    .map(|n| n.is_buffer())
                    .unwrap_or(false)
            })
            .collect();

        let pool = if preferred.is_empty() {
            warnings.push(format!(
                "layout: non-trivial SCC without a buffer edge in region {}; \
                 falling back to latest-source-range edge",
                region.id
            ));
            cand
        } else {
            preferred
        };
        let chosen_idx = *pool
            .iter()
            .max_by(|&&a, &&b| {
                let wa = in_scope[a];
                let wb = in_scope[b];
                let oa = node_by_id
                    .get(&wa.source.node_id)
                    .map(|n| n.source_range.start.offset)
                    .unwrap_or(0);
                let ob = node_by_id
                    .get(&wb.source.node_id)
                    .map(|n| n.source_range.start.offset)
                    .unwrap_or(0);
                oa.cmp(&ob).then_with(|| {
                    wa.source
                        .node_id
                        .cmp(&wb.source.node_id)
                        .then_with(|| wa.target.node_id.cmp(&wb.target.node_id))
                })
            })
            .expect("pool is non-empty");
        let chosen = in_scope[chosen_idx];

        g.remove_edge(&chosen.source.node_id, &chosen.target.node_id);
        feedback_edges.push((chosen.source.node_id, chosen.target.node_id));
    }

    // Weakly-connected-components split. Use a union-find seeded with the
    // current (post-feedback-removal) edge set, walked as undirected.
    let mut idx: HashMap<&NodeId, usize> = HashMap::new();
    for (i, id) in node_ids.iter().enumerate() {
        idx.insert(*id, i);
    }
    let mut uf = UnionFind::<usize>::new(node_ids.len());
    for (u, v, _) in g.all_edges() {
        if let (Some(&ui), Some(&vi)) = (idx.get(&u), idx.get(&v)) {
            uf.union(ui, vi);
        }
    }
    // Bucket nodes by root.
    let mut wccs: HashMap<usize, Vec<&NodeId>> = HashMap::new();
    for (i, id) in node_ids.iter().enumerate() {
        wccs.entry(uf.find(i)).or_default().push(*id);
    }
    // Deterministic WCC order: by the least source_range.start among each
    // WCC's nodes (then by least node_id).
    let mut wcc_list: Vec<(usize, Vec<&NodeId>)> = wccs.into_iter().collect();
    wcc_list
        .sort_by(|(_, a), (_, b)| wcc_sort_key(a, &node_by_id).cmp(&wcc_sort_key(b, &node_by_id)));

    // Per-WCC: longest-path depth (from sources) + y-rank by source order.
    // Accumulate placements into a global map with a vertical offset per WCC.
    let mut local: HashMap<NodeId, LocalPlacement> = HashMap::new();
    let mut y_offset: i32 = 0;
    let mut max_width: i32 = 0;

    for (_, mut wcc_nodes) in wcc_list {
        // Subgraph filter: only edges between wcc_nodes remain.
        let wcc_set: HashSet<&NodeId> = wcc_nodes.iter().copied().collect();

        // Longest-path depth: for each node in topo order, depth = max
        // over predecessors of (pred_depth + 1). Sources have depth 0.
        let topo_all =
            petgraph::algo::toposort(&g, None).expect("cycles have been broken before WCC layout");
        let topo: Vec<&NodeId> = topo_all
            .into_iter()
            .filter(|n| wcc_set.contains(n))
            .collect();

        let mut depth: HashMap<&NodeId, i32> = HashMap::new();
        for &n in &topo {
            let d = g
                .neighbors_directed(n, petgraph::Direction::Incoming)
                .filter(|p| wcc_set.contains(p))
                .map(|p| depth.get(&p).copied().unwrap_or(0) + 1)
                .max()
                .unwrap_or(0);
            depth.insert(n, d);
        }

        // Group by depth, sort siblings by source_range.start then id.
        let mut by_level: HashMap<i32, Vec<&NodeId>> = HashMap::new();
        for &n in &topo {
            by_level.entry(depth[&n]).or_default().push(n);
        }
        let mut levels: Vec<i32> = by_level.keys().copied().collect();
        levels.sort();

        let mut local_height = 0i32;
        for lvl in levels {
            let mut ids = by_level.remove(&lvl).unwrap();
            ids.sort_by(|a, b| {
                let na = node_by_id[a];
                let nb = node_by_id[b];
                na.source_range
                    .start
                    .offset
                    .cmp(&nb.source_range.start.offset)
                    .then_with(|| na.id.cmp(&nb.id))
            });
            for (rank, id) in ids.iter().enumerate() {
                local.insert(
                    *(*id),
                    LocalPlacement {
                        dx: lvl,
                        dy: y_offset + rank as i32,
                    },
                );
                local_height = local_height.max(rank as i32 + 1);
            }
            max_width = max_width.max(lvl + 1);
        }

        y_offset += local_height;
        // Gutter between WCCs.
        if !wcc_nodes.is_empty() {
            y_offset += 1;
        }
        wcc_nodes.clear();
    }

    // Isolated nodes that weren't in the toposort path (e.g., no edges
    // whatsoever AND not unioned into any WCC) still ended up in their
    // own singleton WCC above. Sanity-check: every own_node has a placement.
    for n in &region.own_nodes {
        local.entry(n.id).or_insert(LocalPlacement { dx: 0, dy: 0 });
    }

    // If the only WCC was empty (empty region), trim the trailing gutter.
    let height = y_offset.saturating_sub(1).max(0);
    let bbox = if region.own_nodes.is_empty() {
        (0, 0)
    } else {
        (max_width.max(1), height.max(1))
    };

    RegionLayout {
        local,
        bbox,
        feedback_edges,
        warnings,
    }
}

fn wcc_sort_key(
    nodes: &[&NodeId],
    node_by_id: &HashMap<&NodeId, &crate::ir::Node>,
) -> (usize, NodeId) {
    let mut min_offset = usize::MAX;
    let mut min_id: Option<&NodeId> = None;
    for id in nodes {
        if let Some(n) = node_by_id.get(id)
            && n.source_range.start.offset < min_offset
        {
            min_offset = n.source_range.start.offset;
            min_id = Some(id);
        }
        if min_id.is_none() || id < &min_id.unwrap() {
            min_id = Some(id);
        }
    }
    (min_offset, min_id.copied().unwrap_or(NodeId(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Pos, SourceRange};
    use crate::ir::port_registry::WirePort;
    use crate::ir::{GateIO, Literal, Module, Node, NodeKind, PortRef, ROOT_SCOPE_ID, ScopeId};
    use std::sync::Arc;

    fn make_node(gate_class: &'static str, offset: usize) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class,
            properties: Arc::new(HashMap::new()),
            ports: Arc::new(GateIO::default()),
            source_range: SourceRange {
                file: "t".into(),
                start: Pos {
                    offset,
                    line: 0,
                    col: 0,
                },
                end: Pos {
                    offset: offset + 1,
                    line: 0,
                    col: 0,
                },
            },
            chip_id: None,
            chain_id: None,
            scope_id: ROOT_SCOPE_ID,
            note: None,
        }
    }

    fn buffer_node(offset: usize) -> Node {
        make_node("BrickComponentType_Internal_Variable", offset)
    }

    fn make_wire(src: NodeId, dst: NodeId) -> Wire {
        Wire {
            source: PortRef {
                node_id: src,
                port: WirePort::Output,
            },
            target: PortRef {
                node_id: dst,
                port: WirePort::Input,
            },
        }
    }

    fn leaf_region<'a>(module: &'a Module) -> super::super::region::Region<'a> {
        super::super::region::build_region_tree(module)
    }

    fn into_module(nodes: Vec<Node>, wires: Vec<Wire>) -> Module {
        let mut m = Module::default();
        for n in nodes {
            m.nodes.insert(n.id, n);
        }
        m.wires = wires;
        m
    }

    #[test]
    fn linear_chain_gets_increasing_depth() {
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let c = make_node("G", 2);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let m = into_module(
            vec![a, b, c],
            vec![make_wire(a_id, b_id), make_wire(b_id, c_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(lay.local[&a_id].dx, 0);
        assert_eq!(lay.local[&b_id].dx, 1);
        assert_eq!(lay.local[&c_id].dx, 2);
        assert!(lay.feedback_edges.is_empty());
    }

    #[test]
    fn buffer_cycle_drops_buffer_edge_and_does_not_panic() {
        // a → buf → a  (buf is a Variable so is_buffer() = true)
        let a = make_node("G", 0);
        let buf = buffer_node(1);
        let a_id = a.id;
        let buf_id = buf.id;
        let m = into_module(
            vec![a, buf],
            vec![make_wire(a_id, buf_id), make_wire(buf_id, a_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        // The a → buf (Exec, target is buffer) edge is the preferred
        // feedback edge.
        assert_eq!(lay.feedback_edges.len(), 1);
        assert_eq!(lay.feedback_edges[0].1, buf_id);
        assert!(lay.warnings.is_empty());
    }

    #[test]
    fn non_buffer_cycle_falls_back_and_warns() {
        // Two regular gates forming a cycle — no buffer available.
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let a_id = a.id;
        let b_id = b.id;
        let m = into_module(
            vec![a, b],
            vec![make_wire(a_id, b_id), make_wire(b_id, a_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(lay.feedback_edges.len(), 1);
        assert_eq!(lay.warnings.len(), 1);
    }

    #[test]
    fn disconnected_subgraphs_dont_overlap() {
        // Two isolated pairs: (a → b) and (c → d). b's dy and d's dy
        // must differ (they're at the same dx=1 but in different WCCs).
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let c = make_node("G", 2);
        let d = make_node("G", 3);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let d_id = d.id;
        let m = into_module(
            vec![a, b, c, d],
            vec![make_wire(a_id, b_id), make_wire(c_id, d_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        // No two placements share the same (dx, dy).
        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        for p in lay.local.values() {
            assert!(
                seen.insert((p.dx, p.dy)),
                "duplicate placement ({}, {})",
                p.dx,
                p.dy
            );
        }
    }

    #[test]
    fn empty_region_has_zero_bbox() {
        let m = Module::default();
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(lay.bbox, (0, 0));
        assert!(lay.local.is_empty());
    }

    #[test]
    fn layout_is_deterministic() {
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let c = make_node("G", 2);
        let d = make_node("G", 3);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let d_id = d.id;
        let m = into_module(
            vec![a, b, c, d],
            vec![
                make_wire(a_id, b_id),
                make_wire(a_id, c_id),
                make_wire(b_id, d_id),
                make_wire(c_id, d_id),
            ],
        );
        let root = leaf_region(&m);
        let a = layout_leaf(&root, &m.wires);
        let b = layout_leaf(&root, &m.wires);
        assert_eq!(a.local, b.local);
        assert_eq!(a.bbox, b.bbox);
    }

    #[test]
    fn source_order_breaks_y_ties() {
        // Two nodes both at dx=0 (no predecessors) — the one with the
        // earlier source_range must get the smaller dy.
        let early = make_node("G", 0);
        let late = make_node("G", 100);
        let early_id = early.id;
        let late_id = late.id;
        let m = into_module(vec![early, late], vec![]);
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        // Both are their own WCCs → stacked vertically.
        let early_p = lay.local[&early_id];
        let late_p = lay.local[&late_id];
        assert!(early_p.dy < late_p.dy, "early should come first on y axis");
        // Unused-import guard for Literal/ScopeId.
        let _ = (Literal::Bool(false), ROOT_SCOPE_ID as ScopeId);
    }
}
