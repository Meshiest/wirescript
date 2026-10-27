//! Compiles every `.ws` fixture in `tests/fixtures/const/` and asserts the
//! const half of each differential actually evaluated away.
//!
//! The fixtures prove in game that const and runtime agree on values. They
//! cannot prove the const path emitted no gates, since a `const mod` that fell
//! back to gates still computes the right answer; this file checks that.

use crate::common;

use wirescript::FoldMode;
use wirescript::intern::intern;
use wirescript::ir::{Literal, Module, gate_class};
use wirescript::typecheck;
use wirescript::{FsLoader, Severity, resolve};

fn count(m: &Module) -> usize {
    let mut n = 0;
    m.walk(&mut |_, sub| n += sub.nodes.len());
    n
}

/// Whether `m` or any of its nested chip modules contains a node of gate
/// class `class`.
fn has_gate_class(m: &Module, class: &str) -> bool {
    let mut found = false;
    m.walk(&mut |_, sub| found |= sub.nodes.values().any(|n| n.gate_class == class));
    found
}

/// Resolve, typecheck and lower `source`, asserting no errors at any stage.
///
/// Folding is FORCED OFF, so any gate that disappears did so because const
/// evaluation removed it, not because the optimizer did.
fn lower_ok(source: &str, file: &str) -> Module {
    let (resolved, tc, lowered) = common::run_stages(source, file, FoldMode::ForceOff);
    assert!(
        resolved
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "resolve errors in {file}: {:?}",
        resolved.diagnostics
    );
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors in {file}: {:?}",
        tc.diagnostics
    );
    assert!(
        lowered
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "lower errors in {file}: {:?}",
        lowered.diagnostics
    );
    lowered.module
}

/// Gate count of `source`, lowered with folding off as in [`lower_ok`].
fn gates(source: &str, file: &str) -> usize {
    count(&lower_ok(source, file))
}

/// Error codes `source` produces up to and including typecheck, for the
/// rejected half of a differential ([`lower_ok`] panics on errors).
fn error_codes(source: &str, file: &str) -> Vec<String> {
    let resolved = resolve(source, file, &FsLoader);
    let tc = typecheck(&resolved.ast, &wirescript::CeSlotMap::default());
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
    let mut out = Vec::new();
    m.walk(&mut |_, sub| {
        out.extend(
            sub.nodes
                .values()
                .filter(|n| n.gate_class == gate_class::SEND_CUSTOM_EVENT)
                .filter_map(|n| match n.properties.get(&intern("EventName")) {
                    Some(Literal::String(s)) => Some(s.clone()),
                    _ => None,
                }),
        );
    });
    out
}

/// Every fixture compiles with no diagnostics at all, warnings included: a
/// WSP001 "emitted placeholder" warning is the only signal a partially-lowered
/// construct gives before it reaches the game.
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
        let (resolved, tc, lowered) = common::run_stages(&src, file, FoldMode::ForceOff);
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
    assert!(
        seen > 0,
        "no fixtures found — the suite must not silently pass empty"
    );
}

/// A const parameter carries a literal into a position that requires one,
/// which a plain parameter (a wire) cannot reach. `SendCustomEvent`'s channel
/// name is gate config, not a wire port: `name: const string` bakes `"died"`
/// into it, while `name: string` is rejected with WS028.
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
/// initializer) instead of emitting the gates the runtime form needs. A plain
/// `let` folds there too, so this is not const-specific; it also smoke-tests
/// the `gates`/`count` helpers.
#[test]
fn a_const_binding_folds_into_a_literal_position() {
    let c = gates("const OUT = 1 << 4\nvar sink: int = OUT", "const.ws");
    let r = gates(
        "let OUT = 1 << 4\nvar sink: int = 0\non RoundStart() { sink = OUT }",
        "runtime.ws",
    );
    assert!(
        c < r,
        "const form ({c} gates) must emit fewer than the runtime form ({r})"
    );
}

/// A `match` whose scrutinee's `__disc` is compile-time known lowers to the
/// taken arm with no `Expr_Select` gate, even with folding off. The `static var`
/// control proves the same program otherwise reaches the Select-emitting path.
#[test]
fn a_const_scrutinee_match_emits_no_select() {
    let folded = lower_ok(
        "enum Shape { Empty, Circle(float) }\n\
         const s = Shape.Circle(3.0)\n\
         out area = match s { Circle(r) => r, Empty => 0.0 }\n",
        "const_match.ws",
    );
    assert!(
        !has_gate_class(&folded, gate_class::SELECT),
        "a const-scrutinee match must emit no Expr_Select gate"
    );

    let runtime = lower_ok(
        "enum Shape { Empty, Circle(float) }\n\
         static var s: Shape = Shape.Circle(3.0)\n\
         out area = match s { Circle(r) => r, Empty => 0.0 }\n",
        "runtime_match.ws",
    );
    assert!(
        has_gate_class(&runtime, gate_class::SELECT),
        "control: a runtime-scrutinee match must still build a real Select tree"
    );
}

/// A bare prelude construction scrutinee (`match Some(7) { ... }`, with no
/// declaration, qualification or binding) elides the Select gate like a
/// `const`-bound one, via `const_eval::expr::eval_bare_variant_ctor`.
#[test]
fn a_bare_prelude_variant_scrutinee_match_emits_no_select() {
    let folded = lower_ok(
        "out area = match Some(7) { Some(x) => x, None => 0 }\n",
        "bare_const_match.ws",
    );
    assert!(
        !has_gate_class(&folded, gate_class::SELECT),
        "a bare-constructed Option scrutinee match must emit no Expr_Select gate"
    );
}

/// A call to a `const mod` in a const position costs zero gates: it matches
/// the program with the evaluated literal written in, and emits fewer gates
/// than the same body called as an ordinary mod. If `lower_chip_call_inline`
/// lowered a const parameter's argument as a wire, the body's gates would stay.
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

/// A top-level `const` bound from a `const mod` call is usable like any
/// top-level constant: `CHANNEL` bakes into `SendCustomEvent`'s constant-only
/// channel name, checked on the gate's `EventName` property.
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

/// A constant defined from a const-mod call is usable in another `const`
/// binding's expression (`B` reads `A = f(1)`), checked on the baked
/// `EventName`.
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

/// A call to a `const mod` before the mod's declaration is WS021, like any
/// mod call, even though `build_const_env` could resolve the mod from any
/// position.
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

/// `fixtures/const/arrays.ws`'s shape: a const-mod call per element of an
/// indexed array literal in a const position costs the same gates as the
/// program with the evaluated literals written in.
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

/// `fixtures/const/compound.ws`'s claim: a const-mod call nested in a binary
/// operator (`double(3) + 1`) in a const position costs the same gates as
/// writing `7`.
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

/// The same claim for a call under unary `-` and as a named `Vec` argument.
/// Only the gate count is checked; the values are checked in game by
/// `fixtures/const/compound.ws`.
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

/// `fixtures/const/destructure.ws`'s claim: a `const { .. } = ..` destructure
/// (plain, alias and rest) of a `const mod` record in a const position costs
/// the same gates as the evaluated literals.
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

/// Destructuring a multi-output `const mod` (`const { a, b } = pair(2)`)
/// reaches `lower::decl`'s const-mod skip path and costs zero gates. The second
/// half checks the baked channel names, since zero gates also describes a
/// dropped constant.
///
/// Binding the whole result to one name (`const r = pair(2)`) still lowers the
/// call: `lower::decl` keeps it off the skip path so its field reads resolve.
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

/// `fixtures/const/module_index.ws`'s claim: indexing a module-level `const`
/// array (`const z = t[1]`) costs zero gates, both as a baked initializer and as
/// a const argument. Both sides declare `t`, so only the indexing differs.
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
        &format!("{DECL}var counts: int[] = [20, 12345]\nin go: exec\non go {{ ping(20) }}"),
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

// A constant naming a later-declared constant is a typecheck error, since
// top-level `let`/`const` resolve in declaration order; see
// `lower::tests::const_init::a_forward_reference_between_top_level_lets_is_reported`.

/// `fixtures/const/assembly.ws`'s shape: a `const mod` that assembles its array
/// with `if` and `push`, called in a const position, costs the same gates as
/// the evaluated array and fewer than the same body as an ordinary mod.
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

    // Baseline: the same body as an ordinary mod. Its result lands in a `var`
    // before `[1]` reads it, because a plain mod's returned array has no
    // storage of its own.
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

/// Every error diagnostic from all three stages, as `"CODE message"`. Unlike
/// [`error_codes`] it runs lowering, where WS044 is emitted, and unlike
/// [`lower_ok`] it does not panic on errors.
fn all_error_diags(source: &str, file: &str) -> Vec<String> {
    let (resolved, tc, lowered) = common::run_stages(source, file, FoldMode::ForceOff);
    resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .chain(lowered.diagnostics.iter())
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} {}", d.code, d.message))
        .collect()
}

/// A `const mod` that assembles its result by mutating a `const` collection.
/// It has no valid ordinary lowering: a `const` binding has no array var for
/// the runtime `clear`/`push` gates to target.
const MUTATING_CONST_MOD: &str = "const mod rooms(n: int) -> int[] {\n\
   const t = [0]\n\
   t.clear()\n\
   if n >= 1 { t.push(10) }\n\
   if n >= 2 { t.push(20) }\n\
   return t\n\
 }\n";

/// Binding a mutating `const mod`'s result compiles clean: the constant is the
/// whole result, so the initializer is not also lowered as ordinary code.
/// Each binding form takes its own lowering path (`TopDecl::Let` or
/// `Stmt::Let`; `const` demands evaluation succeed, `let` does not).
#[test]
fn binding_a_mutating_const_mod_is_clean_at_every_scope() {
    for (label, src) in [
        (
            "module const",
            format!("{MUTATING_CONST_MOD}const r = rooms(2)\nvar sink: int[] = r"),
        ),
        (
            "module let",
            format!("{MUTATING_CONST_MOD}let r = rooms(2)\nvar sink: int[] = r"),
        ),
        (
            "handler const",
            format!(
                "{MUTATING_CONST_MOD}in go: exec\non go {{ const r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"
            ),
        ),
        (
            "handler let",
            format!(
                "{MUTATING_CONST_MOD}in go: exec\non go {{ let r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"
            ),
        ),
        // Control: a call that is never bound to a name.
        (
            "var initializer",
            format!("{MUTATING_CONST_MOD}var spawn: int[] = rooms(2)"),
        ),
    ] {
        let diags = all_error_diags(&src, "binding.ws");
        assert!(
            diags.is_empty(),
            "binding a mutating const mod ({label}) must be clean, got {diags:?}"
        );
    }
}

/// WS044 still fires for a dropped mutation outside a const-mod body. The
/// binding case above is clean because the evaluated initializer is not
/// lowered, not because mutation methods stopped reporting.
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
    let (_resolved, _tc, lowered) = common::run_stages(source, file, fold_mode);
    count(&lowered.module)
}

/// A `let` initializer the full const evaluator can fold but
/// `expr_to_literal_in` cannot (interpolation, an `if` expression) keeps its
/// gates with folding off: the const-mod-call skip applies only to
/// initializers that call a `const mod`. The `@nofold` variants also keep their
/// gates under `Auto`, where the fold pass would otherwise collapse them.
#[test]
fn a_non_const_mod_initializer_keeps_its_gates() {
    // (label, source, expected gates with folding disabled, is `@nofold`)
    let cases: [(&str, String, usize, bool); 4] = [
        (
            "interpolation, @nofold",
            "in go: exec\non go { @nofold let s = \"a${1 + 1}b\"\n BroadcastChatMessage(s) }"
                .to_string(),
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
        // `@nofold` opts out of the default fold pass, so the gates survive
        // `Auto` too.
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

/// An initializer that calls a `const mod` is skipped even under `@nofold`: a
/// mutating `const mod` has no ordinary lowering, so lowering it would report
/// WS044. See `lower_let_decl`.
#[test]
fn a_const_mod_call_initializer_is_skipped_even_under_nofold() {
    for (label, src) in [
        (
            "plain",
            format!(
                "{MUTATING_CONST_MOD}in go: exec\non go {{ let r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"
            ),
        ),
        (
            "@nofold",
            format!(
                "{MUTATING_CONST_MOD}in go: exec\non go {{ @nofold let r = rooms(2)\n BroadcastChatMessage(\"${{r[0]}}\") }}"
            ),
        ),
    ] {
        for mode in [FoldMode::ForceOff, FoldMode::Auto] {
            let diags = all_error_diags(&src, "nofold_constmod.ws");
            assert!(
                diags.is_empty(),
                "a const-mod-call initializer ({label}) must stay clean, got {diags:?}"
            );
            // 3 gates: the handler's chain (MicrochipInput), the FormatText
            // (with `r[0]` baked as `InputA: 10`) and the BroadcastChatMessage.
            // A fourth gate would be an `_Unsupported` placeholder for `r[0]`.
            assert_eq!(
                gates_at(&src, "nofold_constmod.ws", mode),
                3,
                "the const-mod call must contribute no gates ({label}, {mode:?})"
            );
        }
    }
}

/// Every `Literal::Int` baked into a node property anywhere in `m`, flattening
/// array initializers, since the runtime spelling bakes a scalar `Value` where
/// the const spelling bakes an `InitialValue` array.
fn baked_ints(m: &Module) -> Vec<i64> {
    fn push(lit: &Literal, out: &mut Vec<i64>) {
        match lit {
            Literal::Int(i) => out.push(*i),
            Literal::Array(items) => items.iter().for_each(|i| push(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    m.walk(&mut |_, sub| {
        sub.nodes
            .values()
            .flat_map(|n| n.properties.values())
            .for_each(|lit| push(lit, &mut out));
    });
    out
}

/// When `out r = ...` precedes a valued `return` for the same output, the wire
/// graph keeps the first assignment while const evaluation stops at the
/// `return`, so the two would disagree. The const spelling is rejected (WS046);
/// the plain `mod` still compiles and wires its first assignment.
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

/// The converse ordering, `return` first and `out` after, AGREES: an output
/// written in more than one place goes through the return storage, so the
/// runtime spelling stores 222 on the path that returns and 111 on the other,
/// and reads the one that ran; const evaluation of `pick(5)` returns 222. The
/// rejection above is keyed on source order, so this program stays valid.
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
    assert!(
        const_ints.contains(&222),
        "const spelling must produce 222, got {const_ints:?}"
    );
    assert!(
        !const_ints.contains(&111),
        "const spelling must NOT produce the assignment after the return, got {const_ints:?}"
    );
    // At runtime `n` is not known, so both paths store their value.
    assert!(
        runtime_ints.contains(&222) && runtime_ints.contains(&111),
        "runtime spelling must store the value of each path, got {runtime_ints:?}"
    );
    // The const side additionally proves it evaluated away rather than
    // emitting gates: the literal control travels with it in the same array.
    assert!(
        const_ints.contains(&12345),
        "the control literal must bake alongside it, got {const_ints:?}"
    );
}
