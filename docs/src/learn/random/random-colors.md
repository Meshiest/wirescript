# Random Colors

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Dice](dice.md), [Vectors and Colors](../values/vectors-and-colors.md)

**Next steps:** [Shuffling](shuffling.md)

</div>
<!-- /learn-links -->

`Random` on two colors picks each channel on its own, between the two colors'
values for it. Choose the two colors well and you get every shade of one
family and nothing else. This lesson makes a fire that flickers.

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
switch Enabled -> chip burning
chip lamp -> light Color
chip lit -> light Enabled
```

## Code

```wirescript name=rn2-fire
@left in burning: bool
var flame: color = Color(1.0, 0.5, 0.1)
@right out lamp = flame.Value
@right out lit = burning

on Clock(interval = 0.1, enabled = burning) {
  flame = Random(Color(0.8, 0.3, 0.0, 1.0), Color(1.0, 0.6, 0.1, 1.0))
}
```

- Ten times a second, while `burning` is on, the Clock picks a new `flame`.
- `Random(low, high)` on two colors rolls red between the two reds (0.8 to
  1.0), green between the two greens (0.3 to 0.6), blue between the two blues
  (0 to 0.1), and alpha between the two alphas.
- Both alphas are 1.0, so alpha always comes out 1.0. Give the bounds
  different alphas and the alpha is random too.
- `lit` passes the switch straight through, so the light is off when the fire
  is out.

Can this fire ever turn blue? Can it turn pure yellow (full red, full green)?

## Run

1. Compile the chip and paste it in-game. Place a **Switch** and a **Point
   Light**. Turn off the light's **Use Brick Color**. While it is on, the
   light takes the brick's color and ignores the wire.
2. Wire the Switch's **Enabled** to `burning`. Wire `lamp` to the Point
   Light's **Color** and `lit` to its **Enabled**.
3. Flip the switch on. The light flickers through reds and oranges. Flip it
   off, and the light goes out.

## Under the Hood

Open the chip. There is a **Clock** with **Interval Seconds** set to 0.1 and
the switch wired to its **Enabled**, one **Random** gate with the two colors
typed into **Min** and **Max**, and a **Set Variable**. The same Random gate
that rolls dice rolls colors. The type of **Min** and **Max** decides what it
picks.

The fire cannot turn blue, because blue never rises above 0.1 and red never
falls below 0.8. It cannot turn pure yellow either, since green stops at 0.6. Each
channel stays inside its own range, whatever the others roll.

## Modify

1. Make a blue flame. Change the bounds to `Color(0.0, 0.2, 0.6, 1.0)` and
   `Color(0.3, 0.6, 1.0, 1.0)` for flickering blues.
2. Pick from a list of colors instead of mixing channels. A `const` array
   holds the colors, and a random index picks one:

   ```wirescript name=rn2-palette
   const PALETTE = [
     Color(1.0, 0.0, 0.0),
     Color(1.0, 0.5, 0.0),
     Color(1.0, 1.0, 0.0),
     Color(0.0, 1.0, 0.0),
     Color(0.0, 0.3, 1.0),
     Color(0.6, 0.0, 1.0),
   ]

   @left in pressed: exec
   var shade: color = Color(1.0, 1.0, 1.0)
   @right out lamp = shade.Value

   on pressed {
     shade = PALETTE[Random(0, 5)]
   }
   ```

   Wire a **Button**'s **Held** to `pressed`. The light only ever shows one of
   the six colors. Indexes run from 0, so six colors need `Random(0, 5)`.
3. Flicker the brightness too. `Random` gives whole numbers, so roll a whole
   number and divide for a decimal: `Random(70, 100) / 100.0` gives 0.70 to
   1.00. Keep it in a `float` variable, and wire it to the light's
   **Brightness**.

## Make

Make a party floor of four lights. On every beat (a Clock at half a second),
each light picks a color from the palette. Then make sure no two lights next
to each other show the same color on the same beat.

```wiring
chip lamp1 -> light1 Color
chip lamp2 -> light2 Color
chip lamp3 -> light3 Color
chip lamp4 -> light4 Color
switch Enabled -> light1 Enabled
switch Enabled -> light2 Enabled
switch Enabled -> light3 Enabled
switch Enabled -> light4 Enabled
```

## Good to Know

- A Point Light with **Use Brick Color** on ignores a wired **Color**. Turn it
  off.
- `Random` on two colors picks alpha too. Give both bounds the same alpha to
  keep it fixed.
- `Random` rolls whole numbers, or colors, vectors and rotators. For a
  decimal, roll a whole number and divide.
- A Clock this fast changes the light every tenth of a second for as long as
  the switch is on. Turn it off with `enabled` when nothing needs to change.

See [Random](../../builtins.md#random-exec) and
[Color](../../builtins.md#color-pure) in the reference.

Next: [Shuffling](shuffling.md).
