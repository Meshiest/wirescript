# Healing Zone

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Looping Over a List](../loops/looping-over-a-list.md)

</div>
<!-- /learn-links -->

Needs admin to load: it changes players' health with the **Set Player
Damage** gate, which only an admin can place.

Stand in the zone and your wounds heal a little every second. Turned around,
the same chip makes a zone that hurts.

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

- a brick with an **Attached Zone** covering the healing area

```wiring
zone "Zone Ref" -> chip room
```

## Code

```wirescript name=pp5-healing admin
// Damage healed per tick of the zone.
const HEAL = 10.0
// Seconds between ticks.
const EVERY = 1.0

@left in room: zone

var inside: character[]

on Clock(interval = EVERY) {
  inside.fillFromZonePlayers(room)
  for p in inside {
    let hurt = p.GetDamage()
    if hurt > 0.0 {
      p.SetDamage(max(hurt - HEAL, 0.0))
      p.ShowStatusMessage("Healing...")
    }
  }
}
```

## Wire it

1. Compile the chip and paste it in-game, as an admin.
2. Wire the Attached Zone's **Zone Ref** to `room`.

## Check it in game

- Get hurt first (a second copy of this chip, made into a hurting zone as in
  Change it, can do that), then walk into the zone: once
  a second a status message says "Healing...", and your health climbs back.
- Stay in the zone at full health: no message.
- With a second player: both of you heal at once.
- Walk out: the healing stops.

## Change it

- Change `HEAL` and `EVERY` for faster or slower healing.
- Make a hurting zone: use `p.SetDamage(hurt + HEAL)` and drop the
  `if hurt > 0.0` check. A player dies when their damage reaches the limit
  that `p.GetDamage().DamageLimit` reports.
- Heal only players who carry a tag: pass it as
  `inside.fillFromZonePlayers(room, "medic")`, and give players the tag with
  `character.SetTag("medic")`, from a button for example.

## How it works

The Clock fires every `EVERY` seconds
([Reacting to the Game](../reacting-to-the-game.md)), fills a list with the
players standing in the zone, and loops over it
([Looping Over a List](../loops/looping-over-a-list.md)). A player's health
is kept as damage taken: 0 is unhurt, and `GetDamage()` reads it.
`max(a, b)` gives the larger of two numbers, so healing never takes the
damage below 0. The list is filled again on every tick, so a player who
walked out is no longer in it.
