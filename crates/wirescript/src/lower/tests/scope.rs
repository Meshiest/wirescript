use super::*;

#[test]
fn module_has_root_scope_only_for_empty_program() {
    let r = compile("");
    assert_eq!(r.module.scopes.len(), 1);
    let root = r
        .module
        .scopes
        .get(&crate::ir::ROOT_SCOPE_ID)
        .expect("root scope must exist");
    assert!(matches!(root.kind, ScopeKind::ModuleRoot));
    assert!(root.parent.is_none());
}

#[test]
fn handler_allocates_handler_body_scope() {
    let r = compile("on RoundStart { }");
    let has_handler_body = r.module.scopes.values().any(|s| match &s.kind {
        ScopeKind::HandlerBody { trigger_label } => trigger_label == "RoundStart",
        _ => false,
    });
    assert!(
        has_handler_body,
        "expected a HandlerBody scope for RoundStart"
    );
    // Handler body's parent must be ModuleRoot.
    let hb = r
        .module
        .scopes
        .values()
        .find(|s| matches!(&s.kind, ScopeKind::HandlerBody { .. }))
        .unwrap();
    assert_eq!(hb.parent, Some(crate::ir::ROOT_SCOPE_ID));
}

#[test]
fn if_creates_if_group_with_cond_then_else_children() {
    let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
    let r = compile(src);
    let group_id = r
        .module
        .scopes
        .iter()
        .find(|(_, s)| matches!(s.kind, ScopeKind::IfGroup))
        .map(|(id, _)| *id)
        .expect("expected an IfGroup scope");

    let mut kinds: Vec<&ScopeKind> = r
        .module
        .scopes
        .values()
        .filter(|s| s.parent == Some(group_id))
        .map(|s| &s.kind)
        .collect();
    kinds.sort_by_key(|k| match k {
        ScopeKind::IfCond => 0,
        ScopeKind::IfThen => 1,
        ScopeKind::IfElse => 2,
        _ => 99,
    });
    assert_eq!(kinds.len(), 3);
    assert!(matches!(kinds[0], ScopeKind::IfCond));
    assert!(matches!(kinds[1], ScopeKind::IfThen));
    assert!(matches!(kinds[2], ScopeKind::IfElse));
}

#[test]
fn every_node_has_a_valid_scope_id() {
    let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
    let r = compile(src);
    for node in r.module.nodes.values() {
        assert!(
            r.module.scopes.contains_key(&node.scope_id),
            "node {} has scope_id {} not present in Module.scopes",
            node.id,
            node.scope_id
        );
    }
}

#[test]
fn if_branches_own_their_body_nodes() {
    // The Set gate for `n = 1` must live in the IfThen scope;
    // the Set gate for `n = 2` must live in the IfElse scope.
    let src = "var n: int = 0\non RoundStart { if (n > 0) { n = 1 } else { n = 2 } }";
    let r = compile(src);

    let then_id = r
        .module
        .scopes
        .iter()
        .find(|(_, s)| matches!(s.kind, ScopeKind::IfThen))
        .map(|(id, _)| *id)
        .unwrap();
    let else_id = r
        .module
        .scopes
        .iter()
        .find(|(_, s)| matches!(s.kind, ScopeKind::IfElse))
        .map(|(id, _)| *id)
        .unwrap();

    let in_then: Vec<&str> = r
        .module
        .nodes
        .values()
        .filter(|n| n.scope_id == then_id)
        .map(|n| n.gate_class)
        .collect();
    let in_else: Vec<&str> = r
        .module
        .nodes
        .values()
        .filter(|n| n.scope_id == else_id)
        .map(|n| n.gate_class)
        .collect();

    assert!(!in_then.is_empty(), "IfThen should own at least one node");
    assert!(!in_else.is_empty(), "IfElse should own at least one node");
}
