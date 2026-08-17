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
//!
//! Two codes (`WS012`, `WS014`) are RESOLVE-phase and fire only on
//! multi-file programs (imports), so they can't go through the
//! single-source [`diags`] helper — they use `resolve()` with a
//! [`MemLoader`] directly instead (see the bottom of this file).

use wirescript::{CompileError, CompileInput, FoldMode, compile};

/// Run a single-source program through the full compile pipeline
/// (parse -> resolve -> typecheck -> lower -> cycle-analyze) and return
/// every diagnostic code produced, across BOTH the success path
/// (`CompileResult::diagnostics`, which chains resolve+typecheck+lower+
/// cycle diagnostics) and the hard-error path (`CompileError::HasErrors`,
/// which is the same chained list filtered to `Severity::Error` — so
/// error-severity codes are still reachable through it; warning-severity
/// codes require the program to compile clean through emit).
///
/// Uses the crate's default (disk-backed) loader; every trigger program
/// here is single-source with no `import`, so the loader is never
/// actually invoked to read a file from disk.
fn diags(src: &str) -> Vec<String> {
    let input = CompileInput {
        source: src,
        file: "ws_reachability_test.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    };
    match compile(input) {
        Ok(result) => result.diagnostics.iter().map(|d| d.code.clone()).collect(),
        Err(CompileError::HasErrors(errors)) => errors.iter().map(|d| d.code.clone()).collect(),
        Err(CompileError::Emit(e)) => {
            panic!("unexpected emit error for src={src:?}: {e:?}")
        }
    }
}

// ---------------------------------------------------------------------
// WS001 - unknown event/trigger name on an `on` handler [typecheck]
#[test]
fn ws001_is_reachable() {
    let src = "on Foo {\n}\n";
    assert!(diags(src).contains(&"WS001".to_string()));
}

// WS002 - unknown type name in a type annotation [typecheck]
#[test]
fn ws002_is_reachable() {
    let src = "var x: Bogus = 0\n";
    assert!(diags(src).contains(&"WS002".to_string()));
}

// WS003 - type mismatch on assignment (vector -> int, inside exec) [typecheck]
// NOTE: a bool RHS was tried first, but bool -> int is an automatic
// coercion (CoerceRule::Coerce), not a mismatch — a vector RHS is a
// genuine, unconditional mismatch (numeric<->vector never coerces).
#[test]
fn ws003_is_reachable() {
    let src = "var x: int = 0\nin start: exec\non start {\n  x = Vec(1.0, 2.0, 3.0)\n}\n";
    assert!(diags(src).contains(&"WS003".to_string()));
}

// WS004 - no operator overload for a binary op on the given operand types [typecheck]
#[test]
fn ws004_is_reachable() {
    let src = "let x = \"a\" - \"b\"\n";
    assert!(diags(src).contains(&"WS004".to_string()));
}

// WS005 - wire-graph cycle with no tick barrier (Buffer/Queue) [lower/analyze]
#[test]
fn ws005_is_reachable() {
    let src = "in run: exec\non run {\n  let loop: exec\n  emit loop\n  await loop\n  emit loop\n}\n";
    assert!(diags(src).contains(&"WS005".to_string()));
}

// WS006 - `*x` deref used outside an exec context (top-level `let` is pure) [typecheck]
#[test]
fn ws006_is_reachable() {
    let src = "var x: int = 0\nlet y = *x\n";
    assert!(diags(src).contains(&"WS006".to_string()));
}

// WS007 - exec-only builtin called at top level with no `exec = ...` override [typecheck]
#[test]
fn ws007_is_reachable() {
    let src = "BroadcastChatMessage(\"hi\")\n";
    assert!(diags(src).contains(&"WS007".to_string()));
}

// WS008 - `&` of a non-reference (temporary), not a variable/ref param/array element [typecheck]
#[test]
fn ws008_is_reachable() {
    let src = "in go: exec\non go { let r = &5 }\n";
    assert!(diags(src).contains(&"WS008".to_string()));
}

// (WS009 intentionally absent - see top-of-file note.)

// WS010 - unknown field access on a record type [typecheck]
#[test]
fn ws010_is_reachable() {
    let src = "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet q = p.z\n";
    assert!(diags(src).contains(&"WS010".to_string()));
}

// WS011 - builtin call arity mismatch (SetLocation needs 2 args) [typecheck]
#[test]
fn ws011_is_reachable() {
    let src = "in e: entity\non RoundStart() { SetLocation(e) }\n";
    assert!(diags(src).contains(&"WS011".to_string()));
}

// (WS012 - RESOLVE-phase, multi-file; see the loader-based test below.)

// WS013 - duplicate top-level declaration [typecheck]
#[test]
fn ws013_is_reachable() {
    let src = "var x: int = 0\nvar x: int = 0\n";
    assert!(diags(src).contains(&"WS013".to_string()));
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
    assert!(diags(src).contains(&"WS016".to_string()));
}

// WS017 - ambiguous untyped `out` inferring its type from a var [typecheck, warning]
#[test]
fn ws017_is_reachable() {
    let src = "var myVar: int = 0\nout o = myVar\n";
    assert!(diags(src).contains(&"WS017".to_string()));
}

// (WS018 intentionally absent - see top-of-file note.)

// WS019 - a `$` prefab reference must end in `.brz` [typecheck]
#[test]
fn ws019_is_reachable() {
    let src = "let x = $./level\n";
    assert!(diags(src).contains(&"WS019".to_string()));
}

// WS020 - recursive chip/mod call (self- or mutually-recursive) [lower]
#[test]
fn ws020_is_reachable() {
    let src = "mod foo() -> int { return foo() }\nout result = foo()\n";
    assert!(diags(src).contains(&"WS020".to_string()));
}

// WS021 - call to a chip/mod before its declaration (source-order registration)
// [lower + typecheck double-emit; may appear more than once, assert contains not count]
#[test]
fn ws021_is_reachable() {
    let src = "in z: exec\nmod caller() { let x = target(1) BroadcastChatMessage(\"${x}\") }\non z { caller() }\nmod target(n: int) -> int { return n + 1 }\n";
    assert!(diags(src).contains(&"WS021".to_string()));
}

// WS022 - user mod/chip call arity mismatch [typecheck]
#[test]
fn ws022_is_reachable() {
    let src = "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }\n";
    assert!(diags(src).contains(&"WS022".to_string()));
}

// WS023 - `@side` (@left/@right/@top/@bottom) annotation on a non-root port
// (only top-level ports of the compiled file may carry one) [lower]
#[test]
fn ws023_is_reachable() {
    let src = "chip { @left in a: exec }\n";
    assert!(diags(src).contains(&"WS023".to_string()));
}

// WS024 - asset/prefab reference inlined into an array initializer [typecheck, warning]
#[test]
fn ws024_is_reachable() {
    let src = "var songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]\n";
    assert!(diags(src).contains(&"WS024".to_string()));
}

// WS025 - `any` (or a zone/teleport rerouter-only reference) can't be stored in a var [typecheck]
#[test]
fn ws025_is_reachable() {
    let src = "var v: any = 0\n";
    assert!(diags(src).contains(&"WS025".to_string()));
}

// WS026 - a map literal used outside a Map var's init/assignment position [typecheck]
#[test]
fn ws026_is_reachable() {
    let src = "let x = { 1 => 2 }\n";
    assert!(diags(src).contains(&"WS026".to_string()));
}

// WS027 - whole-map assignment from a non-literal (`m = m2`); no copy-by-assign gate
// exists, use `.copyFrom(src)` instead [lower]
#[test]
fn ws027_is_reachable() {
    let src = "var m: Map<int, int>\nvar m2: Map<int, int>\nin t: exec\non t {\n  m = m2\n}\n";
    assert!(diags(src).contains(&"WS027".to_string()));
}

// WS028 - bad enum member name for a config (non-wire) builtin parameter [typecheck]
#[test]
fn ws028_is_reachable() {
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bogus)\n";
    assert!(diags(src).contains(&"WS028".to_string()));
}

// WS042 - untyped CustomEvent handler param with no in-unit sender to infer its
// type from (defaults to float on emit) [typecheck, warning]. Replaces the
// retired WS029 ("always annotate"), now that unannotated slots are inferred
// from a matching in-unit sender when one exists.
#[test]
fn ws042_is_reachable() {
    let src = "static var n: int = 0\non CustomEvent(\"dmg\") -> (amount) {\n  n = n + 1\n}\n";
    assert!(diags(src).contains(&"WS042".to_string()));
}

// WS030 - custom-event sender/receiver data-type mismatch (needs both a typed
// receiver and a mismatched-type sender in the same unit) [typecheck, warning]
#[test]
fn ws030_is_reachable() {
    let src = "in go: exec\nvar last: int = 0\non CustomEvent(\"dmg\") -> (amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", 1.5)\n}\n";
    assert!(diags(src).contains(&"WS030".to_string()));
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
    assert!(diags(src).contains(&"WS045".to_string()));
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
    assert!(!diags(src).contains(&"WS045".to_string()));
}

// WS046 - a `const` binding whose initializer cannot be evaluated at compile
// time (a runtime value, here an `in` port) [typecheck]
#[test]
fn ws046_is_reachable() {
    let src = "in live: int\nmod f() { const n = live + 1 }\n";
    assert!(diags(src).contains(&"WS046".to_string()));
}

// WS047 - a `const` binding whose initializer IS a compile-time constant, but
// the certified fold evaluator declines to compute it (here, `ToUpper` on a
// non-ASCII string operand was never certified by the in-game probe) [typecheck]
#[test]
fn ws047_is_reachable() {
    let src = "mod f() { const s = \"café\".ToUpper() }\n";
    assert!(diags(src).contains(&"WS047".to_string()));
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
    assert!(diags(src).contains(&"WS048".to_string()));
}

// WS031 - a reference (var ref / zone / teleport) used as an if-then-else branch
// (Select routes a value, not a reference) [typecheck]
// NOTE: a bare `*int` param reads back auto-dereferenced (its symbol is
// registered SymbolKind::Var, and an Ident read of a Var unwraps its Ref) —
// so passing it straight into an if-branch never types as `Ref` and can't
// trip this check. `&x` (an explicit ref-of) DOES infer as `Type::Ref`, so
// it's the trigger that actually produces a reference-typed branch.
#[test]
fn ws031_is_reachable() {
    let src = "var x: int = 0\nin c: bool\nlet y = if c then &x else 0\n";
    assert!(diags(src).contains(&"WS031".to_string()));
}

// WS032 - a type annotation resolves to `any` [typecheck, warning]
#[test]
fn ws032_is_reachable() {
    let src = "in x: any\n";
    assert!(diags(src).contains(&"WS032".to_string()));
}

// WS033 - generic type-parameter inference produces a type outside its bound's mask
// (a string arg against a `T: Numeric` bound) [typecheck]
#[test]
fn ws033_is_reachable() {
    let src = "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\nlet x = onlyNumeric(\"hi\")\n";
    assert!(diags(src).contains(&"WS033".to_string()));
}

// (WS034 intentionally absent - see top-of-file note.)

// WS035 - a user `self`-receiver mod shadows a builtin (Dot) [typecheck]
#[test]
fn ws035_is_reachable() {
    let src = "mod Dot(self: vector, o: vector) -> float { return 0.0 }\n";
    assert!(diags(src).contains(&"WS035".to_string()));
}

// WS036 - `.f()` receiver-syntax call on a user mod whose first param isn't `self` [typecheck]
#[test]
fn ws036_is_reachable() {
    let src = "mod f(a: vector, o: vector) -> float { return a.Dot(o) }\nin v: vector\nin w: vector\nin go: exec\non go { let d = v.f(w) }\n";
    assert!(diags(src).contains(&"WS036".to_string()));
}

// WS037 - explicit `<...>` type arguments on a builtin call are ignored (its result
// type is derived from the arguments, not pinned) [typecheck, warning]
#[test]
fn ws037_is_reachable() {
    let src = "in a: float\nin b: float\nout r: float = Blend<int>(a, b, 0.5)\n";
    assert!(diags(src).contains(&"WS037".to_string()));
}

// WS038 - calling a non-callable symbol (e.g. an array var mistaken for indexing) [typecheck]
#[test]
fn ws038_is_reachable() {
    let src = "on CharacterSpawned() -> (ch) {\n  var xs: int[] = [1, 2, 3]\n  let r = xs(0)\n}\n";
    assert!(diags(src).contains(&"WS038".to_string()));
}

// WS039 - a Map key type other than int/string/entity/character/controller [typecheck]
#[test]
fn ws039_is_reachable() {
    let src = "var d: Map<vector, int>\n";
    assert!(diags(src).contains(&"WS039".to_string()));
}

// WS040 - a `@label(...)` expression must be a compile-time constant (a literal or
// a constant `let`), not a runtime value like an `in` port [typecheck]
#[test]
fn ws040_is_reachable() {
    let src = "in x: int\n@label(x) in y: int\n";
    assert!(diags(src).contains(&"WS040".to_string()));
}

// WS041 - a named argument that matches no parameter and no data-driven config
// field (a typo'd arg name on a gate call) [typecheck]
#[test]
fn ws041_is_reachable() {
    let src = "in c: controller\nin go: exec\non go {\n  c.DisplayText(\"hi\", bogusArg = 0.0)\n}\n";
    assert!(diags(src).contains(&"WS041".to_string()));
}

// WS043 - a general (non-event) `on <call> -> <pattern>` expr trigger whose
// call has multiple data outputs but no exec-typed one to auto-extract
// (a 2-output mod called without `exec = ...`, so its result record is
// `{a, b}` with no `exec` field for `on` to trigger on) [lower]
#[test]
fn ws043_is_reachable() {
    let src = "mod pair(x: int) -> (a: int, b: int) {\n  emit a = x\n  emit b = x\n}\non pair(5) -> (p, q) {\n}\n";
    assert!(diags(src).contains(&"WS043".to_string()));
}

// WSP001 - lexer error: unexpected character [parse/lexer]
#[test]
fn wsp001_is_reachable() {
    let src = "var x: int = 0\n\\\n";
    assert!(diags(src).contains(&"WSP001".to_string()));
}

// ---------------------------------------------------------------------
// WS012 / WS014 - RESOLVE-phase codes that only fire on a multi-file
// program (an `import` against a second file), so they can't go through
// the single-source `diags()` helper above. Use a `MemLoader` directly,
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

