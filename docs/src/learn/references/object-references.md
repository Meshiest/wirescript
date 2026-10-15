# Object References

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [True and False](../values/true-and-false.md)

**Next steps:** [Ref Parameters](ref-parameters.md), [Persistent Players](../admin/persistent-players.md), [Keeping It Current](../screen/keeping-it-current.md)

</div>
<!-- /learn-links -->

Some values are not numbers or text but things in the world: a player, a
player's body, a brick. A wire carries a reference to the thing, not a copy of
it. This lesson keeps a player in a variable to play tag.

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
zone "Zone Ref" -> chip base
chip sign -> decal Text
```

## Code

```wirescript name=r1-tag
import { escape } from "sanitize"

@left in base: zone
var it: character = null
var shown: string = "Nobody is it"
@right out sign = shown.Value

on ZoneEntered(zone = base) -> (character) {
  if character != it {
    let old = it
    it = character
    shown = "${escape(character.GetDisplayName())} is it"
    character.ShowChatMessage("You are it!")
    old.ShowChatMessage("You are free.")
  }
}
```

- `var it: character` holds a character: the body of the player who is "it".
- `= null` starts it empty. `null` is a reference to nothing.
- `character != it` asks whether the player who walked in is a different
  character from the one stored. It compares which character, not names.
- `let old = it` keeps the previous holder, so they can be told they are free
  after `it` has moved on.

The player who is it walks out and back in. What happens? Now they fall off
the map, respawn, and walk in again. What happens then?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   for the base, and place a brick with a **Text Decal**.
2. Wire the Attached Zone's **Zone Ref** to `base`, and `sign` to the Text
   Decal's **Text**.
3. Walk into the base: you get "You are it!" and the sign shows your name.
   Walk out and in again: nothing happens, you are already it.
4. A second player walks in: they are it, and you get "You are free."

## Under the Hood

Open the chip. `it` is one **Var** gate, and its value is a character. The
handler starts with one **Get Variable** that reads `it`. A **Not Equal**
compares that with the event's **Character**, and a **Branch** decides.
`old` adds no gate. It is that same first read.

Both **Show Chat Message** gates have a **Player State** input, the game's
name for a player. The character wire goes straight into it with no gate in
between, because a player input accepts a character.

The first time anyone walks in, `old` is `null`, so the last message has
nobody to go to. `!= null` asks whether a reference holds anything:

```wirescript name=r1-null
@left in base: zone
var it: character = null

on ZoneEntered(zone = base) -> (character) {
  if it != null {
    it.ShowChatMessage("You are free.")
  }
  it = character
}
```

In a comparison, `null` takes the type of the other side, here `character`.
The chip compares `it` with an empty character in a **Not Equal** gate.

References come in a few kinds:

| Type | What it is | Where a circuit gets one |
|---|---|---|
| `character` | a player's body, while they are alive | zone events, a Seat's **Sitting Character**, a Target's **Character That Just Hit** |
| `controller` | the player, the same through every death and respawn | `ControllerOf` (an admin gate); a character wired into a player input still works, but stays the body |
| `entity` | any object in the world: a player, a brick grid | any character, or `ReadBrickGrid()`, the grid the chip sits on |
| `brick` | one brick | `GetMicrochipBrick()`, the chip's own brick |

## Modify

1. Count the tags. Add `var tags: int = 0` and show it on the sign. The very
   first walk-in is not a tag, since nobody was it: add 1 to `tags` only
   `if old != null`.
2. Play tag with a gun instead. Replace the zone with a **Target**: shooting
   it makes you it.

   ```wirescript name=r1-target
   import { escape } from "sanitize"

   @left in hit: exec
   @left in shooter: character
   var it: character = null
   var shown: string = "Nobody is it"
   @right out sign = shown.Value

   on hit {
     if shooter != it {
       let old = it
       it = shooter
       shown = "${escape(shooter.GetDisplayName())} is it"
       shooter.ShowChatMessage("You are it!")
       old.ShowChatMessage("You are free.")
     }
   }
   ```

   Wire the Target's **Just Hit** to `hit` and its **Character That Just
   Hit** to `shooter`.
3. Show who is it on the chip itself. A brick is a reference too, and
   `GetMicrochipBrick()` is the chip's own brick. Add this line to the
   handler, after `it = character`:

   ```wirescript nocompile
   GetMicrochipBrick().SetBrickProperty(BrickProperty.Color, Color(1.0, 0.2, 0.2))
   ```

   `BrickProperty.Color` picks which setting of the brick to change, and
   `Color(1.0, 0.2, 0.2)` is a color made of red, green and blue amounts from
   0 to 1. The chip turns red at the first tag. Keep the brick in a variable first,
   `var chipBrick: brick`, set it with `chipBrick = GetMicrochipBrick()`, and
   recolor `chipBrick`. A brick can be stored like a character.

## Make

Make a king of the hill. A zone sits on top of a hill. The last player to
walk in is king, and a sign shows the king's name. A **Point Light** by the
hill is on while the king is standing in the zone and off when they leave.
Use `on ZoneLeft(zone = ...) -> (character)` and compare the character who
left with the king.

```wiring
zone "Zone Ref" -> chip hill
chip sign -> decal Text
chip lit -> light Enabled
```

## Good to Know

- A player who dies loses their character. Until they respawn they have no
  character at all, and when they do, they get a new one. `!=` treats the new
  one as a different character, so in the tag chip a respawned "it" who walks
  in is made it again.
- Do not keep a player's score under their character for the same reason. A
  later lesson, [Per-Player State](per-player-state.md), keys it by the
  player's user id instead.
- A Seat's **Sitting Character** is empty while nobody sits in it.
- The message to the old holder goes last on purpose. By then the new player
  is already it, whatever happens to a message sent to a character that may
  be gone.
- A character goes straight into any player input, so the
  `ControllerOf` and `CharacterOf` helpers are not needed to message them
  or read their keys. `ControllerOf` also uses a gate only an admin can load.
- A character wired in where a controller goes is still the character. It
  is not equal to that player's controller, and a stored one stops
  standing for the player at their next respawn.

See [Object references & assets](../../types.md#object-references--assets),
[The `null` literal](../../types.md#the-null-literal) and
[Bricks](../../types.md#bricks) in the reference.

Next: [Ref Parameters](ref-parameters.md).
