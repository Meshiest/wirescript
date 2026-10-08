// Measures the runtime's per-tick loop ceiling. Paste into a grid and read
// chat. Nothing here can be answered by the compiler: the cost of a loop is
// paid at evaluation time, and only a running server knows where it stops.
//
// Three questions, one ladder each:
//   1. trivial  how many iterations of a near-empty body fit in one tick
//   2. heavy    the same with a body that has real gates, which separates an
//               invocation cap from a gate-evaluation cap
//   3. shared   whether two loops in one tick draw down one common budget
//
// A ladder climbs by ten and stops the moment a rung comes back short, comes
// back aborted, or costs more than DT_CEILING seconds. Without those stops an
// uncapped runtime would be handed a rung large enough to hang the server.
//
// Iteration counts alone do not say what was paid. Body sizes, from
// `just ir examples/loop_budget_bench.ws`: the trivial body is 3 nodes, the
// heavy body is 90. Gate evaluations for a rung are iterations times that, so
// equal iteration counts across the two ladders mean the cap is on invocations
// and a heavy count near one thirtieth of the trivial one means it is on gate
// evaluations instead.

const RUNG_START: int = 100
const RUNGS: int = 6
const DT_CEILING: float = 1.0

var phase: int = 0
var rung: int = 0
var budget: int = RUNG_START
// Read back in the summary line. Without a reader the bodies are dead and the
// optimizer is free to delete the work being measured.
var acc: int = 1

var pending: bool = false
var pendBudget: int = 0
var pendIters: int = 0
var pendAborted: bool = false

var trivialBest: int = 0
var heavyBest: int = 0

// One multiply-shift-mask round. The heavy body runs eight of these, so its
// cost is a known multiple of one mix instead of an estimate.
mod mix(v: *int) {
  v = v * 1103515245 + 12345
  v = v ^ (v >> 16)
  v = v % 1000000007
}

buffer tick: int = tick + 1

on tick {
  if phase > 2 { return }

  // DeltaTime is the length of the tick that just ended, so it prices the rung
  // run on the previous tick, not the one about to run.
  let dt = DeltaTime()

  if pending {
    let short = pendIters < pendBudget
    let slow = dt > DT_CEILING
    let capped = short || pendAborted
    let name = if phase == 0 then "trivial" else "heavy"
    BroadcastChatMessage("${name} ${pendBudget}: ran ${pendIters} aborted ${pendAborted} tick ${dt}s")
    pending = false

    if phase == 0 { trivialBest = pendIters }
    if phase == 1 { heavyBest = pendIters }

    if capped {
      BroadcastChatMessage("${name} capped at ${pendIters} of ${pendBudget}")
    }
    if slow && !capped {
      BroadcastChatMessage("${name} stopped on wall clock: ${pendBudget} cost ${dt}s")
    }

    if capped || slow || rung >= RUNGS - 1 {
      phase = phase + 1
      rung = 0
      budget = RUNG_START
    } else {
      rung = rung + 1
      budget = budget * 10
    }
  }

  if phase == 0 {
    let runT = for _ in 0..budget {
      acc = acc + 1
    }
    pendBudget = budget
    pendIters = runT.iterations
    pendAborted = runT.aborted
    pending = true
  }

  if phase == 1 {
    let runH = for _ in 0..budget {
      mix(acc)
      mix(acc)
      mix(acc)
      mix(acc)
      mix(acc)
      mix(acc)
      mix(acc)
      mix(acc)
    }
    pendBudget = budget
    pendIters = runH.iterations
    pendAborted = runH.aborted
    pending = true
  }

  // Two loops in one tick, each asking for the whole cap. Sizing them at half
  // the cap cannot tell the two answers apart, because a shared budget and a
  // per-loop budget both let two halves through. At the full cap a per-loop
  // budget gives two full runs and a shared one starves the second.
  if phase == 2 {
    let ask = trivialBest
    let runA = for _ in 0..ask {
      acc = acc + 1
    }
    let runB = for _ in 0..ask {
      acc = acc + 1
    }
    BroadcastChatMessage("shared: asked ${ask} twice, got ${runA.iterations} aborted ${runA.aborted} then ${runB.iterations} aborted ${runB.aborted}")
    BroadcastChatMessage("summary: trivial ${trivialBest}, heavy ${heavyBest}, acc ${acc}")
    phase = phase + 1
  }
}
