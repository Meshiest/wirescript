# Moderation

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Player Control](player-control.md), [Every Player](every-player.md)

</div>
<!-- /learn-links -->

A chat command answers whoever types it. A command for running the server
has to check who that is first. This lesson builds an `/a` channel that only
admins can use and read, and a `/reset` that only trusted builders can run.

Needs admin to load: it uses **Chat Command Event** and **Fill Array From
Players**.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Who may do what](#who-may-do-what)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

## Code

```wirescript name=a5-admin-chat admin
import { escape, sanitize } from "sanitize"

const ROLE = "Admin"
const ADMIN_COLOR = "ff8040"
const MAX_LEN = 200

var players: controller[]

on ChatCommand("a", "Message every admin: /a message") -> (who, arguments) {
  if !who.HasRole(ROLE) {
    who.ShowChatMessage("Only admins can use /a")
    return
  }
  let name = escape(who.GetDisplayName())
  let line = '<color="${ADMIN_COLOR}">[admin] ${name}:</> ${sanitize(arguments, MAX_LEN)}'
  players.fillFromPlayers()
  for p in players {
    if p.HasRole(ROLE) {
      p.ShowChatMessage(line)
    }
  }
}
```

- `who.HasRole(ROLE)` is true when the player has the server role named
  `Admin`. Anyone else is told no, and the handler stops.
- The line is built once: the sender's escaped name, then their message,
  cleaned with `sanitize`, in the colors from [Rich Text](../strings/rich-text.md).
- The loop from [Every Player](every-player.md) sends it to every player who
  has the role, the sender included.

A player without the role types `/a hello`. What do they see, and does any
admin see it? An admin types `/a` with nothing after it. What do the admins
get?

## Run

1. Check the name of your server's admin role in its role settings, and set
   `ROLE` to it.
2. Compile the chip and paste it in-game. Nothing needs wiring.
3. Type `/a testing`. You see the line in orange with your name. A friend
   without the role gets "Only admins can use /a", and nobody sees their
   message.

## Under the Hood

Open the chip. The chain starts at **Chat Command Event** and goes straight
into **Has Role**, whose **Player State** comes from the event and whose role
name is the constant `Admin`. A **NOT** of its **Has Role** output decides
the **Branch**: one side answers no, the other reads the name, calls
`sanitize`, then runs **Fill Array From Players** and the loop. Inside the loop, a second **Has Role** checks each
player.

**Has Role** itself is not admin. Any chip can check a role, so a circuit
anyone can paste can still treat admins differently. This visitor counter
leaves admins out of the count:

```wirescript name=a5-count-guests
@left in door: zone
var visits: int = 0
@right out sign = "Visitors: ${visits.Value}"

on ZoneEntered(zone = door) -> (character) {
  if character.HasRole("Admin") {
    return
  }
  visits += 1
}
```

## Who may do what

There are three checks on a player, and any chip can use them.

- `HasRole(name)`: the player has a role on this server, by its name.
- `HasPermission(name)`: the player has one of the game's gameplay
  permissions right now, the kind a team's settings grant and
  [`SetTempPermission`](player-control.md#temporary-permissions) overrides.
- `IsTrusted()`: the player is trusted by the owner of the chip's bricks.
  Use it for a build's own controls, so the people the owner builds with can
  use them too.

This `/reset` puts the visitor count back to 0, for trusted builders only:

```wirescript name=a5-reset admin
@left in door: zone
var visits: int = 0
@right out sign = "Visitors: ${visits.Value}"

on ZoneEntered(zone = door) {
  visits += 1
}

on ChatCommand("reset", "Reset the visitor count") -> (who, arguments) {
  if !who.IsTrusted() {
    who.ShowChatMessage("Only builders the owner trusts can reset the count")
    return
  }
  visits = 0
  who.ShowChatMessage("Visitor count reset")
}
```

## Modify

1. Refuse an empty message: if `arguments.Trim()` is empty, tell the sender
   how to use `/a` and stop.
2. Let `/reset` say who did it: send every admin a line naming the player
   who reset the count.
3. Add `/bench name` for admins: find the player with `FindPlayer`, stop them
   respawning with `SetCanRespawn(false)` as in
   [Player Control](player-control.md), and `/unbench name` to let them back.

## Make

Make a moderated sign. Any player can set it with `/sign text`. Only
trusted builders can lock it with `/sign lock` and unlock it with
`/sign unlock`. While it is locked, everyone else is told it is locked. Keep
the text short with `sanitize`.

```wiring
chip sign -> decal Text
```

## Good to Know

- A role is checked by its name, as typed in the server's role settings. A
  misspelled name is not an error: the check is false for everyone.
- Every check runs when the command is typed, so a player who gains or loses
  a role is treated by their new roles at once.
- A command is there for everyone to type. The check is the only thing that
  keeps it to some players, so put it first, before the command does
  anything.

See [Player info](../../builtins.md#player-info) and
[ChatCommand](../../builtins.md#chatcommand-event) in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
