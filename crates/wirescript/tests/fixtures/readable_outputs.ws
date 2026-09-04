// In-game check for readable output ports and handler-declared ports. Paste
// the compiled grid into a build and read the chat: a healthy run prints only
// the two tally lines.

let start = ReadBrickGrid()

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

static var seed: int = 7

// A port read is a wire from the port's rerouter, and one port may feed
// another.
out doubled: int = seed * 2
out quadrupled: int = doubled * 2

// A same-named var must still win the first resolution tier: this port reads
// the var, and every later read of `count` reads the var too, never the port.
static var count: int = 3
out count: int = count + 1

on start {
  var pass: int = 0
  var total: int = 0
  check(pass, total, doubled == 14, "port carries its driver")
  check(pass, total, quadrupled == 28, "one port feeds another")
  check(pass, total, count == 3, "a same-named var wins over the port")
  BroadcastChatMessage("readable outputs: " .. pass .. "/" .. total)
}

// A port declared inside a top-level handler. One unconditional site, so its
// value wires straight into the port with no backing var.
static var ticks: int = 0
static var reported: bool = false
on Clock(interval = 0.2) {
  ticks = ticks + 1
  out counted: int = ticks
  if !reported {
    if ticks > 2 {
      reported = true
      var pass: int = 0
      var total: int = 0
      check(pass, total, counted == ticks, "handler-declared port carries its value")
      BroadcastChatMessage("handler out: " .. pass .. "/" .. total)
    }
  }
}
