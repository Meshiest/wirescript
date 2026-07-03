// Async Signals — emit values, await triggers, local exec signals
//
// These features let you build event-driven pipelines where
// one handler fires a signal and another picks up where it left off.

// Local exec signals — synchronization points
let ready: exec
let done: exec

// State
var score: int = 0
var phase: int = 0
out result: int
out status: int = 0

in player: character
in start: exec
in cancel: exec

// emit target = expr — set output value and fire exec
on player {
  score = score + 10
  phase = phase + 1
  emit result = score
  if phase >= 3 {
    emit ready
  }
}

// await — suspend exec chain, resume when signal fires
on start {
  score = 0
  phase = 0

  // everything after this line runs when 'ready' fires
  await ready

  player.DisplayText(
    "Phase complete! score=${score}",
    fontSize = 24,
    lifetime = 5.0,
    textId = 0
  )

  emit done
}

// let foo = await expr — capture the exec expression's value
on start {
  let finalScore = await score on done
  emit status = finalScore
}

// await a || b — race, first signal wins
on start {
  await done || cancel
  player.DisplayText(
    "Finished or cancelled",
    fontSize = 18,
    lifetime = 3.0,
    textId = 1
  )
}

// Sleep -- _ is the await armed flag, delayed by the buffer gate
on start {
  await SleepTicks(_, delay = 60)
  player.DisplayText(
    "Slept for 1 second!",
    fontSize = 18,
    lifetime = 3.0,
    textId = 2
  )
}
