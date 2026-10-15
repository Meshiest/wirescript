# Rotations

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Angles](angles.md), [Looping Over a List](../loops/looping-over-a-list.md)

</div>
<!-- /learn-links -->

The last lesson built a rotator from angles by hand. The game has gates that
work with whole rotations instead: point along a direction, turn a vector,
undo a turn. Most of them use a second rotation type, the `quat`. This lesson
builds a spotlight that tracks the nearest player in a zone.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Rotator or quat](#rotator-or-quat)
- [Turning vectors](#turning-vectors)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip aim -> light Rotation
switch Enabled -> light Enabled
```

## Code

```wirescript name=sp4-tracker
@left in room: zone
@left in occupied: bool

var players: character[]
var facing: rotator = Rotation(0.0, 0.0, 0.0)
@right out aim = facing.Value

on Clock(interval = 0.1, enabled = occupied) {
  players.fillFromZonePlayers(room)
  let here = GetOwnTransform().Location
  var nearest: float = -1.0
  var target: vector = here
  for p in players {
    let spot = p.GetLocation()
    let d = Distance(here, spot)
    if nearest < 0.0 || d < nearest {
      nearest = d
      target = spot
    }
  }
  if nearest >= 0.0 {
    facing = (target - here).ToRotation()
  }
}
```

Ten times a second while someone is in the zone,
`players.fillFromZonePlayers(room)` fills the list with everyone standing in
it. The loop keeps the closest one, comparing their distances. `nearest`
starts at -1, so if the list comes back empty it stays there and the light
keeps its last aim.

`(target - here).ToRotation()` is the whole of the last lesson's compass math
in one gate: it turns the arrow toward the player into a rotation that points
along it. The answer is a `quat`, and storing it in the `rotator` variable
converts it.

Two players stand in the zone, one 300 units away and one 400. Which one
does the light follow? What happens when the near one leaves?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   covering the area to watch. Place a **Spot Light** without turning its
   brick, and put the chip right next to it: the chip measures from its own
   position, and the beam starts at the light.
2. Wire the Attached Zone's **Zone Ref** to `room` and its **Occupied** to
   `occupied`. Wire `aim` to the Spot Light's **Rotation**, and a **Switch**
   to its **Enabled**.
3. Walk into the zone. The beam follows you. Bring a friend: it switches to
   whoever is closer, and stays where it last pointed when the zone empties.

## Under the Hood

Open the chip. The Clock's chain runs **Get Players In Zone**, **Get Own
Location and Rotation**, and two **Set Variable** gates for the loop's
starting values. The **For Each Array Entry** chip holds a **Get Location of
Entity**, a **Distance**, and the **Less**, **OR** and **Branch**
gates of the comparison. After the loop, a **Greater/Equal** and a
**Branch** check that someone was found, then a **Subtract**, one **Direction
to Quaternion**, and the **Set Variable** for `facing`. No gate sits between
the quaternion and the rotator variable: the wire carries either kind.

A `quat` holds no angles. Its parts are four numbers, `.x` `.y` `.z` `.w`,
that only mean something together, so asking it for a pitch fails:

```wirescript name=sp4-quat-pitch expect=WS010
@left in dir: vector
@right out tilt = dir.ToRotation().pitch
```

```text
Error: [WS010] no field `pitch` on quat
```

`dir.ToRotation().ToEuler().Pitch` converts first and reads the angle.

## Rotator or quat

Both types hold a rotation, and each converts to the other with no gate: pass
a rotator to a quat method, or store a quat in a rotator.

| Use a `rotator` for | Use a `quat` for |
|---------------------|------------------|
| angles you read or write as numbers | pointing along a direction: `dir.ToRotation()` |
| pins that take a rotation, such as a Spot Light's **Rotation** | turning a vector: `v.Rotate(q)` |
| `Rotation(pitch, yaw, roll)` and `.pitch` `.yaw` `.roll` | undoing a turn: `q.Invert()` |
|  | the turn between two directions: `a.RotationTo(b)` |

The quat gates work on rotators too, so `facing.Value.ToDirection()` is fine.

## Turning vectors

`v.Rotate(q)` turns the vector `v` by the rotation `q`. `q.ToDirection()` is
the way `q` points, length 1. `q.Invert()` is the turn that undoes `q`.

```wirescript name=sp4-local
@left in turret: rotator
@left in offset: vector
let local = offset.Rotate(turret.Invert())
@right out muzzle = Vec(100.0, 0.0, 0.0).Rotate(turret)
@right out inFront = local.x > 0.0
@right out onRight = local.y > 0.0
```

- `Vec(100.0, 0.0, 0.0)` is 100 units along x, the way a rotation of zero
  points. Turned by `turret`, it is 100 units along the turret's aim: the same
  as `turret.ToDirection() * 100.0`.
- `offset` is an arrow in world directions, such as from the turret to a
  player. Turning it by the inverse undoes the turret's turn and gives the
  arrow in the turret's own directions: `x` ahead, `y` to its right, `z` up.
  A positive `local.x` means the player is in front of the turret.

`a.RotationTo(b)` is the turn that carries direction `a` onto direction `b`,
so `a.Rotate(a.RotationTo(b))` points along `b`.
`q.ToAxisAngle().Angle` is how far a turn goes, in degrees, and
`a.AngleTo(b)` is the angle between two rotations, also in degrees.

## Modify

1. Watch the turn. Add `var shown: string = ""` and `@right out readout =
   shown.Value`. In the `if`, compute `let wanted = (target - here).ToRotation()`,
   set `shown = "Turned ${round(facing.AngleTo(wanted)) as int} degrees"`, and
   then `facing = wanted`. Walk sideways across the beam and the number grows.
2. Mount the light on a wall, so it should never aim behind itself. Read the
   chip's own rotation with `let own = GetOwnTransform()` (then `own.Location`
   and `own.Rotation`), and in the loop skip players behind the chip: compute
   `let local = (spot - here).Rotate(own.Rotation.Invert())` and add
   `local.x > 0.0 &&` to the `if`. Turn the chip and see which side it guards.
3. Show where the beam lands 300 units out: add
   `here + facing.Value.ToDirection() * 300.0` to the sign. Stand still and
   compare it with your own position.

## Make

Make a security camera. When the zone is empty, the Spot Light sweeps slowly
left and right, a few degrees of yaw per Clock tick. When a player comes
within 30 degrees of where it points (use `Dot` from
[Dot and Cross](dot-and-cross.md)), it locks on and follows them, and it goes
back to sweeping when they leave.

```wiring
zone "Zone Ref" -> chip room
zone Occupied -> chip occupied
chip aim -> light Rotation
switch Enabled -> light Enabled
```

## Good to Know

- The beam snaps to each new aim. For a light that swings smoothly, see
  [Blending Values](../motion/blending-values.md) and
  [Over Time](../motion/over-time.md): `Blend` and `Tween` take rotations
  too.
- `a * b` on two rotations combines the two turns into one. See
  [Arithmetic Operators](../../expressions.md#arithmetic-operators).
- `axis.RotationByAngle(angle)` is a turn by `angle` around `axis`, such as
  `Vec(0.0, 0.0, 1.0).RotationByAngle(90.0)` for a quarter turn about the up
  axis. The game's gate takes the angle in degrees.
- A player's location is a point on their body, so the beam aims at their
  middle, not their feet.
- `ToRotation` on a zero-length arrow has no direction to point along. If a
  player can stand exactly on the chip, check the distance before aiming.

See [Rotation / Quaternion](../../builtins.md#rotation--quaternion-pure) and
[Zone array fills](../../builtins.md#zone-array-fills-exec-array-methods) in the
reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
