# First Circuit

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Tools](tools.md)

**Next steps:** [Gates and Wires](gates-and-wires.md)

</div>
<!-- /learn-links -->

Your first circuit is a chip with one pin in and one pin out, and nothing
between them but a wire. You will wire a switch to one side and a light to the
other.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
<!-- /toc -->

```wiring
switch Enabled -> chip a
chip b -> light Enabled
```

## Code

```wirescript name=first-circuit
@left in a: bool
@right out b = a
```

`@left in a: bool` puts a pin named `a` on the chip's left side. It takes a
`bool`: on or off. `@right out b = a` puts a pin named `b` on the right side
and gives it whatever arrives at `a`.

What does the light do when you flip the switch?

## Run

1. Compile the example and paste it in-game. The chip has a pin
   labeled `a` on its left and one labeled `b` on its right.
2. Place a **Switch** to the left of the chip and a **Point Light** to the
   right.
3. Wire the Switch's **Enabled** output to pin `a`.
4. Wire pin `b` to the Point Light's **Enabled** input.
5. Flip the switch. The light turns on and off with it.

## Under the Hood

Open the chip. Inside there is only a wire from `a` to `b`. The pins on the
chip's sides are rerouters: `@left` and `@right` each add one. Leave the side
annotations off and the chip has no gates at all. The source decides which
pins a compiled chip has and what connects them.

The rerouters are why the side annotations exist. Your wires end on them, not
on the chip, so they work as a socket: to replace the chip, break it and paste
the new version over the same rerouters, and the wires stay as they were.

## Modify

1. Replace the Switch with a **Button**, wired from its **Held** output. Now
   the light is on only while you hold the button.
2. Give the output a label of its own: `@label("Light") @right out b = a`.
   The pin's floating label now reads "Light".

   ```wirescript name=first-circuit-label
   @left in a: bool
   @label("Light") @right out b = a
   ```

3. Flip the light: change the output to `b = !a`. `!` is NOT, so the light is
   now on while the switch is off, and off while it is on.

   ```wirescript name=first-circuit-not
   @left in a: bool
   @right out b = !a
   ```

   Compile it, break the old chip and paste the new one over its rerouters.
   The Switch and the light stay wired. Open the chip again: the wire now
   passes through one NOT gate.

## Make

Make a chip with two inputs and two outputs that passes each input straight
across to its own output, then wire two switches to two lights through it.

```wiring
switch1 Enabled -> chip a
switch2 Enabled -> chip b
chip outA -> light1 Enabled
chip outB -> light2 Enabled
```

Next: [Gates and Wires](gates-and-wires.md).
