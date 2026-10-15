# Code Door

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Bits](bits.md)

</div>
<!-- /learn-links -->

This project uses [Bits](bits.md) for a door with four switches beside it. Set the switches to the secret code and the door opens. A light beside
the switches glows brighter for each switch that is set right.

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
switch1 Enabled -> chip s0
switch2 Enabled -> chip s1
switch3 Enabled -> chip s2
switch4 Enabled -> chip s3
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
chip glow -> light Brightness
switch5 Enabled -> light Enabled
```

## Code

```wirescript name=l5-code-door
const CODE = 0b1011

@left in s0: bool
@left in s1: bool
@left in s2: bool
@left in s3: bool
let code = s0 | (s1 << 1) | (s2 << 2) | (s3 << 3)
let right = 4 - BitCount(code ^ CODE)
@right out solid = code != CODE
@right out glow: float = right
```

- `const CODE = 0b1011` is the secret code, written in binary so each digit
  is one switch: `s3` on, `s2` off, `s1` on, `s0` on, reading left to right.
- `code` packs the four switches into one number, as in [Bits](bits.md).
- `solid` is on while the code is wrong. Wired to the door, it keeps the door
  there until the code matches.
- `code ^ CODE` is XOR: a bit of it is on wherever a switch differs from the
  code. `BitCount` counts those wrong switches, so `4 - BitCount(...)` is the
  number of right ones.
- `glow: float` makes the output a decimal number, the kind a light's
  **Brightness** takes. `right` is a whole number, and the light reads it as
  a decimal.

All switches start off. How many are right before you touch anything? Which
switches do you flip to open the door?

## Run

1. Compile the chip and paste it in-game. Build a door from a few bricks and
   give one of them a **Brick Property Changer**. Place four **Switch**es in a
   row beside it, and a **Point Light**.
2. Wire each Switch's **Enabled** to `s0`, `s1`, `s2` and `s3`. Put `s3` on
   the left, so the row of switches reads like the binary number.
3. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility** inputs.
4. Wire `glow` to the Point Light's **Brightness**, and give the light's
   **Enabled** a Switch of its own, flipped on.
5. With every switch off, one switch is right: `s2`, which the code wants
   off. Flip `s0`, `s1` and `s3` on. The light grows brighter at each one,
   and on the last the door vanishes.

## Under the Hood

Open the chip. The packing is the same three Shift Left and three OR
(Bitwise) gates as before. After it come an **XOR (Bitwise)**, a **Bit
Count**, a **Subtract** for `4 - ...`, and a **Not Equal** for `solid`.

There is no gate for `CODE`. A `const` is written into the gates that use
it: the number 11 is typed straight into an input of the XOR and of the Not
Equal. Anyone who opens the chip can read the code there.

The light gives the code away too. Flip any switch. If the light gets
brighter, leave it, and if it gets dimmer, flip it back. Four tries find the
code.

## Modify

1. Choose your own code: change `CODE` to another four-bit number, from
   `0b0000` to `0b1111`. Add a sign that shows the hint as text,
   `@right out hint = "${right} of 4 right"`, wired to a **Text Decal**'s
   **Text**.

2. Make the hint something you ask for. Add `@left in peek: bool` from a
   **Button**'s **Held**, and make the light dark unless it is held:
   `@right out glow: float = if peek then right else 0`.

3. Make the door wait for a button. Here it opens only when someone presses
   **Try**, and it stays as it is until the next press, whatever the switches
   do in between:

   ```wirescript name=l5-try-button
   const CODE = 0b1011

   @left in s0: bool
   @left in s1: bool
   @left in s2: bool
   @left in s3: bool
   @left in attempt: exec
   let code = s0 | (s1 << 1) | (s2 << 2) | (s3 << 3)
   var shut: bool = true
   @right out solid = shut.Value

   on attempt {
     shut = code != CODE
   }
   ```

   Wire a Button's **Held** to `attempt`. Now no light tells you which
   switches are right, and flipping a switch after the door opens does not
   close it.

## Make

Make a door with eight switches and two codes: a guest code and an owner
code, where either one opens it. Then add a light that shows which of the
two was used.

```wiring
switch1 Enabled -> chip s0
switch2 Enabled -> chip s1
switch3 Enabled -> chip s2
switch4 Enabled -> chip s3
switch5 Enabled -> chip s4
switch6 Enabled -> chip s5
switch7 Enabled -> chip s6
switch8 Enabled -> chip s7
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
chip owner -> light Enabled
```

## Good to Know

- A door needs both **Collision Enabled** (players cannot walk through) and
  **Visibility** (players can see it). Wire `solid` to both.
- A Switch's **Enabled** stays on until it is flipped back, so in the first
  version the door closes again as soon as any switch moves.
- An unwired switch pin reads as off. With `s3` unwired, the code `0b1011`
  can never match.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
