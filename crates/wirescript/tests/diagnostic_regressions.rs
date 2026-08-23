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
