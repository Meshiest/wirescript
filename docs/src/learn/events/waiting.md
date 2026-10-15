# Waiting

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Custom Events](custom-events.md)

**Next steps:** [Forking and Merging](forking-and-merging.md), [Loops Across Ticks](../loops/loops-across-ticks.md), [Ready Check](../input/ready-check.md)

**Parts that use this:** [Proximity Door](../parts/proximity-door.md), [Pressure Pad Door](../parts/pressure-pad-door.md), [Hold to Use](../parts/hold-to-use.md), [Moving Platform](../parts/moving-platform.md), [Keycard Door](../parts/keycard-door.md), [Whack-a-Light](../parts/arcade-game.md)

</div>
<!-- /learn-links -->

A handler so far ran from top to bottom in one go. `await` lets it stop
partway, wait for something to happen, and carry on from there. This lesson
builds a reaction timer, then a guessing game that waits for one of two
buttons.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Racing two waits](#racing-two-waits)
- [Signals of your own](#signals-of-your-own)
- [Calling from outside a handler](#calling-from-outside-a-handler)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button1 Held -> chip start
button2 Held -> chip hit
chip light -> light Enabled
chip sign -> decal Text
```

## Code

```wirescript name=e3-reaction
@left in start: exec
@left in hit: exec
var lit: bool = false
var began: float = 0.0
var shown: string = "Press start"
@right out light = lit.Value
@right out sign = shown.Value

on start {
  shown = "Wait for the light..."
  await Sleep(_, delay = 2.0)
  lit = true
  began = ServerUptime()
  await hit
  lit = false
  shown = "${ServerUptime() - began} seconds"
}
```

The handler is one sequence with two waits in it. `await Sleep(...)` waits
two seconds. `await hit` waits for the `hit` button to be pressed.
`ServerUptime()` is the number of seconds the server has been running, so
storing it when the light comes on and subtracting it after the press gives
the reaction time.

What happens if you press `hit` while the sign still says "Wait for the
light..."?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s, a **Point
   Light** and a brick with a **Text Decal**.
2. Wire one Button's **Held** to `start` and the other's to `hit`. Wire
   `light` to the Point Light's **Enabled** and `sign` to the Text Decal's
   **Text**.
3. Press start. Two seconds later the light comes on. Press hit as fast as
   you can. The light goes off and the sign shows your time.
4. Press hit early, during the wait. Nothing happens, and the timer still
   waits for a press after the light comes on.

## Under the Hood

Open the chip. Each `await` became the same handful of gates:

- a **Var** that holds the await's armed flag,
- a **Set Variable** that turns the flag on when the chain reaches the
  `await`,
- a **Get Variable** and a **Branch** that let the waited-for pulse through
  only while the flag is on,
- a **Set Variable** that turns the flag off again.

That is about five gates per `await`. The Sleep adds one **Buffer
(Seconds)**. Its input is the armed flag (that is what `_` stands for), and
its output turns on two seconds after the flag does.

The flag is why an early press does nothing. `hit` is wired into the second
await's Get Variable all the time, but until the chain gets there, the flag
is off and the Branch drops the pulse.

## Racing two waits

Nobody pressing `hit` leaves the timer waiting forever. `||` between two
waits resumes on whichever comes first:

```wirescript name=e3-race
@left in start: exec
@left in hit: bool
var shown: string = "Press start"
@right out sign = shown.Value

on start {
  shown = "Press hit within 3 seconds"
  await hit || Sleep(_, delay = 3.0)
  if hit {
    shown = "In time"
  } else {
    shown = "Too slow"
  }
}
```

The two sides go into an **OR** gate, and the await resumes when the OR
turns on. The race does not say which side won. A press normally still holds
`hit` on when the chain resumes, so `if hit` tells the two apart. That
reads the pin's value, so `hit` is a `bool` here, not an `exec`.

## Signals of your own

`let answer: exec` makes a signal, an exec with a name, which any handler can
fire with `emit` and any handler can wait for with `await`. It can carry a
value along:

```wirescript name=e3-guess
@left in ask: exec
@left in left: exec
@left in right: exec
var shown: string = "Press ask"
@right out sign = shown.Value

let answer: exec

on left {
  emit answer = 1
}

on right {
  emit answer = 2
}

on ask {
  let secret = Random(1, 2)
  shown = "Left or right?"
  let picked = await answer
  if picked == secret {
    shown = "Right guess!"
  } else {
    shown = "Wrong guess"
  }
}
```

`emit answer = 1` fires the signal and stores `1` with it. `let picked =
await answer` waits for the signal and reads the value that came with it. The
value is kept in a hidden variable, so it is still there when the waiting
handler picks it up.

Wire three Buttons to `ask`, `left` and `right`. Pressing left or right
before ask does nothing, for the same reason the early `hit` did nothing.

## Calling from outside a handler

Some calls are steps, like `Random`, and need a pulse to run. Inside a
handler the pulse comes from the chain. Outside one, give it with `exec =`:

```wirescript name=e3-exec-arg
@left in roll: exec
let r = Random(1, 6, exec = roll)
@right out face = r
```

`exec = roll` wires `roll` into the **Random** gate's exec input. There is no
handler and no Set Variable, just one gate. The same works for array and map
steps.

## Modify

1. Make the wait before the light random: store `Random(1, 4)` in a
   `var delay: float` first, then `await Sleep(_, delay = delay)`.
2. Give the reaction timer the race: wait for `hit || Sleep(_, delay = 3.0)`
   and show "Too slow" when the light was missed. The race reads `hit` as a
   value, so make it `@left in hit: bool`, as in the race above. Then catch a
   cheater: check `hit` right before the light comes on, and show "Too
   early!" if it is already held.
3. In the guessing game, add a third button and a third answer. Then add a
   timeout: a second `on ask` handler that waits five seconds and does
   `emit answer = 0`. Show "Out of time" when `picked` is 0. (A race with
   `||` cannot carry a value, so the timeout sends one of its own.)

## Make

Make a "Simon says" for one button: the light blinks one to three times
(chosen with `Random`), then the player presses the button that many times
within four seconds. Use a signal that the button handler fires, and count
the presses in a `var`.

```wiring
button1 Held -> chip start
button2 Held -> chip press
chip light -> light Enabled
```

## Good to Know

- A pulse that arrives before its `await` is reached is dropped, not saved
  for later.
- `await` waits for a pulse. With a Button's **Held** wired to the pin, that
  is the moment Held turns on, so holding the button down while the await is
  reached does not count as a press.
- `Sleep` measures seconds and `SleepTicks` measures ticks. `SleepTicks` with
  `delay = 0` resumes on the next tick, and `delay = N` after N + 1 ticks.
- A value stored with `ServerUptime()` is only good while the server keeps
  running. A restart sets the uptime back to zero.

See [Await](../../exec-context.md#await),
[Payload Ferrying](../../statements.md#payload-ferrying) and
[Explicit Exec Argument](../../exec-context.md#4-explicit-exec-argument) in the
reference.

Next: [Forking and Merging](forking-and-merging.md).
