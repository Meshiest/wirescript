# Blending Values

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Numbers](../values/numbers.md), [Vectors and Colors](../values/vectors-and-colors.md)

**Next steps:** [Easing](easing.md)

</div>
<!-- /learn-links -->

A blend picks a value part way between two others, such as a color between
blue and red. This lesson fades a light with `Blend` and turns one range of
numbers into another with `Remap`.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Other values blend too](#other-values-blend-too)
- [From one range to another](#from-one-range-to-another)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip pressed
chip lamp -> light Color
chip text -> decal Text
switch Enabled -> light Enabled
```

## Code

```wirescript name=mo1-fade
@left in pressed: exec
var presses: int = 0
let t = presses.Value / 10.0
@right out lamp = Blend(Color(0.0, 0.2, 1.0), Color(1.0, 0.1, 0.0), t, clampAlpha = true)
@right out text = "${presses.Value} presses"

on pressed {
  presses += 1
}
```

`Blend(a, b, t)` mixes `a` and `b`. `t` says how far along: 0.0 gives `a`,
1.0 gives `b`, and 0.5 gives the value halfway. It works channel by channel:
each channel is `a + (b - a) * t`.

`presses` counts the button's presses, the way
[Remembering Values](../state.md) does, and `presses.Value / 10.0` turns it
into `t`: a tenth more blue-to-red for each press. `clampAlpha = true`
stops `t` at 1.0, so after ten presses the light stays red.

After five presses, what are the light's red, green and blue amounts? What
color is it after fifteen?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a **Point
   Light** and a brick with a **Text Decal**.
2. Turn off the Point Light's **Use Brick Color**: while it is on, the light
   takes the brick's color and ignores the wire.
3. Wire the Button's **Held** to `pressed`, `lamp` to the Point Light's
   **Color**, and `text` to the Text Decal's **Text**. Give the light's
   **Enabled** a **Switch**, flipped on.
4. Press the button. The light starts blue and moves toward red, one step
   per press. At five presses it is purple: red 0.5, green 0.15, blue 0.5.
   From ten presses on it stays red.

## Under the Hood

Open the chip. Past the counter's **Var** and its add, there is a
**Divide** for `t` and one **Blend** gate, with the two colors typed into its
**Input A** and **Input B** and `t` wired to its **Blend** input. **Clamp Alpha** is ticked in the Blend gate's settings.

`lerp(a, b, t)` is the same Blend gate without the setting. `lerp` is the
name most other languages use, and `Blend` adds `clampAlpha`.

The setting is part of the gate, chosen when the chip is built, so it takes
only a constant. A value worked out while the circuit runs is an error:

```wirescript name=mo1-clamp-wire expect=WS028
@left in presses: int
@left in stop: bool
@right out lamp = Blend(Color(0.0, 0.2, 1.0), Color(1.0, 0.1, 0.0), presses / 10.0, clampAlpha = stop)
```

```text
Error: [WS028] 'clampAlpha' is constant-only gate config and cannot take a variable or computed value
```

## Other values blend too

`Blend` and `lerp` take two numbers, two vectors, two colors, two rotators or
two quaternions, and give back the same kind. On two points, `t = 0.5` is
the point halfway between them:

```wirescript name=mo1-midpoint
@left in pressed: exec
var presses: int = 0
let start = Vec(0.0, 0.0, 0.0)
let finish = Vec(100.0, 0.0, 50.0)
let spot = lerp(start, finish, presses.Value / 4.0)
@right out text = spot as string

on pressed {
  presses += 1
}
```

Two presses make `t` 0.5, and the sign shows `X=50.000 Y=0.000 Z=25.000`.
Four presses reach `finish`. With no `clampAlpha`, six presses carry on past
it to `X=150.000 Y=0.000 Z=75.000`.

On two rotations, the blend turns the shortest way round.
[Rotations](../space/rotations.md) covers rotators and quaternions.

## From one range to another

`Remap` moves a number from one range to the same place in another. A Rigid
Slider reports how far along it is, and `Remap` turns that into a brightness:

```wirescript name=mo1-remap
@left in up: bool
@left in position: float
@right out target: float = if up then 100.0 else 0.0
@right out brightness = Remap(position, 0.0, 100.0, 0.0, 4.0)
```

`Remap(value, inMin, inMax, outMin, outMax)` finds where `value` sits between
`inMin` and `inMax`, then gives the number at the same place between
`outMin` and `outMax`. A position of 25 is a quarter of the way from 0 to
100, so the brightness is a quarter of the way from 0 to 4: 1.0. It is a
`lerp` that works out its own `t`:
`lerp(outMin, outMax, (value - inMin) / (inMax - inMin))`.

`if up then 100.0 else 0.0` picks 100 while the switch is on and 0 while it
is off ([Choosing a Value](../logic/choosing-a-value.md)).

1. Place a **Rigid Slider**, a short platform welded to its moving end, a
   **Switch** and a **Point Light**. In the slider's settings, turn on
   **Positions Are Percentages**.
2. Wire the Switch's **Enabled** to `up`, `target` to the slider's **Target
   Position**, and the slider's **Current Position** to `position`. Wire
   `brightness` to the Point Light's **Brightness**.
3. Flip the switch. The platform slides out, and the light brightens as it
   goes. Flip it back: the light dims as the platform returns.

## Modify

1. Make the light dim as the platform slides out: swap the output range,
   `Remap(position, 0.0, 100.0, 4.0, 0.0)`.
2. Color the light by the slider's position too. Add
   `@right out lamp = Blend(Color(0.0, 0.2, 1.0), Color(1.0, 0.1, 0.0), position / 100.0, clampAlpha = true)`
   to the Remap chip and wire it to the light's **Color**.
3. Make the fade in the first chip start over.
   `(presses.Value % 11) / 10.0` counts 0.0 to 1.0 in tenths and then goes
   back to 0.0.

## Make

Make a heat lamp with three colors. A counted button runs it from green
through yellow to red over twenty presses: green to yellow for the
first ten, yellow to red for the next ten. Blend twice, and choose between
the two blends with `if presses.Value < 10 then ... else ...`.

```wiring
button Held -> chip pressed
chip lamp -> light Color
switch Enabled -> light Enabled
```

## Good to Know

- `t` must be a `float`. `presses.Value / 10` is an `int` divided by an
  `int`, which drops the fraction: it is 0 until the tenth press and then jumps
  ([Numbers](../values/numbers.md)). The compiler accepts it without a
  warning. Write `10.0`.
- `Remap` does not stop at the ends: a value past `inMax` gives a number past
  `outMax`. Wrap it in `clamp(...)` when the input can overshoot.
- `Blend` on two colors mixes the channel numbers. `a.ColorBlend(b, t)` mixes
  colors in a color space of your choice (`blendSpace = Oklab`), which can
  look more even to the eye.
- A Point Light's **Use Brick Color** overrides a wired **Color**. Turn it off
  whenever a wire sets the color.

See [Math](../../builtins.md#math--trigonometry-pure), [Color](../../builtins.md#color-pure),
[Value conversions](../../builtins.md#value-conversions-pure) and
[Gate config properties](../../builtins.md#gate-config-properties) in the reference.

Next: [Easing](easing.md).
