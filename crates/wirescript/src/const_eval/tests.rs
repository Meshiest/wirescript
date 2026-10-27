use std::sync::Arc;

use super::error::ConstError;
use super::interp::eval_call;
use super::*;
use crate::ir::Literal;

fn empty_ctx() -> ConstCtx<'static> {
    ConstCtx::new(
        crate::const_scope::ConstNames::default(),
        Arc::new(crate::collections::HashMap::default()),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        None,
    )
}

fn eval_str(src_expr: &str) -> Result<Literal, ConstError> {
    let p = crate::parse(&format!("let probe = {src_expr}"), "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else {
        panic!("expected a let")
    };
    eval_expr(&l.value, &empty_ctx(), &mut Budget::default())
}

/// The whole-module `ConstEnv` of `src`, built the way a real compile builds
/// it (`const_eval::build_const_env`, including the enum registry).
fn const_env(src: &str) -> crate::const_eval::ConstEnv {
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    crate::const_eval::build_const_env(&p.ast.decls, &enum_defs)
}

/// Parses `src` (a whole script, typically an `enum` declaration plus a
/// top-level `const`/`let`), builds its whole-module `ConstEnv` the same way
/// a real compile does, and returns the bound value for `name`.
fn eval_ok(src: &str, name: &str) -> Literal {
    let env = const_env(src);
    env.get(name)
        .unwrap_or_else(|| {
            panic!("'{name}' did not evaluate to a compile-time constant; env = {env:?}")
        })
        .clone()
}

/// Call the first `mod`/`chip` in `src` with `args` through the interpreter
/// and a fresh [`Budget`]. The top-level `const`/`let`s of `src` are the
/// module constants, built as a real compile builds them.
fn eval_mod_call(src: &str, args: &[Literal]) -> Result<Literal, ConstError> {
    eval_mod_call_with(
        src,
        args,
        Budget::default(),
        crate::collections::HashMap::default(),
    )
}

/// Same as [`eval_mod_call`], but with the call-chain DEPTH allowance
/// overridden (the step budget stays at its default).
fn eval_mod_call_with_budget(
    src: &str,
    args: &[Literal],
    depth: u32,
) -> Result<Literal, ConstError> {
    let budget = Budget {
        depth,
        ..Budget::default()
    };
    eval_mod_call_with(src, args, budget, crate::collections::HashMap::default())
}

/// [`eval_mod_call`] with caller-local constants in `consts` but not in
/// `module_consts`, to show a callee cannot read the caller's locals.
fn eval_mod_call_with_caller_locals(
    src: &str,
    args: &[Literal],
    caller_locals: &[(&str, Literal)],
) -> Result<Literal, ConstError> {
    let mut locals = crate::collections::HashMap::default();
    for (name, lit) in caller_locals {
        locals.insert((*name).to_string(), lit.clone());
    }
    eval_mod_call_with(src, args, Budget::default(), locals)
}

fn eval_mod_call_with(
    src: &str,
    args: &[Literal],
    mut budget: Budget,
    caller_locals: crate::collections::HashMap<String, Literal>,
) -> Result<Literal, ConstError> {
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let decl = p
        .ast
        .decls
        .iter()
        .find_map(|d| match d {
            crate::ast::TopDecl::Chip(c) => Some(c),
            _ => None,
        })
        .expect("expected a mod/chip declaration");
    // As in `TypeCheckCtx::const_ctx`: `module_consts` is the top-level env,
    // `consts` adds the caller's open scope frames.
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let module_consts = crate::const_eval::build_const_env(&p.ast.decls, &enum_defs);
    let mut consts = module_consts.clone();
    for (name, lit) in caller_locals {
        consts.insert(name, lit);
    }
    let cx = ConstCtx::new(
        crate::const_scope::ConstNames::of(Arc::new(consts)),
        Arc::new(module_consts),
        Arc::default(),
        Arc::clone(&enum_defs),
        Arc::default(),
        Arc::new(super::DeclaredTypes::for_program(&p.ast.decls, &enum_defs)),
        None,
    );
    eval_call(decl, args, &[], &cx, &mut budget)
}

/// An expected const-evaluation failure: its code, a test on its reason, and
/// a test on its rendered message.
type ErrCheck = (&'static str, fn(&ConstReason) -> bool, fn(&str) -> bool);

fn any_reason(_: &ConstReason) -> bool {
    true
}

fn any_message(_: &str) -> bool {
    true
}

fn refused(r: &ConstReason) -> bool {
    matches!(r, ConstReason::Refused(_))
}

/// `None` when `got` is the error `want` describes, else what differs.
fn err_mismatch(
    got: Result<Literal, ConstError>,
    (code, reason, message): ErrCheck,
) -> Option<String> {
    match got {
        Ok(v) => Some(format!("folded to {v:?}, want {code}")),
        Err(e) if e.code() != code => {
            Some(format!("code {}, want {code}: {:?}", e.code(), e.reason))
        }
        Err(e) if !reason(&e.reason) => Some(format!("reason {:?}", e.reason)),
        Err(e) if !message(&e.message()) => Some(format!("message {:?}", e.message())),
        Err(_) => None,
    }
}

/// Every (label, result, want) row whose result differs from its value.
fn value_failures(
    rows: impl IntoIterator<Item = (String, Result<Literal, ConstError>, Literal)>,
) -> (Vec<String>, usize) {
    let mut failures = Vec::new();
    let mut n = 0;
    for (label, got, want) in rows {
        n += 1;
        match got {
            Ok(v) if v == want => {}
            other => failures.push(format!("{label}\n  got {other:?}\n  want {want:?}")),
        }
    }
    (failures, n)
}

fn int(v: i64) -> Literal {
    Literal::Int(v)
}

fn ints(vs: &[i64]) -> Literal {
    Literal::Array(vs.iter().map(|v| Literal::Int(*v)).collect())
}

fn str_lit(s: &str) -> Literal {
    Literal::String(s.into())
}

fn record(fields: &[(&str, Literal)]) -> Literal {
    Literal::Record(
        fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

/// Expressions that fold through `eval_expr` in an empty environment.
#[test]
fn eval_str_folds() {
    let rows = vec![
        ("a shift", "1 << 4", int(16)),
        ("string concatenation", "\"a\" .. \"b\"", str_lit("ab")),
        ("interpolation", "\"a${1 + 1}b\"", str_lit("a2b")),
        ("a string method", "\"hi\".ToUpper()", str_lit("HI")),
        (
            "chained string methods",
            "\"  x  \".Trim().Length()",
            int(1),
        ),
        (
            "a const if takes its true arm",
            "if true then 1 else 2",
            int(1),
        ),
        // The untaken arm need not even be const.
        (
            "a const if evaluates only the taken arm",
            "if false then someRuntimeThing else 9",
            int(9),
        ),
        ("an array literal", "[1, 2, 3]", ints(&[1, 2, 3])),
        ("an array index", "[10, 20][1]", int(20)),
        ("an array length", "[10, 20, 30].length()", int(3)),
        (
            "a map literal",
            "{ 1 => \"a\", 2 => \"b\" }",
            Literal::Map(vec![(int(1), str_lit("a")), (int(2), str_lit("b"))]),
        ),
        ("a map index", "{ 1 => \"a\", 2 => \"b\" }[2]", str_lit("b")),
        (
            "a map length",
            "{ 1 => \"a\", 2 => \"b\" }.length()",
            int(2),
        ),
        ("a record field", "{ rooms: 2, timer: 60 }.timer", int(60)),
        // `.a` yields the inner record, `.b` reads its field.
        ("a nested record field", "{ a: { b: 1 } }.a.b", int(1)),
        // Named constructor arguments bind by parameter name, as the runtime
        // path does, not by position.
        (
            "constructor named arguments bind by name",
            "Vec(z = 3.0, x = 1.0, y = 2.0)",
            Literal::Vector {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
        ),
        (
            "positional args fill from the left, a named one targets its parameter",
            "Rotation(1.0, roll = 3.0, yaw = 2.0)",
            Literal::Rotator {
                pitch: 1.0,
                yaw: 2.0,
                roll: 3.0,
            },
        ),
        // `Color`'s 4th parameter is optional: omitting it takes the opaque form.
        (
            "an omitted optional constructor argument defaults",
            "Color(b = 0.25, r = 0.5, g = 0.75)",
            Literal::LinearColor {
                r: 0.5,
                g: 0.75,
                b: 0.25,
                a: 1.0,
            },
        ),
        (
            "an optional constructor argument given by name",
            "Color(0.5, 0.75, 0.25, a = 0.5)",
            Literal::LinearColor {
                r: 0.5,
                g: 0.75,
                b: 0.25,
                a: 0.5,
            },
        ),
    ];
    let (failures, n) = value_failures(
        rows.into_iter()
            .map(|(label, src, want)| (format!("{label}: {src}"), eval_str(src), want)),
    );
    crate::test_support::report_failures(&failures, n);
}

/// Expressions `eval_expr` refuses, and how each refusal is reported.
#[test]
fn eval_str_refusals() {
    let rows: &[(&str, &str, ErrCheck)] = &[
        (
            "a runtime name is named in the message",
            "someRuntimeThing + 1",
            ("WS046", any_reason, |m| m.contains("someRuntimeThing")),
        ),
        // Both operands are constant; blaming one would call a literal a runtime value.
        (
            "an operator refusing constant operands names the operator, not the operands",
            "1 << 100",
            ("WS047", refused, |m| {
                !m.contains("not a compile-time constant") && m.contains("<<")
            }),
        ),
        // `Vec` folds itself, so the failure is about the argument.
        (
            "a constructor call blames its runtime argument, not the constructor",
            "Vec(someRuntimeThing, 1.0, 2.0)",
            (
                "WS046",
                |r| *r == ConstReason::NotConstant("someRuntimeThing".into()),
                |m| !m.contains("Vec"),
            ),
        ),
        // Non-ASCII string operands are never certified for folding.
        (
            "a refused value is WS047, not WS046",
            "\"café\".ToUpper()",
            ("WS047", any_reason, any_message),
        ),
        // At runtime an out-of-range read keeps the PREVIOUS value; at compile
        // time there is none.
        (
            "an out-of-range const index is an error, not a stale read",
            "[1, 2][9]",
            (
                "WS046",
                |r| *r == ConstReason::ArrayIndexOutOfRange { index: 9, len: 2 },
                |m| !m.contains("not a compile-time constant"),
            ),
        ),
        (
            "a missing const map key is an error, not a stale read",
            "{ 1 => \"a\" }[2]",
            ("WS046", |r| *r == ConstReason::MapKeyNotFound, any_message),
        ),
        (
            "a missing record field names the field",
            "{ rooms: 2 }.timer",
            (
                "WS046",
                |r| *r == ConstReason::RecordFieldNotFound("timer".to_string()),
                |m| !m.contains("not a compile-time constant"),
            ),
        ),
        // A named argument matching no parameter, or a positional one past the
        // last, has no constant form.
        (
            "a named constructor argument that binds no parameter",
            "Vec(x = 1.0, y = 2.0, bogus = 3.0)",
            ("WS047", refused, any_message),
        ),
        (
            "a positional constructor argument past the last parameter",
            "Vec(1.0, 2.0, 3.0, 4.0)",
            ("WS047", refused, any_message),
        ),
        // A HOLE (an unbound parameter with a bound one after it) must refuse:
        // `bind_constructor_args` stops at the first unbound parameter, and
        // skipping it instead slides later arguments onto the wrong axis.
        (
            "a hole: optional `a` bound, required `b` not",
            "Color(0.5, 0.75, a = 0.25)",
            ("WS047", refused, any_message),
        ),
        (
            "a hole: `g` unbound",
            "Color(r = 1.0, b = 1.0)",
            ("WS047", refused, any_message),
        ),
        (
            "a hole: `b` unbound before a named `a`",
            "Color(r = 1.0, g = 2.0, a = 0.5)",
            ("WS047", refused, any_message),
        ),
        (
            "a hole: `y` unbound",
            "Vec(x = 1.0, z = 3.0)",
            ("WS047", refused, any_message),
        ),
        (
            "too few constructor arguments",
            "Vec(1.0, 2.0)",
            ("WS047", refused, any_message),
        ),
        // The runtime path is last-write-wins here; folding would pick a winner silently.
        (
            "a constructor parameter bound twice",
            "Vec(1.0, x = 2.0, y = 3.0)",
            ("WS047", refused, any_message),
        ),
        (
            "a spread argument to a constructor",
            "Vec(...someRuntimeThing, 2.0, 3.0)",
            (
                "WS046",
                |r| matches!(r, ConstReason::Unsupported(w) if w.contains("spread")),
                any_message,
            ),
        ),
        (
            "a spread in an array literal",
            "[1, ...someRuntimeThing]",
            (
                "WS046",
                |r| matches!(r, ConstReason::Unsupported(w) if w.contains("spread")),
                any_message,
            ),
        ),
    ];
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|&(label, src, want)| {
            err_mismatch(eval_str(src), want).map(|e| format!("{label}: {src}\n  {e}"))
        })
        .collect();
    crate::test_support::report_failures(&failures, rows.len());
}

/// `const mod` bodies called through the interpreter with the given arguments.
#[test]
fn const_mod_call_values() {
    let rows: Vec<(&str, &str, Vec<Literal>, Literal)> = vec![
        (
            "a const mod body",
            "const mod f(n: int) -> int { return n * 3 }",
            vec![int(4)],
            int(12),
        ),
        (
            "a const binding inside a const mod",
            "const mod f(n: int) -> int { const doubled = n * 2\n return doubled + 1 }",
            vec![int(5)],
            int(11),
        ),
        // The parameter frame sits on top of the module scope rather than
        // replacing it, as an ordinary `mod` body's does.
        (
            "a const mod body sees module-level constants",
            "const N = 4\nconst mod scaled(x: int) -> int { return x * N }",
            vec![int(3)],
            int(12),
        ),
        (
            "a const mod builds a channel name from a module constant",
            "const PFX = \"evt_\"\nconst mod chan(s: string) -> string { return PFX .. s }",
            vec![str_lit("hit")],
            str_lit("evt_hit"),
        ),
        (
            "a parameter shadows a module constant of the same name",
            "const n = 100\nconst mod f(n: int) -> int { return n }",
            vec![int(4)],
            int(4),
        ),
    ];
    // A mutation inside a branch is visible on the `return` after it.
    let rooms = "const mod rooms(n: int) -> int[] {\n\
                   const t = []\n\
                   if n >= 1 { t.push(10) }\n\
                   if n >= 2 { t.push(20) }\n\
                   return t\n\
                 }";
    let mut rows = rows;
    rows.extend([
        ("an array assembled conditionally, n = 1", rooms, vec![int(1)], ints(&[10])),
        ("an array assembled conditionally, n = 2", rooms, vec![int(2)], ints(&[10, 20])),
        ("an array assembled conditionally, n = 0", rooms, vec![int(0)], ints(&[])),
        (
            "array set",
            "const mod f() -> int[] { const t = [1, 2, 3]\n t.set(1, 99)\n return t }",
            vec![],
            ints(&[1, 99, 3]),
        ),
        ("array clear", "const mod f() -> int[] { const t = [1, 2, 3]\n t.clear()\n return t }", vec![], ints(&[])),
        (
            "array append",
            "const mod f() -> int[] {\n const t = [1, 2]\n const more = [3, 4]\n t.append(more)\n return t\n }",
            vec![],
            ints(&[1, 2, 3, 4]),
        ),
        ("map set of a new key", "const mod f() -> int { const m = { 1 => 10 }\n m.set(2, 20)\n return m[2] }", vec![], int(20)),
        ("map set overwrites", "const mod f() -> int { const m = { 1 => 10 }\n m.set(1, 20)\n return m[1] }", vec![], int(20)),
        (
            "map remove",
            "const mod f() -> int {\n const m = { 1 => 10, 2 => 20 }\n m.remove(1)\n return m.length()\n }",
            vec![],
            int(1),
        ),
        ("map clear", "const mod f() -> int { const m = { 1 => 10 }\n m.clear()\n return m.length() }", vec![], int(0)),
        // A fresh `const t` inside the branch shadows the outer `t`; both
        // writes land under the key "t", so merge-back must know the branch re-bound it.
        (
            "a branch-local redeclaration does not leak its mutations",
            "const mod f() -> int[] {\n const t = [1]\n if true {\n const t = [99]\n t.push(100)\n }\n return t\n }",
            vec![],
            ints(&[1]),
        ),
        (
            "a branch mutation of an outer binding survives the branch",
            "const mod f() -> int[] {\n const t = [1]\n if true { t.push(2) }\n return t\n }",
            vec![],
            ints(&[1, 2]),
        ),
        // Mutating the PARAMETER must not reach the module constant it shadows,
        // nor leak into a second call.
        (
            "mutating a shadowing parameter, first call",
            "const t = [1, 2, 3]\nconst mod f(t: int[]) -> int[] { t.push(9)\n return t }",
            vec![ints(&[0])],
            ints(&[0, 9]),
        ),
        (
            "mutating a shadowing parameter, second call",
            "const t = [1, 2, 3]\nconst mod f(t: int[]) -> int[] { t.push(9)\n return t }",
            vec![ints(&[5])],
            ints(&[5, 9]),
        ),
        // A shadow confines only itself: the push before it mutated the outer
        // `t`, which keeps that push.
        (
            "a shadow declared after a mutation keeps the mutation",
            "const mod f() -> int[] {\n const t = [1]\n if true {\n t.push(2)\n const t = [9]\n t.push(10)\n }\n return t\n }",
            vec![],
            ints(&[1, 2]),
        ),
        // Snapshots replay in REVERSE declaration order so the earliest wins.
        (
            "repeated shadows in one branch restore the earliest snapshot",
            "const mod f() -> int[] {\n const t = [1]\n if true {\n t.push(2)\n const t = [9]\n const t = [99]\n }\n return t\n }",
            vec![],
            ints(&[1, 2]),
        ),
        (
            "a shadow after a mutation two levels deep keeps both mutations",
            "const mod f() -> int[] {\n const t = [1]\n if true {\n t.push(2)\n if true {\n t.push(3)\n const t = [9]\n t.push(10)\n }\n }\n return t\n }",
            vec![],
            ints(&[1, 2, 3]),
        ),
        // `interp::exec_block`'s `Stmt::Let` arm is separate from typecheck's
        // top-level site; a swapped binding computes 403.
        (
            "a const destructure inside a const mod body binds",
            "const mod f() -> int {\n const p = { x: 3, y: 4 }\n const { x, y } = p\n return x * 100 + y\n }",
            vec![],
            int(304),
        ),
        (
            "a const destructure with an alias binds the alias",
            "const mod f() -> int {\n const p = { x: 3, y: 4 }\n const { x: a, y: b } = p\n return a * 100 + b\n }",
            vec![],
            int(304),
        ),
        (
            "a const destructure with a rest collects the remaining fields",
            "const mod f() -> int {\n const p = { x: 3, y: 4, z: 5 }\n const { x, ...rest } = p\n return x * 10000 + rest.y * 100 + rest.z\n }",
            vec![],
            int(30405),
        ),
        // Each destructured name needs its own shadow snapshot.
        (
            "a branch-local destructure restores every shadowed name",
            "const mod f() -> int {\n const a = 1\n const b = 2\n if true {\n const { a, b } = { a: 9, b: 99 }\n }\n return a * 1000 + b\n }",
            vec![],
            int(1002),
        ),
        // `-> (a, b)` with `out` statements assembles a record like `return { a, b }`.
        (
            "a multi-output const mod yields a record",
            "const mod pair(n: int) -> (a: int, b: int) { out a = n\n out b = n + 1 }",
            vec![int(2)],
            record(&[("a", int(2)), ("b", int(3))]),
        ),
        // `eval_call` walks `decl.outputs`, not the `out` statements.
        (
            "a multi-output record is in declaration order, not assignment order",
            "const mod pair(n: int) -> (a: int, b: int) { out b = n + 1\n out a = n }",
            vec![int(2)],
            record(&[("a", int(2)), ("b", int(3))]),
        ),
        // One named output is not a record: typecheck types it as the bare
        // value.
        ("a single named output yields a bare value", "const mod C(v: int) -> (r: int) { out r = v }", vec![int(7)], int(7)),
        // A RECORD `return` is the documented multi-output mechanism.
        (
            "a record return in a multi-output const mod is forwarded",
            "const mod cfg(n: int) -> (a: int, b: int) { return { a: n, b: n + 1 } }",
            vec![int(1)],
            record(&[("a", int(1)), ("b", int(2))]),
        ),
        // The `outputs` accumulator is threaded through the `if` arm.
        (
            "an out inside a taken THEN branch is collected",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n if n > 0 { out b = 77 } else { out b = 88 }\n }",
            vec![int(1)],
            record(&[("a", int(1)), ("b", int(77))]),
        ),
        (
            "an out inside a taken ELSE branch is collected",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n if n > 0 { out b = 77 } else { out b = 88 }\n }",
            vec![int(0)],
            record(&[("a", int(0)), ("b", int(88))]),
        ),
        // A bare `return` inside an `if` stops the whole body, so `out b = 999` never runs.
        (
            "a bare return stops the body and builds the record",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n out b = 1\n if n > 0 { return }\n out b = 999\n }",
            vec![int(1)],
            record(&[("a", int(1)), ("b", int(1))]),
        ),
        (
            "an untaken bare return lets the later out win",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n out b = 1\n if n > 0 { return }\n out b = 999\n }",
            vec![int(0)],
            record(&[("a", int(0)), ("b", int(999))]),
        ),
        // A body-declared output is supported and is not one of the result's fields.
        (
            "an out naming an output outside the signature is ignored",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n out zz = 999\n out b = n + 1 }",
            vec![int(1)],
            record(&[("a", int(1)), ("b", int(2))]),
        ),
        // With the `return` first, lowering's first-wins and const evaluation agree.
        (
            "a valued return before a later out is allowed",
            "const mod pick(n: int) -> (r: int) { if n > 0 { return 222 }\n out r = 111 }",
            vec![int(5)],
            int(222),
        ),
        (
            "an untaken return before a later out lets the out supply the value",
            "const mod pick(n: int) -> (r: int) { if n > 0 { return 222 }\n out r = 111 }",
            vec![int(0)],
            int(111),
        ),
        (
            "a returned record is reordered to declaration order",
            "const mod cfg(n: int) -> (a: int, b: int) { return { b: n + 1, a: n } }",
            vec![int(1)],
            record(&[("a", int(1)), ("b", int(2))]),
        ),
        // Mutating methods take their arguments by the names the method table spells.
        (
            "array mutations take named arguments",
            "const mod f() -> int[] { const t = [1, 2]\n t.push(value = 3)\n t.set(index = 0, value = 9)\n return t }",
            vec![],
            ints(&[9, 2, 3]),
        ),
        (
            "map mutations take named arguments",
            "const mod g() -> int { const m = { 1 => 10 }\n m.set(key = 2, value = 20)\n m.remove(key = 1)\n return m[2] * 100 + m.length() }",
            vec![],
            int(2001),
        ),
    ]);
    let (failures, n) = value_failures(rows.into_iter().map(|(label, src, args, want)| {
        (format!("{label}\n  {src}"), eval_mod_call(src, &args), want)
    }));
    crate::test_support::report_failures(&failures, n);
}

/// `const mod` bodies the interpreter refuses, and how each refusal is reported.
#[test]
fn const_mod_call_errors() {
    fn unsupported_with(r: &ConstReason, what: &str) -> bool {
        matches!(r, ConstReason::Unsupported(w) if w.contains(what))
    }
    fn message_with(r: &ConstReason, what: &str) -> bool {
        matches!(r, ConstReason::UnsupportedMessage(w) if w.contains(what))
    }
    // (label, src, argument, expected error, source text the range starts at)
    let rows: &[(&str, &str, Option<i64>, ErrCheck, Option<&str>)] = &[
        // The message says "this statement"; the range is what names it.
        (
            "a non-const statement is blamed on its own range",
            "const mod f(n: int) -> int { BroadcastChatMessage(\"x\")\n return n }",
            Some(1),
            ("WS046", any_reason, |m| m.contains("this statement")),
            Some("BroadcastChatMessage"),
        ),
        // The fault is at the call, not "'b' is a runtime value" in the body.
        (
            "a missing argument blames the parameter, not the body",
            "const mod f(a: int, b: int) -> int { return a + b }",
            Some(1),
            (
                "WS046",
                |r| unsupported_with(r, "missing an argument"),
                any_message,
            ),
            None,
        ),
        (
            "an out-of-range const set is an error, not a stale write",
            "const mod f() -> int[] { const t = [1, 2]\n t.set(9, 0)\n return t }",
            None,
            (
                "WS046",
                |r| *r == ConstReason::ArrayIndexOutOfRange { index: 9, len: 2 },
                any_message,
            ),
            None,
        ),
        // `sort` is a catalogued method, so this is "not a compile-time mutation",
        // not "unrecognized method".
        (
            "an unsupported array method names itself",
            "const mod f() -> int[] { const t = [2, 1]\n t.sort()\n return t }",
            None,
            (
                "WS046",
                |r| matches!(r, ConstReason::UnsupportedMethod(m) if m.contains("sort")),
                any_message,
            ),
            None,
        ),
        (
            "an unsupported map method names itself",
            "const mod f() -> int { const m = { 1 => 10 }\n m.has(1)\n return m.length() }",
            None,
            (
                "WS046",
                |r| matches!(r, ConstReason::UnsupportedMethod(m) if m.contains("has")),
                any_message,
            ),
            None,
        ),
        // Restoring a snapshot of `None` means removing the key.
        (
            "a branch declaration of a previously unbound name vanishes entirely",
            "const mod f() -> int[] {\n if true {\n const fresh = [9]\n fresh.push(10)\n }\n return fresh\n }",
            None,
            (
                "WS046",
                |r| *r == ConstReason::NotConstant("fresh".into()),
                any_message,
            ),
            None,
        ),
        (
            "a const destructure naming a missing field",
            "const mod f() -> int {\n const p = { x: 1 }\n const { x, missing } = p\n return x\n }",
            None,
            (
                "WS046",
                |r| *r == ConstReason::RecordFieldNotFound("missing".into()),
                any_message,
            ),
            None,
        ),
        (
            "a const destructure of a non-record",
            "const mod f() -> int {\n const { x } = 1\n return x\n }",
            None,
            (
                "WS046",
                |r| unsupported_with(r, "not a record"),
                any_message,
            ),
            None,
        ),
        // Every name a branch-local destructure introduces vanishes, probed per name.
        (
            "a branch-local destructure of unbound names: `a` vanishes",
            "const mod f() -> int {\n if true {\n const { a, b } = { a: 9, b: 99 }\n }\n return a\n }",
            None,
            (
                "WS046",
                |r| *r == ConstReason::NotConstant("a".into()),
                any_message,
            ),
            None,
        ),
        (
            "a branch-local destructure of unbound names: `b` vanishes",
            "const mod f() -> int {\n if true {\n const { a, b } = { a: 9, b: 99 }\n }\n return b\n }",
            None,
            (
                "WS046",
                |r| *r == ConstReason::NotConstant("b".into()),
                any_message,
            ),
            None,
        ),
        // Blamed on the unassigned output's own declaration in the signature.
        (
            "an unassigned output is an error",
            "const mod pair(n: int) -> (a: int, b: int) { out a = n }",
            Some(2),
            ("WS046", |r| message_with(r, "never assigns"), any_message),
            Some("b: int"),
        ),
        // A scalar `return` with 2+ outputs is dropped by lowering; folding it
        // would make `c.a` lower to a `SplitColor` gate. An untaken guard
        // `return` is never evaluated, so only the argument taking it errors.
        (
            "a scalar return after the outs of a multi-output const mod",
            "const mod cfg(n: int) -> (a: int, b: int) { out a = n\n out b = n + 1\n return 42 }",
            Some(1),
            (
                "WS046",
                |r| matches!(r, ConstReason::UnsupportedMessage(_)),
                any_message,
            ),
            None,
        ),
        (
            "a guard scalar return before the outs of a multi-output const mod",
            "const mod cfg(n: int) -> (a: int, b: int) { if n < 0 { return 0 }\n out a = n\n out b = n + 1 }",
            Some(-1),
            (
                "WS046",
                |r| matches!(r, ConstReason::UnsupportedMessage(_)),
                any_message,
            ),
            None,
        ),
        // Lowering keeps the FIRST assignment in source order while const
        // evaluation stops at the `return`, so a valued `return` past an
        // earlier `out` is refused at any arity.
        (
            "out then return",
            "const mod pick(n: int) -> (r: int) { out r = 111\n return 222 }",
            Some(5),
            ("WS046", |r| message_with(r, "earlier `out`"), any_message),
            None,
        ),
        (
            "out then guarded return",
            "const mod pick(n: int) -> (r: int) { out r = 111\n if n > 0 { return 222 } }",
            Some(5),
            ("WS046", |r| message_with(r, "earlier `out`"), any_message),
            None,
        ),
        // The untaken `out` still lowers and wins the port, so the check is positional.
        (
            "out in an untaken branch, then return",
            "const mod pick(n: int) -> (r: int) { if n < 0 { out r = 111 }\n return 222 }",
            Some(5),
            ("WS046", |r| message_with(r, "earlier `out`"), any_message),
            None,
        ),
        // Not "the `_` output is never assigned".
        (
            "a single-output body that produces nothing falls off the end",
            "const mod f(n: int) -> int { }",
            Some(1),
            (
                "WS046",
                |r| unsupported_with(r, "falls off the end"),
                any_message,
            ),
            None,
        ),
        (
            "a multi-output body that produces nothing falls off the end",
            "const mod f(n: int) -> (a: int, b: int) { }",
            Some(1),
            (
                "WS046",
                |r| unsupported_with(r, "falls off the end"),
                any_message,
            ),
            None,
        ),
        // A body that hit a `return` must not claim it fell off the end.
        (
            "a bare return that produces nothing blames the return",
            "const mod f(n: int) -> int { return }",
            Some(1),
            (
                "WS046",
                |r| unsupported_with(r, "`return` with no value"),
                any_message,
            ),
            Some("return"),
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, arg, want, blame) in rows {
        let args: Vec<Literal> = arg.into_iter().map(int).collect();
        let got = eval_mod_call(src, &args);
        let range_start = got.as_ref().err().map(|e| e.range.start.offset);
        if let Some(e) = err_mismatch(got, want) {
            failures.push(format!("{label}\n  {src}\n  {e}"));
        } else if let Some(text) = blame {
            let want_start = src.find(text).unwrap();
            if range_start != Some(want_start) {
                failures.push(format!(
                    "{label}: range starts at {range_start:?}, want {want_start} (`{text}`)"
                ));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn a_runaway_call_chain_reports_ws048() {
    // Depth is bounded even though WS020 forbids true recursion: a long chain
    // of distinct const mods must fail with a diagnostic, never a stack
    // overflow.
    let err = eval_mod_call_with_budget(
        "const mod f(n: int) -> int { return n }",
        &[Literal::Int(1)],
        0,
    );
    assert_eq!(err.unwrap_err().code(), "WS048");
}

/// A constant local to the caller's scope is not a module constant, so the
/// callee's body must not resolve it, just as an ordinary `mod` body cannot.
#[test]
fn a_const_mod_body_cannot_see_the_callers_local_constants() {
    let src = "const mod peek(x: int) -> int { return x + callerLocal }";
    let err = eval_mod_call_with_caller_locals(
        src,
        &[Literal::Int(1)],
        &[("callerLocal", Literal::Int(9))],
    )
    .unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(err.reason, ConstReason::NotConstant("callerLocal".into()));
}

/// Every `FOLDABLE_CONSTRUCTORS` name still folds, has a `catalog::calls` entry
/// to bind named arguments through, and has only trailing optional parameters,
/// which `bind_constructor_args` stopping at the first unbound one relies on.
#[test]
fn foldable_constructors_are_all_really_foldable() {
    for name in super::expr::FOLDABLE_CONSTRUCTORS {
        assert!(
            eval_str(&format!("{name}(1.0, 2.0, 3.0)")).is_ok(),
            "'{name}' is listed as a foldable constructor but no longer folds"
        );

        let spec = crate::catalog::calls::find_call(name).unwrap_or_else(|| {
            panic!("'{name}' is a foldable constructor but has no catalog entry to bind named arguments through")
        });
        let first_optional = spec.params.iter().position(|p| p.optional);
        if let Some(i) = first_optional {
            assert!(
                spec.params[i..].iter().all(|p| p.optional),
                "'{name}' has a REQUIRED parameter after an optional one \
                 ({:?}) — `bind_constructor_args` stops collecting at the first \
                 unbound parameter, which is only sound while every optional \
                 parameter trails",
                spec.params
                    .iter()
                    .map(|p| (p.name, p.optional))
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// The reported range covers the sub-expression the message names, so the
/// editor underlines `someRuntimeThing`, not the whole `Vec(...)` call.
#[test]
fn the_reported_range_blames_the_offending_sub_expression_not_the_whole_call() {
    let src_expr = "Vec(someRuntimeThing, 1.0, 2.0)";
    let full_src = format!("let probe = {src_expr}");
    let err = eval_str(src_expr).unwrap_err();
    let want_start = full_src.find("someRuntimeThing").unwrap();
    let want_end = want_start + "someRuntimeThing".len();
    assert_eq!(
        err.range.start.offset, want_start,
        "range should start at the offending argument, not the whole call: {:?}",
        err.range
    );
    assert_eq!(
        err.range.end.offset, want_end,
        "range should end at the offending argument, not the whole call: {:?}",
        err.range
    );
}

/// The two evaluators `expr.rs` uses agree where both are defined. Each case
/// runs through `const_eval` and the certified fold evaluator; a divergence is
/// a bug in one of them.
#[test]
fn both_evaluators_agree_where_both_are_defined() {
    use crate::lower::fold::eval::Value;

    // Parse a two-operand `left op right` expression and hand its operands to
    // the certified evaluator directly, bypassing const_eval entirely.
    fn eval_via_fold(src_expr: &str, gate_class: &str) -> Option<Literal> {
        let p = crate::parse(&format!("let probe = {src_expr}"), "test");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else {
            panic!("expected a let")
        };
        let crate::ast::Expr::BinOp { left, right, .. } = &l.value else {
            panic!("expected a binary expression")
        };
        let env = crate::const_eval::ConstEnv::default();
        let a = Value::from_literal(&crate::const_eval::expr_to_literal_in(left, &env).unwrap())?;
        let b = Value::from_literal(&crate::const_eval::expr_to_literal_in(right, &env).unwrap())?;
        crate::lower::fold::eval::eval(gate_class, &[Some(a), Some(b)]).map(|v| v.to_literal())
    }

    for (src, gate_class) in [
        ("2 + 3", "BrickComponentType_Expr_MathAdd"),
        ("7 - 2", "BrickComponentType_Expr_MathSubtract"),
        ("\"a\" .. \"b\"", crate::ir::gate_class::STRING_CONCATENATE),
        // Mixed operands, which both evaluators must answer alike.
        ("1 + true", "BrickComponentType_Expr_MathAdd"),
        ("1 == true", "BrickComponentType_Expr_CompareEqual"),
        ("10 > \"9\"", "BrickComponentType_Expr_CompareGreater"),
    ] {
        let ours = eval_str(src).unwrap();
        let theirs = eval_via_fold(src, gate_class).expect("fold must evaluate this too");
        assert_eq!(ours, theirs, "evaluators disagree on {src}");
    }
}

/// Module constants that fold through `build_const_env`: enum construction,
/// discriminants, `match`, `is`, and `const mod` calls with defaults.
#[test]
fn module_const_folds() {
    let disc = |n: i64| record(&[("__disc", int(n))]);
    const SCALE: &str = "const mod scale(v: int, by: int = 2) -> int { return v * by }\nconst K = scale(5)\nconst L = scale(5, 3)\nconst M = scale(v = 4)\n";
    let rows = vec![
        // `Expr::MatchExpr`'s own const-eval arm; needs the enum registry.
        (
            "a const match folds to the taken arm's value",
            "enum Shape { Empty, Circle(float) }\nconst AREA = match Shape.Circle(3.0) { Circle(r) => r, Empty => 0.0 }\n",
            "AREA",
            Literal::Float(3.0),
        ),
        // The fold reads the actual disc rather than always taking one arm.
        (
            "a const match takes the matching unit variant arm",
            "enum Shape { Empty, Circle(float) }\nconst AREA = match Shape.Empty { Circle(r) => r, Empty => 0.0 }\n",
            "AREA",
            Literal::Float(0.0),
        ),
        // The match reads the scrutinee's VALUE, not its syntactic shape.
        (
            "a const match on a named constant scrutinee folds",
            "enum Shape { Empty, Circle(float) }\nconst s = Shape.Circle(5.0)\nconst AREA = match s { Circle(r) => r, Empty => 0.0 }\n",
            "AREA",
            Literal::Float(5.0),
        ),
        // Every arm parses as a binding; the scrutinee `Dir.E` names the enum.
        (
            "an all-unit match folds via the scrutinee's enum reference",
            "enum Dir { N, E, S, W }\nconst D = match Dir.E { N => 10, E => 20, S => 30, W => 40 }\n",
            "D",
            int(20),
        ),
        // Not `Some`/`None`: a const match finds its enum by the variants its
        // arms name. A failing guard falls through to the next arm.
        (
            "a const enum match falls through a failing guard, Full(3)",
            "enum Jar { Full(int), Empty }\nconst R = match Jar.Full(3) { Full(x) if x > 5 => 1, Full(0) => 2, Full(_) => 3, Empty => 4 }\n",
            "R",
            int(3),
        ),
        (
            "a const enum match takes a passing guard, Full(9)",
            "enum Jar { Full(int), Empty }\nconst R = match Jar.Full(9) { Full(x) if x > 5 => 1, Full(0) => 2, Full(_) => 3, Empty => 4 }\n",
            "R",
            int(1),
        ),
        (
            "a const enum match takes a literal payload arm, Full(0)",
            "enum Jar { Full(int), Empty }\nconst R = match Jar.Full(0) { Full(x) if x > 5 => 1, Full(0) => 2, Full(_) => 3, Empty => 4 }\n",
            "R",
            int(2),
        ),
        // A bare variant path's `.Discriminant` resolves from the enum registry.
        (
            "a variant path's discriminant",
            "enum Shape { Empty, Circle(float) }\nconst D = Shape.Circle.Discriminant\n",
            "D",
            int(1),
        ),
        (
            "a unit variant's discriminant reads back off the value",
            "enum Dir { N, E, S, W }\nconst d = Dir.E\nconst D = d.Discriminant\n",
            "D",
            int(1),
        ),
        // Slot keys match `lower::predeclare::build_enum_fields`'s format.
        (
            "positional variant construction folds its slots",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst r = Shape.Rect(1.0, 2.0)\n",
            "r",
            record(&[
                ("__disc", int(2)),
                ("__Rect_0", Literal::Float(1.0)),
                ("__Rect_1", Literal::Float(2.0)),
            ]),
        ),
        (
            "a positionally constructed value's discriminant reads back",
            "enum Shape { Empty, Circle(float) }\nconst D = Shape.Circle(5.0).Discriminant\n",
            "D",
            int(1),
        ),
        // `.ToInt()` is an exact alias for `.Discriminant`.
        (
            "a variant path's ToInt",
            "enum Shape { Empty, Circle(float) }\nconst A = Shape.Circle.ToInt()\n",
            "A",
            int(1),
        ),
        (
            "a stored value's ToInt",
            "enum Dir { N, E, S, W }\nconst d = Dir.S\nconst B = d.ToInt()\n",
            "B",
            int(2),
        ),
        // Payloads default at runtime, so `FromInt` folds tag-only.
        (
            "FromInt folds to a tag-only record",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst e = Shape.FromInt(2)\n",
            "e",
            disc(2),
        ),
        (
            "FromInt of a unit variant",
            "enum Shape { Empty, Circle }\nconst e = Shape.FromInt(1)\n",
            "e",
            disc(1),
        ),
        (
            "a const match on FromInt takes the arm by tag",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst picked = match Shape.FromInt(2) { Rect(w, h) => 20, Circle(r) => 10, _ => 0 }\n",
            "picked",
            int(20),
        ),
        (
            "EnumToInt of a variant literal",
            "enum Shape { Empty, Circle(float) }\nconst A = EnumToInt(Shape.Circle(1.0))\n",
            "A",
            int(1),
        ),
        (
            "EnumToInt of a const value",
            "enum Dir { N, E, S, W }\nconst d = Dir.S\nconst B = EnumToInt(d)\n",
            "B",
            int(2),
        ),
        (
            "IntToEnum folds to a tag-only record",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst e = IntToEnum(2)\n",
            "e",
            disc(2),
        ),
        (
            "a const match on IntToEnum takes the arm by tag",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nconst picked = match IntToEnum(2) { Rect(w, h) => 20, Circle(r) => 10, _ => 0 }\n",
            "picked",
            int(20),
        ),
        // The tag-only constructor never shadows a genuine variant.
        (
            "a variant named FromInt constructs normally",
            "enum E { A, FromInt(int) }\nconst e = E.FromInt(3)\n",
            "e",
            record(&[("__disc", int(1)), ("__FromInt_0", int(3))]),
        ),
        (
            "named variant construction folds its slots",
            "enum Shape { Empty, Box { w: float, h: float } }\nconst b = Shape.Box { w: 1.0, h: 2.0 }\n",
            "b",
            record(&[
                ("__disc", int(1)),
                ("__Box_w", Literal::Float(1.0)),
                ("__Box_h", Literal::Float(2.0)),
            ]),
        ),
        (
            "`is` folds true",
            "enum Dir { N, E, S, W }\nconst d = Dir.E\nconst hit = d is Dir.E\nconst miss = d is Dir.N\n",
            "hit",
            Literal::Bool(true),
        ),
        (
            "`is` folds false",
            "enum Dir { N, E, S, W }\nconst d = Dir.E\nconst hit = d is Dir.E\nconst miss = d is Dir.N\n",
            "miss",
            Literal::Bool(false),
        ),
        // An omitted parameter with a default passes the default, as a lowered call does.
        (
            "a parameter default fills an omitted argument",
            SCALE,
            "K",
            int(10),
        ),
        (
            "a given argument overrides the default",
            SCALE,
            "L",
            int(15),
        ),
        (
            "a named argument with the default filling the rest",
            SCALE,
            "M",
            int(8),
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, name, want) in &rows {
        match const_env(src).get(*name) {
            Some(v) if v == want => {}
            other => failures.push(format!(
                "{label}\n  {src}\n  `{name}` is {other:?}, want {want:?}"
            )),
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A `match` on a single value folds through literal, range, string and
/// guarded arms, first match winning, and binds what a string pattern's holes
/// hold.
#[test]
fn a_const_value_match_takes_the_first_arm_that_matches() {
    let cases: &[(&str, Literal)] = &[
        (
            r#"const R = match "-" { "+" => 1, "-" | "<" => 2, _ => 3 }"#,
            Literal::Int(2),
        ),
        (
            r#"const R = match "?" { "+" => 1, "-" => 2, _ => 3 }"#,
            Literal::Int(3),
        ),
        (
            r#"const R = match 7 { 0 => "zero", 1..10 => "small", _ => "big" }"#,
            Literal::String("small".into()),
        ),
        (
            r#"const R = match 10 { 1..10 => "small", n if n > 5 => "big", _ => "other" }"#,
            Literal::String("big".into()),
        ),
        (
            r#"const R = match 3 { n if n > 5 => "big", _ => "other" }"#,
            Literal::String("other".into()),
        ),
        (
            r#"const R = match "say hi" { "say " .. rest => rest, _ => "" }"#,
            Literal::String("hi".into()),
        ),
        (
            r#"const R = match "main.ws" { "say " .. rest => rest, name .. ".ws" => name, _ => "" }"#,
            Literal::String("main".into()),
        ),
        (
            r#"const R = match "a=b=c" { key .. "=" .. value => key .. "|" .. value, _ => "" }"#,
            Literal::String("a|b=c".into()),
        ),
        (
            r#"const R = match "<x>" { "<" .. inner .. ">" => inner, _ => "none" }"#,
            Literal::String("x".into()),
        ),
        (
            r#"const R = match ">" { "<" .. inner .. ">" => inner, _ => "none" }"#,
            Literal::String("none".into()),
        ),
        (
            r#"const R = match "go" { "go " .. dest if dest != "" => dest, "go" .. _ => "bare", _ => "no" }"#,
            Literal::String("bare".into()),
        ),
        (
            r#"const R = match 2.5 { 1 => "one", 2.5 => "two and a half", _ => "?" }"#,
            Literal::String("two and a half".into()),
        ),
        (
            r"const R = match true { true => 1, false => 0 }",
            Literal::Int(1),
        ),
        (r"const R = match 5 { :red => 1, _ => 0 }", Literal::Int(0)),
        (
            r#"const R = match "give sword to bob" { "give ${item} to ${who}" => who .. ":" .. item, _ => "" }"#,
            Literal::String("bob:sword".into()),
        ),
        (
            r#"const R = match "tp home" { "tp ${_}" => 1, _ => 0 }"#,
            Literal::Int(1),
        ),
        (
            r#"const R = match "main.ws" { "${name}.ws" => name, _ => "" }"#,
            Literal::String("main".into()),
        ),
        (
            r#"const R = match "anything" { "${all}" => all }"#,
            Literal::String("anything".into()),
        ),
    ];
    for (src, want) in cases {
        assert_eq!(&eval_ok(&format!("{src}\n"), "R"), want, "{src}");
    }
}

/// Module constants `build_const_env` declines: the name stays out of the
/// environment and falls back to runtime lowering.
#[test]
fn module_consts_that_do_not_fold() {
    let rows: &[(&str, &str, &str)] = &[
        // A constant scrutinee that does not name the enum, with bare unit
        // arms, identifies no enum without type inference: refusing only
        // costs the fold, never a wrong value.
        (
            "a match that identifies no governing enum",
            "enum Dir { N, E }\nconst s = Dir.N\nconst D = match s { N => 1, E => 2 }\n",
            "D",
        ),
        // A non-const argument declines the WHOLE construction, never a partial record.
        (
            "a variant construction with a non-const argument",
            "enum Shape { Empty, Circle(float) }\nvar live: float = 0.0\nconst D = Shape.Circle(live).Discriminant\n",
            "D",
        ),
        // A user `mod Some` shadows the prelude variant as typecheck resolves
        // it; it is not a `const mod`, so the call does not fold at all
        // (folding to the prelude `__disc` record would disagree with typecheck).
        (
            "a user mod named after a prelude variant shadows it",
            "mod Some(x: int) -> (r: int) { return x + 1 }\nlet y = Some(5)\n",
            "y",
        ),
    ];
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|(label, src, name)| {
            const_env(src)
                .get(*name)
                .map(|v| format!("{label}\n  {src}\n  `{name}` folded to {v:?}"))
        })
        .collect();
    crate::test_support::report_failures(&failures, rows.len());
}

/// [`eval_mod_call`] calling `callee_name`, with `lookup_mod` resolving every
/// top-level `chip`/`mod` in `src` by name, so nested const mod calls resolve.
fn eval_mod_call_resolving(
    src: &str,
    callee_name: &str,
    args: &[Literal],
) -> Result<Literal, ConstError> {
    let p = crate::parse(src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let chips: Vec<Arc<crate::ast::ChipDecl>> = p
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            crate::ast::TopDecl::Chip(c) => Some(Arc::new(c.clone())),
            _ => None,
        })
        .collect();
    let decl = chips
        .iter()
        .find(|c| c.name == callee_name)
        .unwrap_or_else(|| panic!("no decl named '{callee_name}'"));
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let module_consts = crate::const_eval::build_const_env(&p.ast.decls, &enum_defs);
    let lookup = |name: &str| chips.iter().find(|c| c.name == name).cloned();
    let module_consts = Arc::new(module_consts);
    let cx = ConstCtx::new(
        crate::const_scope::ConstNames::of(Arc::clone(&module_consts)),
        module_consts,
        Arc::default(),
        Arc::clone(&enum_defs),
        Arc::default(),
        Arc::new(super::DeclaredTypes::for_program(&p.ast.decls, &enum_defs)),
        Some(&lookup),
    );
    eval_call(decl, args, &[], &cx, &mut Budget::default())
}

/// Evaluate `probe_expr`, written as `let probe = <probe_expr>` after
/// `mods_src`, with `lookup_mod` resolving every `chip`/`mod` in `mods_src`.
/// [`eval_str`] has no `lookup_mod`, so it reports every call `NotAConstMod`.
fn eval_str_resolving(mods_src: &str, probe_expr: &str) -> Result<Literal, ConstError> {
    let src = format!("{mods_src}\nlet probe = {probe_expr}");
    let p = crate::parse(&src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let chips: Vec<Arc<crate::ast::ChipDecl>> = p
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            crate::ast::TopDecl::Chip(c) => Some(Arc::new(c.clone())),
            _ => None,
        })
        .collect();
    let lookup = |name: &str| chips.iter().find(|c| c.name == name).cloned();
    let enum_defs = Arc::new(crate::typecheck::enums::build_registry(&p.ast.decls));
    let module_consts = crate::const_eval::build_const_env(&p.ast.decls, &enum_defs);
    let crate::ast::TopDecl::Let(probe) = p.ast.decls.last().expect("expected at least one decl")
    else {
        panic!("expected the last decl to be `let probe = ...`")
    };
    let module_consts = Arc::new(module_consts);
    let cx = ConstCtx::new(
        crate::const_scope::ConstNames::of(Arc::clone(&module_consts)),
        module_consts,
        Arc::default(),
        Arc::clone(&enum_defs),
        Arc::default(),
        Arc::new(super::DeclaredTypes::for_program(&p.ast.decls, &enum_defs)),
        Some(&lookup),
    );
    eval_expr(&probe.value, &cx, &mut Budget::default())
}

/// One const mod's call reaching ANOTHER through the real call machinery
/// (`interp::eval_call`), from a mod body and from a bare expression.
#[test]
fn nested_const_mod_calls() {
    let double = "const mod double(n: int) -> int { return n * 2 }";
    let from_bodies = vec![
        (
            "a const-mod call per element inside an array literal",
            "const mod double(n: int) -> int { return n * 2 }\nconst mod f() -> int[] { return [double(1), double(2), double(3)] }",
            "f",
            vec![],
            ints(&[2, 4, 6]),
        ),
        // `f().b` recurses into the call BEFORE reading the field.
        (
            "a record built in a const mod is field-readable from the result",
            "const mod f() -> { a: int, b: int } { return { a: 1, b: 2 } }\nconst mod g() -> int { return f().b }",
            "g",
            vec![],
            int(2),
        ),
        // `bind_destructured` does not care which statement shape built the record.
        (
            "an out-form multi-output result destructures",
            "const mod pair(n: int) -> (a: int, b: int) { out a = n\n out b = n + 1 }\nconst mod g(n: int) -> int {\n const { a, b } = pair(n)\n return a * 100 + b\n }",
            "g",
            vec![int(2)],
            int(203),
        ),
    ];
    // `expr_to_literal_in` has no notion of a `const mod` call, so each
    // enclosing arm of `eval_expr` must resolve the call itself rather than
    // report `NestedConstModCall`.
    let from_exprs = vec![
        (
            "a call nested in a binary operator",
            double,
            "double(3) + 1",
            int(7),
        ),
        (
            "a call nested in a unary operator",
            double,
            "-double(3)",
            int(-6),
        ),
        // Fails if only the left or only the right operand resolves.
        (
            "calls on both sides of an operator",
            double,
            "double(2) * double(3)",
            int(24),
        ),
        // The constructor match only sees literal-folded arguments.
        (
            "a call nested in a constructor argument",
            "const mod f(n: float) -> float { return n + 1.0 }",
            "Vec(f(1.0), 2.0, 3.0)",
            Literal::Vector {
                x: 2.0,
                y: 2.0,
                z: 3.0,
            },
        ),
    ];
    let rows = from_bodies
        .into_iter()
        .map(|(label, src, callee, args, want)| {
            (
                format!("{label}\n  {src}"),
                eval_mod_call_resolving(src, callee, &args),
                want,
            )
        })
        .chain(from_exprs.into_iter().map(|(label, mods, probe, want)| {
            (
                format!("{label}: {probe}"),
                eval_str_resolving(mods, probe),
                want,
            )
        }));
    let (failures, n) = value_failures(rows);
    crate::test_support::report_failures(&failures, n);
}

// Compile-time record destructuring

/// Parse `let <pattern_src> = <value_src>`, evaluate the value in an empty
/// environment, and split it with `bind_destructured`.
fn bind_str(pattern_src: &str, value_src: &str) -> Result<Vec<(String, Literal)>, ConstError> {
    let src = format!("let {pattern_src} = {value_src}");
    let p = crate::parse(&src, "test");
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else {
        panic!("expected a let")
    };
    let value =
        eval_expr(&l.value, &empty_ctx(), &mut Budget::default()).expect("value must evaluate");
    bind_destructured(&l.binding, value)
}

/// `bind_destructured` splits a value into exactly these (name, value) pairs,
/// in order. Distinct field values make a swapped binding fail.
#[test]
fn bind_destructured_binds() {
    let pairs = |ps: &[(&str, Literal)]| {
        ps.iter()
            .map(|(n, v)| (n.to_string(), v.clone()))
            .collect::<Vec<_>>()
    };
    let rows = vec![
        (
            "each field binds by its own name",
            "{ x, y }",
            "{ x: 11, y: 22 }",
            pairs(&[("x", int(11)), ("y", int(22))]),
        ),
        (
            "an alias binds the alias, not the field name",
            "{ x: a, y: b }",
            "{ x: 11, y: 22 }",
            pairs(&[("a", int(11)), ("b", int(22))]),
        ),
        // In the SOURCE record's field order, excluding only the `Named` fields.
        (
            "a rest collects the unconsumed fields in source order",
            "{ y, ...rest }",
            "{ x: 1, y: 2, z: 3 }",
            pairs(&[
                ("y", int(2)),
                ("rest", record(&[("x", int(1)), ("z", int(3))])),
            ]),
        ),
        // Positional, ignoring field names, so a multi-output `const mod`'s
        // name-keyed record splits like a tuple literal's index-keyed one.
        (
            "a tuple pattern binds positionally",
            "(p, q)",
            "{ a: 11, b: 22 }",
            pairs(&[("p", int(11)), ("q", int(22))]),
        ),
        // Re-keyed from zero, as `lower::decl::install_tuple_destruct` rebuilds the tail.
        (
            "a tuple pattern re-keys its rest",
            "(p, ...tail)",
            "{ a: 11, b: 22, c: 33 }",
            pairs(&[
                ("p", int(11)),
                ("tail", record(&[("0", int(22)), ("1", int(33))])),
            ]),
        ),
    ];
    let mut failures = Vec::new();
    for (label, pattern, value, want) in &rows {
        match bind_str(pattern, value) {
            Ok(got) if &got == want => {}
            other => failures.push(format!(
                "{label}: let {pattern} = {value}\n  got {other:?}\n  want {want:?}"
            )),
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Destructures `bind_destructured` refuses.
#[test]
fn bind_destructured_refusals() {
    let not_a_record: fn(&ConstReason) -> bool =
        |r| matches!(r, ConstReason::Unsupported(w) if w.contains("not a record"));
    let rows: &[(&str, &str, &str, fn(&ConstReason) -> bool)] = &[
        // There is no record to have a missing field in.
        (
            "a record pattern on a non-record value",
            "{ x }",
            "1",
            not_a_record,
        ),
        (
            "a tuple pattern on a non-record value",
            "(a, b)",
            "1",
            not_a_record,
        ),
        // The value is constant and positional; the pattern is the wrong width.
        (
            "a tuple pattern of the wrong width names both arities",
            "(p, q, r)",
            "{ a: 11, b: 22 }",
            |r| {
                matches!(
                    r,
                    ConstReason::TupleArityMismatch {
                        expected: 3,
                        got: 2
                    }
                )
            },
        ),
    ];
    let mut failures = Vec::new();
    for &(label, pattern, value, reason) in rows {
        match bind_str(pattern, value) {
            Err(e) if e.code() == "WS046" && reason(&e.reason) => {}
            other => failures.push(format!("{label}: let {pattern} = {value}\n  got {other:?}")),
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A name with no matching field is `RecordFieldNotFound`, blamed on that
/// name's own range, not the whole pattern.
#[test]
fn bind_destructured_a_missing_field_blames_the_names_own_range() {
    // `missing` comes first: a trailing field's range also covers the
    // whitespace before `}`, which this test should not depend on.
    let src = "let { missing, x } = { x: 1 }";
    let err = bind_str("{ missing, x }", "{ x: 1 }").unwrap_err();
    assert_eq!(err.code(), "WS046");
    assert_eq!(
        err.reason,
        ConstReason::RecordFieldNotFound("missing".into())
    );
    let want_start = src.find("missing").unwrap();
    let want_end = want_start + "missing".len();
    assert_eq!(
        err.range.start.offset, want_start,
        "must blame 'missing' itself, not the whole pattern: {:?}",
        err.range
    );
    assert_eq!(err.range.end.offset, want_end);
}

/// `bound_names` (read from the pattern alone) lists exactly the names
/// `bind_destructured` binds, in order. A drift silently treats a constant as
/// a runtime value or marks a name const with no value behind it.
#[test]
fn bound_names_agrees_with_bind_destructured() {
    for (pattern, value_src) in [
        ("x", "{ a: 1, b: 2 }"),                       // Ident: the whole value
        ("{ a, b }", "{ a: 11, b: 22 }"),              // plain named fields
        ("{ a: p, b: q }", "{ a: 11, b: 22 }"),        // aliases
        ("{ a, ...rest }", "{ a: 11, b: 22, c: 33 }"), // rest
    ] {
        let src = format!("let {pattern} = {value_src}");
        let p = crate::parse(&src, "test");
        assert!(p.diagnostics.is_empty(), "{pattern}: {:?}", p.diagnostics);
        let crate::ast::TopDecl::Let(l) = &p.ast.decls[0] else {
            panic!("expected a let")
        };
        let value = eval_expr(&l.value, &empty_ctx(), &mut Budget::default()).unwrap();
        let bound: Vec<String> = bind_destructured(&l.binding, value)
            .unwrap_or_else(|e| panic!("{pattern}: {e:?}"))
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(
            super::destructure::bound_names(&l.binding),
            bound,
            "`bound_names` and `bind_destructured` disagree for `{pattern}`"
        );
    }
}

/// A float made an `int` truncates toward zero, says whether that kept the
/// value written, and is `None` past the 64-bit range or when not finite;
/// `coerce_const_to_type` holds the result as an integer.
#[test]
fn a_float_made_an_int_truncates_or_has_no_int() {
    use super::{DeclaredTypes, coerce_const_to_type, float_as_int, written_float};
    use crate::ir::Type;
    assert_eq!(float_as_int(2.5), Some((2, false)));
    assert_eq!(float_as_int(-2.5), Some((-2, false)));
    assert_eq!(float_as_int(-0.5), Some((0, false)));
    assert_eq!(float_as_int(13.0), Some((13, true)));
    assert_eq!(
        float_as_int(-9_223_372_036_854_775_808.0),
        Some((i64::MIN, true))
    );
    assert_eq!(
        float_as_int(9_223_372_036_854_774_784.0),
        Some((9_223_372_036_854_774_784, true))
    );
    for none in [
        9_223_372_036_854_775_808.0,
        -9_223_372_036_854_777_856.0,
        1e30,
        -1e30,
        f64::INFINITY,
        f64::NAN,
    ] {
        assert_eq!(float_as_int(none), None, "{none}");
    }
    let types = DeclaredTypes::default();
    assert_eq!(
        coerce_const_to_type(Literal::Float(2.5), &Type::Int, &types),
        Literal::Int(2)
    );
    assert_eq!(
        coerce_const_to_type(Literal::Float(1e30), &Type::Int, &types),
        Literal::Float(1e30)
    );
    let arr = Literal::Array(vec![Literal::Float(1.5), Literal::Float(-3.9)]);
    assert_eq!(
        coerce_const_to_type(arr, &Type::Array(Box::new(Type::Int)), &types),
        Literal::Array(vec![Literal::Int(1), Literal::Int(-3)])
    );
    assert_eq!(written_float(1e10), "1e10");
    assert_eq!(written_float(1e30), "1e30");
    assert_eq!(written_float(2_147_483_648.0), "2147483648.0");
    assert_eq!(written_float(2.5), "2.5");
}
