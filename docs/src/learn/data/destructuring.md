# Destructuring

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Tuples](tuples.md)

</div>
<!-- /learn-links -->

[Tuples](tuples.md) took a mod's outputs apart with `let (lo, hi) = ...`.
Records come apart the same way, by field name instead of by position. This
works in a `let`, in a mod's parameter list and in an event's data.

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
zone "Zone Ref" -> chip room
chip text -> decal Text
```

## Code

```wirescript name=d4-stay-timer
import { escape } from "sanitize"

type Visit = { name: string, seconds: int }

mod describe({ name, seconds }: Visit) -> string {
  return "${escape(name)} stayed ${seconds} seconds"
}

@left in room: zone
var entered: Map<string, int>
var last: Visit = { name: "Nobody", seconds: 0 }
@right out text = describe(last)

on ZoneEntered(zone = room) -> { character: who } {
  entered.set(who.GetUserId(), GetUnixTime())
}

on ZoneLeft(zone = room) -> { character: who } {
  let { Value: since, Found } = entered.get(who.GetUserId())
  if Found {
    last = { name: who.GetDisplayName(), seconds: GetUnixTime() - since }
  }
}
```

`ZoneLeft` is the opposite of `ZoneEntered`. It runs when a player walks out.
`GetUnixTime()` is the current time as a whole number of seconds, so the
difference between two readings is a time in seconds.

The code uses three kinds of destructuring:

- `-> { character: who }` takes the event's data by field name. The zone
  events have one field, `character`, and `character: who` names it `who`
  here.
- `entered.get(...)` gives back a record with two fields, `Value` and
  `Found`. `let { Value: since, Found } = ...` takes both out, renaming `Value`
  to `since` and keeping `Found` under its own name.
- `mod describe({ name, seconds }: Visit)` takes a `Visit` and names its
  fields straight away, so the body uses `name` and `seconds` with no
  `visit.` in front.

Walk into the zone, wait ten seconds, and walk out. What does the sign say?
What happens to a player whose entry time was never stored?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**,
   and place a brick with a **Text Decal**.
2. Wire the Attached Zone's **Zone Ref** to `room`, and `text` to the Text
   Decal's **Text**.
3. The sign starts as "Nobody stayed 0 seconds". When you walk in, wait and
   walk out, it shows your name and how long you stayed.

## Under the Hood

Open the chip. Destructuring builds no gates. The **Get Map Element** in the
`ZoneLeft` chain has a **Value** output and a **Found** output, and the two
names are those wires. `Found` goes into a Branch, and `since` into the
Subtract that works out the stay.

`describe(last)` is a **Replace in String** (from `escape`) and a Format
String, wired from the `last.name` and `last.seconds` Var gates. Passing the
record to the mod and taking it apart again costs nothing.

A pattern can only name fields the value has:

```wirescript name=d4-no-field expect=WS002
type Visit = { name: string, seconds: int }

var last: Visit = { name: "Nobody", seconds: 0 }
let { name, time } = last
@right out text = name
```

```text
Error: [WS002] no field `time` on this value; available fields: name, seconds
```

## Modify

1. Destructure a record in a `let`. In the `ZoneLeft` handler, after setting
   `last`, add `let { seconds } = last` and send the player their own time
   with `who.ShowChatMessage("You stayed ${seconds} seconds")`.
2. Keep the longest stay. Add `var longest: Visit = { name: "Nobody",
   seconds: 0 }` and a second sign. Replace it whenever a stay beats it.
3. Write the events with tuple capture instead, `-> (who)`. Both forms name
   the same output. Record capture can also pick fields out by name when an
   event has several.

## Make

Make a "hold the hill" zone. Keep each player's total time inside across
visits, in a `Map<string, Visit>` keyed by user id, and show who has held it
longest. Write one mod that takes a `Visit` apart in its parameter list to
build the sign's text.

```wiring
zone "Zone Ref" -> chip hill
chip text -> decal Text
```

## Good to Know

- `ZoneLeft` may fire for a player with no stored entry time, for example
  one who was already inside when the chip was pasted. `Found` is then false,
  and the sign keeps the last real visit. Over a long game a zone event can
  also be missed, which loses that one visit the same way.
- `GetUnixTime()` counts whole seconds, so a short stay can show as 0 or 1.
- A display name is chosen by the player. `describe` escapes it before it
  reaches the sign.

See [Record Destructuring](../../statements.md#record-destructuring),
[Destructured Parameters](../../chips.md#destructured-parameters) and
[Binding Event Data](../../statements.md#binding-event-data).

Next: pick a lesson from the tree on the [Learn](../README.md) page.
