//! Reachability proof for every wirescript diagnostic code: one trigger
//! program per code, asserting the code actually fires end-to-end.
//!
//! Each test compiles a small `.ws` snippet through the FULL pipeline
//! (parse -> resolve -> typecheck -> lower -> cycle-analyze) via
//! [`diags`] and asserts the expected `WSxxx` code is present in the
//! resulting diagnostics. This is a reachability net, not a behavior
//! spec — it only proves a code CAN fire, not everything about when.
//!
//! Deliberate gaps (do not add tests for these — they are not reachable):
//! - `WS009`, `WS018` — reserved codes, never assigned by any emit site in
//!   the compiler.
//! - `WS034` — a removed generic-chip guard; the code is retired.
//! - `WS015` — the `fn`-deprecation warning; `fn` is now removed (rejected at
//!   parse), so WS015 is retired.
//! - `WS029` — the old "always annotate a custom-event param" lint; replaced
//!   by inference from an in-unit sender (WS042 when none is inferable), so
//!   WS029 is retired.
//! - `WS094` - an internal compiler error that no program should reach; its
//!   check is exercised directly by
//!   `lower::tests::regression_bugs::a_stranded_wire_is_an_internal_error`.
//!
//! Two codes (`WS012`, `WS014`) are RESOLVE-phase and fire only on
//! multi-file programs (imports), so they can't go through the
//! single-source [`diags`] helper — they use `resolve()` with a
//! [`MemLoader`] directly instead (see the bottom of this file).


mod common;

// ---------------------------------------------------------------------
// WS001 - unknown event/trigger name on an `on` handler [typecheck]
#[test]
fn ws001_is_reachable() {
    let src = "on Foo {\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS001".to_string()));
}

// WS002 - unknown type name in a type annotation [typecheck]
#[test]
fn ws002_is_reachable() {
    let src = "var x: Bogus = 0\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS002".to_string()));
}

// WS003 - type mismatch on assignment (vector -> int, inside exec) [typecheck]
// NOTE: bool -> int is an automatic coercion (CoerceRule::Coerce), not a
// mismatch — a vector RHS is a genuine, unconditional mismatch
// (numeric<->vector never coerces).
#[test]
fn ws003_is_reachable() {
    let src = "var x: int = 0\nin start: exec\non start {\n  x = Vec(1.0, 2.0, 3.0)\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS003".to_string()));
}

// WS004 - no operator overload for a binary op on the given operand types [typecheck]
#[test]
fn ws004_is_reachable() {
    let src = "let x = \"a\" - \"b\"\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS004".to_string()));
}

// WS005 - wire-graph cycle with no tick barrier (Buffer/Queue) [lower/analyze]
#[test]
fn ws005_is_reachable() {
    let src = "in run: exec\non run {\n  let loop: exec\n  emit loop\n  await loop\n  emit loop\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS005".to_string()));
}

// WS006 - `*x` deref used outside an exec context (top-level `let` is pure) [typecheck]
#[test]
fn ws006_is_reachable() {
    let src = "var x: int = 0\nlet y = *x\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS006".to_string()));
}

// WS007 - exec-only builtin called at top level with no `exec = ...` override [typecheck]
#[test]
fn ws007_is_reachable() {
    let src = "BroadcastChatMessage(\"hi\")\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS007".to_string()));
}

// WS008 - `&` of a non-reference (temporary), not a variable/ref param/array element [typecheck]
#[test]
fn ws008_is_reachable() {
    let src = "in go: exec\non go { let r = &5 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS008".to_string()));
}

// (WS009 intentionally absent - see top-of-file note.)

// WS010 - unknown field access on a record type [typecheck]
#[test]
fn ws010_is_reachable() {
    let src = "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet q = p.z\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS010".to_string()));
}

// WS011 - builtin call arity mismatch (SetLocation needs 2 args) [typecheck]
#[test]
fn ws011_is_reachable() {
    let src = "in e: entity\non RoundStart() { SetLocation(e) }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS011".to_string()));
}

// (WS012 - RESOLVE-phase, multi-file; see the loader-based test below.)

// WS013 - duplicate top-level declaration [typecheck]
#[test]
fn ws013_is_reachable() {
    let src = "var x: int = 0\nvar x: int = 0\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS013".to_string()));
}

// (WS014 - RESOLVE-phase, multi-file; see the loader-based test below.)

// WS015 - RETIRED. `fn` has been removed (rejected at parse with a hard error),
// so the old "`fn` is deprecated" warning no longer exists. See the top-of-file
// gaps note. A `fn` decl now produces a parse error (covered by WSP001).

// WS016 - `let` type-annotation mismatch (string -> int; the reverse int -> string
// coerces via ViaString and does NOT fire) [typecheck]
#[test]
fn ws016_is_reachable() {
    let src = "var s: string = \"hi\"\nlet n: int = s\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS016".to_string()));
}

// WS017 - ambiguous untyped `out` inferring its type from a var [typecheck, warning]
#[test]
fn ws017_is_reachable() {
    let src = "var myVar: int = 0\nout o = myVar\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS017".to_string()));
}

// (WS018 intentionally absent - see top-of-file note.)

// WS019 - a `$` prefab reference must end in `.brz` [typecheck]
#[test]
fn ws019_is_reachable() {
    let src = "let x = $./level\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS019".to_string()));
}

// WS020 - recursive chip/mod call (self- or mutually-recursive) [lower]
#[test]
fn ws020_is_reachable() {
    let src = "mod foo() -> int { return foo() }\nout result = foo()\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS020".to_string()));
}

// WS021 - call to a chip/mod before its declaration (source-order registration)
// [lower + typecheck double-emit; may appear more than once, assert contains not count]
#[test]
fn ws021_is_reachable() {
    let src = "in z: exec\nmod caller() { let x = target(1) BroadcastChatMessage(\"${x}\") }\non z { caller() }\nmod target(n: int) -> int { return n + 1 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS021".to_string()));
}

// WS022 - user mod/chip call arity mismatch [typecheck]
#[test]
fn ws022_is_reachable() {
    let src = "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS022".to_string()));
}

// WS023 - `@side` (@left/@right/@top/@bottom) annotation on a non-root port
// (only top-level ports of the compiled file may carry one) [lower]
#[test]
fn ws023_is_reachable() {
    let src = "chip { @left in a: exec }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS023".to_string()));
}

// WS024 - asset/prefab reference inlined into an array initializer [typecheck, warning]
#[test]
fn ws024_is_reachable() {
    let src = "var songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS024".to_string()));
}

// WS025 - a zone/teleport/prefab reference or an exec edge can't be stored in a var [typecheck]
#[test]
fn ws025_is_reachable() {
    let src = "var c: exec\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS025".to_string()));
}

// WS026 - a map literal used outside a Map var's init/assignment position [typecheck]
#[test]
fn ws026_is_reachable() {
    let src = "let x = { 1 => 2 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS026".to_string()));
}

// WS027 - whole-map assignment from a non-literal (`m = m2`); no copy-by-assign gate
// exists, use `.copyFrom(src)` instead [lower]
#[test]
fn ws027_is_reachable() {
    let src = "var m: Map<int, int>\nvar m2: Map<int, int>\nin t: exec\non t {\n  m = m2\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS027".to_string()));
}

// WS028 - bad enum member name for a config (non-wire) builtin parameter [typecheck]
#[test]
fn ws028_is_reachable() {
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bogus)\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS028".to_string()));
    // A constant setting its gate's data field cannot hold [lower].
    let src = "var n: int = 0\nin go: exec\non go { await SleepTicks(_, delay = 1e30)\n n = 1 }\nout o: int = n\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS028".to_string()));
    // A float constant no `int` holds, in an `int` place [typecheck].
    let src = "var n: int = 1e30\nout o: int = n\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS028".to_string()));
}

// WS042 - untyped CustomEvent handler param with no in-unit sender to infer its
// type from (defaults to float on emit) [typecheck, warning]. Replaces the
// retired WS029 ("always annotate"), now that unannotated slots are inferred
// from a matching in-unit sender when one exists.
#[test]
fn ws042_is_reachable() {
    let src = "static var n: int = 0\non CustomEvent(\"dmg\") -> (amount) {\n  n = n + 1\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS042".to_string()));
}

// WS030 - custom-event sender/receiver data-type mismatch (needs both a typed
// receiver and a mismatched-type sender in the same unit) [typecheck, warning]
#[test]
fn ws030_is_reachable() {
    let src = "in go: exec\nin fv: float\nvar last: int = 0\non CustomEvent(\"dmg\") -> (amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", fv)\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS030".to_string()));
}

// WS045 - a custom-event data arg with no concrete type (`any` / `Opaque(...)`),
// which leaves the send port untyped so it emits the float variant and cannot
// match a receiver that declares a real type [typecheck, warning]. Unlike WS030
// this needs NO in-unit receiver: the untyped send is wrong on its own, and the
// receiver is usually in another file (a spawned chip) where WS030 is blind.
#[test]
fn ws045_is_reachable() {
    let src = "in go: exec
var who: character
on go {
  SendGlobalCustomEvent(\"a\", who, Opaque(true))
}
";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS045".to_string()));
}

// WS045 must reach the TYPECHECK-ONLY entry point, which is what `wirescript-check`
// and the LSP call (`check.rs` never lowers). A diagnostic that only appears in a
// full compile is invisible in the editor, which is where it is most useful.
#[test]
fn ws045_reaches_typecheck_only_path() {
    let src = "in go: exec
var who: character
on go {
  SendGlobalCustomEvent(\"a\", who, Opaque(true))
}
";
    let resolved = wirescript::resolve(src, "t.ws", &wirescript::FsLoader);
    let tc = wirescript::typecheck::typecheck_with_inference(&resolved.ast, "t.ws").0;
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS045"
            && matches!(d.severity, wirescript::Severity::Warning)),
        "expected a WS045 warning from typecheck_with_inference: {:?}",
        tc.diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}

// WS045 must NOT fire when every data arg carries a concrete type — otherwise it
// would flag the correct spelling it is trying to steer people toward.
#[test]
fn ws045_quiet_when_typed() {
    let src = "in go: exec
var who: character
var flag: bool = true
on go {
  SendGlobalCustomEvent(\"a\", who, flag)
}
";
    assert!(!common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS045".to_string()));
}

// WS046 - a `const` binding whose initializer cannot be evaluated at compile
// time (a runtime value, here an `in` port) [typecheck]
#[test]
fn ws046_is_reachable() {
    let src = "in live: int\nmod f() { const n = live + 1 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS046".to_string()));
}

// WS047 - a `const` binding whose initializer IS a compile-time constant, but
// the certified fold evaluator declines to compute it (here, `ToUpper` on a
// non-ASCII string operand was never certified by the in-game probe) [typecheck]
#[test]
fn ws047_is_reachable() {
    let src = "mod f() { const s = \"café\".ToUpper() }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS047".to_string()));
}

// WS048 - const-mod call-chain depth budget exceeded. A `const mod` that
// calls ITSELF is perfectly legal to declare and type-check — WS020
// (`lower/call.rs`) only guards recursion reached through LOWERING's
// wire-expansion of an ORDINARY call (`lower_chip_call`); a call resolved
// through `ConstCtx::lookup_mod` never goes through that function at all, so
// nothing but the interpreter's own `Budget` depth counter stops it from
// recursing forever. Reached here exactly like WS046/WS047 above: `f(1)`
// sits in `SendCustomEvent`'s constant-only channel-name position, which
// forces it to be const-evaluated. [typecheck]
#[test]
fn ws048_is_reachable() {
    let src = "const mod f(n: int) -> string { return f(n) }\n\
               mod ping(v: int) { SendCustomEvent(f(1), v) }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS048".to_string()));
}

// WS031 - a reference-only value (zone / teleport / prefab) used as an
// if-then-else branch (Select routes a value, not a reference) [typecheck]
// NOTE: a `*T` ref (or an array/map ref, or a `brick`) routes through Select
// unchanged (measured), so it does NOT trip this — only the reference-only
// types (which carry no value a Select could route) do.
#[test]
fn ws031_is_reachable() {
    let src = "in z1: zone\nin z2: zone\nin c: bool\nlet y = if c then z1 else z2\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS031".to_string()));
}

// WS032 - a type annotation resolves to `any` [typecheck, warning]
// on a mod parameter or output; an `in`/`out` port takes no type parameter, so
// it no longer warns.
#[test]
fn ws032_is_reachable() {
    let src = "mod f(a: any) -> any { return a }\nin x: int\nout o = f(x)\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS032".to_string()));
    let port = "in x: any\n";
    assert!(!common::compile_diag_codes(port, "ws_reachability_test.ws").contains(&"WS032".to_string()));
}

// WS033 - generic type-parameter inference produces a type outside its bound's mask
// (a string arg against a `T: Numeric` bound) [typecheck]
#[test]
fn ws033_is_reachable() {
    let src = "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\nlet x = onlyNumeric(\"hi\")\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS033".to_string()));
}

// (WS034 intentionally absent - see top-of-file note.)

// WS035 - a user `self`-receiver mod shadows a builtin (Dot) [typecheck]
#[test]
fn ws035_is_reachable() {
    let src = "mod Dot(self: vector, o: vector) -> float { return 0.0 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS035".to_string()));
}

// WS036 - `.f()` receiver-syntax call on a user mod whose first param isn't `self` [typecheck]
#[test]
fn ws036_is_reachable() {
    let src = "mod f(a: vector, o: vector) -> float { return a.Dot(o) }\nin v: vector\nin w: vector\nin go: exec\non go { let d = v.f(w) }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS036".to_string()));
}

// WS037 - explicit `<...>` type arguments on a builtin call are ignored (its result
// type is derived from the arguments, not pinned) [typecheck, warning]
#[test]
fn ws037_is_reachable() {
    let src = "in a: float\nin b: float\nout r: float = Blend<int>(a, b, 0.5)\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS037".to_string()));
}

// WS038 - calling a non-callable symbol (e.g. an array var mistaken for indexing) [typecheck]
#[test]
fn ws038_is_reachable() {
    let src = "on CharacterSpawned() -> (ch) {\n  var xs: int[] = [1, 2, 3]\n  let r = xs(0)\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS038".to_string()));
}

// WS039 - a Map key type other than int/string/entity/character/controller [typecheck]
#[test]
fn ws039_is_reachable() {
    let src = "var d: Map<vector, int>\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS039".to_string()));
}

// WS040 - a `@label(...)` expression must be a compile-time constant (a literal or
// a constant `let`), not a runtime value like an `in` port [typecheck]
#[test]
fn ws040_is_reachable() {
    let src = "in x: int\n@label(x) in y: int\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS040".to_string()));
}

// WS041 - a named argument that matches no parameter and no data-driven config
// field (a typo'd arg name on a gate call) [typecheck]
#[test]
fn ws041_is_reachable() {
    let src = "in c: controller\nin go: exec\non go {\n  c.DisplayText(\"hi\", bogusArg = 0.0)\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS041".to_string()));
}

// WS043 - a general (non-event) `on <call> -> <pattern>` expr trigger whose
// call has multiple data outputs but no exec-typed one to auto-extract
// (a 2-output mod called without `exec = ...`, so its result record is
// `{a, b}` with no `exec` field for `on` to trigger on) [lower]
#[test]
fn ws043_is_reachable() {
    let src = "mod pair(x: int) -> (a: int, b: int) {\n  emit a = x\n  emit b = x\n}\non pair(5) -> (p, q) {\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS043".to_string()));
}

// WS056 - `let v = await sig` on a signal that carries no payload [lower]
#[test]
fn ws056_is_reachable() {
    let src = "let sig: exec\non start { emit sig }\non go { let v = await sig }\nin start: exec\nin go: exec\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS056".to_string()));
}

// WS057 - `emit` to a target that is neither an out port nor an exec signal [lower]
#[test]
fn ws057_is_reachable() {
    let src = "in go: exec\non go { emit go }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS057".to_string()));
}

// WS058 - an exec statement (an assignment) dropped in a pure position [lower]
#[test]
fn ws058_is_reachable() {
    let src = "static var x: int = 0\nmod bump() { x = x + 1 }\nlet dummy = bump()\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS058".to_string()));
}

// WS059 - Change/Changed on a reference or container (no single value to watch) [typecheck]
#[test]
fn ws059_is_reachable() {
    let src = "in m: Map<int, int>\nstatic var v: int = 0\non Change(m) { v = 1 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS059".to_string()));
}

// WS064 - duplicate discriminant value in an enum declaration [typecheck]
#[test]
fn ws064_is_reachable() {
    let src = "enum E { A = 1, B = 1 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS064".to_string()));
}

// WS060 - unknown variant on an enum path (`Enum.Variant`) [typecheck]
#[test]
fn ws060_is_reachable() {
    let src = "enum Shape { Empty }\nout d = Shape.Nope\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS060".to_string()));
}

// WS065 - wrong bracket form for an enum variant's payload [typecheck]
#[test]
fn ws065_is_reachable() {
    let src = "enum S { Box { w: float } }\nout x = S.Box(1.0)\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS065".to_string()));
}

// WS066 - `.Discriminant` / match on a non-enum type [typecheck]
#[test]
fn ws066_is_reachable() {
    let src = "var x: int = 0\nout d = x.Discriminant\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS066".to_string()));
}

// WS062 - a `let ... else` whose `else` block does not diverge on every path
// (it can fall through instead of `return`/`emit`-ing out), which would leave
// the pattern's binding unavailable past the statement [typecheck]
#[test]
fn ws062_is_reachable() {
    let src = "enum Opt { Some(int), None }\nin o: Opt\nmod f() -> int {\n  let Some(x) = o else { let y = 1 }\n  return x\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS062".to_string()));
}

// WS063 - a generic enum's type parameter cannot be inferred (no payload to
// infer it from, and no annotation supplying it) [typecheck]
#[test]
fn ws063_is_reachable() {
    let src = "enum Option<T> { Some(T), None }\nout n = Option.None\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS063".to_string()));
}

// WS054 - non-exhaustive match (a scrutinee variant left uncovered) [typecheck]
#[test]
fn ws054_is_reachable() {
    let src = "enum S { A, B, C }\nvar s: S = S.A\nout x = match s { A => 1, B => 2 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS054".to_string()));
}

// WS061 - unreachable match arm (an earlier arm already covers it) [typecheck, warning]
#[test]
fn ws061_is_reachable() {
    let src = "enum S { A, B }\nvar s: S = S.A\nout x = match s { _ => 0, A => 1 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS061".to_string()));
}

// WSP001 - lexer error: unexpected character [parse/lexer]
#[test]
fn wsp001_is_reachable() {
    let src = "var x: int = 0\n\\\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WSP001".to_string()));
}

// ---------------------------------------------------------------------
// WS012 / WS014 - RESOLVE-phase codes that only fire on a multi-file
// program (an `import` against a second file), so they can't go through
// the single-source `common::compile_diag_codes(, "ws_reachability_test.ws")` helper above. Use a `MemLoader` directly,
// mirroring `crates/wirescript/src/typecheck/tests.rs`'s namespace-import
// tests.
mod resolve_phase {
    use wirescript::resolve::{MemLoader, resolve};

    // WS012 - a named import binding that doesn't exist in the target file
    #[test]
    fn ws012_is_reachable() {
        let loader = MemLoader {
            files: [("utils.ws".into(), "let existing = 1\n".into())]
                .into_iter()
                .collect(),
        };
        let resolved = resolve("import { nope } from \"utils\"", "main.ws", &loader);
        assert!(
            resolved.diagnostics.iter().any(|d| d.code == "WS012"),
            "expected WS012: {:?}",
            resolved.diagnostics
        );
    }

    // WS014 - a named import that is never referenced by the importing file [warning]
    #[test]
    fn ws014_is_reachable() {
        let loader = MemLoader {
            files: [(
                "utils.ws".into(),
                "mod clamp(v: int) -> int { return v }\n".into(),
            )]
            .into_iter()
            .collect(),
        };
        let resolved = resolve("import { clamp } from \"utils\"", "main.ws", &loader);
        assert!(
            resolved.diagnostics.iter().any(|d| d.code == "WS014"),
            "expected WS014: {:?}",
            resolved.diagnostics
        );
    }
}


// WS069 - an enum payload field that would need container storage (an array or
// a map), which no variant construction can fill [typecheck]
#[test]
fn ws069_is_reachable() {
    let src = "enum E { A, B { xs: int[] } }\nout n = 1\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS069".to_string()));
}

// WS070 - an `unsafe` payload access missing its variant or field segment
// [typecheck]
#[test]
fn ws070_is_reachable() {
    let src = "enum E { A, B { s: string } }\nvar e: E = E.B { s: \"h\" }\nin go: exec\non go { unsafe e.B = \"x\" }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS070".to_string()));
}

// WS071 - a record value used where a single value is expected (it is several
// wires, so it cannot drive one port), or a choice between records holding a
// record array [lowering]
#[test]
fn ws071_is_reachable() {
    let src = "type P = { x: int, y: int }
type H = { pts: P[], n: int }
var h: H
var h2: H
var o: int = 0
in go: exec
in c: bool
on go { let w = if c then h else h2
o = w.pts[0].y }
";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS071".to_string()));
}

// WS091 - a local shadows storage that a mod called in its scope uses
// [typecheck]
#[test]
fn ws091_is_reachable() {
    let src = "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  var q: int = 5\n  bump()\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS091".to_string()));
}

// WS098 - an unannotated `var` initialized with an empty `[]` or `{}`
// [typecheck]
#[test]
fn ws098_is_reachable() {
    let src = "var k: int = 0\nin go: exec\non go {\n  var x = []\n  x.push(k)\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS098".to_string()));
}

// WS099 - `return <value>` in a top-level handler [typecheck]
#[test]
fn ws099_is_reachable() {
    let src = "out o: int\nin go: exec\non go { return 1 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS099".to_string()));
}

// WS100 - a record type read as a value [typecheck]
#[test]
fn ws100_is_reachable() {
    let src = "type Box = { v: int }\nvar n: int = 3\nvar r: int = 0\nin go: exec\non go {\n  let c = Box { v: n }\n  r = c.v\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS100".to_string()));
}

// WS101 - an initializer on a new `let s: exec` signal [typecheck]
#[test]
fn ws101_is_reachable() {
    let src = "var n: int = 0\nin go: exec\nlet s: exec = go\non s { n = 2 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS101".to_string()));
}

// WS102 - an `if`-expression arm calls a chip whose own handler acts [lowering]
#[test]
fn ws102_is_reachable() {
    let src = "in go: exec\nin c: bool\nvar n: int = 0\nout onn: int = n\nvar cnt: int = 0\nout ocnt: int = cnt\n\
               chip Cx(t: exec, v: int) -> (r: int) {\non t {\ncnt = cnt + 1\nemit r = v\n}\n}\n\
               on go { n = if c then Cx(go, 3) else 0 }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS102".to_string()));
}

// WS103 - an untyped output written with two types [typecheck]
#[test]
fn ws103_is_reachable() {
    let src = "in go: exec\nin c: bool\non go {\n  if c { out z = 5 } else { out z = \"six\" }\n}\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS103".to_string()));
}

// WS104 - an integer constant a float cannot hold exactly, in a float place [typecheck]
#[test]
fn ws104_is_reachable() {
    let src = "var x: float = 9007199254740993\nout ox: float = x\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS104".to_string()));
}

// WS105 - a record or tuple written to an output with no type [lower]
#[test]
fn ws105_is_reachable() {
    let src = "type P = { a: int, b: float }\nvar pv: P = { a: 1, b: 2.0 }\nin go: exec\non go { out z = pv }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS105".to_string()));
}

// WS106 - a constant stored as another value: a fraction dropped in an `int`
// place [typecheck], or a gate setting its field holds inexactly [lower]
#[test]
fn ws106_is_reachable() {
    let src = "var n: int = 2.5\nout o: int = n\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS106".to_string()));
    let src = "var n: int = 0\nin go: exec\non go { await SleepTicks(_, delay = 0.999)\n n = 1 }\nout o: int = n\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS106".to_string()));
}

// WS107 - a `type` or `enum` named like a builtin type [typecheck]
#[test]
fn ws107_is_reachable() {
    let src = "type float = int\nvar x: float = 1.5\nout ox: float = x\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS107".to_string()));
    // `Array` or `Ref` with one type parameter.
    let src = "type Ref<V> = V[]\nvar x: float = 1.5\nout ox: float = x\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS107".to_string()));
}

// WS108 - a top-level name declared again, read by position [typecheck, warning]
#[test]
fn ws108_is_reachable() {
    let src = "in go: exec\nvar n: int = 0\nout o1: int = n\nconst k = 1\nconst k = 2\non go { n = k }\n";
    assert!(common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS108".to_string()));
}
