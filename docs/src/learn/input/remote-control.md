# Remote Control

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Seat Controls](seat-controls.md), [Choosing a Value](../logic/choosing-a-value.md)

**Next steps:** [Ready Check](ready-check.md)

**Parts that use this:** [Paint Gun](../parts/paint-gun.md)

</div>
<!-- /learn-links -->

A Seat hands its rider to a circuit, but only while they sit. A Switch hands
over a player too. Its **Character** output holds whoever flipped it last,
wherever they go. This lesson builds a remote for a light. Flip the switch, walk away, and your keys still work the light.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Reading the Keys Once](#reading-the-keys-once)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
switch Character -> chip holder
chip shining -> light Enabled
chip brightness -> light Brightness
```

## Code

```wirescript name=i2-remote
@left in holder: character
let keys = InputReader(holder)
var lit: bool = false
var level: float = 2.0
@right out shining = lit.Value
@right out brightness = level.Value

on keys.PressedQ {
  lit = !lit
}

on keys.Forward > 0.5 {
  level = min(level + 0.5, 4.0)
}

on keys.Forward < -0.5 {
  level = max(level - 0.5, 0.0)
}
```

`holder` is wired from the Switch's **Character**. `InputReader(holder)` is
the same as `holder.InputReader()` from [Seat Controls](seat-controls.md).
It reads the holder's keys as they press them.

- `on keys.PressedQ` runs once each time Q goes down, and flips the light.
- `on keys.Forward > 0.5` runs once per push forward, and raises the level
  by half a step. Back lowers it.
- `min(..., 4.0)` and `max(..., 0.0)` keep the level between 0 and 4.

Nothing here runs on a timer. Every handler starts from a key.

One player flips the switch on and walks off. A second player flips it off.
Whose Q turns the light on now?

## Run

1. Compile the chip and paste it in-game. Place a **Switch** and a **Point
   Light**.
2. Wire the Switch's **Character** to `holder`. Wire `shining` to the Point
   Light's **Enabled** and `brightness` to its **Brightness**.
3. Flip the switch and walk away from it. Press Q and the light comes on.
   Push forward a few times and it gets brighter, one step per push. Pull
   back and it dims.
4. With a second player: they flip the switch, and now their Q works the
   light and yours does nothing. Which way the switch points does not
   matter. Flipping it hands over the remote.

## Under the Hood

Open the chip. `InputReader` is a **Seat Control Splitter**, the same
component as in Seat Controls. Its **Character** input is wired from
`holder`, so from the Switch. Its outputs are steady wires that change while
the holder presses keys. Nothing samples them.

The **Pressed Q** output is wired straight into the **Exec** input of a
**Get Variable**. An exec input fires when a bool wired into it turns on, so
the chain runs once per press: **Get Variable**, **NOT**, **Set Variable**.
**Input Forward** goes into a **Greater** and a **Less** gate, and their
outputs start the other two chains the same way, each with an **Add** or a
**Subtract**, a **Min** or a **Max**, and a **Set Variable**.

A Seat's **Sitting Character** empties when the rider stands up. A Switch's
**Character** does not. It changes only when someone else flips the switch.
That makes it a steady character wire to feed into `InputReader`.

## Reading the Keys Once

`GetInputs` reads the same fields as `InputReader`, but it is a step that
reads the keys at the moment a handler reaches it. Use it when a handler
already has a player from an event, and no character wire exists to feed a
splitter. This hallway greets anyone who walks in backwards:

```wirescript name=i2-moonwalk
@left in hall: zone
var walkers: int = 0
@right out count = walkers.Value

on ZoneEntered(zone = hall) -> (character) {
  if character.GetInputs().Forward < -0.5 {
    walkers += 1
    character.ShowStatusMessage("Nice moonwalk!")
  }
}
```

The zone event gives the player who entered, and one **Get Inputs** gate
reads whoever that is. Since it is a step, it cannot drive a pin outside a
handler:

```wirescript name=i2-pure expect=WS007
@left in holder: character
@right out forward = holder.GetInputs().Forward
```

```text
Error: [WS007] exec call 'GetInputs' outside an exec context (pass exec = ... to override)
```

Keep `GetInputs` for a one-time read like this. To follow someone's keys,
wire their character into `InputReader`.

## Modify

1. Let the remote work only while the switch is on. Add
   `@left in active: bool`, wire the Switch's **Enabled** to it, and write
   `on keys.PressedQ && active`. Do the same for the two forward handlers.
2. Make the light glide between levels instead of jumping. Wrap the output
   in a **Dampen** gate, which follows its target smoothly:
   `@right out brightness = Dampen(level.Value, 0.3)`. The second value is
   the gate's **Smooth Time**, and larger values catch up more slowly.
3. Show who holds the remote. Add `var shown: string = ""`,
   `@right out sign = shown.Value` wired to a **Text Decal**'s **Text**, and
   `on Change(holder) { shown = escape(holder.GetDisplayName()) }`, with
   `import { escape } from "sanitize"` at the top, as in
   [Reacting to the Game](../reacting-to-the-game.md). A name can contain
   `<`, and `escape` keeps it from reading as formatting.

## Make

Make a remote door. A Switch hands the remote to a player. Pushing forward
opens the door (wire a `solid` output to a **Brick Property Changer**'s
**Collision Enabled** and **Visibility**), and pulling back closes it. Q
locks the door: while it is locked, forward does nothing, until Q is
pressed again.

```wiring
switch Character -> chip holder
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Good to Know

- A Switch's **Character** keeps the last player who flipped it, wherever
  they walk. A respawn makes a new character, so after dying the holder
  flips the switch again to take the remote back.
- A handler on a key runs once per press, when the key goes down, not on
  every tick it stays held.
- E flips any switch and presses any button the player looks at. That is
  why the light toggles on Q.
- One `InputReader` reads one player. To read the keys of every player at
  once, give each player a chip of their own, as in
  [One Chip per Player](../prefabs/one-chip-per-player.md).
- `GetInputs` reads any player, wherever they are, but only at the moment
  it runs. A key pressed and released between two reads is never seen.

See [InputReader](../../builtins.md#inputreader) and
[GetInputs](../../builtins.md#getinputs) in the reference.

Next: [Ready Check](ready-check.md).
