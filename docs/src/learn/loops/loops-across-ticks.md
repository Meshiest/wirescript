# Loops Across Ticks

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Index and Value](index-and-value.md), [Waiting](../events/waiting.md)

</div>
<!-- /learn-links -->

A `for` loop finishes inside one tick, so it cannot wait. To repeat a step
with a pause between each, you build the loop from `await` and a buffered
`emit` instead. This lesson fades a lamp up like a sunrise, one small step
every quarter of a second.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [The loop's result](#the-loops-result)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip pressed
chip lamp -> light Color
chip lit -> light Enabled
chip text -> decal Text
```

## Code

```wirescript name=lp5-sunrise
const STEPS = 20

@left in pressed: exec
var step: int = 0
var running: bool = false
let t = (step.Value as float) / (STEPS as float)
@right out lamp = Color(1.0, 0.8, 0.5) * t
@right out lit = step.Value > 0
@right out text = "${step.Value * 5}%"

on pressed {
  if running {
    return
  }
  running = true
  step = 0
  let ramp: exec
  buffer emit ramp
  await ramp
  if step < STEPS {
    step += 1
    buffer(0.25s) emit ramp
  } else {
    running = false
  }
}
```

- `let ramp: exec` is a local signal, and `await ramp` waits for it, as in
  [Waiting](../events/waiting.md).
- `buffer emit ramp` fires `ramp` one tick later. By then the handler has
  reached `await ramp` and is waiting, so it wakes up.
- `buffer(0.25s) emit ramp` fires `ramp` again a quarter of a second later.
  The handler goes back to the `await` and runs the `if` again. That jump back
  is the loop.
- When `step` reaches `STEPS`, the `else` runs instead, nothing fires `ramp`
  again, and the loop ends.
- `running` stops a second press from starting a second loop on top of the
  first. `return` ends the handler early, so a press during the sunrise
  does nothing.
- `t` goes from 0 to 1 as `step` goes from 0 to 20, and the lamp's color is
  the warm white times `t`. `lit` turns the light on from the first step.

How long does the sunrise take? What happens if you press the button again
halfway through?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a **Point
   Light** and a brick with a **Text Decal**. Turn off the light's **Use
   Brick Color**. While it is on, the light ignores the wired color.
2. Wire the Button's **Held** to `pressed`, `lamp` to the Point Light's
   **Color**, `lit` to its **Enabled**, and `text` to the Text Decal's **Text**.
3. Press the button. The lamp warms up over five seconds and the sign counts
   to 100%. Presses during the sunrise are ignored.

## Under the Hood

Open the chip. `buffer emit ramp` is a **Buffer (Ticks)** gate set to one
tick, and `buffer(0.25s) emit ramp` is a **Buffer (Seconds)** set to 0.25.
Both feed a **Union**, which merges the two into one exec. The `await` is a
hidden **Var** that remembers it is waiting, read by a **Get Variable**
and a **Branch**. Follow the exec from the Buffer (Seconds) through the Union
and the `if`, and you are back at the Buffer (Seconds). It is a loop of wires
with a buffer in it, like the latch in [Buffers](../state/buffers.md).

A `for` loop cannot do this. Its iterations all run in one tick, and the
compiler refuses an `await` inside one:

```wirescript name=lp5-for-await expect=WS079
const STEPS = 20

@left in pressed: exec
var step: int = 0

on pressed {
  for i in 0..STEPS {
    step = i
    await Sleep(_, delay = 0.25)
  }
}
```

```text
Error: [WS079] a loop body runs synchronously within one tick, so it cannot suspend. Move the `await` outside the loop, or use a back-edge loop (`emit` + `await`) if you need work spread across ticks.
```

The emit that starts the loop must be buffered as well. A plain `emit ramp`
fires before the `await` on the next line is waiting, so nothing hears it:

```wirescript name=lp5-plain-start expect=WS053
const STEPS = 20

@left in pressed: exec
var step: int = 0
@right out text = "${step.Value * 5}%"

on pressed {
  step = 0
  let ramp: exec
  emit ramp
  await ramp
  if step < STEPS {
    step += 1
    buffer(0.25s) emit ramp
  }
}
```

```text
WARN [WS053] `emit ramp` fires in this tick, before the following `await ramp` arms - so the await never resumes from it and the chain parks forever. Use `buffer emit ramp`, which lands the next tick, after the await is armed
```

## The loop's result

A `for` loop has a result too. `let run = for ...` names it, and after the
loop `run.iterations` says how many iterations ran:

```wirescript name=lp5-for-result
@left in roll: exec
var rolls: int[]
var shown: string = "No rolls yet"
@right out text = shown.Value

on roll {
  rolls.push(Random(1, 6))
  var total: int = 0
  let run = for r in rolls {
    total += r
  }
  if run.aborted {
    shown = "Cut short after ${run.iterations} rolls"
  } else {
    shown = "${run.iterations} rolls, total ${total}"
  }
}
```

`run.aborted` is true when the game cut the loop off, for running too many
iterations in one tick. A `break` does not count as cut off. These are the
loop chip's **Iterations Executed** and **Aborted** outputs.

## Modify

1. Make it a sunset too. Add a second button and a second handler that runs
   the same loop downward: `step` starts at `STEPS` and goes down by one each
   pass while it is above 0. Give it its own signal, and share `running`, so
   neither starts while the other runs.
2. Change the pace. `buffer(0.1s)` is faster. `buffer(3) emit ramp` waits
   three ticks instead of a time in seconds.
3. Make the steps visible. Instead of a color, show a bar on the sign, built
   with a `for` loop as in [Counting Loops](counting-loops.md), redrawn on
   every pass of the slow loop. The fast loop runs inside the slow one.

## Make

Make a light show. A `const` array holds six colors. A button plays them on a
Point Light, one every half second, then turns the light off. A second press
during the show does nothing.

```wiring
button Held -> chip play
chip lamp -> light Color
chip lit -> light Enabled
```

## Good to Know

- A loop can sit inside an `if` (for example `if !running { ... }` around
  the whole loop) when its signal, `let ramp: exec`, is declared inside that
  same `if`. If the signal is declared outside it, the loop would run once,
  and the compiler warns `WS118` at the `await`. The code uses `return`
  instead, which keeps the loop at the top of the handler.
- A `buffer emit` ends its path through the handler. Code after the `if`
  that holds it runs only on the path that did not loop, which here is once,
  when the loop ends. A line after a `buffer emit` in the same block never runs, and
  the compiler warns `WS058` there.
- Keep loop counters like `step` outside the handler. They have to keep their
  value from one pass to the next.
- Use a `for` loop for work that must finish now, and a buffered loop for
  work that should take time.

See [Loops](../../statements.md#loops) under `emit` and
[Loop result](../../statements.md#loop-result-let-run--for) in the reference.

Next: [Dice](../random/dice.md).
