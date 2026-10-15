# Doorbell

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Exec Pins](../events/exec-pins.md)

</div>
<!-- /learn-links -->

A button by a front door that plays a doorbell sound where the visitor
stands, and sends a pulse you can use to ring a second bell inside.

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

- a **Button**
- optional: a **One Shot Audio** brick inside the house

```wiring
button "Press Count" -> chip holding
button Character -> chip visitor
chip rang -> speaker Play
```

## Code

```wirescript name=bp5-doorbell
const SOUND = $BrickOneShotAudioDescriptor/OBA_DoorBell_1 // the sound it plays
const PITCH = 1.0 // 1.0 is the sound as recorded; higher is higher

@left in holding: int
@left in visitor: character
@right out rang: exec
var last: int = 0

on Change(holding) {
  if holding > last {
    emit rang
    visitor.PlayAudioAt(SOUND, pitch = PITCH)
  }
  last = holding
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the **Button** beside the
   door.
2. Wire the Button's **Press Count** to `holding`, and its **Character**
   to `visitor`. **Press Count** is how many players are holding the button
   right now. **Character** is the player who pressed it last.
3. Optional: place the One Shot Audio brick inside, open its settings, pick a
   sound, and wire `rang` to its **Play**.

## Check it in game

- Press the button: the doorbell sound plays once, around you.
- Hold the button down: it plays only once until you let go and press again.
- Let go: nothing plays.
- Ask a friend to stand a little way off: they hear it too, quieter.
- While your friend holds the button down, press it yourself: it rings
  again, around you.
- With the One Shot Audio wired, its sound plays inside at the same moment.

## Change it

- Pick another sound: type `$BrickOneShotAudioDescriptor/` in the
  playground, and the editor lists the sounds. `OBA_DoorBell_2`,
  `OBA_DoorBell_SciFi` and `OBA_Door_Knock_Soft` suit a door.
- Make it a ding-dong: after the `PlayAudioAt` line, add
  `await Sleep(_, delay = 0.6)` and a second
  `visitor.PlayAudioAt(SOUND, pitch = 0.8)`.
- Count the visitors: add `var rings: int = 0`, `rings += 1` in the handler,
  and `@right out sign = "Rings: ${rings.Value}"` for a Text Decal.

## How it works

**Press Count** goes up on each press and down on each release. `on Change(holding)` runs
on both ([Change](../../builtins.md#edge--change-detectors)), and
`holding > last` keeps only the rises, so the bell rings once per press by
anyone and never on a release. `last` remembers the count from the run
before. **Held** would miss some presses: it stays on while anyone holds the
button, so a second visitor's press does not turn it on again, though
**Press Count** and **Character** both change.
`$BrickOneShotAudioDescriptor/...` names one of the game's sounds. `visitor.PlayAudioAt(SOUND)` plays it at the visitor's position, so
players nearby hear it and players far away do not.
`emit rang` sends a pulse out of the `rang` pin
([Exec Pins](../events/exec-pins.md)), wired to anything with an exec input.
