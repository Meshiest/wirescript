# Payloads

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Enums](unit-enums.md)

**Next steps:** [Option and Result](option-and-result.md)

**Parts that use this:** [Round Manager](../parts/round-manager.md), [Whack-a-Light](../parts/arcade-game.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

A variant can carry data of its own. A countdown carries how many seconds are
left, and a finished game carries how long it took. This lesson builds a small
game round as a **state machine**: one enum value, moved from variant to
variant by buttons and a clock.

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
button1 Held -> chip start
button2 Held -> chip finish
chip sign -> decal Text
```

## Code

```wirescript name=en2-phase
enum Phase { Lobby, Countdown(int), Playing { started: int }, Over { seconds: int } }

@left in start: exec
@left in finish: exec
var phase: Phase = Phase.Lobby
@right out sign = match phase {
  Lobby => "Press start",
  Countdown(n) => "Starting in ${n}",
  Playing { .. } => "Go!",
  Over { seconds } => "Finished in ${seconds} seconds",
}

on start {
  if phase is Phase.Lobby || phase is Phase.Over {
    phase = Phase.Countdown(3)
  }
}

on finish {
  let Playing { started } = phase else { return }
  phase = Phase.Over { seconds: GetUnixTime() - started }
}

on Clock(interval = 1.0) {
  match phase {
    Countdown(n) => {
      if n <= 1 {
        phase = Phase.Playing { started: GetUnixTime() }
      } else {
        phase = Phase.Countdown(n - 1)
      }
    }
    _ => {}
  }
}
```

- `Countdown(int)` carries one value by position, built as
  `Phase.Countdown(3)`. `Playing { started: int }` carries a named field,
  built as `Phase.Playing { started: ... }`.
- In a `match` arm, `Countdown(n)` names the carried value `n`, and
  `Over { seconds }` names the field. `Playing { .. }` matches without naming
  anything.
- `phase is Phase.Lobby` is `true` while `phase` is that variant.
- `let Playing { started } = phase else { return }` takes the data out if
  `phase` is `Playing`, and otherwise runs the `else`, which must leave the
  handler, here with `return`. After it, `started` is ready to
  use.
- `GetUnixTime()` is the current time as a whole number of seconds.
- `_ => {}` covers every variant not named above it.

Press `start`. What does the sign show over the next few seconds? What
happens if you press `finish` before the countdown ends?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire the Buttons' **Held** outputs to `start` and `finish`, and `sign` to
   the Text Decal's **Text**.
3. Press `start`. The sign counts "Starting in 3", 2, 1, then "Go!". Wait a
   while and press `finish`. The sign shows "Finished in 7 seconds", or
   however long it was.
4. `finish` does nothing until the round is playing, and `start` does nothing
   while one is running.

## Under the Hood

Open the chip. `phase` is four **Var** gates: `phase.__disc` for the variant's
number, and one for each piece of data, `phase.__Countdown_0`,
`phase.__Playing_started` and `phase.__Over_seconds`. Every variant has its
own slots, side by side. The sign's `match` reads the number to choose an arm,
and each arm reads only its own variant's slot.

Each `phase = Phase.Countdown(3)` writes the number and the new data with Set
Variables, and clears the slots of the other variants. The extra Var gates
named `Phase.__...` hold the zeros it clears them with.

`is` is a single Equal on the number, as is the test in `let ... else`. That
number is also readable as `phase.Discriminant`: 0 for `Lobby` up to 3 for
`Over`.

An enum has no `==`, because comparing two values would mean comparing every
slot. Test the variant with `is`:

```wirescript name=en2-no-equals expect=WS004
enum Phase { Lobby, Countdown(int) }

@left in start: exec
var phase: Phase = Phase.Lobby

on start {
  if phase == Phase.Lobby {
    phase = Phase.Countdown(3)
  }
}
```

```text
Error: [WS004] no overload for '==' on Phase, Phase
```

## Modify

1. Show the step number on a second sign:
   `@right out step = phase.Discriminant`. It reads 0 in the lobby and 3 when
   the round is over.
2. Ignore a `finish` in the first five seconds of play. After the
   `let ... else`, add `if GetUnixTime() - started < 5 { return }`.
3. Add a `Paused { left: int }` variant and a pause button that stops a
   countdown and keeps the seconds left. The sign's `match` stops compiling
   until it has a `Paused` arm. Pressing `start` while paused carries on from
   where it stopped.

## Make

Make a microwave. It is `Idle`, `Cooking(int)` with the seconds left, or
`Done`. A button adds ten seconds while it is idle or cooking, a Clock counts
down, and a light shows `Done` until a door button sends it back to `Idle`.

```wiring
button1 Held -> chip add
button2 Held -> chip door
chip done -> light Enabled
```

## Good to Know

- `GetUnixTime()` counts whole seconds, so a round's time is rounded to a
  second.
- The Clock and the buttons start separate handlers with no order between
  them. A `finish` pressed in the same tick as the last countdown step may see
  either the countdown or the round.

See [Enums](../../enums.md), [`is`](../../enums.md#is) and
[`if let` / `let else`](../../enums.md#if-let--let-else).

Next: [Option and Result](option-and-result.md).
