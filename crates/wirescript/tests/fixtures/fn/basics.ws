// `fn`: one definition plane, a Function Call per call site, outer storage
// reached through handles. Paste it: it checks itself when the grid loads.
// FAIL lines must not appear.

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

var sum: int = 0
var seen: int[]
var byKey: Map<int, int>

fn add(x: int, y: int = 1) -> int {
  sum += x
  seen.push(x)
  byKey.set(x, y)
  return x + y
}

fn split(p: { a: int, b: int }) -> (s: int, d: int) {
  return (p.a + p.b, p.a - p.b)
}

fn bump(v: *int) {
  *v = *v + 1
}

var counter: int = 0
var fires: int = 0

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  assert(4, add(3), "a default fills an omitted argument")
  assert(9, add(4, 5), "a second call site")
  assert(7, sum, "both calls wrote the outer var through a handle")
  assert(2, seen.length(), "both calls pushed onto the outer array")
  assert(5, byKey.get(4).Value, "the outer map holds the second call's entry")

  let r = split({ a: 10, b: 3 })
  assert(13, r.s, "a record argument, first result")
  assert(7, r.d, "second result")

  bump(&counter)
  bump(&counter)
  assert(2, counter, "a ref parameter writes the caller's var")

  BroadcastChatMessage("fn basics: ${pass}/${total}")
}
