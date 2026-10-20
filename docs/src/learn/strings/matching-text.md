# Matching Text

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Taking Text Apart](taking-text-apart.md), [Choosing a Value](../logic/choosing-a-value.md)

</div>
<!-- /learn-links -->

A chain of `if text == "..." ... else if text == "..."` gets long fast, and
cutting the useful part out of text takes a `Find` and a `Substring` every
time. `match` does both in one place: it lists what the text might look like,
runs the first arm that fits, and hands you the pieces. This lesson greets
players by the shape of their name, so no admin rights are needed.

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

```wirescript name=s5-greeter
import { escape } from "sanitize"

@left in room: zone
var said: string = "Nobody yet"
@right out text = said.Value

on ZoneEntered(zone = room) -> (character) {
  let name = character.GetDisplayName().Trim()
  let greeting = match name {
    "" => "Hello, stranger!",
    "Builder" | "Brickadian" => "Hello, fellow builder!",
    "${first} ${_}" => "Hi ${escape(first)}! Love the long name.",
    _ => "Hi ${escape(name)}!",
  }
  character.ShowChatMessage(greeting)
  said = greeting
}
```

`match name { ... }` checks `name` against each arm in order, top to bottom,
and gives the value after the `=>` of the first one that fits:

- `""` fits only the empty text. A plain string in an arm means "exactly this".
- `"Builder" | "Brickadian"` fits either one. `|` reads as "or".
- `"${first} ${_}"` looks like text with `${...}` in it, but in an arm it works
  the other way round. Instead of putting a value into the text, it takes one
  out: it fits any text with a space in it, and `first` becomes everything
  before the first space. `${_}` matches the rest without naming it.
- `_` fits anything. A `match` on text must end with it (or a name), since no
  list of exact strings covers every possible name.

The arms are tried in order, so put specific ones above general ones. If the
`_` arm came first, nothing below it would ever run, and the compiler would
warn you.

A player named `Builder` walks in. What does the chat say? What about `sam
the builder`, and `Sam`?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   and wire its **Zone Ref** to `room`.
2. Wire `text` to a **Text Decal**'s **Text**.
3. Walk into the zone. Chat greets you, and the sign shows the last greeting.
   A one-word name gets the last arm; a name with a space gets the third.

## Under the Hood

Open the chip. A `match` that gives a value is a stack of **Select** gates,
one for each arm except the last, just like the `if ... then ... else` from
[Choosing a Value](../logic/choosing-a-value.md):

- `""` is one **Compare Equal**.
- `"Builder" | "Brickadian"` is two **Compare Equal** gates joined by a
  **Logical OR**.
- `"${first} ${_}"` is a **Find in String** looking for `" "`, a **Compare
  Greater or Equal** checking that it found one, and a **Get Substring** that
  cuts out `first`. `${_}` needs no gate, since nothing reads it.

These are the same gates you would build by hand with `==`, `Find` and
`Substring`. `match` just writes them for you.

## Modify

1. An arm can have a condition after `if`, called a guard. The arm only runs
   when the pattern fits *and* the guard is true; when the guard is false,
   `match` moves on to the next arm. The guard can read the names the
   pattern took out:

   ```wirescript name=s5-guard
   import { escape } from "sanitize"

   @left in room: zone

   on ZoneEntered(zone = room) -> (character) {
     let name = character.GetDisplayName().Trim()
     character.ShowChatMessage(match name {
       "${first} ${_}" if first.Length() > 8 => "Hi ${escape(first)}, what a first name!",
       "${first} ${_}" => "Hi ${escape(first)}!",
       _ => "Hi ${escape(name)}!",
     })
   }
   ```

2. The text around the holes decides where they go. A hole at the end takes
   everything after the text before it, and a hole at the start takes
   everything before the text after it:

   ```wirescript name=s5-ends
   import { escape } from "sanitize"

   @left in room: zone

   on ZoneEntered(zone = room) -> (character) {
     let name = character.GetDisplayName().Trim()
     character.ShowChatMessage(match name {
       "Sir ${rest}" => "Welcome, Sir ${escape(rest)}.",
       "${who}Bot" => "Beep boop, ${escape(who)}.",
       "[${tag}] ${who}" => "${escape(who)} from team ${escape(tag)}",
       _ => "Hi!",
     })
   }
   ```

   `"Sir ${rest}"` can also be written `"Sir " .. rest`, joining the parts
   with `..` the way you join text. Both build the same gates.

3. `match` works on numbers too. An arm can be an exact number, or a range
   `low..high`, which fits numbers from `low` up to, but not including,
   `high`:

   ```wirescript name=s5-visits
   @left in room: zone
   var visits: int = 0

   on ZoneEntered(zone = room) -> (character) {
     visits += 1
     character.ShowChatMessage(match visits {
       1 => "You are the first visitor!",
       2..10 => "Visitor number ${visits}.",
       n if n % 100 == 0 => "Visitor ${n}, a round number!",
       _ => "Welcome back.",
     })
   }
   ```

## Make

Make a keypad lock: three digit buttons, an enter button and a door. The digit
buttons add `1`, `2` or `3` to a typed text, and a sign shows it. On enter,
`match` the typed text with arms that run steps instead of giving a value
(each arm is a `{ ... }` block):

- the code `"213"` opens the door for three seconds;
- a code starting with `3` says "Codes never start with 3" in chat;
- a code longer than three digits says "Too long" (a guard on
  `typed.Length()`);
- anything else says "Wrong code".

Clear the typed text after every try.

```wiring
button1 Held -> chip one
button2 Held -> chip two
button3 Held -> chip three
button4 Held -> chip enter
chip text -> decal Text
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Good to Know

- In a pattern, `${name}` makes a new name for the text it matched. It never
  reads a `name` that already exists, and the compiler warns when the two
  share a name. To compare against a value you already have, use a guard:
  `"hi ${n}" if n == friend => ...`.
- Matching is case-sensitive: `"builder"` does not fit `"Builder"`. Match on
  `name.ToLower()` and write the arms in lower case to ignore case.
- A literal between two holes is found at its first appearance:
  `"${a}-${b}"` splits `x-y-z` into `x` and `y-z`.
- `match` also works on [enums](../enums/unit-enums.md), where each arm names a
  variant.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
