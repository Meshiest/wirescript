# Round Manager

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Payloads](../enums/payloads.md), [Exec Pins](../events/exec-pins.md), [Number Options](../templates/number-options.md)

</div>
<!-- /learn-links -->

Runs a game round: a lobby, a countdown, a timed round and a results
pause, then back to the lobby. It tells the rest of your build when a round
begins and ends, and a sign shows the phase and the time left.

<!-- toc -->
## Contents

- [You need](#you-need)
- [Code](#code)
- [Wire it](#wire-it)
- [Check it in game](#check-it-in-game)
- [Change it](#change-it)
- [How it works](#how-it-works)
<!-- /toc -->

## You need

- Two **Button**s: start and end early.
- A brick with a **Text Decal** for the phase and the time.
- Whatever the round controls: a **Point Light**, a door, other chips.

```wiring
button1 Held -> chip start
button2 Held -> chip stop
chip sign -> decal Text
chip playing -> light Enabled
chip began -> circuit "exec input"
chip ended -> circuit "exec input"
```

## Code

```wirescript name=gs4-round-manager
const COUNTDOWN = 5 // seconds from start to the round beginning
const ROUND = 120   // seconds a round lasts
const RESULTS = 10  // seconds to show "Round over" before the lobby

enum Phase { Lobby, Countdown { until: int }, Playing { until: int }, Over { until: int } }

mod pad2(n: int) -> string {
  let s = "0${n}"
  return s.Substring(s.Length() - 2, 2)
}

@left in start: exec
@left in stop: exec
var phase: Phase = Phase.Lobby
var secondsLeft: int = 0
@right out sign = match phase {
  Lobby => "Press start",
  Countdown { .. } => "Starting in ${secondsLeft.Value}",
  Playing { .. } => "${pad2(secondsLeft.Value / 60)}:${pad2(secondsLeft.Value % 60)}",
  Over { .. } => "Round over",
}
@right out playing = phase is Phase.Playing
@right out began: exec
@right out ended: exec

on start {
  if phase is Phase.Lobby {
    secondsLeft = COUNTDOWN
    phase = Phase.Countdown { until: GetUnixTime() + COUNTDOWN }
  }
}

on stop {
  if phase is Phase.Playing {
    phase = Phase.Playing { until: GetUnixTime() }
  }
}

on Clock(interval = 0.25, enabled = !(phase is Phase.Lobby)) {
  let now = GetUnixTime()
  match phase {
    Lobby => {}
    Countdown { until } => {
      secondsLeft = until - now
      if now >= until {
        secondsLeft = ROUND
        phase = Phase.Playing { until: now + ROUND }
        emit began
      }
    }
    Playing { until } => {
      secondsLeft = until - now
      if now >= until {
        phase = Phase.Over { until: now + RESULTS }
        emit ended
      }
    }
    Over { until } => {
      if now >= until {
        phase = Phase.Lobby
      }
    }
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the start Button's **Held** to `start` and the other Button's
   **Held** to `stop`.
3. Wire `sign` to the Text Decal's **Text**.
4. Wire `playing` to anything that should be on only during a round, such as
   a Point Light's **Enabled**. Wire `began` and `ended` to exec inputs that
   should fire once at each edge of the round, such as a chip's reset pin.

## Check it in game

- [ ] The sign reads "Press start" and the light is off.
- [ ] Press start: "Starting in 5", 4, 3, 2, 1.
- [ ] The round begins: the sign counts down from `02:00`, the light comes
  on, and whatever `began` is wired to fires once.
- [ ] Press start during the round: nothing changes.
- [ ] Press end early: within a quarter of a second the sign reads "Round
  over", the light goes off and `ended` fires.
- [ ] Ten seconds later the sign reads "Press start" again.

## Change it

- Change the times: `ROUND = 300` for five minutes.
- Carry a winner into the results: add `winner: string` to `Over`, pass a
  name in through a pin, and show `"${winner} wins!"` in the sign's
  `Over` arm.
- Start automatically: wire a zone's **Occupant Count** to a new
  `@left in waiting: int` pin, and add `on Change(waiting)` with the body of
  `on start` inside `if waiting >= 2 { ... }`.

## How it works

The round is one [enum](../enums/payloads.md) value, and each phase except
the lobby carries `until`: the Unix time at which it ends. Outside the
lobby the Clock reads the time four times a second, to count the sign down
and to move the phase on when `now` passes `until`, so one `match` holds
all the rules. In the lobby nothing counts down, and the Clock is off until
`start`. "End early" sets the playing phase's end to now, and the Clock
ends the round at its next pulse. The exec outputs ([Exec Pins](../events/exec-pins.md))
are emitted only from the Clock handler, so every phase change happens in
one place.

The times are `GetUnixTime()`, the server's clock in whole seconds, and the
phase is a var saved with the bricks. After a server restart the round
carries on where the clock says it should be, and a phase whose end passed
while the server was down moves on at the Clock's next pulse. The `mm:ss` text uses
`pad2` from [Number Options](../templates/number-options.md).

Back to [Parts](README.md).
