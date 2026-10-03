// Guards record assignment: a record local resets on every run of its call
// site, every source field is read before any field is written, and a call in
// a field runs once. Each wrong order still compiles and reads plausibly.

var pass: int = 0
var total: int = 0

mod assert(want: int, got: int, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

type P = { x: int, y: int }
type H = { xs: int[], n: int }

var p: P
var h: H
var a: int[]
var runs: int[]
var calls: int[]
var fires: int = 0

mod bump() -> int {
  var local: P = { x: 1, y: 2 }
  local.x += 10
  var fresh: P
  fresh.y += 1
  return local.x + fresh.y
}

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  assert(12, bump(), "record local initializer, first run")
  for i in 0..3 {
    calls.push(bump())
  }
  assert(3, calls.length(), "loop ran the call site three times")
  assert(12, calls[2], "record local initializer runs on every call")
  for i in 0..3 {
    var q: P = { x: 5, y: 0 }
    q.x += 1
    runs.push(q.x)
  }
  assert(6, runs[0], "record local initializer, first iteration")
  assert(6, runs[2], "record local initializer resets each iteration")

  p = { x: 1, y: 2 }
  p = { x: p.y, y: p.x }
  assert(2, p.x, "field swap x")
  assert(1, p.y, "field swap y")

  a.push(1)
  a.push(2)
  a.push(3)
  p = { x: a.pop(), y: 0 }
  assert(3, p.x, "a call in a record field runs once")
  assert(2, a.length(), "a call in a record field pops once")

  h = { xs: [1, 2, 3], n: 0 }
  h = { xs: [9], n: h.xs.length() }
  assert(3, h.n, "a field reads the old contents before the rebuild")
  assert(1, h.xs.length(), "the rebuild still happens")
  assert(9, h.xs[0], "the rebuilt contents")

  BroadcastChatMessage("record assign: ${pass}/${total}")
}
