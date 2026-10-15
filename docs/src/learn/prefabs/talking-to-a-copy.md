# Talking to a Copy

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Prefab Files](prefab-files.md)

**Next steps:** [Lifecycle and Storage](lifecycle.md)

</div>
<!-- /learn-links -->

A spawned copy sees none of the spawner's variables. To tell it
something, send it an event. This lesson spawns a
marker for each player who walks through a doorway, and tells the marker
whom to follow.

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

```wirescript name=p4-marker
@left in room: zone
var height: float = 120.0

on ZoneEntered(zone = room) -> (character) {
  let marker = SpawnPrefab(
    prefab = $```
      @label("Marker")
      var owner: character
      var lift: float = 0.0
      var ready: bool = false

      on CustomEvent("marker.init", isObject = true) -> (who: character, up: float) {
        owner = who
        lift = up
        ready = true
        on Clock(interval = 0.1) {
          if ready {
            if owner == null {
              ReadBrickGrid().DestroySpawnedPrefab()
            } else {
              ReadBrickGrid().SetLocation(owner.GetLocation() + Vec(0.0, 0.0, lift))
            }
          }
        }
      }
    ```,
    limit = 8
  )
  marker.SendCustomEvent("marker.init", character, height)
}
```

- `SpawnPrefab` gives back the copy it just spawned, as an `entity`. `let
  marker = ...` keeps it.
- `marker.SendCustomEvent("marker.init", character, height)` sends an event
  to that copy only, carrying the player and the height. The height is a
  `float`, a number that can have a fraction. Sending to one grid
  is from [Custom Events](../events/custom-events.md#sending-to-one-grid).
- Inside the copy, `on CustomEvent("marker.init", isObject = true)` hears it
  and stores both values in the copy's own variables.
- With no `offset`, the copy appears at the spawner.
- `ReadBrickGrid()` inside a copy is the copy itself. Every tenth of a
  second, `SetLocation` moves it to the player's position, raised by `lift`.
  A spawned copy can move itself like this. A pasted chip cannot.
- A player who leaves the server turns the stored `owner` into `null`.
  The marker then removes itself with `DestroySpawnedPrefab`, so no
  marker is left floating for a player who has gone. A death does the
  same a little later: the dead player's character turns into `null` some
  time after they die.

The Clock is written inside the init handler, next to the values it uses,
but it is a handler of its own and starts running the moment the copy
appears. What would it do on its first few pulses without the `if ready`
check?

## Run

1. Compile the chip and paste it in-game. Give a doorway an **Attached
   Zone** and wire its **Zone Ref** to `room`.
2. Walk through the doorway. A Marker chip appears at the spawner, jumps
   over your head, and from then on floats 120 units above it, following
   you around.
3. Walk through again: a second marker follows you too. A friend who walks
   through gets one of their own.

## Under the Hood

Open the spawner. After the **Spawn Prefab** gate comes a **Get Variable**
for `height` and a **Send Personal Custom Event**. The player and the height
are wired into **Data In1** and **Data In2**, and the Spawn Prefab gate's
**Entity** output is wired into the send's **Target**.

`height` is a `var` on purpose. A constant is not wired into an event: send
`120.0` directly and the copy can read 0 instead. A variable is read by a
Get Variable gate, whose output is wired in like the player is.

The copy is compiled on its own, so it cannot see the sender to work out its
event's types. Write them out (`who: character, up: float`), and keep them
in the same order and types as the send.

Three things here fail silently, with no error from the compiler:

- Leave out `isObject = true` and the copy never hears the event: a send to
  one grid is heard only by receivers marked that way.
- Leave out the `ready` check and the copy moves before the event arrives,
  with an empty `owner` and a `lift` of 0. The event arrives a tick after the
  send, and the copy's clock is already running by then.
- Send the height as `120.0` and the copy may read 0.

## Modify

1. Make the marker float in front of the player instead of above:
   `Vec(100.0, 0.0, 50.0)`. Is "in front" the same direction whichever way
   the player faces?
2. Remove `isObject = true`, compile, and walk through the doorway. Where
   does the marker go? Put it back.
3. Give the spawner a second zone that raises the newest marker. Keep it in
   a `var last: entity` as you spawn it. From the second zone's handler, send
   `last` a `"marker.height"` event carrying a `var taller: float = 300.0`,
   and give the copy a second receiver that sets `lift`.

## Make

Make a guide: when a player steps on a start pad, spawn a marker that sits
above a goal position (a `var goal: vector` the spawner sends along) until
that player gets within 200 units of it, then removes itself with
`ReadBrickGrid().DestroySpawnedPrefab()`. `Distance(a, b)` gives the distance
between two positions as a `float`.

```wiring
zone "Zone Ref" -> chip start
```

## Good to Know

- The copy's handlers are live from the moment it spawns, before any event
  from the spawner arrives, even one written inside the init handler.
  Guard work on a value the first event sets.
- After a death the marker holds the old body, so it stops following the
  player and removes itself once that body turns into `null`. Walking
  through the doorway again gives the new body a marker of its own.
- A send to one copy needs `isObject = true` on the copy's receiver, and
  nothing reports it when it is missing.
- A constant sent in an event can arrive as 0. Send a `var`.
- Events reach only circuits pasted by the same player, and a spawned copy
  counts as the spawner's. Copies spawned by a friend's chip do not hear
  yours.

See [Custom Events](../../builtins.md#custom-events) and
[SpawnPrefab](../../builtins.md#spawnprefab) in the reference.

Next: [Lifecycle and Storage](lifecycle.md).
