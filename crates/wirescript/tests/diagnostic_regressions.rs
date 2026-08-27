//! Diagnostic-behaviour regressions for record, namespace, and control-flow
//! fixes. Each test asserts either that a false positive is gone (the code is
//! ABSENT) or that a former silent miscompile now surfaces (the code is
//! PRESENT). The full pipeline runs (parse -> resolve -> typecheck -> lower ->
//! cycle-analyze) so lowering-stage diagnostics are included.

use wirescript::{CompileError, CompileInput, FoldMode, compile};

/// Every diagnostic code produced across the success and hard-error paths.
fn diags(src: &str) -> Vec<String> {
    let input = CompileInput {
        source: src,
        file: "diagnostic_regressions.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    };
    match compile(input) {
        Ok(result) => result.diagnostics.iter().map(|d| d.code.clone()).collect(),
        Err(CompileError::HasErrors(errors)) => errors.iter().map(|d| d.code.clone()).collect(),
        Err(CompileError::Emit(e)) => panic!("unexpected emit error for src={src:?}: {e:?}"),
    }
}

fn has(src: &str, code: &str) -> bool {
    diags(src).iter().any(|c| c == code)
}

/// The `(code, message)` pairs for a source, across both paths - for tests
/// that assert on the wording, not just the code.
fn diag_msgs(src: &str) -> Vec<(String, String)> {
    let input = CompileInput {
        source: src,
        file: "diagnostic_regressions.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    };
    let pull = |ds: &[wirescript::Diagnostic]| {
        ds.iter().map(|d| (d.code.clone(), d.message.clone())).collect::<Vec<_>>()
    };
    match compile(input) {
        Ok(result) => pull(&result.diagnostics),
        Err(CompileError::HasErrors(errors)) => pull(&errors),
        Err(CompileError::Emit(e)) => panic!("unexpected emit error for src={src:?}: {e:?}"),
    }
}

// --- R3: assigning a `let`/input-backed record field is WS007, not a silent drop.
#[test]
fn r3_let_record_field_assign_is_ws007() {
    let src = "type P = { x: int, y: int }\nin go: exec\non go {\n  let p = { x: 1, y: 2 }\n  p.x = 5\n}\n";
    assert!(has(src, "WS007"), "{:?}", diags(src));
}

#[test]
fn r3_var_backed_record_field_assign_is_clean() {
    let src = "type P = { x: int, y: int }\nin go: exec\nvar xa: int = 0\nvar yb: int = 0\non go {\n  let p = { x: xa, y: yb }\n  p.x = 5\n}\n";
    assert!(!has(src, "WS007"), "{:?}", diags(src));
}

// --- R7: `==` on two record values is WS004, not a clean typecheck + placeholder.
#[test]
fn r7_record_equality_is_ws004() {
    let src = "type P = { x: int, y: int }\nin go: exec\non go {\n  let p = { x: 1, y: 2 }\n  let q = { x: 1, y: 2 }\n  let eq = p == q\n}\n";
    assert!(has(src, "WS004"), "{:?}", diags(src));
}

#[test]
fn r7_record_vs_scalar_multioutput_unwrap_stays_clean() {
    // A multi-output result (a record) compared to a scalar auto-unwraps to its
    // first output, so the record-vs-record guard must NOT sweep it up.
    let src = "in go: exec\non go {\n  var arr: int[]\n  arr.push(5)\n  let z = arr.pop()\n  let ok = z == 5\n}\n";
    assert!(!has(src, "WS004"), "{:?}", diags(src));
}

// --- R8: a `...rest` destructured mod param types the rest as a record (not `any`).
#[test]
fn r8_rest_param_typed_as_record_no_ws004() {
    let src = "type Config = { data: int, alpha: int, beta: int }\nmod f({ data, ...opts }: Config) -> int {\n  return opts.alpha + opts.beta\n}\nin go: exec\non go {\n  let cfg: Config = { data: 1, alpha: 2, beta: 3 }\n  let r = f(cfg)\n}\n";
    assert!(!has(src, "WS004"), "{:?}", diags(src));
}

// --- R9: a tuple-typed field inside a record annotation coerces (no bogus WS003).
#[test]
fn r9_tuple_field_in_record_annotation_is_clean() {
    let src = "type T = { pair: (int, int), n: int }\nin go: exec\non go {\n  let t: T = { pair: (1, 2), n: 3 }\n  let s = t.n + t.pair.0\n}\n";
    assert!(!has(src, "WS003"), "{:?}", diags(src));
}

// --- S1: a non-lvalue arg to a `*T` ref param is WS008, not a silent drop.
#[test]
fn s1_literal_to_ref_param_is_ws008() {
    let src = "mod inc(v: *int) { v = v + 1 }\nin go: exec\non go { inc(5) }\n";
    assert!(has(src, "WS008"), "{:?}", diags(src));
}

#[test]
fn s1_arr_element_to_scalar_ref_param_is_ws008() {
    let src = "mod inc(v: *int) { v = v + 1 }\nin go: exec\non go {\n  var arr: int[]\n  arr.push(3)\n  inc(arr[0])\n}\n";
    assert!(has(src, "WS008"), "{:?}", diags(src));
}

#[test]
fn s1_var_to_ref_param_is_clean() {
    let src = "mod inc(v: *int) { v = v + 1 }\nin go: exec\nvar a: int = 0\non go { inc(a) }\n";
    assert!(!has(src, "WS008"), "{:?}", diags(src));
}

// --- S3/S4: an `emit` to a non-emittable target is WS057, not a silent no-op.
#[test]
fn s4_emit_to_input_port_is_ws057() {
    let src = "in go: exec\non go { emit go }\n";
    assert!(has(src, "WS057"), "{:?}", diags(src));
}

// --- S6: `let v = await sig` on a valueless signal is WS056, not a garbage wire.
#[test]
fn s6_await_valueless_signal_is_ws056() {
    let src = "let sig: exec\nstatic var captured: int = 0\non start { emit sig }\non go {\n  let v = await sig\n  captured = v\n}\nin start: exec\nin go: exec\n";
    assert!(has(src, "WS056"), "{:?}", diags(src));
}

// --- A tuple `return` to a mod with named multi-outputs distributes each
//     element to the matching output; `let (a, b) = f()` used to read
//     `_Unsupported` placeholders.
#[test]
fn tuple_return_named_outputs_no_placeholder() {
    let src = "mod pair(x: int) -> (a: int, b: int) {\n  return (x, x + 1)\n}\nstatic var res: int = 0\nin go: exec\non go {\n  let (a, b) = pair(3)\n  res = a * 10 + b\n}\n";
    assert!(!has(src, "WSP001"), "{:?}", diags(src));
}

// --- C1: a component field borrowed from another component type (or a typo)
//     is WS010, not a silent SplitColor/SplitVector fed the wrong-typed value.
#[test]
fn c1_cross_type_component_access_is_ws010() {
    // `v.r` on a vector previously compiled clean and emitted a SplitColor.
    let vr = "in go: exec\nvar v: vector = Vec(1.0,2.0,3.0)\nvar f: float = 0.0\non go { f = v.r }\n";
    assert!(has(vr, "WS010"), "{:?}", diags(vr));
    let cx = "in go: exec\nvar c: color = Color(1.0,0.0,0.0)\nvar f: float = 0.0\non go { f = c.x }\n";
    assert!(has(cx, "WS010"), "{:?}", diags(cx));
}

#[test]
fn c1_valid_component_access_stays_clean() {
    let src = "in go: exec\nvar v: vector = Vec(1.0,2.0,3.0)\nvar c: color = Color(1.0,0.0,0.0)\n\
               var f: float = 0.0\non go { f = v.x }\non go { f = c.g }\n";
    assert!(!has(src, "WS010"), "{:?}", diags(src));
}

// --- C2: a negated union / double negation trigger is WS001, not a silently
//     dropped handler.
#[test]
fn c2_negated_union_trigger_is_ws001() {
    let un = "in a: exec\nin b: exec\nstatic var f: int = 0\non !(a | b) { f = 1 }\n";
    assert!(has(un, "WS001"), "{:?}", diags(un));
    let dbl = "in a: exec\nstatic var f: int = 0\non !!a { f = 1 }\n";
    assert!(has(dbl, "WS001"), "{:?}", diags(dbl));
}

#[test]
fn c2_valid_negated_and_union_triggers_stay_clean() {
    let src = "in a: exec\nin b: exec\nstatic var f: int = 0\non !a { f = 1 }\non (a | b) { f = 2 }\n";
    assert!(!has(src, "WS001"), "{:?}", diags(src));
}

// --- C3: assigning to a non-lvalue (a call result, a literal) is WS007, not a
//     silently dropped assignment.
#[test]
fn c3_assign_to_call_result_is_ws007() {
    let src = "mod f() -> int { return 1 }\nin go: exec\non go { f() = 5 }\n";
    assert!(has(src, "WS007"), "{:?}", diags(src));
}

#[test]
fn c3_assign_to_var_field_element_stays_clean() {
    let src = "type P = { x: int, y: int }\nin go: exec\nstatic var p: P = { x: 0, y: 0 }\n\
               static var arr: int[]\non go {\n  p.x = 5\n  arr.push(0)\n  arr[0] = 9\n}\n";
    assert!(!has(src, "WS007"), "{:?}", diags(src));
}

// --- C4: two sources into one input port (here a duplicate `out o`) is an emit
//     fan-in error, not a format-valid `.brz` the game rejects at load.
#[test]
fn c4_fan_in_is_an_emit_error() {
    let input = CompileInput {
        source: "in x: int\nout o = x + 1\nout o = x + 2\n",
        file: "c4.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    };
    match compile(input) {
        Err(CompileError::Emit(e)) => {
            assert!(format!("{e:?}").contains("FanIn"), "expected FanIn, got {e:?}")
        }
        Ok(_) => panic!("fan-in must be rejected, not compiled to a broken .brz"),
        Err(other) => panic!("expected an emit fan-in error, got {other:?}"),
    }
}

// --- `Change`/`Changed` on a reference or container (map/array/`*T`/zone/
//     teleport) is WS059: the detector watches a single wire value, and those
//     carry none, so it used to compile to a dead gate.
#[test]
fn change_on_a_map_is_ws059() {
    let m = "in m: Map<int, int>\nstatic var v: int = 0\non Change(m) { v = 1 }\n";
    assert!(has(m, "WS059"), "{:?}", diags(m));
    // Nested inside a `Union(...)` trigger is caught too (the whole trigger is
    // an expr trigger, so every `Change` arg is type-checked).
    let u = "in a: Map<int,int>\nin b: int[]\nstatic var v: int = 0\non Union(Change(a), Change(b)) { v = 1 }\n";
    assert!(has(u, "WS059"), "{:?}", diags(u));
}

#[test]
fn change_on_a_scalar_stays_clean() {
    let src = "in x: int\nin s: string\nstatic var v: int = 0\non Change(x) { v = 1 }\non Change(s) { v = 2 }\n";
    assert!(!has(src, "WS059"), "{:?}", diags(src));
}

// --- C6: `Substring` with a near-i64::MAX length must clamp to the string end
//     (a constant-folded fold path), not overflow into a slice-range panic.
#[test]
fn c6_substring_huge_length_does_not_panic() {
    // If the fold overflowed, `diags` would panic (the test process would crash)
    // rather than return; reaching the assert at all proves it clamped.
    let src = "out r = \"hello\".Substring(1, 9223372036854775807)\n";
    assert!(!has(src, "WSP001"), "{:?}", diags(src));
}

// --- `return a, b` (parens optional) parses and behaves like `return (a, b)`.
#[test]
fn tuple_return_optional_parens() {
    let src = "mod pair(x: int) -> (a: int, b: int) {\n  return x, x + 1\n}\nstatic var res: int = 0\nin go: exec\non go {\n  let (a, b) = pair(3)\n  res = a * 10 + b\n}\n";
    let d = diags(src);
    assert!(
        !d.iter().any(|c| c == "WSP001" || c == "WS002"),
        "{:?}",
        d
    );
}

// --- Enum discriminant assignment: auto-numbering collides with an explicit
//     value written later in the same declaration.
#[test]
fn duplicate_discriminant_is_ws064() {
    let src = "enum E { A = 2, B, C = 3 }\n"; // B auto-numbers to 3, colliding with C
    assert!(has(src, "WS064"), "{:?}", diags(src));
}
#[test]
fn distinct_discriminants_stay_clean() {
    let src = "enum E { A, B = 5, C }\n"; // 0, 5, 6
    assert!(!has(src, "WS064"), "{:?}", diags(src));
}

// --- Enum payload construction: bracket form (positional vs named) must match
//     the variant's declared payload shape.
#[test]
fn wrong_bracket_form_is_ws065() {
    let named_as_pos = "enum S { Box { w: float } }\nout b = S.Box(1.0)\n";
    assert!(has(named_as_pos, "WS065"), "{:?}", diags(named_as_pos));
    let pos_as_named = "enum S { Circle(float) }\nout c = S.Circle { r: 1.0 }\n";
    assert!(has(pos_as_named, "WS065"), "{:?}", diags(pos_as_named));
}
#[test]
fn unknown_variant_is_ws060() {
    let src = "enum S { A, B }\nout x = S.C\n";
    assert!(has(src, "WS060"), "{:?}", diags(src));
}
// --- A known variant constructed with its own declared bracket form (positional
//     args for a tuple-payload variant, named fields for a record-payload one)
//     must compile clean: no unknown-variant flag and no bracket-form mismatch.
#[test]
fn valid_positional_and_named_construction_stay_clean() {
    let positional = "enum S { Circle(float) }\nout c = S.Circle(1.0)\n";
    let d = diags(positional);
    assert!(!d.iter().any(|c| c == "WS060" || c == "WS065"), "{d:?}");
    let named = "enum S { Box { w: float } }\nout b = S.Box { w: 1.0 }\n";
    let d = diags(named);
    assert!(!d.iter().any(|c| c == "WS060" || c == "WS065"), "{d:?}");
}
#[test]
fn wrong_payload_arity_is_ws022_or_ws065() {
    let src = "enum S { Rect(float, float) }\nout x = S.Rect(1.0)\n";
    let d = diags(src);
    assert!(d.iter().any(|c| c == "WS022" || c == "WS065"), "{d:?}");
}

// --- A bare variant name for a variant that HAS a payload (`Circle` instead of
//     `Circle(_)`) parses as a catch-all binding, silently swallowing every value;
//     WS067 warns and suggests the paren form.
#[test]
fn bare_payload_variant_in_match_is_ws067() {
    let src = "enum S { Empty, Circle(float) }\n\
               static var s: S = S.Circle(1.0)\n\
               out a = match s { Circle => 2.0, Empty => 1.0 }\n";
    assert!(has(src, "WS067"), "{:?}", diags(src));
}
#[test]
fn paren_payload_and_bare_unit_variant_are_not_ws067() {
    // A payload variant with `(_)` and a bare UNIT variant are both correct.
    let paren = "enum S { Empty, Circle(float) }\n\
                 static var s: S = S.Circle(1.0)\n\
                 out a = match s { Circle(r) => r, Empty => 1.0 }\n";
    assert!(!has(paren, "WS067"), "{:?}", diags(paren));
    let all_unit = "enum Dir { N, E, S, W }\n\
                    static var d: Dir = Dir.E\n\
                    out b = match d { N => 10, E => 20, S => 30, W => 40 }\n";
    assert!(!has(all_unit, "WS067"), "{:?}", diags(all_unit));
}
// --- A braced `{ .. }` construction on a path that is NOT an enum named-payload
//     variant must error (WS065), not silently type as `any`.
#[test]
fn braced_construction_on_non_variant_path_is_ws065() {
    // A field-access chain that isn't an enum variant at all.
    let chain = "enum S { Box { w: float } }\nin flag: bool\nout y = flag.foo { w: 1.0 }\n";
    assert!(has(chain, "WS065"), "{:?}", diags(chain));
    // A shadowed enum name (a value symbol shadows the enum type).
    let shadowed = "enum Shape { Box { w: float } }\ntype R = { Box: int }\n\
                    mod f(Shape: R) -> int {\n  let s = Shape.Box { w: 1.0 }\n  return 0\n}\n";
    assert!(has(shadowed, "WS065"), "{:?}", diags(shadowed));
}
// --- `A.B { <non-record body> }` in a value position (braces instead of parens
//     for a positional variant, or a stray block) must NOT compile clean with the
//     body silently dropped - it commits to construction and errors.
#[test]
fn malformed_braced_construction_is_not_a_silent_clean_compile() {
    // The single most plausible real typo: braces for a positional variant.
    let braces_for_parens = "enum S { Circle(float) }\nout c = S.Circle { 5.0 }\n";
    assert!(!diags(braces_for_parens).is_empty(), "must not compile clean");
    // A stray non-record block body.
    let block_body = "enum S { A }\nout b = S.A { let x = 1 }\n";
    assert!(!diags(block_body).is_empty(), "must not compile clean");
}
// --- Shorthand fields in a braced construction ref-collapse like record-literal
//     shorthand: `Enum.V { w, h }` from local `var`s type-checks (a scalar var's
//     `*float` auto-derefs to `float`), with no WS003 and the construction accepted.
#[test]
fn shorthand_variant_fields_typecheck_clean() {
    let shorthand = "enum Shape { Box { w: float, h: float } }\n\
                     var w: float = 1.0\nvar h: float = 2.0\nout b = Shape.Box { w, h }\n";
    let mixed = "enum Shape { Box { w: float, h: float } }\n\
                 var w: float = 1.0\nout b = Shape.Box { w, h: 2.0 }\n";
    for src in [shorthand, mixed] {
        let d = diags(src);
        assert!(
            !d.iter().any(|c| c == "WS003" || c == "WS010" || c == "WS065"),
            "valid shorthand construction must type-check clean: src={src:?} diags={d:?}"
        );
    }
}
// --- Match exhaustiveness / reachability (WS054 / WS061) and a non-enum
//     scrutinee (WS066).
#[test]
fn non_exhaustive_match_is_ws054() {
    let src = "enum S { A, B, C }\nout x = match s { A => 1, B => 2 }\nin s: S\n";
    assert!(has(src, "WS054"), "{:?}", diags(src));
}
#[test]
fn exhaustive_match_stays_clean() {
    let src = "enum S { A, B }\nin s: S\nout x = match s { A => 1, B => 2 }\n";
    let d = diags(src);
    assert!(!d.iter().any(|c| c == "WS054" || c == "WS061"), "{d:?}");
}
#[test]
fn unreachable_arm_is_ws061() {
    let src = "enum S { A, B }\nin s: S\nout x = match s { _ => 0, A => 1 }\n";
    assert!(has(src, "WS061"), "{:?}", diags(src));
}
#[test]
fn match_on_non_enum_is_ws066() {
    let src = "in n: int\nout x = match n { _ => 0 }\n";
    assert!(has(src, "WS066"), "{:?}", diags(src));
}
// A `.Discriminant` read on an actual enum-typed value must not flag - only a
// non-enum target does.
#[test]
fn valid_discriminant_access_on_enum_stays_clean() {
    let src = "enum S { A, B }\nin s: S\nout d = s.Discriminant\n";
    assert!(!has(src, "WS066"), "{:?}", diags(src));
}
// An unknown named field in a match pattern is the same typo the construction
// side flags: WS010, not a silent `any` capture.
#[test]
fn match_unknown_named_field_is_ws010() {
    let src = "enum Box { Dims { w: float } }\nin b: Box\nout x = match b { Dims { wdith } => 1.0 }\n";
    assert!(has(src, "WS010"), "{:?}", diags(src));
}

// --- WS062: a `let ... else` whose `else` block can fall through (does not
//     diverge on every path) is rejected - the pattern's binding would be
//     unavailable on the non-match path, so the `else` must `return`/`emit` out.
#[test]
fn let_else_without_divergence_is_ws062() {
    let src = "enum Opt { Some(int), None }\nin o: Opt\nmod f() -> int {\n  let Some(x) = o else { let y = 1 }\n  return x\n}\n";
    assert!(has(src, "WS062"), "{:?}", diags(src));
}

// An `else` that ends in `return` diverges, so no WS062.
#[test]
fn let_else_with_return_stays_clean() {
    let src = "enum Opt { Some(int), None }\nin o: Opt\nmod f() -> int {\n  let Some(x) = o else { return 0 }\n  return x\n}\n";
    assert!(!has(src, "WS062"), "{:?}", diags(src));
}

// --- An `if let` / `let else` that emits one output from both the match and
//     non-match paths needs an output backing var; the emit pre-scan
//     (`count_emits_in_block`) must descend into these forms, else the two sites
//     fan into the output rerouter - a load-time FanIn surfaced only at emit.
//     `diags` runs the full pipeline (including emit) and PANICS on an emit
//     error, so a clean run here proves the backing var is created.
#[test]
fn if_let_and_let_else_multi_emit_output_reach_emit_cleanly() {
    let if_let = "enum Opt { Some(int), None }\nstatic var o: Opt = Opt.Some(7)\nout r: int\non ReadBrickGrid() {\n  if let Some(x) = o { emit r = x } else { emit r = 0 }\n}\n";
    assert!(!has(if_let, "WS062"), "{:?}", diags(if_let));
    let let_else = "enum Opt { Some(int), None }\nstatic var o: Opt = Opt.Some(7)\nout r: int\non ReadBrickGrid() {\n  let Some(x) = o else { emit r = -1 }\n  emit r = x\n}\n";
    assert!(!has(let_else, "WS062"), "{:?}", diags(let_else));
}

// --- WS065 for a UNIT variant given a payload suggests the bare form, not a
//     `{ .. }` / `(..)` payload the variant can't take.
#[test]
fn uninferrable_enum_param_is_ws063() {
    // `None` carries no payload, so with no annotation the type parameter `T`
    // is unknowable -> WS063.
    let src = "enum Option<T> { Some(T), None }\nout n = Option.None\n";
    assert!(has(src, "WS063"), "{:?}", diags(src));
}

// A payload-carrying variant pins its type parameter from the argument, so no
// annotation is needed and WS063 must stay quiet.
#[test]
fn inferable_generic_construction_stays_clean() {
    let src = "enum Option<T> { Some(T), None }\nout n = Option.Some(42)\n";
    assert!(!has(src, "WS063"), "{:?}", diags(src));
}

// A generic enum used as a match scrutinee: the exhaustiveness engine must
// resolve a nested type-parameter payload column through the scrutinee's
// concrete instantiation (`Option<Inner>` -> `Some(T)`'s slot is `Inner`), so
// a missing nested variant surfaces WS054 and a full cover stays clean. Without
// threading the instantiation args the `Some` payload reads as opaque and both
// cases wrongly report exhaustive (a silent wrong verdict).
#[test]
fn nested_generic_match_missing_variant_is_ws054() {
    let src = "enum Inner { On, Off }\nenum Option<T> { Some(T), None }\n\
               in o: Option<Inner>\nout x = match o { Some(On) => 1, None => 0 }\n";
    assert!(has(src, "WS054"), "{:?}", diags(src));
}

#[test]
fn nested_generic_match_full_cover_is_clean() {
    let src = "enum Inner { On, Off }\nenum Option<T> { Some(T), None }\n\
               in o: Option<Inner>\nout x = match o { Some(On) => 1, Some(Off) => 2, None => 0 }\n";
    assert!(!has(src, "WS054"), "{:?}", diags(src));
}

#[test]
fn ws065_unit_variant_suggests_bare_form() {
    for src in [
        "enum S { Empty }\nout x = S.Empty(5.0)\n",
        "enum S { Empty }\nout x = S.Empty { w: 1.0 }\n",
    ] {
        let msg = diag_msgs(src)
            .into_iter()
            .find(|(c, _)| c == "WS065")
            .unwrap_or_else(|| panic!("expected WS065 for {src:?}: {:?}", diags(src)))
            .1;
        assert!(
            msg.contains("takes no payload") && msg.contains("S.Empty"),
            "unit-variant WS065 should suggest the bare form, got: {msg:?}"
        );
        assert!(
            !msg.contains("{ .. }") && !msg.contains("(..)"),
            "unit-variant WS065 must not suggest a payload bracket form, got: {msg:?}"
        );
    }
}
