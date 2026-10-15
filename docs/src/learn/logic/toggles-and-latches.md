# Toggles and Latches

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Choosing a Value](choosing-a-value.md), [Remembering Values](../state.md)

</div>
<!-- /learn-links -->

The circuits in [Truth Tables](truth-tables.md) forget: their output depends
only on what the switches say right now. A button is different. Its **Held**
is on only for a moment, so a light that should stay on after the press needs
a circuit that remembers. This lesson builds two: a toggle, which flips on
each press, and a latch, which one button turns on and another turns off.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Use it: a set/reset latch](#use-it-a-setreset-latch)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip pressed
chip light -> light Enabled
```

## Code

```wirescript name=l3-toggle
@left in pressed: exec
var lit: bool = false
@right out light = lit.Value

on pressed {
  lit = Toggle()
}
```

`Toggle()` is a gate with a memory of its own: one on/off value. Each time a
handler's chain runs through it, it flips that value and gives the new one.
`lit = Toggle()` stores the answer in the variable, and `light` follows the
variable.

`pressed` is an exec pin. Wired from a Button's **Held**, `on pressed` runs
its chain when Held turns on: the moment it goes from off to on, called the
rising edge. `on !pressed` runs when it turns off, the falling edge, but it
needs a `bool` pin, as the next example shows.

The first press turns the light on. What does the second press do? What
happens while you hold the button down?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a **Point
   Light**.
2. Wire the Button's **Held** to `pressed`, and `light` to the Point Light's
   **Enabled**.
3. Press the button: the light turns on. Press it again: off. Holding the
   button does nothing more, because the chain runs once, on the rising edge.

## Under the Hood

Open the chip. The **Toggle** gate has an **Exec** input, and two outputs:
**Exec Out**, which carries the pulse on to the next step, and **Output**, its
on/off value. The next step is a **Set Variable** that stores **Output** in
`lit`.

`pressed` is wired straight into the Toggle's **Exec** input. That is what
`on pressed` compiles to: the Button's **Held** reaches an exec input, which
runs the chain when Held turns on. `on !pressed` puts a NOT gate in between,
so the chain runs when `pressed` turns off. The NOT reads the pin's value,
so a pin used with `on !` stays a `bool`.

This chip counts both edges. Wire `text` to a **Text Decal**'s **Text**, then
press, hold, and let go:

```wirescript name=l3-edges
@left in pressed: bool
var downs: int = 0
var ups: int = 0
@right out text = "down ${downs.Value}, up ${ups.Value}"

on pressed {
  downs += 1
}

on !pressed {
  ups += 1
}
```

A long hold still counts one down, and the up comes only when you let go.

## Use it: a set/reset latch

A latch has two inputs. `set` turns it on and `reset` turns it off, and
between presses it stays as it was. An alarm is one: one button arms it,
another disarms it, and a light shows which happened last.

The latch's value depends on three things: `set`, `reset`, and its own value
a moment ago.

```wirescript name=l3-latch
@left in set: bool
@left in reset: bool
let state: bool = set || (held && !reset)
buffer held: bool = state
@right out q = state
```

Read `state` as: on while `set` is held, or if it was on and `reset` is not
held. That "was on" is `held`: what the latch holds, not a Button's
**Held**.

`buffer held: bool = state` makes `held` a copy of `state` delayed by one
tick, the game's next update. The latch reads its own past value, so its wires
form a loop: `state` feeds `held`, and `held` feeds `state`. Every wire loop
must pass through a buffer. Without one, the compiler stops with `WS005`.

`held` is used on the line above the one that declares it. That is allowed
for a buffer, but the buffer then needs its type, `: bool`. Leave the type out
and the compiler cannot tell what `held` is when it reaches the `&&`:

```wirescript name=l3-latch-untyped expect=WS004
@left in set: bool
@left in reset: bool
let state: bool = set || (held && !reset)
buffer held = state
@right out q = state
```

```text
Error: [WS004] no overload for '&&' on any, bool
```

The message talks about `&&`, but the fix is the missing `: bool` on the
buffer.

To run the latch, wire two **Button**s' **Held** to `set` and `reset`, and `q`
to a **Point Light**'s **Enabled**. Press set: the light comes on and stays on
after you let go. Press reset: it goes off and stays off. Open the chip: an
OR, an AND, a NOT, and a **Buffer (Ticks)** gate whose output wires back into
the AND.

The toggle runs on exec pulses and stores into a variable. The latch is pure:
nothing in it runs in steps, and it remembers only through its buffer.

## Modify

1. Make the toggle flip when the button is let go instead: change
   `on pressed` to `on !pressed`, and the pin to `@left in pressed: bool` so
   `!pressed` can read it. Press and hold: nothing happens until you
   release.

2. The toggle does not need the variable. A handler can declare an output pin
   of its own, wired straight from the Toggle gate's **Output**:

   ```wirescript name=l3-toggle-direct
   @left in pressed: exec

   on pressed {
     @right out light: bool = Toggle()
   }
   ```

   Open it: there is no Variable gate, just the Toggle.

3. In the latch, holding both buttons at once turns the light on: `set` wins.
   Change it so that `reset` wins: `!reset && (set || held)`. Write the truth
   table for `set`, `reset` and `held` both ways and find the row where they
   differ.

## Make

Make a "which was pressed last" sign: two buttons and two lights. Pressing
the first lights the first light and turns off the second. Pressing the
second does the opposite. One latch is enough: the second light is `!q`.

```wiring
button1 Held -> chip set
button2 Held -> chip reset
chip q -> light1 Enabled
chip notQ -> light2 Enabled
```

## Good to Know

- `on` fires once when the wired value turns on, never again while it
  stays on. A button held down for a minute is one press.
- A Button's **Held** is on only while someone holds it, which is why the
  toggle and the latch need memory to keep the light on.

Next: [Bits](bits.md).
