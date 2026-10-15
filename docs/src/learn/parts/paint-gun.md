# Paint Gun

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remote Control](../input/remote-control.md), [Forking and Merging](../events/forking-and-merging.md), [Game Enums](../enums/game-enums.md), [Weighted Loot](../random/weighted-loot.md)

</div>
<!-- /learn-links -->

A paint studio. Flip the switch to pick up the brush, look at a brick and
click: the brick turns the current paint color. A button steps through a
palette, and a light shows the color you will paint with.

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

- a **Switch**, to pick up the brush
- a **Button**, to pick the next color
- a **Point Light**, as the color swatch

```wiring
switch Character -> chip painter
button Held -> chip next
chip swatch -> light Color
```

## Code

```wirescript name=pp8-paint-gun
// How far the painter can reach.
const REACH = 1000.0
// The colors the button steps through.
const PALETTE = [
  Color(0.9, 0.1, 0.1),
  Color(0.1, 0.6, 0.1),
  Color(0.1, 0.3, 0.9),
  Color(1.0, 0.85, 0.1),
  Color(1.0, 1.0, 1.0),
]

@left in painter: character
@left in next: exec

var pick: int = 0
var paint: color = Color(0.9, 0.1, 0.1)
@right out swatch = paint.Value

let keys = InputReader(painter)

on next {
  pick = (pick + 1) % PALETTE.length()
  paint = PALETTE[pick]
}

on keys.PressedLeftMouse {
  let aim = painter.GetAim()
  let r = Sweep(aim.Origin, aim.Direction, REACH, ignore = painter, detectBricks = true)
  on r.Hit {
    r.HitBrick.SetBrickProperty(BrickProperty.Color, paint)
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Switch's **Character** to `painter`.
3. Wire the Button's **Held** to `next`.
4. Wire `swatch` to the Point Light's **Color**, and turn off the light's
   **Use Brick Color**, or it ignores the wired color.

## Check it in game

- Before any press: the swatch light is red.
- Flip the switch, look at a brick you built and click: the brick turns
  red. Empty your hands first, so the click does nothing else.
- Hold the mouse button and sweep your view across other bricks: only the
  first brick changes. Let go and click again for the next one.
- Press the button: the swatch turns green, and the next brick you paint is
  green. Five presses bring it back to red.
- With a second player: they flip the switch, and now their clicks paint
  and yours do nothing.

## Change it

- Add colors to `PALETTE`, or change them. The first one should match the
  starting `paint`.
- Shorten `REACH` so the painter must walk up to what they paint.
- Paint only while the switch is on: add `@left in active: bool`, wire the
  Switch's **Enabled** to it, and write `on keys.PressedLeftMouse && active`.
- Let several players paint at once, each with their own brush: give each
  painter a chip of their own, as in
  [One Chip per Player](../prefabs/one-chip-per-player.md).

## How it works

The Switch's **Character** holds the last player who flipped it, and
`InputReader` turns that player's keys into live wires
([Remote Control](../input/remote-control.md)). `on keys.PressedLeftMouse`
runs once per click, with no timer. `GetAim()` gives where the painter looks
from and the direction, and `Sweep` casts a ray along it. `on r.Hit` runs
only when the ray met a brick
([Forking and Merging](../events/forking-and-merging.md)).
`SetBrickProperty(BrickProperty.Color, ...)` recolors that brick
([Game Enums](../enums/game-enums.md)). The palette is a `const` array, read
in a handler ([Weighted Loot](../random/weighted-loot.md)).
