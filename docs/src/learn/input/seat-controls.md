# Seat Controls

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Remote Control](remote-control.md), [One Chip per Player](../prefabs/one-chip-per-player.md), [Over Time](../motion/over-time.md)

</div>
<!-- /learn-links -->

A seat knows who is sitting in it, and a circuit can read that player's keys.
This lesson turns a seat into a joystick that drives a light.

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
seat "Sitting Character" -> chip rider
chip brightness -> light Brightness
chip readout -> decal Text
switch Enabled -> light Enabled
```

## Code

```wirescript name=i1-joystick
@left in rider: character
let input = rider.InputReader()
@right out brightness = (input.Forward + 1.0) * 2.0
@right out readout = "Forward ${input.Forward}, Right ${input.Right}"
```

`rider: character` is a pin for a player's character, here the person sitting
in the seat. `rider.InputReader()` reads that player's controls. It gives a
record of fields, among them:

- `Forward`: the forward and back keys, from -1 (back) to 1 (forward),
- `Right`: the left and right keys, from -1 to 1,
- `PressedQ`, `PressedE`, `PressedC`: `true` while that key is held.

`input.Forward + 1.0` runs from 0 to 2, so `brightness` runs from 0 (holding
back) to 4 (holding forward).

Someone sits down and presses nothing. How bright is the light? What does
the readout say?

## Run

1. Compile the chip and paste it in-game. Place a **Seat**, a **Point
   Light** and a brick with a **Text Decal**.
2. Wire the Seat's **Sitting Character** to `rider`. Wire `brightness` to the
   Point Light's **Brightness**, and `readout` to the Text Decal's **Text**.
3. Give the light's **Enabled** a **Switch** of its own, flipped on.
4. Sit in the seat. Hold forward and the light brightens. Hold back and it
   dims. Let go and it returns to the middle. The readout follows every key.

## Under the Hood

Open the chip. `InputReader` is not a gate. It is a **Seat Control
Splitter**, the brick the game offers for splitting a seat's controls into
separate wires. Its **Character** input is wired from `rider`, and its
outputs (**Input Forward**, **Input Right**, **Pressed Q** and the rest) feed
the gates after it: an **Add** and a **Multiply** for the brightness, and a
**Format String** for the readout.

Every gate here is pure. Nothing waits for a pulse. The splitter's outputs
change while the player presses keys, the same way a Switch's **Enabled**
does, and the light follows on the same tick.

`InputReader` reads one player, the one wired into it. A second seat needs a
second splitter, wired from its own Sitting Character.

## Modify

1. Steer a second light with `input.Right`, so left dims it and right
   brightens it.
2. Make a horn. Add `@right out honk: exec` and
   `on input.PressedQ { emit honk }`, and wire `honk` to a **One Shot
   Audio**'s **Play** with a sound picked.
3. Count how many times the rider pushes forward. Holding a key is not a
   press, but a comparison turns it into one:

   ```wirescript name=i1-steps
   @left in rider: character
   let input = rider.InputReader()
   var pushes: int = 0
   @right out readout = "Pushes: ${pushes.Value}"

   on input.Forward > 0.5 {
     pushes += 1
   }
   ```

   `input.Forward > 0.5` turns on when the key goes down, so the handler runs
   once per push, however long it is held.

## Make

Make a seat that works a drawbridge sign. Holding forward for a full second
shows "Bridge down" on a Text Decal, and holding back for a full second shows
"Bridge up". A short tap does nothing. Use `await Sleep(...)` inside the
handler, then check the key again. Keep the sign's text in a
`var shown: string`.

```wiring
seat "Sitting Character" -> chip rider
chip sign -> decal Text
```

## Good to Know

- A Seat's **Sitting Character** is empty while nobody sits in it. Keep the
  rest of your build sensible when `rider` is empty.
- A handler on a held key runs once per press, when the value turns on, not
  on every tick it stays held.
- `Forward` and `Right` are decimals between -1 and 1. Compare them with
  `> 0.5` rather than `== 1.0`.

See [InputReader](../../builtins.md#inputreader) in the reference.

Next: [Remote Control](remote-control.md).
