//! Reachability proof for every wirescript diagnostic code: one trigger
//! program per code, asserting the code actually fires end-to-end.
//!
//! Each `reach!` test compiles a small `.ws` snippet through the full pipeline
//! (parse, resolve, typecheck, lower, cycle-analyze) and asserts the expected
//! `WSxxx` code is present in the resulting diagnostics. This is a
//! reachability net, not a behavior spec: it proves a code can fire, not
//! everything about when.
//!
//! `WS054` and `WS065` are proven by their tests in `diagnostic_regressions.rs`.
//!
//! Deliberate gaps (not reachable, so no tests):
//! - `WS009`, `WS018` - reserved codes, never assigned by any emit site in the
//!   compiler.
//! - `WS034` - a retired generic-chip guard.
//! - `WS015` - a retired `fn`-deprecation warning.
//! - `WS029` - a retired custom-event-param annotation lint; a slot's type is
//!   inferred from an in-unit sender (WS042 when none is).
//! - `WS094` - an internal compiler error that no program should reach; its
//!   check is exercised directly by
//!   `lower::tests::regression_bugs::a_stranded_wire_is_an_internal_error`.
//!
//! Two codes (`WS012`, `WS014`) are resolve-phase and fire only on multi-file
//! programs (imports), so they can't go through the single-source `reach!`
//! helper; they use `resolve()` with a `MemLoader` directly (see
//! `resolve_phase` below).

use crate::common;

/// A test named `$name` asserting each source's full compile reports `$code`.
/// Every source that misses it is listed in the failure.
macro_rules! reach {
    ($name:ident, [$($src:expr),+ $(,)?], $code:literal) => {
        #[test]
        fn $name() {
            let failures: Vec<String> = [$($src),+]
                .iter()
                .filter_map(|src| {
                    let codes = common::compile_diag_codes(src, "ws_reachability_test.ws");
                    (!codes.iter().any(|c| c == $code)).then(|| format!("{src:?} gave {codes:?}"))
                })
                .collect();
            assert!(failures.is_empty(), "expected {}:\n{}", $code, failures.join("\n"));
        }
    };
    ($name:ident, $src:expr, $code:literal) => {
        reach!($name, [$src], $code);
    };
}

// ---------------------------------------------------------------------
// WS001 - unknown event/trigger name on an `on` handler [typecheck]
reach!(ws001_is_reachable, "on Foo {\n}\n", "WS001");

// WS002 - unknown type name in a type annotation [typecheck]
reach!(ws002_is_reachable, "var x: Bogus = 0\n", "WS002");

// WS003 - type mismatch on assignment (vector -> int, inside exec) [typecheck]
// NOTE: bool -> int is an automatic coercion (CoerceRule::Coerce), not a
// mismatch; a vector RHS is an unconditional mismatch (numeric<->vector never
// coerces).
reach!(
    ws003_is_reachable,
    "var x: int = 0\nin start: exec\non start {\n  x = Vec(1.0, 2.0, 3.0)\n}\n",
    "WS003"
);

// WS004 - no operator overload for a binary op on the given operand types [typecheck]
reach!(ws004_is_reachable, "let x = \"a\" - \"b\"\n", "WS004");

// WS005 - wire-graph cycle with no tick barrier (Buffer/Queue) [lower/analyze]
reach!(
    ws005_is_reachable,
    "in run: exec\non run {\n  let loop: exec\n  emit loop\n  await loop\n  emit loop\n}\n",
    "WS005"
);

// WS006 - `*x` deref used outside an exec context (top-level `let` is pure) [typecheck]
reach!(ws006_is_reachable, "var x: int = 0\nlet y = *x\n", "WS006");

// WS007 - exec-only builtin called at top level with no `exec = ...` override [typecheck]
reach!(
    ws007_is_reachable,
    "BroadcastChatMessage(\"hi\")\n",
    "WS007"
);

// WS008 - `&` of a non-reference (temporary), not a variable/ref param/array element [typecheck]
reach!(
    ws008_is_reachable,
    "in go: exec\non go { let r = &5 }\n",
    "WS008"
);

// (WS009 intentionally absent - see top-of-file note.)

// WS010 - unknown field access on a record type [typecheck]
reach!(
    ws010_is_reachable,
    "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet q = p.z\n",
    "WS010"
);

// WS011 - builtin call arity mismatch (SetLocation needs 2 args) [typecheck]
reach!(
    ws011_is_reachable,
    "in e: entity\non RoundStart() { SetLocation(e) }\n",
    "WS011"
);

// (WS012 - RESOLVE-phase, multi-file; see the loader-based test below.)

// WS013 - duplicate top-level declaration [typecheck]
reach!(
    ws013_is_reachable,
    "var x: int = 0\nvar x: int = 0\n",
    "WS013"
);

// (WS014 - RESOLVE-phase, multi-file; see the loader-based test below.)

// WS015 - RETIRED: the `fn`-deprecation warning is never emitted (see the
// top-of-file gaps note).

// WS016 - `let` type-annotation mismatch (string -> int; the reverse int -> string
// coerces via ViaString and does NOT fire) [typecheck]
reach!(
    ws016_is_reachable,
    "var s: string = \"hi\"\nlet n: int = s\n",
    "WS016"
);

// WS017 - ambiguous untyped `out` inferring its type from a var [typecheck, warning]
reach!(
    ws017_is_reachable,
    "var myVar: int = 0\nout o = myVar\n",
    "WS017"
);

// (WS018 intentionally absent - see top-of-file note.)

// WS019 - a `$` prefab reference must end in `.brz` [typecheck]
reach!(ws019_is_reachable, "let x = $./level\n", "WS019");

// WS020 - recursive chip/mod call (self- or mutually-recursive) [lower]
reach!(
    ws020_is_reachable,
    "mod foo() -> int { return foo() }\nout result = foo()\n",
    "WS020"
);

// WS021 - call to a chip/mod before its declaration (source-order registration)
// [lower + typecheck double-emit; may appear more than once, assert contains not count]
reach!(
    ws021_is_reachable,
    "in z: exec\nmod caller() { let x = target(1) BroadcastChatMessage(\"${x}\") }\non z { caller() }\nmod target(n: int) -> int { return n + 1 }\n",
    "WS021"
);

// WS022 - user mod/chip call arity mismatch [typecheck]
reach!(
    ws022_is_reachable,
    "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }\n",
    "WS022"
);

// WS023 - `@side` (@left/@right/@top/@bottom) annotation on a non-root port
// (only top-level ports of the compiled file may carry one) [lower]
reach!(ws023_is_reachable, "chip { @left in a: exec }\n", "WS023");

// WS024 - asset/prefab reference inlined into an array initializer [typecheck, warning]
reach!(
    ws024_is_reachable,
    "var songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]\n",
    "WS024"
);

// WS025 - a zone/teleport/prefab reference or an exec edge can't be stored in a var [typecheck]
reach!(ws025_is_reachable, "var c: exec\n", "WS025");

// WS026 - a map literal used outside a Map var's init/assignment position [typecheck]
reach!(ws026_is_reachable, "let x = { 1 => 2 }\n", "WS026");

// WS027 - whole-map assignment from a non-literal (`m = m2`); no copy-by-assign gate
// exists, use `.copyFrom(src)` instead [lower]
reach!(
    ws027_is_reachable,
    "var m: Map<int, int>\nvar m2: Map<int, int>\nin t: exec\non t {\n  m = m2\n}\n",
    "WS027"
);

// WS028 - bad enum member name for a config (non-wire) builtin parameter [typecheck];
// a constant setting its gate's data field cannot hold [lower]; a float constant
// no `int` holds, in an `int` place [typecheck]
reach!(
    ws028_is_reachable,
    [
        "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bogus)\n",
        "var n: int = 0\nin go: exec\non go { await SleepTicks(_, delay = 1e30)\n n = 1 }\nout o: int = n\n",
        "var n: int = 1e30\nout o: int = n\n",
    ],
    "WS028"
);

// WS042 - untyped CustomEvent handler param with no in-unit sender to infer its
// type from (defaults to float on emit) [typecheck, warning].
reach!(
    ws042_is_reachable,
    "static var n: int = 0\non CustomEvent(\"dmg\") -> (amount) {\n  n = n + 1\n}\n",
    "WS042"
);

// WS030 - custom-event sender/receiver data-type mismatch (needs both a typed
// receiver and a mismatched-type sender in the same unit) [typecheck, warning]
reach!(
    ws030_is_reachable,
    "in go: exec\nin fv: float\nvar last: int = 0\non CustomEvent(\"dmg\") -> (amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", fv)\n}\n",
    "WS030"
);

// WS045 - a custom-event data arg with no concrete type (`any` / `Opaque(...)`),
// which leaves the send port untyped so it emits the float variant and cannot
// match a receiver that declares a real type [typecheck, warning]. Unlike WS030
// this needs NO in-unit receiver: the untyped send is wrong on its own, and the
// receiver is usually in another file (a spawned chip) where WS030 is blind.
reach!(
    ws045_is_reachable,
    "in go: exec
var who: character
on go {
  SendGlobalCustomEvent(\"a\", who, Opaque(true))
}
",
    "WS045"
);

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
    let tc = wirescript::typecheck_with_inference(&resolved.ast).0;
    assert!(
        tc.diagnostics
            .iter()
            .any(|d| d.code == "WS045" && matches!(d.severity, wirescript::Severity::Warning)),
        "expected a WS045 warning from typecheck_with_inference: {:?}",
        tc.diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}

// WS045 must not fire when every data arg carries a concrete type, or it would
// flag the spelling it steers people toward.
#[test]
fn ws045_quiet_when_typed() {
    let src = "in go: exec
var who: character
var flag: bool = true
on go {
  SendGlobalCustomEvent(\"a\", who, flag)
}
";
    assert!(
        !common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS045".to_string())
    );
}

// WS046 - a `const` binding whose initializer cannot be evaluated at compile
// time (a runtime value, here an `in` port) [typecheck]
reach!(
    ws046_is_reachable,
    "in live: int\nmod f() { const n = live + 1 }\n",
    "WS046"
);

// WS047 - a `const` binding whose initializer IS a compile-time constant, but
// the certified fold evaluator declines to compute it (here, `ToUpper` on a
// non-ASCII string operand is not certified by the in-game probe) [typecheck]
reach!(
    ws047_is_reachable,
    "mod f() { const s = \"café\".ToUpper() }\n",
    "WS047"
);

// WS048 - const-mod call-chain depth budget exceeded. A `const mod` that calls
// itself declares and type-checks fine: WS020 (`lower/call.rs`) only guards
// recursion through lowering's wire-expansion of an ordinary call
// (`lower_chip_call`), and a call resolved through `ConstCtx::lookup_mod`
// bypasses it, so only the interpreter's `Budget` depth counter stops the
// recursion. Reached like WS046/WS047 above: `f(1)` sits in `SendCustomEvent`'s
// constant-only channel-name position, which forces const evaluation.
// [typecheck]
reach!(
    ws048_is_reachable,
    "const mod f(n: int) -> string { return f(n) }\n\
     mod ping(v: int) { SendCustomEvent(f(1), v) }\n",
    "WS048"
);

// WS031 - a reference-only value (zone / teleport / prefab) used as an
// if-then-else branch (Select routes a value, not a reference) [typecheck]
// NOTE: a `*T` ref (or an array/map ref, or a `brick`) routes through Select
// unchanged (measured), so it does not trip this; only the reference-only types
// (which carry no value a Select could route) do.
reach!(
    ws031_is_reachable,
    "in z1: zone\nin z2: zone\nin c: bool\nlet y = if c then z1 else z2\n",
    "WS031"
);

// WS032 - a type annotation resolves to `any` [typecheck, warning]
// on a mod parameter or output; an `in`/`out` port takes no type parameter, so
// it does not warn.
#[test]
fn ws032_is_reachable() {
    let src = "mod f(a: any) -> any { return a }\nin x: int\nout o = f(x)\n";
    assert!(
        common::compile_diag_codes(src, "ws_reachability_test.ws").contains(&"WS032".to_string())
    );
    let port = "in x: any\n";
    assert!(
        !common::compile_diag_codes(port, "ws_reachability_test.ws").contains(&"WS032".to_string())
    );
}

// WS033 - generic type-parameter inference produces a type outside its bound's mask
// (a string arg against a `T: Numeric` bound) [typecheck]
reach!(
    ws033_is_reachable,
    "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\nlet x = onlyNumeric(\"hi\")\n",
    "WS033"
);

// (WS034 intentionally absent - see top-of-file note.)

// WS035 - a user `self`-receiver mod shadows a builtin (Dot) [typecheck]
reach!(
    ws035_is_reachable,
    "mod Dot(self: vector, o: vector) -> float { return 0.0 }\n",
    "WS035"
);

// WS036 - `.f()` receiver-syntax call on a user mod whose first param isn't `self` [typecheck]
reach!(
    ws036_is_reachable,
    "mod f(a: vector, o: vector) -> float { return a.Dot(o) }\nin v: vector\nin w: vector\nin go: exec\non go { let d = v.f(w) }\n",
    "WS036"
);

// WS037 - explicit `<...>` type arguments on a builtin call are ignored (its result
// type is derived from the arguments, not pinned) [typecheck, warning]
reach!(
    ws037_is_reachable,
    "in a: float\nin b: float\nout r: float = Blend<int>(a, b, 0.5)\n",
    "WS037"
);

// WS038 - calling a non-callable symbol (e.g. an array var mistaken for indexing) [typecheck]
reach!(
    ws038_is_reachable,
    "on CharacterSpawned() -> (ch) {\n  var xs: int[] = [1, 2, 3]\n  let r = xs(0)\n}\n",
    "WS038"
);

// WS039 - a Map key type other than int/string/entity/character/controller [typecheck]
reach!(ws039_is_reachable, "var d: Map<vector, int>\n", "WS039");

// WS040 - a `@label(...)` expression must be a compile-time constant (a literal or
// a constant `let`), not a runtime value like an `in` port [typecheck]
reach!(
    ws040_is_reachable,
    "in x: int\n@label(x) in y: int\n",
    "WS040"
);

// WS041 - a named argument that matches no parameter and no data-driven config
// field (a typo'd arg name on a gate call) [typecheck]
reach!(
    ws041_is_reachable,
    "in c: controller\nin go: exec\non go {\n  c.DisplayText(\"hi\", bogusArg = 0.0)\n}\n",
    "WS041"
);

// WS043 - a general (non-event) `on <call> -> <pattern>` expr trigger whose
// call has multiple data outputs but no exec-typed one to auto-extract
// (a 2-output mod called without `exec = ...`, so its result record is
// `{a, b}` with no `exec` field for `on` to trigger on) [lower]
reach!(
    ws043_is_reachable,
    "mod pair(x: int) -> (a: int, b: int) {\n  emit a = x\n  emit b = x\n}\non pair(5) -> (p, q) {\n}\n",
    "WS043"
);

// WS056 - `let v = await sig` on a signal that carries no payload [lower]
reach!(
    ws056_is_reachable,
    "let sig: exec\non start { emit sig }\non go { let v = await sig }\nin start: exec\nin go: exec\n",
    "WS056"
);

// WS057 - `emit` to a target that is neither an out port nor an exec signal [lower]
reach!(
    ws057_is_reachable,
    "in go: exec\non go { emit go }\n",
    "WS057"
);

// WS058 - an exec statement (an assignment) dropped in a pure position [lower]
reach!(
    ws058_is_reachable,
    "static var x: int = 0\nmod bump() { x = x + 1 }\nlet dummy = bump()\n",
    "WS058"
);

// WS059 - Change/Changed on a reference or container (no single value to watch) [typecheck]
reach!(
    ws059_is_reachable,
    "in m: Map<int, int>\nstatic var v: int = 0\non Change(m) { v = 1 }\n",
    "WS059"
);

// WS064 - duplicate discriminant value in an enum declaration [typecheck]
reach!(ws064_is_reachable, "enum E { A = 1, B = 1 }\n", "WS064");

// WS060 - unknown variant on an enum path (`Enum.Variant`) [typecheck]
reach!(
    ws060_is_reachable,
    "enum Shape { Empty }\nout d = Shape.Nope\n",
    "WS060"
);

// WS066 - `.Discriminant` / match on a non-enum type [typecheck]
reach!(
    ws066_is_reachable,
    "var x: int = 0\nout d = x.Discriminant\n",
    "WS066"
);

// WS062 - a `let ... else` whose `else` block does not diverge on every path
// (it can fall through instead of `return`/`emit`-ing out), which would leave
// the pattern's binding unavailable past the statement [typecheck]
reach!(
    ws062_is_reachable,
    "enum Opt { Some(int), None }\nin o: Opt\nmod f() -> int {\n  let Some(x) = o else { let y = 1 }\n  return x\n}\n",
    "WS062"
);

// WS063 - a generic enum's type parameter cannot be inferred (no payload to
// infer it from, and no annotation supplying it) [typecheck]
reach!(
    ws063_is_reachable,
    "enum Option<T> { Some(T), None }\nout n = Option.None\n",
    "WS063"
);

// WS061 - unreachable match arm (an earlier arm already covers it) [typecheck, warning]
reach!(
    ws061_is_reachable,
    "enum S { A, B }\nvar s: S = S.A\nout x = match s { _ => 0, A => 1 }\n",
    "WS061"
);

// WSP001 - lexer error: unexpected character [parse/lexer]
reach!(wsp001_is_reachable, "var x: int = 0\n\\\n", "WSP001");

// ---------------------------------------------------------------------
// WS012 / WS014 - RESOLVE-phase codes that only fire on a multi-file
// program (an `import` against a second file), so they can't go through
// the single-source `reach!` helper above. Use a `MemLoader` directly,
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
reach!(
    ws069_is_reachable,
    "enum E { A, B { xs: int[] } }\nout n = 1\n",
    "WS069"
);

// WS070 - an `unsafe` payload access missing its variant or field segment
// [typecheck]
reach!(
    ws070_is_reachable,
    "enum E { A, B { s: string } }\nvar e: E = E.B { s: \"h\" }\nin go: exec\non go { unsafe e.B = \"x\" }\n",
    "WS070"
);

// WS071 - a record value used where a single value is expected (it is several
// wires, so it cannot drive one port), or a choice between records holding a
// record array [lowering]
reach!(
    ws071_is_reachable,
    "type P = { x: int, y: int }
type H = { pts: P[], n: int }
var h: H
var h2: H
var o: int = 0
in go: exec
in c: bool
on go { let w = if c then h else h2
o = w.pts[0].y }
",
    "WS071"
);

// WS091 - a local shadows storage that a mod called in its scope uses
// [typecheck]
reach!(
    ws091_is_reachable,
    "var q: int = 0\nmod bump() { q = q + 1 }\nin go: exec\non go {\n  var q: int = 5\n  bump()\n}\n",
    "WS091"
);

// WS098 - an unannotated `var` initialized with an empty `[]` or `{}`
// [typecheck]
reach!(
    ws098_is_reachable,
    "var k: int = 0\nin go: exec\non go {\n  var x = []\n  x.push(k)\n}\n",
    "WS098"
);

// WS099 - `return <value>` in a top-level handler [typecheck]
reach!(
    ws099_is_reachable,
    "out o: int\nin go: exec\non go { return 1 }\n",
    "WS099"
);

// WS100 - a record type read as a value [typecheck]
reach!(
    ws100_is_reachable,
    "type Box = { v: int }\nvar n: int = 3\nvar r: int = 0\nin go: exec\non go {\n  let c = Box { v: n }\n  r = c.v\n}\n",
    "WS100"
);

// WS101 - an initializer on a new `let s: exec` signal [typecheck]
reach!(
    ws101_is_reachable,
    "var n: int = 0\nin go: exec\nlet s: exec = go\non s { n = 2 }\n",
    "WS101"
);

// WS102 - an `if`-expression arm calls a chip whose own handler acts [lowering]
reach!(
    ws102_is_reachable,
    "in go: exec\nin c: bool\nvar n: int = 0\nout onn: int = n\nvar cnt: int = 0\nout ocnt: int = cnt\n\
     chip Cx(t: exec, v: int) -> (r: int) {\non t {\ncnt = cnt + 1\nemit r = v\n}\n}\n\
     on go { n = if c then Cx(go, 3) else 0 }\n",
    "WS102"
);

// WS103 - an untyped output written with two types [typecheck]
reach!(
    ws103_is_reachable,
    "in go: exec\nin c: bool\non go {\n  if c { out z = 5 } else { out z = \"six\" }\n}\n",
    "WS103"
);

// WS104 - an integer constant a float cannot hold exactly, in a float place [typecheck]
reach!(
    ws104_is_reachable,
    "var x: float = 9007199254740993\nout ox: float = x\n",
    "WS104"
);

// WS105 - a record or tuple written to an output with no type [lower]
reach!(
    ws105_is_reachable,
    "type P = { a: int, b: float }\nvar pv: P = { a: 1, b: 2.0 }\nin go: exec\non go { out z = pv }\n",
    "WS105"
);

// WS106 - a constant stored as another value: a fraction dropped in an `int`
// place [typecheck], or a gate setting its field holds inexactly [lower]
reach!(
    ws106_is_reachable,
    [
        "var n: int = 2.5\nout o: int = n\n",
        "var n: int = 0\nin go: exec\non go { await SleepTicks(_, delay = 0.999)\n n = 1 }\nout o: int = n\n",
    ],
    "WS106"
);

// WS107 - a `type` or `enum` named like a builtin type, or `Array`/`Ref` with
// one type parameter [typecheck]
reach!(
    ws107_is_reachable,
    [
        "type float = int\nvar x: float = 1.5\nout ox: float = x\n",
        "type Ref<V> = V[]\nvar x: float = 1.5\nout ox: float = x\n",
    ],
    "WS107"
);

// WS108 - a top-level name declared again, read by position [typecheck, warning]
reach!(
    ws108_is_reachable,
    "in go: exec\nvar n: int = 0\nout o1: int = n\nconst k = 1\nconst k = 2\non go { n = k }\n",
    "WS108"
);
