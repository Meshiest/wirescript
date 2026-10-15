# Whack-a-Light

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Payloads](../enums/payloads.md), [Mods](../abstraction/mods.md), [Dice](../random/dice.md), [Safe Text](../strings/safe-text.md), [Waiting](../events/waiting.md)

</div>
<!-- /learn-links -->

An arcade game on a 3 by 3 board of lights, each with a button in front of
it. One light at a time comes on: press its button before it goes out. Every
few hits the round goes up and the lights get faster. Three misses end the
game, and a sign keeps the high score.

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

- Nine **Point Light**s in a 3 by 3 grid, with a **Button** in front of each.
- One more **Button** to start a game.
- A brick with a **Text Decal** for the score.

```wiring
button1 Held -> chip b1
button2 Held -> chip b2
button3 Held -> chip b3
button4 Held -> chip b4
button5 Held -> chip b5
button6 Held -> chip b6
button7 Held -> chip b7
button8 Held -> chip b8
button9 Held -> chip b9
button10 Held -> chip start
button10 Character -> chip player
chip l1 -> light1 Enabled
chip l2 -> light2 Enabled
chip l3 -> light3 Enabled
chip l4 -> light4 Enabled
chip l5 -> light5 Enabled
chip l6 -> light6 Enabled
chip l7 -> light7 Enabled
chip l8 -> light8 Enabled
chip l9 -> light9 Enabled
chip sign -> decal Text
```

## Code

```wirescript name=cp2-whack-a-light
import { escape } from "sanitize"

const LIGHTS = 9       // lights on the board, numbered 1 to 9
const LIVES = 3        // misses before the game ends
const PER_ROUND = 5    // hits to finish a round
const WINDOW = 15      // tenths of a second a light stays on in round 1
const FASTER = 2       // tenths of a second less each round
const SHORTEST = 5     // the window never gets shorter than this
const GAP = 4          // tenths of a second between two lights
const FLASH = 8        // tenths of a second the board flashes after a miss

enum Phase { Ready, Gap(int), Lit { light: int, left: int }, Flash(int), Over }

@left in start: exec
@left in player: character
@left in b1: exec
@left in b2: exec
@left in b3: exec
@left in b4: exec
@left in b5: exec
@left in b6: exec
@left in b7: exec
@left in b8: exec
@left in b9: exec

var phase: Phase = Phase.Ready
var score: int = 0
var lives: int = 0
var name: string = ""
var best: int = 0
var bestName: string = "nobody"
var button: int = 0

let round = score.Value / PER_ROUND + 1
let lit = match phase {
  Lit { light, .. } => light,
  _ => 0,
}
let all = phase is Phase.Flash
let idle = phase is Phase.Ready || phase is Phase.Over
@right out l1 = lit == 1 || all
@right out l2 = lit == 2 || all
@right out l3 = lit == 3 || all
@right out l4 = lit == 4 || all
@right out l5 = lit == 5 || all
@right out l6 = lit == 6 || all
@right out l7 = lit == 7 || all
@right out l8 = lit == 8 || all
@right out l9 = lit == 9 || all
@right out sign = match phase {
  Ready => "Press start<br>Best: ${bestName.Value} ${best.Value}",
  Over => "Game over! ${name.Value} scored ${score.Value}<br>Best: ${bestName.Value} ${best.Value}",
  _ => "Round ${round}<br>Score ${score.Value}  Lives ${lives.Value}",
}

let press: exec

mod window() -> int {
  return max(WINDOW - (round - 1) * FASTER, SHORTEST)
}

mod miss() {
  let left = lives - 1
  lives = left
  if left > 0 {
    phase = Phase.Flash(FLASH)
    return
  }
  phase = Phase.Over
  if score > best {
    best = score
    bestName = name
  }
}

on press {
  match phase {
    Lit { light, .. } => {
      if light == button {
        score += 1
        phase = Phase.Gap(GAP)
      } else {
        miss()
      }
    }
    _ => {}
  }
}

on start {
  if phase is Phase.Ready || phase is Phase.Over {
    score = 0
    lives = LIVES
    name = escape(player.GetDisplayName())
    phase = Phase.Gap(GAP * 3)
  }
}

on b1 { button = 1  emit press }
on b2 { button = 2  emit press }
on b3 { button = 3  emit press }
on b4 { button = 4  emit press }
on b5 { button = 5  emit press }
on b6 { button = 6  emit press }
on b7 { button = 7  emit press }
on b8 { button = 8  emit press }
on b9 { button = 9  emit press }

on Clock(interval = 0.1, enabled = !idle) {
  match phase {
    Gap(left) => {
      if left > 1 {
        phase = Phase.Gap(left - 1)
      } else {
        phase = Phase.Lit { light: Random(1, LIGHTS), left: window() }
      }
    }
    Lit { light, left } => {
      if left > 1 {
        phase = Phase.Lit { light: light, left: left - 1 }
      } else {
        miss()
      }
    }
    Flash(left) => {
      if left > 1 {
        phase = Phase.Flash(left - 1)
      } else {
        phase = Phase.Gap(GAP)
      }
    }
    _ => {}
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Number the lights 1 to 9, left to right and top to bottom. Wire each
   light's **Enabled** from the pin with its number: `l1` to light 1, up to
   `l9`.
3. Wire the **Held** of the button in front of light 1 to `b1`, and so on to
   `b9`. Check each pair: a button wired to the wrong pin turns every hit on
   that light into a miss.
4. Wire the start Button's **Held** to `start` and its **Character** to
   `player`.
5. Wire `sign` to the Text Decal's **Text**.

## Check it in game

- [ ] The lights are off, and the sign reads "Press start" and "Best:
  nobody 0".
- [ ] Press start: the sign shows "Round 1", "Score 0" and "Lives 3".
  About a second later one light comes on.
- [ ] Press its button while it is lit: the score goes up, the light goes
  out, and another comes on soon after.
- [ ] Let a light go out without pressing: all nine lights flash, and the
  lives drop to 2.
- [ ] Press a button whose light is off while another is lit: the same miss.
- [ ] Score 5: the sign says "Round 2", and each light stays on for less time.
- [ ] Miss three times: "Game over!" with your name and score, the lights
  stay off, and if you scored at least one hit, the best score shows your
  name.
- [ ] Press start again: a new game, and the best score stays.

## Change it

- A bigger board: add `l10` to `l16` and `b10` to `b16` pins with their
  one-line handlers, and `LIGHTS = 16` for a 4 by 4 grid.
- An easier game: `WINDOW = 20`, `LIVES = 5`.
- Never light the same light twice in a row: keep the last light in a var,
  and in the `Gap` arm, when `Random` gives that one again, take the next
  light instead (`n % LIGHTS + 1`).
- A sound for every hit: add `@right out scored: exec`, put `emit scored`
  after `score += 1`, and wire `scored` to a **One Shot Audio**'s **Play**.

## How it works

The whole game is one [enum](../enums/payloads.md) value, `phase`. `Ready`
and `Over` wait for the start button. The other three carry a countdown in
Clock pulses: `Gap` is the pause before the next light, `Lit` holds which
light is on and how long it has left, and `Flash` is the pause after a miss.
Ten times a second the Clock's `match` counts the current phase down or
moves on to the next one, so all the timing is in one place. `idle` turns
the Clock off between games. Each button's
handler stores its number in `button` and fires `press`, the chip's own
signal, so the nine buttons share one `press` handler. It acts only in
`Lit`: the right button scores, any other is a miss. A press during a pause
does nothing, so a second press on the same light is safe.

Every output is computed from `phase`, never stored on its own. `lit` is
the number of the light that is on, `all` is true while the board flashes,
and each light compares its own number with them. The sign is a `match` on
the same value, and `round` is worked out from the score, so nothing can
drift out of step. `Random(1, LIGHTS)` picks the light
([Dice](../random/dice.md)), `max` keeps the window from getting too short,
and the player's name is escaped when it is stored ([Safe
Text](../strings/safe-text.md)). `miss` is a
[mod](../abstraction/mods.md), so its two callers, a wrong press and a
light that ran out, each get a copy. The buttons fire one shared signal
because nine copies of the press logic, one per button, would cost far more.

Back to [Parts](README.md).
