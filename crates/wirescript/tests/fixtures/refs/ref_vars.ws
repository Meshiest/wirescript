// Scalar refs as values: `&x` makes one, `*h` reads and writes through it, and
// a `*T` value binds a `*T` parameter of an inline mod and of a chip.

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
var h: *int
var k: *int
var fires: int = 0

mod bump(p: *int, by: int) {
  p += by
}

chip store(p: *int, v: int) {
  p = v
}

on Loaded() {
  fires += 1
  if fires > 1 { return }

  h = &a
  *h = 5
  assert(5, a, "a write through h reaches a")
  assert(5, *h, "a read through h sees a")
  *h += 2
  assert(7, a, "an increment through h reaches a")

  k = h
  assert(1, 0 + (k == h), "a copied ref equals its original")
  *k += 1
  assert(8, a, "a write through a copied ref reaches the original")
  k = &b
  assert(0, 0 + (k == h), "refs to different vars differ")
  BroadcastChatMessage("info: a stored var ref prints as [${h}]")

  await SleepTicks(_, delay = 2)
  *h += 1
  assert(9, a, "a stored ref still reaches its var a few ticks later")

  bump(h, 10)
  assert(19, a, "a *int value binds a mod's *int parameter")
  bump(k, 3)
  assert(3, b, "the same mod through another ref")
  bump(a, 1)
  assert(20, a, "a plain var still binds by alias")

  store(k, 42)
  assert(42, b, "a *int value binds a chip's *int parameter")

  let pick = if a > 0 then &a else &b
  *pick = 1
  assert(1, a, "an if-expression selects between refs")

  BroadcastChatMessage("ref vars: ${pass}/${total}")
}
