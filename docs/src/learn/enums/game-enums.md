# Game Enums

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Enums from Pins](enums-from-pins.md)

**Parts that use this:** [Paint Gun](../parts/paint-gun.md)

</div>
<!-- /learn-links -->

Some enums come from the game itself, for easing curves, brick settings and
the typefaces of on-screen text. They need no `enum` declaration. This lesson eases a light on and off with them, and recolors
the chip's own brick.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Brick properties](#brick-properties)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip held
chip brightness -> light Brightness
switch Enabled -> light Enabled
```

## Code

```wirescript name=en5-bounce
@left in held: bool
let target = if held then 100.0 else 0.0
@right out brightness: float = Tween(target, 1.0, function = EasingFunction.Bounce, direction = EasingDirection.Out)
```

`Tween(target, 1.0, ...)` is a value that moves toward `target`, taking one
second to get there. `function` picks the shape of the move, and `direction`
picks which end of it the shape applies to.

`EasingFunction` and `EasingDirection` are **game enums**. The compiler reads
them from the game's own lists, so they need no declaration. `EasingFunction` has
`Linear`, `Sine`, `Quad`, `Cubic`, `Quart`, `Quint`, `Expo`, `Circ`, `Back`,
`Elastic` and `Bounce`, and `EasingDirection` has `In`, `Out` and `InOut`.

Hold the button. How does the light come on? What does it do when you let
go?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a **Point
   Light** and a **Switch**.
2. Wire the Button's **Held** to `held`, `brightness` to the Point Light's
   **Brightness**, and the Switch's **Enabled** to the light's **Enabled**.
   Flip the switch on.
3. Hold the button. The light rises over a second and bounces as it
   arrives. Let go, and it falls back the same way.

## Under the Hood

Open the chip. There are two gates: a **Select** for `target`, and a
**Tween**. The Tween's **Function** input has 10 typed into it, and its
**Direction** input has 1.

Those are the game's own numbers for `Bounce` and `Out`.
`EasingFunction.Bounce.Discriminant` is 10. An enum you declare numbers its
variants from 0 in your order. A game enum uses the numbers the game saves.

The Tween takes its function on a wire, so a variable can choose it while
the circuit runs:

```wirescript name=en5-styles
@left in held: bool
@left in next: exec
var style: EasingFunction = EasingFunction.Linear
@right out brightness: float = Tween(if held then 100.0 else 0.0, 1.0, function = style)
@right out name = match style {
  Linear  => "Linear",
  Bounce  => "Bounce",
  Elastic => "Elastic",
  _       => "Style ${style.Discriminant}",
}

on next {
  style = EasingFunction.FromInt((style.Discriminant + 1) % 11)
}
```

Each press of `next` moves to the next of the eleven functions, and the
sign names it.

Not every gate takes its enum on a wire. The **Easing** gate has its
function as a setting, chosen when the chip is built, so it needs a constant:

```wirescript name=en5-setting expect=WS028
@left in t: float
var style: EasingFunction = EasingFunction.Bounce
@right out level = Easing(0.0, 100.0, t, function = style)
```

```text
Error: [WS028] unknown enum member 'style' for EBREasingFunction; expected one of: Linear, Sine, Quad, Cubic, Quart, Quint, Expo, Circ, Back, Elastic, Bounce
```

The message reads `style` as a member name it does not know. The same error
goes on to say that `Function` is a constant-only setting.

## Brick properties

`BrickProperty` names the settings of a brick: `Visible`, `Color`,
`Material`, the collision flags and more. This chip recolors its own brick
at each press:

```wirescript name=en5-chip-color
@left in pressed: exec
var lit: bool = false

on pressed {
  lit = !lit
  let c = if lit then Color(1.0, 0.8, 0.0) else Color(0.2, 0.2, 0.2)
  GetMicrochipBrick().SetBrickProperty(BrickProperty.Color, c)
}
```

`GetMicrochipBrick()` is the chip's own brick. The **Set Brick Property**
gate has the property as a setting, so `BrickProperty.Color` must be written
out, not chosen by a variable.

## Modify

1. Try other curves in the first chip: `EasingFunction.Elastic`,
   `EasingFunction.Back`, and `direction = EasingDirection.InOut`.
2. Add a second button to the styles chip that steps the direction too:
   `var dir: EasingDirection = EasingDirection.In`, passed as
   `direction = dir`, and stepped with `% 3`.
3. In the brick chip, also set `BrickProperty.Material` to `BrickMaterial.Glow`
   when lit and `BrickMaterial.Plastic` when not.

## Make

Make a light show. A Clock flips the light's target every two seconds, and
on every second flip it moves to the next easing function. A sign shows the
name of the curve being shown.

```wiring
chip brightness -> light Brightness
chip label -> decal Text
switch Enabled -> light Enabled
```

## Good to Know

- A Tween that gets a new target part way through should head for it from
  where it is, so letting go early turns the light around mid-bounce.
- `GetMicrochipBrick()` does not yet name the right brick inside a spawned
  prefab.

See [Built-in game enums](../../enums.md#built-in-game-enums),
[Gate config properties](../../builtins.md#gate-config-properties) and
[GetBrickProperty / SetBrickProperty](../../builtins.md#getbrickproperty--setbrickproperty).

Next: pick a lesson from the tree on the [Learn](../README.md) page.
