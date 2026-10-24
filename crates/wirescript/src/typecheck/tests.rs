use super::*;
use crate::parser::parse;

fn tc(src: &str) -> TypeCheckResult {
    let p = parse(src, "test");
    assert!(
        p.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        p.diagnostics
    );
    typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
}

fn assert_no_diags(r: &TypeCheckResult) {
    let errors: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
}

use crate::test_support::report_failures;
use Want::{
    Codes, Count, Dropped, ErrMsg, Error, FirstMsg, Has, HasExact, HasMsg, Lacks, Msg, NoErrors,
    Silent, Warning,
};

/// What a table row expects of a program's typecheck result.
#[derive(Clone, Copy, Debug)]
enum Want {
    /// No error-severity diagnostic.
    NoErrors,
    /// No diagnostic of any severity.
    Silent,
    /// Some diagnostic has this code.
    Has(&'static str),
    /// Some diagnostic has this code and a message containing the text.
    HasMsg(&'static str, &'static str),
    /// Some diagnostic has this code and exactly this message.
    HasExact(&'static str, &'static str),
    /// The first diagnostic with this code has a message containing the text.
    FirstMsg(&'static str, &'static str),
    /// Some error-severity diagnostic has this code.
    Error(&'static str),
    /// Some warning-severity diagnostic has this code.
    Warning(&'static str),
    /// Some error-severity diagnostic has a message containing the text.
    ErrMsg(&'static str),
    /// Some diagnostic has a message containing the text.
    Msg(&'static str),
    /// No diagnostic has this code.
    Lacks(&'static str),
    /// Exactly this many diagnostics have this code.
    Count(&'static str, usize),
    /// The diagnostic codes are exactly these, in order.
    Codes(&'static [&'static str]),
    /// Exactly this many blocks were dropped by a compile-time `if`.
    Dropped(usize),
}

impl Want {
    fn holds(self, r: &TypeCheckResult) -> bool {
        let ds = &r.diagnostics;
        let is_err = |d: &&crate::diagnostic::Diagnostic| d.severity == Severity::Error;
        match self {
            NoErrors => !ds.iter().any(|d| is_err(&d)),
            Silent => ds.is_empty(),
            Has(c) => ds.iter().any(|d| d.code == c),
            HasMsg(c, m) => ds.iter().any(|d| d.code == c && d.message.contains(m)),
            HasExact(c, m) => ds.iter().any(|d| d.code == c && d.message == m),
            FirstMsg(c, m) => ds
                .iter()
                .find(|d| d.code == c)
                .is_some_and(|d| d.message.contains(m)),
            Error(c) => ds.iter().filter(is_err).any(|d| d.code == c),
            Warning(c) => ds
                .iter()
                .any(|d| d.code == c && d.severity == Severity::Warning),
            ErrMsg(m) => ds.iter().filter(is_err).any(|d| d.message.contains(m)),
            Msg(m) => ds.iter().any(|d| d.message.contains(m)),
            Lacks(c) => !ds.iter().any(|d| d.code == c),
            Count(c, n) => ds.iter().filter(|d| d.code == c).count() == n,
            Codes(cs) => crate::test_support::codes(ds) == cs,
            Dropped(n) => r.dropped_ranges.len() == n,
        }
    }
}

/// Check each `(label, source, wants)` row against `check(source)`, which
/// returns the typecheck result or why there is none; report every miss.
#[track_caller]
fn check_rows_with<S: AsRef<str>>(
    rows: &[(&str, S, &[Want])],
    check: impl Fn(&str) -> Result<TypeCheckResult, String>,
) {
    let mut failures = Vec::new();
    for (label, src, wants) in rows {
        let src = src.as_ref();
        match check(src) {
            Err(why) => failures.push(format!("{label}: {why}\n{src}")),
            Ok(r) => {
                let missed: Vec<Want> = wants.iter().copied().filter(|w| !w.holds(&r)).collect();
                if !missed.is_empty() {
                    failures.push(format!(
                        "{label}: missed {missed:?}\n{src}\n{:#?}",
                        r.diagnostics
                    ));
                }
            }
        }
    }
    report_failures(&failures, rows.len());
}

/// `check_rows_with` over a single parse-clean source file.
#[track_caller]
fn check_rows<S: AsRef<str>>(rows: &[(&str, S, &[Want])]) {
    check_rows_with(rows, |src| {
        let p = parse(src, "test");
        if !p.diagnostics.is_empty() {
            return Err(format!("parse diagnostics: {:?}", p.diagnostics));
        }
        Ok(typecheck(&p.ast, "test", &CeSlotMap::default()))
    });
}

#[test]
fn statement_form_gate_builtin_reports_helpful_message() {
    // `SetArrayElement`/`SetVariable`/`IncrementVariable` desugar to
    // assignments only in statement position with the right arg count.
    // Anything else — used as a value, or still being typed with too few
    // args — used to report a bare "unknown identifier" even though
    // completion offers the name. Both now get a describe-the-builtin hint.
    let hint = |d: &crate::diagnostic::Diagnostic| {
        d.code == "WS002"
            && d.message.contains("SetArrayElement")
            && d.message.contains("exec statement")
            && d.message.contains("a[i] = x")
    };
    // Used as a value.
    let r = tc("var a: int[]\nin go: exec\non go { let r = SetArrayElement(a, 0, 9) }");
    assert!(
        r.diagnostics.iter().any(hint),
        "value use: {:?}",
        r.diagnostics
    );
    // Incomplete / wrong arg count (the mid-typing case) — NOT "unknown identifier".
    let r2 = tc("var a: int[]\nin go: exec\non go { SetArrayElement(a, 0) }");
    assert!(
        r2.diagnostics.iter().any(hint)
            && !r2
                .diagnostics
                .iter()
                .any(|d| d.message.contains("unknown identifier")),
        "wrong arg count: {:?}",
        r2.diagnostics
    );
    // Correct statement desugars to `a[0] = 9` and is clean.
    assert_no_diags(&tc(
        "var a: int[]\nin go: exec\non go { SetArrayElement(a, 0, 9) }",
    ));
}

/// A call with no value to give, read as one, is a WS072 error; the same call
/// in statement position stays legal. Typed `any`, a no-output call would
/// hand its consumer the exec continuation as data. Void mutations
/// (`push`/`set`/`clear`) return `Never` for the same reason.
#[test]
fn a_call_with_no_value_read_as_one_is_ws072() {
    const NO_VALUE: &[Want] = &[HasMsg("WS072", "no value")];
    let m = "mod noret(x: int) { }\nvar o: int = 0\nin go: exec";
    let c = "var a: int[]\nvar m: Map<string,int>\nin go: exec\n";
    let rows: &[(&str, String, &[Want])] = &[
            ("void builtin as a value", "static var v: int = 0\non RoundStart() { v = PrintToConsole(\"x\") }".into(), NO_VALUE),
            ("void receiver method as a value", "var a: int[]\nstatic var v: int = 0\nin go: exec\non go { v = a.push(1) }".into(), NO_VALUE),
            ("void builtin as a statement", "on RoundStart() { PrintToConsole(\"ok\") }".into(), &[NoErrors]),
            ("void receiver method as a statement", "var a: int[]\nin go: exec\non go { a.push(1) }".into(), &[NoErrors]),
            ("no-output mod: assignment", format!("{m}\non go {{ o = noret(1) }}"), NO_VALUE),
            // A bare `let` binds nothing but is the documented chip-instantiation
            // spelling; reading the name is what reports.
            ("no-output mod: bare let", format!("{m}\non go {{ let v = noret(1) }}"), &[NoErrors]),
            ("no-output mod: let then read", format!("{m}\non go {{ let v = noret(1)\n o = v }}"), NO_VALUE),
            ("no-output mod: annotated var", format!("{m}\non go {{ var v: int = noret(1) }}"), NO_VALUE),
            ("no-output mod: argument", format!("{m}\non go {{ PrintToConsole(noret(1)) }}"), NO_VALUE),
            // An operand reports what it is, not a missing '+' overload on `never`.
            ("no-output mod: operand", format!("{m}\non go {{ o = noret(1) + 1 }}"), NO_VALUE),
            ("no-output mod: unary operand", format!("{m}\non go {{ o = -noret(1) }}"), NO_VALUE),
            ("no-output mod: interpolation", format!("{m}\non go {{ PrintToConsole(\"${{noret(1)}}\") }}"), NO_VALUE),
            ("no-output chip", "chip nochip(x: int) { }\nvar q: int = 0\nin g2: exec\non g2 { q = nochip(1) }".into(), NO_VALUE),
            ("no-output mod as a statement", format!("{m}\non go {{ noret(1) }}"), &[NoErrors]),
            ("a mod with an output still binds", "mod ret(x: int) -> (r: int) { out r = x }\nvar o: int = 0\nin go: exec\non go { o = ret(1) }".into(), &[NoErrors]),
            ("let = push", format!("{c}on go {{ let r = a.push(5) }}"), NO_VALUE),
            ("let = map set", format!("{c}on go {{ let r = m.set(\"k\", 1) }}"), NO_VALUE),
            ("var = clear", format!("{c}on go {{ var x = a.clear() }}"), NO_VALUE),
            ("out = push", format!("{c}out y = a.push(5)"), NO_VALUE),
            ("push as a statement", format!("{c}on go {{ a.push(5) }}"), &[NoErrors]),
            ("pop still binds", format!("{c}on go {{ let r = a.pop() }}"), &[NoErrors]),
            ("map get still binds", format!("{c}on go {{ let r = m.get(\"k\") }}"), &[NoErrors]),
        ];
    check_rows(rows);
}

/// Programs that must typecheck with no error.
#[test]
fn programs_that_typecheck_clean() {
    const OK: &[Want] = &[NoErrors];
    let rows: &[(&str, &str, &[Want])] = &[
        ("an empty script", "", OK),
        // `Array<V>` is an alternate spelling of `V[]`, and `Ref<V>` of `*V`.
        ("`Array<V>` spelling", "var a: Array<int> = [1, 2]", OK),
        ("`V[]` spelling", "var a: int[] = [1, 2]", OK),
        (
            "`Ref<V>` spelling",
            "mod inc(v: Ref<int>) { v = v + 1 }",
            OK,
        ),
        ("`*V` spelling", "mod inc(v: *int) { v = v + 1 }", OK),
        (
            "`Map<K, V>` resolves in an annotation",
            "mod f(m: Map<string, int>) { }",
            OK,
        ),
        (
            "an int literal into a float var coerces",
            "var x: float = 1",
            OK,
        ),
        // Strings can be stored in vars (`WireValue` supports `Str`).
        ("a string-annotated var", "var x: string = \"hi\"", OK),
        ("a string-inferred var", "var x = \"hello\"", OK),
        // An `any` operand has no `==` overload and would emit WS004.
        (
            "an inferred string var is a string, not any",
            "var s = \"\"\nout r = s == \"ready\"",
            OK,
        ),
        (
            "an inferred int var works in math",
            "var n = 0\nout d = n + 1",
            OK,
        ),
        (
            "an inferred float var works in math",
            "var f = 1.5\nout d = f * 2.0",
            OK,
        ),
        (
            "an inferred bool var works in logic",
            "var b = true\nout d = b && false",
            OK,
        ),
        ("a negative literal infers", "var n = -5\nout d = n + 1", OK),
        // No literal init: the type refines from the RHS in pass 2, so vector
        // math resolves.
        (
            "a non-literal init refines the var's type",
            "var v = Vec(1.0, 2.0, 3.0)\nout d = v + Vec(0.0, 0.0, 1.0)",
            OK,
        ),
        (
            "a handler-local var infers",
            "on RoundStart() { var v = Vec(1.0, 2.0, 3.0)\n let w = v + v }",
            OK,
        ),
        (
            "a string annotation accepts the entity family",
            "in c: controller\nlet msg: string = c",
            OK,
        ),
        (
            "concat casts a character to string",
            "in p: character\nout s = \"hi \" .. p",
            OK,
        ),
        // A constant Vec(...) folds to a literal, so it is a legal top-level
        // array initializer element.
        (
            "constant vectors in an array initializer",
            "var pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]",
            OK,
        ),
        // `literal_expr_type` knows constructor calls, so this infers vector[], not any[].
        (
            "an array of vectors infers its element type",
            "var pts = [Vec(1.0, 1.0, 1.0)]",
            OK,
        ),
        // Color() returns `color`, so the var refines and a later color assignment checks.
        (
            "a color var infers and is reassignable",
            "var tint = Color(1.0, 0.0, 0.0)\non RoundStart() { tint = Color(0.0, 1.0, 0.0) }",
            OK,
        ),
        (
            "a string var in a handler",
            "on RoundStart() { var x: string = \"hi\" }",
            OK,
        ),
        ("a string let", "let x = \"hello\"", OK),
        // Desugared by the parser to `let _on_expr_0 = a && b` + `on _on_expr_0 { .. }`.
        (
            "an `on a && b` trigger",
            "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }",
            OK,
        ),
        (
            "a typed event param",
            "on CharacterDied() -> { character: c } { }",
            OK,
        ),
        (
            "an assignment in a handler",
            "var n: int = 0\non RoundStart() { n = n + 1 }",
            &[Silent],
        ),
        (
            "a return in a handler",
            "var x: int = 0\non RoundStart() { x = 1\nreturn\nx = 2 }",
            OK,
        ),
        (
            "a return inside an if",
            "var x: int = 0\non RoundStart() { if x > 5 { return } }",
            OK,
        ),
        ("`!` on an int", "var x: int = 0\nlet y = !x", OK),
        (
            "interpolating a var",
            "var x: int = 0\nlet s = \"value: ${x}\"",
            OK,
        ),
        // A single-output chip call auto-unwraps.
        (
            "a single-output chip read in pure context",
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f == 42",
            OK,
        ),
        (
            "a single-output chip read in exec context",
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nvar err: int = 0\non RoundStart() {\n  if f != 42 { err = 1 }\n}",
            OK,
        ),
        (
            "an annotated buffer",
            "var x: int = 0\nbuffer prev: int = x",
            OK,
        ),
        (
            "an inferred buffer",
            "var x: int = 0\nbuffer prev = x + 1",
            OK,
        ),
        (
            "a mod call in a handler",
            "var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart() { inc(x) }",
            OK,
        ),
        (
            "an anon chip shares its parent's scope",
            "var x: int = 0\nchip { var y: int = 0 }\non RoundStart() { x = 1 }",
            OK,
        ),
        (
            "`chip on` a handler",
            "var x: int = 0\nchip on RoundStart() { x = 1 }",
            OK,
        ),
        (
            "emit in a handler",
            "var x: int = 0\nout result = x\non RoundStart() { emit result }",
            OK,
        ),
        (
            "bool literals",
            "var x: bool = true\nvar y: bool = false",
            OK,
        ),
        (
            "chip exec params as triggers",
            "chip Counter(bump: exec, reset: exec) -> (value: int) {\n  var n: int = 0\n  on bump { n = n + 1 }\n  on reset { n = 0 }\n  out value = n.Value\n}",
            OK,
        ),
        (
            "a character coerces to an entity receiver",
            "in ch: character\non RoundStart() { ch.SetLocation(Vec(0.0, 0.0, 0.0)) }",
            OK,
        ),
        (
            "`chip let` is pure",
            "var x: int = 0\nchip let doubled = x * 2",
            OK,
        ),
        (
            "a receiver method call",
            "var ctrl: controller\non RoundStart() { ctrl.DisplayText(\"hi\") }",
            OK,
        ),
        // An entity wire (e.g. Sweep's HitEntity) can be a player, so
        // character/controller receiver methods and params accept it.
        (
            "an entity receiver takes a character method",
            "in e: entity\nin t: exec\non t { e.ShowStatusMessage(\"hi\") }",
            OK,
        ),
        (
            "an entity arg takes a character param",
            "in e: entity\nin t: exec\non t { ShowStatusMessage(e, \"hi\") }",
            OK,
        ),
        // Array reads need exec context (Exec_ArrayVar_Get).
        (
            "an array index is its element type",
            "var items: int[]\nin trigger: exec\non trigger { let x = items[0]\nlet ok = x + 1 }",
            OK,
        ),
        // Array params put the mod in exec context.
        (
            "an array param index",
            "mod process(arr: int[], idx: int) {\n  let old = arr[idx]\n  out r = old\n}",
            OK,
        ),
        (
            "an array param index `.value`",
            "mod process(arr: int[], idx: int) {\n  let old = arr[idx].value\n  out r = old\n}",
            OK,
        ),
        // Map subscript reads need exec context and type to the map's VALUE type.
        (
            "a map index is its value type",
            "var m: Map<int, int>\nin trigger: exec\non trigger { let x: int = m[1]\nlet ok = x + 1 }",
            OK,
        ),
        (
            "array push and pop",
            "var items: int[]\nin trigger: exec\non trigger { items.push(1)\nitems.pop() }",
            OK,
        ),
        (
            "an if-then-else value",
            "var x: int = 0\nlet y = if x > 0 then 1 else 0",
            OK,
        ),
        (
            "several interpolations",
            "var a: int = 1\nvar b: float = 2.0\nlet s = \"a=${a} b=${b}\"",
            OK,
        ),
        (
            "hex, binary and octal literals",
            "var a: int = 0xFF\nvar b: int = 0b1010\nvar c: int = 0o77",
            OK,
        ),
        (
            "record field access",
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet sum = p.x + p.y",
            OK,
        ),
        (
            "record shorthand",
            "type Point = { x: int, y: int }\nlet x = 1\nlet y = 2\nlet p: Point = { x, y }",
            OK,
        ),
        (
            "record spread",
            "type Point = { x: int, y: int }\nlet a: Point = { x: 1, y: 2 }\nlet b: Point = { ...a, y: 99 }",
            OK,
        ),
        (
            "record destructure",
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet { x, y } = p\nlet sum = x + y",
            OK,
        ),
        // A plain `let`'s `...rest` keeps the remaining fields' real types, so
        // arithmetic on them resolves (as `Type::Any` it was "no overload for
        // '+' on Any, Any").
        (
            "a runtime `let` rest carries the unconsumed field types",
            "type P = { x: int, y: int, z: int }\nlet p: P = { x: 1, y: 2, z: 3 }\nlet { x, ...rest } = p\nlet sum = rest.y + rest.z",
            OK,
        ),
        (
            "a record as a mod param",
            "type Point = { x: int, y: int }\nmod sum(p: Point) -> (r: int) { return p.x + p.y }\nlet p: Point = { x: 3, y: 4 }\nlet s = sum(p)",
            OK,
        ),
        (
            "a destructured record mod param",
            "type Point = { x: int, y: int }\nmod add({ x, y }: Point) -> int { return x + y }\nlet p: Point = { x: 3, y: 4 }\nlet sum = add(p)",
            OK,
        ),
        // `any` on a port resolves real operator overloads (the wildcard
        // `Type::Opaque` gives it), not the `Type::Any` that WS004 rejects.
        (
            "`any` port operators",
            "in t: any\nlet a = t & 1\nlet b = t + 1\nlet c = t == \"x\"",
            OK,
        ),
        (
            "an `any` let annotation",
            "let x: any = 5\nlet y = x + 1",
            OK,
        ),
        (
            "an `any` mod param",
            "mod f(v: any) { let inner = v + 1 }",
            OK,
        ),
        (
            "an `any` chip param and output",
            "chip C(v: any) -> (z: any) { out z = v }\nlet c = C(1)\nlet y = c + 1",
            OK,
        ),
        ("an `any` out annotation", "in t: any\nout y: any = t", OK),
        // Random rides the PrimMath variant: vector/color min/max give a matching result.
        (
            "Random is polymorphic on the math variant",
            "in a: vector\nin b: vector\nin c1: color\nin c2: color\nvar rv: vector = Vec(0.0, 0.0, 0.0)\nvar rc: color = ColorHex(\"#000000\")\nin go: exec\non go {\n  rv = Random(a, b)\n  rc = Random(c1, c2)\n}",
            OK,
        ),
        (
            "Random(int, int) into an int var",
            "var n: int = 0\nin go: exec\non go { n = Random(1, 10) }",
            OK,
        ),
        // Tween's `Value` resolves to the target's type, not the whole variant
        // union, so a float target reads as a float and formats into a string.
        (
            "a float Tween's Value is a float",
            "in target: float\nlet tw = Tween(target, 1.0)\nout v: float = tw.Value\nout s: string = \"${tw}\"\n",
            OK,
        ),
        // A tuple literal is an index-keyed record and a tuple annotation is a
        // `Type::Tuple`; they are the same shape.
        (
            "a tuple value fits a tuple param",
            "let w = (1, 2)\nmod f((a, b): (int, int)) -> int { return a + b }\nout o = f(w)\n",
            OK,
        ),
        (
            "a tuple literal fits a tuple param",
            "mod f((a, b): (int, int)) -> int { return a + b }\nout o = f((1, 2))\n",
            OK,
        ),
        // Record fields match ignoring ref-ness (the gba/chip8 `*int[]` vs
        // `int[]` and secret-hitler/2raab data-model plumbing).
        (
            "an array field via shorthand fits a plain array field",
            "var regs: int[]\ntype Cpu = { regs: int[] }\nmod cpu_init({ regs }: Cpu) { regs.push(0) }\non RoundStart() {\n  let cpu: Cpu = { regs }\n  cpu_init(cpu)\n}\n",
            OK,
        ),
        (
            "a plain scalar field fits a ref field",
            "var counter_a: int = 0\ntype State = { counter: *int, label: int }\nmod bump({ counter }: State) { counter = counter + 1 }\non RoundStart() {\n  let sa: State = { counter: counter_a, label: 1 }\n  bump(sa)\n}\n",
            OK,
        ),
        (
            "plain arrays and ref scalars by shorthand",
            "var regs: int[]\nvar cpsr: int = 0\nvar halted: bool = false\ntype Cpu = { regs: int[], cpsr: *int, halted: *bool }\nmod cpu_init({ regs, cpsr, halted }: Cpu) { cpsr = 1 }\non RoundStart() {\n  let cpu: Cpu = { regs, cpsr, halted }\n  cpu_init(cpu)\n}\n",
            OK,
        ),
        // The enclosing handler's EVENT data param is bound as EventParam.
        (
            "`on p` over an event data param",
            "on CustomEvent(\"x\") -> (p: character) {\n  on p { }\n}\n",
            OK,
        ),
        (
            "`on !p` over an event data param",
            "on CustomEvent(\"x\") -> (p: character) {\n  on !p { }\n}\n",
            OK,
        ),
        // Contextual keywords: only `unsafe <ident>` / `is <name>` is the construct.
        (
            "`unsafe` as an identifier",
            "var unsafe: int = 1\nout n = unsafe + 1",
            OK,
        ),
        (
            "`is` as an identifier",
            "var is: int = 1\nout n = is + 1",
            OK,
        ),
    ];
    check_rows(rows);
}

/// `@label` expressions, prefab references and custom-event params.
#[test]
fn labels_prefabs_and_event_params_are_checked() {
    const OK: &[Want] = &[NoErrors];
    let rows: &[(&str, &str, &[Want])] = &[
        // A rerouter pin has no text component to wire, so a runtime `@label`
        // on a PORT stays an error.
        (
            "a runtime label on a port",
            "in x: int\n@label(x) out v: int = 0",
            &[Has("WS040")],
        ),
        // A top-level `var`'s runtime label is DYNAMIC: wired into the label's
        // Text port at emit.
        ("a var labelled by itself", "@label(hp) var hp: int = 0", OK),
        (
            "a var labelled by another var",
            "var hp: int = 0\n@label(hp * 2) var shown: int = 0",
            OK,
        ),
        // A var label's names are still resolved.
        (
            "an undefined name in a var label",
            "@label(nope) var v: int = 0",
            &[HasMsg("WS002", "'nope'")],
        ),
        ("a folded constant label", "@label(1 + 2) chip { }", OK),
        (
            "a constant let as a label",
            "let title = \"Score\"\n@label(title) out v: int = 0",
            OK,
        ),
        // Decls inside a handler and inside an `if` are visited too.
        (
            "a runtime label inside a handler",
            "in x: int\non ControllerJoined() -> { controller: c, userId: id, userName: name } {\n@label(x) var y: int = 0\n}",
            &[Has("WS040")],
        ),
        (
            "a runtime label inside an `if`",
            "in go: exec\nin x: int\non go {\nif x > 0 {\n@label(x) var y: int = 0\n}\n}",
            &[Has("WS040")],
        ),
        // A prefab bound to a `let` is a constant for the constant-only `prefab=`.
        (
            "a body-local prefab let reaches SpawnPrefab",
            "in go: exec\non go { let pf = $./foo.brz\n  SpawnPrefab(prefab = pf) }",
            OK,
        ),
        (
            "a top-level prefab let reaches SpawnPrefab",
            "let pf = $./foo.brz\nin go: exec\non go { SpawnPrefab(prefab = pf) }",
            OK,
        ),
        // A runtime `var` is never a constant, whatever its initializer.
        (
            "a var prefab",
            "var pf: string = \"x\"\nin go: exec\non go { SpawnPrefab(prefab = pf) }",
            &[Has("WS028")],
        ),
        (
            "a prefab ref that is neither .brz nor .ws",
            "in go: exec\non go { SpawnPrefab(prefab = $./x.png) }",
            &[Has("WS019")],
        ),
        // An asset is wired in from its own brick and would be silently
        // dropped from a constant array initializer.
        (
            "an asset in an array initializer",
            "var songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]",
            &[Warning("WS024")],
        ),
        (
            "a constant array initializer",
            "var nums: int[] = [1, 2, 3]",
            &[Lacks("WS024")],
        ),
        // An event data slot is ONE wire; a record (or an enum: `__disc` plus
        // slots) param bound to one `Binding::EventParam` produced no gates,
        // so the statement vanished.
        (
            "a record-typed custom-event param",
            "type P = { a: int, b: int }\nvar q: P = { a: 0, b: 0 }\non CustomEvent(\"ch\") -> (p: P) { q = p }\n",
            &[Error("WS068"), FirstMsg("WS068", "p")],
        ),
        (
            "a scalar custom-event param",
            "var n: int = 0\non CustomEvent(\"ch\") -> (v: int) { n = v }\n",
            &[Lacks("WS068")],
        ),
        // A coercible annotation cannot tell a real type from `any`, so each
        // GetInputs field is checked against `vector`, which nothing coerces to.
        (
            "GetInputs outputs are typed",
            "in go: exec\nin pl: character\non go {\nlet inp = pl.GetInputs()\nlet axis: vector = inp.Forward\nlet flag: vector = inp.PressedC\n}",
            &[
                HasMsg("WS016", "has type float"),
                HasMsg("WS016", "has type bool"),
            ],
        ),
        (
            "an unknown GetInputs control",
            "in go: exec\nin pl: character\non go {\nlet inp = pl.GetInputs()\nlet x = inp.Jump\n}",
            &[Has("WS010")],
        ),
    ];
    check_rows(rows);
}

/// Exec-only work in a pure position is WS007: a runtime array/map read, or
/// a pure call of a mod whose inlined body touches a container (reported at
/// the call site). A compile-time index into a compile-time container emits
/// no gate, so it is not WS007.
#[test]
fn exec_only_work_in_a_pure_position_is_ws007() {
    const WS007: &[Want] = &[Has("WS007")];
    let tag = "mod tag(k: int) -> string { let v = m.get(k); return v.Value }";
    let rows: &[(&str, String, &[Want])] = &[
            // An exec gate samples where the chain reaches it; in pure position it
            // would lower to a placeholder reading a default for every control.
            ("GetInputs in pure position", "in pl: character\nlet inp = pl.GetInputs()".into(), WS007),
            ("a map index read outside exec", "var m: Map<int, int>\nlet x = m[1]".into(), WS007),
            ("a const index into a const array", "const t = [10, 20]\nconst z = t[1]".into(), &[Lacks("WS007")]),
            // Only the WS007 half: a bare `const`-bound map literal is a separate
            // WS026 error, so this shape never reaches emit.
            ("a const index into a const map", "const m = { :k => 5 }\nconst z = m[:k]".into(), &[Lacks("WS007")]),
            // A `const` receiver does not make a runtime index compile-time.
            ("a runtime index into a const array", "const t = [10, 20]\nvar someVar: int\nlet x = t[someVar]".into(), WS007),
            ("a const index into a runtime array", "var runtimeArr: int[]\nlet x = runtimeArr[1]".into(), WS007),
            // A `const` param's value in its own body is a placeholder zero seeded
            // before any call site exists; folding `t[m]` with it would suppress
            // WS007 for an index that is unknown here. The anon `chip {}` puts the
            // statements in pure context so WS007 is reachable at all.
            ("a const index from a const param's placeholder", "mod f(m: const int) {\n  chip {\n    const t = [10, 20]\n    const z = t[m]\n  }\n}\nin go: exec\non go { f(1) }".into(), WS007),
            // Assigning a scalar `let` checked clean and emitted no gate.
            ("an assignment to a scalar let", "in start: exec\nvar x: int = 3\nlet y = x + 1\non start { y = 5 }".into(), WS007),
            ("an array let stays writable through its methods", "in start: exec\nvar xs: int[] = []\nlet ys = xs\non start { ys.push(1) }".into(), &[NoErrors]),
            ("a pure call of a map-reading mod", format!("var m: Map<int, string>\nin go: exec\non go {{ m.set(1, \"x\") }}\n{tag}\nout bad = tag(1)"), WS007),
            ("the same mod called from a handler", format!("var m: Map<int, string>\nvar res: string\n{tag}\nin go: exec\non go {{ m.set(1, \"x\"); res = tag(1) }}"), &[NoErrors]),
            ("an uncalled map-reading mod", format!("var m: Map<int, string>\nin go: exec\non go {{ m.set(1, \"x\") }}\n{tag}"), &[NoErrors]),
            ("a pure arithmetic mod called from pure", "mod dbl(k: int) -> int { return k * 2 }\nout good = dbl(21)".into(), &[NoErrors]),
            ("a pure call of a transitively map-reading mod", "var m: Map<int, string>\nin go: exec\non go { m.set(1, \"x\") }\nmod inner(k: int) -> string { let v = m.get(k); return v.Value }\nmod outer(k: int) -> string { return inner(k) }\nout bad = outer(1)".into(), WS007),
            ("a pure call of a subscript-reading mod", "var m: Map<int, string>\nin go: exec\non go { m.set(1, \"x\") }\nmod tag(k: int) -> string { return m[k] }\nout bad = tag(1)".into(), WS007),
            // `exec = <trigger>` supplies the exec context the pure call lacks.
            ("a pure call with an `exec =` override", format!("var m: Map<int, string>\nin go: exec\n{tag}\nout bad = tag(1, exec = go)"), &[Lacks("WS007")]),
            // `sort` is an array method name, but this is a pure self mod on a record.
            ("a self mod named like a container method", "type Point = { x: int, y: int }\nmod sort(self: Point) -> int { return self.x + self.y }\nlet p: Point = { x: 1, y: 2 }\nout good = p.sort()".into(), &[NoErrors]),
            // A read on a `const` container is exempt (it should const-fold).
            ("a pure call of a const-map-reading mod", format!("const m: Map<int, string> = {{ 1 => \"x\" }}\n{tag}\nout ok = tag(1)"), &[NoErrors]),
            ("`emit` as a value in pure context", "let d: exec = emit".into(), &[Error("WS007")]),
        ];
    check_rows(rows);
}

/// Container method arguments are checked against the container's types and
/// the method's arity.
#[test]
fn container_method_arguments_are_checked() {
    let rows: &[(&str, &str, &[Want])] = &[
        // Storing the map itself where an `entity` value is expected (the
        // `prefabs.set(id, prefabs)` bug) is a WS003, not a dangling load wire.
        (
            "map.set value mismatch",
            "var m: Map<string, entity>\nin go: exec\non go { m.set(\"x\", m) }",
            &[Has("WS003")],
        ),
        (
            "map.get key mismatch",
            "var m: Map<string, int>\nin go: exec\non go { let v = m.get(m) }",
            &[Has("WS003")],
        ),
        (
            "map set/get with matching types",
            "var m: Map<string, int>\nin go: exec\non go { m.set(\"x\", 5)\n  let v = m.get(\"x\") }",
            &[NoErrors],
        ),
        // Map methods have fixed params, so a missing arg is arity (WS011): the
        // old validator coerced only the args present, so `m.set("k")` checked
        // clean and emitted a dangling wire at load.
        (
            "map.set missing value",
            "var m: Map<string, int>\nin s: exec\non s { m.set(\"k\") }",
            &[Has("WS011")],
        ),
        (
            "map.get missing key",
            "var m: Map<string, int>\nin s: exec\non s { let g = m.get() }",
            &[Has("WS011")],
        ),
        (
            "map.keys missing dest array",
            "var m: Map<string, int>\nin s: exec\non s { let g = m.keys() }",
            &[Has("WS011")],
        ),
        (
            "map.set string value into an int map",
            "var m: Map<string, int>\nin s: exec\non s { m.set(\"k\", \"v\") }",
            &[Has("WS003")],
        ),
        (
            "map.get with the map as its key",
            "var m: Map<string, int>\nin s: exec\non s { let g = m.get(m) }",
            &[Has("WS003")],
        ),
        (
            "map set/get fully supplied",
            "var m: Map<string, int>\nin s: exec\non s { m.set(\"k\", 1)\n  let g = m.get(\"k\") }",
            &[NoErrors],
        ),
        (
            "map.keys into a dest array",
            "var m: Map<string, int>\nvar destArr: string[]\nin s: exec\non s { m.keys(destArr) }",
            &[NoErrors],
        ),
        // A method receiver is typed through the whole field chain, as
        // lowering resolves it, so `g.ready.sum()` is not `any`.
        (
            "an array method through a record field",
            "type G = { ready: int[] }\nmod f(g: G) { let n = g.ready.sum() * 4 }",
            &[NoErrors],
        ),
        (
            "a map method through a record field",
            "type G = { counts: Map<int, int> }\nmod f(g: G) { let n = g.counts.get(1) + 1 }",
            &[NoErrors],
        ),
        (
            "an array method through a nested record",
            "type G = { ready: int[] }\ntype O = { g: G }\nmod f(o: O) { let n = o.g.ready.sum() * 2 }",
            &[NoErrors],
        ),
        (
            "the element type survives the field chain",
            "type G = { ready: int[] }\nmod f(g: G) { g.ready.push(\"nope\") }",
            &[Has("WS003")],
        ),
        // Array-method args go through `check_args`: a string pushed onto an
        // `int[]` would emit a mistyped pin.
        (
            "arr.push(wrong type)",
            "var xs: int[]\nin s: exec\non s { xs.push(\"a\") }",
            &[Has("WS003")],
        ),
        (
            "arr.insert(wrong index type, _)",
            "var xs: int[]\nin s: exec\non s { xs.insert(\"i\", 1) }",
            &[Has("WS003")],
        ),
        (
            "array calls with matching types",
            "var xs: int[]\nin s: exec\non s { xs.push(1)\n  xs.insert(0, 2)\n  let v = xs.get(0) }",
            &[NoErrors],
        ),
        (
            "arr.pop(extra arg)",
            "var xs: int[]\nin s: exec\non s { xs.pop(5) }",
            &[Has("WS011")],
        ),
        // `sortMultiple` is a true variadic (empty declared params opt it out of
        // arity), so parallel arrays must not hit "expects at most 0, got N".
        (
            "sortMultiple is variadic",
            "var xs: int[]\nin s: exec\non s { xs.sortMultiple(xs, xs) }",
            &[NoErrors],
        ),
        (
            "`exec =` on a pure array read",
            "var lut: color[]\nin i: int\nout c: color = lut.get(i, exec = i + 1).Value",
            &[NoErrors],
        ),
        // `fillFromZone*`'s zone param is `zone` ONLY: a zone rerouter, never a
        // plain entity.
        (
            "fillFromZonePlayers(zone)",
            "in z: zone\nvar players: character[]\nin s: exec\non s { players.fillFromZonePlayers(z) }",
            &[NoErrors],
        ),
        (
            "fillFromZonePlayers(zone, tag)",
            "in z: zone\nvar players: character[]\nin s: exec\non s { players.fillFromZonePlayers(z, \"red\") }",
            &[NoErrors],
        ),
        (
            "fillFromZoneEntities(zone, tag)",
            "in z: zone\nvar ents: entity[]\nin s: exec\non s { ents.fillFromZoneEntities(z, \"tag\") }",
            &[NoErrors],
        ),
        (
            "an entity in a zone slot",
            "in e: entity\nvar players: character[]\nin s: exec\non s { players.fillFromZonePlayers(e) }",
            &[Has("WS003")],
        ),
    ];
    check_rows(rows);
}

/// Values are checked against where they go: annotations, fields, params,
/// vars and storage.
#[test]
fn values_are_checked_against_their_destination() {
    const WS003: &[Want] = &[Has("WS003")];
    const OK: &[Want] = &[NoErrors];
    let foo = "chip Foo(x: int) -> (result: int) {\n out result = x * 2\n}\nlet f = Foo(21)\n";
    let two = "mod two() -> (a: int, b: string) {\n";
    let rows: &[(&str, String, &[Want])] = &[
            // A tuple/record `let` annotation validates its value like `var`, params
            // and returns. The `let` path checked record field NAMES only and skipped
            // tuple annotations (a tuple literal is an index-keyed record whose
            // expected `Type::Tuple` the name check bailed on).
            ("a tuple element type mismatch", "let t: (int, int) = (\"s\", 5)".into(), WS003),
            ("a tuple arity mismatch", "let t: (int, int) = (1, 2, 3)".into(), WS003),
            ("a record in a tuple missing a field", "type Card = { baz: int }\nlet t: (Card, int) = ({}, 5)".into(), WS003),
            ("a record in a tuple with a wrong field type", "type Card = { baz: int }\nlet t: (Card, int) = ({ baz: \"s\" }, 5)".into(), WS003),
            ("a record field value type", "type Card = { baz: int }\nlet c: Card = { baz: \"s\" }".into(), WS003),
            ("a nested record field type", "type C = { bar: { baz: int } }\nlet c: C = { bar: { baz: \"s\" } }".into(), WS003),
            ("a valid tuple let", "let t: (int, int) = (1, 2)".into(), OK),
            ("a valid record let", "type Card = { baz: int }\nlet c: Card = { baz: 1 }".into(), OK),
            // Everything primitive casts to string: a format, not a WS016 type lie.
            ("a string annotation on a number", "let s: string = 5".into(), OK),
            // Representation-insensitivity must not accept a real arity mismatch.
            ("a 3-tuple into a 2-tuple param", "mod f((a, b): (int, int)) -> int { return a + b }\nout o = f((1, 2, 3))\n".into(), &[HasMsg("WS003", "expected (int, int), got (int, int, int)")]),
            // Ref-insensitivity must not loosen a genuine field value mismatch.
            ("an int field where a string field is expected", "type A = { x: int }\ntype B = { x: string }\nmod take(b: B) { }\non RoundStart() {\n  let a: A = { x: 5 }\n  take(a)\n}\n".into(), WS003),
            ("Random(int, int) into a vector var", "var v: vector = Vec(0.0, 0.0, 0.0)\nin go: exec\non go { v = Random(1, 10) }".into(), &[HasMsg("WS003", "expected vector, got int")]),
            // Inference makes the var `int`, so the vector is a mismatch.
            ("a vector into an inferred int var", "var n = 0\non RoundStart() { n = Vec(1.0, 1.0, 1.0) }".into(), WS003),
            ("a vector into an inferred color var", "var tint = Color(1.0, 0.0, 0.0)\non RoundStart() { tint = Vec(1.0, 1.0, 1.0) }".into(), WS003),
            // A plain `let`'s `...rest` is the record of the unconsumed fields, not
            // `Type::Any`, which satisfied every parameter and field access.
            ("a rest where the full record is expected", "type P = { x: int, y: int, z: int }\nmod takesP(p: P) -> int { return p.x }\nlet p: P = { x: 1, y: 2, z: 3 }\nlet { x, ...rest } = p\nlet bad = takesP(rest)".into(), WS003),
            ("a consumed field read off a rest", "type P = { x: int, y: int }\nlet p: P = { x: 1, y: 2 }\nlet { x, ...rest } = p\nlet bad = rest.x".into(), &[HasMsg("WS010", "no field `x`")]),
            // A scalar has no fields; typed `any`, lowering would read the whole
            // base value.
            ("a field on a scalar", "in go: exec\nvar a: int = 5\nvar b: int = 0\non go {\n  let c = a\n  b = c.whatever\n}".into(), &[Has("WS010")]),
            // The one exception: a single-output call result projected by its
            // output name.
            ("a single-output result projected by name", format!("{foo}let ok = f.result\nout o = ok"), OK),
            ("a re-bound single-output result projected by name", format!("{foo}let g = f\nlet ok = g.result\nout o = ok"), OK),
            ("a mis-typed output name", format!("{foo}let bad = f.reslt\nout o = bad"), &[Has("WS010")]),
            // A `{Vector, Rotation}` record unwraps to outputs[0] (the Vector) into
            // a rotator sink.
            ("a record unwrapping to a non-first field", "on CharacterSpawned() -> (c) {\n  let rot: rotator = c.GetVelocity()\n  c.SetRotation(rot)\n}".into(), &[Error("WS003")]),
            ("a tuple literal as a scalar", "let x: int = (1, \"abc\")\nout o = x".into(), WS003),
            // Typed from element 0, the odd element was pushed with no check.
            ("a heterogeneous array literal assigned", "in start: exec\nvar xs: int[] = []\non start { xs = [1, \"hello\", 2] }".into(), WS003),
            ("a homogeneous array literal assigned", "in start: exec\nvar xs: int[] = []\non start { xs = [1, 2, 3] }".into(), OK),
            // `==`/`!=` on the certified composite variants; ordering is scalar-only.
            ("vector equality", "let a = Vec(1.0, 2.0, 3.0)\nlet b = Vec(1.0, 2.0, 3.0)\nout eq = a == b".into(), OK),
            ("color inequality", "let a = Color(1.0, 0.0, 0.0, 1.0)\nlet b = Color(0.0, 1.0, 0.0, 1.0)\nout ne = a != b".into(), OK),
            ("vector ordering", "let a = Vec(1.0, 2.0, 3.0)\nlet b = Vec(1.0, 2.0, 3.0)\nout o = a < b".into(), &[Has("WS004")]),
            ("a concrete array into an `any[]` param", "in start: exec\nmod firstlen(xs: any[]) { BroadcastChatMessage(\"x\") }\nvar ns: int[] = []\non start { firstlen(ns) }".into(), OK),
            // Distributing `*T` over the fields must not let a record of refs back
            // a container: a stored record needs a real value per field.
            ("a ref-field record array", "type T = { a: *float, b: float }\nvar xs: T[]\n".into(), &[Has("WS049")]),
            ("a ref-field record map", "type T = { a: *float, b: float }\nvar m: Map<int, T>\n".into(), &[Has("WS049")]),
            // A `*T` param distributes to ref fields; a read is the VALUE.
            ("a ref record field reads as its value", "type S = { a: float, b: bool }\nchip M(s: *S, f: float) -> (r: float) {\n  var out1: float = s.a\n  out r = if s.b then s.a else f\n}\nvar g: S = { a: 1.0, b: false }\nvar res: float = 0.0\nin go: exec\non go { res = M(g, 2.0) }\n".into(), OK),
            // A ref field needs storage: a literal of `in` ports has none.
            ("a port-built literal into a `*T` param", "type S = { a: float }\nchip M(s: *S) { s.a = 1.0 }\nin a: float\nin go: exec\non go { M({ a: a }) }\n".into(), &[Has("WS008")]),
            ("a var-built literal into a `*T` param", "type S = { a: float }\nchip M(s: *S) { s.a = 1.0 }\nvar v: float = 0.0\nin go: exec\non go { M({ a: v }) }\n".into(), &[Lacks("WS008")]),
            // A `*T` param is the caller's own gate, with no wire to convert on.
            ("`*string` bound to an int var", "mod put(v: *string) { v = \"hi\" }\nvar n: int = 0\nin go: exec\non go { put(n) }".into(), &[Error("WS003")]),
            ("`*int` bound to a float var", "mod inc(v: *int) { v = v + 1 }\nvar y: float = 0.0\nin go: exec\non go { inc(y) }".into(), &[Error("WS003")]),
            ("a chip's `*int` bound to a float var", "chip C(a: *int, t: exec) { on t { a = a + 1 } }\nvar y: float = 0.0\nin go: exec\nlet c = C(y, go)".into(), &[Error("WS003")]),
            ("`*int` bound to an int var", "mod inc(v: *int) { v = v + 1 }\nvar n: int = 0\nin go: exec\non go { inc(n) }".into(), OK),
            ("`*string` bound to a string var", "mod put(v: *string) { v = \"hi\" }\nvar s: string = \"\"\nin go: exec\non go { put(s) }".into(), OK),
            ("`*vector` bound to a vector var", "mod setv(v: *vector) { v = Vec(1.0, 2.0, 3.0) }\nvar p: vector\nin go: exec\non go { setv(p) }".into(), OK),
            ("`ref int` bound to an int var", "mod inc(v: ref int) { v = v + 1 }\nvar n: int = 0\nin go: exec\non go { inc(n) }".into(), OK),
            // An array literal is the widening join of its elements, not element 0:
            // `[1, 2.5]` read as `int[]` and stored the float in an int gate.
            ("a float in an int array param", "mod f(xs: int[]) -> (n: int) { return xs.length() }\nin go: exec\non go { let n = f([1, 2.5]) }\n".into(), &[ErrMsg("expected int[], got float[]")]),
            ("a widening array literal into a float array", "mod g(xs: float[]) -> (n: int) { return xs.length() }\nin go: exec\non go { let n = g([1, 2.5]) }\n".into(), OK),
            ("an int then a string element", "in go: exec\non go { let a = [1, \"x\"] }\n".into(), &[Msg("array element")]),
            ("a string then an int element", "in go: exec\non go { let a = [\"x\", 1] }\n".into(), &[Msg("array element")]),
            // A multi-output `return` is checked per port: a bare `infer` over the
            // whole value wired the string into the int gate in both forms.
            ("a swapped record return", format!("{two}  let t = \"x\"\n  let i = 5\n  return {{ a: t, b: i }}\n}}\nin go: exec\non go {{ let r = two() }}\n"), &[ErrMsg("expected int, got string")]),
            ("a swapped tuple return", format!("{two}  return (\"x\", 2)\n}}\nin go: exec\non go {{ let r = two() }}\n"), &[ErrMsg("expected int, got string")]),
            ("both return forms written correctly", format!("{two}  let i = 5\n  let t = \"x\"\n  return {{ a: i, b: t }}\n}}\nmod three() -> (a: int, b: string) {{\n  return (1, \"y\")\n}}\nin go: exec\non go {{ let r = two()\n  let q = three() }}\n"), OK),
        ];
    check_rows(rows);
}

#[test]
fn pure_chip_stmt_records_var_read_as_pure() {
    // A `let` inside a pure `chip { }` — even one that follows a handler, so
    // the chip is exec-wrapped — reads its vars continuously. The read must
    // record as PURE in `var_read_contexts` (false), or the hover shows the
    // wrong exec context. (Mirrors the lowering's per-statement chip purity.)
    let r =
        tc("var v: int = 0\nin go: exec\non go { v = 5 }\nchip { let x = v + 1 }\nout r: int = x");
    assert!(!r.var_read_contexts.is_empty(), "no var reads recorded");
    assert!(
        r.var_read_contexts.values().all(|&is_exec| !is_exec),
        "a pure chip's var read was recorded as exec: {:?}",
        r.var_read_contexts
    );
}

/// Calls are checked against what they call: arity, argument types, the
/// receiver form and the names involved.
#[test]
fn calls_are_checked_against_their_signature() {
    let dist = "mod dist(self: vector, o: vector) -> float { return 0.0 }\n";
    let rows: &[(&str, String, &[Want])] = &[
            ("an unknown identifier", "on RoundStart() { x = 1 }".into(), &[Has("WS002")]),
            ("an unknown generic", "mod f(x: Bogus<int>) { }".into(), &[Has("WS002")]),
            // A namespace call whose base is not in scope (an `import * as card`
            // removed while `card.drawLobby(..)` calls remain) is reported, not
            // lowered to an `_Unsupported` gate that does nothing.
            ("an undefined namespace base", "mod drawLobby(n: int) { }\non RoundStart() { card.drawLobby(1) }".into(), &[HasMsg("WS002", "card")]),
            // Top-level assigns have no enclosing exec chain.
            ("an assignment outside exec", "var n: int = 0\nn = 1".into(), &[Has("WS007")]),
            // User chips/mods have no default params, so too few or too many
            // positional args leaves a param unbound or an arg dropped.
            ("too few args to a mod", "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }".into(), &[Has("WS022")]),
            ("too many args to a mod", "mod g(a: int) -> int { return a }\nin z: exec\non z { let x = g(1, 2) }".into(), &[Has("WS022")]),
            ("matching arity", "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1, 2) }".into(), &[Lacks("WS022")]),
            // An `exec =` trigger is not a parameter, so it is not counted.
            ("matching arity plus an `exec =` trigger", "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1, 2, exec = z) }".into(), &[Silent]),
            // A plain user-mod call's positional args are checked, and arity stays
            // WS022-only (no WS011 from `check_args`'s own arity check).
            ("a user-mod arg type mismatch", "mod f(v: vector) { }\nin z: exec\non z { f(5) }".into(), &[Has("WS003"), Lacks("WS011")]),
            ("a valid user-mod call", "mod f(v: vector) { }\nin p: vector\nin z: exec\non z { f(p) }".into(), &[NoErrors]),
            ("a valid self-receiver call", format!("{dist}in a: vector\nin b: vector\nin z: exec\non z {{ let d = a.dist(b) }}"), &[NoErrors]),
            ("a self-receiver call with a mismatched arg", format!("{dist}in a: vector\nin z: exec\non z {{ let d = a.dist(5) }}"), &[Has("WS003")]),
            // The receiver binds to `self` as positional arg 0.
            ("a mismatched self receiver", format!("{dist}in n: int\nin b: vector\nin z: exec\non z {{ let d = n.dist(b) }}"), &[Has("WS003")]),
            ("too many args to a builtin", "on RoundStart() { Random(1, 2, 3, 4, 5) }".into(), &[Has("WS011")]),
            ("a builtin arg type mismatch", "on RoundStart() { SetLocation(42, Vec(0.0, 0.0, 0.0)) }".into(), &[HasMsg("WS003", "argument")]),
            // A receiver-method arg mismatch fires the WS003 the plain call does.
            ("a receiver-method arg mismatch", "in e: entity\non RoundStart() { e.SetLocation(5) }".into(), &[HasMsg("WS003", "argument")]),
            ("a valid receiver-method call", "in e: entity\non RoundStart() { e.SetLocation(Vec(0.0, 0.0, 0.0)) }".into(), &[NoErrors]),
            // A `self` mod named like a builtin receiver method on the same receiver
            // type would be shadowed by the builtin at every call site.
            ("a self mod shadowing a builtin receiver method", "mod Dot(self: vector, o: vector) -> float { return 0.0 }".into(), &[Has("WS035")]),
            ("a distinctly named self mod", "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }".into(), &[Lacks("WS035")]),
            ("the builtin's name on a different receiver type", "mod Dot(self: int) -> int { return self }".into(), &[Lacks("WS035")]),
            // A non-`self` mod is never a receiver method, so it cannot shadow one.
            ("a non-self mod with a builtin's name", "mod Dot(a: vector, o: vector) -> float { return a.Dot(o) }".into(), &[Lacks("WS035")]),
            // With the receiver bound as arg 0, `a.dist()` still lacks `o`.
            ("a self-mod method call missing an arg", "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\nin a: vector\nin go: exec\non go { let d = a.dist() }".into(), &[Has("WS022")]),
            // `Sweep`/`SweepSimple` act on their own brick, so
            // `body.SweepSimple(..)` takes no receiver.
            ("a no-receiver builtin called with a receiver", "in body: character\nin go: exec\non go { let hit = body.SweepSimple(500.0, detectPlayers1 = true) }".into(), &[Has("WS036")]),
            ("a no-receiver builtin called plainly", "in go: exec\non go { let hit = SweepSimple(500.0, detectPlayers1 = true)\n let d = hit.HitDistance }".into(), &[Silent]),
            // An unknown base is the primary problem and must not be masked by
            // the non-self-mod WS036 check.
            ("an unknown base before a known mod name", "mod scale(v: vector, k: float) -> vector { return v * k }\nin go: exec\non go { let s = missing.scale(2.0) }".into(), &[Has("WS002"), Lacks("WS036")]),
            // A typo'd event config arg matches no slot and silently no-ops.
            ("an unknown event config arg", "on Clock(intreval = 2.0) { BroadcastChatMessage(\"t\") }".into(), &[Has("WS041")]),
            ("a real event config arg", "on Clock(enabled = true) { BroadcastChatMessage(\"t\") }".into(), &[Lacks("WS041")]),
            // A call to a mod declared later cannot resolve during lowering
            // (decls register in source order).
            ("use before declaration", "mod caller() { let x = target(1) }\nmod target(n: int) -> int { return n }".into(), &[Has("WS021")]),
            ("declaration before use", "mod target(n: int) -> int { return n }\nmod caller() { let x = target(1) }".into(), &[Lacks("WS021")]),
        ];
    check_rows(rows);
}

/// Generic calls: inference failures and bound violations are WS033 with no
/// spurious WS003, and in-bound calls stay clean. Masks (`class_mask`):
/// Scalar = {int, float}; Numeric = {int, float, vector, rotator, quat,
/// color}; an unbounded `<T>` admits all 11 wire value variants.
#[test]
fn generic_calls_are_checked_against_their_bounds() {
    const OUT_OF_MASK: Want = HasMsg("WS033", "isn't allowed by its bound");
    let only_scalar = "mod onlyScalar<T: Scalar>(v: T) -> T { return v }\n";
    let rows: &[(&str, String, &[Want])] = &[
        // Two args pin `T` to int and vector, which have no common widening
        // (InferError::Conflict). The param stays an unresolved `Type::Param`,
        // which the coerce path must skip rather than emit a WS003 beside it.
        (
            "conflicting arg types",
            "in flag: bool\nin v: vector\n\
              mod pick<T>(c: bool, a: T, b: T) -> T { return a }\n\
              let x = pick(flag, 1, v)\n"
                .into(),
            &[HasMsg("WS033", "must be the same type"), Lacks("WS003")],
        ),
        // `T` only in the return, no explicit type arg (InferError::Unpinnable).
        (
            "a return-only type param",
            "mod zero<T: Numeric>() -> T { static var z: T\n return z }\n\
              out r: int = zero()\n"
                .into(),
            &[HasMsg("WS033", "cannot infer type parameter")],
        ),
        (
            "a string into a Numeric bound",
            "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\n\
              let x = onlyNumeric(\"hi\")\n"
                .into(),
            &[OUT_OF_MASK],
        ),
        (
            "conflicting Select args",
            "in c: bool\nout y = Select(c, 5, \"hello\")".into(),
            &[Has("WS033")],
        ),
        (
            "Select of two ints",
            "in c: bool\nout y = Select(c, 1, 2)".into(),
            &[NoErrors],
        ),
        (
            "Select of int and float joins",
            "in c: bool\nvar i: int = 0\nvar f: float = 0.0\nout y = Select(c, i, f)".into(),
            &[NoErrors],
        ),
        (
            "Select of two zones",
            "in c: bool\nin z: zone\nlet w = Select(c, z, z)".into(),
            &[NoErrors],
        ),
        // Swap's two output fields share `T`; the `resolved` memo reports once.
        (
            "a Swap conflict reports once",
            "in c: bool\nout y = Swap(c, 5, \"hello\")".into(),
            &[Count("WS033", 1)],
        ),
        // Select/Swap/SleepTicks resolve their `Type::Param` output to the
        // args' type: were it `any`, no annotated-let mismatch would fire.
        (
            "Select of ints is an int",
            "in go: exec\non go { let v: vector = Select(true, 1, 2) }\n".into(),
            &[HasMsg("WS016", "int")],
        ),
        (
            "Select of strings is a string",
            "in go: exec\non go { let v: vector = Select(false, \"x\", \"y\") }\n".into(),
            &[HasMsg("WS016", "string")],
        ),
        (
            "Swap's Output is the arg type",
            "in go: exec\non go { let s = Swap(true, 1, 2)\n  let v: vector = s.Output }\n".into(),
            &[HasMsg("WS016", "int")],
        ),
        (
            "SleepTicks passes its value's type",
            "in go: exec\non go { let v: vector = SleepTicks(5, delay = 1) }\n".into(),
            &[HasMsg("WS016", "int")],
        ),
        // `+` has an operator rule for every Numeric member.
        (
            "a Numeric bound accepts every mask member",
            "mod addT<T: Numeric>(a: T, b: T) -> T { return a + b }\n\
              in i: int\nin f: float\nin v: vector\nin rot: rotator\nin q: quat\nin c: color\n\
              out ri: int = addT(i, i)\n\
              out rf: float = addT(f, f)\n\
              out rv: vector = addT(v, v)\n\
              out rr: rotator = addT(rot, rot)\n\
              out rq: quat = addT(q, q)\n\
              out rc: color = addT(c, c)\n"
                .into(),
            &[NoErrors],
        ),
        (
            "a Scalar bound accepts int and float",
            "mod addS<T: Scalar>(a: T, b: T) -> T { return a + b }\n\
              in i: int\nin f: float\n\
              out ri: int = addS(i, i)\n\
              out rf: float = addS(f, f)\n"
                .into(),
            &[NoErrors],
        ),
        // vector is in Numeric but not Scalar. OutOfMask leaves `subst` None,
        // so `check_args` must skip the still-generic `v: T` param.
        (
            "a vector into a Scalar bound",
            format!("{only_scalar}in vec: vector\nlet x = onlyScalar(vec)\n"),
            &[OUT_OF_MASK, Lacks("WS003")],
        ),
        (
            "a bool into a Scalar bound",
            format!("{only_scalar}in b: bool\nlet x = onlyScalar(b)\n"),
            &[OUT_OF_MASK],
        ),
        (
            "an entity into a Scalar bound",
            format!("{only_scalar}in e: entity\nlet x = onlyScalar(e)\n"),
            &[OUT_OF_MASK],
        ),
        (
            "a vector into a Numeric bound",
            "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\n\
              in vec: vector\nlet x = onlyNumeric(vec)\n"
                .into(),
            &[NoErrors],
        ),
        // `&` has rules only for int/float/bool. The bounded body is checked
        // once per mask member, called or not; a binary op with no overload is
        // WS011 (see the `matches!` dispatch in `Expr::BinOp` inference).
        (
            "a body op missing for some Numeric members",
            "mod bitAnd<T: Numeric>(a: T, b: T) { let x = a & b }".into(),
            &[Has("WS011")],
        ),
        (
            "a body op present for every Numeric member",
            "mod addT<T: Numeric>(a: T, b: T) -> T { return a + b }".into(),
            &[NoErrors],
        ),
        // An explicit type argument is validated against the bound like inference.
        (
            "an explicit type arg outside a Scalar bound",
            "mod onlyScalarZero<T: Scalar>() -> T { static var z: T\n return z }\n\
              out r: int = onlyScalarZero<string>()\n"
                .into(),
            &[OUT_OF_MASK],
        ),
        (
            "too few explicit type args",
            "mod pair<A: Scalar, B: Scalar>(a: A, b: B) -> A { return a }\n\
              in x: int\nin y: float\nout r: int = pair<int>(x, y)\n"
                .into(),
            &[HasMsg("WS033", "type argument")],
        ),
        (
            "type args on a non-generic mod",
            "mod addOne(a: int) -> int { return a + 1 }\nin x: int\nout r: int = addOne<int>(x)\n"
                .into(),
            &[HasMsg("WS033", "not generic")],
        ),
        // int and vector are both Numeric, but have no common widening.
        (
            "a Numeric-bounded conflict",
            "mod pick2<T: Numeric>(a: T, b: T) -> T { return a }\n\
              in n: int\nin v: vector\nlet x = pick2(n, v)\n"
                .into(),
            &[HasMsg("WS033", "must be the same type")],
        ),
        // `T[]` and `Map<string, V>` resolve T and V from the args' shapes. The
        // key stays concrete: an unbounded `K` as a map key is WS039.
        (
            "array and map generic params",
            "mod combo<T, V>(xs: T[], m: Map<string, V>, fallback: T) -> T { return fallback }\n\
              var ints: int[] = [1, 2, 3]\nvar strs: Map<string, bool>\n\
              in go: exec\non go { let r = combo(ints, strs, 5) }\n"
                .into(),
            &[NoErrors],
        ),
        // `+ 1` only checks if the return was substituted to the call's type.
        (
            "a generic return substitutes",
            "mod identity<T>(v: T) -> T { return v }\n\
              in n: int\nout r: int = identity(n) + 1\n"
                .into(),
            &[NoErrors],
        ),
    ];
    check_rows(rows);
}

/// A `$```...```` block types as a prefab ref and maps to a
/// `Literal::NestedPrefab` carrying the raw inner source verbatim, as
/// `Expr::PrefabRef` maps to `Literal::PrefabRef`.
#[test]
fn nested_prefab_types_as_prefab_and_lowers_to_literal() {
    let src = "in go: exec\non go { SpawnPrefab($```in a: exec```) }\n";
    let r = tc(src);
    assert_no_diags(&r);
    let block = "$```in a: exec```";
    let at = src.find(block).unwrap();
    assert_eq!(
        r.type_of_expr
            .get(&(std::sync::Arc::from("test"), at, at + block.len())),
        Some(&Type::PrefabRef),
        "the block must type as a prefab ref"
    );
    let expr = crate::ast::Expr::NestedPrefab {
        source: "in a: exec".to_string(),
        range: crate::diagnostic::SourceRange::default(),
    };
    assert_eq!(
        crate::lower::expr_to_literal(&expr),
        Some(crate::ir::Literal::NestedPrefab {
            source: "in a: exec".to_string()
        })
    );
}

#[test]
fn nested_prefab_inner_error_remaps_into_outer_file() {
    // A syntax error INSIDE the block must surface (not be dropped), remapped
    // into the block's outer-file span (not left at the inner offset 0).
    let src = "in go: exec\non go { SpawnPrefab($```on q { let y = }```) }\n";
    let r = tc(src);
    let err = r.diagnostics.iter().find(|d| d.severity == Severity::Error);
    assert!(
        err.is_some(),
        "inner error should surface: {:?}",
        r.diagnostics
    );
    let block = src.find("$```").unwrap();
    let d = err.unwrap();
    assert!(
        d.range.start.offset >= block,
        "diagnostic remapped into the block span (offset {} vs block start {})",
        d.range.start.offset,
        block
    );
}

#[test]
fn nested_prefab_custom_event_slot_defaults_to_float_and_warns() {
    // A self-contained prefab block runs the same two-phase inference the
    // compile path uses: an unannotated custom-event slot with no in-block
    // sender defaults to float (WS042) and its typed use surfaces WS003 —
    // in the editor / `just check`, not just at emit time. Both remap into
    // the block's outer span.
    let src = "in go: exec\non go { SpawnPrefab($```on CustomEvent(\"init\") -> p { let n = p.GetUserId() }```) }\n";
    let r = tc(src);
    let block = src.find("$```").unwrap();
    let ws042 = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS042")
        .expect("WS042 for the uninferable slot should surface");
    assert!(
        ws042.range.start.offset >= block,
        "WS042 should remap into the block span (offset {} vs block {})",
        ws042.range.start.offset,
        block
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS003" && d.range.start.offset >= block),
        "the float slot's typed use should surface WS003 inside the block: {:?}",
        r.diagnostics
    );
}

#[test]
fn find_player_returns_controller_not_character() {
    // FindPlayer emits the found player's persistent PlayerState (modeled as
    // `controller`), NOT a character — its output was mistyped `Type::Character`
    // before the PlayerState upgrade. A minimal program's only object-typed
    // expression is the call, so its inferred type must be Controller.
    let r = tc("in go: exec\non go { let p = FindPlayer(\"id\") }");
    assert_no_diags(&r);
    assert!(
        r.type_of_expr.values().any(|t| *t == Type::Controller),
        "FindPlayer must type as controller/PlayerState: {:?}",
        r.type_of_expr.values().collect::<Vec<_>>()
    );
    assert!(
        !r.type_of_expr.values().any(|t| *t == Type::Character),
        "FindPlayer must not type as character: {:?}",
        r.type_of_expr.values().collect::<Vec<_>>()
    );
}

#[test]
fn array_record_field_is_never_ref_wrapped() {
    // An array is already a reference, so a shorthand `{ arr }` field (and
    // `&arr`) types as `int[]`, never the redundant, write-dropping `*int[]`.
    // A mixed record built from vars matches a `{ int[], *int }` type
    // EXACTLY (plain array field, ref scalar field) — no coercion.
    let ok = "var regs: int[]\nvar cpsr: int = 0\n\
                  type Cpu = { regs: int[], cpsr: *int }\n\
                  mod f(c: Cpu) { }\n\
                  on RoundStart() {\n  let cpu = { regs, cpsr }\n  f(cpu)\n}\n";
    let r = tc(ok);
    assert!(
        !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "shorthand `int[]` + scalar `*int` must match `Cpu` exactly: {:?}",
        r.diagnostics
    );

    // A genuine field mismatch renders the array field as `int[]`, not `*int[]`.
    let bad = "var regs: int[]\n\
                   type Bad = { regs: string }\n\
                   mod g(b: Bad) { }\n\
                   on RoundStart() {\n  let rr = { regs }\n  g(rr)\n}\n";
    let rb = tc(bad);
    let ws003 = rb
        .diagnostics
        .iter()
        .find(|d| d.code == "WS003")
        .expect("field mismatch should be WS003");
    assert!(
        ws003.message.contains("regs: int[]") && !ws003.message.contains("*int[]"),
        "array field should render as `int[]`, never `*int[]`: {}",
        ws003.message
    );
}

/// A namespaced call `ns.f(args)` has its arguments' types and arity
/// checked like a local one.
#[test]
fn namespace_calls_are_checked() {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: [
            ("lib.ws", "mod foo(v: *int) { v = v + 1 }"),
            ("util.ws", "mod f(v: vector) { }"),
            (
                "gutil.ws",
                "mod maxT<T: Numeric>(a: T, b: T) -> T { return a }",
            ),
        ]
        .into_iter()
        .map(|(name, src)| (name.to_string(), src.to_string()))
        .collect(),
    };
    let rows: &[(&str, &str, &[Want])] = &[
        (
            "a namespace import registers its symbols",
            "import * as lib from \"lib\"",
            &[NoErrors],
        ),
        (
            "a namespaced call arg type mismatch",
            "import * as u from \"util\"\non RoundStart() { u.f(5) }",
            &[Has("WS003")],
        ),
        // A param named like the alias shadows the namespace, so `u.f(..)`
        // is WS002.
        (
            "a local shadowing the namespace",
            "import * as u from \"util\"\nmod g(u: int) { u.f(Vec(0.0, 0.0, 0.0)) }\non RoundStart() { g(1) }",
            &[Has("WS002")],
        ),
        (
            "the same call without the shadowing param",
            "import * as u from \"util\"\nmod g(v: int) { u.f(Vec(0.0, 0.0, 0.0)) }\non RoundStart() { g(1) }",
            &[Lacks("WS002")],
        ),
        // Namespaced user mods use the local positional-arity check (WS022),
        // not the builtin WS011 path.
        (
            "a namespaced call arity mismatch",
            "import * as u from \"util\"\non RoundStart() { u.f() }",
            &[Has("WS022")],
        ),
        (
            "a valid namespaced call",
            "import * as u from \"util\"\non RoundStart() { u.f(Vec(0.0, 0.0, 0.0)) }",
            &[NoErrors],
        ),
        // Import resolves each member's param types eagerly, so a generic
        // member's own type params must be in scope or the bare import emits
        // WS002 "unknown type 'T'".
        (
            "an uncalled generic member",
            "import * as g from \"gutil\"\non RoundStart() { }",
            &[Lacks("WS002"), NoErrors],
        ),
        (
            "a namespaced no-output mod used as a value",
            "import * as u from \"util\"\nvar o: int = 0\non RoundStart() { o = u.f(Vec(0.0, 0.0, 0.0)) }",
            &[HasMsg("WS072", "no value")],
        ),
    ];
    check_rows_with(rows, |src| {
        let resolved = resolve(src, "main.ws", &loader);
        Ok(typecheck(&resolved.ast, "main.ws", &CeSlotMap::default()))
    });
}

#[test]
fn array_length_returns_int() {
    let src = "var items: int[]\nin trigger: exec\non trigger { let len = items.length()\nlet ok = len + 1 }";
    let r = tc(src);
    assert_no_diags(&r);
    // `len + 1` would also resolve were `length()` `any`, so check the
    // call's own type.
    let call = "items.length()";
    let at = src.find(call).unwrap();
    assert_eq!(
        r.type_of_expr
            .get(&(std::sync::Arc::from("test"), at, at + call.len())),
        Some(&Type::Int),
        "length() should infer as Int"
    );
}

/// What storage, exec signals, triggers and annotated buffers accept.
#[test]
fn storage_signals_and_triggers_are_checked() {
    const OK: &[Want] = &[NoErrors];
    let buf = "var n: int = 0\nvar x: float = 0.0\nin go: exec\nin v: vector\nin k: int\n";
    let bad_buf: &[Want] = &[Has("WS003")];
    let good_buf: &[Want] = &[Lacks("WS003"), NoErrors];
    let rows: &[(&str, String, &[Want])] = &[
        // A var holds a `WireValue` already, and an `any` array or map stores
        // `WireValue` elements (the game's Any Value).
        ("an `any` var", "var foo: any = 0".into(), OK),
        ("a static `any` var", "static var foo: any = 0".into(), OK),
        (
            "an `any` array",
            "var arr: any[] = [1, \"two\", 3.5]".into(),
            OK,
        ),
        ("an `any` buffer", "buffer buf: any = 0".into(), OK),
        (
            "a handler-local `any` var",
            "in t: exec\non t { var foo: any = 0 }".into(),
            OK,
        ),
        ("an `any`-valued map", "var m: Map<string, any>".into(), OK),
        (
            "an `any` map key",
            "var m: Map<any, int>".into(),
            &[Has("WS039")],
        ),
        // A map key is an int, a string or an object reference.
        ("an int map key", "var d: Map<int, int>\n".into(), OK),
        ("a string map key", "var d: Map<string, int>\n".into(), OK),
        ("an entity map key", "var d: Map<entity, int>\n".into(), OK),
        (
            "a character map key",
            "var d: Map<character, int>\n".into(),
            OK,
        ),
        (
            "a float map key",
            "var d: Map<float, int>\n".into(),
            &[Error("WS039")],
        ),
        (
            "a bool map key",
            "var d: Map<bool, int>\n".into(),
            &[Error("WS039")],
        ),
        (
            "a vector map key",
            "var d: Map<vector, int>\n".into(),
            &[Error("WS039")],
        ),
        // The placeholder type is `Type::Any`, never `Type::Opaque`.
        (
            "an unannotated var",
            "var foo = 0".into(),
            &[Lacks("WS025")],
        ),
        // A storage gate holds one wire variant; these reach it as whatever
        // variant it defaults to.
        (
            "an exec var",
            "in go: exec\nvar c: exec\non go { }\n".into(),
            &[HasMsg("WS025", "control flow")],
        ),
        (
            "an exec array",
            "in go: exec\nvar c: exec[]\non go { }\n".into(),
            &[HasMsg("WS025", "control flow")],
        ),
        (
            "a never var",
            "in go: exec\nvar c: never\non go { }\n".into(),
            &[HasMsg("WS025", "cannot be stored")],
        ),
        (
            "an int var",
            "in go: exec\nvar c: int\non go { }\n".into(),
            &[Lacks("WS025")],
        ),
        (
            "a float array",
            "in go: exec\nvar c: float[]\non go { }\n".into(),
            &[Lacks("WS025")],
        ),
        (
            "a map",
            "in go: exec\nvar c: Map<int, string>\non go { }\n".into(),
            &[Lacks("WS025")],
        ),
        (
            "a vector var",
            "in go: exec\nvar c: vector\non go { }\n".into(),
            &[Lacks("WS025")],
        ),
        (
            "a nested array",
            "in go: exec\nvar c: int[][]\non go { }\n".into(),
            &[Lacks("WS025")],
        ),
        (
            "a map of arrays",
            "in go: exec\nvar c: Map<int, int[]>\non go { }\n".into(),
            &[Lacks("WS025")],
        ),
        // `let s: exec` is a new signal fired by `emit s`; its initializer was
        // dropped, and with it every `on s` handler's trigger.
        (
            "an initializer on a top-level exec signal",
            "var n: int = 0\nin go: exec\nlet s: exec = go\non s { n = 2 }".into(),
            &[Has("WS101")],
        ),
        (
            "an initializer on a handler-local exec signal",
            "var n: int = 0\nin go: exec\non go {\n let s: exec = emit\n n = 1\n}".into(),
            &[Has("WS101")],
        ),
        (
            "a new exec signal fired by emit",
            "var n: int = 0\nin go: exec\nlet s: exec\non s { n = 2 }\non go { emit s }".into(),
            OK,
        ),
        (
            "an unannotated alias of an exec input",
            "var n: int = 0\nin go: exec\nlet s = go\non s { n = 2 }".into(),
            OK,
        ),
        // `||` with an object is one value true while the object exists, so
        // the handler fires once. A warning: the code still means something.
        (
            "an object in a trigger `||`",
            "in reset: bool\nvar n: int = 0\non reset || ReadBrickGrid() { n = 0 }".into(),
            &[Warning("WS116")],
        ),
        (
            "bools in a trigger `||`",
            "in a: bool\nin b: bool\nvar n: int = 0\non a || b { n = 0 }".into(),
            &[Lacks("WS116")],
        ),
        (
            "exec pulses in a trigger `||`",
            "in a: exec\nin b: exec\nvar n: int = 0\non a || b { n = 0 }".into(),
            &[Lacks("WS116")],
        ),
        (
            "an object in `Union`",
            "in reset: bool\nvar n: int = 0\non Union(reset, ReadBrickGrid()) { n = 0 }".into(),
            &[Lacks("WS116")],
        ),
        (
            "an object guarded by `&&`",
            "in who: character\nin ok: bool\nvar n: int = 0\non who && ok { n = 0 }".into(),
            &[Lacks("WS116")],
        ),
        // A port cannot take a type parameter, so the WS032 advice has nothing
        // to follow; a mod parameter can be generic.
        (
            "`any` on a port",
            "in probe: any\nout seen: any = probe".into(),
            &[Lacks("WS032")],
        ),
        (
            "`any` on a mod parameter",
            "mod pass(x: any) -> any { return x }".into(),
            &[Has("WS032")],
        ),
        // An annotated buffer's initializer was never checked: a string in an
        // `int` buffer, or a vector in a `float` one, compiled.
        (
            "a string into an int buffer",
            format!("{buf}on go {{ buffer bb: int = \"abc\"\n n = bb }}"),
            bad_buf,
        ),
        (
            "a vector into a float buffer",
            format!("{buf}on go {{ buffer bb: float = v\n x = bb }}"),
            bad_buf,
        ),
        (
            "an int into a string buffer",
            format!("{buf}buffer bb: string = 5\non go {{ n = bb.Length() }}"),
            good_buf,
        ),
        (
            "an int into a float buffer",
            format!("{buf}on go {{ buffer bb: float = 5\n x = bb / 2 }}"),
            good_buf,
        ),
        (
            "an int expression into a float buffer",
            format!("{buf}on go {{ buffer bb: float = k + 1\n x = bb / 2 }}"),
            good_buf,
        ),
        (
            "an int into a bool buffer",
            format!("{buf}on go {{ buffer bb: bool = k\n n = bb }}"),
            good_buf,
        ),
    ];
    check_rows(rows);
}

/// String -> bool is a native coercion (lowering inserts a `!= ""` compare),
/// and it never chains into numerics: every consumer of `coerce()` applies
/// exactly ONE rule, and operator resolution never consults coercions.
#[test]
fn string_and_bool_coercions() {
    let rows: &[(&str, &str, &[Want])] = &[
        // No bool-condition check exists for `if`; lowering inserts the
        // `CompareNotEqual(s, "")` gate in front of the Branch.
        (
            "a string `if` condition",
            "in s: string\nin t: exec\nvar a: int = 0\non t { if s { a = 1 } }",
            &[NoErrors],
        ),
        // A string coerces to bool (`s != ""`).
        (
            "`let b: bool = s`",
            "in s: string\nlet b: bool = s",
            &[Silent],
        ),
        (
            "a string assigned into a bool var",
            "in s: string\nin t: exec\nvar v: bool = false\non t { v = s }",
            &[NoErrors],
        ),
        (
            "`let n: int = s` still warns",
            "in s: string\nlet n: int = s",
            &[Has("WS016")],
        ),
        (
            "a string assigned into an int var",
            "in s: string\nin t: exec\nvar n: int = 0\non t { n = s }",
            &[Error("WS003")],
        ),
        // The math gates have no string operand rules (`..` concatenates).
        ("`s + 1`", "in s: string\nlet a = s + 1", &[Has("WS004")]),
        ("`\"a\" + 1`", "let a = \"a\" + 1", &[Has("WS004")]),
        // Arg checking does one coerce(String, Int) = Mismatch, with no bool hop.
        (
            "a string into an int builtin param",
            "in s: string\nlet c = ColorSRGB(s, 0, 0, 255)",
            &[HasMsg("WS003", "expected int")],
        ),
        (
            "bool -> int, annotated and assigned",
            "in f: bool\nlet n: int = f\nin t: exec\nvar m: int = 0\non t { m = f }",
            &[NoErrors],
        ),
        (
            "`out y: bool = s`",
            "in s: string\nout y: bool = s",
            &[NoErrors],
        ),
        // Annotated outs never checked their value, so this emitted a mistyped pin.
        (
            "`out y: int = s`",
            "in s: string\nout y: int = s",
            &[Error("WS003")],
        ),
        // int -> string is `ViaString` (a format gate), not a mismatch.
        (
            "`out label: string = n`",
            "in n: int\nout label: string = n",
            &[NoErrors],
        ),
        // A `*int` port carries a ref: `&x` is x's ref; a bare `x` is its value.
        (
            "a ref out given a ref",
            "var x: int = 0\nout y: *int = &x",
            &[NoErrors],
        ),
        (
            "a ref out given a var's value",
            "var x: int = 0\nout y: *int = x",
            &[Has("WS003")],
        ),
        (
            "a mismatched statement `out` annotation",
            "chip { out x: int = \"s\" }",
            &[Has("WS003")],
        ),
        (
            "a matching statement `out` annotation",
            "chip { out x: int = 5 }",
            &[NoErrors],
        ),
        (
            "a statement `out` formatting an int as string",
            "chip { out s: string = 5 }",
            &[NoErrors],
        ),
    ];
    check_rows(rows);
}

#[test]
fn a_string_method_on_a_value_that_is_not_a_string_is_ws003() {
    // `vv.Length()` on a vector read the length of the text the vector
    // formats to; each string method on each value that is not a string
    // is WS003, with the right spelling for a length.
    let head = "var vv: vector = Vec(3.0, 4.0, 0.0)\nvar n: int = 3\nvar fl: float = 1.5\n\
                    var b: bool = true\nvar cl: color = Color(1.0, 0.0, 0.0)\nvar rt: rotator = Rotation(0.0, 0.0, 0.0)\n\
                    var xs: int[] = [1]\nvar m: Map<string, int> = {}\nvar s: string = \"abc\"\n\
                    var r: int = 0\nvar rs: string = \"\"\nvar rb: bool = false\nin go: exec\nin who: entity\n";
    let string_methods: Vec<&str> = crate::catalog::calls::calls()
        .values()
        .filter(|c| c.receiver == Some(Type::String) && c.receiver_target_param().is_none())
        .map(|c| c.name)
        .collect();
    assert!(string_methods.contains(&"Length"), "{string_methods:?}");
    for recv in ["vv", "n", "fl", "b", "cl", "rt", "xs", "m", "who"] {
        for method in &string_methods {
            let r = tc(&format!("{head}on go {{ let q = {recv}.{method}() }}"));
            let d = r.diagnostics.iter().find(|d| d.code == "WS003");
            assert!(
                d.is_some_and(|d| d
                    .message
                    .contains(&format!("`.{method}()` is a string method"))),
                "`{recv}.{method}()`: {:?}",
                r.diagnostics
            );
        }
    }
    let hint = |src: &str| {
        let r = tc(&format!("{head}on go {{ {src} }}"));
        r.diagnostics
            .iter()
            .find(|d| d.code == "WS003")
            .map(|d| d.message.clone())
            .unwrap_or_default()
    };
    assert!(hint("r = vv.Length()").contains("`Magnitude(vv)`"));
    assert!(hint("r = xs.Length()").contains("`xs.length()`"));
    assert!(hint("r = m.Length()").contains("`m.length()`"));
    assert!(hint("rs = n.ToUpper()").contains("(`ToUpper(n)`)"));
    // A literal receiver is spelled as written, with the article its type
    // takes.
    let lit = hint("r = 5.Length()");
    assert!(
        lit.contains("and `5` is an `int`") && lit.contains("(`Length(5)`)"),
        "{lit}"
    );
    // The receiver's own errors are reported, once.
    for src in [
        "r = (nosuch).Length() - 1",
        "r = (if b then nosuch else 2).Length()",
        "r = nosuch.Length()",
    ] {
        let r = tc(&format!("{head}on go {{ {src} }}"));
        let unknown = r.diagnostics.iter().filter(|d| d.code == "WS002").count();
        assert_eq!(unknown, 1, "`{src}`: {:?}", r.diagnostics);
    }
    assert!(hint("r = true.Length()").contains("and `true` is a `bool`"));
    // A string receiver, and the argument form, still work.
    let ok = tc(&format!(
        "{head}on go {{ r = s.Length()\n r = Length(vv) }}"
    ));
    assert!(
        ok.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "{:?}",
        ok.diagnostics
    );
}

#[test]
fn a_method_on_an_unbounded_type_parameter_names_the_parameter() {
    // The body is checked for every type `T` may be, so the message says
    // `T` is unbounded and which bound to add, through a `let` too, and for
    // a `self` mod as for a string method. A bounded `T` compiles.
    let head = "var n: int = 0\nvar s: string = \"abc\"\nvar f2: float = 0.0\n\
                    var vv: vector = Vec(1.0, 2.0, 3.0)\nin go: exec\n";
    for (body, want) in [
        (
            "mod g<T>(v: T) -> int { return v.Length() }\non go { n = g(s) }",
            "`T` is unbounded, so `v` may be a `bool`; bound it: `<T: string>`",
        ),
        (
            "mod g<T>(v: T) -> int { let w = v\n return w.Length() }\non go { n = g(s) }",
            "so `w` may be a `bool`",
        ),
        (
            "mod g<U>(v: U) -> bool { return v.StartsWith(\"a\") }\non go { let q = g(s) }",
            "`U` is unbounded",
        ),
        (
            "mod Length(self: vector) -> float { return 777.0 }\nmod g<T>(v: T) -> float { return v.Length() }\non go { f2 = g(vv) }",
            "`.Length()` calls `Length(self: vector)`, and `T` is unbounded, so `v` may be a `bool`; bound it: `<T: string | vector>`",
        ),
        (
            "mod g<T>(v: T) -> int { let w: T = v\n return w.Length() }\non go { n = g(s) }",
            "so `w` may be a `bool`",
        ),
        // A receiver that reads an element or a field of a value declared
        // with `T`, or a name taken from one, is `T` too.
        (
            "mod g<T>(v: T[]) -> int { return v[0].Length() }\non go { n = g([s]) }",
            "`T` is unbounded, so `v[0]` may be a `bool`; bound it: `<T: string>`",
        ),
        (
            "mod g<T>(v: T[]) -> int { var t: int = 0\n for x in v { t = t + x.Length() }\n return t }\non go { n = g([s]) }",
            "`T` is unbounded, so `x` may be a `bool`; bound it: `<T: string>`",
        ),
        (
            "type B<T> = { v: T }\nmod g<T>(b: B<T>) -> int { return b.v.Length() }\non go { n = g({ v: s }) }",
            "`T` is unbounded, so `b.v` may be a `bool`; bound it: `<T: string>`",
        ),
        (
            "mod g<T>(v: T[]) -> int { let w = v[0]\n return w.Length() }\non go { n = g([s]) }",
            "so `w` may be a `bool`",
        ),
        // A call's result bound to `T`, and an element read with a
        // string key, which is spelled.
        (
            "mod id<A>(v: A) -> A { return v }\nmod g<T>(v: T) -> int { return id(v).Length() }\non go { n = g(s) }",
            "`T` is unbounded, so the receiver may be a `bool`; bound it: `<T: string>`",
        ),
        (
            "mod g<T>(m: Map<string, T>) -> int { return m[\"a\"].Length() }\nvar ms: Map<string, string> = {}\non go { n = g(ms) }",
            "`T` is unbounded, so `m[\"a\"]` may be a `bool`; bound it: `<T: string>`",
        ),
    ] {
        let r = tc(&format!("{head}{body}"));
        let d = r.diagnostics.iter().find(|d| d.code == "WS003");
        assert!(
            d.is_some_and(|d| d.message.contains(want)),
            "`{body}`: {:?}",
            r.diagnostics
        );
        // The bound it suggests compiles.
        let bounded = format!(
            "{head}{}",
            body.replacen("<T>(", "<T: string>(", 1)
                .replacen("<U>(", "<U: string>(", 1)
        );
        if !body.contains("self: vector") {
            let ok = tc(&bounded);
            assert!(
                ok.diagnostics.iter().all(|d| d.severity != Severity::Error),
                "`{bounded}`: {:?}",
                ok.diagnostics
            );
        }
    }
    let ok = tc(&format!(
        "{head}mod g<T: string>(v: T) -> int {{ return v.Length() }}\non go {{ n = g(s) }}"
    ));
    assert!(
        ok.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "{:?}",
        ok.diagnostics
    );
    // The bound the `self` mod form suggests admits a string too, so the
    // program's own string call still compiles.
    let both = "mod Length(self: vector) -> float { return 777.0 }\n\
                    mod g<T: string | vector>(v: T) -> float { return v.Length() }\non go { f2 = g(vv)\n f2 = g(s) }";
    let ok = tc(&format!("{head}{both}"));
    assert!(
        ok.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "{:?}",
        ok.diagnostics
    );
    // A non-generic receiver keeps naming its own type, also beside a
    // type parameter bound to the same type.
    for body in [
        "mod g(v: bool) -> int { return v.Length() }",
        "mod g<T>(v: T, b: bool) -> int { return b.Length() }\non go { n = g(s, true) }",
    ] {
        let r = tc(&format!("{head}{body}"));
        let d = r.diagnostics.iter().find(|d| d.code == "WS003").unwrap();
        assert!(
            !d.message.contains("unbounded") && d.message.contains("is a `bool`"),
            "{}",
            d.message
        );
    }
}

#[test]
fn ws_source_prefab_ref_surfaces_inner_errors_on_span() {
    // `$./child.ws` is a SOURCE prefab: the type-checker reads + checks the
    // referenced file and surfaces its diagnostics on the reference span, so
    // a broken child underlines `$./child.ws` in the parent (not a silent
    // failure discovered only at emit).
    // The check reads the child from disk, so it gets a directory of its own
    // beside the test binary, removed again on drop.
    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let exe = std::env::current_exe().expect("test binary path");
    let dir = Dir(exe.parent().expect("test binary directory").join(format!(
        "ws_source_prefab_ref_surfaces_inner_errors_on_span_{}",
        std::process::id()
    )));
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(
        dir.0.join("broken_child.ws"),
        "in go: exec\non go { let x = undefinedThing + 1 }",
    )
    .unwrap();
    let parent_path = dir.0.join("parent.ws").to_string_lossy().into_owned();
    let src = "in go: exec\non go { SpawnPrefab(prefab = $./broken_child.ws) }";
    let p = parse(src, &parent_path);
    let result = crate::typecheck::typecheck_with_inference(&p.ast, &parent_path).0;
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == "WS002" && d.message.contains("in prefab")),
        "a broken $./child.ws must surface its WS002 on the reference span; got {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_ce_slots_adopts_sender_type_and_warns_when_absent() {
    // A receiver with unannotated `amount`; an in-unit send of an int -> amount:int.
    let src = "static var last: int = 0\n\
                   in go: exec\n\
                   on CustomEvent(\"dmg\") -> (amount) {\n  last = last + 1\n}\n\
                   on go {\n  SendCustomEvent(\"dmg\", 42)\n}\n";
    let p = parse(src, "test");
    assert!(p.diagnostics.is_empty(), "parse: {:?}", p.diagnostics);
    let pass1 = typecheck(&p.ast, "test", &CeSlotMap::default());
    let (map, diags) = infer_custom_event_slots(&p.ast, &pass1.type_of_expr);
    // The single receiver's slot 0 resolved to int.
    let slots = map.values().next().expect("one custom-event receiver");
    assert_eq!(slots.get(0).cloned().flatten(), Some(Type::Int));
    assert!(
        diags.is_empty(),
        "no WS042 when a sender exists: {:?}",
        diags
    );

    // No in-unit sender -> float + WS042.
    let src2 = "static var last: int = 0\n\
                    on CustomEvent(\"ext\") -> (amount) {\n  last = last + 1\n}\n";
    let p2 = parse(src2, "test");
    let pass1b = typecheck(&p2.ast, "test", &CeSlotMap::default());
    let (map2, diags2) = infer_custom_event_slots(&p2.ast, &pass1b.type_of_expr);
    let slots2 = map2.values().next().expect("one receiver");
    assert_eq!(slots2.get(0).cloned().flatten(), Some(Type::Float));
    crate::test_support::assert_code(&diags2, "WS042");
}

#[test]
fn inferred_ce_slot_type_is_enforced_in_body() {
    // `amount` inferred as int from the sender; using it where a vector is
    // required is a WS003 (type error) — proving the body sees `int`, not
    // `any` (which coerces to everything, including vector — `int` doesn't:
    // numeric-to-vector has no coercion rule, see `types::coerce::coerce`).
    let src = "static var v: vector = Vec(0.0, 0.0, 0.0)\n\
                   in go: exec\n\
                   on CustomEvent(\"dmg\") -> (amount) {\n  v = amount\n}\n\
                   on go {\n  SendCustomEvent(\"dmg\", 42)\n}\n";
    let p = parse(src, "test");
    let (r, _map) = typecheck_with_inference(&p.ast, "test");
    crate::test_support::assert_code(&r.diagnostics, "WS003");
}

/// `return`, `emit` and an unannotated `out` are checked against the output
/// they drive.
#[test]
fn writes_are_checked_against_the_declared_output() {
    let flag = "out o: int\nin go: exec\nvar flag: bool = false\n";
    let rows: &[(&str, String, &[Want])] = &[
            ("a mismatched return", "mod f() -> (r: int) { return \"hello\" }\nout y = f()".into(), &[Has("WS003")]),
            ("a matching return", "mod f() -> (r: int) { return 7 }\nout y = f()".into(), &[NoErrors]),
            ("a mismatched emit payload", "out o: int\nin go: exec\non go { emit o = \"nope\" }".into(), &[Has("WS003")]),
            ("a matching emit payload", "out o: int\nin go: exec\non go { emit o = 7 }".into(), &[NoErrors]),
            // A local exec signal is not an output.
            ("an emit into a local exec signal", "in go: exec\non go { let s: exec\n emit s = 7 }".into(), &[NoErrors]),
            ("a mismatched unannotated out in a chip", "chip Foo() -> (r: int) { out r = \"s\" }".into(), &[Has("WS003")]),
            ("a matching unannotated out in a chip", "chip Foo() -> (r: int) { out r = 5 }".into(), &[NoErrors]),
            // A value return in a top-level handler drove the one module output
            // from load time, or was dropped with several: neither is what it says.
            ("a string return in a top-level handler", format!("{flag}on go {{ if flag {{ return \"bail\" }} }}"), &[Has("WS099")]),
            ("an int return in a top-level handler", format!("{flag}on go {{ if flag {{ return 3 }} }}"), &[Has("WS099")]),
            ("a return beside two module outputs", "out o: int\nout y = 5\nin go: exec\nvar flag: bool = false\non go { if flag { return 3 } }".into(), &[Has("WS099")]),
            ("a return beside an unannotated output", "in go: exec\nvar n: int = 0\non go { n = 2\n return 1 }\nout o = n.Value".into(), &[Has("WS099")]),
            ("a bare return in a top-level handler", format!("{flag}on go {{ if flag {{ return }} }}"), &[NoErrors]),
            ("a value return in a chip's handler", "in go: exec\nchip C(t: exec) -> (r: int) { on t { return 4 } }\nvar n: int = 0\non go { n = C(go).r }".into(), &[NoErrors]),
            // A same-named local does NOT shadow the output: `lookup_output` is a
            // disjoint namespace, so the wrong value still wires into the output.
            ("an out to an output shadowed by a local", "chip Foo() -> (r: int) { var r: int = 0\n out r = \"bad\" }".into(), &[Has("WS003")]),
            ("`out r = r` with a same-named local", "chip Foo() -> (r: int) { var r: int = 0\n out r = r }".into(), &[NoErrors]),
            ("an emit to an output shadowed by a local", "out o: int\nin go: exec\non go { let o: exec\n emit o = go }".into(), &[Has("WS003")]),
            // The module `out_ctx` frame build resolves the annotation the
            // `TopDecl::Out` check already reports, so it discards its own.
            ("a bad module output type reports once", "out o: BadType = 0".into(), &[Count("WS002", 1)]),
            // The per-combo chip output frame likewise defers to `register_decl`.
            ("a bad chip output type reports once", "chip Foo() -> (r: BadType) {\n out r = 0\n}\nlet z = Foo()".into(), &[Count("WS002", 1)]),
        ];
    check_rows(rows);
}

/// `const` is a guarantee: an unevaluable `const` binding or parameter is
/// WS046 while a `let` stays opportunistic, and only a name declared `const`
/// decides an `if` (dropping its untaken block). Each destructured form is
/// proven through a consumer of const-ness, never by an absence of errors.
#[test]
fn const_bindings_and_branch_elision() {
    let send = "in go: exec\non go { f() }";
    let rows: &[(&str, String, &[Want])] = &[
            ("a const shift in a mod body", "mod f() { const n = 1 << 4 }".into(), &[NoErrors]),
            ("a top-level const product", "const TOP = 2 * 21".into(), &[NoErrors]),
            ("a const from a runtime value names it", "in live: int\nmod f() { const n = live + 1 }".into(), &[FirstMsg("WS046", "live")]),
            ("a let that cannot fold is fine", "in live: int\nmod f() { let n = live + 1 }".into(), &[NoErrors]),
            // `someRecord` is undefined: `ConstReason::NotConstant`.
            ("a destructure of a runtime value", "mod f() { const { a, b } = someRecord }".into(), &[Has("WS046")]),
            // Swapped bindings would flip both conditions, keep the ill-typed
            // `else` blocks, and fire WS003.
            ("a top-level const destructure by name", "const p = { x: 111, y: 222 }\nconst { x, y } = p\nvar flag: int = 0\nin go: exec\non go {\nif x == 111 { flag = 1 } else { flag = \"wrong x\" }\nif y == 222 { flag = 2 } else { flag = \"wrong y\" }\n}".into(), &[NoErrors, Dropped(2)]),
            ("a top-level const destructure with an alias", "const p = { x: 111, y: 222 }\nconst { x: a, y: b } = p\nvar flag: int = 0\nin go: exec\non go {\nif a == 111 { flag = 1 } else { flag = \"wrong a\" }\nif b == 222 { flag = 2 } else { flag = \"wrong b\" }\n}".into(), &[NoErrors, Dropped(2)]),
            ("a top-level const destructure with a rest", "const p = { x: 111, y: 222, z: 333 }\nconst { x, ...rest } = p\nvar flag: int = 0\nin go: exec\non go {\nif x == 111 { flag = 1 } else { flag = \"wrong x\" }\nif rest.y == 222 { flag = 2 } else { flag = \"wrong rest.y\" }\nif rest.z == 333 { flag = 3 } else { flag = \"wrong rest.z\" }\n}".into(), &[NoErrors, Dropped(3)]),
            // `build_const_env` skipped non-`Ident` bindings, so `x` reached
            // neither it nor `build_const_declared_names`. A swap gives 222111.
            ("a later const reads destructured names", "const p = { x: 111, y: 222 }\nconst { x, y } = p\nconst combined = x * 1000 + y\nvar flag: int = 0\nin go: exec\non go { if combined == 111222 { flag = 1 } else { flag = \"wrong\" } }".into(), &[NoErrors, Dropped(1)]),
            ("destructured names as event config", "const cfg = { rate: 1.0, chan: \"evt_died\" }\nconst { rate, chan } = cfg\nvar n: int = 0\non Clock(interval = rate, enabled = true) { n = n + 1 }".into(), &[NoErrors]),
            ("a destructured name as a channel", "const cfg = { chan: \"evt_died\" }\nconst { chan } = cfg\nmod ping(v: int) { SendCustomEvent(chan, v) }".into(), &[NoErrors]),
            ("a block-scope destructure with an alias", format!("mod f() {{\nconst p = {{ x: 111, y: 222 }}\nconst {{ x: a, y: b }} = p\nconst combined = a * 1000 + b\nSendCustomEvent(\"evt\", combined)\n}}\n{send}"), &[NoErrors]),
            // Had the alias not rebound, `x` would still be in scope.
            ("an alias binds only the alias", format!("mod f() {{\nconst p = {{ x: 111, y: 222 }}\nconst {{ x: a, y: b }} = p\nconst bad = x + a + b\n}}\n{send}"), &[HasMsg("WS002", "'x'")]),
            ("a block-scope destructure with a rest", format!("mod f() {{\nconst p = {{ x: 111, y: 222, z: 333 }}\nconst {{ x, ...rest }} = p\nconst combined = x * 1000000 + rest.y * 1000 + rest.z\nSendCustomEvent(\"evt\", combined)\n}}\n{send}"), &[NoErrors]),
            ("a rest excludes the consumed field", format!("mod f() {{\nconst p = {{ x: 111, y: 222 }}\nconst {{ x, ...rest }} = p\nconst bad = rest.x\n}}\n{send}"), &[HasMsg("WS046", "no field named 'x'")]),
            // `typecheck::stmt`'s block-scope site in a `const mod` body, and
            // `const_eval::interp`'s own site through the top-level call.
            ("a destructure in a const mod body", "const mod f() -> int {\nconst p = { x: 3, y: 4 }\nconst { x, y } = p\nreturn x * 100 + y\n}\nconst total = f()".into(), &[NoErrors]),
            ("a destructure naming a missing field", "const p = { x: 1 }\nconst { x, missing } = p".into(), &[FirstMsg("WS046", "missing")]),
            ("a destructure of a non-record", "const { x } = 1".into(), &[Has("WS046")]),
            // stmt.rs's site without the const-mod machinery.
            ("a block-scope destructure in an ordinary mod body", format!("mod f() {{\nconst p = {{ x: 111, y: 222 }}\nconst {{ x, y }} = p\nconst combined = x * 1000 + y\nSendCustomEvent(\"evt\", combined)\n}}\n{send}"), &[NoErrors]),
            // A plain `let`'s destructured names are not constants: recording them
            // as such let lowering's narrow evaluator drop the value and ship a
            // `SendCustomEvent` with an EMPTY channel name. The `const` spellings
            // are asserted to bake in `lower::tests::const_params`.
            ("a non-const destructure in a mod body", "mod f(v: int) {\nlet { chan } = { chan: \"evt\" }\nSendCustomEvent(chan, v)\n}\nin go: exec\non go { f(1) }".into(), &[Has("WS028")]),
            ("a non-const destructure, alias form", "mod f(v: int) {\nlet { chan: c2 } = { chan: \"evt\" }\nSendCustomEvent(c2, v)\n}\nin go: exec\non go { f(1) }".into(), &[Has("WS028")]),
            ("a non-const destructure in a handler", "in go: exec\nvar hp: int = 0\non go {\nlet { chan } = { chan: \"evt\" }\nSendCustomEvent(chan, hp)\n}".into(), &[Has("WS028")]),
            ("a non-const destructure, rest form", "mod f(v: int) {\nlet { other, ...rest } = { other: 1, chan: \"evt\" }\nSendCustomEvent(rest.chan, v)\n}\nin go: exec\non go { f(1) }".into(), &[Has("WS028")]),
            // One named output types as a bare `string` (`typecheck::call`), so it
            // is scalar config; wrapping it in a record gave a contradictory WS028.
            ("a single-named-output const mod is scalar config", "const mod chan(p: const string) -> (r: string) { out r = p .. \"_evt\" }\nconst NAME = chan(\"hit\")\nvar hp: int = 0\nin go: exec\non go { SendCustomEvent(NAME, hp) }".into(), &[Lacks("WS028"), NoErrors]),
            ("a multi-output const mod is a record", "const mod chan(p: const string) -> (r: string, extra: int) { out r = p\n out extra = 1 }\nconst NAME = chan(\"hit\")\nvar hp: int = 0\nin go: exec\non go { SendCustomEvent(NAME, hp) }".into(), &[Has("WS028")]),
            ("a non-const destructured name into a const param", "mod send(name: const string, v: int) { SendCustomEvent(name, v) }\nmod f(v: int) {\nlet { chan } = { chan: \"evt\" }\nsend(chan, v)\n}\nin go: exec\non go { f(1) }".into(), &[Has("WS046")]),
            // Only the untaken block is skipped, never both.
            ("the kept branch is still checked", "var flag: int = 0\nmod f() {\nconst p = { x: 111, y: 222 }\nconst { x, y } = p\nif x == 111 { flag = \"ill typed, and this branch SHIPS\" } else { flag = 1 }\n}\nin go: exec\non go { f() }".into(), &[Has("WS003")]),
            // A field destructured from a const param's placeholder is as
            // fictional as the placeholder.
            ("a placeholder-derived destructure decides no branch", "var x: int = 0\nmod f(m: const int) {\nconst rec = { a: m }\nconst { a } = rec\nif a == 1 { x = \"wrong\" } else { x = 2 }\n}\nin go: exec\non go { f(1) }".into(), &[Has("WS003"), Dropped(0)]),
            ("a literal into a const param", "mod g(name: const string, v: int) { }\nmod caller(x: int) { g(\"lit\", x) }".into(), &[NoErrors]),
            ("a runtime value into a const param", "in live: string\nmod g(name: const string) { }\nmod caller() { g(live) }".into(), &[Has("WS046")]),
            ("a const param still type-checks its argument", "mod g(n: const int) { }\nmod caller() { g(\"not an int\") }".into(), &[Has("WS003")]),
            ("a named constant into a const param", "const N = 4\nmod g(n: const int) { }\nmod caller() { g(N * 2) }".into(), &[NoErrors]),
            // The receiver side used the environment-free `expr_to_literal`, so a
            // computed channel could be sent but never received.
            ("a named constant as a handler channel", "const CH = \"evt_\" .. \"died\"\nvar n: int = 0\non CustomEvent(CH) -> (v: int) { n = v }".into(), &[NoErrors]),
            ("a computed constant as a handler channel", "const PREFIX = \"evt_\"\nvar n: int = 0\non CustomEvent(PREFIX .. \"died\") -> (v: int) { n = v }".into(), &[NoErrors]),
            // A bare identifier is an enum MEMBER first, a constant only otherwise.
            ("an enum member as enum config", "mod f(a: float, b: float, t: float) { Easing(a, b, t, function = Bounce) }".into(), &[NoErrors]),
            ("a constant as enum config", "const EASE = \"Bounce\"\nmod f(a: float, b: float, t: float) { Easing(a, b, t, function = EASE) }".into(), &[NoErrors]),
            ("a non-const callee keeps its message", "mod double(n: int) -> int { return n * 2 }\nmod f() { const total = double(3) + 1 }".into(), &[FirstMsg("WS046", "'double' is not declared `const mod`")]),
            ("a non-const mod in a const position", "mod evtName(kind: string) -> string { return \"evt_\" .. kind }\nmod ping(v: int) { SendCustomEvent(evtName(\"died\"), v) }".into(), &[Has("WS028")]),
            // The taken block lowers into the parent scope with no Branch gate, so
            // it still pushes and pops its own frame.
            ("a branch-local const does not leak past the `if`", "const A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { const b = 5\n x = b } else { x = 0 }\n x = b }".into(), &[Has("WS002")]),
            // A program with no `const` must compile as before `const` existed:
            // a plain `let` that folds keeps its Branch and both blocks are checked.
            ("a plain let condition keeps both blocks", "let A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { x = 1 } else { x = totallyUndefinedThing() } }".into(), &[Dropped(0), HasMsg("WS002", "totallyUndefinedThing")]),
            ("a const condition drops the untaken block", "const A = 1\nvar x: int = 0\nin go: exec\non go { if A == 1 { x = 1 } else { x = totallyUndefinedThing() } }".into(), &[NoErrors, Dropped(1)]),
            // Lowering keeps the Branch and emits both blocks under `@nofold`, so
            // the "untaken" one is real code that ships.
            ("@nofold keeps checking both blocks", "const MODE = 1\nvar x: int = 0\nin go: exec\n\n@nofold\non go { if MODE == 1 { x = 1 } else { x = \"definitely not an int\" } }".into(), &[Has("WS003"), Dropped(0)]),
            // A const param's placeholder is a zero seeded before any call site, so
            // `m == 1` reads false here while lowering, inlining the real 1, takes
            // the THEN; selecting from it would never check the branch that ships.
            ("a const param placeholder decides no branch", "var x: int = 0\nmod f(m: const int) { if m == 1 { x = \"wrong\" } else { x = 2 } }\nin go: exec\non go { f(1) }".into(), &[Has("WS003"), Dropped(0)]),
            ("a const derived from a placeholder decides no branch", "var x: int = 0\nmod f(m: const int) { const t = m + 1\n if t == 2 { x = \"wrong\" } else { x = 2 } }\nin go: exec\non go { f(1) }".into(), &[Has("WS003")]),
            ("a real constant still elides inside a mod body", "const MODE = 1\nvar x: int = 0\nmod f(n: int) { if MODE == 1 { x = n } else { x = totallyUndefinedThing() } }\nin go: exec\non go { f(1) }".into(), &[NoErrors]),
            // A constant collection has no single scalar to bake: an array baked a
            // wrong default and a record aborted the compile with `unreachable!`.
            ("a record constant in a named scalar config slot", "const cfg = { rooms: 2, timer: 60 }\nvar x: int = 0\non ChatCommand(\"greet\", description = cfg) { x = x + 1 }".into(), &[FirstMsg("WS028", "a record")]),
            ("a record constant in a positional config slot", "const cfg = { rooms: 2, timer: 60 }\nvar x: int = 0\non ChatCommand(cfg) { x = x + 1 }".into(), &[FirstMsg("WS028", "a record")]),
            ("an array constant in a scalar config slot", "const arr = [1, 2, 3]\nvar x: int = 0\non ChatCommand(\"greet\", description = arr) { x = x + 1 }".into(), &[FirstMsg("WS028", "an array")]),
            ("a scalar constant in a scalar config slot", "const T = \"help text\"\nvar x: int = 0\non ChatCommand(\"greet\", description = T) { x = x + 1 }".into(), &[NoErrors]),
        ];
    check_rows(rows);
}

// Wiring `lookup_mod` made `NotAConstMod` capable of lying: a `const mod`
// call that const evaluation does not descend to isn't evaluated, and the
// failure used to be reported as "'double' is not declared `const mod`"
// about a mod that plainly IS one. Leaving a position unevaluated is
// fine; stating something false about the user's code is not.
#[test]
fn a_nested_const_mod_call_is_not_blamed_for_being_non_const() {
    // `double(3) + 1` (this test's ORIGINAL trigger) is no longer nested
    // in the sense this test is about: `eval_expr` now resolves a `const
    // mod` call standing as a `BinOp`/`UnOp`/constructor-argument operand
    // directly (see `const_eval::expr`'s
    // `a_const_mod_call_nested_in_a_binary_operator_evaluates`), so that
    // expression just evaluates to `7` and never reaches this
    // diagnostic. Nest one level deeper instead — as an argument to a
    // call whose OWN callee (`scaleUp`) is an ordinary, non-const `mod` —
    // which `eval_expr`'s `Expr::Call` arm still declines to evaluate,
    // falling back to `reason_for`'s walk and landing on the exact
    // `NestedConstModCall` case this test exists to prove.
    let r = tc("const mod double(n: int) -> int { return n * 2 }\n\
             mod scaleUp(n: int) -> int { return n }\n\
             mod f() { const total = scaleUp(double(3)) }");
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS046")
        .unwrap_or_else(|| panic!("expected WS046, got {:?}", r.diagnostics));
    assert!(
        !d.message.contains("is not declared `const mod`"),
        "must not claim a `const mod` isn't one: {}",
        d.message
    );
    assert!(
        d.message.contains("is a `const mod`")
            && d.message.contains("surrounding expression")
            && d.message.contains("never reaches it"),
        "must name the real limitation (the enclosing call, not the callee): {}",
        d.message
    );
}

// The scoping is symmetric: once `A`'s body has finished checking (its
// `mod_decls` frame popped), a call AFTER it in a totally unrelated decl
// must not see `A`'s nested override either. Pins the exact mechanism —
// WHICH range gets dropped — rather than just "no error": under the
// flat-map bug `B` would resolve `size` to `A`'s leaked 999, so
// `size() == 10` reads false and the THEN block (not the ELSE) would be
// the one dropped.
#[test]
fn a_nested_const_mod_frame_is_dropped_once_its_scope_closes() {
    let src = "const mod size() -> int { return 10 }\n\
             var ok: int = 0\n\
             mod A() { const mod size() -> int { return 999 } }\n\
             mod B() { const s = size()\n if s == 10 { ok = 1 } else { ok = 2 } }";
    let r = tc(src);
    assert!(
        r.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "resolving the top-level `size` after A's scope has closed must not error: {:?}",
        r.diagnostics
    );
    assert_eq!(
        r.dropped_ranges.len(),
        1,
        "exactly one branch must be dropped: {:?}",
        r.dropped_ranges
    );
    let dropped_src = &src[r.dropped_ranges[0].0.start.offset..r.dropped_ranges[0].0.end.offset];
    assert!(
        dropped_src.contains("ok = 2"),
        "B must resolve the TOP-LEVEL `size` (10, not A's leaked 999), so the \
             ELSE block is the one dropped — got dropped range {dropped_src:?}"
    );
}

/// Sorted ranges of every block a `tc`/`lower` run dropped, for comparing
/// the two sides directly.
fn typecheck_dropped(src: &str) -> Vec<crate::diagnostic::SourceRange> {
    let mut ranges: Vec<crate::diagnostic::SourceRange> = tc(src)
        .dropped_ranges
        .into_iter()
        .map(|(range, _reason)| range)
        .collect();
    ranges.sort_by_key(|r| (r.start.offset, r.end.offset));
    ranges
}

fn lowering_dropped(src: &str) -> Vec<crate::diagnostic::SourceRange> {
    let crate::compile::FrontEnd {
        resolved: p,
        typecheck: tc_result,
        lowered: out,
        ..
    } = crate::compile::front_end(
        src,
        "test",
        None,
        crate::lower::FoldMode::ForceOff,
        &crate::resolve::FsLoader,
    );
    assert!(
        p.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        p.diagnostics
    );
    assert!(
        tc_result
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "typecheck errors: {:?}",
        tc_result.diagnostics
    );
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "lower errors: {:?}",
        out.diagnostics
    );
    let mut ranges = out.dropped_ranges;
    ranges.sort_by_key(|r| (r.start.offset, r.end.offset));
    ranges
}

/// A port is readable by name only when it carries a single wire. A record-
/// or tuple-shaped one does not: the annotated form dissolves into one pin
/// per field, and the unannotated form leaves its own pin unwired because
/// the value goes to `pending_out_records` (or has no single port at all).
/// The registry must claim neither, in every spelling that reaches the name.
#[test]
fn a_record_typed_port_is_not_readable_by_name() {
    let annotated = "type Point = { x: int, y: int }\n\
                         in a: int\nout p: Point = { x: a, y: 2 }\n";
    for (label, tail) in [
        (
            "a bare read in a handler",
            "in go: exec\non go { var v: Point = p }",
        ),
        ("an annotated port initialized from it", "out q: Point = p"),
        (
            "a field projection",
            "in go: exec\non go { var v: int = p.x }",
        ),
        ("an unannotated port initialized from it", "out q = p"),
    ] {
        let r = tc(&format!("{annotated}{tail}"));
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS002" && d.message.contains("'p'")),
            "reading a record port must be WS002 - {label}: {:?}",
            r.diagnostics
        );
    }
    // The same rule keyed on the VALUE, for a site with no annotation. A
    // tuple literal parses as a record literal, so both spellings land here.
    for (label, src) in [
        (
            "an unannotated record-valued port",
            "in a: int\nout p = { x: a, y: 2 }\nout q: int = p",
        ),
        (
            "an unannotated tuple-valued port",
            "in a: int\nout p = (a, 2)\nout q: int = p",
        ),
    ] {
        let r = tc(src);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS002" && d.message.contains("'p'")),
            "reading an unannotated record-valued port must be WS002 - {label}: {:?}",
            r.diagnostics
        );
    }
    // Ports nobody reads by name still compile, in every one of these forms.
    assert_no_diags(&tc(annotated));
    assert_no_diags(&tc("in a: int\nout t: (int, int) = (a, 2)"));
    assert_no_diags(&tc("in a: int\nout p = { x: a, y: 2 }"));
    let r = tc("in a: int\nout t: (int, int) = (a, 2)\nout u: (int, int) = t");
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS002" && d.message.contains("'t'")),
        "reading a tuple port must be WS002: {:?}",
        r.diagnostics
    );
    // An annotated record port keeps every per-field pin, and each one is
    // driven: refusing the bare read must not cost the port its wiring.
    let lowered = lower_for_ports(annotated);
    let pins: Vec<crate::ir::NodeId> = top_level_ports(&lowered)
        .filter(|(_, label)| label == "p_x" || label == "p_y")
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        pins.len(),
        2,
        "both field pins must exist: {:?}",
        lowered.module.nodes
    );
    for pin in pins {
        assert!(
            lowered.module.wires.iter().any(|w| w.target.node_id == pin),
            "every field pin must be driven; wires: {:?}",
            lowered.module.wires
        );
    }
}

/// Every module-boundary port name typecheck's registry claimed for `src`.
fn typecheck_port_sites(src: &str) -> std::collections::BTreeSet<String> {
    tc(src).port_sites.into_iter().collect()
}

/// Lower `src` with folding off. Neither this nor `typecheck_port_sites`
/// asserts the program is diagnostic-free: several cases in the corpus below
/// are deliberately rejected shapes whose whole point is that NEITHER side
/// claims a port.
fn lower_for_ports(src: &str) -> crate::lower::LowerResult {
    let crate::compile::FrontEnd {
        resolved: p,
        lowered,
        ..
    } = crate::compile::front_end(
        src,
        "test",
        None,
        crate::lower::FoldMode::ForceOff,
        &crate::resolve::FsLoader,
    );
    assert!(
        p.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        p.diagnostics
    );
    lowered
}

/// The `PortLabel` of every top-level (non-chip) `Output` node in `src`'s
/// module. Read off the emitted module rather than from instrumentation: a
/// boundary port IS such a node. A record- or tuple-typed port never appears
/// under its own name, because `record_output_pins` labels its pins
/// `<name>_<field>` instead.
fn lowering_boundary_ports(src: &str) -> std::collections::BTreeSet<String> {
    top_level_ports(&lower_for_ports(src))
        .map(|(_, label)| label)
        .collect()
}

fn top_level_ports(
    r: &crate::lower::LowerResult,
) -> impl Iterator<Item = (crate::ir::NodeId, String)> + '_ {
    r.module
        .nodes
        .values()
        .filter(|n| n.kind == crate::ir::NodeKind::Output && n.chip_id.is_none())
        .filter_map(
            |n| match n.properties.get(&*crate::intern::sym::PORT_LABEL) {
                Some(crate::ir::Literal::String(name)) => Some((n.id, name.to_string())),
                _ => None,
            },
        )
}

/// THE structural invariant behind readable outputs: every port typecheck's
/// registry hands out as readable must be a port lowering actually emits,
/// and each phase must claim exactly the set this corpus pins for it.
///
/// A name only typecheck claims is a silent miscompile: the read
/// type-checks and lowers to nothing, so the read and the write both
/// disappear with no diagnostic. The reverse is slack, and the safe
/// direction: lowering emits a pin the source cannot name (a record port's
/// per-field pins, or an unannotated record-valued port's dead single pin),
/// which at worst leaves a port nobody can read.
///
/// Five hand-written traversals decide this between them
/// (`lower::predeclare::pre_declare_handler_outputs` and
/// `pre_declare_anon_chip`, `typecheck::register::register_handler_outputs`
/// and `register_anon_chip_outputs`, and
/// `analysis::scoped_refs::widen_hoisted_out_bindings`). The site set inside
/// a body comes from the shared `ast::handler_port_sites` /
/// `ast::anon_chip_port_sites` enumerators, but WHICH bodies each traversal
/// feeds them is decided per pass, which is what this corpus pins
/// down. Both the annotated and the unannotated axis are covered: a rule
/// keyed on one of them alone passes every row of the other.
#[test]
fn port_registry_matches_lowerings_boundary_ports() {
    // (label, source, ports typecheck registers, port labels lowering emits)
    let cases: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "a top-level out",
            "in a: int\nout y: int = a",
            &["y"],
            &["y"],
        ),
        (
            "an unannotated top-level out, read by another",
            "in a: int\nout y = a + 1\nout z: int = y",
            &["y", "z"],
            &["y", "z"],
        ),
        (
            "a top-level handler body",
            "on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}",
            &["flash"],
            &["flash"],
        ),
        (
            "a handler body's nested if / if let / let else",
            "in go: exec\nin n: int\non go {\nif n > 0 { out a: int = 1 } else { out b: int = 2 }\nif let Some(v) = Option.Some(n) { out c: int = v }\nlet Some(w) = Option.Some(n) else { out d: int = 4\nreturn }\n}",
            &["a", "b", "c", "d"],
            &["a", "b", "c", "d"],
        ),
        (
            "a handler nested in a handler body",
            "in go: exec\non go {\non Clock(interval = 0.2) { out inner: int = 1 }\n}",
            &["inner"],
            &["inner"],
        ),
        (
            "a captured event's body",
            "let e = on Clock(interval = 0.2) {\nout flash: bool = Toggle()\n}",
            &["flash"],
            &["flash"],
        ),
        (
            "a file-scope anon chip",
            "chip { out shared: int = 1 }",
            &["shared"],
            &["shared"],
        ),
        (
            "a statement-level anon chip inside a handler",
            "in go: exec\non go {\nchip { out shared: int = 1 }\nvar v: int = shared\n}",
            &["shared"],
            &["shared"],
        ),
        (
            "an anon chip nested in an anon chip",
            "chip { chip { out shared: int = 1 } }",
            &["shared"],
            &["shared"],
        ),
        (
            // A named chip's own body `out` is that CHIP's port, declared
            // through its signature frame, not a module-boundary one. Its
            // node carries a `chip_id`, so it is not a top-level port
            // either.
            "a named chip body",
            "out ok: int = 1\nchip Named(t: exec) -> (r: int) { out r = 2 }\nin go: exec\nlet q = Named(go)",
            &["ok"],
            &["ok"],
        ),
        (
            // WS073: a handler inside a chip is not hoisted, so `flash` has
            // no port on either side.
            "a handler inside a named chip",
            "out ok: int = 1\nchip Named(t: exec) { on t { out flash: int = 2 } }\nin go: exec\nlet q = Named(go)",
            &["ok"],
            &["ok"],
        ),
        (
            // WS023: `@side` is only legal on a root-level `out`, so an anon
            // chip's `@side out` never becomes a port.
            "an @side-annotated anon chip out",
            "out ok: int = 1\nchip { @top out sided: int = 2 }",
            &["ok"],
            &["ok"],
        ),
        (
            // A record-typed boundary port dissolves into one pin per field,
            // so there is no single wire for a bare read: typecheck must not
            // claim it, while lowering emits (and drives) the per-field pins
            // under their own labels.
            "a record-typed port",
            "type Point = { x: int, y: int }\nin a: int\nout ok: int = a\nout p: Point = { x: a, y: 2 }",
            &["ok"],
            &["ok", "p_x", "p_y"],
        ),
        (
            "a tuple-typed port",
            "in a: int\nout ok: int = a\nout t: (int, int) = (a, 2)",
            &["ok"],
            &["ok", "t_0", "t_1"],
        ),
        (
            // With no annotation the VALUE decides. A record literal (and a
            // tuple literal, which parses as one) has no single wire, and
            // lowering leaves the port's own pin unwired, so a read would
            // resolve to a permanent zero. Typecheck must not claim it even
            // though lowering still emits the pin.
            "an unannotated record-valued port",
            "in a: int\nout ok: int = a\nout p = { x: a, y: 2 }",
            &["ok"],
            &["ok", "p"],
        ),
        (
            "an unannotated tuple-valued port",
            "in a: int\nout ok: int = a\nout p = (a, 2)",
            &["ok"],
            &["ok", "p"],
        ),
    ];
    let set = |names: &[&str]| -> std::collections::BTreeSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    };
    let mut saw_a_port = false;
    for (label, src, tc_expected, lo_expected) in cases {
        let tc_expected = set(tc_expected);
        let lo_expected = set(lo_expected);
        let tc_ports = typecheck_port_sites(src);
        let lo_ports = lowering_boundary_ports(src);
        assert_eq!(
            tc_ports, tc_expected,
            "typecheck's port registry claimed the wrong set - {label}"
        );
        assert_eq!(
            lo_ports, lo_expected,
            "lowering emitted the wrong boundary ports - {label}"
        );
        // The soundness direction, restated as its own assert so a future
        // row cannot satisfy both tables above while still handing out a
        // readable name that lowers to nothing.
        assert!(
            tc_ports.is_subset(&lo_ports),
            "every readable port must exist in the module - {label}\n  \
                 typecheck claimed: {tc_ports:?}\n  lowering emitted:  {lo_ports:?}"
        );
        saw_a_port |= !tc_expected.is_empty();
    }
    // A corpus that claimed nothing anywhere would satisfy every equality
    // vacuously.
    assert!(saw_a_port, "the corpus must claim at least one real port");
}

/// The EXACT-PARITY invariant that `lower::context`, `lower::stmt`,
/// `lower::decl` and `typecheck::ctx` all cite by name. It was cited by
/// four doc comments for a long time without ever being written, and that
/// is precisely how a real divergence shipped: block-scope `const`
/// recording used the FULL evaluator in `typecheck::stmt` but the NARROW
/// `expr_to_literal_in` in `lower::decl`, so a `const` bound to a record —
/// `const p = { x: 1 }` with `if p.x == 1`, and every destructuring
/// `const` — was compile-time-decidable to typecheck and not to lowering.
/// Typecheck skipped the untaken block; lowering emitted it. Ill-typed
/// code reached the graph with no diagnostic at all.
///
/// The weaker containment property
/// (`typecheck_never_drops_a_block_that_lowering_still_emits`, below) does
/// NOT catch that: dropping LESS on the typecheck side is the safe
/// direction for containment, yet it is exactly what under-checking looks
/// like when it is lowering that drops less. Equality catches both
/// directions at once.
///
/// Corpus covers a plain `const`, a record-valued `const` read by field, a
/// block-scope destructure (plain / alias / rest) and a top-level
/// destructure — every shape whose recording differs between the two
/// sides. Cases where the two stages genuinely hold DIFFERENT information
/// (a `const` param's placeholder, `ident_literal_bool`) are excluded by
/// construction and live in the containment test instead.
#[test]
fn typecheck_and_lowering_drop_exactly_the_same_ranges() {
    let cases: &[(&str, &str)] = &[
        (
            "a plain block-scope const",
            "var x: int = 0\n\
                 mod f() { const A = 1\n if A == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            // The shape that shipped broken with no destructuring at all:
            // `expr_to_literal_in` cannot fold a record literal.
            "a record-valued block-scope const read by field",
            "var x: int = 0\n\
                 mod f() { const p = { a: 1 }\n if p.a == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            "a block-scope destructure",
            "var x: int = 0\n\
                 mod f() { const p = { a: 1, b: 2 }\n const { a, b } = p\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            "a block-scope destructure with an alias",
            "var x: int = 0\n\
                 mod f() { const p = { a: 1, b: 2 }\n const { a: q, b: r } = p\n\
                   if q == 1 { x = 1 } else { x = 2 }\n\
                   if r == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            "a block-scope destructure with a rest",
            "var x: int = 0\n\
                 mod f() { const p = { a: 1, b: 2, c: 3 }\n const { a, ...rest } = p\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if rest.c == 3 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            "a top-level destructure",
            "const p = { a: 1, b: 2 }\nconst { a, b } = p\n\
                 var x: int = 0\nin go: exec\n\
                 on go { if a == 1 { x = 1 } else { x = 2 }\n\
                         if b == 9 { x = 3 } else { x = 4 } }",
        ),
        (
            "a destructure inside a nested block",
            "var x: int = 0\n\
                 mod f(n: int) { if n > 0 { const p = { a: 1 }\n const { a } = p\n\
                   if a == 1 { x = 1 } else { x = 2 } } }\n\
                 in go: exec\non go { f(1) }",
        ),
        // SHADOWING — the other half of keeping the two environments in
        // step, and the half that had no coverage at all. Recording the
        // same constants is only sound if RE-BINDING one clears it on both
        // sides too, and lowering's clearing handled a single spelling
        // (`Ident`, narrow evaluator) where typecheck's handled every one.
        // Each case shadows ONE const and leaves a sibling alone, so the
        // still-const half keeps the corpus's non-vacuity assert honest
        // while the shadowed half is what actually has to agree.
        (
            // `let { a } = …` cleared `a`'s const mark in typecheck and
            // not in lowering: typecheck emitted a real Branch for
            // `if a == 1` while lowering tree-shook it on the STALE value,
            // shipping only the `x = 1` arm and no Branch gate at all.
            "a destructuring let shadowing a destructuring const",
            "var x: int = 0\n\
                 mod f() { const { a, b } = { a: 1, b: 2 }\n let { a } = { a: 9 }\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            // The same divergence reachable with NO destructuring: an
            // `Ident` `let` whose value only the FULL evaluator folds
            // (an `if` expression) never reached lowering's narrow
            // clearing path either.
            "a let shadowing a const with a value only the full evaluator folds",
            "var x: int = 0\n\
                 mod f() { const a = 1\n const b = 2\n let a = if true then 9 else 0\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f() }",
        ),
        (
            // The same shadow with the `const` at the TOP level, which is
            // a different mechanism rather than a rephrasing: eviction in
            // `const_lookup_declared_only` fires on a name being PRESENT
            // in a `scoped_consts` frame and ABSENT from that frame's
            // marks, so a top-level constant is only evicted once the
            // inner `let` RECORDS its own value there. Clearing the mark
            // alone left the top-level value resolving straight through.
            "a block-scope let shadowing a top-level const",
            "const a = 1\nconst b = 2\nvar x: int = 0\n\
                 in go: exec\n\
                 on go { let a = if true then 9 else 0\n\
                   if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }",
        ),
        // NAMED-CHIP BOUNDARY. A chip body is a `Block` of statements, so
        // its own `const`s never reach `build_const_env` (which takes
        // `TopDecl`s) — they are recorded by `lower_let_decl` into
        // `scoped_consts.last_mut()`, and `instance_body` used to hand the
        // child ctx NO frame at all, so every one was silently discarded.
        // Typecheck has always had a frame here, so the two stages read
        // different constants inside the same body.
        (
            // The sharpest form: lowering decided on the OUTER constant
            // and typecheck on the INNER one, so they emitted and checked
            // OPPOSITE arms — a wrong-branch miscompile AND an unchecked
            // block reaching the graph, from one program.
            "a named chip's own const shadowing a top-level one",
            "const a = 1\nvar x: int = 0\n\
                 chip Named(t: exec) { const a = 9\n\
                   on t { if a == 1 { x = 1 } else { x = 2 }\n\
                          if a == 9 { x = 3 } else { x = 4 } } }\n\
                 in go: exec\nlet r = Named(go)",
        ),
        (
            // The same gap with nothing shadowed: typecheck decided the
            // condition on the chip-body `const` while lowering, holding
            // no value for it, emitted a runtime Branch over both arms —
            // typecheck dropping a block lowering still emits.
            "a named chip's own const with no outer binding",
            "var x: int = 0\n\
                 chip Named(t: exec) { const a = 7\n\
                   on t { if a == 9 { x = 1 } else { x = 2 } } }\n\
                 in go: exec\nlet r = Named(go)",
        ),
        (
            // INLINED-CALLEE BOUNDARY. A `mod` body is lowered into the
            // CALLER's ctx with the caller's block scopes still open, and
            // the const lookups walk every open frame — so a caller-local
            // shadow evicted the top-level constant INSIDE the callee.
            // Typecheck checks a `mod` body once, at its declaration,
            // where no call site's scope exists, so it kept resolving the
            // top-level one and elided an arm lowering then emitted.
            "a caller-local shadow must not reach into an inlined callee",
            "const a = 1\nvar x: int = 0\n\
                 mod inner() { if a == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\n\
                 on go { let a = if true then 9 else 0\n  inner() }",
        ),
        (
            "a caller-local var must not reach into an inlined callee",
            "const a = 1\nvar x: int = 0\nvar k: int = 3\n\
                 mod inner() { if a == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\n\
                 on go { var a: int = k\n  inner() }",
        ),
        // A parameter, `var`, loop variable or destructured name shadowing
        // a top-level const is that binding: neither side decides on it.
        (
            "a mod parameter shadowing a top-level const",
            "const a = 1\nconst b = 2\nvar x: int = 0\nvar k: int = 3\n\
                 mod f(a: int) { if a == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f(k) }",
        ),
        (
            "a var, a loop variable and a destructured name shadowing top-level consts",
            "const a = 1\nconst b = 2\nconst c = 3\nconst d = 4\nvar x: int = 0\nvar k: int = 3\n\
                 type P = { c: int }\nvar pv: P = { c: 3 }\n\
                 in go: exec\n\
                 on go { var a: int = k\n if a == 1 { x = 1 } else { x = 2 }\n\
                   for b in 0..k { if b == 2 { x = 3 } }\n\
                   let { c } = pv\n if c == 3 { x = 5 } else { x = 6 }\n\
                   if d == 4 { x = 7 } else { x = 8 } }",
        ),
        (
            "a const record parameter shadowing a top-level const",
            "const a = { v: 1 }\nconst b = 2\nvar x: int = 0\n\
                 type P = { v: int }\n\
                 mod f(a: const P) { if a.v == 1 { x = 1 } else { x = 2 }\n\
                   if b == 2 { x = 3 } else { x = 4 } }\n\
                 in go: exec\non go { f({ v: 5 }) }",
        ),
    ];
    for (label, src) in cases {
        let tc_dropped = typecheck_dropped(src);
        let lo_dropped = lowering_dropped(src);
        assert_eq!(
            tc_dropped, lo_dropped,
            "typecheck and lowering must elide EXACTLY the same blocks — {label}\n  \
                 typecheck dropped: {tc_dropped:?}\n  lowering dropped:  {lo_dropped:?}"
        );
        // A corpus entry that elides nothing on both sides would satisfy
        // the equality vacuously and silently stop testing anything.
        assert!(
            !tc_dropped.is_empty(),
            "case must actually exercise an elision — {label}"
        );
    }
}

/// THE soundness property of this feature, stated exactly:
/// **everything LOWERED must have been CHECKED.** A block is lowered iff
/// it is not in lowering's dropped set; checked iff it is not in
/// typecheck's. So the requirement is the containment
///
/// ```text
/// typecheck_dropped  ⊆  lowering_dropped
/// ```
///
/// — typecheck may skip a block only if lowering also skips it. The
/// reverse slack is deliberately allowed and is the SAFE direction:
/// lowering knows things typecheck cannot (a call site's real argument, a
/// literal bound to a gate), so it may drop MORE. That merely means a
/// block was checked and then not emitted — at worst a diagnostic about
/// absent code, never shipped-unchecked code.
///
/// Every case below is a shape that genuinely diverged at some point, so
/// this is a regression net rather than a smoke test. Cases 1-3 also
/// satisfy exact EQUALITY (both stages hold the same information), which
/// is asserted separately and more tightly; case 4-5 are the intentional
/// strict-subset ones.
#[test]
fn typecheck_never_drops_a_block_that_lowering_still_emits() {
    // (label, source, both stages have equal information)
    let cases: &[(&str, &str, bool)] = &[
        (
            "a plain top-level const elides on both sides",
            "const A = 1\nvar x: int = 0\nin go: exec\n\
                 on go { if A == 1 { x = 1 } else { x = 2 } if A == 9 { x = 3 } }",
            true,
        ),
        (
            "@nofold on the handler keeps BOTH blocks on both sides",
            "const A = 1\nvar x: int = 0\nin go: exec\n\n\
                 @nofold\non go { if A == 1 { x = 1 } else { x = 2 } }",
            true,
        ),
        (
            "a module-level @nofold keeps both blocks on both sides",
            "@nofold\n\nconst A = 1\nvar x: int = 0\nin go: exec\n\
                 on go { if A == 1 { x = 1 } else { x = 2 } }",
            true,
        ),
        (
            // CRITICAL regression: `mod_decls` used to be a flat map, so
            // `A`'s NESTED `const mod size` (checked first, in source
            // order) permanently overwrote the top-level `size` for
            // every later reader — typecheck resolved `B`'s call to
            // `size` to A's leaked 100 (dropping the ELSE), while
            // lowering (already scope-correct through `ctx.scope`)
            // resolved the top-level 10 (dropping the THEN). Disjoint
            // dropped sets — exactly the containment violation this test
            // exists to catch. Scoping `mod_decls` like `scoped_consts`
            // (mirroring `lower::LowerCtx::pass1_chips`) makes both
            // stages resolve `size` identically once `A`'s body (and its
            // frame) has closed.
            "a nested const mod does not leak into a later sibling's resolution",
            "const mod size() -> int { return 10 }\nvar x: int = 0\n\
                 mod A() { const mod size() -> int { return 100 } }\n\
                 mod B() { const s = size()\n if s == 100 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { B() }",
            true,
        ),
        (
            // Typecheck sees a placeholder ZERO and must decline to elide
            // at all; lowering inlines the REAL `1` and elides the ELSE.
            // Opposite selections — which is precisely why typecheck must
            // not elide. Strict subset: {} ⊂ {else}.
            "a const PARAM's placeholder must not decide a branch",
            "var x: int = 0\nmod f(m: const int) { if m == 1 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f(1) }",
            false,
        ),
        (
            "a const derived FROM a const param is just as fictional",
            "var x: int = 0\n\
                 mod f(m: const int) { const t = m + 1\n if t == 2 { x = 1 } else { x = 2 } }\n\
                 in go: exec\non go { f(1) }",
            false,
        ),
    ];
    for (label, src, equal_information) in cases {
        let tc_dropped = typecheck_dropped(src);
        let lo_dropped = lowering_dropped(src);
        for range in &tc_dropped {
            assert!(
                lo_dropped.contains(range),
                "typecheck skipped a block lowering still EMITS (unchecked code ships) \
                     — {label}\n  typecheck dropped: {tc_dropped:?}\n  lowering dropped:  {lo_dropped:?}"
            );
        }
        if *equal_information {
            assert_eq!(
                tc_dropped, lo_dropped,
                "both stages have the same information here, so their dropped sets must \
                     match exactly — {label}"
            );
        } else {
            assert!(
                tc_dropped.len() < lo_dropped.len(),
                "expected typecheck to be strictly more conservative here — {label}\n  \
                     typecheck dropped: {tc_dropped:?}\n  lowering dropped:  {lo_dropped:?}"
            );
        }
    }
}

/// Enum payload captures, payload storage, `unsafe` access, `is` tests and
/// patterns.
#[test]
fn enum_payloads_and_variant_tests() {
    const OK: &[Want] = &[NoErrors];
    let var_e = "enum E { A, B { s: string, b: bool } }\nvar e: E = E.B { s: \"hi\", b: false }\nin go: exec\n";
    let unsafe_e = "enum E { A, B { s: string, n: int } }\nvar e: E = E.B { s: \"hi\", n: 1 }\nvar got: string = \"\"\nin go: exec\n";
    let bad_e = "enum E { A, B { s: string } }\nvar e: E = E.B { s: \"hi\" }\nin go: exec\n";
    let shape = "enum Shape { Empty, Circle(float) }\nin go: exec\nvar s: Shape\nvar n: float\n";
    let kick = "enum E { A, B(int) }\nin go: exec\nvar s: E\nout jump: exec\nvar r: int\non go {\n  s = E.A\n";
    let rows: &[(&str, String, &[Want])] = &[
            // Each capture binds the scrutinee's `__{Variant}_{field}` slot, so a
            // write goes through it. Lowering always emitted the `Var_Set`; the
            // captures were registered as `LetBinding` and the write was rejected.
            ("an `if let` capture of a var is writable", format!("{var_e}on go {{ if let B {{ s, b }} = e {{ b = true\ns = \"x\" }} }}"), OK),
            ("a `match` capture of a var is writable", format!("{var_e}on go {{ match e {{ B {{ s, b }} => {{ b = true }}, A => {{}} }} }}"), OK),
            ("a `let else` capture of a var is writable", format!("{var_e}on go {{ let B {{ s, b }} = e else {{ return }}\nb = true }}"), OK),
            ("a positional capture of a var is writable", "enum Opt { Some(int), None }\nvar o: Opt = Opt.Some(5)\nin go: exec\non go { if let Some(x) = o { x = 9 } }".into(), OK),
            // A `let` scrutinee has no storage, so the write would emit no gate.
            ("a capture of an immutable scrutinee", "enum E { A, B { s: string, b: bool } }\nlet e: E = E.B { s: \"hi\", b: false }\nin go: exec\non go { if let B { s, b } = e { b = true } }".into(), &[Error("WS007")]),
            // An element scrutinee reads into a value with no write-back.
            // `is_ref_able` says yes here (right for `&arr[i]`), which would make
            // every such write a silent no-op. Reported as a capture, not a `let`.
            ("a capture of an array element", "enum E { A, B { n: int } }\nvar es: E[]\nin go: exec\non go { if let B { n } = es[0] { n = 42 } }".into(), &[Error("WS007"), FirstMsg("WS007", "read-only capture"), FirstMsg("WS007", "arr[i]")]),
            ("a capture of a map element", "enum E { A, B { n: int } }\nvar m: Map<string, E>\nin go: exec\non go { if let B { n } = m[\"k\"] { n = 42 } }".into(), &[Error("WS007")]),
            ("a capture of a field of an element", "enum E { A, B { n: int } }\ntype H = { e: E }\nvar hs: H[]\nin go: exec\non go { if let B { n } = hs[0].e { n = 42 } }".into(), &[Error("WS007")]),
            // A `var` record's enum field is its own storage.
            ("a capture through a var record field", "enum E { A, B { n: int } }\ntype H = { e: E, tag: int }\nvar h: H = { e: E.B { n: 1 }, tag: 0 }\nin go: exec\non go { if let B { n } = h.e { n = 42 } }".into(), OK),
            // A `*T` parameter's slots are the caller's gates.
            ("a capture through a `*E` param", "enum E { A, B { n: int } }\nvar e: E = E.B { n: 1 }\nin go: exec\nmod bump(v: *E) { if let B { n } = v { n = 42 } }\non go { bump(&e) }".into(), OK),
            // A payload slot is one gate written at construction, and nothing
            // fills a container there: `push`/`set` on a capture would act on
            // storage nothing ever fills.
            ("a named array payload field", "enum E { A, B { xs: int[] } }\nout n = 1".into(), &[Error("WS069")]),
            ("a positional array payload field", "enum E { A, B(int[]) }\nout n = 1".into(), &[Error("WS069")]),
            ("a map payload field", "enum E { A, B { m: Map<string, int> } }\nout n = 1".into(), &[Error("WS069")]),
            ("an array inside an inline record payload field", "enum E { A, B { r: { xs: int[] } } }\nout n = 1".into(), &[Error("WS069")]),
            ("an array inside an aliased record payload field", "type R = { xs: int[], n: int }\nenum E { A, B { r: R } }\nout n = 1".into(), &[Error("WS069")]),
            ("scalar and record payload fields", "type P = { x: int, y: int }\nenum E { A, B { p: P, s: string, v: vector }, C(int, float) }\nvar e: E = E.B { p: { x: 1, y: 2 }, s: \"hi\", v: Vec(0.0, 0.0, 0.0) }\nout n = 1".into(), OK),
            ("a nested enum payload field", "enum Inner { P { n: int }, Q }\nenum Outer { W { i: Inner }, Z }\nvar o: Outer = Outer.W { i: Inner.P { n: 1 } }\nout n = 1".into(), OK),
            // `Some(T)`'s field is the bare param, so the instantiation is where
            // the container is picked; one report per annotation.
            ("a generic enum instantiated with a container", "var o: Option<int[]> = Option.None\nout n = 1".into(), &[Error("WS069"), Count("WS069", 1)]),
            ("a param no variant stores is not a payload slot", "enum Tag<T> { A, B }\nvar t: Tag<int[]> = Tag.A\nout n = 1".into(), &[Lacks("WS069")]),
            ("a generic enum instantiated with a scalar", "var o: Option<int> = Option.Some(5)\nout n = 1".into(), OK),
            // `unsafe v.Variant.field` asserts the tag instead of testing it.
            ("an unsafe payload read", format!("{unsafe_e}on go {{ got = unsafe e.B.s }}"), OK),
            ("an unsafe payload write", format!("{unsafe_e}on go {{ unsafe e.B.s = \"x\" }}"), OK),
            ("an unsafe payload write of the wrong type", format!("{unsafe_e}on go {{ unsafe e.B.n = \"x\" }}"), &[Has("WS003")]),
            ("an unsafe record payload keeps projecting", "type P = { x: int }\nenum E2 { A, B { p: P } }\nvar e: E2 = E2.B { p: { x: 1 } }\nvar got: int = 0\nin go: exec\non go { unsafe e.B.p.x = 5\ngot = unsafe e.B.p.x }".into(), OK),
            ("an unsafe access to an unknown variant", format!("{bad_e}on go {{ unsafe e.Nope.s = \"x\" }}"), &[Has("WS060")]),
            ("an unsafe access to an unknown payload field", format!("{bad_e}on go {{ unsafe e.B.nope = \"x\" }}"), &[Has("WS010")]),
            ("an unsafe access on a non-enum", "var n: int = 0\nin go: exec\non go { n = unsafe n.B.s }".into(), &[Has("WS066")]),
            // `e.s` reaches no slot (they are keyed by variant); it typed `any` and
            // lowered to a placeholder.
            ("a payload field read directly", "enum E { A, B { s: string } }\nvar e: E = E.B { s: \"hi\" }\nvar got: string = \"\"\nin go: exec\non go { got = e.s }".into(), &[FirstMsg("WS010", "if let"), FirstMsg("WS010", "unsafe")]),
            ("`is` types as a bool", "enum Shape { Empty, Circle(float) }\nvar s: Shape = Shape.Empty\nout hit: bool = s is Shape.Circle".into(), OK),
            ("`is` on a variant path", "enum Shape { Empty, Circle(float) }\nout hit: bool = Shape.Empty is Shape.Circle".into(), OK),
            ("`is` on a non-enum value", "enum Shape { Empty, Circle(float) }\nvar n: int = 0\nout hit = n is Shape.Circle".into(), &[Has("WS066")]),
            // An enum value on the right names no variant.
            ("`is` with a value on the right", "enum Shape { Empty, Circle(float) }\nvar s: Shape = Shape.Empty\nvar other: Shape = Shape.Circle(1.0)\nout hit = s is other".into(), &[Has("WS066")]),
            ("`is` with an unknown variant", "enum Shape { Empty, Circle(float) }\nvar s: Shape = Shape.Empty\nout hit = s is Shape.Nope".into(), &[Has("WS060")]),
            // Construction and `is` require the qualifier, so a pattern accepts it.
            ("a qualified `match` pattern", format!("{shape}on go {{\n  s = Shape.Circle(2.0)\n  n = match s {{\n    Shape.Empty => 0.0,\n    Shape.Circle(r) => r,\n  }}\n}}\n"), OK),
            ("a qualified `if let` pattern", format!("{shape}on go {{\n  s = Shape.Circle(2.0)\n  if let Shape.Circle(r) = s {{ n = r }}\n}}\n"), OK),
            ("an unqualified `match` pattern", format!("{shape}on go {{\n  s = Shape.Circle(2.0)\n  n = match s {{\n    Empty => 0.0,\n    Circle(r) => r,\n  }}\n}}\n"), OK),
            // A variant name alone can belong to several enums.
            ("a qualifier naming another enum", format!("enum Other {{ Empty }}\n{shape}on go {{\n  s = Shape.Circle(2.0)\n  n = match s {{\n    Other.Empty => 0.0,\n    Shape.Circle(r) => r,\n  }}\n}}\n"), &[HasMsg("WS060", "names enum `Other`")]),
            // WS053 is the only guard against a permanently parked exec chain, so
            // its walker descends into `if let` and `match` arms as well as `if`.
            ("a parked kick in an `if`", format!("{kick}  if s is E.A {{ emit jump\n    await jump\n  }}\n}}\n"), &[Has("WS053")]),
            ("a parked kick in an `if let`", format!("{kick}  if let E.B(v) = s {{ emit jump\n    await jump\n  }}\n}}\n"), &[Has("WS053")]),
            ("a parked kick in a `match` arm", format!("{kick}  r = match s {{\n    E.A => {{ emit jump\n      await jump\n      1 }}\n    E.B(v) => 0,\n  }}\n}}\n"), &[Has("WS053")]),
        ];
    check_rows(rows);
}

/// Every site that writes a module-boundary port (a top-level `out`, a
/// handler-declared one, an anon chip's) answers to the port's registered
/// type, and a port reads by name like any value.
#[test]
fn port_sites_are_checked_against_the_port_type() {
    const OK: &[Want] = &[NoErrors];
    const WS003: &[Want] = &[Has("WS003")];
    let two = "in go1: exec\nin go2: exec\non go1 {\nout shared: int = 1\n}\n";
    let rows: &[(&str, String, &[Want])] = &[
            // Registration runs in pass 1, before inference, so the port is `any`,
            // as the namespaced `NsDeclInfo` path reports it.
            ("an unannotated port reads as any", "in a: int\nout y = a + 1\nout z: int = y".into(), OK),
            // Registered here as well as by `pre_declare_handler_outputs`, or a
            // later read in the same block is WS002 while lowering wires it.
            ("a handler-declared port read in its handler", "static var ticks: int = 0\non Clock(interval = 0.2) {\nticks = ticks + 1\nout counted: int = ticks\nvar seen: int = counted\n}".into(), OK),
            // Registration lands at file scope, not in the handler's frame.
            ("a handler-declared port read from another handler", "in go: exec\nstatic var ticks: int = 0\non Clock(interval = 0.2) {\nticks = ticks + 1\nout counted: int = ticks\n}\non go {\nvar seen: int = counted\n}".into(), OK),
            // Registration is first-wins; either file order is a genuine conflict.
            ("a port type conflict, top-level out first", "out counted: int = 5\non Clock(interval = 0.2) {\nout counted: string = \"hi\"\n}\nlet x: int = counted + 1".into(), WS003),
            ("a port type conflict, handler first", "on Clock(interval = 0.2) {\nout counted: string = \"hi\"\n}\nout counted: int = 5\nlet x: int = counted + 1".into(), WS003),
            // An unannotated site states no type, so it is judged against the
            // annotated one identically in both orders.
            ("an unannotated site first", "in go: exec\nout shared = \"x\"\non go {\nout shared: int = 1\n}".into(), &[Count("WS003", 1)]),
            ("an annotated site first", "in go: exec\non go {\nout shared: int = 1\n}\nout shared = \"x\"".into(), &[Count("WS003", 1)]),
            // Lowering wires both branches into ONE typed backing var with no
            // conversion; the fixture guard only checks placeholders and diagnostics.
            ("disagreeing branch sites", "in go: exec\nin cond: bool\non go {\nif cond {\nout branch: int = 1\n} else {\nout branch: string = \"x\"\n}\n}".into(), WS003),
            // The signature frame knows nothing about module ports; the registry does.
            ("a mismatched unannotated update site", format!("{two}on go2 {{\nout shared = \"x\"\n}}\nlet x: int = shared + 1"), WS003),
            ("a matching unannotated update site", format!("{two}on go2 {{\nout shared = 2\n}}\nlet x: int = shared + 1"), OK),
            // `emit` drives the backing variable as an `out` binding does.
            ("a mismatched emit into a handler-declared port", format!("{two}on go2 {{\nemit shared = \"x\"\n}}\nlet x: int = shared + 1"), WS003),
            ("a matching emit into a handler-declared port", format!("{two}on go2 {{\nemit shared = 2\n}}\nlet x: int = shared + 1"), OK),
            ("a mismatched unannotated top-level out", "in go1: exec\non go1 {\nout shared: int = 1\n}\nout shared = \"x\"\nlet x: int = shared + 1".into(), WS003),
            ("a matching unannotated top-level out", "in go1: exec\non go1 {\nout shared: int = 1\n}\nout shared = 2\nlet x: int = shared + 1".into(), OK),
            // Outside every mod and chip a value return has no output; an anon
            // chip's port is written by name like any `out`.
            ("`return s` beside an anon chip port", "in go: exec\nin s: string\nin n: int\non go {\nchip { out shared: int }\nreturn s\n}".into(), &[Has("WS099")]),
            ("`return n` beside an anon chip port", "in go: exec\nin s: string\nin n: int\non go {\nchip { out shared: int }\nreturn n\n}".into(), &[Has("WS099")]),
            // An anon chip opens no scope, so its plain `out` is a module port.
            ("a mismatched write to an anon chip's port", "in go: exec\nchip {\nout shared: int = 1\n}\non go {\nout shared = \"x\"\n}".into(), WS003),
            ("a matching write to an anon chip's port", "in go: exec\nchip {\nout shared: int = 1\n}\non go {\nout shared = 2\n}".into(), OK),
            ("an anon chip's port read by name", "chip {\nout shared: int = 1\n}\nout other: int = shared".into(), OK),
            // A side annotation is only valid on a top-level port, so lowering
            // pre-declares nothing; claiming it would type a port never created.
            ("a side-annotated anon chip out is not a port", "chip {\n@bottom out shared: int = 1\n}\nout other: int = shared".into(), &[Has("WS002")]),
            // Two always-live sites with values drive one port at once (emit made
            // two bricks with one `PortLabel`, one wired to nothing); exec-derived
            // sites fire one per tick.
            ("two top-level initializers", "var v: float = 1.0\nout v6: float = v\nout v6: float = v".into(), &[HasMsg("WS013", "drive it continuously")]),
            ("an initializer and an emit", "in go: exec\nout r: int = 0\non go { emit r = 5 }".into(), OK),
            ("two handler sites", "in a: exec\nin b: exec\non a { out s: int = 1 }\non b { out s: int = 2 }".into(), OK),
            ("an initializer and a bare declaration", "var v: float = 1.0\nout v6: float = v\nout v6: float".into(), OK),
            // When the annotation and the port share a type, one mismatch is one report.
            ("a mismatched annotated top-level out", "in s: string\nout y: int = s".into(), &[Count("WS003", 1)]),
            ("a mismatched annotated handler out", "in go: exec\nin s: string\non go { out y: int = s }".into(), &[Count("WS003", 1)]),
            ("a mismatched annotated chip out", "chip Foo(v: string) -> (r: int) {\nout r: int = v\n}\nin a: string\nout z: int = Foo(a)".into(), &[Count("WS003", 1)]),
            // Coercion is not transitive: string reaches bool and bool reaches
            // int, but string does not reach int.
            ("an annotation the port does not share", "in go: exec\nin s: string\non go { out shared: int = 1 }\non go { out shared: bool = s }".into(), &[HasExact("WS003", "expected int, got string")]),
            // The `if` branch mismatch and the port write are separate problems.
            ("a bad value expression and a bad port write", "in go1: exec\nin go2: exec\nin c: bool\non go1 {\nout shared: int = 1\n}\non go2 {\nout shared = if c then Vec(0.0, 0.0, 0.0) else \"s\"\n}".into(), &[HasMsg("WS003", "expected int"), Count("WS003", 2)]),
            // A handler-declared `out` is a module port; a top-level handler's
            // value return gives it nothing.
            ("`return \"x\"` beside a handler-declared port", "in go: exec\nin go2: exec\non go {\nout shared: int\n}\non go2 {\nreturn \"x\"\n}".into(), &[Has("WS099")]),
            ("`return 5` beside a handler-declared port", "in go: exec\nin go2: exec\non go {\nout shared: int\n}\non go2 {\nreturn 5\n}".into(), &[Has("WS099")]),
            // The value fits the annotation and the port; only comparing the
            // site's claim against the signature catches it.
            ("an out annotation the signature output rejects", "chip Foo(v: int) -> (r: int) {\nout r: string = v\n}".into(), WS003),
            // An unresolved output type does not hand the body to a same-named port.
            ("an unresolvable signature output", "in a: int\nout shared: int = a\nchip Foo(v: string) -> (shared: Nope) {\nout shared = v\n}".into(), &[Has("WS002"), Lacks("WS003")]),
            ("an emit judged by a type a later site states", "in go: exec\nin go2: exec\nout shared = 1\non go {\nout shared: int = 1\n}\non go2 {\nemit shared = \"x\"\n}".into(), WS003),
            // Inside a mod body its signature output wins over a same-named port.
            ("a mod output wins over a top-level port", "out shared: string = \"hi\"\nmod pick(v: int) -> (shared: int) {\nout shared = v\n}".into(), OK),
            ("a mod body is checked against its own output", "out shared: string = \"hi\"\nmod pick(v: string) -> (shared: int) {\nout shared = v\n}".into(), WS003),
            // `any` is a declared type, so the port is never consulted.
            ("an `any` mod output wins over a top-level port", "out shared: int = 1\nmod pick(v: string) -> (shared: any) {\nout shared = v\n}".into(), OK),
        ];
    check_rows(rows);
}

#[test]
fn emit_as_a_value_types_as_exec() {
    let src = "in go: exec\non go {\n  let d = emit\n}";
    let r = tc(src);
    assert_no_diags(&r);
    let at = src.find("emit").unwrap();
    let ty = r
        .type_of_expr
        .get(&(std::sync::Arc::from("test"), at, at + "emit".len()))
        .expect("the `emit` value must be typed");
    assert_eq!(*ty, Type::Exec);
}

#[test]
fn a_record_type_read_as_a_value_is_ws100() {
    // `Box { v: n }` reads as `Box` and a separate `{ v: n }`.
    for decl in ["type Box = { v: int }", "type Box<T> = { v: T }"] {
        let r = tc(&format!(
            "{decl}\nin go: exec\nvar n: int = 3\nvar r: int = 0\non go {{\n let c = Box {{ v: n }}\n r = c.v\n}}"
        ));
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS100" && d.message.contains("a record type has no")),
            "{decl}: {:?}",
            r.diagnostics
        );
    }
    // A scalar alias is worded as one.
    let r = tc("type Score = int\nvar n: int = 0\nin go: exec\non go {\n let s = Score\n}");
    let d = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS100")
        .expect("WS100");
    assert!(
        d.message.contains("another name for `int`") && !d.message.contains("record"),
        "{}",
        d.message
    );
    assert_no_diags(&tc(
        "type Box = { v: int }\nin go: exec\nvar n: int = 3\nvar r: int = 0\non go {\n let c: Box = { v: n }\n r = c.v\n}",
    ));
    // An enum's name still reaches its variants.
    assert_no_diags(&tc(
        "enum Hue { Red, Green }\nvar h: Hue = Hue.Red\nin go: exec\non go { h = Hue.Green }",
    ));
}

#[test]
fn call_dot_exec_types_as_exec() {
    // `.exec` names the call's own exec output, so it types as `exec`
    // rather than falling through to `any`, which coerces to everything
    // and would let a real mistype past unremarked.
    let src = "var m: int = 0\n\
                   chip Later() -> (count: int) {\n\
                   m = 7\n\
                   out count = m.Value\n\
                   }\n\
                   in go: exec\n\
                   on go {\n\
                   let d = Later().exec\n\
                   await d\n\
                   }";
    let r = tc(src);
    assert_no_diags(&r);
    let access = "Later().exec";
    let start = src.find(access).expect("call site");
    let ty = r
        .type_of_expr
        .get(&(std::sync::Arc::from("test"), start, start + access.len()))
        .expect("the `.exec` access must be typed");
    assert_eq!(*ty, Type::Exec, "`.exec` on a call is an exec value");
}

/// `for` loop placement and body rules.
#[test]
fn for_loop_codes() {
    let rows: &[(&str, &str, &[Want])] = &[
        // `for` is a statement, not a `TopDecl`; a top-level `if` block runs in
        // the module's ambient (pure) context, as the `for` inside it does.
        (
            "a pure `for`",
            "var arr: int[]\nif true { for v in arr { } }",
            &[Has("WS074")],
        ),
        (
            "a `for` over a call",
            "mod make() -> int[] { }\nin go: exec\non go { for v in make() { } }",
            &[Has("WS075")],
        ),
        (
            "three bindings",
            "var arr: int[]\nin go: exec\non go { for a, b, c in arr { } }",
            &[Has("WS076")],
        ),
        (
            "a zero step",
            "in go: exec\non go { for i in (0..10).step(0) { } }",
            &[Has("WS077")],
        ),
        (
            "`break` outside a loop",
            "in go: exec\non go { break }",
            &[Has("WS078")],
        ),
        (
            "`return` inside a loop body",
            "mod f() { for i in 0..3 { return } }",
            &[Has("WS078")],
        ),
        (
            "a sleep in a loop body",
            "in go: exec\non go { for i in 0..3 { await Sleep(_, delay = 1.0) } }",
            &[Has("WS079")],
        ),
        // WS079 is keyed on `loop_depth`, not on `await` itself.
        (
            "an await outside a loop",
            "let done: exec\nin go: exec\non go { await done }",
            &[Lacks("WS079")],
        ),
        // `loop_depth` drops back to zero once the body is checked.
        (
            "an await after a loop body",
            "let done: exec\nin go: exec\non go { for i in 0..3 { } await done }",
            &[Lacks("WS079")],
        ),
        (
            "`continue` in a loop",
            "var t: int = 0\nin go: exec\non go { for i in 0..3 { if i == 1 { continue } t += i } }",
            &[Silent],
        ),
        (
            "`continue` outside a loop",
            "in go: exec\non go { continue }",
            &[Codes(&["WS078"])],
        ),
        // A mod body is checked where it is declared, outside any loop.
        (
            "`continue` in a mod called from a loop",
            "mod skip() { continue }\nin go: exec\non go { for i in 0..3 { skip() } }",
            &[Has("WS078")],
        ),
        // `v` is the element (int), not the index; adding it to an int checks.
        (
            "an array's single binding is the value",
            "var arr: int[]\nvar t: int = 0\nin go: exec\non go { for v in arr { t += v } }",
            &[Silent],
        ),
        (
            "a map's single binding is the key",
            "var scores: Map<string, int>\nvar s: string = \"\"\nin go: exec\non go { for k in scores { s = k } }",
            &[Silent],
        ),
        (
            "a well-formed range loop",
            "var t: int = 0\nin go: exec\non go { for i in 0..10 { t += i } }",
            &[Silent],
        ),
    ];
    check_rows(rows);
}

#[test]
fn a_namespace_in_an_assignment_target_keeps_its_type() {
    // `u.xs[1] = 2` types `u` as a read of `u.xs` does, so hover shows it.
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: [("lib.ws".to_string(), "var xs: int[] = [1, 2]".to_string())]
            .into_iter()
            .collect(),
    };
    let src = "import * as u from \"lib\"\nin go: exec\non go { u.xs[1] = 2 }";
    let resolved = resolve(src, "main", &loader);
    let r = typecheck(
        &resolved.ast,
        "main",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let at = src.find("u.xs").unwrap();
    assert!(
        r.type_of_expr
            .keys()
            .any(|(f, s, e)| &**f == "main" && *s == at && *e == at + 1),
        "no type recorded for `u`"
    );
}

/// A `getType()` test narrows the `any` it tests: in a `match` arm and in
/// an `if ... is Type.X` block it reads as that kind; outside, and in the
/// `_` arm, it is still `any`.
#[test]
fn get_type_narrows_the_tested_any() {
    let src = "in e: entity\nin go: exec\nvar n: int = 0\nvar f: float = 0.0\nvar s: string = \"\"\non go {\n  \
                   let p = e.GetCustomProperty(\"k\")\n  match p.Value.getType() {\n    Int => { n += p.Value }\n    \
                   String => { s = p.Value }\n    _ => { s = p.Value }\n  }\n  \
                   if p.Value.getType() is Type.Float {\n    f = p.Value\n  }\n  s = p.Value\n}";
    let r = tc(src);
    assert_no_diags(&r);
    let reads: Vec<Type> = src
        .match_indices("p.Value")
        .filter(|(at, _)| !src[at + "p.Value".len()..].starts_with(".getType"))
        .map(|(at, m)| {
            r.type_of_expr
                .iter()
                .find(|((_, s, e), _)| *s == at && *e == at + m.len())
                .map(|(_, t)| t.clone())
                .expect("p.Value is typed")
        })
        .collect();
    assert_eq!(
        reads,
        [
            Type::Int,
            Type::String,
            Type::Opaque,
            Type::Float,
            Type::Opaque
        ]
    );
}

/// An `if ... then ... else` value narrows its `then` branch, and a `let`
/// that shadows the tested name inside an arm is its own, unnarrowed value.
#[test]
fn get_type_narrowing_covers_if_values_and_skips_shadows() {
    let src = "in a: any\nin b: any\nin go: exec\nvar n: int = 0\nvar s: string = \"\"\non go {\n  \
                   n = if a.getType() is Type.Int then a else 0\n  match a.getType() {\n    \
                   String => {\n      let a = b\n      s = a\n    }\n    _ => {}\n  }\n}";
    let r = tc(src);
    assert_no_diags(&r);
    let type_at = |needle: &str| {
        let at = src.find(needle).unwrap() + needle.len() - 1;
        r.type_of_expr
            .iter()
            .find(|((_, s, e), _)| *s == at && *e == at + 1)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| panic!("{needle:?} is typed"))
    };
    assert_eq!(
        type_at("then a"),
        Type::Int,
        "the then-branch reads `a` as the kind tested"
    );
    assert_eq!(
        type_at("s = a"),
        Type::Opaque,
        "the shadowing `let a` is not the tested `a`"
    );
}
