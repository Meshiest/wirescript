# Buffers

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reading a Variable](reading-a-variable.md)

**Next steps:** [Resetting](resetting.md)

</div>
<!-- /learn-links -->

A `buffer` passes a value on one tick late. With that delay a circuit can feed
its own output back into itself, so memory can be built from gates alone, with
no variable and no handler.

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
button1 Held -> chip set
button2 Held -> chip reset
chip alarm -> light Enabled
```

## Code

```wirescript name=st3-latch
@left in set: bool
@left in reset: bool
let state = set || (held && !reset)
buffer held: bool = state
@right out alarm = state
```

This is a latch: `set` turns it on, `reset` turns it off, and it remembers in
between.

`buffer held: bool = state` makes `held` whatever `state` was one tick ago.
The line above already uses `held`, before it is declared: that is the loop.
`state` is on while `set` is pressed, or while it was on a tick ago and
`reset` is not pressed.

Walk through it. Press `set`: `state` turns on. A tick later `held` is on, so
`state` stays on after you let go. Press `reset`: `state` turns off, and a tick
later so does `held`.

What happens if you hold both buttons at once?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a **Point
   Light**.
2. Wire one Button's **Held** to `set`, the other's **Held** to `reset`, and
   `alarm` to the Point Light's **Enabled**.
3. Press `set`: the light comes on and stays on. Press `reset`: it goes off
   and stays off. Hold both: `set` wins, and the light is on.

## Under the Hood

Open the chip. There is an OR, an AND, a NOT and one **Buffer (Ticks)** gate
set to wait one tick. Follow the wires: OR, into the buffer, into the AND, and
back into the OR. The wires make a loop, and the buffer sits in it.

Every loop of wires needs a buffer somewhere in it. The game works out all
pure gates together each tick, and a loop without a delay would need its own
answer before it could work out its answer. The compiler refuses one. Here an
output pin feeds itself:

```wirescript name=st3-no-buffer expect=WS005
@left in set: bool
@left in reset: bool
@right out alarm: bool = set || (alarm && !reset)
```

```text
Error: [WS005] cycle in the wire graph has no barrier (a Buffer or Queue must break it); 3 gate(s) in the loop:
```

The error goes on to list each gate in the loop. Put a `buffer` anywhere in it
and the loop is legal.

The `: bool` on the buffer matters. `state` uses `held` before the compiler
reaches the buffer's line, so without a type it does not yet know what `held`
holds:

```wirescript name=st3-untyped expect=WS004
@left in set: bool
@left in reset: bool
let state = set || (held && !reset)
buffer held = state
@right out alarm = state
```

```text
Error: [WS004] no overload for '&&' on any, bool
```

The message is about `&&`, but the fix is on the next line: give the buffer
its type.

## Modify

1. Make `reset` win when both are held: `let state = !reset && (set || held)`.
2. A buffer can count ticks. This one adds one to its own value every tick:

   ```wirescript name=st3-ticks
   buffer tick: int = tick + 1
   @right out text = "Ticks: ${tick}"
   ```

   Wire `text` to a **Text Decal**'s **Text**. It climbs by about 60 every
   second.

3. Turn the latch into a burglar alarm. Give a brick an **Attached Zone** and
   wire its **Occupied** output to `set` instead of the first button. The
   light comes on when someone walks in and stays on after they leave, until
   the reset button is pressed.

## Make

Make a quiz buzzer for two players. Each player has a button and a light. The
first to press lights their own light, and the other player's button then does
nothing until a third button resets both. Use one latch per player, and let
each latch's `set` be blocked by the other latch.

```wiring
button1 Held -> chip buzz1
button2 Held -> chip buzz2
button3 Held -> chip reset
chip lit1 -> light1 Enabled
chip lit2 -> light2 Enabled
```

## Good to Know

- A buffer delays by one tick, about a sixtieth of a second, so the delay
  itself is too short to see.
- `on` already runs once when a button goes down, not while it is held. You do
  not need a buffer to catch the moment of a press.
- A buffer read before its declaration needs a type. Without one the compiler
  reports `WS004`, pointing at whatever operator first uses the buffer.

Next: [Resetting](resetting.md).
