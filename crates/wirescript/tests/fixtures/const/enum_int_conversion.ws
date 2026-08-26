// Differential for the enum <-> int conversions. `.ToInt()` is an exact alias
// for `.Discriminant`; `Enum.FromInt(n)` builds a value with `__disc = n` and
// every payload defaulted. `.ToInt()` is checked both folded at compile time
// (through a top-level `const`) and read back at runtime off a live value;
// `FromInt` is exercised at runtime, including a match that routes by the tag
// and reads a defaulted payload. Any disagreement is a const-evaluation bug.
// Run in game: build it, read chat.

let start = ReadBrickGrid()

enum Shape { Empty, Circle(float), Rect(float, float) }

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

// Folded at compile time: a variant path's `.ToInt()` is its discriminant.
const CIRCLE_TOINT = Shape.Circle.ToInt()

on start {
  var pass: int = 0
  var total: int = 0
  static var s: Shape = Shape.Circle(5.0)

  // `.ToInt()` agrees with `.Discriminant` on a live value and folds to the
  // same constant a variant path does.
  check(pass, total, s.ToInt() == s.Discriminant, "ToInt equals Discriminant at runtime")
  check(pass, total, CIRCLE_TOINT == 1, "const Circle.ToInt() folds to 1")
  check(pass, total, s.ToInt() == CIRCLE_TOINT, "runtime ToInt equals const ToInt")

  // `FromInt(2)` builds a value whose tag is 2 (Rect); its `.ToInt()` reads
  // that tag back, and a match routes to Rect with the payload defaulted to 0.
  let built = Shape.FromInt(2)
  let picked = match built { Rect(w, h) => w, Circle(r) => -1.0, Empty => -2.0 }
  check(pass, total, built.ToInt() == 2, "FromInt(2) tag is 2")
  check(pass, total, picked == 0.0, "FromInt(2) match takes Rect with defaulted 0 payload")

  BroadcastChatMessage("enum int conv: " .. pass .. "/" .. total)
}
