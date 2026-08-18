// A generic test assertion, verified against `just check`. One `assert` replaces
// the per-type check helpers: it compares two values, counts the result, and on
// failure prints both, so a wrong value and a value that never arrived read
// differently. See docs/wirescript/testing.md.
//
// The bound is `int | float | string` because the failure line stringifies the
// values with `${...}`, which only those three support; `==` itself works for
// every variant, so widen the bound if a check never needs to print the value.

var pass: int = 0
var total: int = 0

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

on ReadBrickGrid() {

  var xs: int[]
  xs.push(10)
  xs.push(20)

  assert(2, xs.length(), "length after two pushes") // T = int
  assert(20, xs[1], "second element") // T = int
  assert("keep", "keep", "string equality") // T = string
  assert(12345, 12345, "control literal") // control value

  BroadcastChatMessage("checks: ${pass}/${total}")
}
