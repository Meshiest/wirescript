# Moving Platform

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Waiting](../events/waiting.md), [Choosing a Value](../logic/choosing-a-value.md)

</div>
<!-- /learn-links -->

A platform that carries players between two landings. Step on and it leaves
after a short wait, and a call button at each landing brings it over.

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

- a **Rigid Slider** laid along the path, with the platform welded to its
  moving end (the slider's own hint says how: weld it with the placer or
  manipulator)
- a brick with a **Touch** component on the platform's deck
- two **Button**s, one at each landing

```wiring
touch Touching -> chip rider
touch Character -> chip who
button1 Held -> chip callNear
button2 Held -> chip callFar
chip target -> mover "Target Position"
```

## Code

```wirescript name=pp10-platform
// The slider position of the far landing, with Positions Are Percentages on.
const FAR = 100.0
// Seconds between stepping on and leaving.
const DEPART = 2.0

@left in callNear: exec
@left in callFar: exec
@left in rider: bool
@left in who: character

var atFar: bool = false
@right out target: float = if atFar.Value then FAR else 0.0

on callNear {
  atFar = false
}

on callFar {
  atFar = true
}

on rider {
  who.ShowStatusMessage("Departing in ${DEPART} seconds")
  await Sleep(_, delay = DEPART)
  if rider {
    atFar = !atFar
  }
}
```

## Wire it

1. Compile the chip and paste it in-game near the near landing.
2. In the Rigid Slider's settings, turn on **Positions Are Percentages**.
3. Wire `target` to the slider's **Target Position**.
4. Wire the deck Touch's **Touching** to `rider` and its **Character** to
   `who`.
5. Wire the near landing's Button **Held** to `callNear`, and the far
   landing's to `callFar`.

## Check it in game

- Before anyone steps on: the platform rests at the near landing.
- Step onto the platform: a status message says it departs in two seconds,
  and then it slides to the far landing with you on it.
- Step off and back on: it carries you back.
- Step on and off again within two seconds: it stays where it is.
- Stand at the near landing with the platform away and press the button
  there: it comes back empty.

## Change it

- Change `DEPART` for a longer or shorter wait.
- Make it travel part of the way: a `FAR` of `50.0` stops halfway along the
  slider.
- Make it shuttle by itself: add an `on Clock(interval = 10.0)` handler that
  flips `atFar`, so the platform goes back and forth every ten seconds.
- Change how fast it moves with the slider's **Top Speed** and **Smooth
  Time** settings.

## How it works

The chip decides only where the platform should be: `atFar` is one stored
bool, and `if ... then ... else` turns it into the slider's target, a steady
signal the slider follows on its own ([Choosing a
Value](../logic/choosing-a-value.md)). The call buttons set `atFar` directly.
Stepping on starts a wait ([Waiting](../events/waiting.md)). After it,
`if rider` checks that someone is still on the deck before the platform
sets off, so a player who jumped straight off does not send it away empty.
