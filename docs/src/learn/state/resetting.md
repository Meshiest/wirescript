# Resetting

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Buffers](buffers.md)

</div>
<!-- /learn-links -->

Variables keep their values through presses, players and world loads. Nothing
starts over by itself, so most circuits with state need a reset: one input
that puts every variable back where it began.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Use it: a reset that passes on](#use-it-a-reset-that-passes-on)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button1 Held -> chip red
button2 Held -> chip blue
button3 Held -> chip reset
chip text -> decal Text
```

## Code

```wirescript name=st4-scoreboard
@left in red: exec
@left in blue: exec
@left in reset: exec
var redScore: int = 0
var blueScore: int = 0
var lastPoint: string = "none"
@right out text = "Red ${redScore.Value}, Blue ${blueScore.Value} (last: ${lastPoint.Value})"

on red {
  redScore += 1
  lastPoint = "red"
}

on blue {
  blueScore += 1
  lastPoint = "blue"
}

on reset {
  redScore = 0
  blueScore = 0
  lastPoint = "none"
}
```

A scoreboard for two teams. Each team's button adds a point and records who
scored last. The `reset` handler writes every variable back to the value it
was declared with.

After a few points, what does the sign say right after a reset?

## Run

1. Compile the chip and paste it in-game. Place three **Button**s and a brick
   with a **Text Decal**.
2. Wire the Buttons' **Held** outputs to `red`, `blue` and `reset`, and `text`
   to the Text Decal's **Text**.
3. Score a few points for each team. Press reset: the sign reads "Red 0, Blue
   0 (last: none)", the same as when it was pasted.

## Under the Hood

Open the chip. There are three chains, one per handler. The reset chain is
three Set Variable gates in a row, each writing a constant, wired from the
`reset` pin.

When you add a variable to a circuit, add it to the reset handler too. A
variable left out keeps its old value through every reset, which can look like
a bug somewhere else.

## Use it: a reset that passes on

A bigger build has several chips, each with its own state. Instead of running
a wire from one reset button to every chip, each chip can pass the reset on to
the next once it has finished its own. This is a daisy chain.

```wirescript name=st4-chain
@left in point: exec
@left in reset: bool
var score: int = 0
var passing: bool = false
@right out text = "Score: ${score.Value}"
@right out resetNext = passing.Value

on point {
  score += 1
}

on reset {
  score = 0
  passing = true
}

on !reset {
  passing = false
}
```

`resetNext` turns on after this chip's reset steps have run, and turns off
when the reset button is let go. `on !reset` runs when `reset` turns off.
It reads the pin's value, so `reset` is a `bool` in this chip.
Wired into the next chip's `reset`, that rising `resetNext` starts the next
chip's reset, and so on down the line.

1. Paste the chip three times. Give each copy its own **Button** on `point`
   and its own **Text Decal** on `text`.
2. Wire a **Button**'s **Held** to the first copy's `reset`. Wire the first
   copy's `resetNext` to the second copy's `reset`, and the second's
   `resetNext` to the third's `reset`.
3. Score a few points on each, then press the reset button. All three signs
   go back to 0, each one just after the one before it.

## Modify

1. Wire an **Attached Zone**'s **Occupied** output to `reset` instead of the
   button. The scores clear whenever someone steps into the zone.
2. Keep a best score that survives a reset. Add `var best: int = 0`, set it
   whenever a team's score passes it, show it on the sign, and leave it out of
   the reset handler.
3. Flash a light for two seconds after each reset. Add `var flash: bool =
   false` and `@right out light = flash.Value`, then end the reset handler
   with:

   ```wirescript ignore
     flash = true
     await Sleep(_, delay = 2.0)
     flash = false
   ```

## Make

Make a round counter chip with `@left in reset: exec` that counts how many
times it has been reset, and shows "Round 3" on a sign. Put it at the end of a
chain of two scoreboards, so one button clears the scores and starts a new
round.

```wiring
circuit1 "Scoreboard"
circuit2 "Scoreboard"
chip "Round"
button1 Held -> circuit1 point
button2 Held -> circuit2 point
button3 Held -> circuit1 reset
circuit1 resetNext -> circuit2 reset
circuit2 resetNext -> chip reset
circuit1 text -> decal1 Text
circuit2 text -> decal2 Text
chip text -> decal3 Text
```

## Good to Know

- A world save and load keeps every variable, so a circuit starts over only
  when its reset runs.
- A Button's **Held** stays on for as long as it is held, but `on reset` runs
  once per press. Holding the reset button does not keep resetting.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
