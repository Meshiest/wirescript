# Checkpoints

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Maps](../values/maps.md), [Mods](../abstraction/mods.md), [Per-Player State](../references/per-player-state.md)

</div>
<!-- /learn-links -->

Checkpoints along a course. Each player's furthest checkpoint is remembered,
and a player who falls off, or comes back from the spawn point, is sent
back to it.

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

- Three bricks with an **Attached Zone** each: the checkpoints, in course
  order.
- One **Attached Zone** under the course, where a falling player lands.
- One **Attached Zone** across the way out of the spawn area, where every
  player walks after spawning.

```wiring
zone1 "Zone Ref" -> chip cp1
zone2 "Zone Ref" -> chip cp2
zone3 "Zone Ref" -> chip cp3
zone4 "Zone Ref" -> chip fall
zone5 "Zone Ref" -> chip spawn
```

## Code

```wirescript name=gs1-checkpoints
const COUNT = 3 // checkpoints on the course, shown in the message

@left in cp1: zone
@left in cp2: zone
@left in cp3: zone
@left in fall: zone
@left in spawn: zone

var reached: Map<string, int>
var spot: Map<string, vector>

mod reach(who: character, n: int) {
  let id = who.GetUserId()
  let had = reached.get(id)
  if had.Found && had.Value >= n {
    return
  }
  reached.set(id, n)
  spot.set(id, who.GetLocation())
  who.ShowStatusMessage("Checkpoint ${n} of ${COUNT}")
}

mod sendBack(who: character) {
  let saved = spot.get(who.GetUserId())
  if saved.Found {
    who.SetLocation(saved.Value)
  }
}

on ZoneEntered(zone = cp1) -> (character) { reach(character, 1) }
on ZoneEntered(zone = cp2) -> (character) { reach(character, 2) }
on ZoneEntered(zone = cp3) -> (character) { reach(character, 3) }
on ZoneEntered(zone = fall) -> (character) { sendBack(character) }
on ZoneEntered(zone = spawn) -> (character) { sendBack(character) }
```

## Wire it

1. Compile the chip and paste it in-game near the course.
2. Wire each checkpoint's Attached Zone **Zone Ref** to `cp1`, `cp2` and
   `cp3`, in the order players reach them.
3. Wire the zone under the course to `fall`, and the zone at the way out of
   spawn to `spawn`.
4. Make the checkpoint zones tall and wide enough that nobody can jump past
   one, and the fall zone wide enough to catch everyone.

## Check it in game

- [ ] Walk into the first checkpoint: a status message says
  "Checkpoint 1 of 3".
- [ ] Walk back through it: no message, since you have reached it already.
- [ ] Reach the second checkpoint, then jump off the course: you land in the
  fall zone and appear back at the second checkpoint.
- [ ] Walk back into the first checkpoint and fall again: you still go back
  to the second one. Progress only moves forward.
- [ ] Die (or reset), and walk out of spawn: you are moved to your
  checkpoint.
- [ ] A friend who has not reached any checkpoint falls: nothing moves them.

## Change it

- Add a fourth checkpoint: a `cp4` pin, a handler that calls
  `reach(character, 4)`, and `COUNT = 4`.
- Make players take the checkpoints in order: in `reach`, read
  `let last = if had.Found then had.Value else 0` and return unless
  `last == n - 1`.
- Add a restart button at the start: wire a **Button**'s **Press Count**
  and **Character** to two new pins (an `int` and a `character`). **Press
  Count** is how many players hold the button right now, so in an
  `on Change(...)` handler on it, keep the last count in a `var` and act only
  when the count went up: `if holding > last { ... }` then
  `last = holding`. Inside the `if`, remove that player's entries from
  `reached` and `spot`. That catches every press, even one made while
  another player still holds the button, and ignores releases.
- The game also has its own **Checkpoint** brick, which sets a player's
  spawn point without any wires. Try it next to this part.

## How it works

Every zone has its own `on ZoneEntered` handler ([Reacting to the
Game](../reacting-to-the-game.md)), and the checkpoint handlers share one
[mod](../abstraction/mods.md), `reach`, which takes the player and the
checkpoint's number. Progress lives in two [maps](../values/maps.md)
keyed by `GetUserId()`, so it survives a respawn, when the player gets a new
character but keeps their user id. `spot` stores where the player stood when
they reached the checkpoint, a [vector](../values/vectors-and-colors.md), and
`SetLocation` puts them back there. `.Found` guards every read, since a
player who has reached nothing has no entry.

This part does not choose where a player respawns. The `spawn` zone works
around that: a player who respawns walks out of the spawn
area, enters the zone, and is moved on to their checkpoint.

Back to [Parts](README.md).
