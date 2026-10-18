# Every Player

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Chat Commands](chat-commands.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Persistent Players](persistent-players.md), [Mods](../abstraction/mods.md)

**Next steps:** [Moderation](moderation.md)

</div>
<!-- /learn-links -->

Zones and seats hand a circuit the players who come to it. An admin circuit
can also ask for everyone on the server at once, and hear when players join,
leave, spawn and die. This lesson builds a board of who is online, then adds
the player events.

Needs admin to load: it uses **Fill Array From Players**, the player
events, **Chat Command Event** and the broadcast gates.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Player events](#player-events)
- [Finding a player](#finding-a-player)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
chip board -> decal Text
```

## Code

```wirescript name=a7-online admin
import { escape } from "sanitize"

var players: controller[]
var online: Map<string, string>
var shown: string = ""
@right out board = shown.Value

mod redraw() {
  var text: string = ""
  for id, name in online {
    text = "${text}${name}<br>"
  }
  shown = "${online.length()} online<br>${text}"
}

on Loaded() {
  players.fillFromPlayers()
  for p in players {
    online.set(p.GetUserId(), escape(p.GetDisplayName()))
  }
  redraw()
}

on ControllerJoined() -> (player, userId, userName) {
  online.set(userId, escape(player.GetDisplayName()))
  redraw()
}

on ControllerLeft() -> (userId, userName) {
  online.remove(userId)
  redraw()
}
```

- `players.fillFromPlayers()` replaces everything in `players` with every
  player on the server. It is `fillFromZonePlayers` without a zone.
- `on Loaded()` runs once, when the chip appears. It fills the list
  and stores each player's escaped name in `online`, under their user id.
- `on ControllerJoined` and `on ControllerLeft` are player events: the game
  runs them when a player joins or leaves the server. A join adds the new
  player's name, and a leave removes it by the id the event gives.
- `mod redraw() { ... }` names the steps that build the board, and each
  `redraw()` pastes them in ([Mods](../abstraction/mods.md)). The loop from
  [Looping Over a Map](../loops/looping-over-a-map.md) writes one name per
  line.

A friend joins while the board is up. How long until their name appears?
Another friend dies and is waiting to respawn. Are they still on the board?

## Run

1. Compile the chip and paste it in-game. Place a brick with a **Text Decal**.
2. Wire `board` to the Text Decal's **Text**.
3. Your name is on the board, and so is everyone else on the server. A player
   who joins appears at once, and one who leaves is gone at once.

## Under the Hood

Open the chip. **Read Brick Grid** starts a chain into **Fill Array From
Players**, whose **Array Var Ref** is wired from the `players` **Array
Var**. Its **Include Builder Mode** input is not wired, so it is off. Then a
**For Each Array** loop reads each player's **Get User Id** and **Get User Display Name** into a
**Set Map Element**. **Player Joined Event** and **Player Left Event** have no
inputs: the game fires them. Each of the three chains ends in its own copy of
`redraw`: a **For Each Map** loop and **Get Map Length** for the count.

The list holds players, not characters: the same references
[Persistent Players](persistent-players.md) made with `ControllerOf`. A dead
player is still a player, so they stay on the board while they wait to
respawn.

Nothing here runs on a timer. The board is rebuilt only when someone joins
or leaves, which is the only time it changes. A Clock that filled the list
every second would do the same work over and over, and still show a new
player up to a second late.

## Player events

The board uses two of the player events. There are more, and each fires for
every player in the world, with no wiring:

```wirescript name=a7-events admin
import { escape } from "sanitize"

var deaths: Map<string, int>

on ControllerJoined() -> (player, userId, userName) {
  BroadcastChatMessage("${escape(userName)} joined")
  player.ShowChatMessage("Welcome! Type /deaths to see how often you have died")
}

on ControllerLeft() -> (userId, userName) {
  BroadcastStatusMessage("${escape(userName)} left")
}

on CharacterDied() -> (character) {
  let id = character.GetUserId()
  let before = deaths.get(id)
  deaths.set(id, if before.Found then before.Value + 1 else 1)
}

on ChatCommand("deaths", "How often have you died?") -> (who, arguments) {
  let count = deaths.get(who.GetUserId())
  let n = if count.Found then count.Value else 0
  who.ShowChatMessage("You have died ${n} times")
}
```

- `ControllerJoined` gives the new player, already persistent, with their
  user id and user name.
- `ControllerLeft` gives only the id and the name, since the player is
  already gone.
- `CharacterDied` gives the character that died. Its user id is the key, so
  the count survives the respawn.
- `CharacterSpawned() -> (character)` is the fourth, for every new life.

`BroadcastStatusMessage` shows the line in everyone's status bar instead of
chat. Use chat for things to scroll back to, the status bar for things that
matter only now.

## Finding a player

`FindPlayer` from [Chat Commands](chat-commands.md) needs a name that
matches. With the whole list, a search can be looser: this `/find` matches
any part of a name, in any case, and lists everyone who fits.

```wirescript name=a7-find admin
import { escape } from "sanitize"

var players: controller[]

on ChatCommand("find", "Find a player by part of their name: /find sam") -> (who, arguments) {
  let query = arguments.Trim().ToLower()
  if query == "" {
    who.ShowChatMessage("Usage: /find part of a name")
    return
  }
  players.fillFromPlayers()
  var found: string = ""
  for p in players {
    let name = p.GetDisplayName()
    if name.ToLower().Contains(query) {
      found = "${found} ${escape(name)}"
    }
  }
  if found == "" {
    who.ShowChatMessage("Nobody matches")
  } else {
    who.ShowChatMessage("Found:${found}")
  }
}
```

The names are compared before they are escaped, and escaped only to be
shown.

## Modify

1. Mark idle players on the board. `GetUnixTime()` is the time in whole
   seconds, and `p.GetAim().Direction` is where a player looks. Remember both
   per user id, and show "(away)" after a minute without the view moving:

   ```wirescript name=a7-away admin
   import { escape } from "sanitize"

   const AFK_SECONDS = 60

   var players: controller[]
   var lastLook: Map<string, vector>
   var lastActive: Map<string, int>
   var shown: string = ""
   @right out board = shown.Value

   on Clock(interval = 1.0) {
     players.fillFromPlayers()
     let now = GetUnixTime()
     var text: string = ""
     for p in players {
       let id = p.GetUserId()
       let look = p.GetAim().Direction
       let before = lastLook.get(id)
       if !before.Found || look != before.Value {
         lastActive.set(id, now)
       }
       lastLook.set(id, look)
       let since = lastActive.get(id)
       let name = escape(p.GetDisplayName())
       if now - since.Value > AFK_SECONDS {
         text = "${text}${name} (away)<br>"
       } else {
         text = "${text}${name}<br>"
       }
     }
     shown = text
   }
   ```

2. If you have done [Ready Check](../input/ready-check.md), turn its ready
   board into one for the whole server: replace `fillFromZonePlayers(room)` with
   `fillFromPlayers()`, and make `players` a `controller[]`.
3. Add a death count to each name on the online board, from the `deaths` map
   of the second chip. The board now changes on a death too, so call
   `redraw()` from `on CharacterDied` as well.

## Make

Make a team picker sign: a button puts everyone on the server in a random
order with `players.shuffle()`, and the sign lists the first, third, fifth
and so on under Red and the rest under Blue. Nobody's real team changes. If
you have done [Teams](teams.md), add a second button that does change them,
with `SetTeam`.

```wiring
button1 Held -> chip shuffle
button2 Held -> chip assign
chip board -> decal Text
```

## Good to Know

- `fillFromPlayers()` leaves its **Include Builder Mode** input off, which
  should leave out players who are in builder mode. Check with a friend in
  builder mode before you count on it.
- The player events fire for everyone on the server, so a chip with
  `on CharacterDied` counts deaths anywhere in the world, not only near it.
- `ControllerLeft` has no player to give. Keep anything you need about a
  player under their user id while they are still there.
- The players in the list, and the one `ControllerJoined` gives, are
  controllers: values set on them with `SetCustomProperty` stay with the
  player through their deaths, until they leave the server ([Persistent
  Players](persistent-players.md#values-on-the-player)).
- A board that loops over every player costs one pass per player each time
  it is rebuilt. Rebuild it when it changes, not on a timer.

See [Array Methods](../../builtins.md#array-methods-exec),
[Broadcast Messaging](../../builtins.md#broadcast-messaging-exec) and
[Built-in Events](../../statements.md#built-in-events) in the reference.

Next: [Moderation](moderation.md).
