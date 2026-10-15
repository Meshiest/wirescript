# Text on Screen

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Messages to One Player](messages-to-one-player.md), [Rich Text](../strings/rich-text.md)

**Next steps:** [Keeping It Current](keeping-it-current.md)

</div>
<!-- /learn-links -->

A message from the last lesson shows up where the game puts it.
`DisplayText` draws text on one player's screen at a place you choose, in
the size and color you choose. This lesson shows a power switch's state to
the player who flipped it.

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
switch Enabled -> chip power
switch Character -> chip flipper
```

## Code

```wirescript name=sc2-power-readout
@left in power: bool
@left in flipper: character

on Change(power) {
  let word = if power then '<color="44dd44">ON' else '<color="dd4444">OFF'
  flipper.DisplayText('<b>Power</>: ${word}',
    anchorX = 0.0, anchorY = 0.0,
    positionX = 40.0, positionY = 40.0,
    justify = Left, fontSize = 28)
}
```

- `on Change(power)` runs each time the switch turns on or off.
- `flipper` is wired from the Switch's **Character**, which holds the last
  player who flipped it. The text goes to that player only.
- The text is ordinary [rich text](../strings/rich-text.md): `Power` in
  bold, then the state in green or red.
- The settings after the text say where it goes and how it looks.

Two pairs of numbers set where it goes:

- **The anchor** (`anchorX`, `anchorY`) picks a point on the screen, as a
  fraction of its width and height: `0` is the left or top edge, `0.5` the
  middle, `1` the right or bottom edge. `0.0, 0.0` is the top left corner.
- **The position** (`positionX`, `positionY`) moves the text from that
  point, in screen units, which are about one pixel each on a 1080p
  screen. `40.0, 40.0` is 40 units right and 40 down from the corner.

`justify = Left` lines the text up on its left side, and `fontSize = 28`
makes it larger than the default 16.

You flip the switch on, and a moment later a friend flips it off. What does
each of you see, and where?

## Run

1. Compile the chip and paste it in-game. Place a **Switch**.
2. Wire the Switch's **Enabled** to `power` and its **Character** to
   `flipper`.
3. Flip the switch. Near the top left corner of your screen, `Power: ON`
   appears with `ON` in green. A few seconds later it goes away.
4. Flip it again: `Power: OFF`, in red.
5. Ask a friend to flip it. The text appears on their screen, not yours.

## Under the Hood

Open the chip. There are four gates besides the pins: a **Change
Detector (Exec)** on `power`, a **Select** that picks the colored word, a
**Format String** that builds the text, and the **Display Text** gate
itself. The **Change Detector (Exec)**'s **On Changed** fires **Display
Text**'s **Exec**. `flipper` wires into **Player State**, and the Format
String into **Text**.

Everything else is typed into **Display Text** as settings: **Anchor** is
`(0, 0)`, **Position** is `(40, 40)`, **Font Size** 28, **Justification**
Left. **Anchor** and **Position** each have two halves, **X** and **Y**,
and a value that changes while the game runs wires into one half: write
`positionY = row * 30.0` and the result wires into **Position**'s **Y**.

So there is no `position` parameter that takes a vector:

```wirescript name=sc2-position-vector expect=WS041
@left in power: bool
@left in flipper: character

on Change(power) {
  flipper.DisplayText("Power", position = Vec(40.0, 40.0, 0.0))
}
```

```text
Error: [WS041] 'DisplayText' has no parameter 'position'
```

Every setting you leave out keeps the gate's default: the anchor in the
middle of the screen, no position offset, size 16, white text with an
outline, and gone after 5 seconds. [DisplayText](../../builtins.md#displaytext)
lists every setting.

## Modify

1. Move the readout to the bottom middle of the screen: `anchorX = 0.5`,
   `anchorY = 1.0`, `positionY = -120.0`, and `justify = Center`. The
   negative `positionY` moves it up from the bottom edge.

2. Greet the player in a **Seat**. Wire the Seat's **Is Occupied** to a
   `seated` pin and its **Sitting Character** to `rider`:

   ```wirescript name=sc2-seat
   @left in seated: exec
   @left in rider: character

   on seated {
     rider.DisplayText('<size="40"><b>Welcome aboard</></><br>Buckle up',
       anchorX = 0.5, anchorY = 1.0, positionY = -160.0,
       justify = Center, lifetime = 3.0)
   }
   ```

   `<size="40">` makes the first line larger than the rest, and `<br>`
   starts the second line inside the same text. `lifetime = 3.0` keeps it
   up for 3 seconds.

3. Add the flipper's name on a second line:

   ```wirescript name=sc2-flipper-name
   import { escape } from "sanitize"

   @left in power: bool
   @left in flipper: character

   on Change(power) {
     let word = if power then '<color="44dd44">ON' else '<color="dd4444">OFF'
     let name = escape(flipper.GetDisplayName())
     flipper.DisplayText('<b>Power</>: ${word}<br><color="ffffff">by ${name}',
       anchorX = 0.0, anchorY = 0.0,
       positionX = 40.0, positionY = 40.0,
       justify = Left, fontSize = 28)
   }
   ```

   The second color tag turns the second line white again.

## Make

Make a parking sign for a **Seat**. When a player sits down, they see a
card in the top right corner: a bold title, the name of the spot, and a
line in gray that says how many times anyone has sat there (a `var` you
count up). Use `anchorX = 1.0`, a negative `positionX`, and `justify =
Right`. Check it: sit down twice and read 1, then 2.

```wiring
seat "Is Occupied" -> chip seated
seat "Sitting Character" -> chip sitter
```

## Good to Know

- Anchor is a fraction of the screen, and position is in screen units.
  `positionX = 0.9` does not mean 90% across: it is less than one unit
  from the anchor. Text placed off the screen looks exactly like text that
  never drew, so check the numbers first.
- From a corner, move inward: from the right edge `positionX` is
  negative, and from the bottom edge `positionY` is negative.
- The default lifetime is 5 seconds. The next lesson keeps text up for as
  long as you need and updates it in place.
- The player input is called **Player State**, and a character wires
  straight in.
- A player's name can contain tags. Escape it before it goes into text
  ([Safe Text](../strings/safe-text.md)).

See [DisplayText Screen Placement](../../game-knowledge.md#displaytext-screen-placement)
in the reference.

Next: [Keeping It Current](keeping-it-current.md).
