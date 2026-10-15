# Keeping It Current

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Text on Screen](text-on-screen.md), [Object References](../references/object-references.md)

**Next steps:** [A HUD for Everyone](a-hud-for-everyone.md)

**Parts that use this:** [Card Table](../parts/card-game.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

Text that shows a changing value has to change with it. It is drawn when
it is needed, redrawn when the value changes, and taken down when it is
not. This lesson builds a control seat whose panel shows the
state of a gate and how many players are waiting at it, for as long as
someone sits there.

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
seat "Is Occupied" -> chip seated
seat "Sitting Character" -> chip sitter
switch Enabled -> chip gate
zone "Occupant Count" -> chip waiting
```

## Code

```wirescript name=sc3-panel
const PANEL = 7310 // a text id no other circuit uses

@left in seated: bool
@left in sitter: character
@left in gate: bool
@left in waiting: int
var rider: character

let state = if gate then '<color="44dd44">OPEN' else '<color="dd4444">SHUT'
let panel = 'Gate: ${state}<br><color="ffffff">Waiting: ${waiting}'

on seated {
  rider = sitter
  sitter.DisplayText(panel, textId = PANEL, lifetime = 0.0,
    anchorX = 0.0, anchorY = 0.0, positionX = 40.0, positionY = 40.0,
    justify = Left, fontSize = 24)
}

on Change(panel) {
  if seated {
    rider.DisplayText(panel, textId = PANEL, lifetime = 0.0,
      anchorX = 0.0, anchorY = 0.0, positionX = 40.0, positionY = 40.0,
      justify = Left, fontSize = 24)
  }
}

on !seated {
  rider.DisplayText("", textId = PANEL)
}
```

The three handlers draw, redraw and remove the text:

- **Sit down** (`on seated`): remember who sat (`rider`) and draw the
  panel for them.
- **The panel changes** (`on Change(panel)`): the gate moved or the queue
  grew, so the words are different. If someone is seated, draw it again.
- **Stand up** (`on !seated`, when `seated` turns off): draw an empty text
  over it, which takes it down.

All three use `textId = PANEL`, so they all mean the same text on that
player's screen. A draw with a `textId` that is already showing replaces
that text instead of adding another. `lifetime = 0.0` means the text
never runs out on its own: it stays until something replaces it.

You sit down, and the gate opens and closes three times while you sit.
How many texts from this chip are on your screen at the end? What would
it be without the `textId`?

## Run

1. Compile the chip and paste it in-game. Place a **Seat**, a **Switch**
   for the gate, and a brick with an **Attached Zone** over the spot where
   players wait.
2. Wire the Seat's **Is Occupied** to `seated` and its **Sitting
   Character** to `sitter`.
3. Wire the Switch's **Enabled** to `gate`, and the Attached Zone's
   **Occupant Count** to `waiting`.
4. Sit down. The top left corner shows the panel: `Gate: SHUT` in red and
   `Waiting: 0` under it.
5. Ask a friend to flip the switch and stand in the waiting zone. The
   panel changes as they do: `OPEN` in green, then `Waiting: 1`. It stays
   one panel, in one place.
6. Stay seated for a minute with nothing changing: the panel stays up.
   Stand up: it is gone at once.

## Under the Hood

Open the chip. There are three **Display Text** gates, all with **Text
Id** 7310. The two that draw have **Lifetime** 0 and take their **Text**
from the same **Format String**. The third has an empty **Text**. A
**Change Detector (Exec)** watches the Format String's output, so it
fires only when the panel would read differently, and a **Branch** checks
`seated` before the redraw.

### One id, one text

With no `textId` (the default, 0) each **Display Text** gate draws a text
of its own. Leave the ids out and the redraw gate would put a second panel
on top of the first, and the empty draw would take down neither. The shared
id lets three gates work on one text.

Ids are shared further than that: every circuit that draws on a player's
screen uses the same numbers. Two circuits that both use 7310 replace each
other's text. Pick ids for each circuit that are unlikely to clash, and
keep them in a `const` at the top.

### Lifetime and redraws

The default lifetime is 5 seconds. Leave out `lifetime = 0.0` and the
panel vanishes 5 seconds after you sit down, then comes back only when the
gate or the queue changes. Text that is redrawn on a change should have
lifetime 0, and text with lifetime 0 needs a draw that takes it down.
Every id this chip draws has one: the empty draw in `on !seated`.

The other way to keep text up is to redraw it on a `Clock`, with a lifetime
a little longer than the interval. That keeps building and sending the
same words when nothing has changed, and it still shows a change late, by
up to one interval. `Change` redraws once, right when the words change.

### One text or several

The panel's two lines are one text, joined by `<br>`. A player's screen
shows only about 32 texts at once, and every circuit they see shares that
budget. Past it, a text silently never appears, and which one is missing
changes when you change something unrelated. Two lines in one text spend
one of the 32. Use separate texts only for things that must sit in
different places on the screen.

## Modify

1. Add a third line for the lights. Wire a second Switch's **Enabled** to
   `@left in lights: bool`, pick a word with `if`, and add
   `<br>Lights: ${lightWord}` to `panel`. It is still one text and one id.

2. Delete `lifetime = 0.0` from both draws. Sit down and wait: the panel
   goes after 5 seconds. Flip the gate switch, and it comes back for 5
   more. Put it back.

3. Use a control room instead of a seat. This version shows one player in
   the room a panel, and takes it down when they walk out:

   ```wirescript name=sc3-room
   const PANEL = 7311

   @left in room: zone
   @left in gate: bool
   var watcher: character = null

   let panel = if gate then "Gate: OPEN" else "Gate: SHUT"

   on ZoneEntered(zone = room) -> (character) {
     if watcher != null && watcher != character {
       watcher.DisplayText("", textId = PANEL)
     }
     watcher = character
     character.DisplayText(panel, textId = PANEL, lifetime = 0.0)
   }

   on Change(panel) {
     if watcher != null {
       watcher.DisplayText(panel, textId = PANEL, lifetime = 0.0)
     }
   }

   on ZoneLeft(zone = room) -> (character) {
     if character == watcher || character == null {
       watcher.DisplayText("", textId = PANEL)
       watcher = null
     }
   }
   ```

   `watcher` holds the player who has the panel, and `null` when nobody
   does ([Object References](../references/object-references.md)). `&&`
   is "and", and `||` is "or". It keeps
   one `watcher`, so a second player who walks in takes the panel over, and
   the first one's text is taken down first: with lifetime 0 it would
   otherwise stay on their screen for good. [A HUD for
   Everyone](a-hud-for-everyone.md) gives each player in the room their own.

## Make

Make a shop clerk's desk. A **Seat** behind the counter shows the seated
clerk how many items are left in stock, a `var` that starts at 10.
Customers buy with a **Button**: wire its **Held** to an `exec` pin and take
one item off in `on` that pin.
At 0, the line turns red and reads `Sold out`. Check it with the clerk
seated: a customer presses and the count drops at once. The clerk stands
up and the text is gone. The clerk sits back down and sees the current
count.

```wiring
seat "Is Occupied" -> chip seated
seat "Sitting Character" -> chip clerk
button Held -> chip buy
```

## Good to Know

- `lifetime = 0.0` means forever, not "vanish now". Every id you draw with
  lifetime 0 needs a path that draws `""` over it. If the chip is removed
  while the text is up, nothing will.
- A text id is a number shared by every circuit on that player's screen.
  Two circuits that pick the same id overwrite each other.
- About 32 texts per player, shared by every circuit. Count the texts on
  screen at one moment, not the **Display Text** gates.
- The **Player Left Zone Event** gives an empty **Character** when a
  player dies inside the zone. The room version treats an empty one like
  the watcher leaving, and takes the text down through `watcher`, the one
  it stored on the way in.
- `on !seated` runs when `seated` turns off.

See [DisplayText](../../builtins.md#displaytext) and [Default
values](../../game-knowledge.md#default-values) in the reference.

Next: [A HUD for Everyone](a-hud-for-everyone.md).
