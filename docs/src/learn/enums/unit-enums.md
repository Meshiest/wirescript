# Enums

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Records](../data/records.md)

**Next steps:** [Payloads](payloads.md)

</div>
<!-- /learn-links -->

A traffic light is always exactly one of red, yellow or green. An `enum`
names those choices, a variable holds one of them, and `match` picks what to
do for each.

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
chip lamp -> light Color
switch Enabled -> light Enabled
```

## Code

```wirescript name=en1-traffic
enum Light { Red, Yellow, Green }

var state: Light = Light.Red
@right out lamp = match state {
  Red    => Color(1.0, 0.0, 0.0),
  Yellow => Color(1.0, 0.8, 0.0),
  Green  => Color(0.0, 1.0, 0.0),
}

on Clock(interval = 2.0) {
  match state {
    Red    => { state = Light.Green }
    Green  => { state = Light.Yellow }
    Yellow => { state = Light.Red }
  }
}
```

- `enum Light { Red, Yellow, Green }` makes a new type with three
  **variants**. A `Light` is always one of them.
- `Light.Red` names a variant. `var state: Light = Light.Red` holds one and
  starts red.
- The first `match` is a **value**. Each arm, `Red => ...`, gives the value
  for that variant, and the arms are separated by commas. Inside the arms the
  variants are written bare, since `state` is already known to be a `Light`.
- The second `match` is a **statement**. Each arm is a block of steps, and
  only the block for the current variant runs.

The light starts red. In what order do the colors come, and how long does
each one last?

## Run

1. Compile the chip and paste it in-game. Place a **Point Light** and a
   **Switch**.
2. Wire `lamp` to the Point Light's **Color**, and the Switch's **Enabled** to
   the light's **Enabled**. Flip the switch on.
3. In the Point Light's settings, turn off **Use Brick Color**. While it is
   on, the light keeps its brick's color and ignores the wire.
4. Watch the light go red, green, yellow, red, two seconds each.

## Under the Hood

Open the chip. `state` is one **Var** gate, labeled `state.__disc`, holding
a number: 0 for `Red`, 1 for `Yellow`, 2 for `Green`, in the order the
variants are declared. An enum without data is that number, with names
the compiler checks for you.

The value `match` is three **Equal** gates, each comparing that number with
one variant's, and three **Select**s in a row that pick the color. The
statement `match` in the handler is a Get Variable, then an Equal and a
Branch per arm, with a Set Variable on each arm's path.

A `match` must cover every variant. Leave one out and the compiler names it:

```wirescript name=en1-missing-arm expect=WS054
enum Light { Red, Yellow, Green }

var state: Light = Light.Red
@right out lamp = match state {
  Red   => Color(1.0, 0.0, 0.0),
  Green => Color(0.0, 1.0, 0.0),
}
```

```text
Error: [WS054] non-exhaustive match: `Yellow` is not covered
```

When you add a variant to an enum later, every `match` that forgets it
fails to compile, so none are missed.

## Modify

1. Write the handler with a value `match` instead:
   `state = match state { Red => Light.Green, Green => Light.Yellow, Yellow
   => Light.Red }`. The chain becomes one Get Variable and one Set Variable,
   with the choice made by Selects in between.
2. Add a crossing button. Add `@left in walk: exec` from a **Button**'s
   **Held**, and an `on walk` handler with a statement `match` that turns a
   green light yellow and leaves red and yellow alone (an arm can be an empty
   block, `{}`).
3. Add `@right out stop = match state { ... }`, giving `true` for red and
   yellow and `false` for green, and wire it to a **Brick Property Changer**'s
   **Collision Enabled** on a barrier brick.

## Make

Make a sky light that runs through dawn, day, dusk and night, ten seconds
each. Give each part of the day its own color and its own **Brightness**,
both chosen with a `match` on one enum.

```wiring
chip lamp -> light Color
chip brightness -> light Brightness
switch Enabled -> light Enabled
```

## Good to Know

- A Point Light's **Use Brick Color** overrides a wired **Color**. Turn it
  off whenever a circuit sets the color.
- The state is a **Var** gate, so the light comes back at the same color
  after the world is saved and loaded.

See [Enums](../../enums.md) and [`match`](../../enums.md#match).

Next: [Payloads](payloads.md).
