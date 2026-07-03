use super::*;

#[test]
fn plus_equals_uses_incvar() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n += 5 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment"),
        "+= should lower to Exec_Var_Increment");
}

#[test]
fn minus_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n -= 3 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "-= should lower to Var_Get + Sub + Var_Set");
}

#[test]
fn times_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 1\non tick { n *= 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "*= should lower to Exec_Var_Set");
}

#[test]
fn div_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 100\non tick { n /= 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "/= should lower to Exec_Var_Set");
}

#[test]
fn mod_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 100\non tick { n %= 7 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "%= should lower to Exec_Var_Set");
}

#[test]
fn bitand_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0xFF\non tick { n &= 0x0F }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "&= should lower to Exec_Var_Set");
}

#[test]
fn bitor_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n |= 0x80 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "|= should lower to Exec_Var_Set");
}

#[test]
fn xor_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { n ^= 0xFF }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "^= should lower to Exec_Var_Set");
}

#[test]
fn shl_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 1\non tick { n <<= 4 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "<<= should lower to Exec_Var_Set");
}

#[test]
fn shr_equals_uses_var_set() {
    let r = compile("in tick: exec\nvar n: int = 256\non tick { n >>= 4 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        ">>= should lower to Exec_Var_Set");
}

#[test]
fn compound_assign_with_expr_rhs() {
    let r = compile("in tick: exec\nvar a: int = 0\nvar b: int = 10\non tick { a += b * 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment"),
        "+= with expression RHS should still use IncVar");
}

#[test]
fn compound_assign_float() {
    let r = compile("in tick: exec\nvar f: float = 1.0\non tick { f += 0.5 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment"),
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
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment"),
        "should produce IncVar for += 10");
    let incr_node = r.module.nodes.values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Increment")
        .unwrap();
    let incr_exec_source = r.module.wires.iter()
        .find(|w| w.target.node_id == incr_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec)
        .expect("IncVar must have an Exec input wire");
    let source_node = &r.module.nodes[&incr_exec_source.source.node_id];
    assert!(
        source_node.gate_class == "BrickComponentType_WireGraph_Exec_Branch"
            || source_node.gate_class == "BrickComponentType_WireGraph_Exec_Union",
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
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Get")
        .expect("*= should produce Exec_Var_Get");
    let get_exec_wire = r.module.wires.iter()
        .find(|w| w.target.node_id == get_node.id && w.target.port == crate::ir::port_registry::WirePort::Exec)
        .expect("Var_Get must have an Exec input wire");
    let get_source = &r.module.nodes[&get_exec_wire.source.node_id];
    assert!(
        get_source.gate_class == "BrickComponentType_WireGraph_Exec_Branch"
            || get_source.gate_class == "BrickComponentType_WireGraph_Exec_Union",
        "Var_Get Exec should be wired from Branch or Union (after return guard), got: {} ({})",
        get_source.gate_class, get_source.id
    );
    let set_node = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
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
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseShiftLeft"),
        "<<= should produce a BitwiseShiftLeft gate");
}

#[test]
fn shift_right_equals() {
    let r = compile("var x: int = 256\nin tick: exec\non tick { x >>= 2 }");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseShiftRight"),
        ">>= should produce a BitwiseShiftRight gate");
}
