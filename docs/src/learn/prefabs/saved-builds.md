# Saved Builds

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Spawning a Prefab](spawning.md)

**Next steps:** [Prefab Files](prefab-files.md)

</div>
<!-- /learn-links -->

A prefab written in Wirescript compiles to a chip of gates, nothing else. To
spawn real bricks, such as a crate, a lamp post or a target to shoot at, build
the thing in-game, save it as a prefab, and point the spawner at that file.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [A build that does something](#a-build-that-does-something)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip pad
button Held -> chip clear
```

## Code

```wirescript nocompile
@left in pad: zone
@left in clear: exec

on ZoneEntered(zone = pad) -> (character) {
  SpawnPrefab(
    prefab = $./crate.brz,
    offset = Vec(0.0, 0.0, 150.0),
    lifetime = 30.0,
    limit = 3,
    destroyAll = clear
  )
}
```

This is the spawner from [Spawning a Prefab](spawning.md), with one change:
`prefab = $./crate.brz` instead of a block. `$./` starts a path to a file,
relative to the folder the `.ws` file is in. The file must end in `.brz`, the
format Brickadia saves prefabs in.

The compiler reads `crate.brz` once, when you compile, and packs a copy into
the spawner's save. Later you rebuild the crate in-game and save it over
`crate.brz`. Which crate does the spawner already in your world make?

## Run

This block needs a file beside it, so its Compile button is off. Compile it
in one of these two ways.

**In VS Code or with the command line:**

1. Build a crate in-game. Select it with the selector and save the selection
   as a prefab. Brickadia writes it as a `.brz` file in its `Saved/Prefabs`
   folder (on Windows, `%LOCALAPPDATA%\Brickadia\Saved\Prefabs`).
2. Copy that file into the folder with your `.ws` file and name it
   `crate.brz`.
3. Compile the `.ws` file (**Ctrl+Shift+B** in VS Code, or
   `wirescript compile spawner.ws`). The compiler finds `crate.brz` next to
   it.

**In the playground:**

1. Save the crate as in step 1 above.
2. In the sidebar's **Prefabs** panel, press the upload button and pick the
   file. It is listed as `./crate.brz`. Click it to insert `$./crate.brz` at
   the cursor.
3. Compile. Uploaded prefabs stay in your browser for later visits.

Then paste the spawner in-game and wire it as in the last lesson: an
**Attached Zone**'s **Zone Ref** to `pad`, a **Button**'s **Held** to `clear`.
Step on the pad: a crate appears above the spawner.

## Under the Hood

The save holds its own copy of the crate, so the spawner works in any world,
with or without the file. That is also the answer to the question: the
spawner keeps the crate it was compiled with. Rebuilding the crate changes
nothing until you compile again and paste the new spawner.

A missing file stops the compile:

```text
Error: emit: prefab reference `$./crate.brz`: cannot read crate.brz
```

A path without `.brz` is caught sooner, and this one needs no file to show:

```wirescript name=p2-no-extension expect=WS019
@left in pad: zone

on ZoneEntered(zone = pad) -> (character) {
  SpawnPrefab(prefab = $./crate, offset = Vec(0.0, 0.0, 150.0))
}
```

```text
Error: [WS019] prefab reference `$./crate` must end in `.brz` (a prebuilt archive) or `.ws` (a source file compiled on reference)
```

The second ending, `.ws`, is the next lesson.

## A build that does something

A saved build can carry its own circuit. To spawn targets that count hits:

1. Build a target: a post with a **Target** brick on top.
2. Write a small chip for it with a `@left in hit: exec` pin and whatever it
   should do when hit. Compile it and paste it onto the post.
3. Wire the Target's **Just Hit** to `hit`.
4. Select the post, the target and the chip together and save the selection
   as `target.brz`.
5. Spawn `$./target.brz` from the spawner. Every copy brings its own chip,
   already wired.

A copy's chip cannot see the spawner's variables. How the two talk is
[Talking to a Copy](talking-to-a-copy.md), two lessons on.

## Modify

1. Save a second build (a lamp, a sign) and spawn it from a second pad with a
   second `SpawnPrefab` call.
2. Make the crate drop onto the player who stepped on the pad: keep the
   copy with `let crate = SpawnPrefab(...)`, then
   `crate.SetLocation(character.GetLocation() + Vec(0.0, 0.0, 500.0))`.
3. Leave `prefab` out entirely: `SpawnPrefab(offset = ..., limit = 3)`
   compiles to a **Spawn Prefab** gate with no prefab set, and you set one on
   the placed gate in-game instead. Try it, and compare which way is easier
   to change later.

## Make

Make a supply drop. Stepping on a marker pad stores that player's position
in a `var spot: vector`. A button then drops a crate 300 units above `spot`,
at most two at a time, each gone after a minute. The offset is measured from
the spawner, so spawn the crate and then move it with `SetLocation`.

```wiring
zone "Zone Ref" -> chip marker
button Held -> chip drop
```

## Good to Know

- The prefab is copied into the save when you compile. Editing the `.brz`
  afterwards changes nothing until you recompile and paste again.
- Every chip that spawns a prefab carries its own copy of it, so a large
  build makes every spawner that uses it large.
- A spawned build is a new object, separate from the build it came from.

See [Prefab References](../../expressions.md#prefab-references) in the
reference.

Next: [Prefab Files](prefab-files.md).
