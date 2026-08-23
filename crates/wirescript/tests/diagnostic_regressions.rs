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
