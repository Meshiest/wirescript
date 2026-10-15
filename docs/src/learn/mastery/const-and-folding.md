# `const` and Folding

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Testing](testing.md), [Counting the Cost](../abstraction/cost.md)

**Next steps:** [Performance](performance.md)

</div>
<!-- /learn-links -->

The compiler works out every value it can before the game ever sees the
chip, and types the answers into the gates. This is called folding. `const`
asks for it on purpose: it names a value that must be known while compiling,
and refuses to compile when it is not.

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
button Held -> chip fill
chip sign -> decal Text
```

## Code

```wirescript name=ma2-gauge
const WIDTH = 10
const MAX = 200
const STEP = MAX / 4
const PER_CELL = MAX / WIDTH
const TITLE = "Fuel, max ${MAX}"

@left in fill: exec
var fuel: int = 0
var bar: string = "----------"
@right out sign = "${TITLE}<br>[${bar.Value}] ${fuel.Value}"

on fill {
  fuel = if fuel >= MAX then 0 else fuel + STEP
  var text: string = ""
  for i in 0..WIDTH {
    text = text .. (if i < fuel / PER_CELL then "#" else "-")
  }
  bar = text
}
```

A fuel gauge: each press adds a quarter of a tank, and a full tank empties
on the next press. The settings sit at the top as `const`s, so changing the
tank or the bar is one edit. `text .. x` joins two pieces of text into one.

`const` is written like `let`, but its value must be worked out while
compiling. Two of the five are worked out from others: `STEP` is 50 and
`PER_CELL` is 20.

How many gates do the five `const` lines cost? Which numbers in the
handler does the game work out, and which does the compiler?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick
   with a **Text Decal**.
2. Wire the Button's **Held** to `fill`, and `sign` to the Text Decal's
   **Text**.
3. Press four times: the bar fills two or three cells at a time, to
   `[##########] 200`. Press once more and it is empty.

## Under the Hood

Open the chip and click through the gates. None of them belongs to a
`const` line. Their values are typed into the gates that use them:

- The sign's **Format String** has `Fuel, max 200` typed into its first
  input. The title was built while compiling.
- The **Add** adds 50 and the **Greater/Equal** compares with 200.
- The **For Loop** has **Count** set to 10, and the **Divide** inside it
  divides by 20.

The game only works out what depends on `fuel`, which changes as it runs.

### How to tell

Count the gates, as in [Counting the Cost](../abstraction/cost.md):
`just ir gauge.ws` prints 24 gate lines. Then turn folding off for the
whole file: put `@nofold` on the first line, with a blank line after it.

```text
@nofold

const WIDTH = 10
...
```

Now it prints 29: two **Divide**s that work out `STEP` and `PER_CELL`, a
Format String that builds `TITLE`, a pin that carries `PER_CELL` into the
loop, and a `_Literal` line for `MAX`. A `_Literal` is a constant the
compiler types into the gates it feeds, not a gate of its own. Both
versions do exactly the same thing. Folding only changes how much is built.

Counting with and without `@nofold` is the way to find out what folded. A
line you expected to fold that still shows up as a gate is reading
something only the game knows.

### `const` and `let`

Change every `const` to `let`: still 24. A `let` folds too, whenever it
can. The difference is what happens when it cannot. Say the tank size comes
from a pin:

```wirescript name=ma2-runtime-max expect=WS046
@left in fill: exec
@left in cap: int
const MAX = cap
var fuel: int = 0
@right out sign = "${fuel.Value}"

on fill {
  fuel = if fuel >= MAX then 0 else fuel + 50
}
```

```text
Error: [WS046] not a compile-time constant: 'cap' is a runtime value
```

With `let MAX = cap` it compiles, and the game works out the compare and
the add as it runs. With `const`, if a value you meant to be fixed turns
into a wire, the compiler reports it at the line that caused it.

### What cannot fold

Folding needs every input known while compiling. These never are:

- Anything read from a `var`, an array or a map, even one that never
  changes.
- Pins, events, `Random(...)` and `ReadBrickGrid()`.
- A gate whose answers were never recorded from the game. The compiler only
  folds a gate when the game's own answer for that kind of input has been
  measured, so a folded chip gives exactly what the gates would.

`const` also refuses `min`, `max`, `abs` and `clamp`
(`WS046`, "`max` is not declared `const mod`"). A `let` folds them when
their inputs are constant.

## Modify

1. Switch a debug readout on and off with a `const`. Add
   `const DEBUG = false` and `var presses: int = 0`. Put
   `if DEBUG { presses += 1 }` as the handler's first line, and end the sign's text with
   `${if DEBUG then "<br>presses: ${presses.Value}" else ""}`. Count it: 25
   gates. Set `DEBUG = true`: 27. With `false`, the `if` and everything in
   it are deleted while compiling, so a feature behind a false `const`
   costs nothing but its `var`: the `presses` Var stays.

2. A `const` parameter. `SendCustomEvent` sends a message under a name to
   your other chips ([Custom Events](../events/custom-events.md)). The name
   is a setting typed into the gate, so it must be known while compiling. A
   mod that takes the name as a plain `string` does not compile:

   ```wirescript name=ma2-report-plain expect=WS028
   @left in fill: exec
   var fuel: int = 0

   mod report(chan: string, level: int) {
     SendCustomEvent(chan, level)
   }

   on fill {
     fuel += 50
     report("fuel.level", fuel)
   }
   ```

   ```text
   Error: [WS028] 'eventName' is constant-only gate config and cannot take a variable or computed value
   ```

   Inside the mod, `chan` is a parameter, and a parameter could be anything.
   Mark it `chan: const string`. Then every call must pass a value known
   while compiling, the name is typed into each call's **Send Personal
   Custom Event** gate, and `level` stays an ordinary wired input. A call
   that passes a `var` as the name is `WS046`.

3. Name the levels with a `const` list. Add
   `const LEVELS = ["empty", "low", "half", "high", "full"]`, a
   `var level: string = "empty"`, and in the handler, after `fuel` changes,
   `level = LEVELS[fuel / STEP]`. Show `${level.Value}` on the sign instead
   of the number. The list becomes one **Array Var** with the five words
   typed in, read with **Get Array Element**: the index depends on `fuel`,
   so the read happens in game. A read at a fixed index, such as
   `LEVELS[4]`, folds to `"full"`.

## Make

Make a countdown sign whose start time, step and messages are all `const`s
at the top. Add a `DEBUG` const that shows how many times it has been
reset. Count the gates with `DEBUG` true and false, and with `@nofold`, and
explain each difference from the code.

```wiring
button Held -> chip reset
chip sign -> decal Text
```

## Good to Know

- A folded value prints the way the gate would print it. A folded
  `${1000}` is `1,000`, as the Format String gate writes it.
- `/` between two `int`s drops the remainder, folded or not: `MAX / 3` with
  `MAX = 200` is 66.
- Event names, prefabs and sounds are settings typed into the gate. They
  take only a value known while compiling (`WS028`).
- A `var` is never removed, even when every line that used it was folded
  away.

See also: [`const`](../../statements.md#const-compile-time-binding),
[`const` Parameters and `const mod`](../../chips.md#const-parameters-and-const-mod)
and [Constant Folding](../../folding.md) in the reference.

Next: [Performance](performance.md).
