# A HUD for Everyone

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Keeping It Current](keeping-it-current.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Index and Value](../loops/index-and-value.md)

**Parts that use this:** [Scoreboard HUD](../parts/scoreboard-hud.md)

</div>
<!-- /learn-links -->

The last lesson kept one player's panel current. This one gives every
player in an arena a text of their own: their score and the leader, drawn
when they walk in, redrawn when a score changes, and taken down when they
walk out. A Clock checks who is in the arena once a second, but it never
redraws a text that has not changed. You need a second player to see most
of it.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip arena
button "Press Count" -> chip holding
button Character -> chip presser
```

## Code

```wirescript name=sc4-arena-hud
import { escape } from "sanitize"

const HUD = 7320 // a text id no other circuit uses

@left in arena: zone
@left in holding: int
@left in presser: character

var scores: Map<string, int>
var top: int = 0
var leader: string = "nobody"
var last: int = 0
var here: character[]
var shown: character[]

mod show(p: character, mine: int) {
  p.DisplayText("You: ${mine}<br>Leader: ${leader} (${top})",
    textId = HUD, lifetime = 0.0, anchorX = 0.5, anchorY = 0.0,
    positionY = 80.0, justify = Center, fontSize = 28)
}

mod scoreOf(id: string) -> int {
  let s = scores.get(id)
  return if s.Found then s.Value else 0
}

on Clock(interval = 1.0) {
  here.fillFromZonePlayers(arena)
  for p in shown {
    if !here.find(p).Found {
      p.DisplayText("", textId = HUD)
    }
  }
  for p in here {
    if !shown.find(p).Found {
      show(p, scoreOf(p.GetUserId()))
    }
  }
  shown.copyFrom(here)
}

on Change(holding) {
  let rose = holding > last
  last = holding
  here.fillFromZonePlayers(arena)
  if !rose || !here.find(presser).Found {
    return
  }
  let id = presser.GetUserId()
  let mine = scoreOf(id) + 1
  scores.set(id, mine)
  if mine > top {
    leader = escape(presser.GetDisplayName())
    top = mine
  } else {
    show(presser, mine)
  }
}

on Change(top) {
  here.fillFromZonePlayers(arena)
  for p in here {
    show(p, scoreOf(p.GetUserId()))
  }
}
```

- `here.fillFromZonePlayers(arena)` fills `here` with everyone standing in
  the arena. Zone events can be missed over a long game, so the chip asks
  the zone itself each time it needs to know who is inside.
- `shown` is who the Clock drew for last time. Someone in `here` but not
  in `shown` just walked in and gets a text. Someone in `shown` but not in
  `here` walked out and gets an empty one. Then `shown` becomes a copy of
  `here`, so it is never more than a second out of date.
- `scores` keeps each player's points by user id
  ([Per-Player State](../references/per-player-state.md)), so they survive
  a respawn and a walk out and back in.
- `show` draws one player's text: their own score on the first line, the
  leader on the second. Every draw uses the same id, `HUD`, so each player
  has one text that each draw replaces.
- A press on the **Button** scores a point for whoever pressed, if the
  fill finds them inside. Only that player's text changes, unless they
  took the lead.
  The Button's **Press Count** is how many players hold it right now, so it
  also changes when someone lets go. `rose` is true only when it went up
  since the last change (`last`), and a release returns early. `last` is
  updated before the early `return`, so it never goes stale.
- When `top` changes, the leader line is different for everyone, so
  `on Change(top)` fills `here` again and redraws every text in the arena
  ([Looping Over a List](../loops/looping-over-a-list.md)).

Five players are in the arena. One of them, not the leader, scores. How
many texts are redrawn? Then the leader scores again. How many now?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached
   Zone** over the arena, and wire its **Zone Ref** to `arena`.
2. Place a **Button** inside. Wire its **Press Count** to `holding` and its
   **Character** to `presser`.
3. Walk in. Within a second the top of your screen shows `You: 0` and
   `Leader: nobody (0)`.
4. Press the button. Both lines change: `You: 1`, and you lead with 1.
5. A second player walks in and within a second sees `You: 0`, with you
   as the leader. They press twice: on the second press both screens show
   them leading with 2. Your own line still reads `You: 1`.
6. Walk out. Your text goes within a second. Walk back in: `You: 1` again.

## Under the Hood

Open the chip. It has 74 gates. Each call of `show` is its own
**Display Text** gate and its own **Format String**, and there are three
calls: one for a newcomer in the Clock's second loop, one for the scorer,
and one in the `Change(top)` loop. Each `for` becomes a **For Each Array**
chip, three in all, and each `fillFromZonePlayers` is a **Get Players In
Zone** gate, one in each handler. The fourth **Display Text**, in the
Clock's first loop, draws the empty text over a leaver's. All four use **Text Id**
7320, and since each player's screen has ids of its own, one id gives each
player one text.

Count the redraws. A point that does not change the lead redraws one
text. A new top score redraws one per player inside. Joining and leaving
draw one each, on the next Clock pulse. The Clock draws nothing for a
player who was already inside. Text is built only when its words change,
and only for the players whose words changed. Redrawing every text on the
Clock instead would rebuild each player's text every second, changed or
not.

A text could also take itself down with a short `lifetime`, but then the
Clock would have to redraw every text before it ran out. Keeping `shown`
lets the Clock draw only for players who came or went.

The leader's name is stored before `top`. The redraw is started by `top`
changing, and it reads `leader`, so the name has to be there first.

### When the work gets heavy

Here each player's text is cheap: one map read and one text. If each
player needed more, such as a readout of their own speed or distance that
changes every moment, this one chip would have to do it for everyone, over
and over. That is when to give each player a chip of their own, as in
[One Chip per Player](../prefabs/one-chip-per-player.md). The central chip
keeps the scores and sends each copy what it needs, and the copy draws its
own player's text and removes itself when its player leaves. [Multiplayer
Architecture](../mastery/multiplayer.md) builds that split for a
scoreboard. A copy does not need a clock to stay current either: it can
redraw in the handler of the event that brought the new value.

## Modify

1. Add a reset. Wire a second Button's **Held** to `@left in reset: exec`,
   and clear everything:

   ```wirescript nocompile
   on reset {
     scores.clear()
     leader = "nobody"
     top = 0
   }
   ```

   Setting `top` to 0 starts the redraw, and every player reads `You: 0`.
   If `top` was already 0, nothing needs redrawing.

2. Add a third line, `Players: ${count}`, to the text in `show`. The
   Clock already fills `here` once a second, so take the count there,
   right after the fill, and redraw everyone only when the number changes:

   ```wirescript nocompile
   var count: int = 0

   // in the Clock, after the fill:
   count = here.length()

   on Change(count) {
     for p in here {
       show(p, scoreOf(p.GetUserId()))
     }
   }
   ```

3. Show your rank: `Rank: 2 of 5`. Count the scores higher than yours with
   `for id, n in scores`. Your rank can change when someone else scores,
   so work out whose text needs a redraw before you write it.

## Make

Rebuild this HUD with one chip per player. The central chip keeps
`scores`, `top` and `leader`. When its Clock fill finds a new player, it
spawns a copy for them (or reuses theirs) and sends it the character and
current text. A point sends the new score to the scorer's copy alone, and
a new top sends the leader line to every copy with one send. Each copy
draws its own text in the handlers of those events, with no `Clock` for
drawing. When the fill no longer finds a player, the central chip tells
their copy, which takes its text down and removes itself, as it also does
when its player leaves the server. Check it with two players: both
screens agree on the leader, each shows its own score, and walking out
takes your text down.

```wiring
zone "Zone Ref" -> chip arena
button "Press Count" -> chip holding
button Character -> chip presser
```

## Good to Know

- A player who dies inside drops out of the fill, and the next Clock pulse
  sends the empty text to their old character. A dead player's character
  becomes `null` a little after the death, and so does the character of a
  player who leaves the server.
- A respawn makes a new character. When it walks back in, the Clock finds
  it missing from `shown` and draws its text, with the same score, since
  scores are kept by user id.
- The loop fires one **Display Text** gate once per player, all in one
  tick, as the [Scoreboard HUD](../parts/scoreboard-hud.md) part does.
  Do not do the same with a custom event send: one send gate fired many
  times in a tick is capped
  ([Multiplayer Architecture](../mastery/multiplayer.md)).
- A player's screen shows about 32 texts, shared by every circuit. This
  chip uses one per player, since both lines are one text.
- Escape a name when you store it. `leader` is shown to every player.

See [DisplayText](../../builtins.md#displaytext) and [per-entity
fan-out](../../best-practices.md#10-per-entity-work-a-for-sweep-or-one-chip-each)
in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
