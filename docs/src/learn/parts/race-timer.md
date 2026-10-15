# Race Timer

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Number Options](../templates/number-options.md), [Mods](../abstraction/mods.md), [Per-Player State](../references/per-player-state.md)

</div>
<!-- /learn-links -->

A timed course with a start zone, two split zones and a finish zone. Each
racer sees their splits and finish time as `mm:ss`, the circuit keeps every
racer's best time, and a sign shows the course record.

<!-- toc -->
## Contents

- [You need](#you-need)
- [Code](#code)
- [Wire it](#wire-it)
- [Check it in game](#check-it-in-game)
- [Change it](#change-it)
- [How it works](#how-it-works)
<!-- /toc -->

## You need

- Four bricks with an **Attached Zone** each: start, first split, second
  split, finish.
- A brick with a **Text Decal** for the record.

```wiring
zone1 "Zone Ref" -> chip start
zone2 "Zone Ref" -> chip split1
zone3 "Zone Ref" -> chip split2
zone4 "Zone Ref" -> chip finish
chip sign -> decal Text
```

## Code

```wirescript name=gs2-race-timer
import { escape } from "sanitize"

const SPLITS = 2 // split zones between start and finish

@left in start: zone
@left in split1: zone
@left in split2: zone
@left in finish: zone

var startedAt: Map<string, int>
var next: Map<string, int>
var best: Map<string, int>
var record: int = 0
var holder: string = ""

mod pad2(n: int) -> string {
  let s = "0${n}"
  return s.Substring(s.Length() - 2, 2)
}

mod clock(seconds: int) -> string {
  return "${pad2(seconds / 60)}:${pad2(seconds % 60)}"
}

@right out sign = if holder.Value == "" then "No record yet" else "Record: ${holder.Value} ${clock(record.Value)}"

mod split(who: character, n: int) {
  let id = who.GetUserId()
  let at = next.get(id)
  let began = startedAt.get(id)
  if !at.Found || at.Value != n {
    return
  }
  next.set(id, n + 1)
  who.ShowStatusMessage("Split ${n}: ${clock(GetUnixTime() - began.Value)}")
}

on ZoneEntered(zone = start) -> (character) {
  let id = character.GetUserId()
  startedAt.set(id, GetUnixTime())
  next.set(id, 1)
  character.ShowStatusMessage("Go!")
}

on ZoneEntered(zone = split1) -> (character) { split(character, 1) }
on ZoneEntered(zone = split2) -> (character) { split(character, 2) }

on ZoneEntered(zone = finish) -> (character) {
  let id = character.GetUserId()
  let at = next.get(id)
  let began = startedAt.get(id)
  if !at.Found || at.Value != SPLITS + 1 {
    return
  }
  next.remove(id)
  let time = GetUnixTime() - began.Value
  let old = best.get(id)
  if !old.Found || time < old.Value {
    best.set(id, time)
    character.ShowStatusMessage("Finished in ${clock(time)}, a new best!")
  } else {
    character.ShowStatusMessage("Finished in ${clock(time)} (best ${clock(old.Value)})")
  }
  if holder == "" || time < record {
    record = time
    holder = escape(character.GetDisplayName())
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the four Attached Zones' **Zone Ref** outputs to `start`, `split1`,
   `split2` and `finish`.
3. Wire `sign` to the Text Decal's **Text**.
4. Make each zone span the whole track, so nobody can go around it.

## Check it in game

- [ ] The sign reads "No record yet".
- [ ] Walk through the start: "Go!".
- [ ] Walk through the first split: "Split 1: 00:04", or however long it took.
- [ ] Walk through the second split, then the finish: "Finished in 00:12, a
  new best!", and the sign shows your name and time.
- [ ] Walk from the start straight to the finish, around the splits:
  nothing happens. A run only counts with every split in order.
- [ ] Run again, slower: "Finished in 00:20 (best 00:12)", and the record
  stays.

## Change it

- Add a third split: a `split3` pin and handler calling
  `split(character, 3)`, and `SPLITS = 3`.
- Show each racer their best time on a status message when they walk into
  the start.
- Add a reset button that clears `best`, sets `holder` back to `""`, and so
  clears the record.

## How it works

Three [maps](../values/maps.md) keyed by `GetUserId()` hold each
racer's state: when they started, which split they must cross next, and
their best time. The `next` map is the guard. A split only counts when it is
the one expected, and the finish only when every split has been crossed, so
a shortcut or a wrong turn leaves the run unfinished. The finish removes the
racer's `next` entry, so walking through it again does nothing. `pad2` and `clock`
turn whole seconds into `mm:ss`, as in [Number
Options](../templates/number-options.md). Times come from `GetUnixTime()`,
whole seconds of the server's clock, which carries on through a restart.
The racer's name is escaped once, when it is stored ([Safe
Text](../strings/safe-text.md)).

Back to [Parts](README.md).
