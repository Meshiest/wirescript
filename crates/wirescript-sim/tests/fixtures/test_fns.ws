// Tests the runner calls one at a time. Each says nothing when it passes and
// prints a FAIL line when it does not.
fn double(n: int) -> int {
  return n * 2
}

@test fn doubling() {
  if double(21) != 42 {
    BroadcastChatMessage("FAIL: double(21) is ${double(21)}")
  }
}

@test fn joining() {
  let s = "a" .. "b"
  if s != "ab" {
    BroadcastChatMessage("FAIL: joining gave ${s}")
  }
}

// A mod test runs inline, so it can use module state and wait across ticks.
var presses: int = 0

mod press() {
  presses += 1
}

@test mod counts_over_ticks() {
  press()
  await SleepTicks(_, delay = 3)
  press()
  if presses != 2 {
    BroadcastChatMessage("FAIL: presses is ${presses}")
  }
}

// A string stops at 32,768 bytes, so two-byte and three-byte characters fit
// half and a third as many; a longer one is cut at the last whole character.
@test mod strings_stop_at_the_cap() {
  var s: string = "x"
  var two: string = "é"
  var three: string = "█"
  for i in 0..16 {
    s = s .. s
    two = two .. two
    three = three .. three
  }
  if s.Length() != 32768 {
    BroadcastChatMessage("FAIL: a doubled string is ${s.Length()} long")
  }
  if two.Length() != 16384 {
    BroadcastChatMessage("FAIL: a two-byte string is ${two.Length()} long")
  }
  if three.Length() != 10922 {
    BroadcastChatMessage("FAIL: a three-byte string is ${three.Length()} long")
  }
}

// A mod's exec result fires where the body emits it, and not when it does
// not, alone or beside a value result.
var hits: int = 0

mod past(x: int) -> (hit: exec) {
  if x > 5 {
    emit hit
  }
}

mod doubled_past(x: int) -> (v: int, hit: exec) {
  emit v = x * 2
  if x > 5 {
    emit hit
  }
}

@test mod exec_results_fire_at_the_emit() {
  let low = past(3)
  on low.hit { hits += 1 }
  let high = past(7)
  on high.hit { hits += 10 }
  let both = doubled_past(8)
  on both.hit { hits += both.v * 100 }
  await SleepTicks(_, delay = 1)
  if hits != 1610 {
    BroadcastChatMessage("FAIL: exec results gave ${hits}, not 1610")
  }
}
