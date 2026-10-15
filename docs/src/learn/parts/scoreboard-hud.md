# Scoreboard HUD

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Rich Text](../strings/rich-text.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md), [A HUD for Everyone](../screen/a-hud-for-everyone.md)

</div>
<!-- /learn-links -->

Scores shown on each player's own screen instead of on a sign. Everyone in
the arena sees their own score and the leader at the top of the screen. The
text is redrawn only when a score changes, and taken down when they leave.

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

- A brick with an **Attached Zone** covering the arena.
- Something that scores a point and says who scored: a **Target** (**Just
  Hit** and **Character That Just Hit**) or a **Button** (**Held** and
  **Character**).

```wiring
zone "Zone Ref" -> chip arena
target "Just Hit" -> chip point
target "Character That Just Hit" -> chip scorer
button Held -> chip point
button Character -> chip scorer
```

## Code

```wirescript name=gs3-scoreboard-hud
import { escape } from "sanitize"

const TEXT_ID = 7100 // a number no other circuit on the server uses
const FONT_SIZE = 28 // size of the text

@left in arena: zone
@left in point: exec
@left in scorer: character

var scores: Map<string, int>
var leader: string = "nobody"
var top: int = 0
var here: character[]
var shown: character[]

mod scoreOf(id: string) -> int {
  let s = scores.get(id)
  return if s.Found then s.Value else 0
}

mod show(p: character, mine: int) {
  p.DisplayText("Your score: ${mine}<br>Leader: ${leader} (${top})",
    textId = TEXT_ID, anchorX = 0.5, anchorY = 0.0, positionY = 80.0,
    fontSize = FONT_SIZE, justify = Center, lifetime = 0.0)
}

on Clock(interval = 1.0) {
  here.fillFromZonePlayers(arena)
  for p in shown {
    if !here.find(p).Found {
      p.DisplayText("", textId = TEXT_ID)
    }
  }
  for p in here {
    if !shown.find(p).Found {
      show(p, scoreOf(p.GetUserId()))
    }
  }
  shown.copyFrom(here)
}

on point {
  let id = scorer.GetUserId()
  let mine = scoreOf(id) + 1
  scores.set(id, mine)
  if mine > top {
    leader = escape(scorer.GetDisplayName())
    top = mine
  } else {
    here.fillFromZonePlayers(arena)
    if here.find(scorer).Found {
      show(scorer, mine)
    }
  }
}

on Change(top) {
  here.fillFromZonePlayers(arena)
  for p in here {
    show(p, scoreOf(p.GetUserId()))
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Attached Zone's **Zone Ref** to `arena`.
3. For a Target: wire **Just Hit** to `point` and **Character That Just Hit**
   to `scorer`. For a Button, wire **Held** to `point` and **Character** to
   `scorer`. **Held** stays on while anyone holds the button, so it misses a
   second player's press while the first still holds it. To catch that too,
   wire **Press Count** (how many players hold the button right now) to an
   `int` pin instead, and score in `on Change(...)` only when it went up, as
   the [Doorbell](doorbell.md) does.

## Check it in game

- [ ] Walk into the arena: "Your score: 0" and "Leader: nobody (0)" appear
  at the top of your screen within a second.
- [ ] Score a point: your line reads 1, and the leader line shows your name.
- [ ] A second player walks in: within a second they see their own score
  and the same leader.
- [ ] The second player scores twice: both screens show them leading with
  2, and your own line still reads 1.
- [ ] Stand still with nothing scoring: the text stays up.
- [ ] Walk out of the arena: the text disappears within a second.
- [ ] Walk back in: your score is still there.

## Change it

- Add a reset button: a `reset: exec` pin whose handler clears `scores`,
  sets `leader` back to `"nobody"` and then `top` to 0. Changing `top`
  redraws everyone inside.
- Move the text to the top-right corner: `anchorX = 1.0`, `positionX =
  -40.0` and `justify = Right`.
- Show a rank: count the entries in `scores` above the player's score with
  `for id, n in scores` ([Looping Over a
  Map](../loops/looping-over-a-map.md)), and add `Rank: ${rank}` as a third
  line of the same text. Anyone's point can change someone else's rank, so
  redraw everyone inside after each point.

## How it works

`DisplayText` draws text on one player's screen, and a character wires
straight into the gate's player input. Zone events can be missed over a
long game, so the chip asks the zone who is inside with
`fillFromZonePlayers` each time it needs to know. Once a second the Clock
compares that [list](../values/lists.md) with `shown`, the
players it drew for last time: a newcomer gets a text, a player who walked
out gets an empty one, and then `shown` becomes a copy of the fill. A
point redraws only the scorer's text, unless it changes `top`: then the
leader line is different for everyone, and `on Change(top)` redraws every
player inside with a [loop](../loops/looping-over-a-list.md). A short
`lifetime` could also take a leaver's text down, but then the Clock would
have to redraw every text before it ran out. The leader's name is stored before
`top`, because the redraw reads it. Every draw uses the same `textId`, so
each redraw replaces the text already on that player's screen instead of
adding another, and `lifetime = 0.0` keeps it up until it is replaced. The
Clock never redraws a text that has not changed. [A HUD for
Everyone](../screen/a-hud-for-everyone.md) builds the same design step by
step.

The game draws only about 32 of these texts on one player's screen at
once, and the budget is shared by every circuit that player sees. Past it,
a text silently never appears. This part spends one: both lines are in one
text, split by `<br>` ([Rich Text](../strings/rich-text.md)). Scores are
keyed by `GetUserId()`, so a respawn keeps them, and the leader's name is
escaped when it is stored ([Safe Text](../strings/safe-text.md)). A player
who dies inside drops out of the fill, and the next Clock pulse sends the
empty text to their old character, which becomes `null` a little after
the death. Their new character is a newcomer to the Clock when they walk
back in. See
[DisplayText](../../builtins.md#displaytext) and [DisplayText Screen
Placement](../../game-knowledge.md#displaytext-screen-placement) for every
setting.

Back to [Parts](README.md).
