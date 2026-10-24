use crate::diagnostic::{Diagnostic, Severity};
use crate::ir::Type;
use crate::parser::parse;
use crate::test_support::{assert_code_count, report_failures};
use crate::typecheck::infer::{check, infer};
use crate::typecheck::{TypeCheckCtx, TypeCheckResult};

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

fn tc(src: &str) -> TypeCheckResult {
    let p = parse(src, "test");
    crate::typecheck::typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
}

/// Runs the full two-pass typecheck over `src` and returns the display
/// string of the type recorded in `type_of_expr` for the expression whose
/// exact source text is `needle` (its byte range in `src`, via `str::find`).
/// Errs with the diagnostics if `needle` isn't found or has no recorded type.
fn type_of(src: &str, needle: &str) -> Result<String, String> {
    let r = tc(src);
    let start = src
        .find(needle)
        .ok_or_else(|| format!("needle {needle:?} not found in src {src:?}"))?;
    let end = start + needle.len();
    let file: std::sync::Arc<str> = std::sync::Arc::from("test");
    match r.type_of_expr.get(&(file, start, end)) {
        Some(t) => Ok(t.to_string()),
        None => Err(format!(
            "no recorded type for {needle:?} (offsets {start}..{end}); diagnostics: {:?}",
            r.diagnostics
        )),
    }
}

/// What a type-checked program's diagnostics must show.
#[derive(Clone, Copy, Debug)]
enum Want {
    /// Some diagnostic has this code.
    Has(&'static str),
    /// Some warning has this code.
    Warns(&'static str),
    /// No diagnostic has this code.
    Lacks(&'static str),
    /// No error-severity diagnostic.
    NoErrors,
    /// No diagnostic at all.
    NoDiags,
}
use Want::*;

fn holds(diags: &[Diagnostic], want: Want) -> bool {
    match want {
        Has(c) => diags.iter().any(|d| d.code == c),
        Warns(c) => diags
            .iter()
            .any(|d| d.code == c && d.severity == Severity::Warning),
        Lacks(c) => !diags.iter().any(|d| d.code == c),
        NoErrors => !diags.iter().any(|d| d.severity == Severity::Error),
        NoDiags => diags.is_empty(),
    }
}

/// Type-check each `(label, src, wants)` row and report every row with an
/// unmet want.
fn diag_table(rows: &[(&str, &str, &[Want])]) {
    let mut failures = Vec::new();
    for (label, src, wants) in rows {
        let r = tc(src);
        let unmet: Vec<&Want> = wants
            .iter()
            .filter(|w| !holds(&r.diagnostics, **w))
            .collect();
        if !unmet.is_empty() {
            let got: Vec<String> = r
                .diagnostics
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            failures.push(format!(
                "{label}: unmet {unmet:?}\n  src: {src:?}\n  got: {got:#?}"
            ));
        }
    }
    report_failures(&failures, rows.len());
}

#[test]
fn infer_and_check_on_a_bare_context() {
    let mut c = ctx();
    let e = first_expr("let x = 5");
    assert_eq!(infer(&mut c, &e), Type::Int);
    let r = e.range();
    assert_eq!(
        c.type_of_expr
            .get(&(r.file.clone(), r.start.offset, r.end.offset)),
        Some(&Type::Int),
        "infer records the type of what it inferred"
    );
    assert_eq!(infer(&mut c, &first_expr("let x = 5.0")), Type::Float);
    assert_eq!(infer(&mut c, &first_expr("let x = true")), Type::Bool);
    assert_eq!(infer(&mut c, &first_expr("let x = \"s\"")), Type::String);
    assert!(c.diagnostics.is_empty(), "{:?}", c.diagnostics);

    // string does not coerce to vector.
    check(&mut c, &first_expr("let x = \"hi\""), &Type::Vector);
    assert!(
        c.diagnostics.iter().any(|d| d.code == "WS003"),
        "{:?}",
        c.diagnostics
    );
}

#[test]
fn ident_var_auto_derefs_and_records_context() {
    // A `var` read yields the inner type and records its exec/pure context.
    let r = tc("var n: int = 0\nin go: exec\non go { let x = n }");
    assert!(
        holds(&r.diagnostics, NoErrors),
        "unexpected: {:?}",
        r.diagnostics
    );
    assert!(!r.var_read_contexts.is_empty());
}

#[test]
fn binop_records_op_resolution() {
    let r = tc("let x = 1 + 2");
    assert!(!r.op_resolutions.is_empty());
    assert!(
        holds(&r.diagnostics, NoErrors),
        "unexpected: {:?}",
        r.diagnostics
    );
}

#[test]
fn erroring_arg_reports_inner_error_once() {
    // An arg with its own error (undefined ident) must surface WS002 exactly
    // once — the preamble infers args, and check_args must not re-infer them.
    let r = tc("mod f(a: int) { }\nin go: exec\non go { f(nope) }");
    assert_code_count(&r.diagnostics, "WS002", 1);
}

#[test]
fn pure_and_exec_context_codes() {
    diag_table(&[
        (
            "deref in a pure context",
            "var n: int = 0\nout v = *n",
            &[Has("WS006")],
        ),
        // A pure-context mutation is a WS007 hard error like a pure index read,
        // not a silent `_Unsupported`.
        (
            "array mutation outside exec",
            "var xs: int[]\nchip {\n  xs.push(1)\n}",
            &[Has("WS007")],
        ),
        // A pure-context READ (`length`/`find`/…) also needs exec: it lowers to
        // an `Exec_*` gate.
        (
            "array read outside exec",
            "var xs: int[]\nout r = xs.length()",
            &[Has("WS007")],
        ),
        // An explicit `exec = <trigger>` arg supplies the exec context in a pure
        // binding, so it must NOT be WS007.
        (
            "array read with an exec= arg",
            "var lut: color[]\nin i: int\nout c: color = lut.get(i, exec = i + 1).Value",
            &[Lacks("WS007")],
        ),
        // A read on a `const` receiver const-folds, so it is not an exec-context error.
        (
            "const array read",
            "const t = [1, 2, 3]\nconst n = t.length()\nout r = n",
            &[Lacks("WS007")],
        ),
        (
            "map mutation outside exec",
            "var m: Map<int,int>\nchip {\n  m.set(1, 2)\n}",
            &[Has("WS007")],
        ),
        // A mod that reads a container reached through its own PARAMETER is
        // exec-requiring; a pure call to it must be WS007, not a silent miscompile
        // (the map ref used to wire straight into the consumer). The scan resolves
        // the receiver against the mod's parameters, not the caller's scope.
        (
            "pure call of a param-container-reading mod",
            "var mm: Map<int,int> = { 0: 1 }\n\
             mod rd(m: Map<int,int>) -> (r: int) { let v = m.get(0)\n  return if v.Found then v.Value else -1 }\n\
             out r = rd(mm)",
            &[Has("WS007")],
        ),
        // A -> B where B reads a param container: a pure call of A must still be
        // WS007 (the call-graph fixpoint propagates B's exec-requirement to A).
        (
            "transitive param-container read",
            "var mm: Map<int,int> = { 0: 1 }\n\
             mod rb(m: Map<int,int>) -> (r: int) { let v = m.get(0)\n  return if v.Found then v.Value else -1 }\n\
             mod ca(m: Map<int,int>) -> (r: int) { return rb(m) }\n\
             out r = ca(mm)",
            &[Has("WS007")],
        ),
        // A mod parameter named like a global container but typed as a scalar must
        // SHADOW that global in the scan (no false-positive WS007).
        (
            "scalar param shadowing a global container",
            "var mm: Map<int,int> = { 0: 1 }\n\
             mod pass(mm: int) -> (r: int) { return mm + 1 }\n\
             out r = pass(5)",
            &[Lacks("WS007")],
        ),
        // The same param-container-reading mod is legal when called from an exec
        // handler - the scan only fires the call-site WS007 in pure position.
        (
            "exec-context call of a container mod",
            "var mm: Map<int,int> = { 0: 1 }\n\
             mod rd(m: Map<int,int>) -> (r: int) { let v = m.get(0)\n  return if v.Found then v.Value else -1 }\n\
             in go: exec\n\
             out r: int\n\
             on go { emit r = rd(mm) }",
            &[Lacks("WS007")],
        ),
        (
            "array mutation inside exec",
            "var xs: int[]\nin go: exec\non go { xs.push(1) }",
            &[Lacks("WS007")],
        ),
        // `let go` shadowing `in go: exec` used to silently hijack `on go` (the
        // handler bound to the constant, the exec input went dead).
        (
            "let shadowing an in port",
            "in go: exec\nlet go = 5\nout r: int\non go { emit r = 1 }",
            &[Has("WS013")],
        ),
        // Rust-style value shadowing (`let a = 1; let a = 2`) is legal; only a port
        // shadow is the bug.
        (
            "let value shadowing",
            "var x: int = 0\nin go: exec\non go { let a = 1\n let a = 2\n x = a }",
            &[Lacks("WS013")],
        ),
    ]);
}

#[test]
fn value_typing_codes() {
    diag_table(&[
        // An interpolation part that can't format to string is WS003.
        (
            "non-stringable interpolation part",
            "var z: zone\nlet s = \"x ${z}\"",
            &[Has("WS003")],
        ),
        (
            "prefab ref without .brz",
            "let s = $./level",
            &[Has("WS019")],
        ),
        // string + int has no arithmetic overload.
        ("string + int", "let x = \"a\" + 1", &[Has("WS004")]),
        // `&` has rules for int/float/bool operands only.
        (
            "vector & int",
            "let y = Vec(0.0, 0.0, 0.0) & 2",
            &[Has("WS011")],
        ),
        (
            "if branch type mismatch",
            "let x = if true then 1 else Vec(0.0,0.0,0.0)",
            &[Has("WS003")],
        ),
        (
            "misplaced map literal",
            "let m = { \"a\": 1 }",
            &[Has("WS026")],
        ),
        (
            "user mod wrong arg type",
            "mod f(a: int) { }\nin go: exec\non go { f(Vec(0.0,0.0,0.0)) }",
            &[Has("WS003")],
        ),
        (
            "array method wrong arg",
            "var xs: int[]\nin go: exec\non go { xs.push(Vec(0.0,0.0,0.0)) }",
            &[Has("WS003")],
        ),
        // `.x` on a vector is float; a field the record lacks is WS010.
        (
            "vector field access",
            "let v = Vec(1.0,2.0,3.0)\nlet a = v.x",
            &[NoErrors],
        ),
        (
            "missing record field",
            "type P = { x: int }\nlet p: P = { x: 1 }\nlet a = p.y",
            &[Has("WS010")],
        ),
        (
            "& of a literal",
            "in go: exec\non go { let r = &5 }",
            &[Has("WS008")],
        ),
        (
            "& of an expression",
            "var a: int = 0\nvar b: int = 0\nin go: exec\non go { let r = &(a + b) }",
            &[Has("WS008")],
        ),
        (
            "& of a call",
            "mod f() -> int { return 1 }\nin go: exec\non go { let r = &f() }",
            &[Has("WS008")],
        ),
        // An element of a value array has no ref of its own: only the whole array
        // is a reference.
        (
            "& of a value array element",
            "var xs: int[]\nmod inc(v: *int) {}\nin go: exec\non go { xs.push(1)\n  inc(&xs[0]) }",
            &[Has("WS008")],
        ),
        ("prefab var", "var pf: prefab", &[Has("WS025")]),
        (
            "prefab ref in if",
            "in c: bool\nlet x = if c then $./a.brz else $./b.brz",
            &[Has("WS031")],
        ),
        // A record used as a variable/array/map decomposes into one gate per field,
        // so a reference-only field (zone/teleport/ref/prefab/exec) can't be stored.
        (
            "record var with a zone field",
            "type T = { z: zone, n: int }\nvar t: T",
            &[Has("WS049")],
        ),
        (
            "record array with a ref field",
            "type T = { r: *int, n: int }\nvar ts: T[]",
            &[Has("WS049")],
        ),
        // A value-only record (including nested + container fields) is accepted.
        (
            "value-only nested record",
            "type P = { x: int, y: int }\ntype Q = { p: P, tags: int[] }\nvar q: Q",
            &[Lacks("WS049")],
        ),
        // `null` is accepted for value types (var/out/assign/arg/record field).
        (
            "null for value types",
            "type P = { x: int, e: entity }\nmod f(n: int, ent: entity) { }\n\
             var i: int = null\nvar e: entity = null\nvar p: P = { x: null, e: null }\n\
             in go: exec\non go { i = null\n f(null, null) }",
            &[Lacks("WS051")],
        ),
        // ...and rejected for types with no null value: container, record.
        ("null array", "var a: int[] = null", &[Has("WS051")]),
        (
            "null record",
            "type P = { x: int }\nvar p: P = null",
            &[Has("WS051")],
        ),
        (
            "over-length spread into a fixed-arity mod",
            "mod add2(a: int, b: int) -> int { return a + b }\nout r = add2(...(1, 2, 3))",
            &[Has("WS022")],
        ),
        (
            "spread of a non-tuple",
            "in n: int\nin go: exec\non go { SendGlobalCustomEvent(\"c\", ...n) }",
            &[Has("WS003")],
        ),
        (
            "correct-arity spread",
            "mod add2(a: int, b: int) -> int { return a + b }\nin n: int\nout r = add2(...(n, 2))",
            &[Lacks("WS022"), Lacks("WS003")],
        ),
        // A spread ELEMENT whose type mismatches its user-mod param gets the same
        // WS003 the written-out `add2(1, aVector)` gets.
        (
            "spread element type mismatch",
            "mod add2(a: int, b: int) -> int { return a + b }\n\
             let t = (1, Vec(0.0, 0.0, 0.0))\nout r = add2(...t)",
            &[Has("WS003")],
        ),
        // A variadic mod accepts extra trailing args past its fixed params.
        (
            "variadic surplus args",
            "mod f(a: int, ...rest) -> int { return a }\n\
             var t: int = 0\nin go: exec\non go { t = f(1, 2, 3, 4) }",
            &[Lacks("WS022")],
        ),
        // ...but still requires the FIXED params.
        (
            "variadic too few fixed args",
            "mod g(a: int, b: int, ...rest) -> int { return a + b }\n\
             var t: int = 0\nin go: exec\non go { t = g(1) }",
            &[Has("WS022")],
        ),
        // A `...rest` on a physical chip (not a mod) is rejected.
        (
            "variadic chip",
            "chip Bad(a: int, ...rest) { out r = a }",
            &[Has("WS052")],
        ),
    ]);
}

#[test]
fn exec_signal_codes() {
    diag_table(&[
        // A plain `emit s` then a same-chain `await s` parks forever.
        (
            "plain emit before await",
            "in go: exec\nlet s: exec\nvar n: int = 0\n\
             on go { emit s\n  await s\n  if n < 5 { buffer emit s } }",
            &[Warns("WS053")],
        ),
        // A buffered kick lands next tick, after the await arms.
        (
            "buffered kick before await",
            "in go: exec\nlet s: exec\nvar n: int = 0\n\
             on go { buffer emit s\n  await s\n  if n < 5 { buffer emit s } }",
            &[Lacks("WS053")],
        ),
        (
            "emit and await of different signals",
            "in go: exec\nlet s: exec\nlet t: exec\non go { emit s\n  await t }",
            &[Lacks("WS053")],
        ),
        // An ANNOTATED capture types the value (DataOut1) and is clean.
        (
            "annotated event capture",
            "in go: exec\nvar a: int = 0\n\
             on go { let foo: int = await CustomEvent(\"c\")\n  a = foo }",
            &[NoErrors, Lacks("WS055")],
        ),
        // An UNANNOTATED capture still works but warns (type can't be inferred).
        (
            "unannotated event capture",
            "in go: exec\nvar a: int = 0\n\
             on go { let foo = await CustomEvent(\"c\")\n  a = foo }",
            &[Warns("WS055")],
        ),
        // A bare `await CustomEvent(...)` (no capture) just waits.
        (
            "bare await of an event",
            "in go: exec\nvar a: int = 0\non go { await CustomEvent(\"c\")\n  a = 1 }",
            &[Lacks("WS055")],
        ),
        (
            "handler receiver form",
            "var total: int = 0\non CustomEvent(\"c\") -> (p: int, t: int) { total = p + t }",
            &[NoErrors],
        ),
    ]);
}

#[test]
fn enum_codes() {
    diag_table(&[
        // The single `FromInt` argument is an int.
        (
            "FromInt with a string",
            "enum Shape { Empty, Circle(float) }\nout e = Shape.FromInt(\"x\")\n",
            &[Has("WS003")],
        ),
        // A real variant named `FromInt` wins over the tag-only constructor.
        (
            "a variant named FromInt",
            "enum E { A, FromInt(int) }\nlet e = E.FromInt(3)\n",
            &[NoErrors],
        ),
        // `EnumToInt`'s argument MUST be an enum, not an accepted `any`.
        (
            "EnumToInt of an int",
            "enum Shape { Empty, Circle(float) }\nout a = EnumToInt(5)\n",
            &[Has("WS003")],
        ),
        (
            "EnumToInt of a string",
            "enum Shape { Empty, Circle(float) }\nout a = EnumToInt(\"x\")\n",
            &[Has("WS003")],
        ),
        // With no enum-typed expectation there is no way to know which enum the
        // integer names, mirroring `FromInt`'s type-inference failure.
        (
            "un-pinned IntToEnum",
            "enum Shape { Empty, Circle(float) }\nin n: int\nlet e = IntToEnum(n)\n",
            &[Has("WS063")],
        ),
        // A param named like a top-level enum shadows the type, so `Shape.x` is a
        // record field access, not a variant path.
        (
            "value shadowing an enum name",
            "enum Shape { Empty, Circle }\ntype Rec = { x: int }\n\
             mod f(Shape: Rec) -> int {\n    return Shape.x\n}\n",
            &[Lacks("WS060")],
        ),
        // Captures bind their CONCRETE field types, not `any`: int vs string arm
        // results have no common widening. A capture bound `any` would join cleanly.
        (
            "match arm capture type mismatch",
            "enum P { I(int), S(string) }\nvar p: P = P.I(0)\n\
             out r = match p { I(n) => n, S(s) => s }\n",
            &[Has("WS003")],
        ),
        (
            "prelude Option/Result round trip",
            "let a = Some(42)\nlet b: Option<int> = None\nlet c = Ok(1)\nlet d: Result<int, int> = Err(2)\n",
            &[NoDiags],
        ),
        // A user enum with a variant also named `Some` makes the bare name
        // AMBIGUOUS with the prelude's: bare resolution backs off (WS002) rather
        // than silently picking either enum.
        (
            "ambiguous bare variant",
            "enum Custom<T> { Some(T), Other }\nout a = Some(1)\n",
            &[Has("WS002")],
        ),
        // An unannotated `Err(2)` reads `T` from `E` exactly as `Ok(1)` reads `E`
        // from `T`, though `T` is solved first in declaration order.
        ("unannotated Err", "let d = Err(2)\n", &[NoDiags]),
        (
            "built-in game enum var and discriminant comparison",
            "static var e: EasingFunction = EasingFunction.Linear\n\
             out m = e.Discriminant == EasingFunction.Bounce.Discriminant\n",
            &[NoDiags],
        ),
        // A qualified built-in enum value as a gate config argument.
        (
            "qualified built-in enum config arg",
            "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = EasingFunction.Bounce) }",
            &[NoDiags],
        ),
        (
            "legacy bare enum member config arg",
            "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = Bounce) }",
            &[NoDiags],
        ),
        // A built-in enum value of the WRONG enum is an argument type mismatch.
        (
            "wrong built-in enum config arg",
            "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = ColorSpace.Srgb) }",
            &[Has("WS003")],
        ),
    ]);
}

#[test]
fn expression_types() {
    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "a variant path's discriminant",
            "enum Shape { Empty, Circle(float) }\nout d = Shape.Circle.Discriminant\n",
            "Shape.Circle.Discriminant",
            "int",
        ),
        // `value.ToInt()` is an exact alias for `.Discriminant`, on a stored value
        // and on a variant path.
        (
            "ToInt on a stored value",
            "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(1.0)\nout a = s.ToInt()\nout b = Shape.Circle.ToInt()\n",
            "s.ToInt()",
            "int",
        ),
        (
            "ToInt on a variant path",
            "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(1.0)\nout a = s.ToInt()\nout b = Shape.Circle.ToInt()\n",
            "Shape.Circle.ToInt()",
            "int",
        ),
        (
            "FromInt constructs the enum",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin n: int\nout e = Shape.FromInt(n)\n",
            "Shape.FromInt(n)",
            "Shape",
        ),
        (
            "a real FromInt variant",
            "enum E { A, FromInt(int) }\nlet e = E.FromInt(3)\n",
            "E.FromInt(3)",
            "E",
        ),
        // `EnumToInt(value)` is the gate-backed twin of `.ToInt()`.
        (
            "EnumToInt",
            "enum Shape { Empty, Circle(float) }\nstatic var s: Shape = Shape.Circle(1.0)\nout a = EnumToInt(s)\n",
            "EnumToInt(s)",
            "int",
        ),
        // `IntToEnum(value)`'s enum is pinned by the annotated target.
        (
            "IntToEnum in an annotated let",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin n: int\nlet e: Shape = IntToEnum(n)\n",
            "IntToEnum(n)",
            "Shape",
        ),
        (
            "a param shadowing an enum name",
            "enum Shape { Empty, Circle }\ntype Rec = { x: int }\nmod f(Shape: Rec) -> int {\n    return Shape.x\n}\n",
            "Shape.x",
            "int",
        ),
        (
            "positional construction",
            "enum Shape { Empty, Circle(float), Box { w: float, h: float } }\nout a = Shape.Circle(5.0)\nout b = Shape.Box { w: 1.0, h: 2.0 }\nout c = Shape.Empty\n",
            "Shape.Circle(5.0)",
            "Shape",
        ),
        (
            "named construction",
            "enum Shape { Empty, Circle(float), Box { w: float, h: float } }\nout a = Shape.Circle(5.0)\nout b = Shape.Box { w: 1.0, h: 2.0 }\nout c = Shape.Empty\n",
            "Shape.Box { w: 1.0, h: 2.0 }",
            "Shape",
        ),
        (
            "unit variant",
            "enum Shape { Empty, Circle(float), Box { w: float, h: float } }\nout a = Shape.Circle(5.0)\nout b = Shape.Box { w: 1.0, h: 2.0 }\nout c = Shape.Empty\n",
            "Shape.Empty",
            "Shape",
        ),
        (
            "match binds payloads and joins arm types",
            "enum Shape { Circle(float), Rect(float, float) }\nvar s: Shape = Shape.Circle(1.0)\nout area = match s { Circle(r) => r, Rect(w, h) => w }\n",
            "match s { Circle(r) => r, Rect(w, h) => w }",
            "float",
        ),
        (
            "generic variant inferred",
            "enum Option<T> { Some(T), None }\nout a = Option.Some(42)\n",
            "Option.Some(42)",
            "Option<int>",
        ),
        (
            "generic variant annotated",
            "enum Option<T> { Some(T), None }\nout n: Option<int> = Option.None\n",
            "Option.None",
            "Option<int>",
        ),
        // `Option<T>`/`Result<T, E>` are built in, and their variants are usable bare.
        (
            "prelude Some",
            "let a = Some(42)\nlet b: Option<int> = None\nlet c = Ok(1)\nlet d: Result<int, int> = Err(2)\n",
            "Some(42)",
            "Option<int>",
        ),
        (
            "prelude Ok",
            "let a = Some(42)\nlet b: Option<int> = None\nlet c = Ok(1)\nlet d: Result<int, int> = Err(2)\n",
            "Ok(1)",
            "Result<int, int>",
        ),
        (
            "prelude None",
            "let a = Some(42)\nlet b: Option<int> = None\nlet c = Ok(1)\nlet d: Result<int, int> = Err(2)\n",
            "None",
            "Option<int>",
        ),
        (
            "prelude Err",
            "let a = Some(42)\nlet b: Option<int> = None\nlet c = Ok(1)\nlet d: Result<int, int> = Err(2)\n",
            "Err(2)",
            "Result<int, int>",
        ),
        // `Result`'s unconstrained-sibling default is order-independent.
        (
            "unannotated Ok",
            "let c = Ok(1)\n",
            "Ok(1)",
            "Result<int, int>",
        ),
        (
            "unannotated Err",
            "let d = Err(2)\n",
            "Err(2)",
            "Result<int, int>",
        ),
        // `EasingFunction` is a registry enum with no `enum` declaration in source.
        (
            "built-in game enum variant path discriminant",
            "out d = EasingFunction.Bounce.Discriminant\n",
            "EasingFunction.Bounce.Discriminant",
            "int",
        ),
        (
            "built-in game enum discriminant comparison",
            "static var e: EasingFunction = EasingFunction.Linear\nout m = e.Discriminant == EasingFunction.Bounce.Discriminant\n",
            "e.Discriminant == EasingFunction.Bounce.Discriminant",
            "bool",
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, needle, want) in rows {
        match type_of(src, needle) {
            Ok(got) if got == *want => {}
            Ok(got) => failures.push(format!("{label}: `{needle}` typed {got}, want {want}")),
            Err(e) => failures.push(format!("{label}: {e}")),
        }
    }
    report_failures(&failures, rows.len());
}

/// The WS049 diagnostics a program type-checks to.
fn ws049(src: &str) -> Vec<crate::diagnostic::Diagnostic> {
    tc(src)
        .diagnostics
        .into_iter()
        .filter(|d| d.code == "WS049")
        .collect()
}

#[test]
fn record_element_with_a_container_field_is_ws049() {
    // A record stored in an array or map is one column per field, and a column
    // can't hold an array or map, so the declaration itself is rejected.
    let h = "type H = { xs: int[], n: int }\ntype M = { m: Map<string, int> }\n";
    for decl in [
        "var hs: H[]",
        "var hm: Map<string, H>",
        "var ms: M[]",
        "type G = { h: H }\nvar gs: G[]",
        "type K = { hs: H[] }\nvar k: K",
        "type K = { hm: Map<int, H> }\nvar k: K",
        "in go: exec\non go { var hs: H[] }",
    ] {
        let d = ws049(&format!("{h}{decl}\n"));
        assert_eq!(d.len(), 1, "`{decl}` must be exactly one WS049: {d:?}");
        assert!(
            d[0].message.contains("can't contain an array or map"),
            "`{decl}`: {}",
            d[0].message
        );
        assert!(
            !d[0].message.contains('\u{2014}'),
            "no em dash: {}",
            d[0].message
        );
    }
}

#[test]
fn record_with_a_record_container_field_cannot_cross_a_port() {
    // A record array/map field is several columns, and a port pin carries one
    // reference, so a record port or chip parameter can't hold one.
    let h = "type P = { x: int, y: int }\n\
             type H = { pts: P[], n: int }\n\
             type M = { m: Map<string, P> }\n\
             type G = { h: H }\n";
    for decl in [
        "in r: H",
        "in r: M",
        "in r: G",
        "chip C(hh: H) { in k: exec }",
        "chip C() { in r: H }",
        "chip C() -> (r: H) { in k: exec }",
        "out o: H = { pts: [], n: 1 }",
    ] {
        let d = ws049(&format!("{h}{decl}\n"));
        assert_eq!(d.len(), 1, "`{decl}` must be exactly one WS049: {d:?}");
        assert!(
            d[0].message.contains("can't cross a port"),
            "`{decl}`: {}",
            d[0].message
        );
        assert!(
            !d[0].message.contains('\u{2014}'),
            "no em dash: {}",
            d[0].message
        );
    }
    for decl in [
        "mod f(hh: H) { hh.n = 1 }",
        "in r: { xs: int[], n: int }",
        "chip C(xs: { xs: int[] }) { in k: exec }",
    ] {
        let d = ws049(&format!("{h}{decl}\n"));
        assert!(d.is_empty(), "`{decl}` is fine: {d:?}");
    }
}

#[test]
fn tuple_element_with_a_container_is_ws049() {
    for src in [
        "var ts: (int, int[])[]",
        "type H = { t: (int, int[]), n: int }\nvar hs: H[]",
        "var m: Map<int, (int, int[])>",
        "type T = (int, Map<int, int>)\ntype H = { t: T }\nvar hm: Map<string, H>",
    ] {
        let d = ws049(&format!("{src}\n"));
        assert_eq!(d.len(), 1, "`{src}` must be exactly one WS049: {d:?}");
        assert!(
            d[0].message.contains("can't contain an array or map"),
            "`{src}`: {}",
            d[0].message
        );
    }
    assert!(ws049("var t: (int, int[])\nvar ps: (int, int)[]\n").is_empty());
}

#[test]
fn ws049_ref_field_message_does_not_promise_container_fields() {
    let d = ws049("type T = { z: zone, n: int }\nvar ts: T[]\n");
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(!d[0].message.contains("arrays/maps"), "{}", d[0].message);
    assert!(!d[0].message.contains('\u{2014}'), "{}", d[0].message);
}

#[test]
fn ws049_names_tuple_elements_and_element_kinds() {
    let d = ws049("var ts: (int, int[])[]\n");
    assert!(
        d[0].message.contains("tuple element `1`"),
        "{}",
        d[0].message
    );
    let d = ws049("enum C { Red, Green }\ntype H = { cs: C[], n: int }\nin h: H\n");
    assert!(!d[0].message.contains("of records"), "{}", d[0].message);
    assert!(d[0].message.contains("enum"), "{}", d[0].message);
    let d = ws049("in ts: (int, int)[]\n");
    assert!(d[0].message.contains("of tuples"), "{}", d[0].message);
}

#[test]
fn chip_body_var_record_element_with_a_container_is_ws049() {
    let d = ws049("type H = { xs: int[] }\nchip C() { var hs: H[] }\n");
    assert_eq!(d.len(), 1, "{d:?}");
}

#[test]
fn record_array_or_map_cannot_cross_a_port() {
    let h = "type P = { x: int, y: int }\n\
             type H = { pts: P[], n: int }\n\
             var pts: P[]\n\
             var pm: Map<string, P>\n\
             var h: H\n";
    for decl in [
        "in ps: P[]",
        "in pm2: Map<string, P>",
        "out o3: P[] = pts",
        "out o2 = pts",
        "out o4 = pm",
        "out o = h",
        "chip C(ps2: P[]) { in k: exec }",
        "chip C() -> (r: P[]) { in k: exec }",
        "chip C() { var hc: H\n out r: H = hc }",
        "chip C() { var pc: P[]\n out r = pc }",
    ] {
        let d = ws049(&format!("{h}{decl}\n"));
        assert_eq!(d.len(), 1, "`{decl}` must be exactly one WS049: {d:?}");
        assert!(
            d[0].message.contains("can't cross a port"),
            "`{decl}`: {}",
            d[0].message
        );
    }
    for decl in [
        "mod f(ps2: P[]) { ps2.x.sort() }",
        "mod g() -> (r: P[]) { return pts }",
        "mod k() { var hc: H\n out r: H = hc }",
        "in xs: int[]",
        "out n = h.n",
    ] {
        let d = ws049(&format!("{h}{decl}\n"));
        assert!(d.is_empty(), "`{decl}` is fine: {d:?}");
    }
}

#[test]
fn payload_enum_array_cannot_cross_a_port_but_a_unit_enum_array_can() {
    let d = ws049("enum E { A, B(int) }\nin es: E[]\n");
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(ws049("enum C { Red, Green }\nin cs: C[]\n").is_empty());
    // A chip's pin has no column of tags to hand an enum array's elements.
    let d = ws049("enum C { Red, Green }\nchip K(g: exec, cs: C[]) { }\n");
    assert_eq!(d.len(), 1, "{d:?}");
    // A record variable stores any enum array field as columns, so a record
    // holding one can't cross a port even without payloads.
    let d = ws049(
        "enum C { Red, Green }\ntype H = { cs: C[], n: int }\nchip K(hh: H) { in k: exec }\n",
    );
    assert_eq!(d.len(), 1, "{d:?}");
}

#[test]
fn record_with_a_container_field_is_storable_outside_a_container() {
    let h = "type P = { x: int, y: int }\ntype H = { xs: int[], n: int }\n";
    for decl in [
        "var h: H",
        "var pts: P[]",
        "var pm: Map<string, P>",
        "type K = { pts: P[], m: Map<string, P>, xs: int[] }\nvar k: K",
        "type G = { h: H }\nvar g: G",
    ] {
        let d = ws049(&format!("{h}{decl}\n"));
        assert!(d.is_empty(), "`{decl}` is storable: {d:?}");
    }
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
    let r = tc(src);
    let needle = "Some";
    let start = src.rfind(needle).expect("needle not found");
    let end = start + needle.len();
    let file: std::sync::Arc<str> = std::sync::Arc::from("test");
    let ty = r.type_of_expr.get(&(file, start, end)).unwrap_or_else(|| {
        panic!(
            "no recorded type for the referencing `Some`; diagnostics: {:?}",
            r.diagnostics
        )
    });
    assert_eq!(
        ty.to_string(),
        "int",
        "the local `Some` must win over the prelude variant"
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
    let r = tc(&full_src);
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
    let r2 = tc(&short_src);
    assert!(
        r2.diagnostics
            .iter()
            .any(|d| d.code == "WS054" && d.message.contains(missing.as_str())),
        "dropping `{missing}` must surface WS054 naming it: {:?}",
        r2.diagnostics
    );
}
