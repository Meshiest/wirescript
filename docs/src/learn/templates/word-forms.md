# Word Forms

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Number Options](number-options.md)

**Next steps:** [Templates Chosen at Run Time](runtime-templates.md)

</div>
<!-- /learn-links -->

Numbers need the right words around them: 1 apple but 2 apples, 1st but 2nd.
A bool in a slot prints `1` when true, which is rarely what a sign should say.
The Format String gate can pick these words itself, in the template.

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
button Held -> chip2 addRed
chip2 red -> chip presses
switch Enabled -> chip isOpen
chip doorText -> decal1 Text
chip appleText -> decal2 Text
chip placeText -> decal3 Text
```

## Code

```wirescript name=tp4-words
@left in isOpen: bool
@left in presses: int
@right out doorText = Fmt("Door: {A:Open=Closed}", isOpen)
@right out appleText = Fmt("{A} {A}|plural(one=apple,other=apples)", presses)
@right out placeText = Fmt("You came {A}{A}|ordinal(one=st,two=nd,few=rd,other=th)", presses)
```

- `{A:Open=Closed}` is an option for a bool: the word before `=` when the
  value is true, the word after it when false.
- `{A}|plural(...)` is a word form. A slot followed by a bar and a form
  prints a word picked by the value, instead of the value itself. The form
  lists words by category: `one` is the word for a count of one, `other` the
  word for larger counts. That is why the template has `{A}` twice: the first
  prints the number, the second its word.
- `{A}|ordinal(...)` picks an ending for a place: `one`, `two`, `few` and
  `other` are the categories the gate sorts places into, giving `1st`, `2nd`,
  `3rd` and `4th`.

After one press, what do the signs say? After two?

## Run

1. Compile the chip and paste it in-game. Place a **Switch**, a **Button**
   and three bricks with a **Text Decal**.
2. Paste the counter chip from [Interpolation](interpolation.md#run). Wire
   the Button's **Held** to its `addRed`, its `red` to `presses`, and the
   Switch's **Enabled** to `isOpen`. Wire the three outputs to the three Text
   Decals' **Text**.
3. Flip the switch: `Door: Open`, `Door: Closed`. Press the button and watch
   the words change with the number.
4. Find out two things the lesson does not tell you. What does `appleText`
   say before the first press? What does `placeText` say at 11, 12 and 13?

## Under the Hood

Open the chip: three Format String gates and nothing else. The words are
inside each gate's **Format String**, and the gate picks them as the value
changes. No compare or select gates do the choosing.

By hand, the plural alone takes a compare and a select:
`let word = if presses == 1 then "apple" else "apples"`. The ordinal would
take a chain of them.

These templates stay real gates even when every value is a constant: the
compiler leaves options and word forms for the game to apply.

## Modify

1. A form can insert the value itself. This prints `one apple` for 1 and
   `3 apples` for 3:

   ```wirescript name=tp4-basket
   @left in presses: int
   @right out basket = Fmt("{A}|plural(one=one apple,other={A} apples) in the basket", presses)
   ```

   Here the number appears only inside the `other` form, so there is one
   `{A}` before the bar.
2. Two values with their own words. Add a second Button, counted on the
   counter's `blue`, on `coins`:
   `Fmt("{A} {A}|plural(one=player,other=players) found {B} {B}|plural(one=coin,other=coins)", presses, coins)`.
3. Mix an ordinal and a bool option. `place <= 3` is a bool, so `{B}` can turn
   it into words:

   ```wirescript name=tp4-podium
   @left in place: int
   @right out result = Fmt("{A}{A}|ordinal(one=st,two=nd,few=rd,other=th) place. Medal: {B:Yes=No}", place, place <= 3)
   ```

## Make

Make a door sign that reads `The door is open. It has been opened 1 time.`,
with the right word for open or closed and for time or times. Count the
openings in a `var` that a handler adds to each time the switch turns on, as
in [Remembering Values](../state.md).

```wiring
switch Enabled -> chip isOpen
chip text -> decal Text
```

## Good to Know

- A `bool` in a plain slot prints `1` when true. Use `{A:Yes=No}`, with your
  own words, to show it on a sign.

See [String Formatting](../../builtins.md#string-formatting-pure) for the
reference.

Next: [Templates Chosen at Run Time](runtime-templates.md).
