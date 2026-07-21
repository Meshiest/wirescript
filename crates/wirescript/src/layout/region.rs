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
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ir::{GateIO, ROOT_SCOPE_ID};
    use crate::lower::{LowerInput, lower};
    use crate::parser::parse;
    use crate::template_cache::TemplateCache;
    use crate::typecheck::typecheck;
    use crate::{Module, lexer};

    fn compile(src: &str) -> Module {
        // Exercise the full pipeline so scopes match the real lowering.
        let _ = lexer::lex(src, "test"); // compiled-but-unused; parser also lexes
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
            doc_comments: &parsed.doc_comments,
            fold_mode: crate::lower::FoldMode::Auto,
        });
        r.module
    }

    #[test]
    fn empty_module_yields_root_only() {
        let m = compile("");
        let root = build_region_tree(&m);
        assert_eq!(root.id, ROOT_SCOPE_ID);
        assert!(root.own_nodes.is_empty());
        assert!(root.children.is_empty());
    }

    #[test]
    fn handler_gets_one_child_region() {
        let m = compile("on RoundStart { }");
        let root = build_region_tree(&m);
        assert_eq!(root.children.len(), 1);
        let handler = &root.children[0];
        assert!(matches!(
            &handler.info.kind,
            crate::ir::ScopeKind::HandlerBody { .. }
        ));
    }

    #[test]
    fn if_else_builds_group_with_three_children() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = build_region_tree(&m);
        // root > handler_body > if_group > (cond, then, else)
        let handler = root
            .children
            .iter()
            .find(|r| matches!(&r.info.kind, crate::ir::ScopeKind::HandlerBody { .. }))
            .expect("handler region");
        let group = handler
            .children
            .iter()
            .find(|r| matches!(&r.info.kind, crate::ir::ScopeKind::IfGroup))
            .expect("if group region");
        assert_eq!(group.children.len(), 3);
        let kinds: Vec<&crate::ir::ScopeKind> =
            group.children.iter().map(|r| &r.info.kind).collect();
        // Sorted by source range: IfCond starts first, then IfThen, then IfElse.
        assert!(matches!(kinds[0], crate::ir::ScopeKind::IfCond));
        assert!(matches!(kinds[1], crate::ir::ScopeKind::IfThen));
        assert!(matches!(kinds[2], crate::ir::ScopeKind::IfElse));
    }

    #[test]
    fn node_count_matches_module_total() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = build_region_tree(&m);
        assert_eq!(region_node_count(&root), m.nodes.len());
    }

    #[test]
    fn scope_count_matches_module_total() {
        let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
        let m = compile(src);
        let root = build_region_tree(&m);
        assert_eq!(region_scope_count(&root), m.scopes.len());
    }

    #[test]
    fn orphan_nodes_land_on_root_region() {
        // Synthesize a module where a node points to a missing scope —
        // the tree builder must not drop it.
        let mut m = Module::default();
        let nid = crate::ir::NodeId::fresh();
        let node = Node {
            id: nid,
            kind: crate::ir::NodeKind::Gate,
            gate_class: "G",
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO::default()),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 9999, // bogus
            note: None,
        };
        m.nodes.insert(nid, node);

        let root = build_region_tree(&m);
        assert_eq!(root.own_nodes.len(), 1, "orphan must be re-homed to root");
    }
}
