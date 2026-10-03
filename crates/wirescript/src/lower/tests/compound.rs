use super::*;

#[test]
fn plus_equals_uses_incvar() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n += 5 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Increment"),
        "+= should lower to Exec_Var_Increment");
}

/// What one `Var_Increment` adds: a constant baked onto it or wired from a
/// `_Literal`, or the node wired into its `Value` (through a `MathNegate`,
/// the negate's own input).
#[derive(Debug, PartialEq)]
enum Amount {
    Literal(crate::ir::Literal),
    Wired(crate::ir::NodeId),
    Negated(crate::ir::NodeId),
}

fn increment_amounts(r: &LowerResult) -> Vec<Amount> {
    let source = |id: crate::ir::NodeId, port: WirePort| {
        r.module
            .wires
            .iter()
            .find(|w| w.target.node_id == id && w.target.port == port)
            .map(|w| w.source.node_id)
    };
    let mut out = Vec::new();
    for (id, n) in &r.module.nodes {
        if n.gate_class != "BrickComponentType_Exec_Var_Increment" {
            continue;
        }
        let Some(src) = source(*id, WirePort::Value) else {
            out.push(Amount::Literal(n.properties.get(&crate::intern::sym::VALUE).cloned().expect("a baked amount")));
            continue;
        };
        let s = &r.module.nodes[&src];
        out.push(match s.gate_class {
            crate::ir::gate_class::LITERAL => {
                Amount::Literal(s.properties.get(&crate::intern::sym::VALUE).cloned().expect("literal value"))
            }
            "BrickComponentType_Expr_MathNegate" => Amount::Negated(source(src, WirePort::Input).expect("negated input")),
            _ => Amount::Wired(src),
        });
    }
    out
}

/// The input node named `name`.
fn input_node(r: &LowerResult, name: &str) -> crate::ir::NodeId {
    *r.module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.kind == crate::ir::NodeKind::Input
                && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == name)
        })
        .map(|(id, _)| id)
        .expect("input node")
}

/// `n -= d` is `Var_Increment` by `-d`: a constant amount is negated where it
/// is written, any other through one `MathNegate` that feeds the increment,
/// never Get + Subtract + Set.
#[test]
fn minus_equals_uses_incvar_by_the_negated_amount() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n -= 3 }");
    assert_no_errors(&r);
    assert!(!has_gate(&r, "BrickComponentType_Exec_Var_Set"));
    assert!(!has_gate(&r, "BrickComponentType_Expr_MathSubtract"));
    assert!(!has_gate(&r, "BrickComponentType_Expr_MathNegate"), "a constant is negated at compile time");
    assert_eq!(increment_amounts(&r), vec![Amount::Literal(crate::ir::Literal::Int(-3))]);
    let r = compile("in tick: exec\nin d: float\nvar n: float = 0.0\non tick { n -= d }");
    assert_no_errors(&r);
    assert!(!has_gate(&r, "BrickComponentType_Expr_MathSubtract"));
    assert_eq!(increment_amounts(&r), vec![Amount::Negated(input_node(&r, "d"))]);
    // The target on the right is a subtraction from it, not an increment.
    let r = compile("in tick: exec\nin d: int\nvar n: int = 0\non tick { n = d - n }");
    assert_no_errors(&r);
    assert!(!has_gate(&r, "BrickComponentType_Exec_Var_Increment"));
}

/// An int var less a float, or a float var less an int, is not an increment:
/// it keeps the subtraction and the store.
#[test]
fn minus_equals_across_int_and_float_keeps_the_subtraction() {
    for src in [
        "in tick: exec\nin f: float\nvar n: int = 0\non tick { n -= f }",
        "in tick: exec\nin d: int\nvar n: float = 0.0\non tick { n -= d }",
        "in tick: exec\nvar n: float = 0.0\non tick { n -= 1 }",
    ] {
        let r = compile(src);
        assert_no_errors(&r);
        assert!(!has_gate(&r, "BrickComponentType_Exec_Var_Increment"), "{src}");
        assert!(!has_gate(&r, "BrickComponentType_Expr_MathNegate"), "{src}");
        assert_eq!(gate_count(&r, "BrickComponentType_Expr_MathSubtract"), 1, "{src}");
        assert_eq!(gate_count(&r, "BrickComponentType_Exec_Var_Set"), 1, "{src}");
    }
}

/// `*x = *x + d` and `*x = *x - d` increment the var behind `x`, whether `x`
/// is bound to it statically (a `*T` parameter given `&n`) or holds a ref, by
/// `d` and `-d` respectively.
#[test]
fn a_self_update_through_a_ref_increments() {
    let r = compile(
        "in tick: exec\nin d: int\nvar n: int = 0\nvar h: *int\n\
         mod inc(x: *int) { *x = *x + d }\nmod dec(x: *int) { *x = *x - d }\n\
         on tick {\n  inc(&n)\n  dec(&n)\n  h = &n\n  *h = *h - 2\n  *h = *h + d\n}",
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "BrickComponentType_Expr_MathSubtract"));
    assert!(!has_gate(&r, "BrickComponentType_Expr_MathAdd"));
    let d = input_node(&r, "d");
    let mut got = increment_amounts(&r);
    got.sort_by_key(|a| format!("{a:?}"));
    let mut want = vec![
        Amount::Wired(d),
        Amount::Negated(d),
        Amount::Literal(crate::ir::Literal::Int(-2)),
        Amount::Wired(d),
    ];
    want.sort_by_key(|a| format!("{a:?}"));
    assert_eq!(got, want);
}

#[test]
fn times_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 1\non tick { n *= 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "*= should lower to Exec_Var_Set");
}

#[test]
fn div_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 100\non tick { n /= 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "/= should lower to Exec_Var_Set");
}

#[test]
fn mod_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 100\non tick { n %= 7 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "%= should lower to Exec_Var_Set");
}

#[test]
fn bitand_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0xFF\non tick { n &= 0x0F }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "&= should lower to Exec_Var_Set");
}

#[test]
fn bitor_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n |= 0x80 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "|= should lower to Exec_Var_Set");
}

#[test]
fn xor_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n ^= 0xFF }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "^= should lower to Exec_Var_Set");
}

#[test]
fn shl_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 1\non tick { n <<= 4 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        "<<= should lower to Exec_Var_Set");
}

#[test]
fn shr_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 256\non tick { n >>= 4 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Set"),
        ">>= should lower to Exec_Var_Set");
}

#[test]
fn compound_assign_with_expr_rhs() {
    let r = compile("in tick: exec\nvar a: int = 0\nvar b: int = 10\non tick { a += b * 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Increment"),
        "+= with expression RHS should still use IncVar");
}

#[test]
fn compound_assign_float() {
    let r = compile("in tick: exec\nvar f: float = 1.0\non tick { f += 0.5 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Increment"),
        "+= on float should use IncVar");
}

#[test]
fn plus_equals_after_return_guard_exec_chain() {
    let r = compile("\
in flag: bool
mod foo(a: *int, f: bool) {
  if f { return }
  a += 10
}
in tick: exec
on tick {
  var x: int = 5
  foo(x, flag)
}");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Exec_Var_Increment"),
        "should produce IncVar for += 10");
    let incr_node = r.module.nodes.values()
        .find(|n| n.gate_class == "BrickComponentType_Exec_Var_Increment")
        .unwrap();
    let incr_exec_source = r.module.wires.iter()
        .find(|w| w.target.node_id == incr_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec)
        .expect("IncVar must have an Exec input wire");
    let source_node = &r.module.nodes[&incr_exec_source.source.node_id];
    assert!(
        source_node.gate_class == "BrickComponentType_Exec_Branch"
            || source_node.gate_class == "BrickComponentType_Exec_Union",
        "IncVar Exec should be wired from Branch or Union (after return guard), got: {} ({})",
        source_node.gate_class, source_node.id
    );
}

#[test]
fn times_equals_after_return_guard_exec_chain() {
    let r = compile("\
in flag: bool
mod bar(a: *int, f: bool) {
  if f { return }
  a *= 3
}
in tick: exec
on tick {
  var x: int = 5
  bar(x, flag)
}");
    assert_no_errors(&r);
    let get_node = r.module.nodes.values()
        .find(|n| n.gate_class == "BrickComponentType_Exec_Var_Get")
        .expect("*= should produce Exec_Var_Get");
    let get_exec_wire = r.module.wires.iter()
        .find(|w| w.target.node_id == get_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec)
        .expect("Var_Get must have an Exec input wire");
    let get_source = &r.module.nodes[&get_exec_wire.source.node_id];
    assert!(
        get_source.gate_class == "BrickComponentType_Exec_Branch"
            || get_source.gate_class == "BrickComponentType_Exec_Union",
        "Var_Get Exec should be wired from Branch or Union (after return guard), got: {} ({})",
        get_source.gate_class, get_source.id
    );
    let set_node = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_Exec_Var_Set")
        .find(|n| {
            r.module.wires.iter().any(|w|
                w.target.node_id == n.id
                && w.target.port == crate::ir::port_registry::WirePort::Exec
                && w.source.node_id == get_node.id)
        })
        .expect("*= should produce a Var_Set chained from Var_Get");
    let set_exec_wire = r.module.wires.iter()
        .find(|w| w.target.node_id == set_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec)
        .unwrap();
    assert_eq!(
        set_exec_wire.source.node_id, get_node.id,
        "Var_Set Exec must be wired from Var_Get ExecOut, got {}",
        set_exec_wire.source.node_id
    );
}

/// Regression: <<= must desugar to << (not <<), >>= must desugar to >> (not >>).
#[test]
fn shift_left_equals() {
    let r = compile("var x: int = 1\nin tick: exec\non tick { x <<= 3 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Expr_BitwiseShiftLeft"),
        "<<= should produce a BitwiseShiftLeft gate");
}

#[test]
fn shift_right_equals() {
    let r = compile("var x: int = 256\nin tick: exec\non tick { x >>= 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_Expr_BitwiseShiftRight"),
        ">>= should produce a BitwiseShiftRight gate");
}

#[test]
fn var_read_after_if_not_reused_from_branch() {
    // A Var_Get emitted inside a branch only fires when that branch is taken. It must
    // NOT be cached and reused for a read after the join - otherwise that later read is
    // stale whenever the branch wasn't taken. Regression for the "gambler skip":
    // `gamblersTotal` was first read inside an `if phase == P_SNIPER` block and re-read
    // in a later `if phase == P_GATHER` block, coming back stale 0 and skipping the
    // gambler phase. The post-join read must get its own Var_Get on the unconditional
    // chain (>= 2 total, not collapsed to 1).
    let r = compile(
        "in go: exec\nin t: bool\n\
         static var a: int = 0\nstatic var b: int = 0\nstatic var c: int = 0\n\
         on go { if t { b = a }\n c = a }",
    );
    assert_no_errors(&r);
    let gets = gate_count(&r, "BrickComponentType_Exec_Var_Get");
    assert!(gets >= 2,
        "post-join read of `a` must get its own Var_Get, not reuse the in-branch one (got {gets})");
}

/// A record-returning mod call passed DIRECTLY as an argument
/// (`Take(Make())`) must bind the callee's record parameter to the caller's
/// record, exactly as the hand-split `let m = Make()` / `Take(m.o)` does.
///
/// Argument binding only recognised a record LITERAL or something
/// `resolve_field_chain` could walk (an ident or field chain). A call
/// expression is neither, so the record param never received its fields, every
/// destructured name inside the callee was unbound, and each read lowered to an
/// `_Unsupported` placeholder returning a default. `wirescript-check` reported
/// the file clean, so this shipped as a silent miscompile.
#[test]
fn record_returning_call_as_an_argument_binds_the_record() {
    let r = compile(
        "type Rec = { a: string, b: int }\n\
         let recempty: Rec = { a: \"Z\", b: 0 }\n\
         mod Make() -> (o: Rec) { out o = recempty }\n\
         mod Take({a, b}: Rec) -> (s: string) { out s = a }\n\
         in go: exec\n\
         on go { let r = Take(Make())  PrintToConsole(\"${r.s}\") }",
    );
    assert_no_errors(&r);
    let placeholders = gate_count(&r, "_Unsupported");
    assert_eq!(
        placeholders, 0,
        "a record-returning call argument must not lower to a placeholder"
    );
}
