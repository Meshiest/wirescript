// The registry use case: each color var registers its own ref with a
// controller over a custom event, and the controller recolors every one of
// them through the stored refs. No wire runs from the controller to a target.

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

var p0: color = Color(0.1, 0.1, 0.1)
var p1: color = Color(0.1, 0.1, 0.1)
var p2: color = Color(0.1, 0.1, 0.1)
var p3: color = Color(0.1, 0.1, 0.1)
var registry: (*color)[]
var fires: int = 0

on CustomEvent("refs.reg") -> (r: *color) {
  registry.push(r)
}

mod paint(c: *color, to: color) {
  c = to
}

on Loaded() {
  fires += 1
  if fires > 1 { return }

  SendCustomEvent("refs.reg", &p0)
  SendCustomEvent("refs.reg", &p1)
  SendCustomEvent("refs.reg", &p2)
  SendCustomEvent("refs.reg", &p3)
  await SleepTicks(_, delay = 10)
  assert(4, registry.length(), "four targets registered")

  let red = Color(1.0, 0.0, 0.0)
  *registry[0] = red
  *registry[1] = red
  paint(registry[2], red)
  paint(registry[3], red)
  assert(red.ToHex(), p0.ToHex(), "p0 recolored through its ref")
  assert(red.ToHex(), p1.ToHex(), "p1 recolored through its ref")
  assert(red.ToHex(), p2.ToHex(), "p2 recolored through its ref")
  assert(red.ToHex(), p3.ToHex(), "p3 recolored through its ref")

  BroadcastChatMessage("event registry: ${pass}/${total}")
}
