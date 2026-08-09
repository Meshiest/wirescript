//! Region tree over a finished `ir::Module`.
//!
//! Walk `Module.scopes` + `Module.nodes` and produce a nested tree of
//! `Region`s whose leaves hold the nodes assigned to that scope in
//! lowering. Children within a region are sorted by their scope's
//! `source_range.start` (then `ScopeId`) so the tree is deterministic.
//!
//! Consumed by [`crate::layout::compose`] in Phase 5.

use crate::ir::{Node, ScopeId, ScopeInfo};

/// Pure view over a single scope's contents in a `Module`.
#[derive(Debug)]
pub struct Region<'a> {
    pub id: ScopeId,
    pub info: &'a ScopeInfo,
    /// Nodes whose `scope_id` is exactly this region's id. Sorted by
    /// `(source_range.start, node_id)` for determinism.
    pub own_nodes: Vec<&'a Node>,
    /// Child regions, sorted by `(source_range.start, ScopeId)`.
    pub children: Vec<Region<'a>>,
}

#[cfg(test)]
use crate::ir::{Module, ROOT_SCOPE_ID};
#[cfg(test)]
use crate::collections::HashMap;

/// Build the region tree rooted at `ROOT_SCOPE_ID`.
///
/// Scopes referencing an unknown parent, or whose parent chain doesn't
/// reach root, are silently dropped (layout never panics on a malformed
/// `Module`). Orphan nodes — whose `scope_id` is missing from
/// `Module.scopes` — are re-homed onto the root region so nothing is
/// lost.
#[cfg(test)]
pub fn build_region_tree(module: &Module) -> Region<'_> {
    // Bucket nodes by scope.
    let mut nodes_by_scope: HashMap<ScopeId, Vec<&Node>> = HashMap::default();
    for node in module.nodes.values() {
        let sid = if module.scopes.contains_key(&node.scope_id) {
            node.scope_id
        } else {
            ROOT_SCOPE_ID
        };
        nodes_by_scope.entry(sid).or_default().push(node);
    }
    for nodes in nodes_by_scope.values_mut() {
        nodes.sort_by(|a, b| {
            a.source_range
                .start
                .offset
                .cmp(&b.source_range.start.offset)
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    // Build parent → children map over the scope table.
    let mut children_of: HashMap<ScopeId, Vec<ScopeId>> = HashMap::default();
    for (&id, info) in &module.scopes {
        if id == ROOT_SCOPE_ID {
            continue;
        }
        if let Some(parent) = info.parent {
            if module.scopes.contains_key(&parent) {
                children_of.entry(parent).or_default().push(id);
            }
        }
    }
    for ids in children_of.values_mut() {
        ids.sort_by(|a, b| {
            let sa = &module.scopes[a].source_range;
            let sb = &module.scopes[b].source_range;
            sa.start.offset.cmp(&sb.start.offset).then_with(|| a.cmp(b))
        });
    }

    fn build<'a>(
        id: ScopeId,
        module: &'a Module,
        nodes_by_scope: &mut HashMap<ScopeId, Vec<&'a Node>>,
        children_of: &HashMap<ScopeId, Vec<ScopeId>>,
    ) -> Region<'a> {
        let info = &module.scopes[&id];
        let own_nodes = nodes_by_scope.remove(&id).unwrap_or_default();
        let children = children_of
            .get(&id)
            .into_iter()
            .flatten()
            .map(|&cid| build(cid, module, nodes_by_scope, children_of))
            .collect();
        Region {
            id,
            info,
            own_nodes,
            children,
        }
    }

    build(ROOT_SCOPE_ID, module, &mut nodes_by_scope, &children_of)
}

/// Count nodes in the region and all descendants.
#[cfg(test)]
pub fn region_node_count(r: &Region<'_>) -> usize {
    r.own_nodes.len() + r.children.iter().map(region_node_count).sum::<usize>()
}

/// Count scopes in the tree including the root.
#[cfg(test)]
pub fn region_scope_count(r: &Region<'_>) -> usize {
    1 + r.children.iter().map(region_scope_count).sum::<usize>()
}

#[cfg(test)]
mod tests;
