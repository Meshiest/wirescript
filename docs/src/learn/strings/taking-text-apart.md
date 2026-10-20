# Taking Text Apart

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Text Values](text-values.md), [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Safe Text](safe-text.md), [Matching Text](matching-text.md), [Chat Commands](../admin/chat-commands.md)

</div>
<!-- /learn-links -->

Text often holds more than you need: a player's full display name when you
want only the first word, or a number written as digits. This lesson cuts
text into pieces, changes its case, and reads numbers out of it. The text
comes from players walking into a zone, so no admin rights are needed.

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

```wirescript name=s2-greeter
import { escape } from "sanitize"

@left in room: zone
var visitor: string = "Nobody yet"
@right out text = "Last visitor: ${visitor.Value}"

on ZoneEntered(zone = room) -> (character) {
  let name = character.GetDisplayName().Trim()
  let parts = name.Split(" ")
  let first = if parts.Found then parts.Left else name
  character.ShowChatMessage("Hi ${escape(first)}!")
  let initial = name.Substring(0, 1).ToUpper()
  visitor = "${escape(initial)} (${name.Length()} characters)"
}
```

The zone handler is the one from [Reacting to the Game](../reacting-to-the-game.md).
Each new piece is a method called on a string with a dot:

- `GetDisplayName()` gives the name the player chose to show.
- `Trim()` removes spaces from both ends.
- `Split(" ")` cuts the text at the first space. It gives three values:
  `Left` is the text before the space, `Right` is the text after it, and
  `Found` says whether there was a space at all.
- `Substring(0, 1)` takes 1 character, starting at position 0. Positions
  count from 0, so position 0 is the first character.
- `ToUpper()` turns letters into capitals. `ToLower()` does the opposite.

`if parts.Found then parts.Left else name` picks the first word when there is
a space, and the whole name when there is not. `escape(...)` is the helper
from Reacting to the Game that makes player text safe to show.
[Safe Text](safe-text.md) explains why it is needed.

A player named `sam the builder` walks in. What does the chat say to them,
and what does the sign say? What changes for a player named `Sam`?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   and wire its **Zone Ref** to `room`.
2. Wire `text` to a **Text Decal**'s **Text**.
3. Walk into the zone. Chat greets you by the first word of your name, and
   the sign shows your initial in capitals and how long your name is.
4. Ask a friend with a different name to walk in, or change your display name
   and walk in again. The sign shows whoever came last.

## Under the Hood

Open the chip. The handler's chain starts at the zone event, then passes
through a **Get User Display Name** gate. Reading a name is a step in the
chain, so `GetDisplayName()` only works inside a handler.

After it come gates that need no exec: **Trim String**, **Split String**
(with outputs named **Left**, **Right** and **Found**), a **Select** that
picks the first word or the whole name, **Get Substring**, **String to
Upper** and **Get String Length**. Two **Replace in String** gates are the
two `escape` calls. The chain ends with **Show Chat Message** and the Set
Variable that updates the sign.

## Modify

1. Greet the rest of the name too. `parts.Right` is everything after the
   first space, so use it only when `parts.Found` says there was one:

   ```wirescript name=s2-rest
   import { escape } from "sanitize"

   @left in room: zone

   on ZoneEntered(zone = room) -> (character) {
     let name = character.GetDisplayName().Trim()
     let parts = name.Split(" ")
     if parts.Found {
       character.ShowChatMessage("Hi ${escape(parts.Left)}, also known as ${escape(parts.Right)}!")
     } else {
       character.ShowChatMessage("Hi ${escape(name)}!")
     }
   }
   ```

2. `Find` gives the position where some text first appears, or -1 when it
   does not appear. Because positions count from 0, the position of the first
   space is also the length of the first word:

   ```wirescript name=s2-find
   @left in room: zone

   on ZoneEntered(zone = room) -> (character) {
     let name = character.GetDisplayName().Trim()
     let space = name.Find(" ")
     if space < 0 {
       character.ShowChatMessage("One word, ${name.Length()} characters")
     } else {
       character.ShowChatMessage("Your first word has ${space} characters")
     }
   }
   ```

3. Text can hold a number. `ParseNumber()` reads it, and gives two values:
   `Value`, the number, and `Success`, which is false when the text is not a
   number. This keypad builds text from button presses, then sets a light's
   brightness from it:

   ```wirescript name=s2-keypad
   @left in one: exec
   @left in two: exec
   @left in enter: exec
   var typed: string = ""
   var level: float = 5.0
   @right out shown = typed.Value
   @right out brightness = level.Value

   on one {
     typed = typed .. "1"
   }

   on two {
     typed = typed .. "2"
   }

   on enter {
     let n = typed.ParseNumber()
     if n.Success {
       level = n.Value
     }
     typed = ""
   }
   ```

   Wire three **Button**s' **Held** to `one`, `two` and `enter`, `shown` to a
   **Text Decal**'s **Text**, and `brightness` to a **Point Light**'s
   **Brightness**. Type `12` and press enter. `ParseInt()` works the same way
   for whole numbers, with an `int` for its `Value`.

## Make

Make a keypad lock: three digit buttons, an enter button and a door. Pressing
enter reads the typed digits with `ParseInt()`, opens the door for three
seconds when the number equals a `const CODE = 213`, and clears the typed
text either way. Show the typed digits on a sign while the player types.

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

- `Length()` and `Substring` count code points, so a character such as `★`
  is one character, not two.
- A display name is chosen by its player. It can be one word or many, and it
  can contain `<`. Trim it before you split it, and escape any part of it you
  show.

Next: [Rich Text](rich-text.md).
