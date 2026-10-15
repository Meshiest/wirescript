# Vectors and Colors

<!-- learn-links -->
<div class="learn-links">

**Before this:** [True and False](true-and-false.md)

**Next steps:** [Converting Values](converting-values.md), [Random Colors](../random/random-colors.md), [Distance and Direction](../space/distance-and-direction.md), [Blending Values](../motion/blending-values.md)

**Parts that use this:** [Launch Pad](../parts/launch-pad.md)

</div>
<!-- /learn-links -->

Some values are made of several numbers. A `color` is four: red, green, blue
and alpha. A `vector` is three, a point or a direction: x, y and z. A
`rotator` is three angles: pitch, yaw and roll. This lesson builds them, takes
them apart, and puts a color on a light.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Taking values apart](#taking-values-apart)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
switch1 Enabled -> chip red
switch2 Enabled -> chip green
switch3 Enabled -> chip blue
chip lamp -> light Color
chip lit -> light Enabled
```

## Code

```wirescript name=v3-mixer
@left in red: bool
@left in green: bool
@left in blue: bool
@right out lamp = Color(red, green, blue)
@right out lit = red || green || blue
```

`Color(r, g, b)` makes a color from three numbers, each from 0 (none) to 1
(full). A fourth number, alpha, is optional. Here each channel comes from a
switch: a bool is 1 when on and 0 when off, so each switch turns one channel
fully on or off.

With the red and green switches on and blue off, what color is the light?

## Run

1. Compile the chip and paste it in-game. Place three **Switch**es and a
   **Point Light**. Turn off the light's **Use Brick Color** setting: while it
   is on, the light takes the brick's color and ignores the wire.
2. Wire each Switch's **Enabled** to `red`, `green` and `blue`. Wire `lamp` to
   the Point Light's **Color** and `lit` to its **Enabled**.
3. Try the switches. Red and green together make yellow, all three make white,
   and with all three off the light is off.

## Under the Hood

Open the chip. `Color(...)` is one **Make Color** gate with the three pins
wired into its R, G and B inputs. `lit` is two OR gates.

## Taking values apart

Each part of a value has a name: `.r` `.g` `.b` `.a` on a color, `.x` `.y` `.z`
on a vector, and `.pitch` `.yaw` `.roll` on a rotator. Each reads one `float`.

```wirescript name=v3-walk
@left in pressed: exec
var spot: vector = Vec(0.0, 0.0, 0.0)
let step = Vec(1.0, 2.0, 0.5)
@right out text = "${spot.Value as string}, height ${spot.Value.z}"

on pressed {
  spot = spot + step
}
```

`Vec(x, y, z)` makes a vector. `+` on two vectors adds them part by part, so
each press moves `spot` by 1 along x, 2 along y and 0.5 along z.
`spot.Value as string` writes the whole vector as text, `X=1.000 Y=2.000
Z=0.500` after the first press, and `spot.Value.z` reads just the height.

Reading a part is a **Split Vector** gate (or Split Color, Split Rotation).
Reading several parts of the same value the same way (`.x`, `.y`, `.z`)
shares one split gate.

## Modify

1. Add a `@left in dim: bool` pin and dim the light while it is on:
   `@right out lamp = if dim then Color(red, green, blue) * 0.25 else Color(red, green, blue)`.
   A number times a color multiplies every channel.
2. Pick a random color on each press. `Random` on two colors picks each
   channel at random between them:

   ```wirescript name=v3-random
   @left in pressed: exec
   var shade: color = Color(1.0, 1.0, 1.0)
   @right out lamp = shade.Value
   @right out text = shade.Value.ToHex()

   on pressed {
     shade = Random(Color(0.0, 0.0, 0.0), Color(1.0, 1.0, 1.0))
   }
   ```

   Like a variable write, `Random` runs as a step in a handler. `.ToHex()`
   turns a color into text, so a sign can show which color was picked.

3. Turn a rotator one eighth of a circle per press. `Rotation(pitch, yaw,
   roll)` takes angles in degrees:

   ```wirescript name=v3-turn
   @left in pressed: exec
   var steps: int = 0
   let facing = Rotation(0.0, steps.Value * 45.0, 0.0)
   @right out aim = facing
   @right out text = "yaw ${facing.yaw}"

   on pressed {
     steps = (steps + 1) % 8
   }
   ```

   A **Spot Light** has a **Rotation** input that takes a rotator. Wire `aim`
   to it, and `text` to a **Text Decal** to watch the yaw count up by 45.

## Make

Make a light that steps from blue to red. Each press moves it one step of
five, and the sixth press starts again at blue. Keep a step count with `%`,
and mix the two colors with arithmetic:
`Color(1.0, 0.0, 0.0) * t + Color(0.0, 0.0, 1.0) * (1.0 - t)`, where `t` is a
`float` that goes from 0.0 (blue) to 1.0 (red).

```wiring
button Held -> chip pressed
chip lamp -> light Color
```

## Good to Know

- Color channels run from 0 to 1, not 0 to 255. `ColorSRGB(255, 128, 0, 255)`
  builds a color from 0 to 255 values instead.
- `Random` on two colors picks all four channels at random, alpha included.
- Asking a value for a part it does not have, such as `.z` on a color, is a
  compile error, never a silent zero.

Next: [Converting Values](converting-values.md).
