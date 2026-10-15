# Fmt

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Interpolation](interpolation.md)

**Next steps:** [Number Options](number-options.md)

</div>
<!-- /learn-links -->

A `${}` template holds its text and its values together. `Fmt` keeps them
apart: you write the template once, with lettered slots, and fill it with
values each time you use it. One template can then serve many signs.

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
chip2 "Counter"
chip "Sign"
button1 Held -> chip2 addRed
button2 Held -> chip2 addBlue
chip2 red -> chip red
chip2 blue -> chip blue
chip redText -> decal1 Text
chip blueText -> decal2 Text
```

## Code

```wirescript name=tp2-lines
const LINE = "{A}: {B}"

@left in red: int
@left in blue: int
@right out redText = Fmt(LINE, "Red", red)
@right out blueText = Fmt(LINE, "Blue", blue)
```

`const LINE = "{A}: {B}"` names a fixed piece of text. `Fmt(format, ...)`
uses the Format String gate directly. The first argument is the format: the
template text. The rest are the values, up to seven of them. The first value
fills `{A}`, the second fills `{B}`, and so on up to `{G}`. These letters are
the names the gate's own tooltip uses for its inputs.

Red is pressed twice and Blue once. What does each sign show? Now change
`LINE` to `"{B} for {A}"`. What do they show?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and two
   bricks with a **Text Decal**.
2. Paste the counter chip from [Interpolation](interpolation.md#run). Wire
   the Buttons' **Held** to its `addRed` and `addBlue`, its `red` and `blue`
   to this chip's `red` and `blue`, and `redText` and `blueText` to each Text
   Decal's **Text**.
3. Press the red button twice and the blue one once. The signs read
   `Red: 2` and `Blue: 1`.

## Under the Hood

Open the chip: two Format String gates, both with the **Format String**
`{A}: {B}`. In each, **Input A** holds its word, set inside the gate, and
**Input B** is wired from a pin. A `const` is not a gate: its text is copied
into every gate that uses it, so changing `LINE` changes both signs.

- A slot can be used more than once and in any order:
  `Fmt("{B} {A} {B}", x, y)` prints `y x y`.
- The numbered form `{0}` to `{6}` also works: `{0}` is Input A. It is what
  `${}` compiles to, as you saw in [Interpolation](interpolation.md). Use the
  letters in your own templates: the options in the next lessons are written
  with them.
- Passing more than seven values is a compile error (`WS011`): the gate has
  no eighth input. `${}` splits long templates for you, and `Fmt` does not.

## Modify

1. Change `LINE` to `"{A} team: {B} points"`. Both signs change, and you
   edited one line.
2. Add a third value: how many points each team still needs to win.

   ```wirescript name=tp2-to-go
   const GOAL = 10
   const LINE = "{A}: {B} ({C} to go)"

   @left in red: int
   @left in blue: int
   @right out redText = Fmt(LINE, "Red", red, GOAL - red)
   @right out blueText = Fmt(LINE, "Blue", blue, GOAL - blue)
   ```

3. The format does not have to be fixed. Pick one of two templates with a
   **Switch**: the format is then a wire into the gate's **Format String**
   input.

   ```wirescript name=tp2-two-styles
   const SHORT = "{A}: {B}"
   const LONG = "Team {A} has {B} points"

   @left in wide: bool
   @left in red: int
   let line = if wide then LONG else SHORT
   @right out redText = Fmt(line, "Red", red)
   ```

   Wire a Switch's **Enabled** to `wide` and flip it.

## Make

Make a scoreboard for four teams. Use one `const` template for every row, and
join the four rows into one sign with a `${}` template that puts `<br>`
between them. How many Format String gates does it take?

```wiring
button1 Held -> chip red
button2 Held -> chip blue
button3 Held -> chip green
button4 Held -> chip yellow
chip board -> decal Text
```

## Good to Know

- In a format string, a brace you type starts a slot, and a slot the gate
  cannot fill prints `0`: `Fmt("literal{a}brace")` shows `literal0brace`.
  `Fmt` passes its format to the gate as written, unlike `${}`, which doubles
  your braces.
- A numbered slot with no value wired prints `0`: `Fmt("{0}{1}", 1)` shows
  `10`.

See [String Formatting](../../builtins.md#string-formatting-pure) for the
reference.

Next: [Number Options](number-options.md).
