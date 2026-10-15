# Dot and Cross

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Distance and Direction](distance-and-direction.md)

**Next steps:** [Angles](angles.md)

</div>
<!-- /learn-links -->

Two directions can point the same way, at right angles, or opposite ways.
`Dot` turns that into one number, and `Cross` tells left from right. This
lesson builds a light that switches on when a player looks at it.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [What a dot product measures](#what-a-dot-product-measures)
- [Which side: Cross](#which-side-cross)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip lit -> light Enabled
```

## Code

```wirescript name=sp2-look
const WITHIN = 0.985

@left in room: zone
@left in occupied: bool

var visitor: character
var seen: bool = false
@right out lit = seen.Value

on ZoneEntered(zone = room) -> (character) {
  visitor = character
}

on Clock(interval = 0.1, enabled = occupied) {
  let target = GetOwnTransform().Location
  let aim = visitor.GetAim()
  let toTarget = Normalize(target - aim.Origin)
  seen = Dot(aim.Direction, toTarget) > WITHIN
}
```

`visitor.GetAim()` gives two vectors: `Origin`, the point the player looks
from, and `Direction`, the way they look, with length 1. `toTarget` is the
direction from the player's eyes to the chip, also length 1, from the last
lesson's `Normalize`.

`Dot(a, b)` compares the two directions. It is 1 when they point the same
way, 0 at right angles and -1 when opposite. Above 0.985 the player is looking
within about 10 degrees of the chip.

The player looks straight at the chip, then turns to face away. What does the
dot product read in each case? Does stepping back change it?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**,
   put the chip inside it, and place a **Point Light** next to the chip.
2. Wire the Attached Zone's **Zone Ref** to `room` and its **Occupied** to
   `occupied`. Wire `lit` to the Point Light's **Enabled**.
3. Walk into the zone and look around. The light comes on while you look at
   the chip and goes off when you look away. Distance does not matter: from
   across the zone, looking at the chip still lights it.

## Under the Hood

Open the chip. The Clock's chain runs **Get Own Location and Rotation**, a
**Get Variable** for `visitor`, and one **Get Aim** gate, whose **Origin** and
**Direction** outputs both come from that one gate. Then a **Subtract** makes
the arrow to the chip, **Normalize** shortens it to length 1, a **Dot
Product** compares it with the aim, and a **Greater** checks the threshold
before the **Set Variable**.

## What a dot product measures

`Dot(a, b)` multiplies the vectors part by part and adds the results:
`a.x * b.x + a.y * b.y + a.z * b.z`. For two length-1 directions, that sum
is the cosine of the angle between them:

| Angle between | Dot product |
|---------------|-------------|
| 0 degrees     | 1           |
| 10 degrees    | 0.985       |
| 30 degrees    | 0.866       |
| 60 degrees    | 0.5         |
| 90 degrees    | 0           |
| 180 degrees   | -1          |

A threshold picks a cone around the target: `> 0.866` is anywhere within 30
degrees, `> 0` is anywhere in front, `< 0` is behind.

The cosine only holds when both vectors have length 1. Longer vectors scale
the answer: `Dot(Vec(3.0, 0.0, 0.0), Vec(2.0, 0.0, 0.0))` is 6, not 1. That is
why `toTarget` is normalized.

## Which side: Cross

`Cross(a, b)` gives a third vector at right angles to both. Cross the aim with
the direction to the target and its `z` part says which way to turn:

```wirescript name=sp2-side
@left in looking: vector
@left in toTarget: vector
let side = Cross(looking, toTarget).z
@right out hint = if side > 0.0 then "turn right" else "turn left"
```

`if ... then ... else` picks one of two values. Looking along x, a target
along y gives `Cross(Vec(1.0, 0.0, 0.0), Vec(0.0, 1.0, 0.0))`, which is
`Vec(0.0, 0.0, 1.0)`: `z` is positive. Swap the target to the other side,
`Vec(0.0, -1.0, 0.0)`, and `z` is -1. In the game's axes, z points up and y
lies to the right of x, so a positive `z` means the target is on the right.

## Modify

1. Widen the cone to 30 degrees: `const WITHIN = 0.866`. Then try `0.0`: the
   light is on whenever the chip is anywhere in front of the player.
2. Watch the number. Add `var shown: string = ""` and
   `@right out readout = shown.Value`, and in the handler
   `shown = "${Dot(aim.Direction, toTarget)}"`. Wire `readout` to a **Text
   Decal**'s **Text** and turn on the spot: it runs from 1 down to -1 and back.
3. Give the player a hint. Compute `let side = Cross(aim.Direction,
   toTarget).z` in the handler, and set the sign to `"on it"` when `seen`
   would be true, else `"turn right"` or `"turn left"` from `side`.

## Make

Make a "don't look" light: it glows while the visitor's back is turned to the
chip and switches off the moment they turn to look. Add a sign that counts how
many times they looked.

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip lit -> light Enabled
chip sign -> decal Text
```

## Good to Know

- `a * b` on two vectors multiplies part by part and gives a vector. It is not
  the dot product. Use `Dot(a, b)` or `a.Dot(b)` for that.
- The dot product ignores distance only because both directions are
  normalized. Compare positions with `Distance` and directions with `Dot`.
- `Cross(a, b)` and `Cross(b, a)` point opposite ways. Keep the aim first and
  the target second, or left and right swap.
- Looking straight up or down, the cross product's `z` is close to 0, and the
  left-right hint flickers. Which way to turn is a question about the floor
  plan, and the next lesson measures it as an angle.

See [Vector](../../builtins.md#vector-pure) and
[GetAim](../../builtins.md#getaim) in the reference.

Next: [Angles](angles.md).
