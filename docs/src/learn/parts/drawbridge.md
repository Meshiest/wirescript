# Drawbridge

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Choosing a Value](../logic/choosing-a-value.md)

</div>
<!-- /learn-links -->

A bridge on a hinge that a button raises and lowers. A warning light shines
while it moves, and presses during the move are ignored.

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

- a **Bearing (Servo)** brick, the hinge (its component is called **Servo**)
- the bridge deck: a piece built on its own and attached to the servo
- a **Button**
- optional: a brick with a **Point Light**, the warning light

```wiring
button Held -> chip pressed
chip angle -> mover "Target Angle"
chip warning -> light Enabled
```

## Code

```wirescript name=bp7-drawbridge
const DOWN_ANGLE = 0.0 // the servo's angle with the bridge down
const UP_ANGLE = 90.0 // the servo's angle with the bridge up
const MOVE_SECONDS = 4.0 // how long the warning light stays on after a press

@left in pressed: exec
var raised: bool = false
var moving: bool = false
@right out angle: float = if raised.Value then UP_ANGLE else DOWN_ANGLE
@right out warning = moving.Value

on pressed {
  if !moving {
    raised = !raised
    moving = true
    await Sleep(_, delay = MOVE_SECONDS)
    moving = false
  }
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the **Bearing (Servo)** at
   the end of the gap, where the bridge should pivot. Build the deck and
   attach it to the servo. The servo's description in the game says how
   (with the placer or the manipulator).
2. Wire the Button's **Held** to `pressed`.
3. Wire `angle` to the Servo's **Target Angle**.
4. Optional: wire `warning` to the Point Light's **Enabled**.

## Check it in game

- Before any press, the bridge stays as you built it: flat across the gap.
- Press the button: the warning light comes on and the bridge swings up.
- Press again while the light is on: nothing happens.
- After the light goes off, press again: the bridge comes back down.
- If the bridge swings down into the gap instead of up, set
  `UP_ANGLE = -90.0`, or turn on the servo's **Reversed** setting.
- If the bridge does not move at all, check that the servo's **Enabled**
  setting is on.

## Change it

- Raise it only partway: `const UP_ANGLE = 45.0`.
- Match the light to the bridge: time how long the bridge takes to move, and
  set `MOVE_SECONDS` to that.
- Run it from a **Switch** instead: wire the Switch's **Enabled** to a new
  `@left in up: bool` pin and write
  `@right out angle: float = if up then UP_ANGLE else DOWN_ANGLE`. The bridge
  then follows the switch, and the handler and both variables can go.
- Make it a gate or a barrier arm: the same chip turns any servo between two
  angles.

## How it works

`raised` remembers which way the bridge should be ([Remembering Values](../state.md)).
`angle` picks one of the two angles from it with
`if ... then ... else` ([Choosing a Value](../logic/choosing-a-value.md)),
and the servo turns the deck until it reaches that angle. The handler flips
`raised`, then holds `moving` on for `MOVE_SECONDS` with
`await Sleep` ([Pure and Exec](../pure-and-exec.md)). While `moving` is on,
the `if !moving` check skips the press, and `warning` lights the lamp. The
chip never measures the bridge: the light's time is a guess you set.
