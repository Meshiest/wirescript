# Custom Events

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Exec Pins](exec-pins.md)

**Next steps:** [Waiting](waiting.md), [Spawning a Prefab](../prefabs/spawning.md), [Multiplayer Architecture](../mastery/multiplayer.md)

**Parts that use this:** [Obstacle Course](../parts/obstacle-course.md)

</div>
<!-- /learn-links -->

Two chips can talk without a wire between them. One sends a named event, and
every chip listening for that name runs a handler. This lesson splits the
doorbell in two: a button chip by the door and a light chip anywhere else in
your build.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Constants do not travel](#constants-do-not-travel)
- [Sending to one grid](#sending-to-one-grid)
- [Global events](#global-events)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
chip "Button chip"
button Held -> chip pressed
```

```wiring
chip "Light chip"
chip light -> light Enabled
chip sign -> decal Text
```

## Code

The button chip:

```wirescript name=e2-button
@left in pressed: exec
var presses: int = 0

on pressed {
  presses += 1
  SendCustomEvent("doorbell", presses)
}
```

The light chip:

```wirescript name=e2-light
var rings: int = 0
var lit: bool = false
@right out light = lit.Value
@right out sign = "Rings: ${rings.Value}"

on CustomEvent("doorbell") -> (count: int) {
  rings = count
  lit = !lit
}
```

`SendCustomEvent("doorbell", presses)` sends an event on the channel
`"doorbell"`, carrying one value. `on CustomEvent("doorbell")` runs whenever
an event on that channel arrives, and `-> (count: int)` names the value it
carried. A channel name is any text you choose. Both ends must spell it the
same way.

Each event can carry up to eight values, in order. The receiver lists them in
the same order, with the same types the sender sends: here one `int`.

The two chips share no wire. Paste a second copy of the light chip. What
happens to it when you press the button?

## Run

1. Compile both chips and paste them in-game. Place a **Button** by the
   first, and a **Point Light** and a brick with a **Text Decal** by the
   second.
2. Wire the Button's **Held** to `pressed`. Wire `light` to the Point
   Light's **Enabled** and `sign` to the Text Decal's **Text**.
3. Press the button. The light toggles and the sign counts the presses.
4. Paste the light chip again, with its own light. Both lights now toggle on
   every press: every chip listening on `"doorbell"` hears it.

## Under the Hood

Open the button chip. After the **Increment Variable** comes a **Get
Variable** that reads `presses`, then a **Send Personal Custom Event** gate.
The channel name is typed into the gate, and the count is wired into its
**Data In1**.

Open the light chip. A **Personal Custom Event** gate, with `doorbell` typed
in, starts the handler. Its **Data Out1** is wired into the Set Variable for
`rings`.

"Personal" means the event reaches only circuits pasted by the same player.
A light chip someone else pasted does not hear your button.

The handler runs on the tick after the send, not in the same tick. An event
is delivered on the next frame, so a reply to an event, and the reply to
that, each cost a tick.

The light chip has no sender in its own code, so it cannot work out the
type of `count` by itself. Leave the type off and the compiler warns:

```wirescript name=e2-untyped expect=WS042
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

on CustomEvent("doorbell") -> (count) {
  rings = count
}
```

```text
WARN [WS042] custom event 'doorbell' data param 'count' (#1): no in-unit sender to infer its type from, so it defaults to float; annotate `count: <type>` to silence
```

The sender decides what each slot carries. The receiver's types have to
match it, and a mismatch is not always reported, so write the types out on
every receiver.

## Constants do not travel

Send a fixed number and the event arrives, but the number may not:

```wirescript name=e2-constant
@left in pressed: exec

on pressed {
  SendCustomEvent("doorbell", 1)
}
```

This compiles with no warning. Open the chip: **Data In1** has no wire. The
`1` is typed into the gate's box instead, and the receiver can read 0 there.
Put the value in a variable (`var door: int = 1`) and send `door`. The chip
then reads it with a Get Variable and wires it into Data In1.

## Sending to one grid

Every listener hears an ordinary event. To answer just one chip, send to its
grid. Here the button chip sends its own grid along with the count, and the
light chip answers on a second channel:

```wirescript name=e2-reply-button
@left in pressed: exec
var presses: int = 0
var heard: bool = false
@right out lamp = heard.Value

on pressed {
  presses += 1
  SendCustomEvent("doorbell", presses, ReadBrickGrid())
}

on CustomEvent("doorbell.heard", isObject = true) {
  heard = !heard
}
```

```wirescript name=e2-reply-light
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

on CustomEvent("doorbell") -> (count: int, door: entity) {
  rings = count
  door.SendCustomEvent("doorbell.heard")
}
```

`ReadBrickGrid()` is the grid the chip sits on, as an `entity`.
`door.SendCustomEvent(...)` sends to that grid only. A sent-to event is an
object event, and only a receiver marked `isObject = true` hears one. Without
it, `on CustomEvent("doorbell.heard")` never runs, and nothing reports why.

Chips placed straight on the ground all sit on the same grid, so a reply sent
there reaches every `isObject` listener on the ground. Sending to a grid
pays off when chips sit on separate grids, such as a vehicle or a copy the
circuit spawned itself.

## Global events

An ordinary custom event is personal: it reaches only circuits pasted by the
player who pasted the sender. A global event reaches every listening circuit,
whoever pasted it. Send one with `SendGlobalCustomEvent` and listen with
`on GlobalCustomEvent`:

```wirescript name=e2-global-button admin
@left in pressed: exec
var presses: int = 0

on pressed {
  presses += 1
  SendGlobalCustomEvent("doorbell", presses)
}
```

```wirescript name=e2-global-light
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

on GlobalCustomEvent("doorbell") -> (count: int) {
  rings = count
}
```

How they differ from personal events:

- **Who hears it.** Every `on GlobalCustomEvent` with the channel's name runs,
  in every player's circuits.
- **Separate channels.** A global `"doorbell"` and a personal `"doorbell"`
  never meet: `SendGlobalCustomEvent` does not reach `on CustomEvent`, and
  `SendCustomEvent` does not reach `on GlobalCustomEvent`.
- **Admin to send.** **Send Global Custom Event** is an
  [admin gate](../admin/admin-gates.md), so only an admin can load the button
  chip. Listening needs no admin: anyone's chip can react to an admin's
  global events.

The rest works the same way: up to 8 values, `target` with `isObject = true`
for one grid, and the receiver runs on the tick after the send. Use personal
events inside one player's build, and global events when circuits pasted by
different players must talk, such as a server's round system that everyone's
chips follow.

## Modify

1. Send a second value: add `var door: int = 1` to the button chip and send
   `SendCustomEvent("doorbell", presses, door)`. Receive it as
   `-> (count: int, door: int)` and show it on the sign. Paste a second
   button chip with `door = 2`, so the sign says which door rang.
2. Make the light chip's sign show "Back door" or "Front door" instead of the
   number.
3. In the reply version, wire the button chip's `lamp` to a small Point Light
   beside the button, so the person ringing sees that someone heard.

## Make

Make a call board with three buttons in different rooms and one board in a
fourth room. Each button chip sends its room number. The board shows the last
room that called and lights a lamp for it until a reset button on the board
is pressed.

```wiring
chip "Board"
chip2 "Call"
button1 Held -> chip2 pressed
button2 Held -> chip reset
chip sign -> decal Text
chip lamp1 -> light1 Enabled
chip lamp2 -> light2 Enabled
chip lamp3 -> light3 Enabled
```

## Good to Know

- Events reach only circuits pasted by the same player. A chip a friend
  pasted does not hear yours, unless the event is global.
- The receiver runs on the tick after the send.
- A constant argument is not wired, and the receiver can read 0 or an empty
  string. Send a `var` instead.
- A send to a grid needs `isObject = true` on the receiver, and a receiver
  marked that way hears only sends to its grid.
- One send gate fired many times in a single tick does not deliver every
  time. The game logs "Too many events at once!" for that channel. Separate
  send gates firing once each on the same channel all deliver.
- When several events arrive in one tick, their order is not fixed.

See [Custom Events](../../builtins.md#custom-events) in the reference.

Next: [Waiting](waiting.md).
