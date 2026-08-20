//! Compiles every `.ws` fixture in `tests/fixtures/const/` and asserts the
//! const half of each differential actually evaluated away.
//!
//! The fixtures themselves prove const and runtime agree on VALUES — that is
//! checked in game, where the real gates run. What they cannot prove is that
//! the const path vanished: a `const mod` that silently fell back to emitting
//! gates would still compute the right answer and still pass. That is what
//! this test is for.
use std::sync::Arc;

use wirescript::intern::intern;
use wirescript::ir::{gate_class, Literal, Module};
use wirescript::lower::{lower, FoldMode, LowerInput};
use wirescript::template_cache::TemplateCache;
use wirescript::typecheck::typecheck;
use wirescript::{resolve, FsLoader, Severity};

fn count(m: &Module) -> usize {
    m.nodes.len() + m.chips.values().map(count).sum::<usize>()
}

/// Resolve, typecheck and lower `source`, asserting no errors at any stage.
///
/// Folding is FORCED OFF, so any gate that disappears did so because const
/// evaluation removed it, not because the optimizer did.
fn lower_ok(source: &str, file: &str) -> Module {
    let resolved = resolve(source, file, &FsLoader);
    assert!(
        resolved.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "resolve errors in {file}: {:?}", resolved.diagnostics
    );
    let tc = typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default());
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors in {file}: {:?}", tc.diagnostics
    );
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::ForceOff,
        ce_slots: &wirescript::typecheck::CeSlotMap::default(),
    });
    assert!(
        lowered.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "lower errors in {file}: {:?}", lowered.diagnostics
    );
    lowered.module
}

/// Gate count of `source` — see [`lower_ok`] for the fold-mode guarantee.
fn gates(source: &str, file: &str) -> usize {
    count(&lower_ok(source, file))
}

/// Error codes `source` produces up to and including typecheck. Used for the
/// negative half of a differential, where the point is that the program is
/// REJECTED — so it must not go through [`lower_ok`], which panics on errors.
fn error_codes(source: &str, file: &str) -> Vec<String> {
    let resolved = resolve(source, file, &FsLoader);
    let tc = typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default());
    resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code.clone())
        .collect()
}

/// The baked `EventName` of every `SendCustomEvent` node in `m` and its nested
/// chip modules. `EventName` is a data-struct field rather than a pre-interned
/// `sym::` constant, so it is interned on the fly exactly as
/// `lower/call/builtin.rs` does when baking it.
fn event_names(m: &Module) -> Vec<String> {
    let mut out: Vec<String> = m
        .nodes
        .values()
        .filter(|n| n.gate_class == gate_class::PSEUDO_SEND_CUSTOM_EVENT)
        .filter_map(|n| match n.properties.get(&intern("EventName")) {
            Some(Literal::String(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    for chip in m.chips.values() {
        out.extend(event_names(chip));
    }
    out
}

/// Every fixture must compile cleanly end to end.
#[test]
fn every_const_fixture_compiles() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/const");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).expect("fixtures/const must exist") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("ws") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        gates(&src, path.to_str().unwrap());
        seen += 1;
    }
    assert!(seen > 0, "no fixtures found — the suite must not silently pass empty");
}

/// Every fixture compiles with no diagnostics AT ALL, warnings included.
///
/// [`every_const_fixture_compiles`] only pins the absence of ERRORS, which a
/// WSP001 "IR lowering not yet supported — emitted placeholder" slips straight
/// past: `module_index.ws` emitted one on its `const t = [10, 20, 30]` line on
/// every single compile, beside an orphan gate emit writes no component for. A
/// warning is the only signal a partially-lowered construct gives before it
/// reaches the game, so the fixture set holds itself to zero of them.
#[test]
fn every_const_fixture_compiles_without_warnings() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/const");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).expect("fixtures/const must exist") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("ws") {
            continue;
        }
        let file = path.to_str().unwrap();
        let src = std::fs::read_to_string(&path).unwrap();
        let resolved = resolve(&src, file, &FsLoader);
        let tc = typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default());
        let lowered = lower(LowerInput {
            ast: &resolved.ast,
            type_of_expr: &tc.type_of_expr,
            op_resolutions: &tc.op_resolutions,
            file,
            module_name: None,
            template_cache: Arc::new(TemplateCache::new()),
            doc_comments: &resolved.doc_comments,
            fold_mode: FoldMode::ForceOff,
            ce_slots: &wirescript::typecheck::CeSlotMap::default(),
        });
        let diagnostics: Vec<String> = resolved
            .diagnostics
            .iter()
            .chain(tc.diagnostics.iter())
            .chain(lowered.diagnostics.iter())
            .map(|d| format!("{:?} [{}] {}", d.severity, d.code, d.message))
            .collect();
        assert!(
            diagnostics.is_empty(),
            "{file} must compile with no diagnostics, got {diagnostics:?}"
        );
        seen += 1;
    }
    assert!(seen > 0, "no fixtures found — the suite must not silently pass empty");
}

/// The discriminating property of this whole feature: a const parameter
/// carries a literal into a position that REQUIRES one, which no ordinary
/// parameter can reach because a parameter is a wire.
///
/// `SendCustomEvent`'s channel name is the sharpest case — it is gate CONFIG,
/// not a wire port, so it can only be baked into the component data. With
/// `name: const string` the call site's `"died"` reaches it and bakes; with a
/// plain `name: string` the same body is REJECTED (WS028), because a wire
/// cannot drive a non-wireable config port.
///
/// This test fails if const params stop working, in either direction: degrade
/// `const string` to `string` and the first half dies on WS028 inside
/// `lower_ok`; keep it type-checking but lose the bake and the `EventName`
/// assertion goes empty. Verified by doing exactly that — see the task report.
#[test]
fn a_const_param_carries_a_literal_into_gate_config_where_a_plain_param_cannot() {
    let m = lower_ok(
        "mod ping(name: const string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"died\", hp) }",
        "const_param.ws",
    );
    assert_eq!(
        event_names(&m),
        vec!["died".to_string()],
        "the const param must bake as the channel name"
    );

    // The identical mod with an ordinary parameter cannot express this at all.
    let codes = error_codes(
        "mod ping(name: string, v: int) { SendCustomEvent(name, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(\"died\", hp) }",
        "plain_param.ws",
    );
    assert!(
        codes.contains(&"WS028".to_string()),
        "a plain param feeding a config port must be rejected (WS028), got {codes:?}"
    );
}

/// A `const` binding folds into a literal-accepting position (a `var`
/// initializer) instead of emitting the gates the runtime form needs.
///
/// NOT const-specific proof, and must not be read as any: swapping `const` for
/// a plain `let` here produces the IDENTICAL 2 gates, because `let`-into-var
/// -initializer folding predates this feature entirely (see
/// `src/lower/tests/const_init.rs`, e.g. `bare_named_constant_bakes`). This
/// test therefore passes with or without Tasks 1-7. It is kept only as a
/// regression guard that adding `const` did not BREAK the pre-existing
/// folding, and as a smoke test of the `gates`/`count` machinery above. The
/// real const-specific guard is
/// `a_const_param_carries_a_literal_into_gate_config_where_a_plain_param_cannot`.
#[test]
fn a_const_binding_folds_into_a_literal_position() {
    let c = gates("const OUT = 1 << 4\nvar sink: int = OUT", "const.ws");
    let r = gates(
        "let OUT = 1 << 4\nvar sink: int = 0\non RoundStart() { sink = OUT }",
        "runtime.ws",
    );
    assert!(c < r, "const form ({c} gates) must emit fewer than the runtime form ({r})");
}

/// The headline claim of the whole feature: a CALL to a `const mod` — not
/// just a `const` binding or a `const` param's literal argument, but the
/// CALL ITSELF, resolved through `ConstCtx::lookup_mod` (Task 13) — costs
/// ZERO gates, because `interp::eval_call` walks the callee's body in plain
/// Rust and never touches `ctx.builder`/`add_gate`. `ping`'s `name` param is
/// `const string` (Task 6), so `evtName("died")` sits in a position that
/// FORCES it to be evaluated at compile time (a plain wire argument there
/// is WS046, per `a_const_parameter_rejects_a_runtime_argument` in
/// `typecheck/tests.rs`).
///
/// `via_call` (calling `evtName`) is checked against TWO baselines:
/// - `via_literal`: the byte-identical program with the call replaced by
///   the string it evaluates to. Equal gate counts is the strongest form of
///   "zero gates for the call" — if `evtName`'s call left even one gate
///   behind (e.g. its `"evt_" .. kind` body's `Concatenate` gate, or a
///   microchip-instance boundary), `via_call` would exceed `via_literal`.
/// - `ordinary`: the IDENTICAL body (`ordName`, `"evt_" .. kind`) called
///   from an ORDINARY (non-const) position instead — the one shape that CAN
///   take either a literal or a computed value, so it's the only fair
///   apples-to-apples "does this get folded" comparison. Unlike `evtName`,
///   `ordName`'s call has no reason to fold (nothing there requires a
///   constant), so it lowers normally: several more gates than `via_call`'s
///   zero, for the SAME computation.
///
/// Regression note for anyone tempted to "simplify" this test: an earlier
/// draft of the Task 13 implementation passed `via_call` at 5 gates (matching
/// `ordinary`) — `lower_chip_call_inline` was ALSO unconditionally lowering
/// every `const` parameter's argument as an ordinary wire (`lower_expr`)
/// before the const value it computed ever got consulted, leaving `evtName`'s
/// fully-expanded (but never wired to anything) body as 3 orphaned gates.
/// Only `ping`'s param being `const` (not `evtName` itself) is what a caller
/// can control here — see `is_const` in that function for the fix.
#[test]
fn a_const_mod_call_in_a_const_position_emits_zero_gates() {
    let via_call = gates(
        "const mod evtName(kind: string) -> string { return \"evt_\" .. kind }\n\
         mod ping(name: const string) { BroadcastChatMessage(name) }\n\
         in go: exec\non go { ping(evtName(\"died\")) }",
        "via_call.ws",
    );
    let via_literal = gates(
        "mod ping(name: const string) { BroadcastChatMessage(name) }\n\
         in go: exec\non go { ping(\"evt_died\") }",
        "via_literal.ws",
    );
    assert_eq!(
        via_call, via_literal,
        "a const-mod call in a const position must cost ZERO extra gates over \
         writing the literal directly (got {via_call} vs {via_literal} gates)"
    );

    let ordinary = gates(
        "mod ordName(kind: string) -> string { return \"evt_\" .. kind }\n\
         mod ping2(name: string) { BroadcastChatMessage(name) }\n\
         in go: exec\non go { ping2(ordName(\"died\")) }",
        "ordinary.ws",
    );
    assert!(
        via_call < ordinary,
        "a const mod call ({via_call} gates) must emit strictly fewer gates than \
         the identical ordinary mod's call from a non-const position ({ordinary} gates)"
    );
}

/// A top-level `const` binding whose value comes from a `const mod` CALL must
/// itself be usable everywhere an ordinary top-level constant is: `CHANNEL`
/// below feeds `SendCustomEvent`'s `eventName` — constant-only gate config,
/// not a wire port — from a plain reference to the name, with no `const`
/// parameter of its own in `ping` to force evaluation. This only works if
/// `CHANNEL`'s value actually lands in the SAME constant environment
/// lowering reads when it bakes the gate — checked here by reading the real
/// gate's `EventName` property, not merely by the absence of diagnostics (an
/// earlier task in this feature shipped a silent miscompile — a program that
/// type-checked cleanly while lowering silently dropped the value — that a
/// diagnostics-only assertion would not have caught).
#[test]
fn a_const_binding_from_a_const_mod_call_bakes_downstream() {
    let m = lower_ok(
        "const mod evtName(kind: string) -> string { return \"evt_\" .. kind }\n\
         const CHANNEL = evtName(\"died\")\n\
         mod ping(v: int) { SendCustomEvent(CHANNEL, v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(hp) }",
        "const_from_call.ws",
    );
    assert_eq!(
        event_names(&m),
        vec!["evt_died".to_string()],
        "CHANNEL must resolve through the const-mod call and bake as the literal channel name"
    );
}

/// A constant defined from a const-mod call is itself usable in ANOTHER
/// `const` binding's expression — `B` reads `A`, and `A`'s own value only
/// exists because `f(1)` was evaluated. Proven the same way as the test
/// above: by reading the baked `EventName`s the two constants actually
/// produce, not just the absence of diagnostics.
#[test]
fn a_const_from_a_const_mod_call_is_usable_in_another_const_binding() {
    let m = lower_ok(
        "const mod f(n: int) -> int { return n + 1 }\n\
         const A = f(1)\n\
         const B = A + 1\n\
         mod ping(v: int) { SendCustomEvent(\"ch${A}-${B}\", v) }\n\
         in go: exec\n\
         var hp: int = 0\n\
         on go { ping(hp) }",
        "const_chain_from_call.ws",
    );
    assert_eq!(
        event_names(&m),
        vec!["ch2-3".to_string()],
        "A = f(1) = 2 and B = A + 1 = 3 must both resolve through the const-mod call"
    );
}

/// Pins the boundary of "declaration order does not matter": it does not
/// extend to a call site preceding the CALLEE MOD's own declaration. That is
/// WS021 — "chips and mods must be declared before the point where they are
/// used" — a pre-existing rule that applies to every call (const or not),
/// unrelated to this fix and unaffected by it: `build_const_env` resolves a
/// top-level `const mod` from the WHOLE decls list regardless of position
/// (see the next test), but the program below is rejected before that
/// tolerance would ever matter, because the call to `evtName` — a plain
/// mod call, checked the same way regardless of whether its result ends up
/// feeding a `const` — textually precedes `evtName`'s declaration.
#[test]
fn a_const_mod_call_must_still_be_declared_before_its_use() {
    let codes = error_codes(
        "const CHANNEL = evtName(\"died\")\n\
         const mod evtName(kind: string) -> string { return \"evt_\" .. kind }\n",
        "const_before_decl.ws",
    );
    assert!(
        codes.contains(&"WS021".to_string()),
        "a call to a not-yet-declared mod must still be rejected, const or not: {codes:?}"
    );
}

/// The array/map analog of `a_const_mod_call_in_a_const_position_emits_zero_gates`
/// above, for `fixtures/const/arrays.ws`'s own shape: a const-mod call PER
/// ELEMENT of an array literal, itself in a const-required position
/// (`checkInt`'s `cv: const int`), via `.length()`/`[i]` on the literal.
/// `via_call` (the array literal calling `size`) is checked against
/// `via_literal` (the byte-identical program with each call replaced by the
/// int it evaluates to) — equal gate counts is the strongest form of "zero
/// gates for the array + its calls": if baking left so much as an
/// `ArrayVar_Push`/`MathMultiply`/`ArrayVar_Get` gate behind, `via_call`
/// would exceed `via_literal`.
#[test]
fn a_const_mod_call_per_array_element_in_a_const_position_emits_zero_gates() {
    let via_call = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         const mod size(n: int) -> int { return n * 10 }\n\
         in go: exec\n\
         on go { ping([size(1), size(2), size(3)][1]) }",
        "via_call.ws",
    );
    let via_literal = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { ping([10, 20, 30][1]) }",
        "via_literal.ws",
    );
    assert_eq!(
        via_call, via_literal,
        "a const-mod call per array element, indexed, in a const position must cost \
         ZERO extra gates over writing the already-evaluated literals directly \
         (got {via_call} vs {via_literal} gates)"
    );
}

/// `fixtures/const/compound.ws`'s own claim: a const-mod call nested inside
/// an OPERATOR, in a const-required position, costs zero gates — not just
/// the standalone call `a_const_mod_call_in_a_const_position_emits_zero_gates`
/// already covers. `via_call` (`double(3) + 1`, itself `ping`'s `cv: const
/// int` argument) is checked against `via_literal` (the byte-identical
/// program with the whole expression replaced by `7`, the value it
/// evaluates to) — equal gate counts is the strongest form of "zero gates
/// for the nested call": before `eval_expr` grew a `BinOp` arm, this exact
/// program was rejected outright (WS046), not merely gate-costly, so this
/// also doubles as an end-to-end regression guard for that fix.
#[test]
fn a_const_mod_call_nested_in_an_operator_in_a_const_position_emits_zero_gates() {
    let via_call = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         const mod double(n: int) -> int { return n * 2 }\n\
         in go: exec\n\
         on go { ping(double(3) + 1) }",
        "via_call.ws",
    );
    let via_literal = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { ping(7) }",
        "via_literal.ws",
    );
    assert_eq!(
        via_call, via_literal,
        "a const-mod call nested in an operator, in a const position, must cost \
         ZERO extra gates over writing the already-evaluated literal directly \
         (got {via_call} vs {via_literal} gates)"
    );
}

/// The unary-operator and constructor-argument shapes of the same claim,
/// pinned together since they share one `ping`/`double` setup. `negSix`
/// nests the call under unary `-`; `v` nests it as a `Vec` argument, mixed
/// with a NAMED argument (`z = ...`) that is not the call's own axis, so a
/// binding-by-position regression (see
/// `const_eval::expr::constructor_named_arguments_bind_by_name_not_by_position`)
/// would also show up here as a wrong-but-still-zero-gate value rather than
/// an extra gate — this test only proves the gate COUNT, not the value; the
/// value is `fixtures/const/compound.ws`'s job, checked in game.
#[test]
fn a_const_mod_call_nested_in_a_unary_operator_or_constructor_argument_emits_zero_gates() {
    let via_call = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         mod pingV(v: const vector) { BroadcastChatMessage(\"${v}\") }\n\
         const mod double(n: int) -> int { return n * 2 }\n\
         in go: exec\n\
         on go {\n\
           ping(-double(3))\n\
           pingV(Vec(z = double(1), x = 2.0, y = 3.0))\n\
         }",
        "via_call.ws",
    );
    let via_literal = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         mod pingV(v: const vector) { BroadcastChatMessage(\"${v}\") }\n\
         in go: exec\n\
         on go {\n\
           ping(-6)\n\
           pingV(Vec(2.0, 3.0, 2.0))\n\
         }",
        "via_literal.ws",
    );
    assert_eq!(
        via_call, via_literal,
        "a const-mod call nested in a unary operator or a NAMED constructor \
         argument, in a const position, must cost ZERO extra gates over writing \
         the already-evaluated literals directly (got {via_call} vs {via_literal} gates)"
    );
}

/// `fixtures/const/destructure.ws`'s own claim: a `const { .. } = ..` record
/// destructure, in a const-required position, costs zero gates. `via_call`
/// builds a record via a `const mod` call and destructures it (plain +
/// alias + rest, matching the fixture); `via_literal` is the byte-identical
/// program with each destructured name replaced by the int it evaluates to.
/// Equal gate counts is the strongest form of "zero gates for the
/// destructure" — before this task, ANY destructuring `const` was rejected
/// outright (WS046), so this also doubles as an end-to-end regression guard.
#[test]
fn a_const_record_destructure_in_a_const_position_emits_zero_gates() {
    let via_call = gates(
        "type Point = { x: int, y: int, z: int }\n\
         const mod mkPoint() -> Point { return { x: 1, y: 2, z: 3 } }\n\
         mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         const p = mkPoint()\n\
         const { x, y } = p\n\
         const { x: alias } = p\n\
         const { x: bx, ...rest } = p\n\
         in go: exec\n\
         on go { ping(x + y + alias + bx + rest.y + rest.z) }",
        "via_call.ws",
    );
    let via_literal = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { ping(1 + 2 + 1 + 1 + 2 + 3) }",
        "via_literal.ws",
    );
    assert_eq!(
        via_call, via_literal,
        "a const record destructure (plain + alias + rest), in a const position, \
         must cost ZERO extra gates over writing the already-evaluated literals \
         directly (got {via_call} vs {via_literal} gates)"
    );
}

/// The multi-output analog of the destructure test above:
/// `fixtures/const/destructure.ws`'s `const { a, b } = pairC(2)`.
///
/// A MULTI-OUTPUT `const mod` evaluates to a `Literal::Record`, and
/// `const { a, b } = pair(2)` is the only way to bind its outputs — so it has
/// to reach `lower::decl`'s const-mod skip path just like the single-output
/// spelling does, or the body lowers as ordinary gates. It did not, and the
/// cost was not a rounding error: `out a = "x" .. "y"` / `out b = "p" .. "q"`
/// emitted SIX dead `Concatenate` gates, a body mutating a compile-time
/// collection failed outright with WS044, and one whose `out` sat inside a
/// const `if` failed with WS002 "no field ... available fields: ...". So this
/// asserts the same ZERO-GATE property every other differential in this file
/// asserts, against a pure-literal baseline.
///
/// Not covered here, deliberately: binding the whole result to one name
/// (`const dummy = pair(2)`) still lowers the call. That spelling stores a
/// `Literal::Record` under a single name, which `lower::decl`'s gate 3
/// excludes from the skip path because baking it early-returns past the
/// `Binding::Record` its later field reads resolve through — see that gate's
/// own comment.
///
/// The gate count alone cannot tell a real gate from an `_Unsupported`
/// placeholder, and "zero gates" is exactly what a silently-dropped constant
/// also looks like, so the second half asserts the VALUES that reached the
/// graph: two `const mod` string outputs, built from the mod's own const
/// parameter, baked into `SendCustomEvent`'s non-wireable channel name.
#[test]
fn a_multi_output_const_mod_destructure_emits_zero_gates() {
    let via_destructure = gates(
        "const mod pair(n: const int) -> (a: int, b: int) {\n\
           out b = n + 1\n\
           out a = n\n\
         }\n\
         mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         const { a, b } = pair(2)\n\
         in go: exec\n\
         on go { ping(a + b) }",
        "via_destructure.ws",
    );
    let via_literal = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { ping(2 + 3) }",
        "via_literal.ws",
    );
    assert_eq!(
        via_destructure, via_literal,
        "destructuring a MULTI-output const-mod call's result, in a const \
         position, must cost ZERO extra gates over writing the already-evaluated \
         literals directly (got {via_destructure} vs {via_literal} gates)"
    );

    let module = lower_ok(
        "const mod pairC(n: const int) -> (a: string, b: string) {\n\
           out a = \"x${n}\"\n\
           out b = \"y${n}\"\n\
         }\n\
         mod send(chan: const string) { SendCustomEvent(chan, 1) }\n\
         const { a, b } = pairC(2)\n\
         in go: exec\n\
         on go { send(a) send(b) }",
        "via_destructure_values.ws",
    );
    let mut names = event_names(&module);
    names.sort();
    assert_eq!(
        names,
        vec!["x2".to_string(), "y2".to_string()],
        "each output of a multi-output const mod must reach the graph as its own \
         evaluated value — an empty or defaulted channel name here means the \
         constant was dropped rather than baked"
    );
}

/// `fixtures/const/module_index.ws`'s own claim: a MODULE-LEVEL `const`
/// array, indexed at module scope (`const z = t[1]`), costs zero gates in
/// both channels a constant is consumed through — a baked initializer
/// (`counts`) and a live wire operand (`ping`'s `cv: const int` argument).
///
/// Unlike this file's other differentials, both sides declare the same
/// unused `const t = [10, 20, 30]`, rather than `via_literal` omitting it.
/// That symmetry was originally needed because a bare top-level collection
/// `const` was partially lowered to an `_Unsupported` placeholder whether or
/// not anything indexed it, and holding that fixed cost on both sides was the
/// only way to isolate the claim. It now costs ZERO gates on both sides (a
/// `const` container materializes only where something reads it at runtime,
/// and nothing here does), so the symmetry is no longer load-bearing — it is
/// kept because keeping the two programs otherwise identical is what makes
/// the comparison mean "indexing `t` costs nothing beyond declaring it".
#[test]
fn a_module_level_const_array_index_adds_no_gates_over_declaring_the_array() {
    const DECL: &str = "const t = [10, 20, 30]\n\
       mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n";
    let via_index = gates(
        &format!(
            "{DECL}const z = t[1]\nvar counts: int[] = [z, 12345]\nin go: exec\non go {{ ping(z) }}"
        ),
        "via_index.ws",
    );
    let via_literal = gates(
        &format!(
            "{DECL}var counts: int[] = [20, 12345]\nin go: exec\non go {{ ping(20) }}"
        ),
        "via_literal.ws",
    );
    assert_eq!(
        via_index, via_literal,
        "indexing a module-level const array, used as both a baked initializer and \
         a wire operand, must cost no MORE gates than declaring the array alone and \
         writing the already-evaluated literals directly \
         (got {via_index} vs {via_literal} gates)"
    );
}

// The other half of "declaration order does not matter" — a constant whose
// own initializer NAMES a later-declared constant (`const B = A + 1` before
// `const A`'s own declaration) — is pinned in
// `lower::tests::const_init::constant_chain_through_a_const_mod_call_resolves_regardless_of_order`,
// against `build_const_env`'s baked VALUE directly (via `baked_array`), not
// through this file's stricter `lower_ok` (which additionally demands zero
// typecheck errors end to end). That distinction matters: a top-level `let`'s
// OWN initializer expression resolves an out-of-order sibling name through
// ordinary scope lookup (`infer::infer`), which — for EVERY top-level
// `let`/`const`, with or without a const-mod call anywhere in sight — is
// strictly declaration-order (`register_decl` does not pre-declare `let`
// names; `check_decl_inner` binds each one into scope only as it is reached).
// `build_const_env`'s fixpoint is a SEPARATE, order-independent computation
// of the same names' VALUES for baking elsewhere (`var`/array initializers,
// labels, …); it was never a promise that the `let` chain itself
// type-checks with zero errors out of order, and this fix does not change
// that pre-existing split — confirmed against the plain (no const mod
// involved) form of the same pattern before writing the const-mod version.

/// The `fixtures/const/assembly.ws` analog of
/// `a_const_mod_call_per_array_element_in_a_const_position_emits_zero_gates`
/// above: a `const mod` that ASSEMBLES its array conditionally — `if`+`push`,
/// not a single array literal — still costs ZERO gates when the call sits in
/// a const-required position, because `if`-tree-shaking (Task 15) and the
/// mutation interpreter (this task) both run in plain Rust inside
/// `interp::eval_call` and never touch `ctx.builder`.
///
/// `via_call` (calling `rooms`, whose body conditionally `push`es) is checked
/// against `via_literal` (the byte-identical program with the call replaced
/// by the array it evaluates to for `n = 2`) — equal gate counts is the
/// strongest form of "zero gates for the conditional assembly": if either the
/// `if`s or the `push`es left so much as one `ArrayVar_Push`/`Branch` gate
/// behind, `via_call` would exceed `via_literal`.
#[test]
fn a_const_mod_conditionally_assembling_an_array_via_push_emits_zero_gates() {
    let via_call = gates(
        "const mod rooms(n: int) -> int[] {\n\
           const t = [0]\n\
           t.clear()\n\
           if n >= 1 { t.push(10) }\n\
           if n >= 2 { t.push(20) }\n\
           return t\n\
         }\n\
         mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { ping(rooms(2)[1]) }",
        "via_call.ws",
    );
    let via_literal = gates(
        "mod ping(name: const int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { ping([10, 20][1]) }",
        "via_literal.ws",
    );
    assert_eq!(
        via_call, via_literal,
        "a const mod conditionally assembling an array with `if` + `push` in a \
         const position must cost ZERO extra gates over writing the \
         already-evaluated literal directly (got {via_call} vs {via_literal} gates)"
    );

    // Baseline: the IDENTICAL conditional-assembly body (`if`s and `push`es),
    // called from an ORDINARY (non-const) position instead — the one shape
    // that can take either a literal or a computed value, so it's the fair
    // apples-to-apples "does this get folded" comparison, same as
    // `a_const_mod_call_in_a_const_position_emits_zero_gates` above. Its
    // result has to land in a `var` before `[1]` can read it back (a plain
    // mod call's return value has no array storage of its own — unrelated to
    // const evaluation, see `assembly.ws`'s own comment on this).
    let ordinary = gates(
        "mod rooms2(n: int) -> int[] {\n\
           var t: int[]\n\
           if n >= 1 { t.push(10) }\n\
           if n >= 2 { t.push(20) }\n\
           return t\n\
         }\n\
         mod ping2(name: int) { BroadcastChatMessage(\"${name}\") }\n\
         in go: exec\n\
         on go { var r: int[] = rooms2(2)\n ping2(r[1]) }",
        "ordinary.ws",
    );
    assert!(
        via_call < ordinary,
        "a const mod's conditional assembly ({via_call} gates) must emit strictly \
         fewer gates than the identical body called from a non-const position \
         ({ordinary} gates)"
    );
}

/// Every ERROR-severity diagnostic from all three stages, as `"CODE message"`.
/// Unlike [`error_codes`] this also runs LOWERING, because the diagnostics
/// these binding tests are about (WS044) are emitted there — and unlike
/// [`lower_ok`] it must not panic on them, since asserting their ABSENCE is
/// the whole point.
fn all_error_diags(source: &str, file: &str) -> Vec<String> {
    let resolved = resolve(source, file, &FsLoader);
    let tc = typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default());
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode: FoldMode::ForceOff,
        ce_slots: &wirescript::typecheck::CeSlotMap::default(),
    });
    resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .chain(lowered.diagnostics.iter())
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} {}", d.code, d.message))
        .collect()
}

/// A `const mod` that ASSEMBLES its result by mutating a `const` collection —
/// the shape this feature's Task 21 introduced, and the one with no valid
/// ordinary lowering (its `clear`/`push` target a `const` binding, which has
/// no array var for the runtime array gates to point at).
const MUTATING_CONST_MOD: &str = "const mod rooms(n: int) -> int[] {\n\
   const t = [0]\n\
   t.clear()\n\
   if n >= 1 { t.push(10) }\n\
   if n >= 2 { t.push(20) }\n\
   return t\n\
 }\n";

/// BINDING a mutating `const mod`'s result must be clean — the constant is
/// the whole result, so lowering must not ALSO lower the initializer as
/// ordinary code.
///
/// This is not merely an inconvenience: binding the result to a name is the
/// natural way to reuse one, so leaving it broken would make the mods Task 21
/// introduced unusable in exactly the spelling users reach for first.
///
/// All four binding forms are covered because they take different lowering
/// paths (`TopDecl::Let` vs `Stmt::Let`, and `const` vs plain `let` differ in
/// whether typecheck DEMANDS the evaluation succeed), and each previously
/// failed: 1 WS044 at module scope, 3 inside a handler.
#[test]
fn binding_a_mutating_const_mod_is_clean_at_every_scope() {
    for (label, src) in [
        ("module const", format!("{MUTATING_CONST_MOD}const r = rooms(2)\nvar sink: int[] = r")),
        ("module let", format!("{MUTATING_CONST_MOD}let r = rooms(2)\nvar sink: int[] = r")),
        (
            "handler const",
            format!("{MUTATING_CONST_MOD}in go: exec\non go {{ const r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"),
        ),
        (
            "handler let",
            format!("{MUTATING_CONST_MOD}in go: exec\non go {{ let r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"),
        ),
        // The form that was already clean before this fix (it never bound the
        // call to a name), kept as a control so a regression here is
        // distinguishable from one in the binding paths above.
        ("var initializer", format!("{MUTATING_CONST_MOD}var spawn: int[] = rooms(2)")),
    ] {
        let diags = all_error_diags(&src, "binding.ws");
        assert!(
            diags.is_empty(),
            "binding a mutating const mod ({label}) must be clean, got {diags:?}"
        );
    }
}

/// The containment that must NOT be weakened by the fix above: WS044 is a
/// genuinely useful diagnostic for a real dropped mutation, so it has to keep
/// firing everywhere a mutation is NOT confined to a const-mod body. The fix
/// works by not lowering an already-evaluated const initializer at all —
/// never by making the mutation methods themselves quieter — and these three
/// cases pin that distinction.
#[test]
fn ws044_still_fires_for_a_real_mutation_outside_a_const_mod_body() {
    for (label, src) in [
        (
            "a method call on a non-collection binding",
            "in go: exec\nlet notAnArray = 5\non go { notAnArray.push(1) }".to_string(),
        ),
        (
            "mutating a const mod's RESULT at a call site",
            format!("{MUTATING_CONST_MOD}in go: exec\non go {{ rooms(2).push(99) }}"),
        ),
        (
            "mutating a const BINDING from ordinary exec code",
            format!("{MUTATING_CONST_MOD}const r = rooms(2)\nin go: exec\non go {{ r.push(99) }}"),
        ),
    ] {
        let diags = all_error_diags(&src, "ws044.ws");
        assert!(
            diags.iter().any(|d| d.starts_with("WS044")),
            "WS044 must still fire for a real dropped mutation ({label}), got {diags:?}"
        );
    }
}

/// Lower `source` at an explicit fold mode and count gates. Mirrors `lower_ok`
/// but takes the mode, so a program can be measured under both.
fn gates_at(source: &str, file: &str, fold_mode: FoldMode) -> usize {
    let resolved = resolve(source, file, &FsLoader);
    let tc = typecheck(&resolved.ast, file, &wirescript::typecheck::CeSlotMap::default());
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
        fold_mode,
        ce_slots: &wirescript::typecheck::CeSlotMap::default(),
    });
    count(&lowered.module)
}

/// A `let` initializer that the FULL const evaluator can fold but the narrow
/// `expr_to_literal_in` cannot — string interpolation, an `if`-EXPRESSION —
/// must still lower to its real gates *by way of the const-mod-call gate*.
/// These programs contain no `const` keyword at all, so the const-mod-call
/// skip must not touch them.
///
/// This is the exact regression that made the const-mod-call gate necessary:
/// skipping ordinary lowering for EVERY initializer only the full evaluator
/// could answer silently elided the FormatText + MathAdd (4 gates -> 2) and
/// the Select (3 -> 2).
///
/// Measured with folding OFF, so a fold-pass elision can't be mistaken for a
/// const-mod one — `ForceOff` isolates the skip that this guards. The now-
/// default fold pass legitimately collapses these constant initializers under
/// `Auto` (that is the optimizer doing its job, not the bug here), so the
/// `@nofold` variants pull double duty: under `Auto` they must still keep
/// every gate, proving `@nofold` opts back out of the default fold.
#[test]
fn a_non_const_mod_initializer_keeps_its_gates() {
    // (label, source, expected gates with folding disabled, is `@nofold`)
    let cases: [(&str, String, usize, bool); 4] = [
        (
            "interpolation, @nofold",
            "in go: exec\non go { @nofold let s = \"a${1 + 1}b\"\n BroadcastChatMessage(s) }".to_string(),
            4,
            true,
        ),
        (
            "interpolation, plain",
            "in go: exec\non go { let s = \"a${1 + 1}b\"\n BroadcastChatMessage(s) }".to_string(),
            4,
            false,
        ),
        (
            "if-expression, @nofold",
            "@nofold let x = if true then 1 else 2\nout y = x * 2".to_string(),
            3,
            true,
        ),
        (
            "if-expression, plain",
            "let x = if true then 1 else 2\nout y = x * 2".to_string(),
            3,
            false,
        ),
    ];
    for (label, src, want, is_nofold) in cases {
        // Folding off: the const-mod-call skip must not fire, so every gate survives.
        let got = gates_at(&src, "nofold.ws", FoldMode::ForceOff);
        assert_eq!(
            got, want,
            "{label} under ForceOff must keep its {want} gates (a program with no \
             `const` in it must not be touched by the const-mod-call skip), got {got}"
        );
        // `@nofold` also opts back out of the now-default fold pass, so the
        // gates must survive `Auto` too.
        if is_nofold {
            let got_auto = gates_at(&src, "nofold.ws", FoldMode::Auto);
            assert_eq!(
                got_auto, want,
                "{label} under Auto must keep its {want} gates (@nofold disables the \
                 default fold), got {got_auto}"
            );
        }
    }
}

/// The other side of the same gate: an initializer that DOES require a
/// const-mod call is still skipped, `@nofold` or not. A mutating `const mod`
/// has no valid ordinary lowering to fall back to, so honoring `@nofold` here
/// would only restore the WS044 breakage under an attribute — see the
/// two-gate comment in `lower_let_decl`.
#[test]
fn a_const_mod_call_initializer_is_skipped_even_under_nofold() {
    for (label, src) in [
        (
            "plain",
            format!("{MUTATING_CONST_MOD}in go: exec\non go {{ let r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"),
        ),
        (
            "@nofold",
            format!("{MUTATING_CONST_MOD}in go: exec\non go {{ @nofold let r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"),
        ),
    ] {
        for mode in [FoldMode::ForceOff, FoldMode::Auto] {
            let diags = all_error_diags(&src, "nofold_constmod.ws");
            assert!(
                diags.is_empty(),
                "a const-mod-call initializer ({label}) must stay clean, got {diags:?}"
            );
            // 3 gates: the handler's own chain (MicrochipInput) + the
            // interpolation's FormatText + the BroadcastChatMessage, with
            // nothing left over from `rooms`' assembly.
            //
            // Was 4 until `r[0]` gained a compile-time lowering. The extra gate
            // was NOT part of `rooms`' result — it was the `_Unsupported`
            // placeholder `lower_index_access` synthesised for `r[0]`, which
            // emit writes no component for, so the interpolation slot's wire
            // died and the message printed 0 instead of 10. The count this test
            // asserted therefore encoded a silent miscompile, which is also why
            // it never matched the comment's own accounting above (which only
            // ever justified three gates). Verified via `read_components`: the
            // FormatText now bakes `InputA: 10i64`.
            assert_eq!(
                gates_at(&src, "nofold_constmod.ws", mode),
                3,
                "the const-mod call must contribute no gates ({label}, {mode:?})"
            );
        }
    }
}

/// Every `Literal::Int` baked into a node property anywhere in `m`, flattening
/// array initializers. Values baked by the two spellings of a differential
/// land in different property shapes — a scalar `Value` on the runtime side, an
/// `InitialValue` array on the const side — so comparing them needs a view that
/// does not care which.
fn baked_ints(m: &Module) -> Vec<i64> {
    fn push(lit: &Literal, out: &mut Vec<i64>) {
        match lit {
            Literal::Int(i) => out.push(*i),
            Literal::Array(items) => items.iter().for_each(|i| push(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for n in m.nodes.values() {
        for lit in n.properties.values() {
            push(lit, &mut out);
        }
    }
    for chip in m.chips.values() {
        out.extend(baked_ints(chip));
    }
    out
}

/// THE invariant this file's header claims: the const and runtime spellings of
/// one program agree on VALUES.
///
/// `out r = …` and a valued `return` can both assign a single declared output.
/// The wire graph keeps the FIRST assignment in source order and drops the
/// rest; const evaluation short-circuits at the `return`. When the `out` comes
/// first the two therefore disagree — measured at
/// `mod pick(n: int) -> (r: int) { out r = 111  if n > 0 { return 222 } }`,
/// which baked 222 as a `const mod` and wired 111 as a plain `mod`, both
/// reporting no errors. "Assign a default, then override in a guard" is the
/// natural idiom that lands on it.
///
/// The const spelling of that program is now REJECTED, so the disagreement
/// cannot ship. The plain-`mod` spelling still compiles, which is what makes
/// the rejection specific to the const path rather than a syntax error.
#[test]
fn a_return_that_would_disagree_with_the_wire_graph_is_rejected() {
    let codes = error_codes(
        "const mod pick(n: const int) -> (r: int) { out r = 111\n\
           if n > 0 { return 222 } }\n\
         const v = pick(5)\n\
         var arr: int[] = [v, 12345]\n\
         in go: exec\non go { }",
        "return_conflict_const.ws",
    );
    assert!(
        codes.iter().any(|c| c == "WS046"),
        "the const spelling must be refused rather than baking a value the \
         wire graph would not produce, got {codes:?}"
    );

    // The identical body as a plain `mod` is untouched: this is a const-path
    // disagreement, not bad syntax.
    let m = lower_ok(
        "mod pick(n: int) -> (r: int) { out r = 111\n\
           if n > 0 { return 222 } }\n\
         var x: int = 0\n\
         in go: exec\non go { x = pick(5) }",
        "return_conflict_runtime.ws",
    );
    let ints = baked_ints(&m);
    assert!(
        ints.contains(&111),
        "the plain mod must still wire its first assignment, got {ints:?}"
    );
}

/// The converse ordering — `return` first, `out` after — genuinely AGREES,
/// because lowering's first-wins keeps the returned value and const evaluation
/// returns the same one. Both spellings are compiled here and asserted to bake
/// the SAME value, which is the differential the header describes.
///
/// This is also what stops the fix above from being over-broad: a rule keyed
/// on "does this mod contain any `out`" rather than on source order would
/// reject this working program.
#[test]
fn const_and_runtime_agree_when_the_return_comes_first() {
    let const_ints = baked_ints(&lower_ok(
        "const mod pick(n: const int) -> (r: int) { if n > 0 { return 222 }\n\
           out r = 111 }\n\
         const v = pick(5)\n\
         var arr: int[] = [v, 12345]\n\
         in go: exec\non go { }",
        "return_first_const.ws",
    ));
    let runtime_ints = baked_ints(&lower_ok(
        "mod pick(n: int) -> (r: int) { if n > 0 { return 222 }\n\
           out r = 111 }\n\
         var x: int = 0\n\
         in go: exec\non go { x = pick(5) }",
        "return_first_runtime.ws",
    ));
    for (label, ints) in [("const", &const_ints), ("runtime", &runtime_ints)] {
        assert!(
            ints.contains(&222),
            "{label} spelling must produce 222, got {ints:?}"
        );
        assert!(
            !ints.contains(&111),
            "{label} spelling must NOT produce the dropped assignment 111, got {ints:?}"
        );
    }
    // The const side additionally proves it evaluated away rather than
    // emitting gates: the literal control travels with it in the same array.
    assert!(
        const_ints.contains(&12345),
        "the control literal must bake alongside it, got {const_ints:?}"
    );
}
