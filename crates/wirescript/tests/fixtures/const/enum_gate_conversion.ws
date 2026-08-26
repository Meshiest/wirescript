// Differential for the gate-backed enum <-> int conversions. `EnumToInt`
// is the twin of `.ToInt()`/`.Discriminant`: a compile-time-known enum folds to
// its discriminant literal (no gate), a runtime enum routes through the real
// gate. `IntToEnum(n, wrap?)` is the twin of `Enum.FromInt(n)`: a constant
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

// Folded at compile time: EnumToInt of a known variant is its discriminant.
const CIRCLE_INT = EnumToInt(Shape.Circle(1.0))

on start {
  var pass: int = 0
  var total: int = 0
  static var s: Shape = Shape.Circle(5.0)

  // EnumToInt agrees with `.ToInt()` on a live value and folds to the same
  // constant a known variant does.
  check(pass, total, EnumToInt(s) == s.ToInt(), "EnumToInt equals ToInt at runtime")
  check(pass, total, CIRCLE_INT == 1, "const EnumToInt(Circle) folds to 1")
  check(pass, total, EnumToInt(s) == CIRCLE_INT, "runtime EnumToInt equals const")

  // IntToEnum with a constant tag folds to the enum record; disc 2 is Rect,
  // and its payload defaults to 0.
  let const_e: Shape = IntToEnum(2)
  let const_picked = match const_e { Rect(w, h) => w, Circle(r) => -1.0, Empty => -2.0 }
  check(pass, total, EnumToInt(const_e) == 2, "const IntToEnum(2) tag is 2")
  check(pass, total, const_picked == 0.0, "const IntToEnum(2) is Rect with defaulted payload")

  // IntToEnum with a runtime tag routes through the gate; a round-trip back
  // through EnumToInt recovers the tag, and a match reads the runtime tag.
  static var tag: int = 2
  let live_e: Shape = IntToEnum(tag)
  let live_picked = match live_e { Rect(w, h) => 2, Circle(r) => 1, Empty => 0 }
  check(pass, total, EnumToInt(live_e) == tag, "runtime IntToEnum round-trips through EnumToInt")
  check(pass, total, live_picked == 2, "runtime IntToEnum(2) matches Rect")

  BroadcastChatMessage("enum gate conv: " .. pass .. "/" .. total)
}
