# Text Values

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remembering Values](../state.md)

**Next steps:** [Taking Text Apart](taking-text-apart.md), [Rich Text](rich-text.md)

</div>
<!-- /learn-links -->

Signs, chat lines and labels all carry text. In Wirescript a piece of text is
a `string`, and it moves along wires like any other value. This lesson builds
a shop sign that two buttons change, and a light that shows whether the sign
has anything on it.

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
button1 Held -> chip away
button2 Held -> chip back
chip text -> decal1 Text
chip count -> decal2 Text
chip lit -> light Enabled
```

## Code

```wirescript name=s1-away-sign
const AWAY = "Back in 5 minutes"

@left in away: exec
@left in back: exec
var note: string = ""
@right out text = "Shop: " .. note.Value
@right out lit: bool = note.Value
@right out count = "${note.Value.Length()} characters"

on away {
  note = AWAY
}

on back {
  note = ""
}
```

Text goes between quotes. `""` is the empty string: text with nothing in it.
`var note: string = ""` is a variable that holds text and starts empty.

`..` joins two pieces of text into one, so `"Shop: " .. note.Value` is the
words `Shop: ` followed by whatever the note says.

`note.Value.Length()` counts the characters in the note.

`lit` is declared `bool`, but it is given a string. Where a `bool` is wanted,
a string reads as false when it is empty and true when it holds anything at
all.

What does each output show before any press, after pressing `away`, and after
pressing `back`?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s, two bricks
   with a **Text Decal** each, and a **Point Light**.
2. Wire one Button's **Held** to `away` and the other's to `back`.
3. Wire `text` to one Text Decal's **Text** and `count` to the other's
   **Text**. Wire `lit` to the Point Light's **Enabled**.
4. At first the sign reads `Shop: `, the count reads `0 characters`, and the
   light is off.
5. Press `away`. The sign reads `Shop: Back in 5 minutes`, the count reads
   `17 characters`, and the light comes on. Press `back` and all three return
   to how they started.

## Under the Hood

Open the chip. `note` is a Variable gate, and each handler is one Set
Variable that writes a fixed text into it.

- `text` comes from an **Append String** gate, the game's join gate. Its
  inputs are `Shop: ` and the variable's value.
- `count` comes from a **Get String Length** gate feeding a **Format
  String** gate. That is the `${...}` from [Remembering Values](../state.md): one gate that
  fills values into a template.
- `lit` comes from a **Not Equal** gate that compares the note with `""`.
  You did not write it: the compiler adds it wherever a string is used as a
  `bool`.

When every piece of a join is fixed text, the compiler joins it for you and
no Append String is placed for it. Only a join with a value that can change
needs a gate.

### Quotes and escapes

A string can use double quotes or single quotes. They mean the same thing,
so pick the one that saves you work. Single quotes let the text hold double
quotes as they are:

```wirescript nocompile
'Ring the "bell"'
"Ring the \"bell\""
```

Both lines are the same text. A backslash in front of a character is an
escape: `\"` is a double quote inside double quotes, `\'` a single quote
inside single quotes, `\\` a backslash, and `\$` a dollar sign that does not
start a `${...}`. [Escape Sequences](../../syntax.md#escape-sequences) lists
them all. For a new line on a sign, use the `<br>` tag from the
[Rich Text](rich-text.md) lesson.

## Modify

1. Change `AWAY` to `'Ring the "bell"'`. Compile and press `away`: the sign
   shows the quotes. How many characters does the count show now?
2. `==` compares two strings and gives a `bool`. Add a pin that is on only
   while the note says exactly `AWAY`, and wire it to a second light:

   ```wirescript name=s1-is-away
   const AWAY = "Back in 5 minutes"

   @left in away: exec
   @left in back: exec
   var note: string = ""
   @right out text = "Shop: " .. note.Value
   @right out isAway = note.Value == AWAY

   on away {
     note = AWAY
   }

   on back {
     note = ""
   }
   ```

   `!=` is the opposite: true while the two texts differ.

3. Use one button for both jobs. Inside a handler, `if note` is the same
   test as `lit`: true while the note is not empty.

   ```wirescript name=s1-toggle
   const AWAY = "Back in 5 minutes"

   @left in pressed: exec
   var note: string = ""
   @right out text = "Shop: " .. note.Value

   on pressed {
     if note {
       note = ""
     } else {
       note = AWAY
     }
   }
   ```

   Open the chip: the `if` is a Branch gate, and its condition is the same
   Not Equal against `""` that the compiler added for `lit`.

## Make

Make a doorbell sign: pressing a **Button** shows `Ding!` on a Text Decal for
three seconds, then clears it, and a light stays on while the sign has text.
`await Sleep(_, delay = 3.0)` from [Pure and Exec](../pure-and-exec.md) does
the waiting.

```wiring
button Held -> chip ring
chip text -> decal Text
chip lit -> light Enabled
```

## Good to Know

- `Length()` counts characters as the game sees them (code points), so `★`
  is 1. Some emoji are several code points joined together: a figure with a
  skin tone and a gender sign counts 5, though it draws as one picture.
- Only the empty string is false. `" "`, `"0"` and `"false"` all hold
  something, so they read as true.
- An input pin that nothing is wired to reads as empty text.

Next: [Taking Text Apart](taking-text-apart.md).
