# Remembering Values

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Pure and Exec](pure-and-exec.md)

**Next steps:** [Reacting to the Game](reacting-to-the-game.md), [Variables](state/variables.md), [Numbers](values/numbers.md), [Toggles and Latches](logic/toggles-and-latches.md), [Text Values](strings/text-values.md), [Interpolation](templates/interpolation.md)

</div>
<!-- /learn-links -->

A `var` keeps its value until a handler changes it. The value stays through
other presses and other players' actions, and when the world is saved and
loaded again.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood: `let` and `var`](#under-the-hood-let-and-var)
- [Use it: a press-count sign](#use-it-a-press-count-sign)
- [Make](#make)
<!-- /toc -->

```wiring
button Held -> chip pressed
chip text -> decal Text
```

## Code

```wirescript name=press-count
@left in pressed: exec
var count: int = 0
@right out text = "Pressed ${count.Value} times"

on pressed {
  count += 1
}
```

`var count: int = 0` is a whole number that starts at 0. `count += 1` adds one
to it. `"Pressed ${count.Value} times"` builds text with the count in the
middle. [String Interpolation](../expressions.md#string-interpolation) has
the details.

What does the sign say after three presses?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with a **Text
   Decal**.
2. Wire the Button's **Held** to `pressed`, and `text` to the Text Decal's
   **Text** input.
3. Press the button a few times. The sign counts.

## Under the Hood: `let` and `var`

A `let` is a name for a wire. It holds nothing. Its value is whatever its
gates compute right now. A `var` is a Variable gate that holds a value until a
handler writes a new one.

There are two ways to read a `var`:

- `count.Value`, outside a handler, is a pure wire from the Variable gate.
  After a handler writes it, this wire shows the new value on the next tick,
  the game's next update.
- `count`, inside a handler, reads the variable at that step of the chain.
  A step after a write sees the new value.

```wirescript name=read-in-handler
@left in pressed: exec
var count: int = 0
var last: int = 0
@right out shown = last.Value

on pressed {
  count += 1
  last = count
}
```

Inside the handler, `last = count` runs after `count += 1`, so it copies the
new count.

A `var` can hold other kinds of value too. `var shown: string = ""` holds
text, and a handler can write a whole message into it: `shown = "Welcome"`.

## Use it: a press-count sign

The first example is already a useful part: a visitor counter for a door, or a
score for a button game.

## Make

Make a sign that counts presses of one button and resets to 0 when a second
button is pressed.

```wiring
button1 Held -> chip pressed
button2 Held -> chip reset
chip text -> decal Text
```

Next: [Reacting to the Game](reacting-to-the-game.md).
