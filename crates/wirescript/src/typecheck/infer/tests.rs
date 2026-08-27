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

/// Runs the full two-pass typecheck over `src` and returns the display
/// string of the type recorded in `type_of_expr` for the expression whose
/// exact source text is `needle` (its byte range in `src`, via `str::find`).
/// Reusable across tests that need the type of an arbitrary sub-expression
/// rather than just a top-level `let`'s initializer (`first_expr` above).
/// Panics with the diagnostics if `needle` isn't found or has no recorded
/// type, so a broken test fails loudly instead of comparing against `None`.
fn type_of(src: &str, needle: &str) -> String {
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    let start = src
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} not found in src {src:?}"));
    let end = start + needle.len();
    let file: std::sync::Arc<str> = std::sync::Arc::from("test");
    match r.type_of_expr.get(&(file, start, end)) {
        Some(t) => t.to_string(),
        None => panic!(
            "no recorded type for {needle:?} (offsets {start}..{end}); diagnostics: {:?}",
            r.diagnostics
        ),
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
fn container_param_read_from_pure_mod_is_ws007() {
    // A mod that reads a container reached through its own PARAMETER is
    // exec-requiring; a pure call to it must be WS007, not a silent miscompile
    // (the map ref used to wire straight into the consumer). The scan resolves
    // the receiver against the mod's parameters, not the caller's scope.
    let p = parse(
        "var mm: Map<int,int> = { 0: 1 }\n\
         mod rd(m: Map<int,int>) -> (r: int) { let v = m.get(0)\n  return if v.Found then v.Value else -1 }\n\
         out r = rd(mm)",
        "test",
    );
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS007"),
        "pure call of a param-container-reading mod must be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn container_param_read_is_transitive_ws007() {
    // A -> B where B reads a param container: a pure call of A must still be
    // WS007 (the call-graph fixpoint propagates B's exec-requirement to A).
    let p = parse(
        "var mm: Map<int,int> = { 0: 1 }\n\
         mod rb(m: Map<int,int>) -> (r: int) { let v = m.get(0)\n  return if v.Found then v.Value else -1 }\n\
         mod ca(m: Map<int,int>) -> (r: int) { return rb(m) }\n\
         out r = ca(mm)",
        "test",
    );
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS007"),
        "transitive param-container read must be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn non_container_param_shadowing_global_is_not_ws007() {
    // A mod parameter named like a global container but typed as a scalar must
    // SHADOW that global in the scan, so a plain read of the parameter is not
    // mistaken for a container op (no false-positive WS007).
    let p = parse(
        "var mm: Map<int,int> = { 0: 1 }\n\
         mod pass(mm: int) -> (r: int) { return mm + 1 }\n\
         out r = pass(5)",
        "test",
    );
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS007"),
        "a scalar param shadowing a global container must not be WS007: {:?}",
        r.diagnostics
    );
}

#[test]
fn container_param_read_from_exec_is_clean() {
    // The same param-container-reading mod is legal when called from an exec
    // handler - the scan only fires the call-site WS007 in pure position.
    let p = parse(
        "var mm: Map<int,int> = { 0: 1 }\n\
         mod rd(m: Map<int,int>) -> (r: int) { let v = m.get(0)\n  return if v.Found then v.Value else -1 }\n\
         in go: exec\n\
         out r: int\n\
         on go { emit r = rd(mm) }",
        "test",
    );
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS007"),
        "an exec-context call of a container mod must not be WS007: {:?}",
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

#[test]
fn variant_path_discriminant_is_int() {
    let src = "enum Shape { Empty, Circle(float) }\nout d = Shape.Circle.Discriminant\n";
    assert_eq!(type_of(src, "Shape.Circle.Discriminant"), "int");
}

#[test]
fn value_to_int_types_as_int_like_discriminant() {
    // `value.ToInt()` is an exact alias for `.Discriminant`: it projects an
    // enum value to its integer tag and types as int, on both a stored value
    // and a variant path.
    let src = "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(1.0)\n\
               out a = s.ToInt()\nout b = Shape.Circle.ToInt()\n";
    assert_eq!(type_of(src, "s.ToInt()"), "int");
    assert_eq!(type_of(src, "Shape.Circle.ToInt()"), "int");
}

#[test]
fn enum_from_int_types_as_the_enum() {
    // `Enum.FromInt(n)` constructs a value of that enum from an int tag; it
    // types as the enum, and its argument type-checks as an int.
    let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
               in n: int\nout e = Shape.FromInt(n)\n";
    assert_eq!(type_of(src, "Shape.FromInt(n)"), "Shape");
}

#[test]
fn enum_from_int_rejects_non_int_argument() {
    // The single argument is an int; a string argument is WS003.
    let src = "enum Shape { Empty, Circle(float) }\nout e = Shape.FromInt(\"x\")\n";
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS003"),
        "a non-int FromInt argument must be WS003: {:?}",
        r.diagnostics
    );
}

#[test]
fn variant_named_from_int_still_constructs() {
    // A real variant literally named `FromInt` wins over the tag-only
    // constructor: `E.FromInt(3)` builds that unit... payload variant, typing
    // as the enum, with no WS060.
    let src = "enum E { A, FromInt(int) }\nout e = E.FromInt(3)\n";
    assert_eq!(type_of(src, "E.FromInt(3)"), "E");
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.severity == crate::diagnostic::Severity::Error),
        "a real FromInt variant must construct without error: {:?}",
        r.diagnostics
    );
}

#[test]
fn enum_to_integer_types_as_int_and_requires_an_enum() {
    // `EnumToInt(value)` projects an enum value to its integer tag and types
    // as int - the gate-backed twin of `.ToInt()`.
    let src = "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(1.0)\n\
               out a = EnumToInt(s)\n";
    assert_eq!(type_of(src, "EnumToInt(s)"), "int");
}

#[test]
fn enum_to_integer_rejects_a_non_enum_argument() {
    // The argument MUST be an enum: an int or a string is a WS003 argument-type
    // error, not an accepted `any`.
    for bad in ["EnumToInt(5)", "EnumToInt(\"x\")"] {
        let src = format!("enum Shape {{ Empty, Circle(float) }}\nout a = {bad}\n");
        let p = parse(&src, "test");
        let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "`{bad}` must be a WS003 non-enum argument error: {:?}",
            r.diagnostics
        );
    }
}

#[test]
fn integer_to_enum_result_is_the_context_enum() {
    // `IntToEnum(value)` produces an enum whose concrete type is pinned by
    // the annotated target (like `Enum.FromInt`/`null`), and its int argument
    // type-checks.
    let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\n\
               in n: int\nlet e: Shape = IntToEnum(n)\n";
    assert_eq!(type_of(src, "IntToEnum(n)"), "Shape");
}

#[test]
fn integer_to_enum_without_an_enum_context_is_ws063() {
    // With no enum-typed expectation there is no way to know which enum the
    // integer names - WS063, mirroring `FromInt`'s type-inference failure.
    let src = "enum Shape { Empty, Circle(float) }\nin n: int\nlet e = IntToEnum(n)\n";
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS063"),
        "an un-pinned IntToEnum must be WS063: {:?}",
        r.diagnostics
    );
}

#[test]
fn value_shadowing_an_enum_name_is_ordinary_field_access() {
    // A param named the same as a top-level enum shadows the type (nearest-first
    // scope), so `Shape.x` is a record field access, not a variant path: it must
    // type as the field and NOT emit WS060.
    let src = "enum Shape { Empty, Circle }\ntype Rec = { x: int }\n\
               mod f(Shape: Rec) -> int {\n    return Shape.x\n}\n";
    assert_eq!(type_of(src, "Shape.x"), "int");
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS060"),
        "shadowed enum name must not emit WS060: {:?}",
        r.diagnostics
    );
}

#[test]
fn positional_and_named_construction_type_as_the_enum() {
    let src = "enum Shape { Empty, Circle(float), Box { w: float, h: float } }\n\
               out a = Shape.Circle(5.0)\nout b = Shape.Box { w: 1.0, h: 2.0 }\nout c = Shape.Empty\n";
    assert_eq!(type_of(src, "Shape.Circle(5.0)"), "Shape");
    assert_eq!(type_of(src, "Shape.Box { w: 1.0, h: 2.0 }"), "Shape");
    assert_eq!(type_of(src, "Shape.Empty"), "Shape");
}

#[test]
fn match_binds_payload_and_joins_arm_types() {
    let src = "enum Shape { Circle(float), Rect(float, float) }\nin s: Shape\n\
               out area = match s { Circle(r) => r, Rect(w, h) => w }\n";
    assert_eq!(type_of(src, "match s { Circle(r) => r, Rect(w, h) => w }"), "float");
}

// Captures must bind their CONCRETE field types, not `any`: two arms returning
// captures of incompatible types (int vs string) have no common widening, so
// the arm-result join is WS003. If either capture bound `any` this would join
// cleanly and no diagnostic would fire, so this locks in the field typing.
#[test]
fn match_arm_capture_type_mismatch_is_ws003() {
    let src = "enum P { I(int), S(string) }\nin p: P\n\
               out r = match p { I(n) => n, S(s) => s }\n";
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS003"),
        "expected WS003 from incompatible arm captures: {:?}",
        r.diagnostics
    );
}

#[test]
fn generic_variant_infers_and_annotates() {
    let src = "enum Option<T> { Some(T), None }\nout a = Option.Some(42)\n";
    assert_eq!(type_of(src, "Option.Some(42)"), "Option<int>");
    let src2 = "enum Option<T> { Some(T), None }\nout n: Option<int> = Option.None\n";
    assert_eq!(type_of(src2, "Option.None"), "Option<int>");
}

// `Option<T>`/`Result<T, E>` are built in - no `enum` declaration
// needed - and their variants are usable bare (`Some`/`None`/`Ok`/`Err`
// instead of `Option.Some`/`Option.None`/`Result.Ok`/`Result.Err`).
#[test]
fn prelude_option_result_are_builtin() {
    let src = "out a = Some(42)\nout b: Option<int> = None\nout c = Ok(1)\nout d: Result<int, int> = Err(2)\n";
    assert_eq!(type_of(src, "Some(42)"), "Option<int>");
    assert_eq!(type_of(src, "Ok(1)"), "Result<int, int>");
    assert_eq!(type_of(src, "None"), "Option<int>");
    assert_eq!(type_of(src, "Err(2)"), "Result<int, int>");
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.is_empty(),
        "the prelude round-trip must be clean: {:?}",
        r.diagnostics
    );
}

// A user symbol of the SAME name wins over the prelude's bare variant -
// resolution falls back to a prelude variant ONLY when the name is otherwise
// undefined.
#[test]
fn a_user_symbol_named_some_shadows_the_prelude_variant() {
    // A `let` named `Some` claims the scope symbol, so the bare `Some` used
    // in `out a`'s initializer reads the LOCAL value (int), not the prelude
    // variant. `type_of` matches its needle's FIRST occurrence, which here is
    // the `let`'s own name (not an expression) - so this looks up the
    // initializer's recorded range directly via `rfind` instead.
    let src = "let Some = 5\nout a = Some\n";
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    let needle = "Some";
    let start = src.rfind(needle).expect("needle not found");
    let end = start + needle.len();
    let file: std::sync::Arc<str> = std::sync::Arc::from("test");
    let ty = r
        .type_of_expr
        .get(&(file, start, end))
        .unwrap_or_else(|| panic!("no recorded type for the referencing `Some`; diagnostics: {:?}", r.diagnostics));
    assert_eq!(ty.to_string(), "int", "the local `Some` must win over the prelude variant");

    // A user's OWN enum with a variant also named `Some` makes the bare name
    // AMBIGUOUS between it and the prelude's `Option.Some` - bare resolution
    // backs off rather than guessing, so `Some(1)` is an unresolved call
    // (WS002), not a silent (and possibly wrong) pick of either enum.
    let src2 = "enum Custom<T> { Some(T), Other }\nout a = Some(1)\n";
    let p2 = parse(src2, "test");
    let r2 = crate::typecheck::typecheck(&p2.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r2.diagnostics.iter().any(|d| d.code == "WS002"),
        "an ambiguous bare variant name must not silently resolve to either enum: {:?}",
        r2.diagnostics
    );
}

// `Result<T, E>`'s unconstrained-sibling default (an unannotated `Ok(1)`
// reads `E` from `T`, since `Ok`'s payload never mentions `E` at all) must be
// ORDER-INDEPENDENT: `Err(2)` alone must read `T` from `E` exactly the same
// way, even though `T` is `Result`'s FIRST declared type param and so is
// solved BEFORE `E` in declaration order. A single left-to-right fallback
// pass would default `T` before `E` has a value to borrow, mis-diagnosing
// `Err(2)` alone as WS063 while `Ok(1)` alone works cleanly.
#[test]
fn unconstrained_sibling_default_is_order_independent() {
    assert_eq!(type_of("out c = Ok(1)\n", "Ok(1)"), "Result<int, int>");
    let src = "out d = Err(2)\n";
    assert_eq!(type_of(src, "Err(2)"), "Result<int, int>");
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.is_empty(),
        "an unannotated `Err(2)` must resolve cleanly, matching `Ok(1)`'s treatment: {:?}",
        r.diagnostics
    );
}

// `EasingFunction` is a registry enum seeded by
// `register_builtin_enums` with NO `enum` declaration anywhere in
// source. `.Discriminant` on a bare variant path must type exactly like the
// hand-written case `variant_path_discriminant_is_int` above tests.
#[test]
fn builtin_game_enum_variant_path_discriminant_is_int() {
    let src = "out d = EasingFunction.Bounce.Discriminant\n";
    assert_eq!(type_of(src, "EasingFunction.Bounce.Discriminant"), "int");
}

// A `static var` typed and
// initialized with a built-in enum, compared against a bare variant path's
// `.Discriminant`. Must type-check with zero diagnostics, same as an
// equivalent hand-written enum would.
#[test]
fn builtin_game_enum_stored_var_and_comparison_typecheck_clean() {
    let src = "static var e: EasingFunction = EasingFunction.Linear\n\
               out m = e.Discriminant == EasingFunction.Bounce.Discriminant\n";
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.is_empty(),
        "a built-in game enum var + discriminant comparison must type-check clean: {:?}",
        r.diagnostics
    );
    assert_eq!(
        type_of(src, "e.Discriminant == EasingFunction.Bounce.Discriminant"),
        "bool"
    );
}

// A `match` is exhaustive only when it covers every variant the SCHEMA
// declares - built dynamically from `catalog::builtin_game_enums()` rather
// than a hardcoded variant list, so this stays correct if the schema's
// variant set ever changes. Covering all of them must stay WS054-free;
// dropping the last one must surface WS054 naming exactly that variant -
// proving exhaustiveness is checked against the real registry entry the
// built-in seeds, not an empty/stub one.
#[test]
fn builtin_game_enum_match_exhaustive_only_over_schema_variants() {
    let variants: Vec<String> = crate::catalog::builtin_game_enums()
        .into_iter()
        .find(|e| e.clean_name == "EasingFunction")
        .expect("EasingFunction is a built-in game enum")
        .variants
        .into_iter()
        .map(|v| v.clean_name)
        .collect();
    assert!(
        variants.len() > 1,
        "EasingFunction must have at least two schema variants for this test to be meaningful"
    );

    let arms = |names: &[String]| -> String {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| format!("{name} => {i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let full_src = format!(
        "static var e: EasingFunction = EasingFunction.{}\nout r = match e {{ {} }}\n",
        variants[0],
        arms(&variants)
    );
    let p = parse(&full_src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS054"),
        "a match covering every schema variant must not be flagged non-exhaustive: {:?}",
        r.diagnostics
    );

    let missing = variants.last().unwrap();
    let short_src = format!(
        "static var e: EasingFunction = EasingFunction.{}\nout r = match e {{ {} }}\n",
        variants[0],
        arms(&variants[..variants.len() - 1])
    );
    let p2 = parse(&short_src, "test");
    let r2 = crate::typecheck::typecheck(&p2.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r2.diagnostics
            .iter()
            .any(|d| d.code == "WS054" && d.message.contains(missing.as_str())),
        "dropping `{missing}` must surface WS054 naming it: {:?}",
        r2.diagnostics
    );
}

// A qualified built-in enum value passed as a matching gate config
// argument type-checks clean, exactly like the legacy bare member name.
#[test]
fn qualified_builtin_enum_config_arg_typechecks() {
    let qualified =
        "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = EasingFunction.Bounce) }";
    let p = parse(qualified, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.is_empty(),
        "a qualified built-in enum config value must type-check clean: {:?}",
        r.diagnostics
    );

    // The legacy bare member-name form is unchanged.
    let bare = "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = Bounce) }";
    let p2 = parse(bare, "test");
    let r2 = crate::typecheck::typecheck(&p2.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r2.diagnostics.is_empty(),
        "the legacy bare enum member name must still type-check clean: {:?}",
        r2.diagnostics
    );
}

// A built-in enum value of the WRONG enum at that config arg is an argument
// type mismatch (WS003), not a silent accept.
#[test]
fn wrong_builtin_enum_config_arg_is_ws003() {
    let src =
        "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = ColorSpace.Srgb) }";
    let p = parse(src, "test");
    let r = crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default());
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS003"),
        "a wrong-enum config value must be WS003: {:?}",
        r.diagnostics
    );
}
