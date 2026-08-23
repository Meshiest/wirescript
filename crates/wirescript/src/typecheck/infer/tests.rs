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
fn array_mutation_outside_exec_is_ws007() {
    // A pure-context mutation used to lower to a silent `_Unsupported`; it must
    // be a WS007 hard error like a pure index read.
    let p = parse("var xs: int[]\nchip {\n  xs.push(1)\n}", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS007"),
        "pure array mutation must be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn array_read_outside_exec_is_ws007() {
    // A pure-context READ (`length`/`find`/…) also needs exec — it lowers to an
    // `Exec_*` gate, and used to fall to a silent `_Unsupported`.
    let p = parse("var xs: int[]\nout r = xs.length()", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS007"),
        "pure runtime array read must be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn array_read_with_exec_arg_is_clean() {
    // An explicit `exec = <trigger>` arg supplies the exec context in a pure
    // binding, so it must NOT be WS007.
    let p = parse(
        "var lut: color[]\nin i: int\nout c: color = lut.get(i, exec = i + 1).Value",
        "test",
    );
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS007"),
        "a read with an exec= arg must not be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn const_array_read_is_not_ws007() {
    // A read on a `const` receiver should const-fold (a separate feature), so it
    // must not be reported as an exec-context error.
    let p = parse("const t = [1, 2, 3]\nconst n = t.length()\nout r = n", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS007"),
        "const array read must not be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn map_mutation_outside_exec_is_ws007() {
    let p = parse("var m: Map<int,int>\nchip {\n  m.set(1, 2)\n}", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS007"),
        "pure map mutation must be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn array_mutation_inside_exec_is_clean() {
    // The guard must NOT fire for a mutation inside an exec handler.
    let p = parse("var xs: int[]\nin go: exec\non go { xs.push(1) }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS007"),
        "exec-context mutation must not be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn let_shadowing_in_port_is_ws013() {
    // `let go` shadowing `in go: exec` used to silently hijack `on go` (the
    // handler bound to the constant, the exec input went dead).
    let p = parse("in go: exec\nlet go = 5\nout r: int\non go { emit r = 1 }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS013"),
        "a let shadowing an in/out port must be WS013: {:?}",
        r.diagnostics
    );
}

#[test]
fn let_value_shadowing_stays_clean() {
    // Rust-style value shadowing (`let a = 1; let a = 2`) is legal and must NOT
    // be flagged - only a port shadow is the bug.
    let p = parse("var x: int = 0\nin go: exec\non go { let a = 1\n let a = 2\n x = a }", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS013"),
        "value shadowing must not be WS013: {:?}",
        r.diagnostics
    );
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

#[test]
fn record_storage_rejects_non_variant_field() {
    // A record used as a variable/array/map decomposes into one gate per field,
    // so a reference-only field (zone/teleport/ref/prefab/exec) can't be stored.
    let p = parse("type T = { z: zone, n: int }\nvar t: T", "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(r.diagnostics.iter().any(|d| d.code == "WS049"), "{:?}", r.diagnostics);

    // A record ARRAY element and a record MAP value are checked the same way.
    let arr = parse("type T = { r: *int, n: int }\nvar ts: T[]", "test");
    let ra = crate::typecheck::typecheck(&arr.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(ra.diagnostics.iter().any(|d| d.code == "WS049"));

    // A value-only record (including nested + container fields) is accepted.
    let ok = parse(
        "type P = { x: int, y: int }\ntype Q = { p: P, tags: int[] }\nvar q: Q",
        "test",
    );
    let ro = crate::typecheck::typecheck(&ok.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(!ro.diagnostics.iter().any(|d| d.code == "WS049"), "{:?}", ro.diagnostics);
}

#[test]
fn null_typing() {
    let tc = |src: &str| {
        let p = parse(src, "test");
        crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
    };
    // Accepted for value types (var/out/assign/arg/record field).
    let ok = tc(
        "type P = { x: int, e: entity }\nmod f(n: int, ent: entity) { }\n\
         var i: int = null\nvar e: entity = null\nvar p: P = { x: null, e: null }\n\
         in go: exec\non go { i = null\n f(null, null) }",
    );
    assert!(!ok.diagnostics.iter().any(|d| d.code == "WS051"), "{:?}", ok.diagnostics);
    // Rejected for types with no null value: container, record, reference-only.
    for bad in ["var a: int[] = null", "type P = { x: int }\nvar p: P = null"] {
        let r = tc(bad);
        assert!(r.diagnostics.iter().any(|d| d.code == "WS051"), "{bad}: {:?}", r.diagnostics);
    }
}

#[test]
fn spread_arg_typing() {
    let tc = |src: &str| {
        let p = parse(src, "test");
        crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
    };
    // Over-length spread into a fixed-arity mod -> WS022.
    let over = tc("mod add2(a: int, b: int) -> int { return a + b }\nout r = add2(...(1, 2, 3))");
    assert!(over.diagnostics.iter().any(|d| d.code == "WS022"), "{:?}", over.diagnostics);
    // A spread of a non-tuple -> WS003.
    let bad = tc("in n: int\nin go: exec\non go { SendGlobalCustomEvent(\"c\", ...n) }");
    assert!(bad.diagnostics.iter().any(|d| d.code == "WS003"), "{:?}", bad.diagnostics);
    // A correct-arity spread is clean.
    let ok = tc("mod add2(a: int, b: int) -> int { return a + b }\nin n: int\nout r = add2(...(n, 2))");
    assert!(!ok.diagnostics.iter().any(|d| matches!(d.code.as_str(), "WS022" | "WS003")), "{:?}", ok.diagnostics);
    // A spread ELEMENT whose type mismatches its user-mod param is caught (arity
    // is right, but the second element is a vector for an int param): the same
    // WS003 the written-out `add2(1, aVector)` gets.
    let bad_elem = tc(
        "mod add2(a: int, b: int) -> int { return a + b }\n\
         let t = (1, Vec(0.0, 0.0, 0.0))\nout r = add2(...t)",
    );
    assert!(
        bad_elem.diagnostics.iter().any(|d| d.code == "WS003"),
        "spread element type mismatch into a user mod must be WS003: {:?}",
        bad_elem.diagnostics
    );
}

#[test]
fn variadic_mod_arity_and_placement() {
    let tc = |src: &str| {
        let p = parse(src, "test");
        crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
    };
    // A variadic mod accepts extra trailing args past its fixed params: clean.
    let ok = tc(
        "mod f(a: int, ...rest) -> int { return a }\n\
         var t: int = 0\nin go: exec\non go { t = f(1, 2, 3, 4) }",
    );
    assert!(
        !ok.diagnostics.iter().any(|d| d.code == "WS022"),
        "variadic accepts surplus args: {:?}",
        ok.diagnostics
    );
    // ...but still requires the FIXED params -> WS022 when too few.
    let few = tc(
        "mod g(a: int, b: int, ...rest) -> int { return a + b }\n\
         var t: int = 0\nin go: exec\non go { t = g(1) }",
    );
    assert!(
        few.diagnostics.iter().any(|d| d.code == "WS022"),
        "too few for the fixed params: {:?}",
        few.diagnostics
    );
    // A `...rest` on a physical chip (not a mod) is rejected -> WS052.
    let chip = tc("chip Bad(a: int, ...rest) { out r = a }");
    assert!(
        chip.diagnostics.iter().any(|d| d.code == "WS052"),
        "variadic chip rejected: {:?}",
        chip.diagnostics
    );
}

#[test]
fn parked_kick_emit_before_await() {
    let tc = |src: &str| {
        let p = parse(src, "test");
        crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
    };
    // A plain `emit s` then a same-chain `await s` parks forever -> WS053 (warning).
    let bad = tc(
        "in go: exec\nlet s: exec\nvar n: int = 0\n\
         on go { emit s\n  await s\n  if n < 5 { buffer emit s } }",
    );
    assert!(
        bad.diagnostics
            .iter()
            .any(|d| d.code == "WS053" && d.severity == crate::diagnostic::Severity::Warning),
        "plain emit-before-await must warn WS053: {:?}",
        bad.diagnostics
    );
    // A buffered kick is correct (lands next tick, after the await arms) -> no WS053.
    let ok = tc(
        "in go: exec\nlet s: exec\nvar n: int = 0\n\
         on go { buffer emit s\n  await s\n  if n < 5 { buffer emit s } }",
    );
    assert!(
        !ok.diagnostics.iter().any(|d| d.code == "WS053"),
        "buffered kick must not warn: {:?}",
        ok.diagnostics
    );
    // Emit and await of DIFFERENT signals -> no WS053.
    let diff = tc("in go: exec\nlet s: exec\nlet t: exec\non go { emit s\n  await t }");
    assert!(
        !diff.diagnostics.iter().any(|d| d.code == "WS053"),
        "different signals must not warn: {:?}",
        diff.diagnostics
    );
}

#[test]
fn await_custom_event_captures_data() {
    use crate::diagnostic::Severity;
    let tc = |src: &str| {
        let p = parse(src, "test");
        crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
    };
    // An ANNOTATED capture types the value (DataOut1) and is clean.
    let typed = tc(
        "in go: exec\nvar a: int = 0\n\
         on go { let foo: int = await CustomEvent(\"c\")\n  a = foo }",
    );
    assert!(
        !typed.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "typed event capture must be accepted: {:?}",
        typed.diagnostics
    );
    assert!(
        !typed.diagnostics.iter().any(|d| d.code == "WS055"),
        "an annotated capture must not warn WS055: {:?}",
        typed.diagnostics
    );
    // An UNANNOTATED capture still works but warns WS055 (type can't be inferred).
    let bare = tc(
        "in go: exec\nvar a: int = 0\n\
         on go { let foo = await CustomEvent(\"c\")\n  a = foo }",
    );
    assert!(
        bare.diagnostics
            .iter()
            .any(|d| d.code == "WS055" && d.severity == Severity::Warning),
        "untyped event capture must warn WS055: {:?}",
        bare.diagnostics
    );
    // A bare `await CustomEvent(...)` (no capture) just waits - no warning.
    let wait = tc("in go: exec\nvar a: int = 0\non go { await CustomEvent(\"c\")\n  a = 1 }");
    assert!(
        !wait.diagnostics.iter().any(|d| d.code == "WS055"),
        "bare await (no capture) must not warn: {:?}",
        wait.diagnostics
    );
    // The handler receiver form is still fine.
    let ok = tc("var total: int = 0\non CustomEvent(\"c\") -> (p: int, t: int) { total = p + t }");
    assert!(
        !ok.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "handler receiver must be accepted: {:?}",
        ok.diagnostics
    );
}
