# Pure and Exec

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Gates and Wires](gates-and-wires.md)

**Next steps:** [Remembering Values](state.md)

</div>
<!-- /learn-links -->

Wires carry two kinds of signal. Telling them apart is the most important
idea in this series.

- A **pure** wire carries a value that is always there, like the switch's
  on/off. A gate on a pure wire recomputes whenever its inputs change. Every
  circuit so far has been pure.
- An **exec** wire carries a pulse: a moment when something happens, like a
  button being pressed. An exec pulse runs a chain of steps, one after
  another, and each step can change something.

Think of a light switch and a doorbell. The switch is always either up or
down (pure). The doorbell rings at a moment, and the ring makes something
happen (exec).

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify: a door that closes by itself](#modify-a-door-that-closes-by-itself)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip pressed
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Code

```wirescript name=toggle-door
@left in pressed: exec
var shut: bool = true
@right out solid = shut.Value

on pressed {
  shut = !shut
}
```

`pressed` is an exec pin: an input whose job is to start something.
`var shut` is a variable: a value the circuit remembers and can change.
`on pressed { ... }` is a handler. It runs its steps once each time `pressed`
fires, and only then. `shut = !shut` flips the variable. `shut.Value`
reads the variable as a pure value, so `solid` follows it.

Wire a **Button** to `pressed`. What happens on the first press? The second?
What happens while you hold the button down?

## Run

1. Compile the chip and paste it in-game. Build a door from a few bricks and give one of
   them a **Brick Property Changer**.
2. Wire a **Button**'s **Held** output to `pressed`.
3. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility** inputs. One output can feed both.
4. Press the button: the door vanishes. Press again: it is back. Holding the
   button does nothing more, because the handler runs once per press, when
   `pressed` turns on.

## Under the Hood

Open the chip. The variable is a Variable gate. The handler is a short chain:
a Get Variable reads `shut`, a NOT flips it, and a Set Variable stores it.
The chain's exec input is wired from `pressed`. **Held** is an on/off value,
not a pulse, but wiring it into an exec pin works: the chain runs when Held
turns on, not while it stays on. `solid` is wired from the Variable gate's
value, a pure wire.

`on` fires when its trigger turns on. `on !pressed { ... }` fires when it
turns off. That reads `pressed` as an on/off value, so declare the pin `bool`
for it: an exec pin stands for the moment something happens, not for a value
that stays on or off.

## Modify: a door that closes by itself

```wirescript name=timed-door
@left in pressed: exec
var shut: bool = true
@right out solid = shut.Value

on pressed {
  shut = false
  await Sleep(_, delay = 3.0)
  shut = true
}
```

`await Sleep(_, delay = 3.0)` pauses this chain for three seconds, then
carries on. Treat the line as a "wait" block for now: the `_` and the other
forms of `await` come in a later lesson.

Press the button: the door opens, and three seconds later it shuts. Change the
delay. What happens if you press again while it is open?

## Make

Make a door that opens while the button is held and closes when it is
released, using `on pressed` and `on !pressed`. Declare the pin
`@left in pressed: bool` for this, since `!pressed` reads its value. Then
compare it with
`@right out solid = !pressed`, which does the same with one pure gate and no
handler. Reach for a handler when something has to be remembered or waited
for.

```wiring
button Held -> chip pressed
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Good to Know

- Declare a pin `exec` when it only starts handlers, and `bool` when the
  circuit also reads its value (`!pressed`, `if pressed`, `out x = pressed`).
- A handler on a pin runs once when the wired value turns on, not
  continuously while it stays on.
- A Button's **Held** is on only while someone holds it. A **Switch**'s
  **Enabled** stays on until it is flipped back.

Next: [Remembering Values](state.md).
