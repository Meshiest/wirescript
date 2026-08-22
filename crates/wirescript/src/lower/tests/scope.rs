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
    let r = compile("on RoundStart() { }");
    let has_handler_body = r.module.scopes.values().any(|s| match &s.kind {
        ScopeKind::HandlerBody { trigger_label } => trigger_label == "RoundStart",
        _ => false,
    });
    assert!(
        has_handler_body,
        "expected a HandlerBody scope for RoundStart"
    );
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
    let src = "var n: int = 0\non RoundStart() { if (n > 0) { n = 1 } else { n = 2 } }";
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
    let src = "var n: int = 0\non RoundStart() { if (n > 0) { n = 1 } else { n = 2 } }";
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
    let src = "var n: int = 0\non RoundStart() { if (n > 0) { n = 1 } else { n = 2 } }";
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

// ── block-scoped storage shadowing (regression) ──
// A `var`/array/map/buffer declared inside a handler/`if`/block whose name
// matches an ancestor storage binding must get its OWN gate, not silently
// reuse the ancestor's. The old `needs_declaration` walked the whole frame
// chain, found the ancestor, and skipped the declaration — so the inner var
// wrote the outer's storage (type-divergent writes, a `static var` reset every
// call, a load-breaking buffer fan-in).

const PVAR: &str = "BrickComponentType_WireGraphPseudo_Var";
const BUF: &str = "BrickComponentType_WireGraphPseudo_BufferTicks";
const AVAR: &str = "BrickComponentType_WireGraphPseudo_ArrayVar";
const MVAR: &str = "BrickComponentType_WireGraphPseudo_MapVar";

/// How many wires leave `node`'s `VarRef` output (its write consumers).
fn varref_consumers(r: &LowerResult, node: crate::ir::NodeId) -> usize {
    r.module
        .wires
        .iter()
        .filter(|w| {
            w.source.node_id == node
                && w.source.port == crate::ir::port_registry::WirePort::VarRef
        })
        .count()
}

#[test]
fn block_local_var_shadowing_ancestor_gets_its_own_gate() {
    // Inner `int` var must not reuse the outer `string` var's storage gate.
    let r = compile("var x: string = \"hi\"\nin go: exec\non go {\n var x: int = 5\n x = x + 1\n}");
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, PVAR),
        2,
        "the outer and inner `x` must be two distinct storage gates"
    );
    // The outer (untouched) gate has zero write consumers; the inner has both
    // the reset Set and the increment — so the int writes never reach the
    // string gate.
    let consumers: Vec<usize> = r
        .module
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == PVAR)
        .map(|(id, _)| varref_consumers(&r, *id))
        .collect();
    assert!(
        consumers.contains(&0) && consumers.contains(&2),
        "inner writes must all target the inner gate, leaving the outer untouched: {consumers:?}"
    );
}

#[test]
fn static_var_not_reset_by_shadowing_block_var() {
    let r =
        compile("static var acc: int = 0\nin go: exec\non go {\n var acc: int = 0\n acc = acc + 1\n}");
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, PVAR),
        2,
        "the block-local `acc` must be its own gate, not reset the static every call"
    );
}

#[test]
fn buffer_shadowing_ancestor_gets_its_own_gate_no_fanin() {
    let r = compile("var src: int = 0\nbuffer b = src\nin go: exec\non go { buffer b = src }");
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, BUF),
        2,
        "the block-local buffer must be its own gate"
    );
    // No buffer Input may have two drivers — that fan-in fails to load in-game.
    for (id, _) in r.module.nodes.iter().filter(|(_, n)| n.gate_class == BUF) {
        let drivers = r
            .module
            .wires
            .iter()
            .filter(|w| {
                w.target.node_id == *id
                    && w.target.port == crate::ir::port_registry::WirePort::Input
            })
            .count();
        assert!(drivers <= 1, "buffer Input fan-in ({drivers} drivers)");
    }
}

#[test]
fn block_local_array_and_map_shadow_get_own_gate() {
    let ra = compile("var g: int[] = [0]\nin go: exec\non go { var g: int[] = []\n g.push(7) }");
    assert_no_errors(&ra);
    assert_eq!(gate_count(&ra, AVAR), 2, "inner array must be its own gate");
    let rm = compile("var m: Map<int,int>\nin go: exec\non go { var m: Map<int,int>\n m[1] = 5 }");
    assert_no_errors(&rm);
    assert_eq!(gate_count(&rm, MVAR), 2, "inner map must be its own gate");
}

#[test]
fn mod_body_var_and_nested_if_var_stay_distinct() {
    // A mod's body-level var and a same-named nested-`if` var are distinct
    // storage. The recursive body pre-pass used to hoist the nested `var k`
    // into the mod frame, collapsing it with the body-level `k` — orphaning one
    // gate and mis-scoping the body-level `return k` onto the nested storage.
    let r = compile(
        "in go: exec\nvar out1: int = 0\nin cond: bool\n\
         mod f() -> (r: int) {\n var k: int = 3\n if cond {\n var k: int = 99\n k = k + 1\n }\n return k\n}\n\
         on go { out1 = f() }",
    );
    assert_no_errors(&r);
    // out1 + body-level k + nested k = exactly 3 gates (no orphan 4th).
    assert_eq!(
        gate_count(&r, PVAR),
        3,
        "body-level and nested `k` must be distinct with no orphaned gate"
    );
}
