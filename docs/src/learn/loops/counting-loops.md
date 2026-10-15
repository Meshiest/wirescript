# Counting Loops

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Lists](../values/lists.md)

**Next steps:** [Looping Over a List](looping-over-a-list.md)

</div>
<!-- /learn-links -->

A `for` loop runs the same steps many times in a row. This lesson draws a
progress bar on a sign, one cell per pass of the loop, and every pass finishes
within the same tick.

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
button Held -> chip add
chip bar -> decal Text
```

## Code

```wirescript name=lp1-bar
const WIDTH = 10

@left in add: exec
var filled: int = 0
var drawn: string = "----------"
@right out bar = drawn.Value

on add {
  filled = (filled + 1) % (WIDTH + 1)
  var cells: string = ""
  for i in 0..WIDTH {
    if i < filled {
      cells = "${cells}#"
    } else {
      cells = "${cells}-"
    }
  }
  drawn = cells
}
```

- `for i in 0..WIDTH { ... }` runs its block once for each number from 0 up to
  `WIDTH`, leaving `WIDTH` out: 0, 1, 2 and so on up to 9. Each pass is
  called an iteration, and `i` holds the number for that pass.
- `cells` starts empty, and each iteration adds one character to the end of
  it: `#` while `i` is below `filled`, `-` after.
- When the loop is done, the handler carries on with the next line, and
  `drawn = cells` puts the finished bar on the sign.
- `% (WIDTH + 1)` wraps `filled` back to 0 after a full bar.

Press the button three times. What does the sign show? What does the
eleventh press show?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with
   a **Text Decal**.
2. Wire the Button's **Held** to `add`, and `bar` to the Text Decal's
   **Text**.
3. Press the button. Each press adds one `#`, and the press after a full bar
   starts again from all dashes.

## Under the Hood

Open the chip. The loop is a microchip of its own, a **For Loop**, with
**Count** set to 10, **First Index** to 0 and **Step** to 1. Its **Exec**
input comes from the step before it in the handler, and its **Completed**
output carries on to `drawn = cells`.

Open the For Loop. Inside is a second plane with a **For Loop Entry** gate.
Its **Body** output starts each iteration and its **Index** output is `i`.
After it come a **Get Variable** for `filled`, a **Less** comparing `i` with
it, a **Branch**, and on each side a **Get Variable**, a **Format String** and
a **Set Variable** that add one character to `cells`.

The game runs this plane once per iteration, ten times in a row, all within
the tick the button was pressed. The gates are placed once. Change `WIDTH`
to 40 and the chip has as many gates as before. Only the Count changes.

`filled` and `cells` live outside the loop, and reach into it on wires
through the For Loop's side, as wires reach into any chip. A variable outside
the loop keeps its value from one iteration to the next, which is how `cells`
grows. A `var` declared inside the loop's block would start again from its
first value on every iteration.

## Modify

1. Fill the bar from the right. Change the loop to
   `for i in (0..WIDTH).rev()`. `.rev()` runs the same numbers backward, 9
   down to 0, so the `#`s are added last and end up on the right.
2. Make a bar half as long. `(0..WIDTH).step(2)` counts in twos: 0, 2, 4, 6
   and 8, five iterations instead of ten. Each cell now stands for two
   presses.
3. Count in any step size. `count(n, from = ..., step = ...)` is another way
   to write a range: `n` iterations, starting at `from`, going up by `step`.
   Here each press picks a new step size, from 1 to 9:

   ```wirescript name=lp1-count-by
   @left in add: exec
   var size: int = 0
   var drawn: string = ""
   @right out text = drawn.Value

   on add {
     size = size % 9 + 1
     var row: string = ""
     for i in count(5, from = size, step = size) {
       row = "${row} ${i}"
     }
     drawn = "Counting by ${size}:${row}"
   }
   ```

   The second press shows `Counting by 2: 2 4 6 8 10`. The start and the step
   come from a variable here, so they reach the For Loop on wires instead of
   being typed in.

## Make

Make a fuel gauge. One button adds fuel, another uses it, and a Clock burns a
little every second while a switch is on. The sign shows a bar of 20 cells
and the amount after it, like `[########------------] 40%`.

```wiring
button1 Held -> chip add
button2 Held -> chip spend
switch Enabled -> chip running
chip bar -> decal Text
```

## Good to Know

- A `for` loop finishes within one tick, however many iterations it has.
  Sixty iterations take one tick, not one second.
- A loop with a large Count does not cost more gates, but every iteration
  runs every gate in the plane again. Work that is the same each time belongs
  above the loop, not inside it.
- The game stops a loop that runs too many times in one tick (about 100,000
  iterations across all loops). A loop that is cut off stops partway.
- `for` is a step, so it lives in a handler. Written at the top of the file,
  it does not compile.

See [`for` Loops](../../statements.md#for-loops) in the reference.

Next: [Looping Over a List](looping-over-a-list.md).
