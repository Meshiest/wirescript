use super::*;

#[test]
fn block_expr_pure_if_with_let() {
    let r = compile("\
var x: int = 5
let result = if x > 3 then { let a = x + 1; a } else { let b = x - 1; b }
out y = result");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let select_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_Select")
        .count();
    assert!(select_count >= 1, "pure block if should produce Select gate");
    let branch_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_Branch")
        .count();
    assert_eq!(branch_count, 0, "pure block if should NOT produce Branch gate");
}

#[test]
fn block_expr_scoped_locals() {
    let r = compile("\
var x: int = 10
let result = if x > 5 then { let inner = x * 2; inner } else { 0 }
out y = result");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let mul_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathMultiply")
        .count();
    assert!(mul_count >= 1, "should have a MathMultiply for x * 2 inside block");
}

#[test]
fn block_expr_in_pure_let() {
    let r = compile("\
var x: int = 10
let doubled = { let tmp = x + x; tmp }
out y = doubled");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let add_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .count();
    assert!(add_count >= 1, "should have MathAdd for x + x in block");
}

#[test]
fn block_expr_no_stmts_is_just_expr() {
    let r = compile("\
var x: int = 5
let y = if x > 0 then { x } else { 0 }
out z = y");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
}

#[test]
fn block_expr_nested() {
    let r = compile("\
var x: int = 0
let result = { let a = { let b = 1; b + 1 }; a + 1 }
out y = result");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let add_count = r.module.nodes.values()
        .filter(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd")
        .count();
    assert!(add_count >= 2, "nested block should produce at least 2 MathAdd gates, found {add_count}");
}

#[test]
fn block_expr_multiple_lets() {
    let r = compile("\
let result = { let a = 1; let b = 2; a + b }
out y = result");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let has_add = r.module.nodes.values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd");
    assert!(has_add, "block with multiple lets should produce a MathAdd gate");
}

#[test]
fn block_expr_in_else_only() {
    let r = compile("\
var x: int = 5
let result = if x > 0 then x else { let neg = 0 - x; neg }
out y = result");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}", r.diagnostics);
    let has_select = r.module.nodes.values()
        .any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_Select");
    assert!(has_select, "pure if with block in else should produce a Select gate");
}

#[test]
fn block_expr_empty_produces_default() {
    let r = compile("\
let x = { }");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "empty block should compile without errors: {:?}", r.diagnostics);
}

#[test]
fn block_expr_scope_does_not_leak() {
    let r = compile("\
let inner = 42
let result = { let inner = 99; inner }
out check = inner");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "block scope should not leak — outer `inner` must resolve: {:?}", r.diagnostics);
}

#[test]
fn block_expr_as_function_arg() {
    let r = compile("\
let y = sin({ let a = 3.14; a })
out z = y");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "block expr as function arg should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathSin"),
        "should produce a MathSin gate for sin()");
}

#[test]
fn block_expr_in_binop() {
    let r = compile("\
let y = { let a = 1; a } + { let b = 2; b }
out z = y");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "two block exprs in binop should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should produce a MathAdd gate for +");
}

#[test]
fn block_expr_as_if_condition() {
    let r = compile("\
var x: int = 5
let y = if { let a = x + 1; a } > 10 then 1 else 0
out z = y");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "block expr as if condition should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_Select"),
        "should produce a Select gate for pure if-then-else");
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should produce MathAdd for x + 1 inside block condition");
}

#[test]
fn block_expr_with_chip_call() {
    let r = compile("\
chip Dbl(x: int) -> (result: int) { out result = x * 2 }
let y = { let d = Dbl(5); d.result }
out z = y");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "chip call inside block expr should compile: {:?}", r.diagnostics);
    assert!(!r.module.chips.is_empty(),
        "should create a chip instance for Dbl call inside block");
}

#[test]
fn block_expr_string_interpolation() {
    let r = compile("\
let v = { let a = 1; a }
let s = \"val=${v}\"
out z = s");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "block expr with string interpolation should compile: {:?}", r.diagnostics);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_String_FormatText"),
        "should produce a FormatText gate for string interpolation");
}

#[test]
fn block_expr_single_expr_no_semicolon() {
    let r = compile("\
let x = { 42 }
out y = x");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "block with single literal should compile: {:?}", r.diagnostics);
}

#[test]
fn block_expr_var_not_accessible() {
    let r = compile("\
on RoundStart {
  var temp: int = 5
  let result = { let t = temp; t }
}");
    assert!(r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "var used inside block expr should compile: {:?}", r.diagnostics);
}
