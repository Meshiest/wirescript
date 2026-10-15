# Converting Values

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Vectors and Colors](vectors-and-colors.md)

**Next steps:** [Lists](lists.md)

</div>
<!-- /learn-links -->

Values often need to change type: a whole number into a `float` so a divide
keeps its fraction, a `float` back into a whole number, a number into text.
Some conversions happen by themselves. For the rest, `as` asks for one.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Conversions that happen by themselves](#conversions-that-happen-by-themselves)
- [What `as` converts](#what-as-converts)
- [`any`](#any)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip roll
chip text -> decal Text
```

## Code

```wirescript name=v4-average
@left in roll: exec
var total: int = 0
var rolls: int = 0
var last: int = 0
let average = total.Value as float / rolls.Value
let nearest = round(average) as int
@right out text = "Rolled ${last.Value}. Average ${average}, about ${nearest}"

on roll {
  last = Random(1, 6)
  total += last
  rolls += 1
}
```

Each press rolls a die. `Random(1, 6)` gives a whole number from 1 to 6, and
runs as a step in the handler like a variable write. The handler adds the
roll to `total` and counts it in `rolls`.

`value as T` converts a value to type `T`. `total.Value as float` turns the
total into a `float` before the divide, so the average keeps its fraction.
`round(average)` rounds to the nearest whole number, but its answer is still a
`float`. `as int` makes it an `int`.

After rolling a 3 and then a 4, the average shows its fraction. What would it
show without `as float`?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with
   a **Text Decal**.
2. Wire the Button's **Held** to `roll`, and `text` to the Text Decal's
   **Text**.
3. Press a few times. The average moves toward 3.5 as the rolls add up, and
   `about` shows the nearest whole number.

## Under the Hood

Open the chip. Each `as` is one **Convert** gate: one before the Divide, one
after the Round. The text needs no Convert of its own: `${}` builds the text
with a Format String gate, which writes any value it is given.

## Conversions that happen by themselves

- **`bool` to a number.** `true` is 1 and `false` is 0, as in
  [True and False](true-and-false.md).
- **`int` and `float`.** Mixing them in arithmetic gives a `float`, as in
  [Numbers](numbers.md).
- **Anything into text** inside `${}`.
- **Text to `bool`**, where a bool is wanted: text is `true` when it is not
  empty. The compiler checks `!= ""` for you.

```wirescript name=v4-message
@left in pressed: exec
var message: string = ""
@right out lamp: bool = message.Value
@right out text = message.Value

on pressed {
  message = if message == "" then "Hello" else ""
}
```

Each press shows or clears a message, and the light is on while there is one.
The chip has a **Not Equal** gate between the message and `lamp`, comparing
it with empty text.

## What `as` converts

- `bool`, `int`, `float` and `string` into one another.
- A `float` to an `int` drops the fraction toward zero: `2.7 as int` is 2, and
  `-2.7 as int` is -2. Use `round` first to get the nearest instead.
- A number to a `bool` is `true` when it is not zero.
- Text to a number reads the number at its start: `"42abc" as int` is 42, and
  `"abc" as int` is 0.
- A `vector`, `rotator` or `color` into text: `X=1.000 Y=2.000 Z=3.000` for a
  vector.

A conversion the game cannot make, such as a vector into a number, would only
give the target type's empty value, so the compiler warns (`WS109`) instead.
The full list is in [Type Conversion](../../expressions.md#type-conversion-as).

## `any`

`any` is a type that stands for every type. A pin declared `@left in probe:
any` accepts whatever is wired to it, and `probe as string` shows it as text:

```wirescript name=v4-any-pin
@left in probe: any
@right out text = probe as string
```

`any` throws the type away, so the compiler can no longer check what you do
with the value. A variable can never hold `any`: its Variable gate needs one
real type to store.

```wirescript name=v4-any-var expect=WS025
var held: any = 0
```

```text
Error: [WS025] 'any' cannot be stored: a variable gate needs a concrete wire type (int/float/bool/string/vector/...)
```

Use a real type whenever you know it. The reference covers the rest in
[`any` Type](../../types.md#any-type).

## Modify

1. Remove `as float` from `average`. After a 3 and a 4 it shows 3: the divide
   is now between two `int`s and drops the remainder.
2. Count sixes. Add `var sixes: int = 0`, add one when `last == 6`, and show
   the share of sixes as a whole percent: `(sixes.Value as float / rolls.Value
   * 100.0) as int`.
3. Light a lamp while the average is above 3.5:
   `@right out lucky = average > 3.5`.

## Make

Make a fuel gauge. Each press of a button adds 7 liters to a 40-liter tank,
but the tank never goes past 40. A sign shows "21 of 40 liters (52%)", with
the percent as a whole number, and a light is on while the tank is at least
three quarters full. A second button empties it.

```wiring
button1 Held -> chip fill
button2 Held -> chip empty
chip text -> decal Text
chip full -> light Enabled
```

## Good to Know

- `round`, `floor`, `ceil` and `sqrt` give back a `float`, even when the
  answer is whole. Add `as int` when you need an `int`. `min`, `max`, `abs`
  and `clamp` give back the type they are given: an `int` stays an `int`.
- Text that does not start with a number converts to 0, with no error.

Next: [Lists](lists.md).
