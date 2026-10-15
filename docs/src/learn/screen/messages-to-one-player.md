# Messages to One Player

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Text on Screen](text-on-screen.md)

</div>
<!-- /learn-links -->

A circuit can put words in front of one player without anyone else seeing
them: a line in their chat, a short status message, or a hint with a
title. This lesson builds a lava pit that greets, warns and helps each
player who walks in.

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
zone "Zone Ref" -> chip pit
```

## Code

```wirescript name=sc1-lava-pit
import { escape } from "sanitize"

@left in pit: zone

on ZoneEntered(zone = pit) -> (character) {
  character.ShowChatMessage("Welcome to the lava pit, ${escape(character.GetDisplayName())}.")
  character.ShowStatusMessage("Careful: the floor is lava")
  character.ShowHint("Crossing the Pit", "Jump from stone to stone. Do not touch the orange floor.")
}
```

All three go to `character`, the player who walked in:

- `ShowChatMessage` adds a line to that player's chat, like a whisper.
- `ShowStatusMessage` shows a short status message on their screen.
- `ShowHint` shows a hint: a title (the first text) and its words (the
  second).

The display name is escaped with `escape`, as in [Reacting to the
Game](../reacting-to-the-game.md), because the player chose it.

Three players walk into the pit together. How many messages does each one
get? Does anyone see another player's welcome?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached
   Zone** that covers the pit, and wire its **Zone Ref** to `pit`.
2. Walk into the pit. Your chat gets a line with your name, a status
   message warns you about the floor, and a hint titled "Crossing the Pit"
   appears.
3. Ask a second player to walk in while you watch. They get their own
   three messages, and nothing new appears on your screen.

## Under the Hood

Open the chip. The **Player Entered Zone Event** starts one exec chain:
**Get User Display Name**, **Show Chat Message**, **Show Status Message**,
then **Show Hint**, each **Exec Out** wired into the next one's **Exec**.
The event's **Character** wires into all of them. The two message gates
call that input **Player State**, and a character wires straight in.

The chat line is built from your name, so a **Replace in String** (from
`escape`) and a **Format String** sit beside the chain and feed **Show
Chat Message**'s **Message**. The status message has no wire on its
**Message**: a fixed text is typed into the gate.

**Show Hint** has no **Message** input at all. Its title and text are
settings typed into the gate, so they cannot change while the game runs.
Try to build the hint from a value:

```wirescript name=sc1-hint-wired expect=WS028
@left in pit: zone
@left in stones: int

on ZoneEntered(zone = pit) -> (character) {
  character.ShowHint("Crossing the Pit", "There are ${stones} stones")
}
```

```text
Error: [WS028] 'text' is constant-only gate config and cannot take a variable or computed value
```

Pick the call by whether the words need to change:

| Call | Gate | The words can change |
|---|---|---|
| `ShowChatMessage(text)` | **Show Chat Message** | yes, wired into **Message** |
| `ShowStatusMessage(text)` | **Show Status Message** | yes, wired into **Message** |
| `ShowHint(title, text)` | **Show Hint** | no, fixed when you compile |

To tell every player at once there are **Broadcast Chat Message** and
**Broadcast Status Message**, but both are admin gates
([Admin Gates](../admin/admin-gates.md)). For text that stays put on the
screen, where you choose, the next lesson uses `DisplayText`.

## Modify

1. Say goodbye. Add a second handler that runs when a player leaves:

   ```wirescript name=sc1-goodbye
   @left in pit: zone

   on ZoneLeft(zone = pit) -> (character) {
     character.ShowStatusMessage("You made it out")
   }
   ```

   Walk out of the pit: a status message says goodbye.

2. Count the crossings. Add `var crossings: int = 0` and, in the enter
   handler, `crossings += 1`. Put the count in the status message:
   `"Careful: the floor is lava (crossing ${crossings})"`. Try the same in
   the hint, and read the error.

3. Add a help button at the edge of the pit. Wire the **Button**'s **Press
   Count** to `@left in help: int` and its **Character** to `@left in
   asker: character`. **Press Count** is how many players are holding the
   button right now, so it goes up on a press and down on a release.
   `on Change(help)` runs on both, and `help > last` keeps only the
   presses, by anyone. **Character** is the player who pressed:

   ```wirescript name=sc1-help
   @left in help: int
   @left in asker: character
   var last: int = 0

   on Change(help) {
     if help > last {
       asker.ShowHint("Crossing the Pit", "The stones that look cracked will sink.")
     }
     last = help
   }
   ```

## Make

Make a shop doorway. A **Switch** behind the counter marks the shop open or
closed. Every player who walks through the door gets a hint with the
opening hours, and a status message that says `Open` or `Closed`, read
from the switch (`if ... then ... else` picks one of two values). Check it
with the switch off: walk in and read `Closed`. Then flip it, walk out and
back in, and read `Open`.

```wiring
zone "Zone Ref" -> chip door
switch Enabled -> chip open
```

## Good to Know

- Each of these reaches one player only. Nobody else sees it.
- A hint's title and text are fixed when you compile (`WS028`). Put
  anything that changes in a chat or status message instead.
- The **Player Left Zone Event** also fires when a character is destroyed
  inside the zone, for example by dying there, and then its **Character**
  is empty. A goodbye sent to an empty character reaches no one.
- Chat and status messages read the same markup as other game text: `<b>`,
  `<color="...">` and the rest ([Rich Text](../strings/rich-text.md)).
- `ShowHint` takes a character, and the two message calls take a player
  state. A character wires into either.

See [ShowChatMessage](../../builtins.md#showchatmessage),
[ShowStatusMessage](../../builtins.md#showstatusmessage) and
[ShowHint](../../builtins.md#showhint) in the reference.

Next: [Text on Screen](text-on-screen.md).
