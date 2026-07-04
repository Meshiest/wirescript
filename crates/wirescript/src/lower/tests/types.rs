use super::*;

#[test]
fn typed_vars_carry_correct_default_literal() {
    use crate::ir::Literal;
    // A var of any variant member type gets a type-matched initial value,
    // not a numeric default (which would mis-tag the WireGraphVariant).
    let initial = |src: &str| {
        let r = compile(src);
        assert_no_errors(&r);
        r.module
            .nodes
            .values()
            .find(|n| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
            .and_then(|n| n.properties.get(&crate::intern::intern_static("InitialValue")))
            .cloned()
    };
    assert!(matches!(initial("var v: vector"), Some(Literal::Vector { .. })));
    assert!(matches!(initial("var s: string"), Some(Literal::String(_))));
    assert!(matches!(initial("var b: bool"), Some(Literal::Bool(false))));
    assert!(matches!(initial("var i: int"), Some(Literal::Int(0))));
}

#[test]
fn bool_shift_left_int() {
    let r = compile("in flag: bool\nlet x = flag << 3\nout result = x");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseShiftLeft"));
}

#[test]
fn bool_bitor_int() {
    let r = compile("in a: bool\nlet c = a | 0xFF\nout result = c");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseOR"));
}

#[test]
fn bool_bitand_bool() {
    let r = compile("in a: bool\nin b: bool\nlet c = a & b\nout result = c");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseAND"));
}

#[test]
fn float_shift_left_int() {
    let r = compile("in f: float\nlet x = f << 2\nout result = x");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseShiftLeft"));
}

#[test]
fn float_bitwise_and_float() {
    let r = compile("in a: float\nin b: float\nlet c = a & b\nout result = c");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseAND"));
}

#[test]
fn bitwise_not_bool() {
    let r = compile("in a: bool\nlet c = ~a\nout result = c");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseNOT"));
}

#[test]
fn bool_packed_into_bitmask() {
    let r = compile("in a: bool\nin b: bool\nin c: bool\nlet mask = a | (b << 1) | (c << 2)\nout result = mask");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseOR"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseShiftLeft"));
}

#[test]
fn let_type_annotation_matching() {
    let r = compile("let x: int = 42\nout result = x");
    assert_no_errors(&r);
}

#[test]
fn let_type_annotation_mismatch_warns() {
    let r = compile("let x: string = 42\nout result = x");
    let warns: Vec<_> = r.diagnostics.iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Warning && d.code == "WS016")
        .collect();
    assert_eq!(warns.len(), 1, "should warn on type mismatch: {:?}", r.diagnostics);
    assert!(warns[0].message.contains("string"));
}

#[test]
fn let_type_annotation_coercible_no_warn() {
    // int coerces to float, so no warning
    let r = compile("let x: float = 42\nout result = x");
    let warns: Vec<_> = r.diagnostics.iter()
        .filter(|d| d.code == "WS016")
        .collect();
    assert!(warns.is_empty(), "coercible types should not warn: {:?}", warns);
}

#[test]
fn let_type_annotation_in_exec() {
    let r = compile("in tick: exec\nvar n: int = 0\non tick { let x: int = n + 1 }");
    assert_no_errors(&r);
}

#[test]
fn let_type_annotation_bool() {
    let r = compile("in a: bool\nlet flag: bool = a\nout result = flag");
    assert_no_errors(&r);
}

#[test]
fn out_var_without_type_warns() {
    let r = compile("var x: int = 0\nout result = x");
    let warns: Vec<_> = r.diagnostics.iter()
        .filter(|d| d.code == "WS017")
        .collect();
    assert_eq!(warns.len(), 1, "out from var without type should warn WS017: {:?}", r.diagnostics);
    assert!(warns[0].message.contains("int"), "warning should mention the var's type");
}

#[test]
fn out_var_with_type_no_warn() {
    let r = compile("var x: int = 0\nout result: int = x");
    let warns: Vec<_> = r.diagnostics.iter()
        .filter(|d| d.code == "WS017")
        .collect();
    assert!(warns.is_empty(), "out from var WITH type should not warn: {:?}", warns);
}

#[test]
fn out_let_no_warn() {
    let r = compile("let x = 42\nout result = x");
    let warns: Vec<_> = r.diagnostics.iter()
        .filter(|d| d.code == "WS017")
        .collect();
    assert!(warns.is_empty(), "out from let should not warn: {:?}", warns);
}

#[test]
fn out_expr_no_warn() {
    let r = compile("in a: int\nout result = a + 1");
    let warns: Vec<_> = r.diagnostics.iter()
        .filter(|d| d.code == "WS017")
        .collect();
    assert!(warns.is_empty(), "out from expression should not warn: {:?}", warns);
}

#[test]
fn deref_in_exec_produces_var_get() {
    let r = compile("var x: int = 0\nin tick: exec\non tick { let v = *x }");
    assert_no_errors(&r);
    let var_gets = gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Get");
    assert!(var_gets >= 1, "should produce at least one Var_Get for deref");
}

#[test]
fn ampersand_ref_parses() {
    // &x parses as RefOf, same as `ref x`. Verify it compiles.
    let r = compile("var x: int = 0\nout result: *int = &x");
    assert_no_errors(&r);
}

#[test]
fn bitwise_and_still_works_with_ampersand_ref() {
    let r = compile("in a: int\nin b: int\nlet c = a & b\nout result = c");
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_BitwiseAND"),
        "infix & should still be bitwise AND");
}
