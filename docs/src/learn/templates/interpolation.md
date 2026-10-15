# Interpolation

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remembering Values](../state.md)

**Next steps:** [Fmt](fmt.md)

</div>
<!-- /learn-links -->

Most text a circuit shows mixes fixed words with live values: a score, a
count, a name. `${}` puts a value into a piece of text. This lesson shows what
it compiles to, and why it is cheaper than joining the pieces yourself.

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
chip joined -> decal1 Text
chip templated -> decal2 Text
```

## Code

```wirescript name=tp1-scoreboard
@left in red: int
@left in blue: int
@right out joined = "Red " .. red .. ", Blue " .. blue .. ", Total " .. (red + blue)
@right out templated = "Red ${red}, Blue ${blue}, Total ${red + blue}"
```

Text in quotes is a string. `..` joins two strings into one, and turns a
number into text on the way. `${...}` inside a string puts the value of what
is between the braces at that spot. The text around it stays as written. A
string with `${}` in it is called a template.

Both outputs build the same words. Which one takes fewer gates? Guess a number
for each before you open the chip.

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and two
   bricks with a **Text Decal**.
2. Paste this counter chip as well. It counts each button's presses, the way
   [Remembering Values](../state.md) does, and puts the two counts on pins.
   The next lessons in this branch use it too.

   ```wirescript name=tp1-counter
   @left in addRed: exec
   @left in addBlue: exec
   var redCount: int = 0
   var blueCount: int = 0
   @right out red = redCount.Value
   @right out blue = blueCount.Value

   on addRed { redCount += 1 }
   on addBlue { blueCount += 1 }
   ```

3. Wire the first Button's **Held** to the counter's `addRed` and the
   second's to `addBlue`. Wire the counter's `red` and `blue` to the first
   chip's `red` and `blue`.
4. Wire `joined` to one Text Decal's **Text** and `templated` to the other's.
5. Press each button a few times. The two signs always read the same.

## Under the Hood

Open the first chip and count the gates.

- `joined` is five **Append String** gates in a row. Each `..` is one gate,
  and each gate joins exactly two pieces. Three values with text in front of
  each make six pieces, and six pieces take five joins. In general, N values
  with text in front of each cost 2N-1 gates.
- `templated` is one **Format String** gate. Its **Format String** is
  `Red {0}, Blue {1}, Total {2}`: each `${}` became a numbered slot, and the
  values are wired into the gate's **Input A**, **Input B** and **Input C**.
- `red + blue` is one Add gate, and both outputs share it.

A Format String gate has seven inputs, **Input A** to **Input G**, so one gate
holds up to seven values. Write more and the compiler splits the template
across several gates and joins their outputs, so you never have to count:

```wirescript name=tp1-countdown
@left in n: int
@right out row = "${n} ${n - 1} ${n - 2} ${n - 3} ${n - 4} ${n - 5} ${n - 6} ${n - 7} ${n - 8}"
```

This row has nine values. Open the chip: there are three Format String gates,
one with seven slots, one with two, and one that joins the first two.

### A brace in the text

The Format String gate reads `{0}` as a slot, so braces mean something to it.
In a `${}` template you can still type `{` and `}` as text: the compiler
doubles each one in the gate's text so the gate does not take it for a slot.

```wirescript name=tp1-braces
@left in red: int
@right out sign = "{red} = ${red}"
```

Open the chip: the gate's **Format String** is `{{red}} = {0}`. Wire `sign` to
a Text Decal and check that it shows single braces. To show a `$`, write `\$`.

## Modify

1. Add who is ahead. `if ... then ... else` picks one of two values: here
   `"Red"` when `red > blue` is true, `"Blue"` when it is false. Give it a
   name with `let`, then put the name in the template.

   ```wirescript name=tp1-leader
   @left in red: int
   @left in blue: int
   let leader = if red > blue then "Red" else "Blue"
   @right out text = "Red ${red}, Blue ${blue}. ${leader} leads."
   ```

   What does the sign say when the scores are equal?
2. Fix the tie: an `else` can hold another `if`.
   `if red == blue then "Nobody" else if red > blue then "Red" else "Blue"`.
3. Put the two scores on separate lines with `<br>`, and color the names
   with `<color="RRGGBB">` ([Rich Text Markup](../../game-knowledge.md#rich-text-markup)).
   Write the template in single quotes so the double quotes in the tag need
   no escaping: `'<color="ff4040">Red ${red}<br><color="4080ff">Blue ${blue}'`.
   It is still one gate: tags are just more text.

## Make

Make a sign for three teams that shows each score and the total, written as
one template. How many gates does it take? How many would the `..` version
take?

```wiring
button1 Held -> chip red
button2 Held -> chip blue
button3 Held -> chip green
chip text -> decal Text
```

## Good to Know

- A `bool` in a slot prints as a number: `true` shows `1`.
  [Word Forms](word-forms.md) prints words instead.

See [String Interpolation](../../expressions.md#string-interpolation) for the
reference.

Next: [Fmt](fmt.md).
