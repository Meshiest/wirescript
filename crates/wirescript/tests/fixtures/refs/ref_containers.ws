// Refs held in containers: an array of refs and a map of refs, each element
// read back and used to read and write its target in place.

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

var a: int = 0
var b: int = 0
var c: int = 0
var regs: (*int)[]
var byName: Map<string, *int>
var fires: int = 0

mod total3(xs: (*int)[]) -> int {
  return *xs[0] + *xs[1] + *xs[2]
}

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  regs.push(&a)
  regs.push(&b)
  regs.push(&c)
  assert(3, regs.length(), "three refs pushed")
  *regs[0] = 1
  *regs[1] = 2
  *regs[2] = 3
  assert(1, a, "element 0 writes a")
  assert(2, b, "element 1 writes b")
  assert(3, c, "element 2 writes c")
  assert(6, total3(regs), "a read through each element")
  assert(1, 0 + (regs[1] == &b), "an element equals the ref it holds")

  byName.set("b", &b)
  let r = byName["b"]
  *r += 40
  assert(42, b, "a map value writes through")
  assert(42, *byName["b"], "a map value reads through")

  BroadcastChatMessage("ref containers: ${pass}/${total}")
}
