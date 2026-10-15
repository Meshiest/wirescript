# Keycard Door

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Maps](../values/maps.md), [Waiting](../events/waiting.md)

</div>
<!-- /learn-links -->

A door that opens only for players who have picked up the keycard. Press the
keycard button somewhere in your build, then walk up to the door: it opens
for you, and stays shut for everyone without a card.

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

- a **Button**, where players pick up the keycard
- a brick with an **Attached Zone** in front of the door
- a door: a few bricks, one of them with a **Brick Property Changer**
- a second **Button**, to take every keycard back (put it somewhere only you
  can reach)

```wiring
button1 "Press Count" -> chip holding
button1 Character -> chip grabber
zone "Zone Ref" -> chip door
button2 Held -> chip reset
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Code

```wirescript name=pp11-keycard
// Seconds the door stays open for a card holder.
const OPEN_SECONDS = 4.0

@left in holding: int
@left in grabber: character
@left in door: zone
@left in reset: exec

var cards: Map<string, bool>
var opened: bool = false
var last: int = 0
@right out solid = !opened.Value

on Change(holding) {
  if holding > last {
    cards.set(grabber.GetUserId(), true)
    grabber.ShowStatusMessage("You picked up the keycard")
  }
  last = holding
}

on ZoneEntered(zone = door) -> (character) {
  if cards.has(character.GetUserId()) {
    opened = true
    await Sleep(_, delay = OPEN_SECONDS)
    opened = false
  } else {
    character.ShowStatusMessage("Locked. You need the keycard.")
  }
}

on reset {
  cards.clear()
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the keycard Button's **Press Count** to `holding` and its
   **Character** to `grabber`.
3. Wire the Attached Zone's **Zone Ref** to `door`.
4. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility** inputs.
5. Wire the second Button's **Held** to `reset`.

## Check it in game

- Walk up to the door without a card: it stays shut, and a status message
  says you need the keycard.
- Press the keycard button: a status message says you picked it up.
- Walk up to the door again: it opens, and four seconds later it shuts.
- Respawn and walk up again: it still opens. The card belongs to the
  player, not to the character.
- With a second player who has no card, walking up alone: the door stays
  shut for them. While it is open for a card holder, anyone can walk
  through.
- Press the reset button: the door is locked for everyone again.

## Change it

- Change `OPEN_SECONDS`.
- Make the card single use: add `cards.remove(character.GetUserId())`
  right after `opened = true`.
- Several doors, several cards: give each card a name, store it in a
  `Map<string, string>`, and let each door check for its own name.
- Show who holds a card on a sign: loop over `cards` with
  `for id, has in cards` ([Looping Over a Map](../loops/looping-over-a-map.md)),
  with a second map from user id to display name.

## How it works

There is no keycard item. The card is an entry in a map keyed by the player's
user id ([Maps](../values/maps.md)). The button's
**Character** says who pressed it last, and the zone event says who walked
up ([Reacting to the Game](../reacting-to-the-game.md)). The button's
**Press Count** is how many players hold it right now. The grab handler runs
on every change of it ([Change](../../builtins.md#edge--change-detectors)),
and `holding > last` keeps only the rises. A press counts even while someone
else holds the button and **Held** stays on, and letting go does nothing.
`has()` asks whether
that player's key is in the map. A card holder starts the same timed opening
as an auto-closing door ([Waiting](../events/waiting.md)). Because the user
id stays the same across respawns, the card survives a death. `reset` clears
the map to take every card back.
