# Prefab Files

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Saved Builds](saved-builds.md)

**Next steps:** [Talking to a Copy](talking-to-a-copy.md)

</div>
<!-- /learn-links -->

An inline prefab is handy while it is short. Once it grows, move it into a
`.ws` file of its own, next to the spawner, and reference the file. The
compiler builds it when it builds the spawner.

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
chip count -> decal Text
```

## Code

Two files in one folder. The spawner, `spawner.ws`:

```wirescript nocompile
@left in pad: zone
@left in clear: exec
var spawned: int = 0
@right out count = "Spawned: ${spawned.Value}"

on ZoneEntered(zone = pad) -> (character) {
  spawned += 1
  SpawnPrefab(
    prefab = $./box.ws,
    offset = Vec(0.0, 0.0, 150.0),
    lifetime = 10.0,
    limit = 5,
    destroyAll = clear
  )
}
```

The prefab, `box.ws`:

```wirescript nocompile
@label("Box")
var age: int = 0

on Clock(interval = 1.0) {
  age += 1
}
```

`$./box.ws` works like `$./crate.brz` from [Saved Builds](saved-builds.md),
except that the file is Wirescript source. The compiler compiles `box.ws` on
its own, then packs the result into the spawner's save, the same way it packs
an inline block.

The spawner counts its spawns in `spawned`. Could `box.ws` show that number
on each box?

## Run

These blocks need each other, so their Compile buttons are off.

**In VS Code or with the command line:** save both files in one folder,
open `spawner.ws` and press **Ctrl+Shift+B** (or run
`wirescript compile spawner.ws`). You compile only the spawner, and `box.ws`
is built with it. Paste it in-game and wire it as in
[Spawning a Prefab](spawning.md), plus `count` to a **Text Decal**'s
**Text**.

**In the playground:** a `$./...ws` reference is not available there. Compile
`box.ws` by itself, which downloads `box.brz`. Upload that in the **Prefabs**
panel, and change the reference to `$./box.brz`. The spawner then works the
same way, but a change to `box.ws` needs both steps again.

## Under the Hood

The compiled spawner is the same as the one from the inline version: a
**Spawn Prefab** gate whose prefab setting points at a copy of the box
inside the save.

The answer to the question is no. A prefab is a separate program. It has its
own variables and sees none of the spawner's, whether it lives in a file or
inline. Inline makes this easy to forget, because the code sits inside the
spawner's handler:

```wirescript name=p3-closed expect=WS002
@left in pad: zone
var spawned: int = 0

on ZoneEntered(zone = pad) -> (character) {
  spawned += 1
  SpawnPrefab(prefab = $```
    var shown: int = 0
    on Clock(interval = 1.0) {
      shown = spawned
    }
  ```, offset = Vec(0.0, 0.0, 150.0), limit = 5)
}
```

```text
Error: [WS002] unknown identifier 'spawned'
```

A copy learns values from the spawner by events, which is
[Talking to a Copy](talking-to-a-copy.md).

A misspelled file name is reported at the reference:

```text
Error: [WS019] cannot read prefab source `$./box.ws`
```

## Modify

1. Move the inline prefab from [Spawning a Prefab](spawning.md) into a file
   of its own, and check that the spawner behaves the same.
2. Reference `$./box.ws` from a second spawner in the same folder. Each
   spawner's save gets its own copy of the box.
3. Put the prefab in a subfolder, `parts/box.ws`, and reference
   `$./parts/box.ws`.

## Make

Keep two prefab files, `small.ws` and `big.ws`, each labeled differently,
and a spawner with two pads that spawns one or the other. Then change the
label in `small.ws`, recompile, and paste the spawner again to see the
change.

```wiring
zone1 "Zone Ref" -> chip smallPad
zone2 "Zone Ref" -> chip bigPad
```

## Good to Know

- A prefab is copied into the save when you compile. Changing `box.ws`
  changes nothing in-game until you recompile the spawner and paste it
  again.
- A prefab and its spawner share no variables. A spawned copy is a separate
  program in the world, with storage of its own.

See [Prefab References](../../expressions.md#prefab-references) in the
reference.

Next: [Talking to a Copy](talking-to-a-copy.md).
