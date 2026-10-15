# Gates and Wires

<!-- learn-links -->
<div class="learn-links">

**Before this:** [First Circuit](first-circuit.md)

**Next steps:** [Pure and Exec](pure-and-exec.md), [Truth Tables](logic/truth-tables.md)

</div>
<!-- /learn-links -->

A gate is a brick that computes something from its inputs, like AND or
addition. Wirescript writes gates as expressions: every operator becomes one
gate, and every name you use becomes a wire to the gate that made it.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Use it: a two-way hall light](#use-it-a-two-way-hall-light)
- [Make](#make)
<!-- /toc -->

```wiring
switch1 Enabled -> chip a
switch2 Enabled -> chip b
chip both -> light1 Enabled
chip either -> light2 Enabled
```

## Code

```wirescript name=both-switches
@left in a: bool
@left in b: bool
@right out both = a && b
@right out either = a || b
```

`&&` is AND: true when both sides are. `||` is OR: true when either is. With
two switches wired to `a` and `b`, and two lights on `both` and `either`,
which lights are on for each of the four switch positions?

## Run

1. Compile the chip and paste it in-game. Place two **Switch**es and two **Point
   Light**s.
2. Wire each Switch's **Enabled** to `a` and `b`, and `both` and `either` to
   each light's **Enabled**.
3. Try all four positions. `both` lights only with both switches on.
   `either` lights with any switch on.

## Under the Hood

Open the chip. It holds one AND gate and one OR gate, each wired from both
pins, and nothing else.

The same circuit built by hand is two gate bricks and six wires. Wirescript
writes the bricks and wires for you, and the source stays readable as the
circuit grows.

`let` gives a value a name without adding a gate. The name is a wire you can
use again:

```wirescript name=named-wire
@left in a: bool
@left in b: bool
let both = a && b
@right out lit = both
@right out dark = !both
```

`!` is NOT. This chip has two gates (AND and NOT): `both` names the AND
gate's output, and both outputs read it.

## Use it: a two-way hall light

A hallway light with a switch at each end should change whenever either
switch is flipped. That is exclusive OR, written `^^`: true when exactly one
side is true.

```wirescript name=hall-light
@left in near: bool
@left in far: bool
@right out light = near ^^ far
```

Wire a **Switch** at each end to `near` and `far`, and `light` to a **Point
Light**'s **Enabled**. Flipping either switch changes the light.

## Make

Make a chip with three switch inputs whose light turns on when at least two of
them are on.

```wiring
switch1 Enabled -> chip a
switch2 Enabled -> chip b
switch3 Enabled -> chip c
chip lit -> light Enabled
```

Next: [Pure and Exec](pure-and-exec.md).
