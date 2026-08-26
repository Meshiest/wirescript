// Differential: an enum's `.Discriminant` is checked twice, once folded at
// compile time through a top-level `const` and once read back at runtime off
// a constructed value. Any disagreement is a const-evaluation bug. Run in
// game: build it, then read chat.

let start = ReadBrickGrid()

enum Shape { Empty, Circle(float), Rect(float, float) }

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

// Folded at compile time by `build_const_env`: a bare variant path's
// `.Discriminant`.
const CIRCLE_DISC = Shape.Circle.Discriminant

// A match on a compile-time-known scrutinee (Task 16): the taken arm's value
// is what a real Branch/Select tree would also compute, so this is a
// differential over the VALUE; `const_differential.rs` covers the structural
// half (no Select gate for this expression).
const s3 = Shape.Circle(3.0)

on start {
  var pass: int = 0
  var total: int = 0
  static var s: Shape = Shape.Circle(5.0)

  check(pass, total, CIRCLE_DISC == 1, "const variant-path discriminant is 1")
  check(pass, total, s.Discriminant == Shape.Circle.Discriminant, "runtime disc read == variant disc")
  check(pass, total, Shape.Rect.Discriminant == 2, "rect disc is 2")
  check(pass, total, CIRCLE_DISC == s.Discriminant, "const and runtime discriminants agree")
  check(pass, total, (match s3 { Circle(r) => r, Empty => 0.0, Rect(w, h) => w + h }) == 3.0, "const match folds")

  // Task 21: the built-in `Option<T>` prelude, constructed and matched via
  // its BARE variant names (`Some`/`None`, not `Option.Some`/`Option.None`) -
  // no `enum` declaration anywhere in this file.
  check(pass, total, (match Some(7) { Some(x) => x, None => 0 }) == 7, "bare Option round-trip")

  BroadcastChatMessage("const enums: " .. pass .. "/" .. total)
}
