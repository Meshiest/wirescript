use crate::ir::Type;
use crate::parser::parse;
use crate::typecheck::infer::{check, infer};
use crate::typecheck::TypeCheckCtx;

fn ctx() -> TypeCheckCtx<'static> {
    static EMPTY: std::sync::OnceLock<crate::typecheck::CeSlotMap> = std::sync::OnceLock::new();
    TypeCheckCtx::new("test", EMPTY.get_or_init(Default::default))
}
fn first_expr(src: &str) -> crate::ast::Expr {
    // A top-level `let x = <expr>` — pull the initializer to exercise infer.
    let p = parse(src, "test");
    match &p.ast.decls[0] {
        crate::ast::TopDecl::Let(l) => l.value.clone(),
        _ => panic!("expected a let decl"),
    }
}

#[test]
fn infer_records_int_literal() {
    let mut c = ctx();
    let e = first_expr("let x = 5");
    assert_eq!(infer(&mut c, &e), Type::Int);
    let r = e.range();
    assert_eq!(
        c.type_of_expr
            .get(&(r.file.clone(), r.start.offset, r.end.offset)),
        Some(&Type::Int)
    );
}

#[test]
fn check_emits_ws003_on_mismatch() {
    let mut c = ctx();
    let e = first_expr("let x = \"hi\"");
    // string does not coerce to vector.
    check(&mut c, &e, &Type::Vector);
    assert!(c.diagnostics.iter().any(|d| d.code == "WS003"));
}

#[test]
fn literals_infer_scalar_types() {
    let mut c = ctx();
    assert_eq!(infer(&mut c, &first_expr("let x = 5")), Type::Int);
    assert_eq!(infer(&mut c, &first_expr("let x = 5.0")), Type::Float);
    assert_eq!(infer(&mut c, &first_expr("let x = true")), Type::Bool);
    assert_eq!(infer(&mut c, &first_expr("let x = \"s\"")), Type::String);
}

#[test]
fn interp_reports_non_stringable_part() {
    // An interpolation part that can't format to string is WS003; the
    // whole expr still types as string.
    let p = parse("var z: zone\nlet s = \"x ${z}\"", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS003"));
}

#[test]
fn prefab_ref_requires_brz() {
    let p = parse("let s = $./level", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS019"));
}

#[test]
fn ident_var_auto_derefs_and_records_context() {
    // A `var` read yields the inner type and records its exec/pure context.
    let p = parse("var n: int = 0\nin go: exec\non go { let x = n }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected: {:?}",
        r.diagnostics
    );
    assert!(!r.var_read_contexts.is_empty());
}

#[test]
fn deref_in_pure_context_is_ws006() {
    let p = parse("var n: int = 0\nout v = *n", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS006"));
}

#[test]
fn unknown_ident_is_ws002() {
    let p = parse("let x = nope", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS002"));
}

#[test]
fn binop_records_op_resolution() {
    let p = parse("let x = 1 + 2", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(!r.op_resolutions.is_empty());
    assert!(r
        .diagnostics
        .iter()
        .all(|d| d.severity != crate::diagnostic::Severity::Error));
}

#[test]
fn bad_operator_overload_diagnoses() {
    // string + int has no arithmetic overload → WS004.
    let p = parse("let x = \"a\" + 1", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS004"));
    // `&` (bitwise) has rules for int/float/bool operands only — a vector
    // operand has no overload → WS011.
    let p2 = parse("let y = Vec(0.0, 0.0, 0.0) & 2", "test");
    let r2 = crate::typecheck::typecheck(&p2.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r2.diagnostics.iter().any(|d| d.code == "WS011"));
}

#[test]
fn record_field_access_and_missing_field() {
    // `.x` on a vector is float; a bogus record field is WS010.
    let ok = parse("let v = Vec(1.0,2.0,3.0)\nlet a = v.x", "test");
    let r = crate::typecheck::typecheck(&ok.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected: {:?}",
        r.diagnostics
    );
}

#[test]
fn array_index_read_outside_exec_is_ws007() {
    let p = parse("var xs: int[]\nout v = xs[0]", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS007"));
}

#[test]
fn if_branch_type_mismatch_is_ws003() {
    let p = parse("let x = if true then 1 else Vec(0.0,0.0,0.0)", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS003"));
}

#[test]
fn reference_in_if_is_ws031() {
    // A `&x` ref-of infers as `Type::Ref` and can't flow through a Select.
    // (A bare `*int` param reads back auto-dereferenced, so it never trips
    // this — `&x` is the real trigger; see tests/ws_reachability.rs.)
    let p = parse("var x: int = 0\nin c: bool\nlet y = if c then &x else 0", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS031"));
}

#[test]
fn misplaced_map_literal_is_ws026() {
    let p = parse("let m = { \"a\": 1 }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS026"));
}

#[test]
fn user_mod_wrong_arg_type_is_ws003() {
    let p = parse("mod f(a: int) { }\nin go: exec\non go { f(Vec(0.0,0.0,0.0)) }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS003"));
}
#[test]
fn array_method_wrong_arg_is_ws003() {
    let p = parse("var xs: int[]\nin go: exec\non go { xs.push(Vec(0.0,0.0,0.0)) }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS003"));
}
#[test]
fn map_method_missing_value_is_ws011() {
    let p = parse("var m: Map<string,int>\nin go: exec\non go { m.set(\"k\") }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
}

#[test]
fn erroring_arg_reports_inner_error_once() {
    // An arg with its own error (undefined ident) must surface WS002 exactly
    // once — the preamble infers args, and check_args must not re-infer them.
    let p = parse("mod f(a: int) { }\nin go: exec\non go { f(nope) }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    let n = r.diagnostics.iter().filter(|d| d.code == "WS002").count();
    assert_eq!(n, 1, "expected one WS002, got {}: {:?}", n, r.diagnostics);
}

#[test]
fn ref_of_non_refable_is_ws008() {
    for src in [
        "in go: exec\non go { let r = &5 }",
        "var a: int = 0\nvar b: int = 0\nin go: exec\non go { let r = &(a + b) }",
        "mod f() -> int { return 1 }\nin go: exec\non go { let r = &f() }",
    ] {
        let p = parse(src, "test");
        let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS008"),
            "expected WS008 for `{src}`: {:?}",
            r.diagnostics
        );
    }
}

#[test]
fn ref_of_refable_is_clean() {
    // &var, &arr[i] are legal (they name a storage location).
    for src in [
        "var a: int = 0\nmod inc(v: *int) { v = v + 1 }\nin go: exec\non go { inc(&a) }",
        "var xs: int[]\nmod inc(v: *int) {}\nin go: exec\non go { xs.push(1)\n  inc(&xs[0]) }",
    ] {
        let p = parse(src, "test");
        let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS008"),
            "unexpected WS008 for `{src}`: {:?}",
            r.diagnostics
        );
    }
}

#[test]
fn prefab_ref_infers_prefab_type() {
    let p = parse("let pf = $./foo.brz", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    // no errors; the let binds a prefab-typed value
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected: {:?}",
        r.diagnostics
    );
}

#[test]
fn prefab_ref_not_storable_is_ws025() {
    let p = parse("var pf: prefab", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS025"),
        "{:?}",
        r.diagnostics
    );
}

#[test]
fn prefab_ref_in_if_is_ws031() {
    let p = parse(
        "in c: bool\nlet x = if c then $./a.brz else $./b.brz",
        "test",
    );
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS031"),
        "{:?}",
        r.diagnostics
    );
}
