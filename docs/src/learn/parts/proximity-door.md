# Proximity Door

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Waiting](../events/waiting.md)

</div>
<!-- /learn-links -->

A door that opens by itself when a player walks up to it, and closes a
moment after the last player has gone.

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

- a door: a few bricks, one with a **Brick Property Changer**
- a brick with an **Attached Zone**, covering the space in front of the door
  (and behind it, to open from both sides)

```wiring
zone Occupied -> chip near
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Code

```wirescript name=bp8-proximity-door
const STAY_OPEN_SECONDS = 2.0 // how long the door stays open after the last player leaves

@left in near: bool
@right out solid = !Sleep(near, delay = 0.0, hold = STAY_OPEN_SECONDS)
```

## Wire it

1. Compile the chip and paste it in-game. Build the door. Give a brick next
   to it an **Attached Zone** that covers the floor in front of the door.
2. Wire the Attached Zone's **Occupied** to `near`.
3. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility**. One output can feed both.

## Check it in game

- Walk toward the door: it vanishes as you enter the zone.
- Stand in the zone: it stays open.
- Walk away: two seconds after you leave the zone, the door is back.
- Walk out and back in within two seconds: the door stays open throughout.
- With a friend: the door stays open until both of you have left.
- If the door never opens, check the Attached Zone's **Detect Players**
  setting.

## Change it

- Close faster or slower: `const STAY_OPEN_SECONDS = 0.5`.
- Open a moment late: set `delay = 1.0`. The door then opens about a
  second after a player steps into the zone.
- Lock it: add `@left in locked: bool`, wire a **Switch**'s **Enabled** to
  it, and write `@right out solid = locked || !Sleep(...)`. While the switch
  is on, the door stays shut.

## How it works

The chip has no handler and no variable: it is all pure wires
([Pure and Exec](../pure-and-exec.md)). The zone's **Occupied** is on while
anyone is inside it. `Sleep(near, delay = 0.0, hold = ...)` is one
**Buffer (Seconds)** gate, the same gate `await Sleep` uses
([Waiting](../events/waiting.md)). Its output follows `near`, but when
`near` turns off it stays on for `hold` more seconds. If `near` turns on
again in that time, the output never goes off at all. `!` turns "someone is
near" into "the door is solid".
