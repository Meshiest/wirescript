# Distance and Direction

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Vectors and Colors](../values/vectors-and-colors.md), [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Dot and Cross](dot-and-cross.md)

</div>
<!-- /learn-links -->

Every position in the world is a `vector`: three numbers, x, y and z.
Subtract two positions and you get the arrow from one to the other, with a
length (how far) and a direction (which way). This lesson builds a light that
brightens as a player walks toward it.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Arrows between points](#arrows-between-points)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip brightness -> light Brightness
chip readout -> decal Text
zone Occupied -> light Enabled
```

## Code

```wirescript name=sp1-proximity
const RANGE = 500.0

@left in room: zone
@left in occupied: bool

var visitor: character
var glow: float = 0.0
var shown: string = ""
@right out brightness = glow.Value
@right out readout = shown.Value

on ZoneEntered(zone = room) -> (character) {
  visitor = character
}

on Clock(interval = 0.1, enabled = occupied) {
  let here = GetOwnTransform().Location
  let there = visitor.GetLocation()
  let d = Distance(here, there)
  glow = clamp(1.0 - d / RANGE, 0.0, 1.0) * 4.0
  shown = "${round(d) as int} units away"
}
```

`var visitor: character` remembers the player who last walked into the zone.
Ten times a second, while someone is inside, the Clock handler measures:

- `GetOwnTransform().Location` is where the chip's own brick is.
- `visitor.GetLocation()` is where the player is.
- `Distance(here, there)` is how far apart the two points are, in the game's
  units.

`d / RANGE` is 0 at the chip and 1 at 500 units away, so `1.0 - d / RANGE`
runs the other way: 1 up close, 0 at the edge. `clamp(..., 0.0, 1.0)` keeps it
from going below 0 farther out, and `* 4.0` turns it into a brightness from 0
to 4. `round(d)` rounds to the nearest whole number and `as int` makes it an
`int`, so the sign shows no decimals.

A player stands 125 units from the chip. How bright is the light? And at 600
units?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   big enough to walk around in, and put the chip inside it. Place a **Point
   Light** and a brick with a **Text Decal** near the chip.
2. Wire the Attached Zone's **Zone Ref** to `room`, and its **Occupied** to
   `occupied` and to the Point Light's **Enabled**.
3. Wire `brightness` to the Point Light's **Brightness**, and `readout` to the
   Text Decal's **Text**.
4. Walk into the zone and toward the chip. If you were already inside when
   you pasted the chip, step out and back in: `visitor` is set when someone
   enters. The light grows brighter and the sign counts down. At 125 units
   the brightness is 3, at 250 it is 2, and from 500 units out it is 0.
   Leave the zone and the light goes off.

## Under the Hood

Open the chip. The **Clock** starts a chain: **Get Own Location and
Rotation** for the chip's position, a **Get Variable** for `visitor`, and
**Get Location of Entity** for the player. One **Distance** gate takes the two
positions. After it come a **Divide**, a **Subtract**, a **Clamp** and a
**Multiply** for the brightness, a **Round**, a **Convert** and a **Format
String** for the sign, and two **Set Variable** gates.

A distance is a number and a position is a vector, and the two cannot be
compared. Asking whether the arrow between them is short fails to compile:

```wirescript name=sp1-compare expect=WS004
@left in here: vector
@left in there: vector
@right out near = there - here < 500.0
```

```text
Error: [WS004] no overload for '<' on vector, float
```

Measure the arrow first, with `Magnitude` or `Distance`, and compare the
number.

## Arrows between points

`there - here` subtracts part by part and gives the arrow from `here` to
`there`. With `here` at `Vec(0.0, 0.0, 0.0)` and `there` at
`Vec(300.0, 400.0, 0.0)`, the arrow is `Vec(300.0, 400.0, 0.0)`.

- `Magnitude(there - here)` is the arrow's length: 500. `Distance(here,
  there)` is the same number in one gate.
- `Normalize(there - here)` keeps the direction and makes the length 1:
  `Vec(0.6, 0.8, 0.0)`. A length-1 vector is a pure direction.
- A direction times a distance gives the arrow back:
  `Vec(0.6, 0.8, 0.0) * 500.0` is `Vec(300.0, 400.0, 0.0)`. So
  `here + Normalize(there - here) * 100.0` is the point 100 units from `here`
  on the way to `there`.

These also work as methods: `(there - here).Magnitude()`,
`here.Distance(there)`. `DistanceSq` and `MagnitudeSq` give the same values
squared.

## Modify

1. Show which way the chip lies from the player. Add a second line to the sign:
   `shown = "${round(d) as int} units away<br>${Normalize(here - there)}"`.
   `<br>` starts a new line, and a vector shows as `X=... Y=... Z=...`.
2. Add a door that opens only within 200 units. Add `const CLOSE = 200.0`,
   `var near: bool = false` and `@right out open = near.Value`, and in the
   handler `near = Distance(here, there) < CLOSE`.
3. Measure along the floor only, so a player on a balcony above the chip
   reads as close as one standing beside it. Flatten both points first:
   `Distance(Vec(here.x, here.y, 0.0), Vec(there.x, there.y, 0.0))`.

## Make

Make a "warmer, colder" sign for a hidden prize. The chip is the prize. Each
sample, compare the distance with the one from the sample before (keep it in
a `var`) and show `Warmer` when the player got closer, `Colder` when they
moved away, and `Found it!` within 50 units.

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip readout -> decal Text
```

## Good to Know

- Distances are in the game's units (Unreal units). A 1x1 brick is 10 units
  wide, so 500 units is 50 studs.
- A player's location is a single point on their body, so standing right on
  the chip still reads a small distance.
- `visitor` is whoever walked in last. With two players in the zone, the
  light follows the newer one. Reading every player in the zone takes a loop:
  [Looping Over a List](../loops/looping-over-a-list.md).
- `Normalize` of a zero-length vector has no direction to keep. Check the
  distance before you use a direction between two points that may meet.

See [Vector](../../builtins.md#vector-pure) in the reference.

Next: [Dot and Cross](dot-and-cross.md).
