# Shooting Gallery

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Looping Over a Map](../loops/looping-over-a-map.md), [Rich Text](../strings/rich-text.md), [Per-Player State](../references/per-player-state.md)

</div>
<!-- /learn-links -->

A target and a scoreboard. Every hit scores a point for the player who fired
the shot, the shooter sees their running total, and a sign lists everyone's
hits. A button clears the board for the next round.

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

- a **Target** brick
- a brick with a **Text Decal**, for the scoreboard
- a **Button**, to clear the board

```wiring
target "Just Hit" -> chip hit
target "Character That Just Hit" -> chip shooter
button Held -> chip reset
chip board -> decal Text
```

## Code

```wirescript name=pp4-gallery
import { escape } from "sanitize"

// The first line of the board.
const TITLE = "<b>Shooting Gallery</b>"

@left in hit: exec
@left in shooter: character
@left in reset: exec

var hits: Map<string, int>
var names: Map<string, string>
var shown: string = TITLE
@right out board = shown.Value

on hit {
  let id = shooter.GetUserId()
  let old = hits.get(id)
  let count = if old.Found then old.Value + 1 else 1
  hits.set(id, count)
  names.set(id, escape(shooter.GetDisplayName()))
  shooter.ShowStatusMessage("Hit! ${count} so far")
  var text: string = TITLE
  for key, n in hits {
    let name = names.get(key)
    text = "${text}<br>${name}: ${n}"
  }
  shown = text
}

on reset {
  hits.clear()
  names.clear()
  shown = TITLE
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Target's **Just Hit** to `hit` and its **Character That Just
   Hit** to `shooter`.
3. Wire the Button's **Held** to `reset`.
4. Wire `board` to the Text Decal's **Text**.

## Check it in game

- Before any shot: the sign shows only the title.
- Shoot the target: the board shows a line with your name and `1`, and a
  status message says "Hit! 1 so far".
- Shoot it again, waiting a moment between shots: your line goes up by one
  each time.
- With a second player: each of you has a line, and each hit counts for the
  one who fired it.
- Press the button: the board is back to the title, and the next hit starts
  from 1.

## Change it

- Count fast shots: the Target's **Just Hit** stays on for its **On Time**
  setting, so a second hit while it is still on is likely to be missed.
  Lower **On Time** for fast weapons.
- Score by damage instead of hits: wire the Target's **Damage** to a new
  `@left in damage: float` pin, make `hits` a `Map<string, float>`, and add
  `damage` in place of the 1.
- Put several targets on one board: wire each Target into its own copy of
  `hit` and `shooter`, with a handler for each that calls a shared
  [mod](../abstraction/mods.md) to score the hit.

## How it works

`on hit` fires once per hit, on the moment **Just Hit** turns on, and
**Character That Just Hit** says who fired. Scores live in two maps keyed by
user id, one for the count and one for the name, so a player who respawns
keeps their score ([Maps](../values/maps.md)). The board
is rebuilt after every hit with a loop over the map
([Looping Over a Map](../loops/looping-over-a-map.md)). `<br>` starts a new
line and `<b>` makes the title bold ([Rich Text](../strings/rich-text.md)).
A map of records cannot be looped over directly, so the name and the count
are kept in two maps.
