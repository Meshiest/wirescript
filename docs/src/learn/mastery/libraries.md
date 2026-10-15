# Writing Libraries

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Imports](../abstraction/imports.md), [Multiplayer Architecture](multiplayer.md)

</div>
<!-- /learn-links -->

A library is a file of helpers written for other files to import. A good
one works for more than one caller and leaves the decisions to the file
that imports it. This lesson writes a small score library and a game that
uses it.

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
chip "Arena"
button1 Held -> chip red
button1 Character -> chip redBy
button2 Held -> chip blue
button2 Character -> chip blueBy
chip sign -> decal Text
```

```wiring
chip "Horn"
chip light -> light Enabled
chip sign -> decal Text
```

## Code

The library, `scores.ws`:

```wirescript nocompile
// Helpers for keeping score in maps. No `on` handlers here:
// each entry file declares its own.

/// Reads `key` from `m`, or `fallback` when the key is missing.
mod getOr<K: int | string, V>(m: Map<K, V>, key: K, fallback: V) -> V {
  let v = m.get(key)
  return if v.Found then v.Value else fallback
}

/// Adds `by` to the number filed under `key`, starting a missing key at 0.
/// Returns the new total.
mod bump<K: int | string>(m: Map<K, int>, key: K, by: int = 1) -> int {
  let total = getOr(m, key, 0) + by
  m.set(key, total)
  return total
}

/// Sends `scores.<name>` to every listening chip, carrying the values after `name`.
mod announce(name: const string, ...rest) {
  SendCustomEvent("scores." .. name, ...rest)
}
```

The game that imports it, `arena.ws`:

```wirescript nocompile
import { getOr, bump, announce } from "scores"

@left in red: exec
@left in redBy: character
@left in blue: exec
@left in blueBy: character
var teams: Map<int, int>
var goals: Map<string, int>
var shown: string = "Red 0 - Blue 0"
@right out sign = shown.Value

mod goal(team: int, who: character) {
  let total = bump(teams, team)
  let mine = bump(goals, who.GetUserId())
  shown = "Red ${getOr(teams, :red, 0)} - Blue ${getOr(teams, :blue, 0)}"
  who.ShowChatMessage("Your goals: ${mine}")
  announce("goal", total, mine)
}

on red {
  goal(:red, redBy)
}

on blue {
  goal(:blue, blueBy)
}
```

Four things are new:

- **Type parameters.** `<K: int | string>` after a mod's name says `K`
  stands for a type the caller chooses, here either `int` or `string`.
  `bump` works on a `Map<int, int>` and a `Map<string, int>` alike, and
  `getOr` adds a second parameter, `V`, for the kind of value stored.
- **Atoms.** `:red` and `:blue` are atoms: names that stand for fixed
  numbers. `teams` is keyed by them, so the code reads as words while the
  map holds `int`s.
- **Variadic parameters.** `...rest` in `announce` collects every argument
  after `name`, however many there are, and `...rest` in the body passes
  them all on. `const string` means `name` must be text written in the
  code, so the channel name can be built at compile time.
- **Doc comments.** A line starting `///` describes the declaration under
  it. Editors show it when you point at a call.

`bump` is called with two different maps: one keyed by atoms, one by user
ids. How many copies of `bump` does the pasted chip hold? Does the game ever
see the word `red`?

## Run

The **Compile** button on this page cannot see a file of yours, so build
this one in the playground or VS Code, as in [Imports](../abstraction/imports.md):

1. In the playground, press **+** under **Files** and name the new file
   `scores.ws`. In VS Code, save `scores.ws` in the same folder as
   `arena.ws`. Paste the library into it and the game into `arena.ws`.
2. Compile `arena.ws` and paste the chip in-game. Place two **Button**s and
   a **Text Decal**.
3. Wire one Button's **Held** to `red` and its **Character** to `redBy`, the
   other's to `blue` and `blueBy`, and `sign` to the Text Decal's **Text**.
4. Press the red button twice and the blue button once. The sign reads
   `Red 2 - Blue 1`, and chat tells you your own goal count.
5. Point at `bump` in `arena.ws`: the editor shows the `///` lines from the
   library.

To see `announce` arrive, paste this chip too, with a **Point Light** and a
second Text Decal:

```wirescript name=ma5-horn
var lit: bool = false
var shown: string = "No goals yet"
@right out light = lit.Value
@right out sign = shown.Value

on CustomEvent("scores.goal") -> (teamTotal: int, mine: int) {
  lit = !lit
  shown = "Goal! Team total ${teamTotal}, scorer ${mine}"
}
```

Wire `light` to the Point Light's **Enabled** and `sign` to the Text Decal's
**Text**. Every goal toggles the light.

## Under the Hood

### What the chip holds

Open the chip. There is no library in it, only copies. `goal` is called
twice and calls `bump` twice, so there are four copies of `bump`, each a
**Get Map Element**, a **Select**, an **Add** and a **Set Map Element**. The
two that count a team have a number typed into their **Key**. The two that
count a player take their **Key** from a **Get User Id**. A
generic mod is built again at each call for the types used there, exactly
as if you had written a separate mod for each.

The atoms are those typed-in numbers. `:red` is a large number worked out
from its name, and the game sees only the number. Every file and every
compile works out the same number from the same name, so two chips built
separately agree on `:red` without sharing any code.

`announce` left one **Send Personal Custom Event** gate in each handler.
The channel `scores.goal` is typed into it, joined at compile time from
`"scores."` and `"goal"`, and the two values after the name are wired into
**Data In1** and **Data In2**.

`bump` calls `getOr`, and that works even in a file that imports only
`bump`: a library's helpers can always call each other. The importing file
names only what it calls itself.

### Bounds

`K: int | string` is a bound. Leave it off and `K` could be any value type,
`bool` among them. A map cannot be keyed by a `bool`, and the compiler
checks the body for every type `K` might be, not only the ones you call it
with:

```wirescript name=ma5-unbounded expect=WS039
@left in hit: exec
@left in scorer: character
var hits: Map<string, int>

mod bump<K>(m: Map<K, int>, key: K) -> int {
  let v = m.get(key)
  let total = if v.Found then v.Value + 1 else 1
  m.set(key, total)
  return total
}

on hit {
  bump(hits, scorer.GetUserId())
}
```

```text
Error: [WS039] map key type must be int, string, an object (entity/character/controller) or a brick, got bool
```

The bound tells callers what the helper accepts, and the compiler checks
every call against it. [Generics](../../types.md#generics) lists the named
bounds (`Scalar`, `Numeric`, `Variant`).

Only a `mod` can have type parameters or `...rest`. A `fn` compiles once,
to one plane with a fixed set of slots, so a generic or variadic `fn` is
`WS114`. Make a library helper a `fn` when it is large and called often
(`sanitize` in the `sanitize` library is one), and a `mod` when it must fit many types.

## Modify

1. Add a third team. Give `arena.ws` a `green` and a `greenBy` pin, a
   handler that calls `goal(:green, greenBy)`, and a `Green` count on the
   sign. The library does not change, because it has no list of teams.
2. Add a helper to the library with a doc comment:
   `/// The highest number in m, or 0 when it is empty.` followed by
   `mod best<K: int | string>(m: Map<K, int>) -> int`. Loop over the map
   with `for k, v in m` and keep the largest value in a local `var`. Use it
   to add `Best player: ${best(goals)} goals` to the chat message.
3. See why handlers stay out of libraries. Add `var horn: bool = false` and
   a handler `on CustomEvent("scores.goal") { horn = !horn }` to
   `scores.ws`. In `arena.ws`, add `horn` to the import and
   `@right out light = horn.Value`. Compile:

   ```text
   WARN [WS014] `import { ... } from 'scores'` brings only the names it lists, so the 1 `on` handler(s)/anonymous chip(s) 'scores' installs are NOT included; write `import "scores"` to install them too
   ```

   The handler was left out, so the light would never change. Change the
   import to `import "scores"` and the warning goes. Then move the handler
   back into `arena.ws`: there, no way of importing the library can drop it.

## Make

Write a library, `cooldowns.ws`, that other people could use without
reading its body. It holds a generic mod that takes a `Map<K, int>` of
the last time each key acted, a key, the current time from
`GetUnixTime()` (whole seconds), and a number of seconds, and answers whether the key may
act again (recording the time when it may). Give every helper a `///` line
and keep the file free of `on` handlers. Then use it from two entry files:
a button that each player can press once every 3 seconds (keyed by user
id), and a door whose two sides share one 10-second cooldown (keyed by
atoms).

```wiring
chip "Button"
chip2 "Door"
button1 Held -> chip press
button1 Character -> chip presser
chip sign -> decal Text
button2 Held -> chip2 front
button3 Held -> chip2 back
chip2 solid -> changer "Collision Enabled"
chip2 solid -> changer Visibility
```

## Good to Know

- A pasted chip holds copies of the helpers it used. Two chips built from
  one library share nothing in-game, not even a library `var`.
- Change the library, and chips already pasted stay as they were. Compile
  and paste each one again.
- An atom is a fixed number, and a fixed value passed to `SendCustomEvent`
  is typed into the gate instead of wired, so the receiver can read 0. Put
  an atom in a `var` before sending it.
- Doc comments are mostly for editors. On a `chip`, the `///` line also
  shows at the top of the chip's plane in-game when it is opened.

See [Generics](../../types.md#generics), [Atom
Literals](../../expressions.md#atom-literals), [Variadic
Parameters](../../chips.md#variadic-parameters-rest), [Doc
Comments](../../syntax.md#doc-comments) and [Imports](../../syntax.md#imports)
in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
