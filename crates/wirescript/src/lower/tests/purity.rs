use super::*;

#[test]
fn pure_chip_no_exec_gates() {
    let r = compile("\
chip Foo(x: int) -> (result: int) { out result = x + 1 }
let v = Foo(10)
out y = v.result");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let chip = r.module.chips.values().next().expect("should have one chip instance");
    let exec_gates: Vec<_> = chip.nodes.values()
        .filter(|n| n.gate_class.contains("Exec_"))
        .collect();
    assert!(exec_gates.is_empty(),
        "pure chip body should have zero Exec_* gates, found: {:?}",
        exec_gates.iter().map(|n| &n.gate_class).collect::<Vec<_>>());
}

#[test]
fn pure_let_no_var_get() {
    let r = compile("\
var x: int = 5
let y = x + 1
out z = y");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let var_get_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Get");
    assert_eq!(var_get_count, 0,
        "pure context should have zero Exec_Var_Get gates, found {var_get_count}");
}

#[test]
fn pure_if_expr_select_not_branch() {
    let r = compile("\
var x: int = 1
let r = if x > 0 then 1 else 0
out y = r");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "pure if expression should produce Expr_Select gate");
    assert!(!has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"),
        "pure if expression should not produce Exec_Branch gate");
}

#[test]
fn exec_var_read_uses_var_get() {
    let r = compile("\
var x: int = 5
in player: character
on player { let y = x + 1 }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"),
        "exec context should use Exec_Var_Get to read a var");
}

#[test]
fn exec_var_write_uses_var_set() {
    let r = compile("\
var x: int = 0
in player: character
on player { x = 42 }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "exec context should use Exec_Var_Set to write a var");
}

#[test]
fn exec_if_stmt_uses_branch() {
    let r = compile("\
var x: int = 0
in player: character
on player { if x > 0 { x = 1 } }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Branch"),
        "exec if statement should produce Exec_Branch gate");
}

#[test]
fn exec_array_read_uses_exec_get() {
    let r = compile("\
array arr: int[]
in player: character
on player { arr.push(42); let v = arr[0] }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
        "exec array indexing should produce Exec_ArrayVar_Get gate");
}

#[test]
fn pure_op_in_exec_still_uses_var_get() {
    let r = compile("\
var x: int = 5
in player: character
on player { let y = x * 2 + 1 }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathMultiply"),
        "should have Expr_MathMultiply for x * 2");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should have Expr_MathAdd for ... + 1");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"),
        "reading var in exec context should produce Exec_Var_Get");
}

#[test]
fn pure_if_expr_in_exec_uses_select_and_var_get() {
    let r = compile("\
var x: int = 1
in player: character
on player { let r = if x > 0 then 1 else 0 }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "if expression should produce Expr_Select even in exec context");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"),
        "reading var x in exec context should produce Exec_Var_Get");
}

#[test]
fn mod_value_only_params_no_exec_gates() {
    let r = compile("\
mod add_pure(a: int, b: int) -> (result: int) { out result = a + b }
let r = add_pure(3, 4)
out y = r");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let exec_gates: Vec<_> = r.module.nodes.values()
        .filter(|n| n.gate_class.contains("Exec_"))
        .collect();
    assert!(exec_gates.is_empty(),
        "mod with only value params should produce zero Exec_* gates, found: {:?}",
        exec_gates.iter().map(|n| &n.gate_class).collect::<Vec<_>>());
}

#[test]
fn mod_ref_param_has_exec_gates() {
    let r = compile("\
mod inc(x: *int) { x = x + 1 }
var v: int = 0
in player: character
on player { inc(v) }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let has_set = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set");
    let has_incr = has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Increment");
    assert!(has_set || has_incr,
        "mod with ref param should produce Exec_Var_Set or Exec_Var_Increment");
}

#[test]
fn pure_block_expr_stays_pure() {
    let r = compile("\
var x: int = 5
let r = { let a = x + 1; let b = a * 2; b }
out y = r");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let exec_gates: Vec<_> = r.module.nodes.values()
        .filter(|n| n.gate_class.contains("Exec_"))
        .collect();
    assert!(exec_gates.is_empty(),
        "block expression in pure context should have zero Exec_* gates, found: {:?}",
        exec_gates.iter().map(|n| &n.gate_class).collect::<Vec<_>>());
}

#[test]
fn exec_block_expr_reads_use_var_get() {
    let r = compile("\
var x: int = 5
in player: character
on player { let r = { let a = x + 1; a } }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Get"),
        "block expression in exec context should use Exec_Var_Get for var reads");
}

#[test]
fn var_in_exec_emits_var_set() {
    let r = compile(
        "\
on RoundStart {
  var x: int = 5
}",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let set_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
        .count();
    assert!(
        set_count >= 1,
        "var in exec context should emit at least one Exec_Var_Set for init, found {set_count}"
    );
}

#[test]
fn static_var_in_exec_no_var_set() {
    let r = compile(
        "\
on RoundStart {
  static var x: int = 5
}",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    let set_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Var_Set")
        .count();
    assert_eq!(
        set_count, 0,
        "static var should NOT emit any Exec_Var_Set gates, found {set_count}"
    );
}
