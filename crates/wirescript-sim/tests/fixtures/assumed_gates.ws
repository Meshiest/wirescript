// Gates whose laws the simulator first took from the game's descriptions and
// probe 20 measured (projects/sim/ENGINE_MODEL.md). Each test prints a FAIL
// line when a law changes.

@test fn blending() {
  let half = Blend(2.0, 4.0, 0.25)
  if half != 2.5 {
    BroadcastChatMessage("FAIL: Blend(2.0, 4.0, 0.25) is ${half}")
  }
  let ints = Blend(10, 20, 0.5)
  if ints != 15 {
    BroadcastChatMessage("FAIL: Blend(10, 20, 0.5) is ${ints}")
  }
  let quadIn = Easing(0.0, 1.0, 0.5, function = Quad, direction = In)
  let quadOut = Easing(0.0, 1.0, 0.5, function = Quad, direction = Out)
  if quadIn != 0.25 || quadOut != 0.75 {
    BroadcastChatMessage("FAIL: Quad easing at 0.5 is ${quadIn} in, ${quadOut} out")
  }
  let mid = Remap(50.0, 0.0, 100.0, 0.0, 1.0)
  let past = Remap(150.0, 0.0, 100.0, 0.0, 1.0)
  if mid != 0.5 || past != 1.5 {
    BroadcastChatMessage("FAIL: Remap gives ${mid} and ${past}")
  }
}

@test fn comparing_and_shifting() {
  let shifted = LogicalShiftRight(-1, 1)
  if shifted != 9223372036854775807 {
    BroadcastChatMessage("FAIL: LogicalShiftRight(-1, 1) is ${shifted}")
  }
  if !NearlyEqual(1.001, 1.0, 0.01) || NearlyEqual(1.1, 1.0, 0.01) {
    BroadcastChatMessage("FAIL: NearlyEqual with tolerance 0.01")
  }
  let pair = Swap(true, 1, 2)
  if pair.Output != 2 || pair.OutputB != 1 {
    BroadcastChatMessage("FAIL: Swap(true, 1, 2) is ${pair.Output}, ${pair.OutputB}")
  }
}

@test fn taking_text_apart() {
  let a = "A".ToCharCode()
  if a.Codepoint != 65 || !a.Success {
    BroadcastChatMessage("FAIL: A is codepoint ${a.Codepoint}")
  }
  if "".ToCharCode().Success {
    BroadcastChatMessage("FAIL: empty text has a codepoint")
  }
  let block = FromCharCode(9608)
  if block.Character != "█" || !block.Success {
    BroadcastChatMessage("FAIL: codepoint 9608 is ${block.Character}")
  }
  let first = "a b c".Split(" ")
  if first.Left != "a" || first.Right != "b c" {
    BroadcastChatMessage("FAIL: the first space splits into ${first.Left} | ${first.Right}")
  }
  let second = "a b c".Split(" ", 2)
  if second.Left != "a b" || second.Right != "c" || !second.Found {
    BroadcastChatMessage("FAIL: the second space splits into ${second.Left} | ${second.Right}")
  }
  let missing = "abc".Split(" ")
  if missing.Found || missing.Left != "abc" || missing.Right != "" {
    BroadcastChatMessage("FAIL: a missing delimiter splits into ${missing.Left} | ${missing.Right}")
  }
}

@test fn splitting_values() {
  let v = SplitVec(Vec(1.0, 2.0, 3.0))
  if v.x != 1.0 || v.y != 2.0 || v.z != 3.0 {
    BroadcastChatMessage("FAIL: SplitVec gives ${v.x} ${v.y} ${v.z}")
  }
  let r = Rotation(10.0, 20.0, 30.0).ToEuler()
  if r.Pitch != 10.0 || r.Yaw != 20.0 || r.Roll != 30.0 {
    BroadcastChatMessage("FAIL: ToEuler gives ${r.Pitch} ${r.Yaw} ${r.Roll}")
  }
  let q = Quat(0.0, 0.0, 0.7071067811865476, 0.7071067811865476)
  let parts = SplitQuat(q)
  if parts.W != 0.7071067811865476 {
    BroadcastChatMessage("FAIL: SplitQuat W is ${parts.W}")
  }
  let turn = q.ToAxisAngle()
  if !NearlyEqual(turn.Angle, 90.0, 0.0001) || !NearlyEqual(turn.Axis.Z, 1.0, 0.0001) {
    BroadcastChatMessage("FAIL: a quarter turn about Z is ${turn.Angle} about ${turn.Axis}")
  }
}

var lit: bool = false
var hp: float = 0.0
var rises: int = 0
var falls: int = 0
var ups: int = 0
var downs: int = 0

let edges = Edge(lit)
on edges.Rising {
  rises += 1
}
on edges.Falling {
  falls += 1
}
let health = EdgeExec(hp)
on health.Rising {
  ups += 1
}
on health.Falling {
  downs += 1
}

@test mod edges_fire_once_per_change() {
  lit = true
  await SleepTicks(_, delay = 3)
  lit = false
  await SleepTicks(_, delay = 3)
  hp = 20.0
  await SleepTicks(_, delay = 2)
  hp = 5.0
  await SleepTicks(_, delay = 2)
  if rises != 1 || falls != 1 {
    BroadcastChatMessage("FAIL: Edge rose ${rises} and fell ${falls} times")
  }
  if ups != 1 || downs != 1 {
    BroadcastChatMessage("FAIL: EdgeExec rose ${ups} and fell ${downs} times")
  }
}
