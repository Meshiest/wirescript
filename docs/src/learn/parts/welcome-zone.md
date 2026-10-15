# Welcome Zone

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Maps](../values/maps.md)

</div>
<!-- /learn-links -->

A zone at the entrance of a build that greets each player. A first-time
visitor gets a hint with the rules and a chat welcome. A returning one gets a
short "welcome back". A sign counts the different visitors.

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

- a brick with an **Attached Zone** covering the entrance
- a brick with a **Text Decal**, for the visitor count

```wiring
zone "Zone Ref" -> chip room
chip sign -> decal Text
```

## Code

```wirescript name=pp2-welcome
import { escape } from "sanitize"

// The name of the place, shown in the hint title.
const PLACE = "the workshop"
// The hint text a first-time visitor sees.
const RULES = "Be kind. Build freely. Ask before editing."

@left in room: zone

var visits: Map<string, int>
var guests: int = 0
@right out sign = "${guests.Value} visitors so far"

on ZoneEntered(zone = room) -> (character) {
  let id = character.GetUserId()
  let before = visits.get(id)
  let count = if before.Found then before.Value + 1 else 1
  visits.set(id, count)
  let name = escape(character.GetDisplayName())
  if count == 1 {
    let number = guests + 1
    guests = number
    character.ShowHint("Welcome to ${PLACE}", RULES)
    character.ShowChatMessage("Welcome, ${name}! You are visitor number ${number}.")
  } else {
    character.ShowStatusMessage("Welcome back, ${name} (visit ${count})")
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Attached Zone's **Zone Ref** to `room`.
3. Wire `sign` to the Text Decal's **Text**.

## Check it in game

- Before anyone walks in: the sign says `0 visitors so far`.
- Walk into the zone: a hint pops up titled "Welcome to the workshop" with
  the rules, a chat line welcomes you by name as visitor number 1, and the
  sign says `1 visitors so far`.
- Walk out and back in: no hint, a status message says "Welcome back" with
  visit 2, and the sign still says 1.
- With a second player: their first visit makes them visitor number 2.

## Change it

- Change `PLACE` and `RULES` to your build's name and rules.
- Fix the "1 visitors" grammar with a word form: build the sign with
  `Fmt("{A}|plural(one={A} visitor,other={A} visitors) so far", guests.Value)`
  ([Word Forms](../templates/word-forms.md)).
- Only greet returning players once a day: store `GetUnixTime()`, the
  server's clock in whole seconds, per player in a second map, and skip the message when the last visit was less than
  86400 seconds ago.

## How it works

The zone event gives the player who walked in
([Reacting to the Game](../reacting-to-the-game.md)). The visit count is a
map keyed by the player's user id, which stays the same across respawns and
visits. `.Found` tells a first visit from a missing key
([Maps](../values/maps.md)). `if ... then ... else`
picks one of two values. The display name is escaped before it is shown,
because a player chooses it ([Safe Text](../strings/safe-text.md)). The hint,
the chat line and the status message each go to that one player only.
