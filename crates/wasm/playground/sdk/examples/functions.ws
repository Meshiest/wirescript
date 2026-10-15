// Functions - `fn` compiles once, every call is one Function Call gate
//
// A mod is copied into each place it is called. A fn's body is built once
// on its own plane, so ten calls cost ten small gates, not ten bodies.

var hits: int = 0
var combo: int = 0

/// Trailing parameters take default values
fn damage(base: int, bonus: int = 0, cap: int = 100) -> int {
  hits += 1
  return min(base + bonus, cap)
}

/// Several named results
fn divmod(a: int, b: int) -> (q: int, r: int) {
  return (a / b, a % b)
}

/// A *T parameter is passed by reference and changed in place
fn bump(v: *int, by: int = 1) {
  *v = *v + by
}

in go: exec
on go {
  let a = damage(30)
  let b = damage(80, bonus = 40)
  let r = divmod(17, 5)
  bump(&combo)
  bump(&combo, 5)
  BroadcastChatMessage("damage ${a} and ${b} after ${hits} hits")
  BroadcastChatMessage("17 / 5 = ${r.q} r ${r.r}, combo ${combo}")
}

// Outside a handler, `exec =` fires the call, and the body runs once it
// completes with the results bound by `-> (...)`
in reroll: exec
on damage(10, bonus = 5, exec = reroll) -> (d) {
  BroadcastChatMessage("rerolled ${d}")
}
