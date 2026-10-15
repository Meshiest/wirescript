# Counting the Cost

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Imports](imports.md)

**Next steps:** [Const and Folding](../mastery/const-and-folding.md)

</div>
<!-- /learn-links -->

Every gate is a brick that has to be pasted, loaded and run. This lesson
counts them: how a mod, a chip and a function multiply, how to read the
count from the compiler, and the second kind of cost that a gate count does
not show.

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

```wirescript name=g5-medal-board
@left in a: exec
@left in b: exec
@left in c: exec
var hitsA: int = 0
var hitsB: int = 0
var hitsC: int = 0

mod medal(hits: int) -> string {
  return if hits >= 10 then "gold" else if hits >= 5 then "silver" else "bronze"
}

on a { hitsA += 1 }
on b { hitsB += 1 }
on c { hitsC += 1 }
@right out board = "A ${medal(hitsA.Value)}<br>B ${medal(hitsB.Value)}<br>C ${medal(hitsC.Value)}"
```

This is the medal board from [Functions](functions.md), with `medal` as a
one-line mod and the medals worked out straight into the sign. `medal` has
two tests and two choices. How many gates does it cost in all? How many
gates does each extra target add?

## Run

1. Compile the chip and paste it in-game. Place three **Button**s and a
   **Text Decal**.
2. Wire the Buttons' **Held** to `a`, `b` and `c`, and `board` to the Text
   Decal's **Text**.
3. Press a target five times and its medal turns to silver.

## Under the Hood

Open the chip and count. Each call to `medal` is two **Greater/Equal** and
two **Select** gates, so 4 gates a call and 12 for three calls. Each target also has its
pin, its **Var** and its **Increment Variable**, so one more target adds 7
gates.

So the cost model is:

- A `mod` or `chip` costs its body times the number of calls. A chip adds a
  microchip brick and its pins on top.
- A `fn` costs its body once, plus one **Function Call** per call.

Counting bricks in-game is slow. If you build the compiler from source,
`just ir board.ws` prints the compiled chip as text (the same as
`wirescript compile board.ws --dump-ir`). Each gate is one line:

```text
  %18 = Expr_CompareGreaterOrEqual(InputA: %hitsA.Value, InputB: 10)
  %21 = Expr_CompareGreaterOrEqual(InputA: %hitsA.Value, InputB: 5)
  %24 = Expr_Select(InputA: "bronze", InputB: "silver", bSelectB: %21)
  %25 = Expr_Select(InputA: %24, InputB: "gold", bSelectB: %18)
```

(The real lines are longer. They also list the gate's input and output
types, and the line and column of the code each gate came from.) Lines that
start `%name = ` are gates. Among them, `in` lines are pins and `chip` lines
are microchips. This chip has 22. To count them:

```text
just ir board.ws > board.wsir
grep -cE '^\s*%\S+ = ' board.wsir
```

In the playground or VS Code, hovering over a mod's name shows a quick
estimate. It is worked out before the compiler simplifies anything, so the
real count can differ.

### Steps in a row

A gate count says how much circuit there is. It does not say how much runs
when a pulse arrives. A handler is a chain of steps, each waiting for the
one before it, all within one tick. A `fn` call is one gate on that chain,
but the pulse goes through every step of the body before the chain carries
on. A `for` loop (in the Loops branch) is one body run many times over: few
gates, many steps. `@sliced` on a loop lets the game spread its passes over
several ticks.

Two handlers on the same pin are two chains. Neither waits for the other,
and there is no saying which runs first, so split work this way only when
the two halves do not depend on each other.

## Modify

1. Write `medal` with early returns. A mod that returns from inside an `if`
   needs a pulse, so the medals are now worked out in the handlers and kept
   in variables:

   ```wirescript name=g5-early-return
   @left in a: exec
   @left in b: exec
   @left in c: exec
   var hitsA: int = 0
   var hitsB: int = 0
   var hitsC: int = 0
   var medalA: string = ""
   var medalB: string = ""
   var medalC: string = ""

   mod medal(hits: int) -> string {
     if hits >= 10 { return "gold" }
     if hits >= 5 { return "silver" }
     return "bronze"
   }

   on a {
     hitsA += 1
     medalA = medal(hitsA)
   }
   on b {
     hitsB += 1
     medalB = medal(hitsB)
   }
   on c {
     hitsC += 1
     medalC = medal(hitsC)
   }
   @right out board = "A ${medalA.Value}<br>B ${medalB.Value}<br>C ${medalC.Value}"
   ```

   The chip grows from 22 gates to 52. Each early return sets a hidden
   variable, and the branches join again with **Union** gates. `medal` is now
   11 gates a call instead of 4.

2. Change `mod medal` to `fn medal`. The count drops to 37, because the body is
   built once, and each call is one Function Call.

3. Keep the `fn`, and write its body as the one-line `if ... then ... else`
   again, and the count is 30. Then try ten targets. The four versions come to:

   | How `medal` is written | 3 targets | 10 targets |
   |---|---|---|
   | `mod`, one expression, in the sign | 22 | 73 |
   | `mod`, early returns, in handlers | 52 | 173 |
   | `fn`, early returns, in handlers | 37 | 88 |
   | `fn`, one expression, in handlers | 30 | 81 |

   (Ten targets is 2 more than 7 a target suggests, because a sign with ten
   values needs three **Format String** gates instead of one.)

   The one-line mod wins at every size here. Its body is 4 gates, and a
   call costs nearly as much as that: the Function Call, plus a variable to
   keep each answer in and the gates that store and read it.

## Make

Take a circuit you built in an earlier lesson that calls a mod more than
once. Count its gates, then try it with the mod written as one expression,
as an early-return mod, and as a `fn`. Which is smallest, and why?

## Good to Know

- Nothing warns about size. A mod called from inside another mod is copied
  once per call of the outer one too, so the counts multiply.
- Every pasted chip is a separate copy. Ten chips pasted from one save cost
  ten times its gates.
- A `fn` saves gates only when its body is bigger than the cost of calling
  it. For a few gates of body, a one-line `mod` is smaller.

See also: [Best Practices: Gate Count & Scaling](../../best-practices.md),
[Profiling](../../best-practices.md#profiling-find-the-hot-spots) and
[Sliced loops](../../statements.md#sliced-loops-sliced).

Next: pick a lesson from the tree on the [Learn](../README.md) page.
