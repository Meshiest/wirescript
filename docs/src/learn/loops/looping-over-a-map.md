# Looping Over a Map

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Looping Over a List](looping-over-a-list.md), [Maps](../values/maps.md)

**Next steps:** [Index and Value](index-and-value.md), [One Chip per Player](../prefabs/one-chip-per-player.md), [Every Player](../admin/every-player.md), [A HUD for Everyone](../screen/a-hud-for-everyone.md)

**Parts that use this:** [Shooting Gallery](../parts/shooting-gallery.md), [Scoreboard HUD](../parts/scoreboard-hud.md), [King of the Hill](../parts/king-of-the-hill.md), [Voting](../parts/voting.md), [Obstacle Course](../parts/obstacle-course.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

A `for` loop over a map visits every key, with the value filed under it. This
lesson counts laps for every player who crosses a finish line, and lists them
all on a board.

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
zone "Zone Ref" -> chip finish
chip board -> decal Text
```

## Code

```wirescript name=lp3-laps
import { escape } from "sanitize"

@left in finish: zone
var laps: Map<string, int>
var names: Map<string, string>
var shown: string = "No laps yet"
@right out board = shown.Value

on ZoneEntered(zone = finish) -> (character) {
  let id = character.GetUserId()
  let before = laps.get(id)
  let count = if before.Found then before.Value + 1 else 1
  laps.set(id, count)
  names.set(id, escape(character.GetDisplayName()))
  var text: string = ""
  for key, n in laps {
    let name = names.get(key)
    text = "${text}${name}: ${n}<br>"
  }
  shown = text
}
```

- The first half of the handler is the visit counter from
  [Maps](../values/maps.md), with one count per
  player filed under their user id. A second map, `names`, files each player's
  display name under the same id, escaped so it is safe to show.
- `for key, n in laps { ... }` runs once per key in `laps`. `key` is the
  user id and `n` the lap count filed under it.
- Inside, `names.get(key)` looks up the name for that id. Every id in `laps`
  was also given a name, so the lookup always finds one.
- `<br>` starts a new line on the sign, so each player gets a row.

Two players cross the line: you twice, a friend once. How many rows does the
board have, and how many times does the loop run?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   across your finish line, and place a brick with a **Text Decal**.
2. Wire the Attached Zone's **Zone Ref** to `finish`, and `board` to the Text
   Decal's **Text**.
3. Walk through the zone a few times. Your row counts up. A second player
   gets a row of their own.

## Under the Hood

Open the chip. The loop is a **For Each Map** microchip, with its **Map Var
Ref** wired from the `laps` map. Inside, the **For Each Map Entry** gate has
**Key** and **Value** outputs for `key` and `n`, and an **Index** output too.

The `names.get(key)` inside the loop is a **Get Map Element** gate. The
`names` map lives outside the loop, and a wire carries its reference in
through the loop chip's side, like any other wire into a chip.

Some languages write a map loop as `for { key, value } in ...`. Here the
braces mean something else, and a map loop with them does not compile:

```wirescript name=lp3-pattern expect=WS076
@left in finish: zone
var laps: Map<string, int>
var total: int = 0

on ZoneEntered(zone = finish) -> (character) {
  laps.set(character.GetUserId(), 1)
  for { key, value } in laps {
    total += value
  }
}
```

```text
Error: [WS076] a `{ ... }` binding destructures the value in its slot, and this slot is `string`, not a record. Name the slot instead (`for v in ...`); a record-typed array or map cannot be iterated at all.
```

Name the slots instead: `for key, value in laps`.

## Modify

1. Add up everyone's laps. Keep a `var total: int = 0` before the loop, add
   `n` to it inside, and show `Total: ${total}` under the rows.
2. Find the leader. Keep `var best: int = 0` and `var leader: string = ""`
   before the loop. Inside, when `n > best`, set both: `best = n` and
   `leader = key`. After the loop, show `Leader: ${names.get(leader)}`.
3. Number the rows. A map loop has a third slot, the index:
   `for key, n, i in laps` counts 0, 1, 2 as it goes. Start each row with
   `${i + 1}.`.

## Make

Make a checkpoint race. A second zone is a checkpoint, and a lap only counts
if the player touched the checkpoint since their last lap. Keep a
`Map<string, bool>` of who has touched it, set it in the checkpoint's
handler, and clear it for that player when their lap counts.

```wiring
zone1 "Zone Ref" -> chip finish
zone2 "Zone Ref" -> chip checkpoint
chip board -> decal Text
```

## Good to Know

- Do not count on the order of the rows. A map loop is not sorted by laps or
  by name. Sort the results yourself if the order matters.
- A respawn makes a new character for the same player, so a map keyed by the
  character would start a new row after every death. Key by
  `GetUserId()`, which stays the same.
- A display name is chosen by the player. Escape it before it goes on a sign.

See [Bindings](../../statements.md#bindings) in the reference for every
binding a loop takes.

Next: [Index and Value](index-and-value.md).
