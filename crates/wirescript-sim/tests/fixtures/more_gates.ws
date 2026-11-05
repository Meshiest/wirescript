// Stateful and timed gates: Toggle, Cycle, Random, Sort Multiple, Tween,
// Dampen, the container change detector, and the date and color gates.

@test mod toggles_cycles_and_rolls() {
  var lit: bool = false
  var step: int = 0
  for i in 0..3 {
    lit = Toggle()
    step = Cycle(2)
  }
  if !lit || step != 1 {
    BroadcastChatMessage("FAIL: after three pulses Toggle is ${lit} and Cycle(2) is ${step}")
  }
  var lo: int = 100
  var hi: int = -100
  for i in 0..200 {
    let r = Random(3, 5)
    lo = min(lo, r)
    hi = max(hi, r)
  }
  if lo != 3 || hi != 5 {
    BroadcastChatMessage("FAIL: Random(3, 5) ranged ${lo} to ${hi}")
  }
  PrintToConsole("rolled")
}

var keys: int[]
var names: string[]

@test mod sorting_together() {
  keys.clear()
  names.clear()
  keys.push(3)
  names.push("c")
  keys.push(1)
  names.push("a")
  keys.push(2)
  names.push("b")
  keys.sortMultiple(names)
  if names.join(",") != "a,b,c" || keys.join(",") != "1,2,3" {
    BroadcastChatMessage("FAIL: sorted together into ${keys.join(",")} and ${names.join(",")}")
  }
}

@test fn dates_and_colors() {
  let day = FormatDate(0, "%Y-%m-%d %H:%M", useUTC = true)
  if day.Output != "1970-01-01 00:00" {
    BroadcastChatMessage("FAIL: epoch 0 formats as ${day.Output}")
  }
  if GetUnixTime() < 1767225600 {
    BroadcastChatMessage("FAIL: the clock reads ${GetUnixTime()}")
  }
  let back = ConvertColor(ConvertColor(Color(0.2, 0.5, 0.8, 1.0), fromSpace = 0, toSpace = 3), fromSpace = 3, toSpace = 0)
  if back.ToHex() != Color(0.2, 0.5, 0.8, 1.0).ToHex() {
    BroadcastChatMessage("FAIL: linear to HSV and back is ${back.ToHex()}")
  }
}

var goal: float = 0.0
var arrived: int = 0
var writes: int = 0
var bag: int[]

let moving = Tween(goal, 0.1)
on moving.Arrived {
  arrived += 1
}
let smooth = Dampen(goal, 0.05)
let touched = ContainerChange(bag)
on touched {
  writes += 1
}

@test mod timers_follow_their_target() {
  goal = 10.0
  await SleepTicks(_, delay = 12)
  if moving.Value != 10.0 || arrived != 1 {
    BroadcastChatMessage("FAIL: Tween is at ${moving.Value}, arrived ${arrived} times")
  }
  if !NearlyEqual(smooth, 10.0, 0.5) {
    BroadcastChatMessage("FAIL: Dampen is at ${smooth}")
  }
  bag.push(1)
  await SleepTicks(_, delay = 2)
  if writes != 1 {
    BroadcastChatMessage("FAIL: the container change fired ${writes} times")
  }
}
