# Obstacle Course

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Custom Events](../events/custom-events.md), [Mods](../abstraction/mods.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Number Options](../templates/number-options.md), [Safe Text](../strings/safe-text.md)

</div>
<!-- /learn-links -->

A timed course with a start, three checkpoints taken in order, a finish, and
a fall zone that puts players back at their last checkpoint. A board by the
start keeps each player's best time and the course record.

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

- Five bricks with an **Attached Zone** each: the start, three checkpoints
  and the finish.
- One **Attached Zone** under the course, where a falling player lands.
- A brick with a **Text Decal** for the board, and a **Button** to clear it.

```wiring
chip "Course"
zone1 "Zone Ref" -> chip start
zone2 "Zone Ref" -> chip cp1
zone3 "Zone Ref" -> chip cp2
zone4 "Zone Ref" -> chip cp3
zone5 "Zone Ref" -> chip finish
zone6 "Zone Ref" -> chip fall
```

```wiring
chip "Board"
button Held -> chip reset
chip board -> decal Text
```

## Code

The part is two chips. The course chip watches the zones. The board chip
needs no wire to it: when a player finishes, the course chip sends a custom
event, and the board hears it.

The course chip:

```wirescript name=cp1-course
const CHECKPOINTS = 3 // checkpoint zones between the start and the finish

@left in start: zone
@left in cp1: zone
@left in cp2: zone
@left in cp3: zone
@left in finish: zone
@left in fall: zone

var startedAt: Map<string, int>
var reached: Map<string, int>
var spot: Map<string, vector>
var falls: Map<string, int>

mod elapsed(id: string) -> int {
  let began = startedAt.get(id)
  return GetUnixTime() - began.Value
}

mod checkpoint(who: character, n: int) {
  let id = who.GetUserId()
  let at = reached.get(id)
  if !at.Found || at.Value != n - 1 {
    return
  }
  reached.set(id, n)
  spot.set(id, who.GetLocation())
  who.ShowStatusMessage("Checkpoint ${n} of ${CHECKPOINTS}, ${elapsed(id)}s")
}

on ZoneEntered(zone = start) -> (character) {
  let id = character.GetUserId()
  startedAt.set(id, GetUnixTime())
  reached.set(id, 0)
  spot.set(id, character.GetLocation())
  falls.set(id, 0)
  character.ShowStatusMessage("Go!")
}

on ZoneEntered(zone = cp1) -> (character) { checkpoint(character, 1) }
on ZoneEntered(zone = cp2) -> (character) { checkpoint(character, 2) }
on ZoneEntered(zone = cp3) -> (character) { checkpoint(character, 3) }

on ZoneEntered(zone = fall) -> (character) {
  let id = character.GetUserId()
  let saved = spot.get(id)
  if !saved.Found {
    return
  }
  character.SetLocation(saved.Value)
  let at = reached.get(id)
  if !at.Found {
    return
  }
  if at.Value == 0 {
    startedAt.set(id, GetUnixTime())
    character.ShowStatusMessage("Back to the start. Go!")
    return
  }
  let f = falls.get(id)
  falls.set(id, f.Value + 1)
  character.ShowStatusMessage("Back to checkpoint ${at.Value}")
}

on ZoneEntered(zone = finish) -> (character) {
  let id = character.GetUserId()
  let at = reached.get(id)
  if !at.Found || at.Value != CHECKPOINTS {
    return
  }
  let time = elapsed(id)
  let f = falls.get(id)
  reached.remove(id)
  SendCustomEvent("course.finish", character, time, f.Value)
}
```

The board chip:

```wirescript name=cp1-board
import { escape } from "sanitize"

const TITLE = "<b>Obstacle Course</b>" // the first line of the board

@left in reset: exec

var best: Map<string, int>
var names: Map<string, string>
var record: int = 0
var holder: string = ""
var shown: string = "No finishers yet"
@right out board = "${TITLE}<br>${shown.Value}"

mod pad2(n: int) -> string {
  let s = "0${n}"
  return s.Substring(s.Length() - 2, 2)
}

mod clock(seconds: int) -> string {
  return "${pad2(seconds / 60)}:${pad2(seconds % 60)}"
}

on CustomEvent("course.finish") -> (runner: character, time: int, falls: int) {
  let id = runner.GetUserId()
  let name = escape(runner.GetDisplayName())
  names.set(id, name)
  let old = best.get(id)
  if !old.Found || time < old.Value {
    best.set(id, time)
    runner.ShowStatusMessage("Finished in ${clock(time)}, falls: ${falls}. A new best!")
  } else {
    runner.ShowStatusMessage("Finished in ${clock(time)}, falls: ${falls}. Best: ${clock(old.Value)}")
  }
  if holder == "" || time < record {
    record = time
    holder = name
  }
  var text: string = "Record: ${holder} ${clock(record)}"
  for key, t in best {
    let n = names.get(key)
    text = "${text}<br>${n.Value}: ${clock(t)}"
  }
  shown = text
}

on reset {
  best.clear()
  names.clear()
  record = 0
  holder = ""
  shown = "No finishers yet"
}
```

## Wire it

1. Compile both chips and paste them in-game: the course chip near the
   course, the board chip by the Text Decal.
2. Wire the zones' **Zone Ref** outputs to the course chip: the start to
   `start`, the checkpoints to `cp1`, `cp2` and `cp3` in course order, the
   finish to `finish`, and the zone under the course to `fall`.

3. Wire the board chip's `board` to the Text Decal's **Text**, and the
   Button's **Held** to `reset`.

4. Make every checkpoint zone span the whole course, so nobody can go around
   one, and the fall zone wide enough to catch everyone.

Paste both chips yourself: custom events reach only circuits pasted by the
same player.

## Check it in game

- [ ] The board reads "Obstacle Course" and "No finishers yet".
- [ ] Walk into the start: "Go!".
- [ ] Jump off before the first checkpoint: you are back at the start, and
  the run starts over.
- [ ] Reach checkpoint 1: "Checkpoint 1 of 3, 6s", or however long it took.
- [ ] Jump off: "Back to checkpoint 1", and you stand where you reached it.
- [ ] If your course lets you reach checkpoint 3 before checkpoint 2, try
  it: nothing happens. Checkpoints count only in order.
- [ ] Take checkpoints 2 and 3, then the finish: "Finished in 00:31,
  falls: 1. A new best!", and the board shows the record and your line.
- [ ] Walk into the finish again: nothing. Fall again: you are back at
  checkpoint 3, with no message.
- [ ] Run again, slower: the status shows your best, and the board keeps it.
- [ ] Press the button: the board is cleared.

## Change it

- Add a checkpoint: a `cp4` pin, a handler that calls
  `checkpoint(character, 4)`, and `CHECKPOINTS = 4`.
- Add a second course: paste another course chip with its own zones, and a
  channel name of its own (`"course2.finish"`) for a second board.
- Make falls cost time: on the board, use `time + falls * 5` everywhere
  `time` is compared or stored.

## How it works

This is [Checkpoints](checkpoints.md) and [Race Timer](race-timer.md) joined
into one course. The course chip keeps each runner's state in four
[maps](../values/maps.md) keyed by `GetUserId()`
([Per-Player State](../references/per-player-state.md)), so a runner who
respawns keeps their progress. The maps hold when they started, the last
checkpoint reached, where they stood there, and how often they fell.
`reached` is the guard: a checkpoint counts only when it is the next one,
and the finish only when all three are reached. The start records a spot
too, so a fall before checkpoint 1 sends the runner to the start and
restarts their clock. The finish removes the runner's `reached` entry, so a
second finish does nothing, and a later fall only puts them back at the
last checkpoint.

The finish sends one [custom event](../events/custom-events.md) carrying
the runner's character, their time and their falls. The board uses the
character to read the name, escaped ([Safe Text](../strings/safe-text.md)),
and to answer with a status message. Its lines come from a [loop over the
map](../loops/looping-over-a-map.md) of best times, and `clock` writes
seconds as `mm:ss` ([Number Options](../templates/number-options.md)).
Times come from `GetUnixTime()`, in whole seconds. A send gate delivers
once per tick, so if two runners finish in the same tick, the board may
hear only one of them.

Back to [Parts](README.md).
