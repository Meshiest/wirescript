# Enums from Pins

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Option and Result](option-and-result.md)

**Next steps:** [Game Enums](game-enums.md)

</div>
<!-- /learn-links -->

Pins carry numbers, and an enum without data is a number underneath. This
lesson turns a count of button presses into an enum with `FromInt`, and then
passes an enum from one chip to another over a wire.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Enum pins](#enum-pins)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
chip2 "Counter"
chip "Dimmer"
button Held -> chip2 pressed
chip2 presses -> chip presses
chip brightness -> light Brightness
chip label -> decal Text
switch Enabled -> light Enabled
```

## Code

```wirescript name=en4-dimmer
enum Mode { Off, Dim, Bright }

@left in presses: int
let mode = Mode.FromInt(presses % 3)
@right out brightness = match mode {
  Off    => 0.0,
  Dim    => 20.0,
  Bright => 100.0,
}
@right out label = match mode {
  Off    => "Off",
  Dim    => "Dim",
  Bright => "Bright",
}
```

`Mode.FromInt(n)` gives the variant whose number is `n`: 0 is `Off`, 1 is
`Dim`, 2 is `Bright`, in the order they are declared. `presses % 3` keeps the
number in that range, so each press moves to the next mode and the fourth
press comes back to `Off`.

After four presses, which mode is the light in? After nine?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a **Point
   Light**, a **Switch** and a brick with a **Text Decal**.
2. Paste a counter chip too, the press counter from
   [Remembering Values](../state.md) with its count on a pin:

   ```wirescript name=en4-counter
   @left in pressed: exec
   var count: int = 0
   @right out presses = count.Value

   on pressed {
     count += 1
   }
   ```

3. Wire the Button's **Held** to the counter's `pressed` and the counter's
   `presses` to the dimmer's `presses`. Wire `brightness` to the Point
   Light's **Brightness**, and `label` to the Text Decal's **Text**. Wire the
   Switch's **Enabled** to the light's **Enabled** and flip it on.
4. Press the button. The light steps dim, bright, off, and the sign names
   each mode.

## Under the Hood

Open the dimmer chip. `FromInt` has no gate. The **Modulo** for
`presses % 3` is the mode. Its output is the variant's number, and a `Mode`
is that number.

The two `match`es share three **Equal** gates, one per variant, and each
has three **Select**s of its own. Matching the same value twice costs only
the Selects.

## Enum pins

An enum without data can be a pin. It travels as its number, and on the
other side it is the enum again. Here one chip works out the mode, and a
second chip shows it:

```wirescript name=en4-mode-out
enum Mode { Off, Dim, Bright }

@left in presses: int
@right out mode: Mode = Mode.FromInt(presses % 3)
```

```wirescript name=en4-mode-in
enum Mode { Off, Dim, Bright }

@left in mode: Mode
@right out label = match mode {
  Off    => "Off",
  Dim    => "Dim",
  Bright => "Bright",
}
```

Paste both, wire the counter's `presses` to the first chip's `presses`, the
first chip's `mode` to the second chip's `mode`, and the second's `label` to
a sign. The wire between them is an `int`.

A variant with data cannot cross a pin, since a pin carries one wire and the
data needs more:

```wirescript name=en4-payload-pin expect=WS049
enum Mode { Off, Dim(float), Bright }

@left in mode: Mode
@right out label = match mode {
  Off        => "Off",
  Dim(level) => "Dim",
  Bright     => "Bright",
}
```

```text
Error: [WS049] `mode` holds an enum value (`Mode`), so it can't cross this port: an enum is a tag plus a slot per payload field and a pin carries a single wire. Only a top-level `in`/`out` port of an enum with no payloads (a built-in game enum included) carries one, as its tag. Pass it to a `mod`, pass a chip a `*Mode` (which wires each part), or pass the tag and payload as plain values (`.Discriminant`, a `match` result) and build the enum inside
```

## Modify

1. Replace the Switch with the mode itself: add
   `@right out lit = !(mode is Mode.Off)` and wire it to the light's
   **Enabled**.
2. Add a fourth mode, `Medium`, between `Dim` and `Bright`. Both `match`es
   stop compiling until they have a `Medium` arm, and `% 3` must become
   `% 4`.
3. Take out the `% 3`. After the third press, the number matches no variant.
   Watch what the light and the sign do then.

## Make

Make a two-chip fan control. The first chip reads two **Switch**es as a
number from 0 to 3 (the first switch counts 1 and the second counts 2, so
`a + b * 2`), and sends a `Speed` enum out of a pin. The second chip takes the `Speed` and sets a light's brightness and
a sign.

```wiring
chip "Switches"
chip2 "Fan"
switch1 Enabled -> chip a
switch2 Enabled -> chip b
chip speed -> chip2 speed
chip2 brightness -> light Brightness
chip2 label -> decal Text
switch3 Enabled -> light Enabled
```

## Good to Know

- A number that matches no variant makes a `match` with no `_` arm fall
  through to a default value: 0 for a number, empty text for a string. Keep
  the number in range, as `% 3` does, or end the `match` with a `_` arm,
  which catches a number no variant has.
- The two chips are compiled separately, so nothing checks that they agree.
  If one lists `Dim` before `Off`, the wire still carries 0, 1 or 2, and the
  second chip reads the wrong mode. Declare the enum the same way in both.

See [Enums at ports](../../enums.md#enums-at-ports) and
[Enum and int conversion](../../enums.md#enum-and-int-conversion).

Next: [Game Enums](game-enums.md).
