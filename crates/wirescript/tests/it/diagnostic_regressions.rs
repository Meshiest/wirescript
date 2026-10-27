//! Diagnostic behaviour for record, namespace, and control-flow code. Each test
//! asserts either that a code is absent (no false positive) or that it is
//! present (a silent miscompile is reported). The full pipeline runs (parse,
//! resolve, typecheck, lower, cycle-analyze) so lowering-stage diagnostics are
//! included.

use wirescript::{CompileError, compile};

use crate::common;

const FILE: &str = "diagnostic_regressions.ws";

fn codes(src: &str) -> Vec<String> {
    common::compile_diag_codes(src, FILE)
}

fn has(src: &str, code: &str) -> bool {
    codes(src).iter().any(|c| c == code)
}

/// Every source whose diagnostics miss a `present` code or carry an `absent`
/// one, as one failure line each.
fn check(srcs: &[&str], present: &[&str], absent: &[&str]) {
    let failures: Vec<String> = srcs
        .iter()
        .filter_map(|src| {
            let got = codes(src);
            let missing: Vec<&&str> = present
                .iter()
                .filter(|c| !got.iter().any(|g| g == **c))
                .collect();
            let unwanted: Vec<&&str> = absent
                .iter()
                .filter(|c| got.iter().any(|g| g == **c))
                .collect();
            (!missing.is_empty() || !unwanted.is_empty()).then(|| {
                format!("{src:?}: missing {missing:?}, unwanted {unwanted:?}, got {got:?}")
            })
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A test named `$name` over one or more sources: `has` asserts each reports
/// the code, `lacks` that none reports any of the listed codes.
macro_rules! diag_case {
    ($name:ident: $($src:expr),+ $(,)? => has $code:literal) => {
        #[test]
        fn $name() {
            check(&[$($src),+], &[$code], &[]);
        }
    };
    ($name:ident: $($src:expr),+ $(,)? => lacks $($code:literal),+) => {
        #[test]
        fn $name() {
            check(&[$($src),+], &[], &[$($code),+]);
        }
    };
}

/// The `(code, message)` pairs for a source, across both paths - for tests
/// that assert on the wording, not just the code.
fn diag_msgs(src: &str) -> Vec<(String, String)> {
    let input = common::input(src, FILE);
    let pull = |ds: &[wirescript::Diagnostic]| {
        ds.iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    };
    match compile(input) {
        Ok(result) => pull(&result.diagnostics),
        Err(CompileError::HasErrors(errors)) => pull(&errors),
        Err(CompileError::Emit(e)) => panic!("unexpected emit error for src={src:?}: {e:?}"),
    }
}

// --- Assigning a `let`/input-backed record field is WS007, not a silent drop.
diag_case!(assigning_a_let_backed_record_field_is_ws007:
    "type P = { x: int, y: int }\nin go: exec\non go {\n  let p = { x: 1, y: 2 }\n  p.x = 5\n}\n" => has "WS007");

diag_case!(assigning_a_var_backed_record_field_stays_clean:
    "type P = { x: int, y: int }\nin go: exec\nvar xa: int = 0\nvar yb: int = 0\non go {\n  let p = { x: xa, y: yb }\n  p.x = 5\n}\n" => lacks "WS007");

// --- `==` on two record values is WS004, not a clean typecheck + placeholder.
diag_case!(comparing_two_records_with_eq_is_ws004:
    "type P = { x: int, y: int }\nin go: exec\non go {\n  let p = { x: 1, y: 2 }\n  let q = { x: 1, y: 2 }\n  let eq = p == q\n}\n" => has "WS004");

// A multi-output result (a record) compared to a scalar auto-unwraps to its
// first output, so the record-vs-record guard must NOT sweep it up.
diag_case!(a_multi_output_result_compared_to_a_scalar_stays_clean:
    "in go: exec\non go {\n  var arr: int[]\n  arr.push(5)\n  let z = arr.pop()\n  let ok = z == 5\n}\n" => lacks "WS004");

// --- A `...rest` destructured mod param types the rest as a record (not `any`).
diag_case!(a_rest_destructured_param_types_as_a_record:
    "type Config = { data: int, alpha: int, beta: int }\nmod f({ data, ...opts }: Config) -> int {\n  return opts.alpha + opts.beta\n}\nin go: exec\non go {\n  let cfg: Config = { data: 1, alpha: 2, beta: 3 }\n  let r = f(cfg)\n}\n" => lacks "WS004");

// --- A tuple-typed field inside a record annotation coerces (no bogus WS003).
diag_case!(a_tuple_typed_field_in_a_record_annotation_stays_clean:
    "type T = { pair: (int, int), n: int }\nin go: exec\non go {\n  let t: T = { pair: (1, 2), n: 3 }\n  let s = t.n + t.pair.0\n}\n" => lacks "WS003");

// --- A non-lvalue arg to a `*T` ref param is WS008, not a silent drop.
#[test]
fn a_literal_passed_to_a_ref_param_is_ws008() {
    // Exact list, not just presence: a ref-typed param with no lvalue-shaped
    // argument is WS008 alone, not WS008 plus a piled-on WS003.
    let src = "mod inc(v: *int) { v = v + 1 }\nin go: exec\non go { inc(5) }\n";
    assert_eq!(codes(src), vec!["WS008"]);
}

diag_case!(an_array_element_passed_to_a_scalar_ref_param_is_ws008:
    "mod inc(v: *int) { v = v + 1 }\nin go: exec\non go {\n  var arr: int[]\n  arr.push(3)\n  inc(arr[0])\n}\n" => has "WS008");

diag_case!(a_var_passed_to_a_ref_param_stays_clean:
    "mod inc(v: *int) { v = v + 1 }\nin go: exec\nvar a: int = 0\non go { inc(a) }\n" => lacks "WS008");

// Each record field is its own storage gate, so `&p.a` names real storage
// and lowers to a correctly wired write; the `&` check accepts a field path.
diag_case!(record_field_to_ref_param_is_clean: "type P = { a: int, b: int }
mod inc(v: *int) { v = v + 1 }
in go: exec
var p: P = { a: 0, b: 0 }
on go { inc(&p.a) }
" => lacks "WS008");

diag_case!(nested_record_field_to_ref_param_is_clean: "type I = { m: int }
type O = { i: I, k: int }
mod inc(v: *int) { v = v + 1 }
in go: exec
var o: O = { i: { m: 0 }, k: 0 }
on go { inc(&o.i.m) }
" => lacks "WS008");

// The control: a scalar `let` is a computed wire with no storage behind it,
// so a field of one must stay rejected rather than silently no-op.
diag_case!(computed_record_let_field_to_ref_param_is_ws008: "type P = { a: int, b: int }
mod inc(v: *int) { v = v + 1 }
in go: exec
in u: int
on go {
  let p: P = { a: u, b: u }
  inc(&p.a)
}
" => has "WS008");

// --- An `emit` to a non-emittable target is WS057, not a silent no-op.
diag_case!(emitting_to_an_input_port_is_ws057: "in go: exec\non go { emit go }\n" => has "WS057");

// A bare `emit` in an exec context is no exec-context error (WS007); a value
// output it names is WS057 instead.
diag_case!(emit_no_value_in_exec_works:
    "out result: int\nin start: exec\non start { emit result }" => lacks "WS007");

// --- `let v = await sig` on a valueless signal is WS056, not a garbage wire.
diag_case!(awaiting_a_value_from_a_valueless_signal_is_ws056:
    "let sig: exec\nstatic var captured: int = 0\non start { emit sig }\non go {\n  let v = await sig\n  captured = v\n}\nin start: exec\nin go: exec\n" => has "WS056");

// --- A tuple `return` to a mod with named multi-outputs distributes each
//     element to the matching output, so `let (a, b) = f()` reads real values.
diag_case!(tuple_return_named_outputs_no_placeholder:
    "mod pair(x: int) -> (a: int, b: int) {\n  return (x, x + 1)\n}\nstatic var res: int = 0\nin go: exec\non go {\n  let (a, b) = pair(3)\n  res = a * 10 + b\n}\n" => lacks "WSP001");

// --- A component field borrowed from another component type (or a typo)
//     is WS010, not a silent SplitColor/SplitVector fed the wrong-typed value
//     (`v.r` on a vector must not emit a SplitColor).
diag_case!(a_component_field_borrowed_from_another_type_is_ws010:
    "in go: exec\nvar v: vector = Vec(1.0,2.0,3.0)\nvar f: float = 0.0\non go { f = v.r }\n",
    "in go: exec\nvar c: color = Color(1.0,0.0,0.0)\nvar f: float = 0.0\non go { f = c.x }\n" => has "WS010");

diag_case!(a_valid_component_field_access_stays_clean:
    "in go: exec\nvar v: vector = Vec(1.0,2.0,3.0)\nvar c: color = Color(1.0,0.0,0.0)\n\
     var f: float = 0.0\non go { f = v.x }\non go { f = c.g }\n" => lacks "WS010");

// --- A negated union / double negation trigger is WS001, not a silently
//     dropped handler.
diag_case!(a_negated_union_trigger_is_ws001:
    "in a: exec\nin b: exec\nstatic var f: int = 0\non !(a | b) { f = 1 }\n",
    "in a: exec\nstatic var f: int = 0\non !!a { f = 1 }\n" => has "WS001");

diag_case!(plain_negated_and_union_triggers_stay_clean:
    "in a: exec\nin b: exec\nstatic var f: int = 0\non !a { f = 1 }\non (a | b) { f = 2 }\n" => lacks "WS001");

// --- Assigning to a non-lvalue (a call result, a literal) is WS007, not a
//     silently dropped assignment.
diag_case!(assigning_to_a_call_result_is_ws007:
    "mod f() -> int { return 1 }\nin go: exec\non go { f() = 5 }\n" => has "WS007");

diag_case!(assigning_to_a_var_field_or_element_stays_clean:
    "type P = { x: int, y: int }\nin go: exec\nstatic var p: P = { x: 0, y: 0 }\n\
     static var arr: int[]\non go {\n  p.x = 5\n  arr.push(0)\n  arr[0] = 9\n}\n" => lacks "WS007");

// --- Two sources into one input port (here a duplicate `out o`) is rejected,
//     not a format-valid `.brz` the game rejects at load.
//
//     Typecheck reports it as WS013 at the second `out`, naming the port and
//     the line, rather than letting emit report a `FanIn` that names brick ids
//     the user cannot see. The emit check remains as the backstop.
#[test]
fn two_drivers_on_one_out_port_is_rejected() {
    let input = common::input("in x: int\nout o = x + 1\nout o = x + 2\n", "fan_in.ws");
    match compile(input) {
        Err(CompileError::HasErrors(ds)) => assert!(
            ds.iter().any(|d| d.code == "WS013"),
            "expected WS013 for two always-live drivers, got {ds:?}"
        ),
        Err(CompileError::Emit(e)) => {
            assert!(
                format!("{e:?}").contains("FanIn"),
                "expected FanIn, got {e:?}"
            );
        }
        Ok(_) => panic!("fan-in must be rejected, not compiled to a broken .brz"),
    }
}

// --- `Change`/`Changed` on a reference or container (map/array/`*T`/zone/
//     teleport) is WS059: the detector watches one wire value and those carry
//     none. Nested inside a `Union(...)` trigger it is caught too.
diag_case!(change_on_a_map_is_ws059:
    "in m: Map<int, int>\nstatic var v: int = 0\non Change(m) { v = 1 }\n",
    "in a: Map<int,int>\nin b: int[]\nstatic var v: int = 0\non Union(Change(a), Change(b)) { v = 1 }\n" => has "WS059");

diag_case!(change_on_a_scalar_stays_clean:
    "in x: int\nin s: string\nstatic var v: int = 0\non Change(x) { v = 1 }\non Change(s) { v = 2 }\n" => lacks "WS059");

// --- `Substring` with a near-i64::MAX length must clamp to the string end
//     (a constant-folded fold path), not overflow into a slice-range panic.
//     If the fold overflowed, the compile would panic rather than return.
diag_case!(substring_with_a_huge_length_clamps_instead_of_panicking:
    "out r = \"hello\".Substring(1, 9223372036854775807)\n" => lacks "WSP001");

// --- `return a, b` (parens optional) parses and behaves like `return (a, b)`.
diag_case!(tuple_return_optional_parens:
    "mod pair(x: int) -> (a: int, b: int) {\n  return x, x + 1\n}\nstatic var res: int = 0\nin go: exec\non go {\n  let (a, b) = pair(3)\n  res = a * 10 + b\n}\n" => lacks "WSP001", "WS002");

// --- Enum discriminant assignment: auto-numbering collides with an explicit
//     value written later in the same declaration (B auto-numbers to 3,
//     colliding with C).
diag_case!(duplicate_discriminant_is_ws064: "enum E { A = 2, B, C = 3 }\n" => has "WS064");
// 0, 5, 6
diag_case!(distinct_discriminants_stay_clean: "enum E { A, B = 5, C }\n" => lacks "WS064");

// --- Enum payload construction: bracket form (positional vs named) must match
//     the variant's declared payload shape.
diag_case!(wrong_bracket_form_is_ws065:
    "enum S { Box { w: float } }\nout b = S.Box(1.0)\n",
    "enum S { Circle(float) }\nout c = S.Circle { r: 1.0 }\n" => has "WS065");

diag_case!(unknown_variant_is_ws060: "enum S { A, B }\nout x = S.C\n" => has "WS060");

// --- A known variant constructed with its own declared bracket form (positional
//     args for a tuple-payload variant, named fields for a record-payload one)
//     must compile clean: no unknown-variant flag and no bracket-form mismatch.
diag_case!(valid_positional_and_named_construction_stay_clean:
    "enum S { Circle(float) }\nout c = S.Circle(1.0)\n",
    "enum S { Box { w: float } }\nout b = S.Box { w: 1.0 }\n" => lacks "WS060", "WS065");

// A positional payload given too few arguments is an arity error.
diag_case!(wrong_payload_arity_is_ws022:
    "enum S { Rect(float, float) }\nout x = S.Rect(1.0)\n" => has "WS022");

// --- A bare variant name for a variant that HAS a payload (`Circle` instead of
//     `Circle(_)`) parses as a catch-all binding, silently swallowing every value;
//     WS067 warns and suggests the paren form.
diag_case!(bare_payload_variant_in_match_is_ws067:
    "enum S { Empty, Circle(float) }\n\
     static var s: S = S.Circle(1.0)\n\
     out a = match s { Circle => 2.0, Empty => 1.0 }\n" => has "WS067");

// A payload variant with `(_)` and a bare UNIT variant are both correct.
diag_case!(paren_payload_and_bare_unit_variant_are_not_ws067:
    "enum S { Empty, Circle(float) }\n\
     static var s: S = S.Circle(1.0)\n\
     out a = match s { Circle(r) => r, Empty => 1.0 }\n",
    "enum Dir { N, E, S, W }\n\
     static var d: Dir = Dir.E\n\
     out b = match d { N => 10, E => 20, S => 30, W => 40 }\n" => lacks "WS067");

// --- A braced `{ .. }` construction on a path that is NOT an enum named-payload
//     variant must error (WS065), not silently type as `any`: a field-access
//     chain that isn't an enum variant at all, and a shadowed enum name (a
//     value symbol shadows the enum type).
diag_case!(braced_construction_on_non_variant_path_is_ws065:
    "enum S { Box { w: float } }\nin flag: bool\nout y = flag.foo { w: 1.0 }\n",
    "enum Shape { Box { w: float } }\ntype R = { Box: int }\n\
     mod f(Shape: R) -> int {\n  let s = Shape.Box { w: 1.0 }\n  return 0\n}\n" => has "WS065");

// --- `A.B { <non-record body> }` in a value position (braces instead of parens
//     for a positional variant, or a stray block) must NOT compile clean with the
//     body silently dropped - it commits to construction and errors.
#[test]
fn malformed_braced_construction_is_not_a_silent_clean_compile() {
    // The single most plausible real typo: braces for a positional variant.
    let braces_for_parens = "enum S { Circle(float) }\nout c = S.Circle { 5.0 }\n";
    assert!(
        !codes(braces_for_parens).is_empty(),
        "must not compile clean"
    );
    // A stray non-record block body.
    let block_body = "enum S { A }\nout b = S.A { let x = 1 }\n";
    assert!(!codes(block_body).is_empty(), "must not compile clean");
}

// --- Shorthand fields in a braced construction ref-collapse like record-literal
//     shorthand: `Enum.V { w, h }` from local `var`s type-checks (a scalar var's
//     `*float` auto-derefs to `float`), with no WS003 and the construction accepted.
diag_case!(shorthand_variant_fields_typecheck_clean:
    "enum Shape { Box { w: float, h: float } }\n\
     var w: float = 1.0\nvar h: float = 2.0\nout b = Shape.Box { w, h }\n",
    "enum Shape { Box { w: float, h: float } }\n\
     var w: float = 1.0\nout b = Shape.Box { w, h: 2.0 }\n" => lacks "WS003", "WS010", "WS065");

// --- Match exhaustiveness / reachability (WS054 / WS061) and a non-enum
//     scrutinee (WS066).
diag_case!(non_exhaustive_match_is_ws054:
    "enum S { A, B, C }\nout x = match s { A => 1, B => 2 }\nvar s: S = S.A\n" => has "WS054");

diag_case!(exhaustive_match_stays_clean:
    "enum S { A, B }\nvar s: S = S.A\nout x = match s { A => 1, B => 2 }\n" => lacks "WS054", "WS061");

#[test]
fn match_on_non_enum_is_ws066() {
    check(
        &["in v: vector\nout x = match v { _ => 0 }\n"],
        &["WS066"],
        &[],
    );
    // An int, float, bool or string is matched by value.
    check(
        &["in n: int\nout x = match n { _ => 0 }\n"],
        &[],
        &["WS066"],
    );
}

// A `.Discriminant` read on an actual enum-typed value must not flag - only a
// non-enum target does.
diag_case!(valid_discriminant_access_on_enum_stays_clean:
    "enum S { A, B }\nvar s: S = S.A\nout d = s.Discriminant\n" => lacks "WS066");

// An unknown named field in a match pattern is the same typo the construction
// side flags: WS010, not a silent `any` capture.
diag_case!(match_unknown_named_field_is_ws010:
    "enum Box { Dims { w: float } }\nvar b: Box = Box.Dims { w: 0.0 }\nout x = match b { Dims { wdith } => 1.0 }\n" => has "WS010");

// An `else` that ends in `return` diverges, so no WS062.
diag_case!(let_else_with_return_stays_clean:
    "enum Opt { Some(int), None }\nin o: Opt\nmod f() -> int {\n  let Some(x) = o else { return 0 }\n  return x\n}\n" => lacks "WS062");

// --- An `if let` / `let else` that emits one output from both the match and
//     non-match paths needs an output backing var; the emit pre-scan
//     (`count_emits_in_block`) must descend into these forms, else the two sites
//     fan into the output rerouter - a load-time FanIn surfaced only at emit.
//     The full pipeline runs through emit and PANICS on an emit error, so a
//     clean run here proves the backing var is created.
diag_case!(if_let_and_let_else_multi_emit_output_reach_emit_cleanly:
    "enum Opt { Some(int), None }\nstatic var o: Opt = Opt.Some(7)\nout r: int\non ReadBrickGrid() {\n  if let Some(x) = o { emit r = x } else { emit r = 0 }\n}\n",
    "enum Opt { Some(int), None }\nstatic var o: Opt = Opt.Some(7)\nout r: int\non ReadBrickGrid() {\n  let Some(x) = o else { emit r = -1 }\n  emit r = x\n}\n" => lacks "WS062");

// A payload-carrying variant pins its type parameter from the argument, so no
// annotation is needed and WS063 must stay quiet.
diag_case!(inferable_generic_construction_stays_clean:
    "enum Option<T> { Some(T), None }\nout n = Option.Some(42)\n" => lacks "WS063");

// A generic enum used as a match scrutinee: the exhaustiveness engine must
// resolve a nested type-parameter payload column through the scrutinee's
// concrete instantiation (`Option<Inner>` -> `Some(T)`'s slot is `Inner`), so
// a missing nested variant surfaces WS054 and a full cover stays clean. Without
// threading the instantiation args the `Some` payload reads as opaque and both
// cases wrongly report exhaustive (a silent wrong verdict).
diag_case!(nested_generic_match_missing_variant_is_ws054:
    "enum Inner { On, Off }\nenum Option<T> { Some(T), None }\n\
     in o: Option<Inner>\nout x = match o { Some(On) => 1, None => 0 }\n" => has "WS054");

diag_case!(nested_generic_match_full_cover_is_clean:
    "enum Inner { On, Off }\nenum Option<T> { Some(T), None }\n\
     in o: Option<Inner>\nout x = match o { Some(On) => 1, Some(Off) => 2, None => 0 }\n" => lacks "WS054");

// --- WS065 for a UNIT variant given a payload suggests the bare form, not a
//     `{ .. }` / `(..)` payload the variant can't take.
#[test]
fn ws065_unit_variant_suggests_bare_form() {
    for src in [
        "enum S { Empty }\nout x = S.Empty(5.0)\n",
        "enum S { Empty }\nout x = S.Empty { w: 1.0 }\n",
    ] {
        let msg = diag_msgs(src)
            .into_iter()
            .find(|(c, _)| c == "WS065")
            .unwrap_or_else(|| panic!("expected WS065 for {src:?}: {:?}", codes(src)))
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

/// Whether the compile stopped with hard ERRORS, as opposed to producing only
/// warnings - `codes` flattens both paths, so a severity assertion needs this.
fn has_errors(src: &str) -> bool {
    let input = common::input(src, FILE);
    matches!(compile(input), Err(CompileError::HasErrors(_)))
}

// --- A bare CAPITALISED pattern that names no variant of the scrutinee's enum
//     is an irrefutable binding, not a variant test, so the typo arm swallows
//     every later arm (the IR shows `Select InputA=200 InputB=200` - the 300 is
//     gone) and the `if let` spelling deletes the whole discriminant test with
//     no diagnostic at all. WS067 covers it, as an error.
#[test]
fn mistyped_bare_variant_in_match_is_a_ws067_error() {
    let src = "enum Color { Red, Green, Blue }\n\
               static var c: Color = Color.Blue\n\
               out v = match c { Red => 100, Gren => 200, Blue => 300 }\n";
    let msg = diag_msgs(src)
        .into_iter()
        .find(|(code, _)| code == "WS067")
        .unwrap_or_else(|| panic!("expected WS067: {:?}", codes(src)))
        .1;
    assert!(
        msg.contains("Color") && msg.contains("Gren"),
        "WS067 should name the enum and the misspelled variant, got: {msg:?}"
    );
    assert!(has_errors(src), "WS067 for a non-variant must be an error");
}

#[test]
fn mistyped_bare_variant_in_if_let_is_a_ws067_error() {
    // The `if let` form loses its `Var_Get` + `CompareEqual` + `Branch` triple
    // entirely, so the body runs unconditionally.
    let src = "enum Color { Red, Green, Blue }\n\
               static var c: Color = Color.Blue\n\
               in go: exec\nstatic var hit: int = 0\n\
               on go {\n  if let Gren = c {\n    hit = 1\n  }\n}\n";
    assert!(has(src, "WS067"), "{:?}", codes(src));
    assert!(has_errors(src), "WS067 for a non-variant must be an error");
}

// A bare unit variant is the correct spelling of a variant test, and a
// lowercase name is how a deliberate named catch-all is written.
diag_case!(bare_unit_variant_and_lowercase_catch_all_are_not_ws067:
    "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\n\
     out b = match d { N => 10, E => 20, S => 30, W => 40 }\n",
    "enum Dir { N, E, S, W }\nstatic var d: Dir = Dir.E\n\
     out b = match d { N => 10, other => 20 }\n" => lacks "WS067");

// --- An array literal element is BAKED as a constant, so a `ViaString`
//     coercion has no `FormatText` gate to run through and emit renders the
//     value as `""`. `var s: string[] = [1, 2]` must not type-check clean and
//     ship two empty strings.
#[test]
fn string_array_initializer_with_int_elements_is_ws003() {
    let src = "static var s: string[] = [1, 2]\non RoundStart() { PrintToConsole(s[0]) }\n";
    let msg = diag_msgs(src)
        .into_iter()
        .find(|(code, _)| code == "WS003")
        .unwrap_or_else(|| panic!("expected WS003: {:?}", codes(src)))
        .1;
    assert!(
        msg.contains("string-formatted"),
        "the element WS003 should say why the coercion can't happen, got: {msg:?}"
    );
}

// The whole-array coerce only inspects element 0, so a `string`-typed first
// element must not carry the rest of the literal through unchecked.
diag_case!(string_array_literal_with_a_non_string_element_is_ws003_in_exec_context:
    "static var s: string[] = []\nin go: exec\non go {\n  s = [\"a\", 2]\n}\n" => has "WS003");

// `Coerce` (int -> float) holds at the literal itself, so it stays legal.
diag_case!(numeric_array_initializers_still_coerce:
    "static var f: float[] = [1, 2.5]\non RoundStart() { PrintToConsole(f[0]) }\n",
    "static var i: int[] = [1, 2]\non RoundStart() { PrintToConsole(i[0]) }\n" => lacks "WS003");

/// An unknown trigger is reported once, by type checking, alone or as a part
/// of a union.
#[test]
fn an_unknown_trigger_is_reported_once() {
    for src in [
        "in start: exec\non Bogus {}\n",
        "in start: exec\non start | Bogus {}\n",
    ] {
        let ws001: Vec<_> = diag_msgs(src)
            .into_iter()
            .filter(|(c, _)| c == "WS001")
            .collect();
        assert_eq!(ws001.len(), 1, "{src}: {ws001:?}");
        assert!(
            ws001[0].1.contains("unknown event or trigger 'Bogus'"),
            "{src}: {ws001:?}"
        );
    }
}
