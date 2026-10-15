# Chips

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Mods](mods.md)

**Next steps:** [Functions](functions.md), [Presentation](presentation.md)

</div>
<!-- /learn-links -->

A `chip` is a mod that gets a microchip brick of its own. Its gates sit on
their own grid inside the brick, which you can open in-game. Chips keep a big
circuit tidy, and a named chip can be placed many times, each with its own
state.

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
button1 Held -> chip red
button2 Held -> chip blue
button3 Held -> chip reset
chip board -> decal Text
```

## Code

```wirescript name=g2-scoreboard
@left in red: exec
@left in blue: exec
@left in reset: exec

chip Tally(press: exec, clear: exec) -> int {
  var n: int = 0
  on press { n += 1 }
  on clear { n = 0 }
  out _ = n.Value
}

let redScore = Tally(red, reset)
let blueScore = Tally(blue, reset)
@right out board = "Red ${redScore}  Blue ${blueScore}"
```

`chip Tally(...)` is written like a mod: inputs in brackets, `-> int` for
the value it gives back. Inside, a chip can hold anything a file can, such as
a variable, handlers, and `out _ = ...` to set the value it gives back (`_` is
the name of a single unnamed output).

`Tally` keeps a count in `var n`. Red and Blue both call it. When Red's
button is pressed, does Blue's score go up too?

## Run

1. Compile the chip and paste it in-game. Place three **Button**s, for Red,
   Blue and Reset, and a **Text Decal**.
2. Wire each Button's **Held** to `red`, `blue` and `reset`, and `board` to
   the Text Decal's **Text**.
3. Press Red three times and Blue once, and the sign says `Red 3  Blue 1`.
   Press Reset and both go back to 0.

## Under the Hood

Open the chip. Inside are two microchip bricks, both labeled `Tally`. Open
one. It holds a **Var** gate `n`, an **Increment Variable** for `on press`,
and a **Set Variable** for `on clear`.

Each call made a new chip with its own `n`, so Red and Blue keep separate
scores. That is the same rule as a mod (every call is a copy), with a brick
around each copy.

Look at the edge of an open Tally: three pins, `press`, `clear` and `_`.
Each is a **Microchip Input** or **Microchip Output** gate. Every wire that
crosses a chip's wall goes through a pin. One pin serves every gate inside
that reads the same thing, and a constant needs no pin at all, because it is
typed into the gate that uses it. So a chip costs one pin for each different thing
that crosses its wall.

The wall is only visual. A chip's body can read and change a top-level
`var` directly, and the wire crosses the wall through a pin. Nothing
about the circuit's behavior changes when gates move into a chip.

The `@left` and `@right` pins belong to the outermost chip, the one the whole
file compiles to. A chip inside it cannot have side pins:

```wirescript name=g2-inner-pin expect=WS023
@left in red: bool
chip {
  @right out shown = red
}
```

```text
Error: [WS023] side annotations only apply to top-level ports of the compiled file
```

Declare the pin at the top level instead, and set it from inside the chip.

## Modify

1. Add a third team, Green: one more input, one more `Tally(...)` call, and
   one more microchip when you open the chip.

2. Give `Tally` a second output that pulses when a team reaches 5:

   ```wirescript name=g2-winner
   @left in red: exec
   @left in blue: exec
   @left in reset: exec
   var winner: string = "nobody yet"

   chip Tally(press: exec, clear: exec) -> (score: int, won: exec) {
     var n: int = 0
     on press {
       n += 1
       if n == 5 { emit won }
     }
     on clear { n = 0 }
     out score = n.Value
   }

   let r = Tally(red, reset)
   let b = Tally(blue, reset)
   on r.won { winner = "Red" }
   on b.won { winner = "Blue" }
   on reset { winner = "nobody yet" }
   @right out board = "Red ${r.score}  Blue ${b.score}  Winner: ${winner.Value}"
   ```

   `won: exec` is a pulse output. `emit won` fires it, and `on r.won` runs
   when Red's chip fires it.

3. Chips without names. This version keeps the scores at the top level and
   puts each piece of work in a chip of its own:

   ```wirescript name=g2-anonymous
   @left in red: exec
   @left in blue: exec
   @left in reset: exec
   var redScore: int = 0
   var blueScore: int = 0

   chip on red { redScore += 1 }
   chip on blue { blueScore += 1 }

   @closed chip ClearScores() {
     redScore = 0
     blueScore = 0
   }
   on reset { ClearScores() }

   chip let board = "Red ${redScore.Value}  Blue ${blueScore.Value}"
   @right out text = board
   ```

   - `chip on red { ... }` is a handler in a chip of its own.
   - `chip let board = ...` puts one calculation in a chip.
   - `ClearScores` has steps but no handler. Called inside `on reset`, it
     runs as a step of that handler, and the handler carries on after it.
   - `@closed` makes a chip arrive shut. Chips arrive open by default.

   Open the chip. The `board` chip has two pins in, one for each score, and
   one out. `ClearScores` has four: a pulse in, a pulse out, and one for each
   variable it sets.

## Make

Make a row of three lights, each with its own Button. Write one named chip
that takes a Button's **Held** and gives back whether its light is on, where
each press flips it. Call it three times. Then add a fourth Button that turns
all three lights off at once.

```wiring
button1 Held -> chip toggle1
button2 Held -> chip toggle2
button3 Held -> chip toggle3
button4 Held -> chip allOff
chip lit1 -> light1 Enabled
chip lit2 -> light2 Enabled
chip lit3 -> light3 Enabled
```

## Good to Know

- A named chip called twice is two chips with two sets of state. If two
  places must share a count, keep the count in a top-level `var` and pass it
  in, or let the chip change it directly.
- A chip's pins are labeled with the names from your code, so good input
  names make the opened chip easy to follow.

See also: [Chips](../../chips.md), [Boundary Pins](../../chips.md#boundary-pins)
and [Open and Closed Chips](../../chips.md#open-and-closed-chips).

Next: [Functions](functions.md).
