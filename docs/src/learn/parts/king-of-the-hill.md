# King of the Hill

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Ready Check](../input/ready-check.md), [Exec Pins](../events/exec-pins.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md)

</div>
<!-- /learn-links -->

A hill that players fight over. Whoever stands on it alone collects a
second of hold time each second, and the first to reach a minute is king. A
sign shows who holds the hill and the longest hold so far.

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

- A brick with an **Attached Zone** covering the hilltop.
- A **Button** to start a new game.
- A brick with a **Text Decal** for the sign.
- Something to celebrate the king: a **One Shot Audio**, a light, a door.

```wiring
zone "Zone Ref" -> chip hill
button Held -> chip reset
chip sign -> decal Text
chip crowned -> speaker Play
```

## Code

```wirescript name=gs5-king-of-the-hill
import { escape } from "sanitize"

const WIN_SECONDS = 60 // seconds on the hill to win

@left in hill: zone
@left in reset: exec

var players: character[]
var held: Map<string, int>
var running: bool = true
var status: string = "The hill is empty"
var bestName: string = "nobody"
var bestTime: int = 0
@right out sign = "${status.Value}<br>Longest: ${bestName.Value} ${bestTime.Value}s"
@right out crowned: exec

on reset {
  held.clear()
  bestName = "nobody"
  bestTime = 0
  status = "The hill is empty"
  running = true
}

on Clock(interval = 1.0, enabled = running) {
  players.fillFromZonePlayers(hill)
  let count = players.length()
  if count == 0 {
    status = "The hill is empty"
    return
  }
  if count > 1 {
    status = "Contested!"
    return
  }
  let king = players[0]
  let name = escape(king.GetDisplayName())
  let id = king.GetUserId()
  let had = held.get(id)
  let time = (if had.Found then had.Value else 0) + 1
  held.set(id, time)
  if time > bestTime {
    bestTime = time
    bestName = name
  }
  if time >= WIN_SECONDS {
    status = "${name} is king!"
    running = false
    emit crowned
  } else {
    status = "${name} holds the hill"
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Attached Zone's **Zone Ref** to `hill` and the Button's
   **Held** to `reset`.
3. Wire `sign` to the Text Decal's **Text**.
4. Wire `crowned` to the One Shot Audio's **Play**, or any other exec input.

## Check it in game

- [ ] The sign reads "The hill is empty" and "Longest: nobody 0s".
- [ ] Stand on the hill: within a second the sign reads "(your name) holds
  the hill", and the longest hold goes up by one each second.
- [ ] A second player steps on: "Contested!", and nobody's time goes up.
- [ ] They step off: your time carries on from where it stopped.
- [ ] Reach 60 seconds: "(your name) is king!", `crowned` fires once, and
  the times stop.
- [ ] Press the button: everything starts over.

## Change it

- A shorter game: `WIN_SECONDS = 30`.
- Let a crowd hold the hill: score whenever anyone is on it (`count > 0`),
  giving each player in `players` a second with `for p in players` around
  the scoring.
- Play it in teams: keep a `Map<string, int>` of each player's team, as in
  [Team Match](team-game.md). Each pulse, count each team's players in
  `players` with `for p in players`, and give the hill to a team only
  while no other team has anyone on it.
- Show everyone's hold time: build a line per entry with
  `for id, t in held` ([Looping Over a Map](../loops/looping-over-a-map.md)),
  keeping each player's name in a second map.

## How it works

Once a second, while the game runs, the Clock fills an
[array](../values/lists.md) with the characters standing in the
zone, as in [Ready Check](../input/ready-check.md), and the length decides
the case: nobody, contested, or one player. The single player is
`players[0]`, read only after the length check, because an empty array
gives back an old value instead of nothing. That player gets a second of
hold time. Hold times live in a map keyed by `GetUserId()`, so dying on the
hill keeps the time collected so far, and the `.Found` guard starts a
newcomer at 0. `running` turns the Clock off and freezes the sign when
someone wins, and the `crowned` pulse leaves the chip from that one handler
([Exec Pins](../events/exec-pins.md)).

The hill asks the zone who is on it at every pulse instead of following
`ZoneEntered` and `ZoneLeft`. Over a long game a few of those events can be
missed, and a count kept from them drifts. The fill is always right. A
Clock's first pulse comes one second after it is turned on, so a new game
starts scoring a second after the button.

Back to [Parts](README.md).
