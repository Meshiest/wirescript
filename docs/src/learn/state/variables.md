# Variables

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remembering Values](../state.md)

**Next steps:** [Reading a Variable](reading-a-variable.md)

</div>
<!-- /learn-links -->

The [Remembering Values](../state.md) lesson used one `var` to count presses. This lesson
covers what a variable can hold, how it differs from a `let`, and how long it
remembers.

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
button Held -> chip toggle
chip text -> decal Text
chip lamp -> light Enabled
```

## Code

```wirescript name=st1-shop-sign
@left in toggle: exec
var isOpen: bool = false
var opened: int = 0
let word = if isOpen.Value then "OPEN" else "CLOSED"
@right out text = "${word} (opened ${opened.Value} times)"
@right out lamp = isOpen.Value

on toggle {
  isOpen = !isOpen
  if isOpen {
    opened += 1
  }
}
```

A shop sign with a button behind the counter. There are two variables, each
with a type and a starting value: `isOpen` is a `bool` that starts `false`,
and `opened` is an `int` (a whole number) that starts at 0.

`word` is a `let`. `if isOpen.Value then "OPEN" else "CLOSED"` picks one of
two values. Unlike the `if` block in a handler, it runs no steps: it is a gate
that always shows one value or the other, and it changes the moment `isOpen`
does. A `let` holds nothing of its own.

Press the button three times, then save the world and load it again. What does
the sign say?

## Run

1. Compile the chip and paste it in-game. Place a **Button**, a brick with a
   **Text Decal**, and a **Point Light**.
2. Wire the Button's **Held** to `toggle`, `text` to the Text Decal's
   **Text**, and `lamp` to the Point Light's **Enabled**.
3. Press the button. The sign reads "OPEN (opened 1 times)" and the light
   comes on. Press again: "CLOSED", light off, and the count stays at 1.
4. Save the world and load it. The sign and the light come back exactly as you
   left them.

## Under the Hood

Open the chip. Each `var` is a Variable gate (the game names it **Var**), with
its starting value written into the gate. The `let` is a Select gate wired
from `isOpen`'s Variable gate. It picks a value and stores nothing.

The handler is a chain: a Get Variable reads `isOpen`, a NOT flips it, a Set
Variable stores it, a second Get Variable reads it for the `if`, and a Branch
picks the path. `opened += 1` is a single Increment Variable gate.

The Variable gates are part of the chip's bricks, so their values are saved
with the world and come back when it loads.

A variable can hold any one kind of value: `bool`, `int`, `float` (a number
with a fraction), `string` (text), and more that later lessons cover. Leave
out the starting value and it starts at that type's zero: `var count: int`
starts at 0, `var label: string` starts empty.

## Modify

1. Change `var isOpen: bool = false` to `true`. A freshly pasted copy now
   starts open, with the light on.
2. Add a variable declared inside the handler:

   ```wirescript name=st1-local
   @left in pressed: exec
   var kept: int = 0
   var lastLocal: int = 0
   @right out text = "kept ${kept.Value}, local ${lastLocal.Value}"

   on pressed {
     var local: int = 0
     local += 1
     kept += 1
     lastLocal = local
   }
   ```

   Press a few times. `kept` climbs, but `local` is always 1. A `var` inside a
   handler is set back to its starting value every time the handler runs. In
   the chip, that is an extra Set Variable at the start of the chain. A `var`
   at the top of the file keeps its value between runs.

3. Count closings too: add `var closed: int = 0`, add one to it in an `else`
   branch of the `if`, and show both counts on the sign.

## Make

Make a lamp with two buttons, **On** and **Off**. A variable remembers whether
the lamp is on, the light follows it, and a sign shows how many times it has
been switched on. Pressing **On** while it is already on should not add to the
count. Save and load the world to check that it remembers.

```wiring
button1 Held -> chip turnOn
button2 Held -> chip turnOff
chip lamp -> light Enabled
chip text -> decal Text
```

## Good to Know

- Every `var` at the top of a file is stored in the chip's bricks, so a world
  save and load brings each value back as it was. Nothing starts over by
  itself. For a fresh start, the circuit needs a way to set its variables
  back.
- You may meet `static var` in the reference. It keeps its value inside a
  `mod`. At the top of a file a plain `var` already does, so these lessons
  never need it.

Next: [Reading a Variable](reading-a-variable.md).
