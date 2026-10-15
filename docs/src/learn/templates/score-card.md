# Score Card

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Templates Chosen at Run Time](runtime-templates.md), [Rich Text](../strings/rich-text.md)

</div>
<!-- /learn-links -->

This project uses the whole Templates branch in one template, filled by one
gate: a sign with a styled title, numbers with options, the right words for
each count, and a status line.

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
chip2 red -> chip hits
chip2 blue -> chip misses
switch Enabled -> chip isOpen
chip card -> decal Text
```

## Code

```wirescript name=tp6-card
const CARD = '<size="32"><b><color="ffd040">TARGET RANGE</></></><br><color="ffffff">{A:,} {A}|plural(one=hit,other=hits), {B:,} {B}|plural(one=miss,other=misses)<br>Accuracy: {C:.1}%<br>{D:Open=Closed}'

@left in hits: int
@left in misses: int
@left in isOpen: bool
let accuracy = hits * 100.0 / (hits + misses)
@right out card = Fmt(CARD, hits, misses, accuracy, isOpen)
```

- The title line is rich text from [Rich Text](../strings/rich-text.md): a
  size, bold and a color, each closed with `</>`. The color tag before the
  body is never closed, so it lasts to the end.
- `{A:,}` and `{C:.1}` are number options from
  [Number Options](number-options.md).
- `{A}|plural(...)` and `{D:Open=Closed}` are word forms from
  [Word Forms](word-forms.md).
- `CARD` is a `const` template filled by `Fmt`, as in [Fmt](fmt.md).

Before anything is pressed, `hits + misses` is 0. What does the accuracy line
say then?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s, a **Switch**
   and a brick with a **Text Decal**.
2. Paste the counter chip from [Interpolation](interpolation.md#run). Wire
   the Buttons' **Held** to its `addRed` and `addBlue`, its `red` to `hits`
   and its `blue` to `misses`, and the Switch's **Enabled** to `isOpen`. Wire
   `card` to the Text Decal's **Text**.
3. Press each button a few times and flip the switch. The card should read
   like this after three hits and one miss, with the switch on:

   ```text
   TARGET RANGE
   3 hits, 1 miss
   Accuracy: 75.0%
   Open
   ```

## Under the Hood

Open the chip. There is one add, one multiply and one divide for the
accuracy, and one **Format String** gate for the whole card: four gates in
all. Everything else (the title, the colors, the words, the options) is text
inside the gate's **Format String**.

The accuracy needs no check for zero shots. Dividing by zero gives 0, as in
[Numbers](../values/numbers.md), so a fresh card reads `Accuracy: 0.0%`.

The template must stay in one `Fmt`. Built with `..` instead, each piece
would be its own gate, and the options and word forms would not work, because
only the Format String gate reads them.

## Modify

1. Add a best streak. Give the chip a third pin, `streak`, add
   `<br>Best streak: {E}` to the template, and pass `streak` as the fifth
   value.
2. Color the accuracy by how good it is. Pick the color with a lookup `Fmt`
   from [Templates Chosen at Run Time](runtime-templates.md), using
   `clamp(hits * 3 / (hits + misses), 0, 2)` as the position, and pass it
   into `CARD` as one more slot: `<color="{F}">Accuracy: {C:.1}%`. The
   position must be a whole number: `hits * 3 / (hits + misses)` divides two
   `int`s, which drops the fraction, while `accuracy` is a decimal and would
   build a format like `{1.47}` that picks nothing.
3. Give the card a place line: `You are {G}{G}|ordinal(one=st,two=nd,few=rd,other=th)`,
   with `G` from a second counter chip's `red`. A template holds at most
   seven values, `A` to `G`, so this one is now full.

## Make

Make a shop sign for a vending machine. It shows the item name in a large
colored title, the price with thousands grouped (`1,250 coins`), how many are
left with the right word (`1 can left`, `3 cans left`), and `Sold out` or
`In stock` from a bool. Use one `const` template and one `Fmt`.

```wiring
chip2 "Counter"
chip "Shop"
button Held -> chip2 addRed
chip2 red -> chip remaining
switch Enabled -> chip inStock
chip card -> decal Text
```

## Good to Know

- A template holds seven values at most, `{A}` to `{G}`. A card that needs
  more must be split into two `Fmt`s joined with `..`.
- A misspelled option is not an error. The compiler cannot check what is
  inside a template, so check the sign itself after every change.
- Text that comes from a player must never be part of the template itself:
  pass it in a slot, escaped, as in [Safe Text](../strings/safe-text.md).

Next: pick a lesson from the tree on the [Learn](../README.md) page.
