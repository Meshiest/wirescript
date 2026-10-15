# Templates Chosen at Run Time

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Word Forms](word-forms.md), [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Score Card](score-card.md)

</div>
<!-- /learn-links -->

The format of an `Fmt` is an input like any other. It can come from a list,
or be built while the circuit runs. This lesson uses that for message lists
and lookup tables, and gives one rule for keeping them safe.

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
```

## Code

```wirescript name=tp5-greeter
import { escape } from "sanitize"

const GREETINGS = [
  "Welcome, {A}!",
  "Good to see you, {A}.",
  "{A} is visitor number {B}.",
]

@left in room: zone
var visitors: int = 0

on ZoneEntered(zone = room) -> (character) {
  visitors += 1
  let template = GREETINGS[(visitors - 1) % 3]
  character.ShowChatMessage(Fmt(template, escape(character.GetDisplayName()), visitors))
}
```

`[ ... ]` is a list of values, an array. `GREETINGS[i]` reads the item at
position `i`, counting from 0. `%` is the remainder after dividing, so
`(visitors - 1) % 3` goes 0, 1, 2, 0, 1, 2 as visitors arrive. The handler
picks a template from the list, and `Fmt` fills it with the visitor's name and
number. The first two templates have no `{B}`, so they leave the number out.

`escape` makes the name safe to show, as in
[Reacting to the Game](../reacting-to-the-game.md).

Which greeting does the fourth visitor get? The sixth?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   and wire its **Zone Ref** output to `room`.
2. Walk into the zone, out, and in again a few times. Each entry shows the
   next greeting in your chat. Only the player who walked in sees it.

## Under the Hood

Open the chip. The list is one **Array Var** gate holding the three
templates. In the handler, a **Get Array Element** step reads one of them,
and its output is wired into the Format String gate's **Format String**
input. The format is no longer text set inside the gate: it changes from one
run of the handler to the next.

Reading an item at a position that changes is a step in a handler. Outside a
handler, `GREETINGS[i]` is an error (`WS007`).

### Fmt as a lookup table

A format can also be built from a number, so that it picks one of the values:

```wirescript name=tp5-levels
@left in presses: int
let level = presses % 3
let hex = Fmt('{' .. level .. '}', 'ff4040', 'ffc040', '40ff40')
@right out sign = '<color="${hex}">Level ${level}'
```

`'{' .. level .. '}'` builds the format `{0}`, `{1}` or `{2}` while the
circuit runs, and the gate prints that one input. One Format String gate is
a table of up to seven values, picked by number, with no handler needed.
Here it picks a color for a `<color>` tag
([Rich Text Markup](../../game-knowledge.md#rich-text-markup)). Wire the
`red` count of the counter chip from [Interpolation](interpolation.md#run)
to `presses`, a Button's **Held** to the counter's `addRed`, and `sign` to a
Text Decal's **Text**.

Build this format with `..`, not `${}`. `"{${level}}"` would have its braces
doubled, as in [Interpolation](interpolation.md), and they would never make a
slot.

### Player text goes in a slot

Only text you wrote belongs in a format string. Text a player chose, such as
a display name or something typed, goes into a slot:

```wirescript nocompile
// Wrong: the name becomes part of the format.
character.ShowChatMessage(Fmt("Welcome, " .. name .. "!"))

// Right: the name is a value in slot A.
character.ShowChatMessage(Fmt("Welcome, {A}!", escape(name)))
```

In the wrong version, a player named `{A}` or `{0}` is read as a slot, and
the gate prints something else in place of their name. In a slot, the name is
only a value. `${}` is safe in the same way: `"Hello, ${escape(name)}"` puts
the name in a slot. Still `escape` it, because a `<` in a name starts a tag.

## Modify

1. Add a fourth greeting, and make the handler follow the list's length so
   you never have to change the `3`:
   `GREETINGS[(visitors - 1) % GREETINGS.length()]`.
2. Use a word form from [Word Forms](word-forms.md) in a greeting:
   `"{A} is our {B}{B}|ordinal(one=st,two=nd,few=rd,other=th) visitor."`.
3. Give the lookup table a fourth color, and change level only every five
   presses: `let level = clamp(presses / 5, 0, 3)`. `clamp(x, low, high)`
   keeps a value between two limits.

## Make

Make a health sign. A pin `hp: int` from 0 to 100 picks a level with
`clamp(hp / 34, 0, 2)`. Use one lookup `Fmt` for the color and a second for
the word (`Healthy`, `Hurt`, `Danger`), and show both in one template. Test it
with the counter chip's `red` count standing in for `hp`.

```wiring
chip2 "Counter"
chip "Health"
button Held -> chip2 addRed
chip2 red -> chip hp
chip text -> decal Text
```

## Good to Know

- Reading a list past its end gives the last value read, not the first item
  or an empty one. Keep the position in range, as `%` does here.
- A slot the gate cannot fill prints `0`, so a mistake in a built format shows
  up as a `0` on the sign.

See [String Formatting](../../builtins.md#string-formatting-pure) and
[`const` containers at runtime](../../statements.md#const-containers-at-runtime)
for the reference.

Next: [Score Card](score-card.md).
