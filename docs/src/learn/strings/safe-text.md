# Safe Text

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Taking Text Apart](taking-text-apart.md), [Rich Text](rich-text.md)

**Next steps:** [Chat Commands](../admin/chat-commands.md)

**Parts that use this:** [Obstacle Course](../parts/obstacle-course.md), [Whack-a-Light](../parts/arcade-game.md), [Card Table](../parts/card-game.md)

</div>
<!-- /learn-links -->

A player picks their own display name, and a name can contain anything,
including tags. When a circuit shows that name, the game reads those tags as
formatting. This lesson shows the problem, then fixes it with the two helpers
in the lessons' starter library.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Use it: a guest sign](#use-it-a-guest-sign)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
switch Enabled -> chip safe
chip text -> decal Text
```

## Code

```wirescript name=s4-tricky-name
import { escape } from "sanitize"

const TRICKY = '<size="96"><color="ff0000">Sam'

@left in safe: bool
let shown = if safe then escape(TRICKY) else TRICKY
@right out text = "Guest: ${shown}<br>Welcome!"
```

`TRICKY` stands in for a player whose display name is
`<size="96"><color="ff0000">Sam`. The switch picks whether the sign shows it
as it is, or after `escape`.

`escape` replaces every `<` with `&lt;`. That is the standard way to write a
`<` that is not the start of a tag: the game shows it as a plain `<`, so the
tag appears as text instead of being obeyed.

With the switch off, what does the word `Welcome!` look like? With it on?

## Run

1. Compile the chip and paste it in-game. Place a **Switch** and a brick with
   a **Text Decal**.
2. Wire the Switch's **Enabled** to `safe`, and `text` to the Text Decal's
   **Text**.
3. With the switch off, the name's tags take over: `Sam` and everything after
   it, `Welcome!` included, is huge and red.
4. Flip the switch on. The sign shows the name exactly as it was typed, tags
   and all, and `Welcome!` looks normal.

## Under the Hood

Open the chip. `escape` added one **Replace in String** gate, which searches
for `<` and puts `&lt;` in its place. A **Select** chooses between the
escaped and the raw text, and a **Format String** builds the sign.

The library is a file named `sanitize.ws`, and
`import { escape } from "sanitize"` brings in one helper from it. This is the
whole file:

```wirescript nocompile file=../lib/sanitize.ws
// Escape markup in a display name before showing it. A `mod`, so each use is a single gate.
mod escape(text: string) -> string {
  return text.Replace("<", "&lt;")
}

// Clean free-form player input: trim, cap the length, then escape.
// Capping first keeps the cut from landing inside an `&lt;`.
fn sanitize(raw: string, maxLen: int = 24) -> string {
  return raw.Trim().Substring(0, maxLen).Replace("<", "&lt;")
}
```

- `escape` is a `mod`. A mod is copied into every place that uses it, so
  each `escape(...)` becomes one Replace in String gate right there.
- `sanitize` is a `fn`. A function is built once, on its own small board,
  and each place that uses it gets a **Function Call** gate that runs it.
  `maxLen: int = 24` gives the second input a default, so `sanitize(name)`
  means `sanitize(name, 24)`.

## Use it: a guest sign

`sanitize` is for text that must also fit a space. It trims spaces from the
ends, keeps at most `maxLen` characters, then escapes what is left. This
sign keeps the last guest's name short enough for a narrow sign, and greets
the guest by their full name in chat:

```wirescript name=s4-guest-sign
import { escape, sanitize } from "sanitize"

const SIGN_WIDTH = 12

@left in room: zone
var guest: string = "Nobody yet"
@right out text = '<size="32">Last guest:<br><b>${guest.Value}</>'

on ZoneEntered(zone = room) -> (character) {
  let name = character.GetDisplayName()
  guest = sanitize(name, SIGN_WIDTH)
  character.ShowChatMessage("Welcome, ${escape(name)}!")
}
```

Wire an **Attached Zone**'s **Zone Ref** to `room` and `text` to a **Text
Decal**'s **Text**, then walk into the zone.

The cap comes before the escape because escaping turns one `<` into four
characters. If the cut came after, it could land in the middle of an `&lt;`
and leave a broken piece such as `&l` at the end of the sign. Cutting first
means every `<` that survives the cut is escaped whole. The cost is that the
result can be longer than `maxLen` when the name contains `<`.

Which one to use:

- `escape(text)` when the whole text should be shown, such as a name in a
  chat line.
- `sanitize(text, maxLen)` when the text must fit, such as a sign, a label or
  one row of a list.

## Modify

1. Change `SIGN_WIDTH` to 4 and walk in again. Only the start of your name
   fits.
2. Run your sign through the switch test: replace `name` with `TRICKY` from
   the first example in the `sanitize` call, and check that the sign shows
   the tags as text.
3. Greet the guest in chat by the first word of their name only, with
   `Split(" ")` from [Taking Text Apart](taking-text-apart.md). Split the raw
   name first, then escape the word you show: escaping is always the last
   step before text is displayed.

## Make

Make a sign that lists the last three guests, one per line, newest first.
Keep three `var`s, move each one down a line when someone new walks in, and
`sanitize` every name before it is stored.

```wiring
zone "Zone Ref" -> chip room
chip text -> decal Text
```

## Good to Know

- Every place that shows text reads tags, chat included, so a name in a
  chat line can restyle that line too. Escape any text a player chose before
  you show it, wherever it goes.
- `escape` only changes `<`. Text without a `<` comes out unchanged.

Next: [Matching Text](matching-text.md).
