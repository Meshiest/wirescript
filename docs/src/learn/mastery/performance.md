# Performance

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Const and Folding](const-and-folding.md), [One Chip per Player](../prefabs/one-chip-per-player.md)

**Next steps:** [Multiplayer Architecture](multiplayer.md)

</div>
<!-- /learn-links -->

A circuit has three different costs, and they move separately: how many
gates are built, how many steps one pulse takes in a row, and how much the
game works out each tick. This lesson measures all three on one chip, then
uses them to choose between a `fn` and a `mod`, and between a loop and one
chip per player.

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
button Held -> chip add
chip sign -> decal Text
```

## Code

```wirescript name=ma3-timers
@left in add: exec
var timers: int[]
var running: int = 0
@right out sign = "Running: ${running.Value}"

on add { timers.push(50) }

on Clock(interval = 0.1) {
  var n: int = 0
  for i in 0..timers.length() {
    if timers[i] > 0 {
      timers[i] -= 1
      n += 1
    }
  }
  running = n
}
```

Each press starts a five-second timer: 50 pulses of a tenth of a second.
Every pulse counts all the timers down and counts how many are still
running.

The list only grows. Press ten times: how many gates does the chip have?
How much does one pulse do with ten timers? With ten thousand?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick
   with a **Text Decal**.
2. Wire the Button's **Held** to `add`, and `sign` to the Text Decal's
   **Text**.
3. Press three times quickly: `Running: 3`. Five seconds after the last
   press, `Running: 0`.

## Under the Hood

### Gates: what is built

`just ir timers.ws` gives 27 gate lines. The loop is a **For Loop** board,
and its body is built once: a **For Loop Entry** and 7 gates (two **Get
Array Element**, a **Greater**, a **Branch**, a **Subtract**, a **Set Array
Element** and an **Increment Variable**). Outside it, five gates (Subtract,
Multiply, Divide, Ceil, Max) work out how many passes to make, because the
length is only known as the game runs. A loop to a `const` has none of
them.

Ten timers or ten thousand, the chip stays 27 gates. Gates are what you
paste and what the world loads, so they matter for size, not for speed.

### Steps in a row

Follow the **Exec** wires from the Clock: Set Variable, Get Array Length,
the For Loop, Get Variable, Set Variable. Five steps, with the loop as one
of them. Inside the loop, each pass is five more after the entry: Get Array
Element, Branch, Get Array Element, Set Array Element, Increment Variable.

So one pulse runs 5 steps plus 5 for every running timer, all within one
tick, one after another. The gate count does not change with the number of
timers. The steps do.

### Work per tick

The game works out each gate in the loop once per pass, plus about 8 more
to enter the loop's board (measured in game, see
[When to use `for`](../../best-practices.md#when-to-use-for-and-what-it-costs)).
That is about 16 per running timer. Measured in game, a gate takes about
44.5 nanoseconds, so a tick at 60 per second has room for about 375,000.

| Timers | Work per pulse | Share of one tick |
|---|---|---|
| 10 | about 160 | nothing |
| 1,000 | about 16,000 | about 4% |
| 23,000 | about 375,000 | all of it |

Past that, the tick takes longer than a tick should. Check these numbers before making anything faster. Ten timers
are nowhere near any limit.

### A `fn` or a `mod`

Three timers, each shown as minutes and seconds:

```wirescript name=ma3-clocks
@left in a: exec
@left in b: exec
@left in c: exec
var ta: int = 0
var tb: int = 0
var tc: int = 0
var shown: string = ""
@right out sign = shown.Value

mod mmss(tenths: int) -> string {
  let secs = (tenths + 9) / 10
  let pad = if secs % 60 < 10 then "0" else ""
  return "${secs / 60}:${pad}${secs % 60}"
}

on a { ta = 900 }
on b { tb = 1800 }
on c { tc = 3000 }

on Clock(interval = 0.1) {
  if ta > 0 { ta -= 1 }
  if tb > 0 { tb -= 1 }
  if tc > 0 { tc -= 1 }
  shown = "${mmss(ta)}<br>${mmss(tb)}<br>${mmss(tc)}"
}
```

The body of `mmss` is 7 gates. As a `mod`, every call is a copy: 7 gates a
call. As a `fn`, the body is built once with a **Function Entry**, a
**Function Input**, a **Function Output** and the **Function Definition**
board, 11 in all, and each call is one **Function Call**.

With fewer calls (the other timers shown as plain numbers), the counts are:

| Calls of `mmss` | `mod` | `fn` |
|---|---|---|
| 1 | 38 | 43 |
| 2 | 45 | 44 |
| 3 | 52 | 45 |

From two calls on, the `fn` is smaller. The smaller the body, the more
calls it takes: with the 4-gate `medal` in
[Counting the Cost](../abstraction/cost.md), the mod stayed smaller even
at ten calls.

The `fn` is not free in steps. Each Function Call is a step in the
handler's chain, and the body runs inside it: the longest chain is 16 steps
with the mod and 19 with the fn. The game works out a fn's gates only when
it is called, though. Gates outside any board are worked out even when an
`if` around them is not taken.

### A loop, or one chip per player

[One Chip per Player](../prefabs/one-chip-per-player.md) counts Q presses
with a pad per player. The same job done by one chip would loop over the
room on a Clock, read each player with `GetInputs`, and keep a map of who
had Q down last time. Counted:

- The sweep is one chip of 21 gates. Its loop body is 15 gates with the
  entry, and it runs for every player in the room on every pulse, whether
  anyone presses or not.
- The per-player version is a 25-gate chip plus a 9-gate pad for each
  player: 115 gates with ten players. No chain runs until someone presses.
  Then the pad sends one event and the central chip counts it once.

Spawning a chip per player costs more gates. Here it also removes the
pulse, and with it the taps a pulse can miss. Where the work has to happen
on every pulse anyway, spreading it over many chips does not make it
smaller, so choose by what the work needs, as that lesson lists, not by
gate count.

## Modify

1. Loop over the timers themselves: `for i, t in timers` gives each
   element's index `i` and value `t`
   ([Index and Value](../loops/index-and-value.md)). Use
   `if t > 0 { timers[i] = t - 1 }`. The chip drops from 27 gates to 19: no
   gates to work out the pass count, no Get Array Element, and each pass is
   3 steps instead of 5.

2. Stop the list from growing. In `add`, reuse a finished timer:

   ```wirescript name=ma3-reuse
   @left in add: exec
   var timers: int[]
   var running: int = 0
   @right out sign = "Running: ${running.Value}"

   on add {
     let free = timers.find(0)
     if free.Found {
       timers[free.Index] = 50
     } else {
       timers.push(50)
     }
   }

   on Clock(interval = 0.1) {
     var n: int = 0
     for i, t in timers {
       if t > 0 {
         timers[i] = t - 1
         n += 1
       }
     }
     running = n
   }
   ```

   `timers.find(0)` is one **Find Array Element** gate: `.Found` says whether
   a 0 is in the list, and `.Index` where. Now the list never holds more
   timers than were running at once, so the pulse never visits a timer
   that finished long ago.

3. Add a fourth timer to the clocks. Count both versions: the `mod` grows
   from 52 to 68 gates and the `fn` from 45 to 55. The timer itself is 9
   gates in both, and the rest is the call.

## Make

Make a sign that tracks every player who walks through a zone and shows
how long ago the last one came in. Write it with a loop. Count its gates,
count the steps one pulse takes for each player, and work out the share of
a tick it needs for 10, 100 and 1,000 players. At what size would you
change how it is built, and to what?

```wiring
zone "Zone Ref" -> chip door
chip sign -> decal Text
```

## Good to Know

- Loops that run more than 100,000 passes in one tick, across all loops,
  are cut off partway. A body like the timers' runs out of time long before
  that. `let run = for ...` gives `run.aborted`, the loop's **Aborted**
  output ([Loops Across Ticks](../loops/loops-across-ticks.md)).
- Measured in game, building a piece of text costs about a microsecond.
  Reading a variable or a map, and showing text, cost almost nothing.
  [Measured costs](../../best-practices.md#measured-costs) has the table.
- Every pasted save is at least about 25 KB, whatever is in it. A small
  chip and a big one paste at much the same size.

See also: [Best Practices: Gate Count & Scaling](../../best-practices.md),
[Profiling](../../best-practices.md#profiling-find-the-hot-spots) and
[`for` Loops](../../statements.md#for-loops) in the reference.

Next: [Multiplayer Architecture](multiplayer.md).
