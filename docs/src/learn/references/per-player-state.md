# Per-Player State

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Ref Values](ref-values.md), [Maps](../values/maps.md)

**Next steps:** [Multiplayer Architecture](../mastery/multiplayer.md), [A HUD for Everyone](../screen/a-hud-for-everyone.md)

**Parts that use this:** [Shooting Gallery](../parts/shooting-gallery.md), [Checkpoints](../parts/checkpoints.md), [Race Timer](../parts/race-timer.md), [Scoreboard HUD](../parts/scoreboard-hud.md), [King of the Hill](../parts/king-of-the-hill.md), [Shop](../parts/shop.md), [Voting](../parts/voting.md), [Obstacle Course](../parts/obstacle-course.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

A circuit that remembers something for each player stores it in a map, under
a key that stands for the player. Which key you pick decides when the memory
starts over. This lesson counts target hits per player for good, and per
life beside it.

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
target "Just Hit" -> chip hit
target "Character That Just Hit" -> chip shooter
chip sign -> decal Text
```

## Code

```wirescript name=r4-hits
import { escape } from "sanitize"

@left in hit: exec
@left in shooter: character
var hits: Map<string, int>
var lifeHits: Map<string, int>
var bodies: Map<string, character>
var shown: string = "No hits yet"
@right out sign = shown.Value

on hit {
  let id = shooter.GetUserId()
  let before = hits.get(id)
  let total = if before.Found then before.Value + 1 else 1
  let body = bodies.get(id)
  let sameLife = body.Found && body.Value == shooter
  let lifeBefore = lifeHits.get(id)
  let life = if sameLife then lifeBefore.Value + 1 else 1
  hits.set(id, total)
  lifeHits.set(id, life)
  bodies.set(id, shooter)
  shown = "${escape(shooter.GetDisplayName())}: ${total} hits, ${life} this life"
}
```

- `shooter.GetUserId()` is the player's user id, a string that belongs to
  their account. All three maps are keyed by it.
- `hits` counts every hit the player has made.
- `bodies` stores the character the player had at their last hit. On the next
  hit, `body.Value == shooter` is true only while they are still in that same
  body.
- `lifeHits` counts on while `sameLife` holds, and starts again at 1 when it
  does not.

You hit the target three times, fall off the map, respawn, and hit it once
more. What does the sign say?

## Run

1. Compile the chip and paste it in-game. Place a **Target** and a brick with
   a **Text Decal**.
2. Wire the Target's **Just Hit** to `hit`, its **Character That Just Hit**
   to `shooter`, and `sign` to the Text Decal's **Text**.
3. Shoot the target three times: `3 hits, 3 this life`. Die and respawn, and
   shoot once more: `4 hits, 1 this life`. A second player's hits start their
   own counts.

## Under the Hood

Open the chip. There are three **Map Var** gates. One **Get User Id** gate
turns the shooter into an id, and every **Get Map Element** and **Set Map
Element** takes its **Key** from it. A map can hold references as well as
numbers. The `bodies` map's values are characters, and an **Equal** compares
the stored one with the shooter.

Each key starts over at a different time:

| Key | Stays the same | Starts over |
|---|---|---|
| `GetUserId()` | through deaths and respawns | never: it belongs to the account |
| the `controller` | through deaths and respawns | it is the player, but getting it from a character needs the admin `ControllerOf` |
| the `character` | while the player stays alive | at every respawn |

So `hits` is keyed by the user id, and the per-life count compares
characters. Keying `hits` by the character would quietly reset a player's
score every time they died, and leave an entry behind for every old body.

The compiler does not catch every wrong key. This compiles with no warning:

```wirescript name=r4-wrong-key
@left in hit: exec
@left in shooter: character
var hits: Map<string, int>

on hit {
  hits.set(shooter, 1)
}
```

The map takes a `string` key, so the compiler turns the character into text
with a **Format String** gate. The key is the character printed as text, not
its user id. Always call `GetUserId()` for a string key.

A `Map<controller, int>` accepts a character too, wired straight into its
**Key** with no gate between. The game then stores the hit under that one
body rather than the player. A character and its player's controller are not
equal, so a respawn starts a new entry. Without admin, the user id is the
key that lasts.

A controller can also carry values itself. `SetCustomProperty` and
`GetCustomProperty` store values on an entity under a name, and on a
controller they follow the player through every respawn. They are lost when
the player reconnects, which a map keyed by user id is not. This needs the
controller, and only the admin `ControllerOf` turns a character into one
([Persistent Players](../admin/persistent-players.md)). Without admin, keep
the maps keyed by user id.

## Modify

1. Keep each player's best life. Add `var best: Map<string, int>`. After
   working out `life`, read `let oldBest = best.get(id)`, and
   `if !oldBest.Found || life > oldBest.Value { best.set(id, life) }`. Show
   the best on the sign.
2. Add a reset button. `on reset { ... }` calls `clear()` on every map and
   sets `shown` back to `"No hits yet"`.
3. Greet players coming back from a respawn. Put a zone in the doorway out of
   the spawn area, and compare the character walking through with the one in
   `bodies`:

   ```wirescript name=r4-welcome
   @left in hit: exec
   @left in shooter: character
   @left in doorway: zone
   var hits: Map<string, int>
   var bodies: Map<string, character>

   on hit {
     let id = shooter.GetUserId()
     let before = hits.get(id)
     hits.set(id, if before.Found then before.Value + 1 else 1)
     bodies.set(id, shooter)
   }

   on ZoneEntered(zone = doorway) -> (character) {
     let id = character.GetUserId()
     let body = bodies.get(id)
     if body.Found && body.Value != character {
       let total = hits.get(id)
       character.ShowChatMessage("Welcome back. ${total.Value} hits so far.")
       bodies.set(id, character)
     }
   }
   ```

   Storing the new body stops the greeting repeating every time they walk
   through.

## Make

Make a checkpoint race. Walking through a checkpoint zone records the player
in a `Map<string, bool>` keyed by user id. Walking through the finish zone
says "Finished!" only to players who passed the checkpoint, and clears their
entry. Check that a player who passes the checkpoint, dies and respawns can
still finish.

```wiring
zone1 "Zone Ref" -> chip checkpoint
zone2 "Zone Ref" -> chip finish
```

## Good to Know

- A respawn makes a new character for the same player. Nothing removes the
  old one from a map keyed by characters: its entry stays forever.
- A player's user id stays the same through every death and respawn. Their
  display name is chosen by the player, so escape it before showing it.
- Every `get` is its own gate and its own read. Bind each one to a `let`, as
  `before` and `body` are here, and read the `let`.
- A character is not equal to its player's controller. A character used as
  a `Map<controller, ...>` key, or stored in a `controller` variable, is
  still the body, and a respawn leaves it behind.
- Do not reach for `ControllerOf` to turn a character into a player: it uses
  a gate only an admin can load. The user id needs no admin.

See [Map Types](../../types.md#map-types-mapk-v) and
[Object references & assets](../../types.md#object-references--assets) in
the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
