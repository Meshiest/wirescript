# Functions

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Chips](chips.md)

**Next steps:** [Imports](imports.md)

</div>
<!-- /learn-links -->

A `fn` is written like a mod, but it is built only once. Every call runs that
one copy, so calling it from many places costs one small gate per call
instead of a whole copy of the body.

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
button1 Held -> chip a
button2 Held -> chip b
button3 Held -> chip c
chip board -> decal Text
```

## Code

```wirescript name=g3-medals
@left in a: exec
@left in b: exec
@left in c: exec
var hitsA: int = 0
var hitsB: int = 0
var hitsC: int = 0
var signA: string = ""
var signB: string = ""
var signC: string = ""

fn medal(hits: int) -> string {
  if hits >= 10 { return "gold" }
  if hits >= 5 { return "silver" }
  return "bronze"
}

on a {
  hitsA += 1
  signA = "A: ${hitsA} hits, ${medal(hitsA)}"
}
on b {
  hitsB += 1
  signB = "B: ${hitsB} hits, ${medal(hitsB)}"
}
on c {
  hitsC += 1
  signC = "C: ${hitsC} hits, ${medal(hitsC)}"
}
@right out board = "${signA.Value}<br>${signB.Value}<br>${signC.Value}"
```

Three targets each count their hits, and the sign gives each one a medal.
`medal` is written with `fn` instead of `mod`, and it is called in three
places. If it were a mod, there would be three copies of its body. How many
are there now?

## Run

1. Compile the chip and paste it in-game. Place three **Button**s as
   targets and a **Text Decal**.
2. Wire the Buttons' **Held** to `a`, `b` and `c`, and `board` to the Text
   Decal's **Text**.
3. Press one target five times and its line changes from bronze to silver.

## Under the Hood

Open the chip. Among the handlers' gates are three **Function Call** gates,
one per call, and one extra board, a **Function Definition** microchip. Open
it. The body of `medal` is in there once:

- a **Function Entry**, where each call starts
- a **Function Input** for `hits` and a **Function Output** for the result
- a **Var** that holds the answer, a **Set Variable** for each `return`
  and a **Get Variable** to read it back
- the two **Greater/Equal** tests, two **Branch** gates, and two **Union**s
  where the paths join

That is 14 gates in all.

Each Function Call is wired to the definition. When a handler reaches its
call, the call hands `hits` to the board, the body runs to its end, and the
handler carries on with the answer.

Count it up. As a `fn`, `medal` costs its 14 gates once, plus one Function
Call per call, so 17 for three calls. As a `mod` it has no entry, input or
output, so it is 11 gates, but copied into every call, so 33 for three
calls. With one call the mod is cheaper. From two calls on, the fn is.

A function's board has no wires coming in from outside. Everything it uses
arrives through the call. A top-level `var` still works inside a `fn` (the
compiler gives the board a handle to it), but a pin or a `let` does not:

```wirescript name=g3-reads-pin expect=WS113
@left in a: exec
@left in bonus: bool
var hitsA: int = 0

fn points(hits: int) -> int {
  if bonus { return hits * 2 }
  return hits
}

on a { hitsA = points(hitsA + 1) }
@right out board = hitsA.Value
```

```text
Error: [WS113] `fn points` reads `bonus` from outside its plane. A function reaches outer storage (a `var`, array or map) and constants, not a computed value: pass it as a parameter
```

As the message says, make it an input,
`fn points(hits: int, bonus: bool)`, and call `points(hitsA + 1, bonus)`.

A function also cannot give back a record or an enum (`WS114`). Use a mod
for those.

## Modify

1. Change `fn medal` to `mod medal` and compile again. It still works. Open
   the chip. There is no Function Definition, and there are three copies of
   the tests and branches, one in each handler.

2. A `fn` needs a pulse to run on. Inside a handler it uses the handler's.
   Outside one, give it a pulse with `exec =`. Here a Check button asks for
   the medal:

   ```wirescript name=g3-check
   @left in hit: exec
   @left in check: exec
   var hits: int = 0
   var sign: string = "Press check"

   fn medal(n: int) -> string {
     if n >= 10 { return "gold" }
     if n >= 5 { return "silver" }
     return "bronze"
   }

   on hit { hits += 1 }
   on medal(hits.Value, exec = check) -> (m) {
     sign = "${hits} hits: ${m}"
   }
   @right out board = sign.Value
   ```

   `on medal(..., exec = check) -> (m)` runs the call each time `check`
   turns on, then runs the block with the answer in `m`.

3. A function can change top-level variables. Keep the best score of all
   targets: add `var best: int = 0`, put `if hits > best { best = hits }` as
   the first line of `medal`, and add `Best: ${best.Value}` to the sign. Open
   the Function Definition. A new **Var** gate labeled `&best` is the
   handle that reaches `best`.

## Make

Make a sign that shows time as text such as `2 min 5 s`. Write a `fn` that
turns a number of seconds into that text. Give it three Buttons that add 10,
30 and 60 seconds to a total, and one that clears it, and show the total
through your function. Between two `int`s, `/` divides and drops the
remainder, and `%` gives that remainder: `125 / 60` is 2 and `125 % 60` is 5.

```wiring
button1 Held -> chip add10
button2 Held -> chip add30
button3 Held -> chip add60
button4 Held -> chip clear
chip sign -> decal Text
```

## Good to Know

- A `fn` call runs the whole body before the handler's next step, within
  the same tick.
- A variable declared inside a `fn` is shared by every call and starts
  again from its first value at each call. One call never sees what another
  left there.
- In a chip that is spawned as a prefab (a later branch), a `fn` cannot reach
  top-level variables. Pass values in and give them back.

See also: [`fn`: Function](../../statements.md#fn-function) and
[When to use `fn`, and its limits](../../best-practices.md#when-to-use-fn-and-its-limits).

Next: [Imports](imports.md).
