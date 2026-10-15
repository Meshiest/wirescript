# Rich Text

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Text Values](text-values.md)

**Next steps:** [Safe Text](safe-text.md), [Score Card](../templates/score-card.md), [Text on Screen](../screen/text-on-screen.md)

**Parts that use this:** [Shooting Gallery](../parts/shooting-gallery.md), [Scoreboard HUD](../parts/scoreboard-hud.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

Text the game shows can carry formatting: bold, color, size, line breaks and
small icons. The formatting is written as tags inside the string itself, so
a circuit can change how a sign looks the same way it changes what it says.
This lesson builds a shop sign driven by two switches.

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
switch1 Enabled -> chip shopOpen
switch2 Enabled -> chip busy
chip text -> decal Text
```

## Code

```wirescript name=s3-status-sign
@left in shopOpen: bool
@left in busy: bool
let word = if shopOpen then "OPEN" else "CLOSED"
let hex = if shopOpen then "44dd44" else "dd4444"
let note = if busy then "<i>Busy, please wait</>" else "Come on in"
@right out text = '<size="48"><b><color="${hex}">${word}</></></><br>${note}'
```

A tag is a word between `<` and `>`. The game reads it as an instruction and
does not show it:

- `<size="48">` makes the text after it 48 points tall.
- `<b>` makes it bold, and `<i>` makes it italic.
- `<color="44dd44">` colors it. The color is six hex digits: two each for
  red, green and blue, so `44dd44` is green and `dd4444` is red.
- `<br>` starts a new line.
- `</>` closes the most recently opened tag. The three `</>` after `${word}`
  close the color, then the bold, then the size, so the second line is
  ordinary text.

The template uses single quotes, so the double quotes inside each tag need
no escaping.

What does the sign show with `shopOpen` on and `busy` on? With both off?

## Run

1. Compile the chip and paste it in-game. Place two **Switch**es and a brick
   with a **Text Decal**.
2. Wire one Switch's **Enabled** to `shopOpen` and the other's to `busy`.
3. Wire `text` to the Text Decal's **Text**.
4. With both switches off, the sign shows a large red bold `CLOSED` over a
   smaller `Come on in`. Flip `shopOpen`: the word turns green and reads
   `OPEN`. Flip `busy`: the second line changes to `Busy, please wait` in
   italics.

## Under the Hood

Open the chip. There are three **Select** gates, one for each `if`, and one
**Format String** gate. The Format String's template holds every tag, and
the Selects fill in the color, the word and the note. To the circuit, a tag
is only text. The sign is what reads it.

The `if` choices are bound to names with `let` before they go into the
template. That keeps the template short, and it is also the safe way to
write it: put a choice in a `let`, then put the name inside `${...}`.

## Modify

1. A color tag does not need a closing `</>`. It lasts until another color
   tag replaces it, even across a `<br>`. Color both lines, with no `</>` at
   all:

   ```wirescript name=s3-carry
   @left in shopOpen: bool
   let status = if shopOpen then '<color="44dd44">OPEN' else '<color="dd4444">CLOSED'
   @right out text = '${status}<br>Snacks and drinks<br><color="ffffff">Ask inside'
   ```

   The second line takes the status color, and the third line is white
   because a new color tag replaced it.

2. Use a character that is not on your keyboard. There is no escape code for
   it: copy the character and paste it into the string, for example
   `'<size="40">★ Open ★'`. It counts as one character in `Length()`.

## Make

Make a sign for a door with two switches, `locked` and `alarm`. It shows a
green `Unlocked` or a red `Locked` on the first line, and while `alarm` is on,
a second line in bold yellow that reads `ALARM`.

```wiring
switch1 Enabled -> chip locked
switch2 Enabled -> chip alarm
chip text -> decal Text
```

## Good to Know

- Tags work anywhere the game shows text: Text Decals, chat, status
  messages. [Rich Text Markup](../../game-knowledge.md#rich-text-markup)
  lists them all, including fonts.
- `<icon>Name</>` draws one of the game's own icons inline, at the size of
  the text around it. These docs do not list the icon names yet.
- A wire carries at most 10,000 characters of text, and the rest is cut off.
  Every tag counts toward that, so a sign that colors every character can hit
  the limit long before it looks full.
- A player's own name can contain tags too. Showing it unchanged lets it
  restyle your sign. [Safe Text](safe-text.md) fixes that.

Next: [Safe Text](safe-text.md).
