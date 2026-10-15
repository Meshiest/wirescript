# Spawning a Prefab

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Custom Events](../events/custom-events.md)

**Next steps:** [Saved Builds](saved-builds.md)

</div>
<!-- /learn-links -->

A circuit can build things while the game runs. It holds a prefab, a small
build packed inside the chip, and the **Spawn Prefab** gate puts a copy of it
into the world each time its exec fires. This lesson spawns a little chip
above the spawner whenever someone steps on a pad.

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
zone "Zone Ref" -> chip pad
button Held -> chip clear
```

## Code

```wirescript name=p1-spawner
@left in pad: zone
@left in clear: exec

on ZoneEntered(zone = pad) -> (character) {
  SpawnPrefab(
    prefab = $```
      @label("Box")
      var age: int = 0

      on Clock(interval = 1.0) {
        age += 1
      }
    ```,
    offset = Vec(0.0, 0.0, 150.0),
    lifetime = 10.0,
    limit = 5,
    destroyAll = clear
  )
}
```

- `SpawnPrefab(...)` spawns one copy each time the handler runs.
- `prefab = $` followed by a block between three backticks is the prefab,
  written in place. It is a whole Wirescript program of its own: here a chip
  labeled "Box" that counts its age in seconds. The compiler builds it first
  and packs it into the spawner's save.
- `offset` is where the copy appears, measured from the **Spawn Prefab**
  gate itself. `Vec(0.0, 0.0, 150.0)` is a `vector` (three numbers: x, y,
  z): here 150 units straight up from the spawner.
- `lifetime = 10.0` removes each copy 10 seconds after it appears.
- `limit = 5` caps how many copies of this gate's prefab exist at once.
- `destroyAll = clear` removes every copy this gate has made when `clear`
  turns on.

You step on the pad seven times in a row, quickly. How many boxes are there
afterwards, and how many are left 15 seconds later?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   for the pad, and place a **Button** beside it.
2. Wire the Attached Zone's **Zone Ref** to `pad`, and the Button's **Held**
   to `clear`.
3. Step on the pad. A Box chip appears above the spawner.
4. Step off and on a few more times. Never more than five exist at once.
5. Wait ten seconds: they vanish one by one, in the order they appeared.
   Spawn a few more and press the button: they all go at once.

## Under the Hood

Open the chip. The handler is a **Player Entered Zone Event**, then a
**Spawn Prefab** gate. The offset, the lifetime and the limit are typed into
its **Spawn Offset**, **Lifetime** and **Limit**. The `clear` pin is wired
into **Destroy All**, a second exec input on the same gate, so one gate both
spawns and clears.

The Box is not in the chip as gates. It is a file inside the save, and the
gate's prefab setting points at it. Pasting the spawner brings the Box with
it.

The offset is a distance from the Spawn Prefab gate, not a position in the
world. To put a copy somewhere else, such as above the player who stepped on
the pad, spawn it first and then move it. `SpawnPrefab` gives back the copy
it spawned, and `SetLocation` moves that copy to a position in the world:

```wirescript name=p1-origin
@left in pad: zone

on ZoneEntered(zone = pad) -> (character) {
  let box = SpawnPrefab(
    prefab = $```
      @label("Box")
      var age: int = 0
    ```,
    limit = 5
  )
  box.SetLocation(character.GetLocation() + Vec(0.0, 0.0, 150.0))
}
```

`character.GetLocation()` is the position of the player who stepped on the
pad, and adding `Vec(0.0, 0.0, 150.0)` raises it 150 units. The copy appears
at the spawner and is moved over the player's head on the same exec chain.
Putting the player's position into `offset` instead would add it to the
spawner's own position, and the box would land far away.

## Modify

1. Set `lifetime = 0.0`. A lifetime of 0 means the copies stay until
   something removes them. Now only the button clears them.
2. Spawn two boxes per step: one 150 units up and one 300 units up. Each
   `SpawnPrefab` call is its own gate with its own limit. What does that do
   to the most boxes you can have?
3. Remember where the last player stood in a `var spot: vector`. From an
   `on Clock(interval = 2.0, enabled = ...)` handler, with a Switch's
   **Enabled** turning the clock on, spawn a box and move it to `spot`.

## Make

Make a tower pad: every step on the pad spawns a box 100 units higher than
the last one, up to ten, and a button clears the tower and starts the height
over.

```wiring
zone "Zone Ref" -> chip pad
button Held -> chip clear
```

## Good to Know

- The offset is measured from the Spawn Prefab gate, not from the middle of
  the world. For a position in the world, spawn the copy and then
  `SetLocation` it.
- `SetLocation` moves a spawned copy. It does not move a pasted chip's own
  grid.
- A copy is a new object in the world. Each one costs the server something
  to keep, so give a spawner a `limit` and either a `lifetime` or a way to
  clear it.
- `destroyAll` removes only what its own gate spawned. A second spawner's
  copies stay.

See [SpawnPrefab](../../builtins.md#spawnprefab) and
[Prefab References](../../expressions.md#prefab-references) in the
reference.

Next: [Saved Builds](saved-builds.md).
