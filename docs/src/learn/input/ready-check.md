# Ready Check

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remote Control](remote-control.md), [Looping Over a List](../loops/looping-over-a-list.md), [Maps](../values/maps.md), [Choosing a Value](../logic/choosing-a-value.md), [Waiting](../events/waiting.md)

**Parts that use this:** [King of the Hill](../parts/king-of-the-hill.md), [Card Table](../parts/card-game.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

A game often waits until its players say they are ready. There are three
common ways to ask: one Button that everyone presses, seats that count as
ready while someone sits in them, and a `/ready` chat command. This lesson
builds the button version as a ready board for a room, then shows the other
two. The board changes only when something happens. There is no timer.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Ready by sitting](#ready-by-sitting)
- [Ready by chat command](#ready-by-chat-command)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip room
button "Press Count" -> chip holding
button Character -> chip presser
chip board -> decal Text
```

## Code

```wirescript name=i3-ready
import { escape } from "sanitize"

@left in room: zone
@left in holding: int
@left in presser: character

var players: character[]
var ready: Map<string, bool>
var last: int = 0
var shown: string = "Nobody here"
@right out board = shown.Value

let redraw: exec

on Change(holding) {
  if holding > last {
    let id = presser.GetUserId()
    let r = ready.get(id)
    ready.set(id, !(r.Found && r.Value))
    emit redraw
  }
  last = holding
}

on ZoneEntered(zone = room) -> (character) {
  emit redraw
}

on ZoneLeft(zone = room) -> (character) {
  ready.remove(character.GetUserId())
  buffer emit redraw
}

on redraw {
  players.fillFromZonePlayers(room)
  var text: string = ""
  var count: int = 0
  for p in players {
    let r = ready.get(p.GetUserId())
    let isReady = r.Found && r.Value
    if isReady {
      count += 1
    }
    let box = if isReady then "[x] " else "[ ] "
    text = "${text}${box}${escape(p.GetDisplayName())}<br>"
  }
  shown = "${count}/${players.length()} ready<br>${text}"
}
```

Three things can change the board, and each has a handler:

1. **A press.** `holding` is the Button's **Press Count**, the number of
   players holding it right now. It goes up when someone presses and down when
   they let go, and `on Change(holding)` runs on both. `holding > last` lets
   only a rise through, so each press counts once and a release does
   nothing. `presser`, the Button's **Character**, is the player who
   pressed. Their mark flips.
2. **Someone walks in.** They join the board, not ready.
3. **Someone walks out.** Their mark is removed, so they come back not
   ready.

Each handler ends by firing `redraw`, a signal of the chip's own. The
`redraw` handler fills `players` with everyone standing in the zone, walks
the list with `for p in players`, and writes one line per player and the
total. `<br>` starts a new line on a sign.

A player holds the button down for three seconds and lets go. How many
times does their mark flip?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   covering the room, put a **Button** inside the room, and place a brick
   with a **Text Decal** where the room can see it.
2. Wire the Attached Zone's **Zone Ref** to `room`, the Button's **Press
   Count** to `holding` and its **Character** to `presser`. Wire `board` to
   the Text Decal's **Text**.
3. Walk into the zone. The board lists you as `[ ]`. Press the button
   and you become `[x]`, and the count goes up. Press it again and you are
   back to `[ ]`.
4. Walk out and you leave the board. Walk back in and you are `[ ]` again.
5. With a second player, hold the button down while they press it. Their
   mark flips, and yours does not.

## Under the Hood

Open the chip. `on Change(holding)` is a **Change Detector (Exec)** on the
count. A **Get Variable** reads `last`, a **Greater** compares, and a
**Branch** picks the path. Both paths meet at a **Union** before `last` is
set. The two zone handlers are a **Player Entered Zone Event** and a
**Player Left Zone Event**. Exec wires cannot join on one input, so
**Union** gates merge the three `emit redraw`s into one chain. The leave
handler's `buffer emit` goes through a **Buffer (Ticks)** first, so its
redraw runs a tick later, once the zone has let the player go.

The redraw chain starts with a **Get Players In Zone** gate, which fills the
`players` **Array Var**. Then a **For Each Array** chip runs its inside once
per player, all within the same tick.

Why the count, and not the Button's **Held**? **Held** stays on while
anyone holds the button, so a second player pressing during that time does
not turn it on again. The count goes up on every press, and **Character**
changes to the player who made it.

The map is keyed by `GetUserId()`, not by the character, because a player who
dies comes back as a new character with the same user id. `var text` and
`var count` are declared inside the handler, so they start over on every
redraw.

## Ready by sitting

Seats need no button. A player is ready while they sit. Each **Seat**'s
**Is Occupied** says whether someone sits there, and its **Sitting
Character** says who:

```wirescript name=i3-seats
import { escape } from "sanitize"

const NEEDED = 2 // seated players needed to start

@left in sat1: bool
@left in who1: character
@left in sat2: bool
@left in who2: character
@left in sat3: bool
@left in who3: character
@right out go: exec

var name1: string = "-"
var name2: string = "-"
var name3: string = "-"
let seated = sat1 + sat2 + sat3
@right out board = "${seated}/3 seated<br>${name1.Value}<br>${name2.Value}<br>${name3.Value}"

on sat1 { name1 = escape(who1.GetDisplayName()) }
on !sat1 { name1 = "-" }
on sat2 { name2 = escape(who2.GetDisplayName()) }
on !sat2 { name2 = "-" }
on sat3 { name3 = escape(who3.GetDisplayName()) }
on !sat3 { name3 = "-" }

on seated >= NEEDED {
  emit go
}
```

Wire each Seat's **Is Occupied** and **Sitting Character** to a `sat` and
`who` pair, `board` to a sign, and `go` to whatever starts the game. A bool
adds as 1 or 0, so `seated` counts the occupied seats. `on seated >= NEEDED`
fires once when the count reaches `NEEDED`, and again only after it drops
below and comes back. Set `NEEDED = 3` to wait for every seat.

## Ready by chat command

Players can also type `/ready`. Add this handler to the button chip, and
each `/ready` flips the mark of the player who typed it:

```wirescript nocompile
on ChatCommand("ready", "Mark yourself ready, or not") -> (who, arguments) {
  let id = who.GetUserId()
  let r = ready.get(id)
  ready.set(id, !(r.Found && r.Value))
  emit redraw
}
```

**Chat Command** is an admin gate, so a chip with this handler loads only
for an admin. [Chat Commands](../admin/chat-commands.md) covers it.

## Modify

1. When everyone in the zone is ready, and there are at least two of them,
   show "All ready!" on the first line instead of the count.
2. Ignore presses from outside the room. In the press handler, fill
   `players` from the room first, and flip the mark only if
   `players.find(presser).Found`.
3. Use two buttons, Ready and Not Ready, instead of one that flips. Each
   needs its own **Press Count** and character pins, its own `last`, and
   its own `on Change(...)` handler.

## Make

Make a voting booth. Players in the room press a Yes button or a No button.
A player can change their vote, but only their last one counts, and walking
out takes the vote away. The board shows the yes and no totals, and nobody's
name.

```wiring
zone "Zone Ref" -> chip room
button1 "Press Count" -> chip yes
button1 Character -> chip yesBy
button2 "Press Count" -> chip no
button2 Character -> chip noBy
chip board -> decal Text
```

## Good to Know

- `Change` does not fire when the chip loads, so pasting it marks nobody.
- A display name is chosen by the player and can contain `<`. Escape it
  before it reaches a sign, as `escape` does here.
- The loop finishes within one tick, however many players there are, but
  its inside runs once per player on every redraw.
- Over a long session a zone event can be missed, and the board then
  stays out of date until the next press or arrival. Each redraw asks the
  zone who is inside, so firing `redraw` from a slow Clock as well keeps
  the board right.
- A button reads one press at a time. To follow the keys of every player in
  a room at once, see [One Chip per Player](../prefabs/one-chip-per-player.md).

See [Edge / Change Detectors](../../builtins.md#edge--change-detectors),
[Zone array fills](../../builtins.md#zone-array-fills-exec-array-methods) and
[`for` Loops](../../statements.md#for-loops) in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
