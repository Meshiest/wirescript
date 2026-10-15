# Nested Prefabs

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Lifecycle and Storage](lifecycle.md)

**Next steps:** [One Chip per Player](one-chip-per-player.md)

</div>
<!-- /learn-links -->

A spawned copy is a full program, so it can spawn copies of its own. This
lesson places a turret that fires a shot every second. The turret is a
prefab, and the shot is a prefab inside it.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [As files](#as-files)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip pad
button Held -> chip clear
```

## Code

```wirescript name=p6-turret
@left in pad: zone
@left in clear: exec

on ZoneEntered(zone = pad) -> (character) {
  SpawnPrefab(
    prefab = $```
      @label("Turret")

      on Clock(interval = 1.0) {
        SpawnPrefab(
          prefab = $```
            @label("Shot")

            on Clock(interval = 0.05) {
              let grid = ReadBrickGrid()
              grid.SetLocation(grid.GetLocation() + Vec(0.0, 20.0, 0.0))
            }
          ```,
          offset = Vec(0.0, 0.0, 30.0),
          lifetime = 3.0,
          limit = 10
        )
      }
    ```,
    offset = Vec(0.0, 200.0, 50.0),
    limit = 1,
    destroyAll = clear
  )
}
```

Three programs, one inside the next:

- The **spawner** places one turret 200 units to the side of itself when
  someone steps on the pad (`limit = 1`, so there is never more than one).
- The **turret** has no events to wait for. Its clock starts when it
  appears, and every second it spawns a shot 30 units above itself. An
  offset is measured from the Spawn Prefab gate that uses it, and this one
  sits in the turret, so the shots start at the turret wherever it is.
- The **shot** moves itself 20 units along the world's Y axis every
  twentieth of a second, and its lifetime removes it after 3 seconds.
  `ReadBrickGrid()` inside a copy is the copy's own grid, and
  `SetLocation` moves a spawned copy.

The `clear` button removes the turret. What happens to the shots already in
the air?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   for the pad and place a **Button** beside it.
2. Wire the **Zone Ref** to `pad` and the Button's **Held** to `clear`.
3. Step on the pad. A Turret chip appears, and a Shot chip rises out of it
   every second and flies off.
4. Press the button while shots are flying.

## Under the Hood

The spawner's save holds the turret, and the turret holds the shot, so one
compile packs both levels. Only the spawner has a pin. The turret and the
shot run on their own.

The turret's **Spawn Prefab** gate belongs to the turret, so `limit = 10`
counts the shots that one turret has made, and the spawner's `clear` reaches
only what the spawner's gate made: the turret. Shots already in the air were
made by the turret's gate, not the spawner's, so expect each one to go when
its own 3 seconds run out.

Prefabs can nest up to 8 levels deep, inline or as files.

## As files

The same three programs as files, with the shot in a subfolder:

```text
spawner.ws
turret.ws
parts/shot.ws
```

`spawner.ws` references `$./turret.ws`. Inside `turret.ws`:

```wirescript nocompile
@label("Turret")

on Clock(interval = 1.0) {
  SpawnPrefab(
    prefab = $./parts/shot.ws,
    offset = Vec(0.0, 0.0, 30.0),
    lifetime = 3.0,
    limit = 10
  )
}
```

A path inside a prefab file is read from that file's folder, not the
spawner's. Here the two are the same folder, but move `turret.ws` into
`parts/` and its reference becomes `$./shot.ws`, while `spawner.ws` changes
to `$./parts/turret.ws`. Compiling `spawner.ws` builds all three, as in
[Prefab Files](prefab-files.md).

## Modify

1. Fire the shots straight up instead: which numbers in the shot change?
2. Give the turret a second clock that fires a second kind of shot, slower
   and longer-lived, every three seconds.
3. Make the shot fall: each pulse, also lower it a little more than the last
   time. Keep the drop so far in a `var` inside the shot.

## Make

Make a fountain: a pad places a fountain that, every half second, spawns a
drop going up. Each drop rises for a second, then falls for a second, then
removes itself with `DestroySpawnedPrefab()`. A button clears the fountain.

```wiring
zone "Zone Ref" -> chip pad
button Held -> chip clear
```

## Good to Know

- `destroyAll` removes what its own gate spawned, not what those copies
  spawned in turn. Give inner prefabs a `lifetime`.
- Every level carries the levels inside it. A large shot makes the turret
  large, and the turret makes the spawner large.
- A path inside a prefab file is relative to that file.

See [Prefab References](../../expressions.md#prefab-references) in the
reference.

Next: [One Chip per Player](one-chip-per-player.md).
