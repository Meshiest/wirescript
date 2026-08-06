/// Self-checking test for builtins + the type system.
///
/// Runs automatically on grid load (`on start`): each assertion runs its
/// builtin / coercion / generic / receiver expression, compares the result to a
/// known constant and, on a mismatch, broadcasts `FAIL: <label>` to chat. A
/// final line reports `ALL <n> PASS` or `<pass>/<total>`. It doubles as a
/// compile-time type test:
/// the program only type-checks if every builtin's argument and result types
/// line up and each receiver call binds a compatible `self` (the
/// `builtins_and_types_program_compiles` unit test compiles + emits it). The
/// builtins' certified numeric semantics are covered separately in
/// `data/gate_semantics.json` / `probes/verify_semantics.ws`.

/// Float equality within a small epsilon. Also exercises the `self`-receiver
/// path: `x.approxEq(y)` desugars to `approxEq(x, y)`.
mod approxEq(self: float, other: float) -> bool {
  let d = self - other
  return d < 0.001 && d > -0.001
}

/// A generic passthrough selector. Inference AND explicit type args are tested.
mod pick<T>(c: bool, a: T, b: T) -> T {
  return if c then a else b
}

/// Record one assertion: bump `total`, and either `pass` or print the failure.
/// `pass`/`total` are ref params so the caller's counters update in place.
mod check(pass: *int, total: *int, ok: bool, label: string) {
  total += 1
  pass += ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label}")
  }
}

let start = ReadBrickGrid()

on start {
  var pass: int = 0
  var total: int = 0

  // --- math builtins ---
  check(pass, total, abs(-5) == 5 && abs(5) == 5, "abs")
  check(pass, total, sqrt(16.0).approxEq(4.0) && sqrt(2.0).approxEq(1.41421356), "sqrt")
  check(pass, total, min(3, 7) == 3 && max(3, 7) == 7 && min(-1, -4) == -4, "min/max")
  check(pass, total, clamp(15, 0, 10) == 10 && clamp(-3, 0, 10) == 0 && clamp(5, 0, 10) == 5, "clamp")
  check(pass, total, floor(3.7).approxEq(3.0) && ceil(3.2).approxEq(4.0) && round(3.5).approxEq(4.0), "rounding")
  check(pass, total, pow(2.0, 3.0).approxEq(8.0) && pow(9.0, 0.5).approxEq(3.0), "pow")
  check(pass, total, sign(-3) == -1 && sign(4) == 1 && sign(0) == 0, "sign")

  // --- types / coercion ---
  let f: float = 5 // int -> float coercion
  check(pass, total, f.approxEq(5.0), "int->float coercion")
  check(pass, total, 1 + 1 == 2, "int arithmetic")
  check(pass, total, ("hi" == "hi") && ("a" != "b"), "string equality")

  // --- vector + receiver dispatch ---
  check(pass, total, Vec(1.0, 2.0, 3.0).Dot(Vec(1.0, 0.0, 0.0)).approxEq(1.0), "vector dot + receiver")

  // --- generics: inference + explicit type args ---
  check(pass, total, pick(true, 10, 20) == 10 && pick(false, 10, 20) == 20, "generic inference")
  check(pass, total, pick<int>(true, 1, 2) == 1, "explicit type args")

  if pass == total {
    BroadcastChatMessage("builtins/types: ALL ${total} PASS")
  } else {
    BroadcastChatMessage("builtins/types: ${pass}/${total} passed")
  }
}
