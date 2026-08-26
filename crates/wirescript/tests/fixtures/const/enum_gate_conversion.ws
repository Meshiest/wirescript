// Differential for the gate-backed enum <-> int conversions. `EnumToInteger`
// is the twin of `.ToInt()`/`.Discriminant`: a compile-time-known enum folds to
// its discriminant literal (no gate), a runtime enum routes through the real
// gate. `IntegerToEnum(n, wrap?)` is the twin of `Enum.FromInt(n)`: a constant
// int folds to the enum record, a runtime int routes through the real gate, and
// the result enum's concrete type comes from the annotated target. Any
// disagreement between the folded and gate paths is a bug. Run in game: build
// it, read chat.

let start = ReadBrickGrid()

enum Shape { Empty, Circle(float), Rect(float, float) }

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

// Folded at compile time: EnumToInteger of a known variant is its discriminant.
const CIRCLE_INT = EnumToInteger(Shape.Circle(1.0))

on start {
  var pass: int = 0
  var total: int = 0
  static var s: Shape = Shape.Circle(5.0)

  // EnumToInteger agrees with `.ToInt()` on a live value and folds to the same
  // constant a known variant does.
  check(pass, total, EnumToInteger(s) == s.ToInt(), "EnumToInteger equals ToInt at runtime")
  check(pass, total, CIRCLE_INT == 1, "const EnumToInteger(Circle) folds to 1")
  check(pass, total, EnumToInteger(s) == CIRCLE_INT, "runtime EnumToInteger equals const")

  // IntegerToEnum with a constant tag folds to the enum record; disc 2 is Rect,
  // and its payload defaults to 0.
  let const_e: Shape = IntegerToEnum(2)
  let const_picked = match const_e { Rect(w, h) => w, Circle(r) => -1.0, Empty => -2.0 }
  check(pass, total, EnumToInteger(const_e) == 2, "const IntegerToEnum(2) tag is 2")
  check(pass, total, const_picked == 0.0, "const IntegerToEnum(2) is Rect with defaulted payload")

  // IntegerToEnum with a runtime tag routes through the gate; a round-trip back
  // through EnumToInteger recovers the tag, and a match reads the runtime tag.
  static var tag: int = 2
  let live_e: Shape = IntegerToEnum(tag)
  let live_picked = match live_e { Rect(w, h) => 2, Circle(r) => 1, Empty => 0 }
  check(pass, total, EnumToInteger(live_e) == tag, "runtime IntegerToEnum round-trips through EnumToInteger")
  check(pass, total, live_picked == 2, "runtime IntegerToEnum(2) matches Rect")

  BroadcastChatMessage("enum gate conv: " .. pass .. "/" .. total)
}
