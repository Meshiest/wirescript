# Persistent Players

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Admin Gates](admin-gates.md), [Object References](../references/object-references.md), [Looping Over a List](../loops/looping-over-a-list.md)

**Next steps:** [Player Control](player-control.md), [Every Player](every-player.md)

</div>
<!-- /learn-links -->

A character lasts one life. The player behind it stays for their whole visit.
To keep hold of a player through their deaths, a circuit asks the game for
the player itself, and that takes an admin gate. This lesson keeps a guest
list that survives a respawn, then shows how far you get without the admin
gate.

Needs admin to load: `ControllerOf` uses **Get Player (Persistent)**.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Without the admin gate](#without-the-admin-gate)
- [Values on the player](#values-on-the-player)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip door
button Held -> chip call
```

## Code

```wirescript name=a3-guests admin
@left in door: zone
@left in call: exec
var guests: controller[]

on ZoneEntered(zone = door) -> (character) {
  let p = character.ControllerOf()
  if !guests.find(p).Found {
    guests.push(p)
    p.ShowChatMessage("You are on the guest list")
  }
}

on call {
  for p in guests {
    p.ShowChatMessage("Dinner is ready!")
  }
}
```

- `character.ControllerOf()` gives the player whose character walked in.
  Unlike the character, the player is the same through every death.
- `guests` is a list of players. `guests.find(p).Found` is true when this
  one is on it already, so each player is added once.
- The button tells everyone on the list that dinner is ready, wherever they
  are.

A guest walks in, falls off the map and respawns somewhere else. Then you
press the button. Do they get the message? What if the chip had stored
`character` in the list instead?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   at the front door, and place a **Button**.
2. Wire the Attached Zone's **Zone Ref** to `door` and the Button's **Held**
   to `call`.
3. Walk through the door: chat says you are on the list. Walk through again:
   nothing, you are on it already.
4. Respawn, walk away, and press the button. The message still reaches you.

## Under the Hood

Open the chip. Between the zone event and **Find Array Element** sits **Get
Player (Persistent)**: the character goes in its **Entity** input, and its
**Player State** output is what the list stores. The loop is a **For Each
Array** microchip with one **Show Chat Message** inside.

Now store the character itself. The list's type still says `controller`, and
this compiles with no warning:

```wirescript name=a3-bodies
@left in door: zone
var guests: controller[]

on ZoneEntered(zone = door) -> (character) {
  guests.push(character)
}
```

Open it: there is no gate between the zone event and **Push To Array**. A
character goes into any player input, as
[Object References](../references/object-references.md) showed, but nothing
turns it into the player. The list holds the character, and after a respawn
that character is gone. A character is never equal to its player's
controller either, so `guests.find(...)` with one never matches the other,
and a character used as a `Map<controller, ...>` key is stored under that body.

The other way round, `p.CharacterOf()` gives a player's current character,
through **Get Player (Temporary)**. That gate is not admin. Use it for the
few things that need a body rather than a player, such as a hint:
`p.CharacterOf().ShowHint("Dinner", "Come to the table")`.

## Without the admin gate

Most circuits never need a persistent player. Three habits cover the rest:

1. **Use the character while you have it.** Everything a handler does to the
   player who walked in, it can do with the character, wired straight in.
2. **Remember players by user id.** `GetUserId()` is the same through deaths
   and visits, and makes a good key for a map or a list, as in
   [Per-Player State](../references/per-player-state.md).
3. **Let the players come back to you.** A zone gives you the current
   characters again each time you read it.

This guest list follows all three, and anyone can paste it. The button calls
only the guests standing in the house:

```wirescript name=a3-guest-ids
@left in door: zone
@left in house: zone
@left in call: exec
var guests: string[]
var here: character[]

on ZoneEntered(zone = door) -> (character) {
  let id = character.GetUserId()
  if !guests.find(id).Found {
    guests.push(id)
    character.ShowChatMessage("You are on the guest list")
  }
}

on call {
  here.fillFromZonePlayers(house)
  for p in here {
    if guests.find(p.GetUserId()).Found {
      p.ShowChatMessage("Dinner is ready!")
    }
  }
}
```

`here.fillFromZonePlayers(house)` fills `here` with every player standing in
the house zone, replacing what was there.

## Values on the player

A controller can hold values of its own. `SetCustomProperty` stores a value
on an entity under a name, and `GetCustomProperty` reads it back, with
`.Found` false when nothing is stored. On a controller the value stays with
the player through every death, so it works as per-player storage with no
map at all, for as long as they stay on the server:

```wirescript name=a3-visits admin
@left in door: zone

on ZoneEntered(zone = door) -> (character) {
  let p = character.ControllerOf()
  let before = p.GetCustomProperty("visits")
  let had: int = if before.Found then before.Value else 0
  p.SetCustomProperty("visits", had + 1)
  p.ShowChatMessage("Visit number ${had + 1}")
}
```

Store the values on the controller, not on the character: a property on the
character goes with the body at the next respawn.

## Modify

1. Add the hint to the first chip: in the loop, after the chat message,
   `p.CharacterOf().ShowHint("Dinner", "Come to the table")`.
2. Let the button empty the list after calling: `guests.clear()` after the
   loop.
3. In the id version, greet a guest who comes back through the door with
   "Welcome back" instead of adding them again.

## Make

Make a reservation desk. A zone at the desk adds the player to a list of at
most four. A button by the table seats the party: everyone on the list who
is standing at the table gets a chat message with their seat number, and
anyone missing is named on a sign. Do it without `ControllerOf`.

```wiring
zone1 "Zone Ref" -> chip desk
zone2 "Zone Ref" -> chip table
button Held -> chip seat
chip sign -> decal Text
```

## Good to Know

- A character dies with its player's life. A player who is dead has no
  character until they respawn, so `CharacterOf()` on them gives nothing to
  show a hint to.
- A `controller` variable or list holds whatever is wired into it. Given a
  character, it holds a character, which is not equal to the player's
  controller.
- Custom properties set on a controller stay with the player through their
  deaths, and are lost when they leave and reconnect. Set on a character,
  they are lost when that body respawns. To remember a player across a
  reconnect, use a map keyed by user id.
- A player who leaves the server cannot be reached through a stored
  reference, and one who comes back is a new player to the game. Keep the
  user id if you want to know them when they return.

See [ControllerOf](../../builtins.md#controllerof) and
[CharacterOf](../../builtins.md#characterof) in the reference.

Next: [Player Control](player-control.md).
