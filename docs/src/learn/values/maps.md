# Maps

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Lists](lists.md)

**Next steps:** [Looping Over a Map](../loops/looping-over-a-map.md), [Ready Check](../input/ready-check.md), [Per-Player State](../references/per-player-state.md)

**Parts that use this:** [Welcome Zone](../parts/welcome-zone.md), [Teleporter Pad](../parts/teleporter-pad.md), [Item Dispenser](../parts/item-dispenser.md), [Keycard Door](../parts/keycard-door.md), [Checkpoints](../parts/checkpoints.md), [Shop](../parts/shop.md)

</div>
<!-- /learn-links -->

A list finds a value by its slot number. Often what you know is something
else: which player walked in, or which team scored. A map files each value
under a key you choose, like a phone number filed under a name in an address
book. Give it the key and it gives back the value.

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
zone "Zone Ref" -> chip room
```

## Code

```wirescript name=v6-visits
import { escape } from "sanitize"

@left in room: zone
var visits: Map<string, int>

on ZoneEntered(zone = room) -> (character) {
  let id = character.GetUserId()
  let before = visits.get(id)
  let count = if before.Found then before.Value + 1 else 1
  visits.set(id, count)
  character.ShowChatMessage("Visit number ${count}, ${escape(character.GetDisplayName())}")
}
```

- `Map<string, int>` is a map with `string` keys and `int` values. The first
  type in the `<>` is the key, the second is the value.
- `character.GetUserId()` is the player's user id, a string that stays the
  same for that player on every visit. It is the key.
- `visits.get(id)` looks the key up and gives two things: `.Found`, whether
  the key was there at all, and `.Value`, the value filed under it.
- `if before.Found then before.Value + 1 else 1` picks one of two values: one
  more than the old count if the player has been here before, 1 if not.
- `visits.set(id, count)` files the new count under the id, replacing any old
  one.
- `escape(...)` makes a name the player chose safe to show in chat.

You walk into the zone three times, then a friend walks in for the first
time. What does the friend's message say?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**.
2. Wire the Attached Zone's **Zone Ref** to `room`.
3. Walk in and out a few times. Each visit, you get a chat message with your
   count. A second player gets their own count, starting at 1.

## Under the Hood

Open the chip. `visits` is one **Map Var** gate, and it stays one gate however
many players it holds. Its **Map Var Ref** output is wired into each gate that
reads or writes the map.

The handler is a chain. **Get User Id** turns the character into an id.
**Get Map Element** looks the id up, and **Set Map Element** files the new
count. Both take their **Key** from the same Get User Id. Between them, an
**Add** works out one more than the old count, and a **Select** picks it when
the **Found** output is true, or 1 when it is false. The message is a **Get
User Display Name**, a **Replace in String** for the escape, a **Format
String** and a **Show Chat Message**. That is eleven gates, plus the zone pin.

Like an array, a map has no pure gate that reads it. Every map gate has an
exec input, so a read outside a handler is an error:

```wirescript name=v6-pure-read expect=WS007
var visits: Map<string, int>
@right out first = visits.get("a").Value
```

```text
Error: [WS007] map read 'visits.get(...)' outside an exec context
```

To show something from a map on a sign, copy it into a `var` inside the
handler, and wire the sign from that.

Keys can be `int`, `string`, a reference to a thing in the world, or a
`brick`. Anything else is an error:

```wirescript name=v6-float-key expect=WS039
var heights: Map<float, int>
```

```text
Error: [WS039] map key type must be int, string, an object (entity/character/controller) or a brick, got float
```

## Modify

1. Show how many different players have visited. Add
   `var players: int = 0`, and at the end of the handler set
   `players = visits.length()`. `length()` gives the number of keys. Wire
   `@right out sign = "${players.Value} players have visited"` to a Text
   Decal.
2. Add a second zone that makes the chip forget you. `has` asks only whether
   a key is there, and `remove` takes a key and its value out:

   ```wirescript name=v6-forget
   @left in room: zone
   @left in forget: zone
   var visits: Map<string, int>

   on ZoneEntered(zone = room) -> (character) {
     let id = character.GetUserId()
     let before = visits.get(id)
     let count = if before.Found then before.Value + 1 else 1
     visits.set(id, count)
     character.ShowChatMessage("Visit number ${count}")
   }

   on ZoneEntered(zone = forget) -> (character) {
     let id = character.GetUserId()
     if visits.has(id) {
       visits.remove(id)
       character.ShowChatMessage("Forgotten.")
     } else {
       character.ShowChatMessage("I don't know you yet.")
     }
   }
   ```

   After the forget zone, your next visit is number 1 again.
3. Give prizes on some visits. The keys of a map do not have to be players:
   this one is keyed by the visit number, and only three numbers have a prize.
   A list would need a slot for every number in between.

   ```wirescript name=v6-prizes
   @left in room: zone
   var visits: Map<string, int>
   var prizes: Map<int, string> = { 1 => "a sticker", 5 => "a hat", 10 => "a crown" }

   on ZoneEntered(zone = room) -> (character) {
     let id = character.GetUserId()
     let before = visits.get(id)
     let count = if before.Found then before.Value + 1 else 1
     visits.set(id, count)
     let prize = prizes.get(count)
     if prize.Found {
       character.ShowChatMessage("Visit ${count}: you win ${prize.Value}!")
     }
   }
   ```

   `{ 1 => "a sticker", ... }` fills the map when you compile. Each
   `key => value` pair is one entry, and the entries are stored in the Map
   Var gate itself, so they cost no extra gates.

## Make

Make a lap counter. Walking into a finish zone adds a lap for that player and
tells them their lap count. On their third lap they get "You finished!" and
their entry is removed, so they can race again from lap 1. A sign shows how
many players are racing right now.

```wiring
zone "Zone Ref" -> chip finish
chip text -> decal Text
```

## Good to Know

- A map starts empty unless its declaration lists constant keys and values.
- A map holds one value per key. `set` on a key that is already there
  replaces its value.
- For a key that might be missing, check `.Found` or `has()` rather than
  counting on what `.Value` holds.
- Every `get` is its own gate and its own read. To use one read twice, bind
  it to a `let`, as `before` is above.
- Key players by `GetUserId()`, not by their character: a player who dies
  comes back as a new character. [Per-Player State](../references/per-player-state.md)
  compares the keys you could pick and when each one starts over.

See [Maps](../../builtins.md#maps-var-m-mapk-v) and
[Map Types](../../types.md#map-types-mapk-v) in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
