# Lifecycle and Storage

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Talking to a Copy](talking-to-a-copy.md)

**Next steps:** [Nested Prefabs](nested-prefabs.md)

</div>
<!-- /learn-links -->

Every spawned copy is its own program, with variables nobody else shares.
This lesson spawns boxes that each know their own number, live for a time
that depends on it, remove themselves, and tell the spawner when they go.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [When the player leaves](#when-the-player-leaves)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip pad
button Held -> chip clear
chip sign -> decal Text
```

## Code

```wirescript name=p5-boxes
@left in pad: zone
@left in clear: exec
var made: int = 0
var number: int = 0
var live: int = 0
var lastGone: int = 0
@right out sign = "Boxes: ${live.Value}, last gone: ${lastGone.Value}"

on ZoneEntered(zone = pad) -> (character) {
  made += 1
  number = made
  live += 1
  let box = SpawnPrefab(
    prefab = $```
      @label("Box")
      var number: int = 0
      var age: int = 0

      on CustomEvent("box.init", isObject = true) -> (n: int) {
        number = n
      }

      on Clock(interval = 1.0) {
        if number > 0 {
          age += 1
          if age >= number * 2 {
            SendCustomEvent("box.gone", number)
            ReadBrickGrid().DestroySpawnedPrefab()
          }
        }
      }
    ```,
    offset = Vec(0.0, 0.0, 150.0),
    limit = 10,
    destroyAll = clear
  )
  box.SendCustomEvent("box.init", number)
}

on CustomEvent("box.gone") -> (n: int) {
  live -= 1
  lastGone = n
}

on clear {
  live = 0
}
```

- The spawner numbers each box and sends the number to that box only.
- Each box keeps the number in its own `number` and counts its own `age`.
  Box 1 lives 2 seconds, box 2 lives 4, box 3 lives 6.
- `ReadBrickGrid().DestroySpawnedPrefab()` removes the copy that runs it.
  Before it goes, the box sends `"box.gone"` with its number. This send has
  no target, so it reaches every listener pasted by the same player, the
  spawner included.
- The spawner counts live boxes up on each spawn and down on each
  `"box.gone"`. The button clears them all with `destroyAll`, and sets the
  count to 0 itself, since cleared boxes send nothing.

The spawner has a `var number`, and so does every box. When box 3 sets its
`number` to 3, what happens to the spawner's `number` and to box 1's?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   for the pad, and place a **Button** and a brick with a **Text Decal**.
2. Wire the **Zone Ref** to `pad`, the Button's **Held** to `clear`, and
   `sign` to the Text Decal's **Text**.
3. Step on the pad three times, quickly. The sign says `Boxes: 3`.
4. Watch: box 1 goes first, then box 2, then box 3, each two seconds after
   the last. The sign counts down and names each one as it goes.
5. Spawn a few more and press the button. They all go, and the sign reads 0.

## Under the Hood

The box's gates are not in the spawner's chip. To look at them, copy the
box's code into a file of its own and compile it. It has its own **Var**
gates for `number` and `age`. The spawner has its own Var for `number` too.
The two never touch, because a copy's variables belong to that copy alone,
so the answer to the question is "nothing".

The same holds for imports. A copy that imports a module gets its own copy
of that module's variables, not the spawner's.

The **Destroy Spawned Prefab** gate takes the copy's own grid, from **Read
Brick Grid**, as its **Entity**. Wire a different spawned entity into it and
it removes that one instead: the spawner could do `box.DestroySpawnedPrefab()`
with an entity it kept.

## When the player leaves

A copy that serves one player should go when that player does. A stored
reference to a player who has left the server turns into `null`, so a copy
can check for that once a second. A dead player's stored character turns
into `null` too, some time after the death, so the same check also
removes the copy of a player who died, a little later. Write the check
inside the handler that
stores the player:

```wirescript nocompile
on CustomEvent("marker.init", isObject = true) -> (who: character) {
  owner = who
  ready = true
  on Clock(interval = 1.0) {
    if ready {
      if owner == null {
        ReadBrickGrid().DestroySpawnedPrefab()
      }
    }
  }
}
```

The inner Clock is still a handler of its own, running from the moment the
copy appears, so it checks `ready` before it reads `owner`. Without such a
check, a copy whose player has gone keeps running until something removes
it.

The spawner can also tag a copy with its player's user id as it spawns it,
`copy.SetCustomProperty("owner", id)`. The copy reads its own tag with
`ReadBrickGrid().GetCustomProperty("owner")`, and the spawner can read any
copy's tag from the entity it kept. [One Chip per
Player](one-chip-per-player.md) uses it to tell the spawner which player's
copy has gone.

## Modify

1. Make every box live five seconds, whatever its number. Which lines of the
   box change, and does the spawner change at all?
2. Add the age to the report: send `"box.gone"` with `number` and `age`, and
   show both on the sign.
3. Give each box an owner: send the player who stepped on the pad along
   with the number, and have the box remove itself early, with a
   `"box.gone"`, when that player leaves the server.

## Make

Make a fuse: each box counts down from 5 seconds, and on reaching 0 it
sends `"box.boom"` with its number and removes itself. The spawner shows
"Box 4 went off" for the most recent one, and a second pad clears all boxes
before they go off.

```wiring
zone1 "Zone Ref" -> chip pad
zone2 "Zone Ref" -> chip clearPad
chip sign -> decal Text
```

## Good to Know

- Each copy has its own variables, and so does each copy's imports. Nothing
  is shared with the spawner or with other copies. Values travel by events.
- A copy whose work is done must remove itself (or be removed by `lifetime`
  or `destroyAll`). Nothing else cleans it up.
- `DestroySpawnedPrefab` on the copy's own grid removes it at once. Send any
  last event before that line.
- The box's Clock only counts toward one moment. With
  [Waiting](../events/waiting.md), the init handler can wait for that moment
  itself, `await Sleep(_, delay = n * 2.0)`, with no Clock.
- A stored reference to a player who has left reads as `null`, and
  `GetUserId()` on it reads back `""`. A dead player's stored character
  reads as `null` too, after a delay that is not fixed.
- The live count here counts steps on the pad. At the `limit` it can drift,
  so keep the limit above what you expect to need.

See [SpawnPrefab](../../builtins.md#spawnprefab) and
[per-entity fan-out](../../best-practices.md#10-per-entity-work-a-for-sweep-or-one-chip-each)
in the reference.

Next: [Nested Prefabs](nested-prefabs.md).
