# Angles

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Dot and Cross](dot-and-cross.md)

**Next steps:** [Rotations](rotations.md)

</div>
<!-- /learn-links -->

A direction vector says which way something points. People think in angles
instead: turn 90 degrees, look up 30. This lesson turns a direction into a
heading and a tilt with `atan2`, builds a `rotator` from them, and goes back
from angles to a direction with `cos` and `sin`. The project is a compass sign
that shows which way a player is looking.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Angles into a rotator](#angles-into-a-rotator)
- [From angles back to a direction](#from-angles-back-to-a-direction)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip readout -> decal Text
```

## Code

```wirescript name=sp3-compass
@left in room: zone
@left in occupied: bool

var visitor: character
var shown: string = ""
@right out readout = shown.Value

on ZoneEntered(zone = room) -> (character) {
  visitor = character
}

on Clock(interval = 0.1, enabled = occupied) {
  let d = visitor.GetAim().Direction
  let yaw = Rad2Deg(atan2(d.y, d.x))
  let flat = sqrt(d.x * d.x + d.y * d.y)
  let pitch = Rad2Deg(atan2(d.z, flat))
  shown = "Heading ${round(yaw) as int}<br>Looking up ${round(pitch) as int}"
}
```

`d` is the way the player looks, a vector of length 1. Two angles describe it:

- **Yaw** is the heading on the floor plan, ignoring up and down: the angle
  from the x axis around toward the y axis. `atan2(d.y, d.x)` measures it.
- **Pitch** is the tilt above or below level. `flat` is the length of the
  direction's shadow on the floor, and `atan2(d.z, flat)` is the angle
  between that shadow and the direction itself.

The trig gates work in radians, where half a turn is pi (about 3.14).
`Rad2Deg` turns radians into degrees. `<br>` starts a new line on the sign.

A player looks along the y axis, level. What does the sign read? And looking
straight down at their feet?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**,
   put the chip inside it, and place a brick with a **Text Decal** where you
   can read it.
2. Wire the Attached Zone's **Zone Ref** to `room` and its **Occupied** to
   `occupied`. Wire `readout` to the Text Decal's **Text**.
3. Walk into the zone and turn on the spot. The heading runs through 0
   (looking along x), 90 (along y), 180 or -180 (back along x) and -90. Look
   up and down: the second line runs from 90 (straight up) to -90 (straight
   down).

## Under the Hood

Open the chip. After **Get Aim**, a **Split Vector** takes the direction
apart into x, y and z. Yaw is one **Atan2** and a **Radians To Degrees**.
Pitch adds two **Multiply** gates, an **Add** and a **Sqrt** for `flat`, then
its own **Atan2** and **Radians To Degrees**. Two **Round** and two
**Convert** gates feed the **Format String**.

### Why atan2

`atan2(y, x)` takes the two parts of a direction separately. The older
one-number `atan` takes their ratio, `atan(y / x)`, and the ratio throws
information away:

| Direction (x, y) | `atan(y / x)` in degrees | `atan2(y, x)` in degrees |
|------------------|--------------------------|--------------------------|
| (3, 4)           | 53.1                     | 53.1                     |
| (-1, 2)          | -63.4                    | 116.6                    |
| (1, -2)          | -63.4                    | -63.4                    |
| (-1, 0)          | 0                        | 180                      |
| (0, 1)           | divides by zero          | 90                       |
| (0, -1)          | divides by zero          | -90                      |

`(-1, 2)` and `(1, -2)` point opposite ways, but their ratio is the same -2,
so `atan` gives them the same answer. Straight along y, `y / x` divides
by zero, which a Divide gate answers with 0, so `atan` reads 0 degrees for a
direction that is really 90. `atan2` uses the signs of both parts and gives
the true heading all the way round, from -180 to 180. It takes `y` first,
then `x`.

### Which unit where

| Takes or gives radians | Takes or gives degrees |
|------------------------|------------------------|
| `sin` `cos` `tan` `asin` `acos` `atan` `atan2` | `Rotation(pitch, yaw, roll)`, `.pitch` `.yaw` `.roll`, `.ToEuler()` |

`Deg2Rad(180.0)` is pi and `Rad2Deg` goes back. Mixing them up is not an
error, only a wrong angle, so convert at the edge between the two columns.
The 0.985 threshold in [Dot and Cross](dot-and-cross.md) is the cosine of 10
degrees: `let WITHIN = cos(Deg2Rad(10.0))` at the top of a chip works it out
when the chip compiles, to 0.98481.

## Angles into a rotator

A `rotator` holds three angles in degrees: pitch, yaw and roll. Roll is a
twist around the direction itself, like tilting your head. A direction has no
twist, so a rotator made from one gets roll 0:

```wirescript name=sp3-rotator
@left in pressed: exec
var turns: int = 0
let facing = Rotation(-30.0, turns.Value * 45.0, 0.0)
let parts = facing.ToEuler()
@right out aim = facing
@right out text = "pitch ${facing.pitch}, yaw ${parts.Yaw}, roll ${parts.Roll}"

on pressed {
  turns = (turns + 1) % 8
}
```

`Rotation(pitch, yaw, roll)` takes pitch first, yaw second. Writing
`Rotation(yaw, pitch, 0.0)` by habit tilts instead of turning, and the
compiler cannot tell. Each press here turns the yaw 45 degrees further while
the pitch stays 30 degrees down.

There are two ways to read the angles back. `.pitch`, `.yaw` and `.roll` read
one each. `.ToEuler()` gives a record with `Pitch`, `Yaw` and `Roll` fields.
Each read goes through its own **Split Rotation** gate.

## From angles back to a direction

The way back uses `cos` and `sin`. Pitch splits the length-1 direction into
a flat part, `cos(pitch)`, and a height, `sin(pitch)`. Yaw then splits the
flat part between x and y:

```wirescript name=sp3-direction
const YAW = 45.0
const PITCH = 30.0
let y = Deg2Rad(YAW)
let p = Deg2Rad(PITCH)
@right out dir = Vec(cos(p) * cos(y), cos(p) * sin(y), sin(p))
```

Everything here is constant, so the compiler works it out:
`Vec(0.612, 0.612, 0.5)`, a direction halfway between x and y and 30 degrees
up, with length 1. Run the compass math on it and you get yaw 45 and pitch 30
back. A yaw of 90 with pitch 0 gives `Vec(0.0, 1.0, 0.0)`, straight along y.

`Rotation(PITCH, YAW, 0.0).ToDirection()` gets the same direction from the
game's **Quaternion to Direction** gate. The next lesson uses it.

## Modify

1. Show the heading from 0 to 360 instead of -180 to 180:
   `let heading = fmod(yaw, 360.0)`. `fmod` wraps negative angles round:
   `fmod(-90.0, 360.0)` is 270. The `%` operator keeps the sign instead, so
   `-90.0 % 360.0` stays -90.
2. Make a flashlight that points where you look. Add `var look: rotator =
   Rotation(0.0, 0.0, 0.0)` and `@right out beam = look.Value`, and in the
   handler `look = Rotation(pitch, yaw, 0.0)`. Place a **Spot Light** without
   turning its brick, and wire `beam` to its **Rotation** and a **Switch** to
   its **Enabled**.
3. Add compass letters. Call the x axis north: turning from x toward y is
   turning right, so the heading from step 1 works like a compass bearing,
   with 90 as east. Show `N`, `E`, `S` or `W` for the nearest of the four.

## Make

Make a sign on the chip that says where the visitor stands, seen from the
chip: their bearing, how far above or below the chip they are as an angle,
and how far away, such as `Visitor at 120, 15 up, 340 units`. Use the arrow
from the chip to the player instead of the aim.

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip readout -> decal Text
```

## Good to Know

- Trig is in radians and rotators are in degrees. A degree value fed to `sin`
  gives a wrong answer with no warning.
- `atan2(0.0, 0.0)` is 0. Looking straight up or down, `flat` is nearly 0,
  so the heading swings wildly at the slightest movement.
- Yaw jumps from 180 to -180 as you turn past it. Both name the same heading.
  Compare headings after `fmod`, or compare directions with `Dot` instead.
- A direction cannot carry a roll. Roll only comes from somewhere that has
  one, such as another rotator.

See [Math / Trigonometry](../../builtins.md#math--trigonometry-pure) and
[Rotation / Quaternion](../../builtins.md#rotation--quaternion-pure) in the
reference.

Next: [Rotations](rotations.md).
