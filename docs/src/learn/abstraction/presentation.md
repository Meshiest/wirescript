# Presentation

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Chips](chips.md)

</div>
<!-- /learn-links -->

A circuit that works can still be hard to use: which pin is which, which
chip does what. Annotations, the words that start with `@`, and `///` doc
comments change how a chip looks in-game without changing what it does.

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
button1 Held -> chip red
button2 Held -> chip blue
button3 Held -> chip reset
chip board -> decal1 Text
chip leader -> decal2 Text
```

## Code

```wirescript name=pres-scoreboard
/// One button per team, one to reset.

@label("Scoreboard")
@color(Color(0.1, 0.1, 0.4))

@left @label("Red +1") in red: exec
@left @label("Blue +1") in blue: exec
@bottom @label("Reset") in reset: exec

/// Counts presses until cleared.
@color(Color(0.6, 0.6, 0.6)) @material(Metal)
chip Tally(press: exec, clear: exec) -> int {
  var n: int = 0
  on press { n += 1 }
  on clear { n = 0 }
  out _ = n.Value
}

let redScore = Tally(red, reset)
let blueScore = Tally(blue, reset)

@label("Rounds: ${rounds}") @material(Glow)
var rounds: int = 0
on reset { rounds += 1 }

@right @label("Board") out board = "Red ${redScore}  Blue ${blueScore}"
@top out leader = if redScore > blueScore then "Red" else if blueScore > redScore then "Blue" else "Tied"
```

This is the scoreboard from [Chips](chips.md), with annotations added:

- The lines at the top, each group followed by a blank line, are about the
  whole chip: `///` is its description, `@label` its title, `@color` the
  color of its brick.
- `@label("Red +1")` on a pin changes the name shown beside it in-game.
- `@bottom` and `@top` put a pin on the chip's bottom or top edge instead of
  a side.
- `@color` and `@material` on `Tally` style its microchip brick. `Metal` is
  one of the brick materials. `Glow` lights the `rounds` variable's brick.
- `@label("Rounds: ${rounds}")` on a top-level `var` is a live label, which
  shows the value as it changes.

Only one of these annotations adds a gate. Which one?

## Run

1. Compile the chip and paste it in-game. The brick is blue and
   labeled `Scoreboard`. `Red +1` and `Blue +1` are on its left side, `Reset`
   on the bottom, `Board` on the right, and `leader` on top.
2. Wire three **Button**s' **Held** to the three input pins, and the `board`
   and `leader` outputs to two **Text Decal**s' **Text**.
3. Open the chip. Its plane has a header, with `Scoreboard` in large letters and
   the description below it. The two `Tally` chips are gray metal. Open one
   and its header shows `Tally` and `Counts presses until cleared.`
4. Press Reset a few times. The glowing variable's label counts the rounds.

## Under the Hood

Compiled without any annotation, this chip has 22 gates. With them it has
23. The extra one is a **Format String** that builds the text
`Rounds: 3` for the live label. Every other annotation only sets a label, a
color or a position on a brick that exists anyway.

A `mod` has no brick of its own (its gates are copied into the caller), so
it cannot take a label, a color or a pin side:

```wirescript name=pres-mod-label expect=WSP001
@label("Board")
mod medal(hits: int) -> string {
  return if hits >= 10 then "gold" else "bronze"
}
```

```text
Error: [WSP001] annotations are not allowed on 'mod' declarations
```

The blank lines at the top matter. An annotation with no blank line after
it belongs to the next declaration. `@label("Scoreboard")` directly above
`@left in red: exec` would rename that pin instead of the chip.

## Modify

1. Lay the gates out like the code. Add `@layout("code")` as the first
   annotation at the top, directly under the description's blank line.
   `@layout` and `@flat` must come before `@label` and `@color`:

   ```wirescript nocompile
   /// One button per team, one to reset.

   @layout("code")
   @label("Scoreboard")
   @color(Color(0.1, 0.1, 0.4))
   ```

   Open the chip. Each line of code is a row of gates, in the same order as
   the source, and indented the same way. A `//` comment on a line of its
   own shows up as text between the rows.

2. Make it small. Replace `@layout("code")` with two lines, `@flat` and
   `@layout("cube")`. `@flat` builds every chip's gates onto one grid, so
   the two `Tally` chips are gone, along with their pins, their color and
   their headers. `@layout("cube")` packs the gates into a tight block. The
   circuit works as before. A file with a `fn` cannot be flat, because the
   function runs on its own board. `@flat` reports `WS080` there.

3. Add `@closed` in front of `chip Tally` so the two chips arrive shut, and
   move `leader` to the bottom edge with `@bottom`. Pins on the same edge
   line up in the order they are declared.

## Make

Take the largest circuit you have built so far and present it: a title and
description for the chip, a label on every pin that says what to wire there,
a color for each inner chip, and a live label on the one variable you most
often want to check.

## Good to Know

- `///` directly above `@label` or `@color` at the top, with no blank line
  between them, keeps the description but hands the annotations to the first
  declaration below, not the chip. Leave a blank line after the description.
- `@label` changes only the name shown in the world. When you wire a pin, it
  is still listed by the name in the code, `red`, not `Red +1`.

See also: [Outer Rerouter Pins](../../statements.md#left--right--top--bottom-outer-rerouter-pins),
[`@label`](../../statements.md#label-port-display-label),
[`@color` / `@material`](../../statements.md#color--material-brick-color-and-material),
[`@layout("code")`](../../statements.md#layoutcode-source-shaped-gate-layout),
[`@flat`](../../statements.md#flat-inline-every-chip) and
[Doc Comments](../../syntax.md#doc-comments).

Next: pick a lesson from the tree on the [Learn](../README.md) page.
