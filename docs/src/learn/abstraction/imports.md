# Imports

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Functions](functions.md)

**Next steps:** [Counting the Cost](cost.md), [Writing Libraries](../mastery/libraries.md)

</div>
<!-- /learn-links -->

Mods, chips and functions can live in a file of their own, and other files
can bring them in with `import`. One file of helpers then serves every
circuit you build.

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
chip text -> decal Text
```

## Code

```wirescript name=g4-guest-book
import { escape, sanitize } from "sanitize"

@left in room: zone
var last: string = "Nobody yet"
var guests: int = 0
@right out text = "Last guest: ${last.Value}<br>Guests: ${guests.Value}"

on ZoneEntered(zone = room) -> (character) {
  let name = character.GetDisplayName()
  last = sanitize(name, 16)
  guests += 1
  character.ShowChatMessage("Welcome, ${escape(name)}!")
}
```

`import { escape, sanitize } from "sanitize"` brings two helpers in from a
file named `sanitize.ws`, the lessons' starter library. `escape` is the mod
from [Reacting to the Game](../reacting-to-the-game.md) that makes a name
safe to show. `sanitize` is a `fn` that also trims spaces from the ends and
cuts the text to a length, here 16 characters. After the import, both are
called as if they were written in this file.

When you paste this chip in-game, where is the code for `escape` and
`sanitize`? Does the game need the library file?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**,
   and place a **Text Decal**.
2. Wire the zone's **Zone Ref** to `room`, and `text` to the Text Decal's
   **Text**.
3. Walk into the zone. The sign shows your name and a count of 1, and chat
   says `Welcome`, with your name.

## Under the Hood

Open the chip. The library's code is in it:

- `escape` is a mod, so it was copied in as one **Replace in String** gate
  beside the **Show Chat Message**.
- `sanitize` is a `fn`, so the chip has a **Function Definition** board
  holding its body (**Trim String**, **Get Substring**, **Replace in String**)
  and one **Function Call** in the handler.

The compiler reads the imported file and builds what you use into the chip.
The save holds everything, and the game never sees the library file.

There are three ways to write an import:

```wirescript nocompile
import { escape, sanitize } from "sanitize"   // just these names
import "sanitize"                             // everything in the file
import * as safe from "sanitize"              // everything, as safe.escape(...)
```

The last form keeps the library's names apart from yours. The calls become
`safe.escape(name)` and `safe.sanitize(name, 16)`. Forget the import, and
the compiler stops at the first call:

```wirescript name=g4-missing-import expect=WS002
@left in room: zone
var last: string = "Nobody yet"
@right out text = "Last guest: ${last.Value}"

on ZoneEntered(zone = room) -> (character) {
  last = safe.sanitize(character.GetDisplayName(), 16)
}
```

```text
Error: [WS002] unknown identifier 'safe' in call `safe.sanitize(...)`: no namespace, variable, or value named 'safe' is in scope (is an import missing?)
```

## Modify

1. Change the import to `import * as safe from "sanitize"` and put `safe.`
   in front of both calls. Then try `import "sanitize"` with the plain
   names. All three compile to the same gates.

2. Start a library of your own. In the playground, press **+** under
   **Files** to add a file, and rename it to `medals.ws`. In VS Code, save
   `medals.ws` in the same folder as your main file. Put a helper in it:

   ```wirescript nocompile
   mod medal(hits: int) -> string {
     return if hits >= 10 then "gold" else if hits >= 5 then "silver" else "bronze"
   }
   ```

   Then use it from your main file:

   ```wirescript nocompile
   import { medal } from "medals"

   @left in hit: exec
   var hits: int = 0
   @right out board = "${hits.Value} hits, ${medal(hits.Value)}"

   on hit { hits += 1 }
   ```

   The path is the file's name without `.ws`. In VS Code it is counted from
   the importing file's folder, so a file in a subfolder is
   `import { medal } from "lib/medals"`.

3. Move a variable into the library: add `var total: int = 0` to
   `medals.ws`, import `total` too, and add one to it in the handler. A
   variable from an import is shared. Every file in the program that
   imports it reads and writes the same **Var** gate.

The examples on these pages can import only the starter library. Their
**Compile** button knows `sanitize` but none of your files. Your own libraries
work in the playground and VS Code.

## Make

Write a library with two helpers for signs: a mod that pads a number to two
digits (`7` becomes `07`) and a `fn` that turns seconds into `m:ss`. Use both
from two different chips, a lap timer and a countdown, each in its own main
file. As in [Functions](functions.md), `/` and `%` split seconds into minutes
and the seconds left over.

```wiring
chip "Lap Timer"
chip2 "Countdown"
button1 Held -> chip lap
chip sign -> decal1 Text
button2 Held -> chip2 start
chip2 sign -> decal2 Text
```

## Good to Know

- Each pasted chip carries its own copy of what it imported. Two chips built
  from the same library share nothing in-game, not even a library `var`.
- A library can have its own `on` handlers. `import { name } from "x"` brings
  only the names it lists and leaves those handlers out. The compiler warns
  (`WS014`). Use `import "x"` or `import * as x from "x"` to keep them.
- Change the library and the chips already pasted in-game stay as they
  were. Compile and paste each one again to update it.

See also: [Imports](../../syntax.md#imports).

Next: [Counting the Cost](cost.md).
