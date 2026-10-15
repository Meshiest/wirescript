# Tuples

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Records in Storage](records-in-storage.md), [Mods](../abstraction/mods.md)

**Next steps:** [Destructuring](destructuring.md)

</div>
<!-- /learn-links -->

A `mod` can give back more than one value. Several values in a row, in
order, are a **tuple**. This lesson builds a mod with two outputs, takes them
apart with `let (lo, hi) = ...`, and passes a tuple on to another mod.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Passing a tuple on](#passing-a-tuple-on)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button1 Held -> chip pressA
button2 Held -> chip pressB
chip text -> decal Text
```

## Code

```wirescript name=d3-minmax
mod minmax(a: int, b: int) -> (lo: int, hi: int) {
  out lo = min(a, b)
  out hi = max(a, b)
}

@left in pressA: exec
@left in pressB: exec
var a: int = 0
var b: int = 0
let (lo, hi) = minmax(a.Value, b.Value)
@right out text = "${lo} to ${hi}, a gap of ${hi - lo}"

on pressA { a += 1 }
on pressB { b += 1 }
```

- `a` and `b` count the presses of two buttons.
- `-> (lo: int, hi: int)` gives the mod two outputs, each with a name and a
  type.
- `out lo = ...` and `out hi = ...` set them, one line each.
- `let (lo, hi) = minmax(a.Value, b.Value)` takes the two values apart and names them,
  in order, so the first output becomes `lo` and the second `hi`. You choose
  the names, and they do not have to match the mod's.

Press one button five times and the other twice. What does the sign say?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire the first Button's **Held** to `pressA`, the second's **Held** to
   `pressB`, and `text` to the Text Decal's **Text**.
3. Press the buttons. With five presses and two, the sign reads "2 to 5, a
   gap of 3". It does not matter which button has more.

## Under the Hood

Open the chip. Past the two counters (a **Var** and an **Increment Variable**
each), there are four gates: a **Min**, a **Max**, a **Subtract** for
`hi - lo`, and the Format String. The mod, the tuple and the `let` have no
gates of their own. A mod puts its body where it is called, and the tuple is
the two wires coming out of it.

The pattern on the left of `=` must have one name per value:

```wirescript name=d3-three-names expect=WS010
mod minmax(a: int, b: int) -> (lo: int, hi: int) {
  out lo = min(a, b)
  out hi = max(a, b)
}

@left in a: int
@left in b: int
let (lo, hi, mid) = minmax(a, b)
@right out gap = hi - lo
```

```text
Error: [WS010] destructure shape: this pattern binds 3 position(s), but the value has 2 value(s)
```

## Passing a tuple on

A tuple can also be kept whole and handed on:

```wirescript name=d3-spread
mod minmax(a: int, b: int) -> (lo: int, hi: int) {
  return min(a, b), max(a, b)
}

mod describe(lo: int, hi: int) -> string {
  return "${lo} to ${hi}"
}

@left in pressA: exec
@left in pressB: exec
var a: int = 0
var b: int = 0
let range = minmax(a.Value, b.Value)
@right out text = describe(...range)
@right out gap = range.hi - range.lo

on pressA { a += 1 }
on pressB { b += 1 }
```

- `return min(a, b), max(a, b)` sets both outputs on one line, in order.
- `let range = minmax(a.Value, b.Value)` keeps the tuple under one name. `range.hi` and
  `range.lo` read its values by the names the mod gave them.
- `describe(...range)` is a **spread**, which passes the tuple's values as the
  call's arguments, `lo` first, then `hi`.

Past the counters, the chip is again a Min, a Max, a Subtract and a Format
String.

## Modify

1. Add a third button and a mod `order3(a: int, b: int, c: int) -> (lo: int,
   mid: int, hi: int)`. The middle value is the total minus the smallest and
   the largest.
2. Make the sign say "a tie" when the gap is 0. Use an `if` expression on
   `hi - lo`.
3. Write your own tuple: `let pair = (a, b)` makes one from two values.
   Spread it into `minmax(...pair)`.

## Make

Make a chip for four counted buttons that shows the most presses and which
button has them. Write a mod that compares two buttons and returns both the
bigger count and the button's number, and call it three times.

```wiring
button1 Held -> chip pressA
button2 Held -> chip pressB
button3 Held -> chip pressC
button4 Held -> chip pressD
chip text -> decal Text
```

## Good to Know

- The sign is pure, so it follows the two counts by itself. Only the counting
  happens in handlers.
- Press only the first button five times, and the sign reads "0 to 5".

See [Tuple Destructuring](../../statements.md#tuple-destructuring) and
[Spread in Call Arguments](../../statements.md#spread-in-call-arguments).

Next: [Destructuring](destructuring.md).
