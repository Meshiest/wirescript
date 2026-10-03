// Guards payload-free enums: a built-in game enum and a user enum cross a
// top-level `in`/`out` pin as their tag, and read back as the same variant
// from vars, arrays, `match`, `is` and mod outputs.
//
// The value checks need no wiring. The port round trip needs these outer pins
// wired after pasting:
//   easeOut -> easeIn, hueOut -> hueIn, easeLive -> easeLiveIn, hueLive -> hueLiveIn
// An `in` still at tag 0 is reported as unwired, and its check does not run.

enum Hue { Unset, Red, Green, Blue }

in easeIn: EasingFunction
in hueIn: Hue
in easeLiveIn: EasingFunction
in hueLiveIn: Hue

out easeOut: EasingFunction = EasingFunction.Bounce
out hueOut: Hue = Hue.Blue
out easeLive: EasingFunction = EasingFunction.Sine
out hueLive: Hue = Hue.Red

let start = ReadBrickGrid()

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

mod pickIf(n: int) -> Hue {
  if n == 1 { return Hue.Red }
  if n == 2 { return Hue.Green }
  return Hue.Blue
}

mod pickMatch(h: Hue) -> Hue {
  return match h {
    Red => Hue.Green,
    Green => Hue.Blue,
    Blue => Hue.Red,
    Unset => Hue.Unset,
  }
}

mod rotate(h: Hue) -> (r: Hue) {
  emit r = pickMatch(h)
}

mod easeFor(n: int) -> EasingFunction {
  if n == 1 { return EasingFunction.Bounce }
  return EasingFunction.Sine
}

var ns: int[]
var hs: Hue[]
var es: EasingFunction[]
var fires: int = 0

on start {
  fires += 1
  if fires > 1 { return }

  var pass: int = 0
  var total: int = 0
  ns.push(1)
  ns.push(2)
  ns.push(3)

  var h: Hue = pickIf(ns[0])
  check(pass, total, h is Hue.Red, "mod returning an enum through if")
  hs.push(pickIf(ns[2]))
  let first = hs[0]
  hs.push(pickMatch(first))
  hs.push(rotate(h))
  check(pass, total, hs[0] is Hue.Blue, "enum array element 0")
  check(pass, total, hs[1] is Hue.Red, "mod returning an enum through match")
  check(pass, total, hs[2] is Hue.Green, "emit to a mod enum output")
  check(pass, total, !(hs[2] is Hue.Red), "is rejects another variant")
  check(pass, total, hs[2].Discriminant == Hue.Green.Discriminant, "stored tag equals the variant tag")
  let m = match hs[1] { Red => 10, Green => 20, Blue => 30, Unset => 0 }
  check(pass, total, m == 10, "match on an array element")
  let sel = if ns[1] == 2 then hs[0] else hs[1]
  check(pass, total, sel is Hue.Blue, "select between two stored enums")
  check(pass, total, sel.Discriminant == hs[0].Discriminant, "compare two stored enums by tag")
  check(pass, total, sel.Discriminant != hs[1].Discriminant, "compare two different stored enums by tag")

  var e: EasingFunction = easeFor(ns[0])
  es.push(e)
  es.push(easeFor(ns[1]))
  check(pass, total, es[0] is EasingFunction.Bounce, "game enum stored in an array")
  check(pass, total, es[0].Discriminant == EasingFunction.Bounce.Discriminant, "game enum stored tag equals the variant tag")
  let em = match es[1] { Sine => 1, Bounce => 2, _ => 0 }
  check(pass, total, em == 1, "match on a stored game enum")
  BroadcastChatMessage("enum values: " .. pass .. "/" .. total)

  emit hueLive = rotate(h)
  emit easeLive = easeFor(ns[0])
  await SleepTicks(_, delay = 10)

  var rp: int = 0
  var rt: int = 0
  var unwired: int = 0
  if easeIn.Discriminant == 0 {
    unwired += 1
    BroadcastChatMessage("SKIP: easeOut -> easeIn is not wired")
  } else {
    check(rp, rt, easeIn is EasingFunction.Bounce, "round trip of a game enum default through a pin (not its numbering)")
  }
  if hueIn.Discriminant == 0 {
    unwired += 1
    BroadcastChatMessage("SKIP: hueOut -> hueIn is not wired")
  } else {
    check(rp, rt, hueIn is Hue.Blue, "round trip of a user enum default through a pin")
  }
  if easeLiveIn.Discriminant == 0 {
    unwired += 1
    BroadcastChatMessage("SKIP: easeLive -> easeLiveIn is not wired")
  } else {
    check(rp, rt, easeLiveIn is EasingFunction.Bounce, "round trip of an emitted game enum through a pin (not its numbering)")
  }
  if hueLiveIn.Discriminant == 0 {
    unwired += 1
    BroadcastChatMessage("SKIP: hueLive -> hueLiveIn is not wired")
  } else {
    check(rp, rt, hueLiveIn is Hue.Green, "round trip of an emitted user enum through a pin")
  }
  if unwired == 0 {
    BroadcastChatMessage("enum port round trip: " .. rp .. "/" .. rt)
  } else {
    BroadcastChatMessage("enum port round trip: NOT RUN for " .. unwired .. " of 4 pins, " .. rp .. "/" .. rt .. " of the wired ones")
  }
}
