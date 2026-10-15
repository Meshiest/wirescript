# Pressure Pad Door

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Waiting](../events/waiting.md)

</div>
<!-- /learn-links -->

Step on a pad and the door in front of you opens. It stays open while anyone
stands on the pad, and for a few seconds after the last player steps off.

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

- a brick with a **Touch** component, as the pad
- a door: a few bricks, one of them with a **Brick Property Changer**

```wiring
touch Touching -> chip pad
touch Character -> chip stepper
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Code

```wirescript name=pp1-pad-door
// Seconds the door stays open after the last player steps off.
const STAY_OPEN = 3.0
// What a player sees when they step on the pad.
const MESSAGE = "The door is open while you stand here"

@left in pad: bool
@left in stepper: character
@right out solid = !Sleep(pad, delay = 0.0, hold = STAY_OPEN)

on pad {
  stepper.ShowStatusMessage(MESSAGE)
}
```

## Wire it

1. Compile the chip and paste it in-game next to the door. Place the pad
   brick with the **Touch** where players walk up to the door.
2. Wire the Touch's **Touching** to `pad` and its **Character** to `stepper`.
3. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility** inputs.

## Check it in game

- Before anyone steps on the pad: the door is there and you cannot walk
  through it.
- Step on the pad: the door vanishes at once, and a status message tells you
  the door is open.
- Stand on the pad for ten seconds: the door stays open the whole time.
- Step off: about three seconds later the door is back.
- Step off and back on within three seconds: the door never closes.

## Change it

- Change `STAY_OPEN` for a longer or shorter wait. `0.0` closes the door the
  moment the pad is free.
- Light the pad while someone stands on it: add `@right out lit = pad` and
  wire it to a **Point Light**'s **Enabled**.
- Put two pads on either side of the door and open it from both:
  add `@left in pad2: bool` and use `Sleep(pad || pad2, ...)`.

## How it works

`Sleep(pad, delay = 0.0, hold = STAY_OPEN)` is one **Buffer (Seconds)**
gate. Its output follows `pad` after `delay` seconds and, once `pad` turns
off, holds on for another `hold` seconds. Outside an `await` it is a steady
signal, not a step, so the door needs no handler at all
([Pure and Exec](../pure-and-exec.md)). The handler only sends the message:
`on pad` fires once when the pad turns on, and `ShowStatusMessage` shows a
line only that player sees ([Reacting to the Game](../reacting-to-the-game.md)).
[Waiting](../events/waiting.md) has the same gate inside an `await`, and the
[Proximity Door](proximity-door.md) opens the same way from a zone instead
of a pad.
