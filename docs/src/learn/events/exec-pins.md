# Exec Pins

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Custom Events](custom-events.md)

**Parts that use this:** [Countdown Sign](../parts/countdown-sign.md), [Doorbell](../parts/doorbell.md), [Round Manager](../parts/round-manager.md), [King of the Hill](../parts/king-of-the-hill.md), [Voting](../parts/voting.md)

</div>
<!-- /learn-links -->

So far a chip's outputs have carried values. An output can also carry a
pulse: a moment when something happened, which another part can react to.
This lesson builds a doorbell that rings a real sound.

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
button Held -> chip pressed
chip sign -> decal Text
chip ring -> speaker Play
```

## Code

```wirescript name=e1-doorbell
@left in pressed: exec
var rings: int = 0
@right out ring: exec
@right out sign = "Rings: ${rings.Value}"

on pressed {
  rings += 1
  emit ring
}
```

`@right out ring: exec` is an exec output, a pin that carries pulses instead
of a value. `emit ring` sends a pulse out of it. It fires at the point the handler
has reached, so here it fires after `rings` has gone up.

`sign` is an ordinary value output, the same as in earlier lessons.

`ring` has no value. What can a wire from it go into? What happens if you
move `emit ring` above `rings += 1`?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a brick with a
   **Text Decal**, and a **One Shot Audio** brick.
2. Open the One Shot Audio's settings and pick a sound for it.
3. Wire the Button's **Held** to `pressed`, `sign` to the Text Decal's
   **Text**, and `ring` to the One Shot Audio's **Play**.
4. Press the button. The sound plays once per press, and the sign counts the
   rings. Holding the button does not ring again.

## Under the Hood

Open the chip. `rings` is a **Var** gate and `sign` comes from a
**Format String** gate. The handler is a single **Increment Variable** gate:
`rings += 1` adds one in place. Its exec input is wired from `pressed`.

There is no gate for `emit ring`. The `ring` pin is wired straight from the
Increment Variable's exec output, so the pulse leaves the chip the moment the
count has gone up. That is why the order of the lines matters. Move
`emit ring` first and the pin is wired from `pressed` itself, beside the
Increment Variable instead of after it.

`emit` can also set a value pin. This dice chip sets its number and then
fires a pulse:

```wirescript name=e1-dice
@left in roll: exec
@right out face: int
@right out rolled: exec

on roll {
  emit face = Random(1, 6)
  emit rolled
}
```

`emit face = ...` makes the **Random** gate drive `face`, and `emit rolled`
fires after the roll. Whatever listens to `rolled` sees the new `face`. Set
the value first, then fire the pulse.

A pin has one driver. An `out` with a value already drives its pin all the
time, so a handler cannot also write it:

```wirescript name=e1-two-drivers expect=WS013
@left in pressed: exec
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

on pressed {
  rings += 1
  emit sign = "Ding!"
}
```

```text
Error: [WS013] this `out sign = ...` drives `sign` continuously, and an `emit sign = ...` also writes it, so the two would fight over the output. Give this one a constant default (`out sign = 0`) and let the other write change it, or remove it
```

Drive each output one way, either with a value that is always computed or
with writes from handlers.

## Modify

1. Ring only on every other press. Wrap the emit:
   `if rings % 2 == 0 { emit ring }`.
2. Ring twice per press, half a second apart:

   ```wirescript name=e1-ding-dong
   @left in pressed: exec
   @right out ring: exec

   on pressed {
     emit ring
     await Sleep(_, delay = 0.5)
     emit ring
   }
   ```

   Open the chip. Two places fire `ring`, but a game input takes only one
   wire, so the compiler adds a **Union** gate that merges the two pulses
   into the pin.
3. Combine the dice and the doorbell: wire `rolled` to a One Shot Audio's
   **Play** and `face` to a sign, so every roll makes a sound and shows its
   number.

## Make

Make a doorbell that rings at most once every three seconds, however often
the button is pressed. The sign should count every press, but only show
"Ding!" when the bell really rang. A `var` can hold text too:
`var shown: string = ""`.

```wiring
button Held -> chip pressed
chip sign -> decal Text
chip ring -> speaker Play
```

## Good to Know

- An exec output carries no value. Wire it only into an input that takes a
  pulse, such as One Shot Audio **Play**. An exec input such as `pressed`
  takes a pulse or an on/off output like **Held**, and starts its handler
  when that output turns on.
- A One Shot Audio with no sound picked plays nothing when pulsed, and
  nothing tells you why.
- A game input takes one wire. When two places in your code fire the same
  exec pin, the compiler merges them with a Union. Wiring two exec outputs
  into one input by hand is not allowed.

See [Exec outputs](../../statements.md#exec-outputs) and
[`emit`](../../statements.md#emit-emit-signal) in the reference.

Next: [Custom Events](custom-events.md).
