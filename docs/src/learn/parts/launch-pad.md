# Launch Pad

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Vectors and Colors](../values/vectors-and-colors.md)

</div>
<!-- /learn-links -->

Run onto the pad and it throws you into the air, and forward in the
direction you are looking.

<!-- toc -->
## Contents

- [You need](#you-need)
- [Code](#code)
- [Wire it](#wire-it)
- [Check it in game](#check-it-in-game)
- [Change it](#change-it)
- [How it works](#how-it-works)
<!-- /toc -->

## You need

- a brick with an **Attached Zone** just above the pad

```wiring
zone "Zone Ref" -> chip pad
```

## Code

```wirescript name=pp6-launch
// Upward push.
const UP = 1500.0
// Push in the direction the player looks.
const AHEAD = 1000.0

@left in pad: zone

on ZoneEntered(zone = pad) -> (character) {
  let look = character.GetAim().Direction
  let flat = Vec(look.x, look.y, 0.0)
  character.AddVelocity(linear = flat * AHEAD + Vec(0.0, 0.0, UP))
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Attached Zone's **Zone Ref** to `pad`.

## Check it in game

- Walk onto the pad looking straight ahead: you fly up and forward.
- Walk on looking to one side: you fly toward where you look, not where you
  walked.
- Walk on looking straight up or down: you go almost straight up.
- Stand still on the pad: nothing more happens until you step off and back
  on.

## Change it

- Change `UP` and `AHEAD` for a bigger or smaller jump. Start small: the
  numbers are speeds, and a large one sends a player very far.
- Always launch the same way, whatever the player looks at: replace the
  whole push with a fixed vector, such as `Vec(800.0, 0.0, 1500.0)`.
- Turn the pad off with a switch: add `@left in armed: bool`, wired from a
  **Switch**'s **Enabled**, and wrap the push in `if armed { ... }`.

## How it works

The zone event gives the player who stepped on
([Reacting to the Game](../reacting-to-the-game.md)). `GetAim()` reads where
they look as a direction vector. Its height is set to 0, so the forward
push stays level. It is weaker the more steeply the player looks up or
down. The upward push is added on top
([Vectors and Colors](../values/vectors-and-colors.md)). `* AHEAD` scales all
three parts of the vector at once. `AddVelocity` adds to the player's speed
rather than replacing it, so a running start carries into the jump.
