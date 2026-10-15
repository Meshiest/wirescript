# Records

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Lists](../values/lists.md)

**Next steps:** [Records in Storage](records-in-storage.md), [Enums](../enums/unit-enums.md)

</div>
<!-- /learn-links -->

A record groups a few named values into one. `type` names the shape once,
and then a single `var` can hold a whole record, such as a round number, a
score and a best score.

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
button1 Held -> chip point
button2 Held -> chip nextRound
chip text -> decal Text
```

## Code

```wirescript name=d1-tally
type Tally = { round: int, points: int, best: int }

@left in point: exec
@left in nextRound: exec
var tally: Tally = { round: 1, points: 0, best: 0 }
@right out text = "Round ${tally.round.Value}: ${tally.points.Value} points, best ${tally.best.Value}"

on point {
  tally = { ...tally, points: tally.points + 1 }
}

on nextRound {
  if tally.points > tally.best {
    tally = { ...tally, best: tally.points }
  }
  tally = { ...tally, round: tally.round + 1, points: 0 }
}
```

- `type Tally = { round: int, points: int, best: int }` names a shape with
  three **fields**, each with its own type. On its own it builds nothing.
- `{ round: 1, points: 0, best: 0 }` is a record **literal**, with a value
  for every field, by name.
- `tally.points` reads one field. Outside a handler, add `.Value`, as with any
  variable.
- `{ ...tally, points: tally.points + 1 }` is a **spread**. `...tally` copies
  every field of `tally`, and `points:` after it replaces that one field.

Score three points, press `nextRound`, then score one more. What does the sign
say?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire one Button's **Held** to `point`, the other's **Held** to
   `nextRound`, and `text` to the Text Decal's **Text**.
3. Press `point` three times. The sign reads "Round 1: 3 points, best 0".
   Press `nextRound` and it reads "Round 2: 0 points, best 3". One more point gives
   "Round 2: 1 points, best 3".

## Under the Hood

Open the chip. There is no gate called `tally`. There are three **Var**
gates, labeled `tally.round`, `tally.points` and `tally.best`. A record in a
`var` is one Var gate per field. The sign's Format String is wired to all
three.

The `point` chain is a Get Variable for `tally.points`, an Add and a single
Set Variable. The spread names all three fields, but only `points` changes, so
the compiler writes only that one. `tally.points += 1` would build the same
three gates.

The `nextRound` chain reads `points` and `best`, compares them with a Greater
and a Branch, and ends with two Set Variables, one for `round` and one for
`points`.

A literal must name every field. Leave out the spread and the compiler says
which fields the value has and which it needed:

```wirescript name=d1-missing-field expect=WS003
type Tally = { round: int, points: int, best: int }

@left in point: exec
var tally: Tally = { round: 1, points: 0, best: 0 }

on point {
  tally = { points: tally.points + 1 }
}
```

```text
Error: [WS003] expected {round: int, points: int, best: int}, got {points: int}
```

## Modify

1. Change the `point` handler to `tally.points += 1`. One field can be
   written on its own, without a spread.
2. Add a `misses` field. Add it to the `type`, give it a starting value in
   the literal, show it on the sign, and add a third button that adds one to
   it.
3. Add a reset button. Name the starting record once with
   `const START: Tally = { round: 1, points: 0, best: 0 }`, start with
   `var tally: Tally = START`, and write `tally = START` in the reset handler.
   The reset chain is three Set Variables, each with a number typed in.

## Make

Make a lap counter for a race. Keep a record with the number of laps, the
time the current lap started and the fastest lap so far. Use a zone at the
finish line and `GetUnixTime()` for the time. Each time a player walks in,
finish the lap, keep the fastest, and start the next one.

```wiring
zone "Zone Ref" -> chip finish
chip text -> decal Text
```

## Good to Know

- Each field is its own Var gate, so a record is saved with the world
  field by field, like any other variable.
- Two handlers that write the same record have no order between them. Press
  both buttons in the same tick, and either handler's write can land last.

See [Record Types](../../types.md#record-types) for the full rules.

Next: [Records in Storage](records-in-storage.md).
