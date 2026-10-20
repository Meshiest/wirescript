# Choosing a Value

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Truth Tables](truth-tables.md)

**Next steps:** [Toggles and Latches](toggles-and-latches.md), [Bits](bits.md), [Matching Text](../strings/matching-text.md), [Remote Control](../input/remote-control.md), [Ready Check](../input/ready-check.md)

**Parts that use this:** [Drawbridge](../parts/drawbridge.md), [Moving Platform](../parts/moving-platform.md)

</div>
<!-- /learn-links -->

The logic operators give on or off. Often you want to pick between two other
values instead: a red light or a green one, one message or another. The `if`
you met in [Reacting to the Game](../reacting-to-the-game.md) chooses which
steps run. This lesson's `if` chooses a value.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
- [Other ways to choose](#other-ways-to-choose)
<!-- /toc -->

```wiring
chip2 "Counter"
chip "Color"
button Held -> chip2 pressed
chip2 presses -> chip presses
chip color -> light Color
switch Enabled -> light Enabled
```

## Code

```wirescript name=l2-color-pick
@left in presses: int
let pick = presses % 3
@right out color = if pick == 0 then Color(1.0, 0.0, 0.0)
  else if pick == 1 then Color(0.0, 1.0, 0.0)
  else Color(0.0, 0.0, 1.0)
```

- `@left in presses: int` is a number pin. A small counter chip, given in
  [Run](#run), counts a button's presses and feeds the number in here.
- `%` is the remainder after dividing. `presses % 3` counts 0, 1, 2, then
  back to 0: 4 presses leave a remainder of 1, and 6 leave 0.
- `Color(r, g, b)` makes a color from its red, green and blue parts, each
  from `0.0` to `1.0`. `Color(1.0, 0.0, 0.0)` is red.
- `if pick == 0 then A else B` is a value: `A` when the condition is true,
  `B` when it is false. The `B` here is another `if`, so the choice has three
  answers. `then` and `else` can start new lines, as they do here.

With no presses yet the light is red. What color is it after four presses?
After nine?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a **Switch** and
   a **Point Light**.
2. Paste this counter chip as well. It adds one each time `pressed` turns on.
   [Remembering Values](../state.md) explains how.

   ```wirescript name=l2-counter
   @left in pressed: exec
   var count: int = 0
   @right out presses = count.Value

   on pressed {
     count += 1
   }
   ```

3. Wire the Button's **Held** to the counter's `pressed`, the counter's
   `presses` to the first chip's `presses`, and `color` to the Point Light's
   **Color**. Turn off the light's **Use Brick Color** setting: while it is
   on, the light takes the brick's color and ignores the wire.
4. Wire the Switch's **Enabled** straight to the Point Light's **Enabled**, and
   flip it on.
5. Press the button. The light goes red, green, blue, red again. Four presses
   leave it green, and nine leave it red.

## Under the Hood

Open the color chip. There is one Modulo gate for `presses % 3`. Both
comparisons read it, because `let pick` names one wire. Then there are two
**Select** gates, one for each `if`.

A Select gate has two value inputs, **Input A** and **Input B**, and a bool
input, **Select B**. Its output is Input B while Select B is on, and Input A
while it is off. An `if` puts the condition on **Select B**, the `then` value
on **Input B**, and the `else` value on **Input A**. The second Select's
output is the first one's Input A: that is the chain.

A Select gate does not choose which input to compute. Both are computed all
the time, and the gate passes one of them along. So an `if` value always does
the work of both arms, and the condition only picks which answer is used:

```wirescript name=l2-both-arms
@left in a: int
@left in b: int
@right out q = if b != 0 then a / b else 0
```

This chip has a Divide gate wired straight to its pins, and it divides even
while `b` is 0. That is harmless here, because the game's Divide gives 0 when
you divide by zero. But the `if` did not protect the division: the arm that is
not chosen still runs. The "Both arms evaluate" note on the reference's
[Expressions](../../expressions.md#conditional-expressions-if-then-else) page
has the details, including the exception for an arm that writes a variable
inside a handler.

## Modify

1. Add a fourth color. Change `% 3` to `% 4` and add one more
   `else if pick == 2 then ...` before the last `else`. Try
   `Color(1.0, 1.0, 0.0)`, which is yellow.

2. Choose text the same way. Add an output for a **Text Decal**'s **Text**
   that names the color:

   ```wirescript name=l2-color-name
   @left in presses: int
   let pick = presses % 3
   @right out color = if pick == 0 then Color(1.0, 0.0, 0.0)
     else if pick == 1 then Color(0.0, 1.0, 0.0)
     else Color(0.0, 0.0, 1.0)
   @right out name = if pick == 0 then "red" else if pick == 1 then "green" else "blue"
   ```

   Open it: the two outputs share the one Modulo and both Equal gates, and
   each has its own pair of Selects.

3. Add a `@left in dim: bool` pin from a Switch, and an output for the Point
   Light's **Brightness** that is one number while `dim` is on and a larger
   one while it is off. Pick the numbers by trying them in-game.

## Make

Make a three-light signal: a red, a yellow and a green **Point Light**, each
with its own `@right out` pin wired to its **Enabled**, so that one press of a
button moves which one is on. Use `presses % 3`, and an `==` for each light
in place of an `if`.

```wiring
chip2 "Counter"
chip "Signal"
button Held -> chip2 pressed
chip2 presses -> chip presses
chip red -> light1 Enabled
chip yellow -> light2 Enabled
chip green -> light3 Enabled
```

## Good to Know

- An unwired number pin reads as 0. Before the counter is wired, `presses` is
  0 and the light is red.
- Divide and Modulo give 0 when the second input is 0, rather than failing.
  That is why an untaken arm that divides by zero does no harm, and also why
  it can hide a mistake: a wrong 0 looks like a real answer.

## Other ways to choose

A chain of `if`s is not the only way to pick one of several values. The same
color picker can be written two more ways.

With `match` over an enum, each choice gets a name instead of a number.
`Hue.FromInt(n)` turns the number into the enum's value with that position
(0 is `Red`, 1 `Green`, 2 `Blue`), and `match` gives the value for each one.
It compiles to the same Selects as the `if` chain:

```wirescript name=l2-match-pick
enum Hue { Red, Green, Blue }

@left in presses: int
@right out color = match Hue.FromInt(presses % 3) {
  Red   => Color(1.0, 0.0, 0.0),
  Green => Color(0.0, 1.0, 0.0),
  Blue  => Color(0.0, 0.0, 1.0),
}
```

With a list, the choices sit in order and the number reads one by its
position. A list can only be read inside a handler, so the chip keeps the
pick in a `var` and reads the list each time `presses` changes
(`on Change(presses)`):

```wirescript name=l2-list-pick
const COLORS = [Color(1.0, 0.0, 0.0), Color(0.0, 1.0, 0.0), Color(0.0, 0.0, 1.0)]

@left in presses: int
var shown: color = Color(1.0, 0.0, 0.0)
@right out color = shown.Value

on Change(presses) {
  shown = COLORS[presses % 3]
}
```

The list costs the same few gates however many choices it holds, while an
`if` chain or a `match` grows by a gate or two per choice, so a list wins once
there are many. [Enums](../../enums.md) and
[Lists](../values/lists.md) cover both in full.

Next: [Toggles and Latches](toggles-and-latches.md).
