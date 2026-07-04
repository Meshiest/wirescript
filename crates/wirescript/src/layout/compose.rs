//! Region composition.
//!
//! Walks the region tree bottom-up. Each call to [`layout_region`]
//! returns a [`RegionLayout`] whose local coordinates sit in the region's
//! own frame (origin = top-left). Parents stitch children using the
//! **virtual-row model**: own nodes and child regions are merged into a
//! single source-ordered stream, and each entry becomes one vertical
//! row. IfGroup is the only special case today — its three children
//! (IfCond / IfThen / IfElse) are placed with IfCond above and
//! IfThen / IfElse side-by-side below.

use std::collections::{HashMap, HashSet};

use crate::ir::{Node, NodeId, ScopeKind, Wire};

use super::dag::{LocalPlacement, RegionLayout, layout_leaf};
use super::region::Region;

/// Vertical gutter between sequential items in a virtual-row stack, in cells.
const ROW_GUTTER: i32 = 2;
/// Horizontal gap between the `then` and `else` columns of an IfGroup, in cells.
const IF_BRANCH_GAP: i32 = 3;

/// Lay out `region` (with its descendants) into a single local frame.
///
/// The returned `local` map contains placements for **every** node in
/// this region and its descendants, relative to this region's top-left.
pub fn layout_region(region: &Region<'_>, wires: &[Wire]) -> RegionLayout {
    if matches!(region.info.kind, ScopeKind::IfGroup) {
        return compose_if_group(region, wires);
    }
    compose_stack(region, wires)
}

/// Default composition: merge own_nodes and children into a vertical
/// virtual-row stack, ordered by source position.
fn compose_stack(region: &Region<'_>, wires: &[Wire]) -> RegionLayout {
    let row_items = build_row_items(region);
    if row_items.is_empty() {
        return RegionLayout {
            local: HashMap::new(),
            bbox: (0, 0),
            feedback_edges: Vec::new(),
            warnings: Vec::new(),
        };
    }

    let mut local: HashMap<NodeId, LocalPlacement> = HashMap::new();
    let mut feedback_edges: Vec<(NodeId, NodeId)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut y_cursor: i32 = 0;
    let mut max_width: i32 = 0;

    for (i, item) in row_items.iter().enumerate() {
        let sub = match item {
            RowItem::Stmt(nodes) => layout_stmt_group(region, nodes, wires),
            RowItem::Region(child) => layout_region(child, wires),
        };
        // Offset the sub-layout's placements by (0, y_cursor).
        for (id, p) in &sub.local {
            local.insert(
                *id,
                LocalPlacement {
                    dx: p.dx,
                    dy: p.dy + y_cursor,
                },
            );
        }
        feedback_edges.extend(sub.feedback_edges);
        warnings.extend(sub.warnings);
        y_cursor += sub.bbox.1;
        max_width = max_width.max(sub.bbox.0);
        if i + 1 < row_items.len() && sub.bbox.1 > 0 {
            y_cursor += ROW_GUTTER;
        }
    }

    RegionLayout {
        local,
        bbox: (max_width, y_cursor),
        feedback_edges,
        warnings,
    }
}

/// IfGroup composition: `IfCond` stacked on top, `IfThen` / `IfElse`
/// placed side-by-side below with a fixed gap. `IfCond` is horizontally
/// centered above the pair.
fn compose_if_group(region: &Region<'_>, wires: &[Wire]) -> RegionLayout {
    // Pick children by kind (lowering guarantees order but we match by
    // kind for robustness — layout doesn't care about order here).
    let cond_region = region
        .children
        .iter()
        .find(|r| matches!(r.info.kind, ScopeKind::IfCond));
    let then_region = region
        .children
        .iter()
        .find(|r| matches!(r.info.kind, ScopeKind::IfThen));
    let else_region = region
        .children
        .iter()
        .find(|r| matches!(r.info.kind, ScopeKind::IfElse));

    let cond = cond_region
        .map(|r| layout_region(r, wires))
        .unwrap_or_else(empty_layout);
    let then = then_region
        .map(|r| layout_region(r, wires))
        .unwrap_or_else(empty_layout);
    let els = else_region
        .map(|r| layout_region(r, wires))
        .unwrap_or_else(empty_layout);

    let mut local: HashMap<NodeId, LocalPlacement> = HashMap::new();
    let mut feedback_edges = Vec::new();
    let mut warnings = Vec::new();

    let (tw, th) = then.bbox;
    let (ew, eh) = els.bbox;
    let (cw, ch) = cond.bbox;

    // Pair width: then + gap + else. If either is empty, omit the gap.
    let pair_width = match (tw, ew) {
        (0, 0) => 0,
        (0, _) => ew,
        (_, 0) => tw,
        _ => tw + IF_BRANCH_GAP + ew,
    };

    let total_width = pair_width.max(cw);
    let cond_x_shift = ((total_width - cw).max(0)) / 2;
    let pair_x_shift = ((total_width - pair_width).max(0)) / 2;

    // IfCond at (cond_x_shift, 0).
    for (id, p) in cond.local {
        local.insert(
            id,
            LocalPlacement {
                dx: p.dx + cond_x_shift,
                dy: p.dy,
            },
        );
    }
    feedback_edges.extend(cond.feedback_edges);
    warnings.extend(cond.warnings);

    // Branches start one gutter below the condition.
    let branch_y = if ch == 0 { 0 } else { ch + ROW_GUTTER };

    // IfThen at (pair_x_shift, branch_y).
    for (id, p) in then.local {
        local.insert(
            id,
            LocalPlacement {
                dx: p.dx + pair_x_shift,
                dy: p.dy + branch_y,
            },
        );
    }
    feedback_edges.extend(then.feedback_edges);
    warnings.extend(then.warnings);

    // IfElse at (pair_x_shift + tw + gap, branch_y).
    let else_x = pair_x_shift
        + match (tw, ew) {
            (_, 0) => 0,
            (0, _) => 0,
            _ => tw + IF_BRANCH_GAP,
        };
    for (id, p) in els.local {
        local.insert(
            id,
            LocalPlacement {
                dx: p.dx + else_x,
                dy: p.dy + branch_y,
            },
        );
    }
    feedback_edges.extend(els.feedback_edges);
    warnings.extend(els.warnings);

    let total_height = branch_y + th.max(eh);

    RegionLayout {
        local,
        bbox: (total_width, total_height),
        feedback_edges,
        warnings,
    }
}

/// Build the virtual-row stream: own nodes are clustered into `Stmt`
/// groups (consecutive nodes that share `chain_id` or originate from the
/// same source line), and child regions are passed through as `Region`
/// items. The stream is ordered by `source_range.start`.
fn build_row_items<'a>(region: &'a Region<'a>) -> Vec<RowItem<'a>> {
    #[derive(Clone, Copy)]
    enum Kind {
        Node,
        Region,
    }
    struct Entry<'b> {
        offset: usize,
        idx: usize,
        kind: Kind,
        _marker: std::marker::PhantomData<&'b ()>,
    }

    let mut entries: Vec<Entry<'_>> = Vec::new();
    for (i, n) in region.own_nodes.iter().enumerate() {
        entries.push(Entry {
            offset: n.source_range.start.offset,
            idx: i,
            kind: Kind::Node,
            _marker: std::marker::PhantomData,
        });
    }
    for (i, c) in region.children.iter().enumerate() {
        entries.push(Entry {
            offset: c.info.source_range.start.offset,
            idx: i,
            kind: Kind::Region,
            _marker: std::marker::PhantomData,
        });
    }
    entries.sort_by_key(|e| e.offset);

    let mut items: Vec<RowItem<'a>> = Vec::new();
    for e in entries {
        match e.kind {
            Kind::Node => {
                let node = region.own_nodes[e.idx];
                // Cluster into the trailing StmtGroup if that group's
                // last node shares chain_id or source line.
                if let Some(RowItem::Stmt(group)) = items.last_mut() {
                    let same_chain = matches!(
                        (group.last().and_then(|n| n.chain_id), node.chain_id),
                        (Some(a), Some(b)) if a == b
                    );
                    let same_line = group
                        .last()
                        .map(|n| n.source_range.start.line == node.source_range.start.line)
                        .unwrap_or(false);
                    if same_chain || same_line {
                        group.push(node);
                        continue;
                    }
                }
                items.push(RowItem::Stmt(vec![node]));
            }
            Kind::Region => items.push(RowItem::Region(&region.children[e.idx])),
        }
    }
    items
}

/// Lay out a `Stmt` cluster — a contiguous run of own_nodes — as if it
/// were a leaf region containing only those nodes.
fn layout_stmt_group(parent: &Region<'_>, nodes: &[&Node], wires: &[Wire]) -> RegionLayout {
    // Build a synthetic leaf region whose `own_nodes` is just this
    // cluster. Reuse the parent's ScopeInfo so `region.id` stays valid
    // for diagnostics.
    let synth = Region {
        id: parent.id,
        info: parent.info,
        own_nodes: nodes.to_vec(),
        children: Vec::new(),
    };
    // Restrict wires: only those with both endpoints in this cluster.
    let cluster_ids: HashSet<&NodeId> = nodes.iter().map(|n| &n.id).collect();
    let sub_wires: Vec<Wire> = wires
        .iter()
        .filter(|w| {
            cluster_ids.contains(&w.source.node_id) && cluster_ids.contains(&w.target.node_id)
        })
        .cloned()
        .collect();
    layout_leaf(&synth, &sub_wires)
}

fn empty_layout() -> RegionLayout {
    RegionLayout {
        local: HashMap::new(),
        bbox: (0, 0),
        feedback_edges: Vec::new(),
        warnings: Vec::new(),
    }
}

/// One entry in a region's virtual-row stream.
enum RowItem<'a> {
    Stmt(Vec<&'a Node>),
    Region(&'a Region<'a>),
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::GateIO;
    use crate::ir::{Module, ROOT_SCOPE_ID, ScopeInfo};
    use crate::lower::{LowerInput, lower};
    use crate::parser::parse;
    use crate::template_cache::TemplateCache;
    use crate::typecheck::typecheck;

    fn compile(src: &str) -> Module {
        let parsed = parse(src, "test");
        assert!(
            parsed.diagnostics.is_empty(),
            "parse diags: {:?}",
            parsed.diagnostics
        );
        let tc = typecheck(&parsed.ast, "test");
        let r = lower(LowerInput {
            ast: &parsed.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file: "test",
            module_name: None,
            template_cache: Arc::new(TemplateCache::new()),
        });
        r.module
    }

    fn lay(src: &str) -> RegionLayout {
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        layout_region(&root, &m.wires)
    }

    #[test]
    fn empty_module_composes_to_empty_bbox() {
        let lay = lay("");
        assert_eq!(lay.bbox, (0, 0));
        assert!(lay.local.is_empty());
    }

    #[test]
    fn every_node_gets_a_placement() {
        let src = "var n: int = 0\non RoundStart { n = n + 1 }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);
        for id in m.nodes.keys() {
            assert!(
                out.local.contains_key(id),
                "node {} missing a placement",
                id
            );
        }
    }

    #[test]
    fn no_two_placements_overlap() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);
        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        for (id, p) in &out.local {
            assert!(
                seen.insert((p.dx, p.dy)),
                "node {} at ({}, {}) collides with a prior placement",
                id,
                p.dx,
                p.dy
            );
        }
    }

    #[test]
    fn if_branches_are_horizontally_separated() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);

        // Find a node from the then branch and one from the else branch;
        // they should sit at different dx ranges (side-by-side).
        let then_id = m
            .scopes
            .iter()
            .find(|(_, s)| matches!(s.kind, ScopeKind::IfThen))
            .map(|(id, _)| *id)
            .unwrap();
        let else_id = m
            .scopes
            .iter()
            .find(|(_, s)| matches!(s.kind, ScopeKind::IfElse))
            .map(|(id, _)| *id)
            .unwrap();

        let then_xs: Vec<i32> = m
            .nodes
            .values()
            .filter(|n| n.scope_id == then_id)
            .filter_map(|n| out.local.get(&n.id).map(|p| p.dx))
            .collect();
        let else_xs: Vec<i32> = m
            .nodes
            .values()
            .filter(|n| n.scope_id == else_id)
            .filter_map(|n| out.local.get(&n.id).map(|p| p.dx))
            .collect();

        if !then_xs.is_empty() && !else_xs.is_empty() {
            let then_max = *then_xs.iter().max().unwrap();
            let else_min = *else_xs.iter().min().unwrap();
            assert!(
                else_min > then_max,
                "else column ({}) must start to the right of then column end ({})",
                else_min,
                then_max
            );
        }
    }

    #[test]
    fn layout_is_deterministic() {
        let src = "var n: int = 0\non RoundStart { n = n + 1 }";
        let m = compile(src);
        let root = super::super::region::build_region_tree(&m);
        let a = layout_region(&root, &m.wires);
        let b = layout_region(&root, &m.wires);
        assert_eq!(a.local, b.local);
        assert_eq!(a.bbox, b.bbox);
    }

    #[test]
    fn synthetic_nested_regions_stack_vertically() {
        // Two sibling child regions, no own nodes, no wires. Their
        // placements must have different dy ranges.
        let mut m = Module::default();
        let r1 = 1;
        let r2 = 2;
        m.scopes.insert(
            r1,
            ScopeInfo {
                kind: ScopeKind::Block,
                source_range: make_range(0, 10),
                parent: Some(ROOT_SCOPE_ID),
            },
        );
        m.scopes.insert(
            r2,
            ScopeInfo {
                kind: ScopeKind::Block,
                source_range: make_range(20, 30),
                parent: Some(ROOT_SCOPE_ID),
            },
        );
        let na = make_node("a", r1, 5);
        let nb = make_node("b", r2, 25);
        let a_id = na.id;
        let b_id = nb.id;
        m.nodes.insert(a_id, na);
        m.nodes.insert(b_id, nb);
        let root = super::super::region::build_region_tree(&m);
        let out = layout_region(&root, &m.wires);
        let ya = out.local[&a_id].dy;
        let yb = out.local[&b_id].dy;
        assert!(ya < yb, "earlier block must stack above later one");
    }

    fn make_range(start: usize, end: usize) -> crate::diagnostic::SourceRange {
        crate::diagnostic::SourceRange {
            file: "t".into(),
            start: crate::diagnostic::Pos {
                offset: start,
                line: 0,
                col: 0,
            },
            end: crate::diagnostic::Pos {
                offset: end,
                line: 0,
                col: 0,
            },
        }
    }

    fn make_node(_label: &str, scope: crate::ir::ScopeId, offset: usize) -> Node {
        Node {
            id: crate::ir::NodeId::fresh(),
            kind: crate::ir::NodeKind::Gate,
            gate_class: "G",
            properties: Arc::new(HashMap::new()),
            ports: Arc::new(GateIO::default()),
            source_range: make_range(offset, offset + 1),
            chip_id: None,
            chain_id: None,
            scope_id: scope,
            note: None,
        }
    }
}
