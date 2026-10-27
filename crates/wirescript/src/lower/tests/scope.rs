use super::*;
use crate::ir::gate_class as gc;

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

/// An `if` in a handler makes an `IfGroup` scope with IfCond/IfThen/IfElse
/// children, each branch owns its body's nodes, and every node's scope id is
/// one the module holds.
#[test]
fn if_creates_an_if_group_whose_branches_own_their_nodes() {
    let src = "var n: int = 0\non RoundStart() { if (n > 0) { n = 1 } else { n = 2 } }";
    let r = compile_ok(src);
    let scope_of = |want: fn(&ScopeKind) -> bool| {
        r.module
            .scopes
            .iter()
            .find(|(_, s)| want(&s.kind))
            .map(|(id, _)| *id)
    };
    let group_id =
        scope_of(|k| matches!(k, ScopeKind::IfGroup)).expect("expected an IfGroup scope");
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

    for node in r.module.nodes.values() {
        assert!(
            r.module.scopes.contains_key(&node.scope_id),
            "node {} has scope_id {} not present in Module.scopes",
            node.id,
            node.scope_id
        );
    }

    // The Set for `n = 1` lives in IfThen, the Set for `n = 2` in IfElse.
    let then_id = scope_of(|k| matches!(k, ScopeKind::IfThen)).unwrap();
    let else_id = scope_of(|k| matches!(k, ScopeKind::IfElse)).unwrap();
    assert!(
        r.module.nodes.values().any(|n| n.scope_id == then_id),
        "IfThen should own at least one node"
    );
    assert!(
        r.module.nodes.values().any(|n| n.scope_id == else_id),
        "IfElse should own at least one node"
    );
}

// Block-scoped storage shadowing: a `var`/array/map/buffer declared inside a
// handler, `if` or block whose name matches an ancestor storage binding gets
// its own gate. Reusing the ancestor's would give type-divergent writes, a
// `static var` reset every call, and a load-breaking buffer fan-in.

const PVAR: &str = gc::VAR;
const BUF: &str = gc::BUFFER_TICKS;
const AVAR: &str = gc::ARRAY_VAR;
const MVAR: &str = gc::MAP_VAR;

/// How many wires leave `node`'s `VarRef` output (its write consumers).
fn varref_consumers(r: &LowerResult, node: crate::ir::NodeId) -> usize {
    r.module
        .wires
        .iter()
        .filter(|w| {
            w.source.node_id == node && w.source.port == crate::ir::port_registry::WirePort::VarRef
        })
        .count()
}

#[test]
fn block_local_var_shadowing_ancestor_gets_its_own_gate() {
    // Inner `int` var must not reuse the outer `string` var's storage gate.
    let r =
        compile_ok("var x: string = \"hi\"\nin go: exec\non go {\n var x: int = 5\n x = x + 1\n}");
    assert_eq!(
        gate_count(&r, PVAR),
        2,
        "the outer and inner `x` must be two distinct storage gates"
    );
    // The outer (untouched) gate has zero write consumers; the inner has both the
    // reset Set and the increment, so the int writes never reach the string gate.
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
fn buffer_shadowing_ancestor_gets_its_own_gate_no_fanin() {
    let r = compile_ok("var src: int = 0\nbuffer b = src\nin go: exec\non go { buffer b = src }");
    assert_eq!(
        gate_count(&r, BUF),
        2,
        "the block-local buffer must be its own gate"
    );
    // No buffer Input may have two drivers: that fan-in fails to load in game.
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

/// A block-local declaration named like an ancestor's storage is its own
/// storage gate: exact storage-gate counts over the whole module tree.
#[test]
fn block_local_storage_shadowing_an_ancestor_gets_its_own_gate() {
    let rows: &[(&str, &str, &str, usize)] = &[
        (
            "the block-local `acc` is its own gate, not a reset of the static every call",
            "static var acc: int = 0\nin go: exec\non go {\n var acc: int = 0\n acc = acc + 1\n}",
            PVAR,
            2,
        ),
        (
            "an inner array is its own gate",
            "var g: int[] = [0]\nin go: exec\non go { var g: int[] = []\n g.push(7) }",
            AVAR,
            2,
        ),
        (
            "an inner map is its own gate",
            "var m: Map<int,int>\nin go: exec\non go { var m: Map<int,int>\n m[1] = 5 }",
            MVAR,
            2,
        ),
        // The body pre-pass must not hoist the nested `var k` into the mod
        // frame, or the body-level `return k` reads the nested storage.
        // out1 + k + k = 3.
        (
            "a mod's body-level var and a same-named nested-`if` var stay distinct",
            "in go: exec\nvar out1: int = 0\nin cond: bool\n\
             mod f() -> (r: int) {\n var k: int = 3\n if cond {\n var k: int = 99\n k = k + 1\n }\n return k\n}\n\
             on go { out1 = f() }",
            PVAR,
            3,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, class, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let got = count_class(&r.module, class);
        if !errors.is_empty() || got != want {
            failures.push(format!(
                "{label}: errors {errors:?}, {got} x {class}, want {want}\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
