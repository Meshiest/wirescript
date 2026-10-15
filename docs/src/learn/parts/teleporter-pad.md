# Teleporter Pad

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Maps](../values/maps.md)

</div>
<!-- /learn-links -->

Walk onto a pad and you are sent to a destination. Each player then has to
wait a few seconds before the pad takes them again, so nobody bounces back
and forth, and one player's trip never blocks anyone else.

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

- a brick with an **Attached Zone** over the pad
- an **Entity Teleport Point** brick where players should arrive (it carries
  the **Teleport Destination** component)

```wiring
zone "Zone Ref" -> chip pad
teleport "This Destination" -> chip target
```

## Code

```wirescript name=pp3-teleporter
// Seconds a player waits before the pad takes them again.
const COOLDOWN = 5

@left in pad: zone
@left in target: teleport

var readyAt: Map<string, int>

on ZoneEntered(zone = pad) -> (character) {
  let id = character.GetUserId()
  let now = GetUnixTime()
  let next = readyAt.get(id)
  if next.Found && now < next.Value {
    character.ShowStatusMessage("Teleporter recharging: ${next.Value - now}s")
  } else {
    readyAt.set(id, now + COOLDOWN)
    character.Teleport(target)
  }
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the Entity Teleport Point
   outside the pad's zone. While you test, put it a few steps from the pad,
   so you can walk back before the cooldown ends.
2. Wire the Attached Zone's **Zone Ref** to `pad`.
3. Wire the Teleport Destination's **This Destination** to `target`.

## Check it in game

- Walk onto the pad: you arrive at the teleport point.
- Walk back onto the pad before five seconds are up: you stay put, and a
  status message says how many seconds are left.
- Wait until the time is up, step off and on again: you are sent again.
- With a second player: while you are recharging, they still go through at
  once.

## Change it

- Change `COOLDOWN`. It counts whole seconds.
- Make a two-way link: paste a second copy next to the teleport point, and
  wire its `target` to a second teleport point beside the first pad. Keep
  the arrival points outside both zones, or players land in the other pad.
- Charge for the trip: keep a second map of coins per player and only
  teleport a player who has enough.

## How it works

The zone event gives the player who walked in
([Reacting to the Game](../reacting-to-the-game.md)). The cooldown is a map
from the player's user id to the time their pad is ready again, so every
player has their own and a respawn does not reset it. `.Found` covers a
player the pad has never seen ([Maps](../values/maps.md)).
`GetUnixTime()` is the server's clock in whole seconds. It keeps counting
through a server restart, so a stored time stays meaningful. `teleport` is a
pin type for a teleport point: it can only be wired, like `zone`.
