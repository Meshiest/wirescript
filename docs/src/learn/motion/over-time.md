# Over Time

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Easing](easing.md), [Seat Controls](../input/seat-controls.md)

</div>
<!-- /learn-links -->

The last lesson drove the progress by hand, with a counter on a Clock. Two
gates keep the time themselves. A **Tween** glides to a new target over a
set time along an easing curve. A **Dampen** follows a target that keeps
moving, and smooths away its jumps. This lesson builds a door with one and a
steady seat control with the other.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Smoothing a jumpy input](#smoothing-a-jumpy-input)
- [Which one to use](#which-one-to-use)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
switch Enabled -> chip wanted
chip position -> mover "Target Position"
chip sign -> decal Text
```

## Code

```wirescript name=mo3-door
const OPEN_AT = 100.0 // slider position with the door open, in percent
const SECONDS = 1.5 // how long one move takes

@left in wanted: bool
var status: string = "Shut"
let door = Tween(if wanted then OPEN_AT else 0.0, SECONDS, function = Cubic, direction = InOut)
@right out position: float = door
@right out sign = status.Value

on wanted {
  status = "Opening"
}

on !wanted {
  status = "Closing"
}

on door.Arrived {
  status = if wanted then "Open" else "Shut"
}
```

`Tween(target, duration)` is a value that moves to `target` over `duration`
seconds. Change the target and it sets off again. `function` and `direction`
are the curves of [Easing](easing.md): `Cubic` `InOut` starts gently and
stops gently, the way a heavy door moves.

A Tween gives two things. `door` on its own reads its **Value**, the moving
number. `door.Arrived` is an exec that fires when it reaches the target, so
`on door.Arrived` runs once per finished move. `on !wanted` runs when the
switch turns off.

Flip the switch on. What does the sign say during the move, and after it?
What should happen if you flip it back halfway?

## Run

1. Compile the chip and paste it in-game. Build a door: a **Rigid Slider**
   with the door welded to its moving end, so it slides sideways out of the
   doorway. Place a **Switch** and a brick
   with a **Text Decal**.
2. In the slider's settings, turn on **Positions Are Percentages**. The
   slider has its own **Smooth Time** and **Top Speed**: set Smooth Time low
   and Top Speed high, so the slider follows the Tween's curve instead of
   adding a smoothing of its own.
3. Wire the Switch's **Enabled** to `wanted`, `position` to the slider's
   **Target Position**, and `sign` to the Text Decal's **Text**.
4. Flip the switch on. The sign says "Opening", the door eases out over a
   second and a half, and the sign says "Open". Flip it off: "Closing", then
   "Shut".

## Under the Hood

Open the chip. A **Select** picks 100 or 0 from the switch, and feeds the
**Tween** gate's **Target**. The Tween's **Duration** has 1.5 typed in, its
**Function** 3 (`Cubic`) and its **Direction** 2 (`InOut`). Its **Value**
output goes to the pin, and its **Arrived** output is the exec that starts
the last handler: a second **Select** chooses the text, and a **Set
Variable** stores it. The `on !wanted` handler starts from a **NOT** gate.

On a Tween, **Function** and **Direction** are inputs, not settings. A
variable can choose the curve while the circuit runs, as
[Game Enums](../enums/game-enums.md) does. The settings of `Blend` and
`Easing`, and the curve of `Remap`, are fixed when the chip is built and
take only a constant (`WS028`).

## Smoothing a jumpy input

A seat's left and right keys jump: `input.Right` goes straight from 0 to 1
and back. Steering a turret or a wheel from it would snap between angles.
`Dampen` turns those jumps into glides:

```wirescript name=mo3-steer
@left in rider: character
let input = rider.InputReader()
@right out angle: float = Dampen(input.Right * 45.0, 0.3)
```

`input.Right * 45.0` is the angle the rider asks for: -45, 0 or 45
([Seat Controls](../input/seat-controls.md)). `Dampen(target, smoothTime)`
is a value that follows it. When the target jumps, the value picks up speed,
then slows as it closes in. The second number is the **Smooth Time**: larger
values catch up more slowly.

1. Place a **Seat** and a **Bearing (Servo)** with something attached to
   turn, like a turret barrel. Paste the chip.
2. Wire the Seat's **Sitting Character** to `rider` and `angle` to the
   Servo's **Target Angle**, and turn on the Servo's **Enabled**.
3. Sit and tap left and right. The turret swings to each side and back to
   the middle without snapping.

`Dampen` follows any of the kinds `Blend` takes, and gives back the same
kind. A point that jumps around follows smoothly as a point:

```wirescript name=mo3-dampen-vector
@left in spot: vector
@right out smooth = Dampen(spot, 0.3)
```

## Which one to use

- **Tween** for a target that changes now and then, like open and shut, or
  a floor number. Every move takes the same time, follows a curve you pick,
  and ends with an **Arrived** pulse.
- **Dampen** for a target that changes all the time, like keys, a sensor or
  a score. There is no set time and no end pulse: it keeps following.
- **Easing** when the circuit already has its own progress from 0 to 1, as in
  the last lesson.

## Modify

1. Make a door that drops and bounces: `function = Bounce, direction = Out`
   on a slider that moves the door down. Use the slider's **Reversed** setting if it
   moves the wrong way.
2. Light the doorway with the door. Add
   `@right out lamp: color = Tween(if wanted then Color(0.0, 1.0, 0.0) else Color(1.0, 0.0, 0.0), SECONDS)`
   and wire it to a **Point Light**'s **Color** (with **Use Brick Color**
   off). It fades between red and green in step with the door.
3. Steer a second servo with the forward keys: `Dampen(input.Forward * 30.0, 0.5)`.
   With a Smooth Time of 0.5 it lags behind the first.

## Make

Make an elevator with three floors. Three Buttons choose the floor, a
**Rigid Slider** carries the car, and a Tween moves it between 0, 50 and
100 percent. A sign says "Moving" during the ride and "Floor 2" (or 1, or 3)
when the car arrives. Store the chosen floor in a `var`.

```wiring
button1 Held -> chip floor1
button2 Held -> chip floor2
button3 Held -> chip floor3
chip position -> mover "Target Position"
chip sign -> decal Text
```

## Good to Know

- Flip the switch back during a move, and the Tween sets off for the new
  target from where it is, not from an end. The way back takes the full
  Duration again, even from halfway.
- The door's slider smooths its own moves too. If the door lags or rounds
  off the curve, lower its **Smooth Time** and raise its **Top Speed**.
- `Back` and `Elastic` go past the target and come back. A door eased with
  them swings past its end before it settles.
- A Dampen closes in more and more slowly. To ask whether it has nearly
  arrived, use `NearlyEqual(value, target, 0.5)`, not `==`.

See [Misc](../../builtins.md#misc-pure--exec) and
[Gate config properties](../../builtins.md#gate-config-properties) in the
reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
