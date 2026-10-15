# Color-Cycling Light

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Lists](../values/lists.md)

</div>
<!-- /learn-links -->

A light that steps through a list of colors, one each second, for a party
floor, a shop sign or a stage. A switch turns it on and off.

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

- a **Switch**
- a brick with a **Point Light**

```wiring
switch Enabled -> chip running
chip lamp -> light Color
chip lit -> light Enabled
```

## Code

```wirescript name=bp4-color-cycling-light
const STEP_SECONDS = 1.0 // how long each color stays on
const COLORS = [
  Color(1.0, 0.0, 0.0),
  Color(1.0, 0.5, 0.0),
  Color(1.0, 1.0, 0.0),
  Color(0.0, 1.0, 0.0),
  Color(0.0, 0.3, 1.0),
  Color(0.6, 0.0, 1.0),
]

@left in running: bool
var shown: color = Color(1.0, 0.0, 0.0)
@right out lamp = shown.Value
@right out lit = running

on Clock(interval = STEP_SECONDS, enabled = running) {
  shown = COLORS[Cycle(COLORS.length())]
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the **Switch** and the
   **Point Light**.
2. Open the Point Light's settings and turn off **Use Brick Color**. While
   it is on, the light takes the brick's color and ignores the wire.
3. Wire the Switch's **Enabled** to `running`.
4. Wire `lamp` to the Point Light's **Color**, and `lit` to its **Enabled**.

## Check it in game

- Flip the switch on: the light comes on and changes color once a second,
  in the list's order: red, orange, yellow, green, blue, purple.
- After purple it goes back to red and carries on.
- Flip the switch off: the light goes out. Flip it on again: it carries on
  from the color it stopped at.

## Change it

- Change the speed: `const STEP_SECONDS = 0.25` for a disco.
- Use your own colors: add, remove or reorder the `Color(red, green, blue)`
  lines. Each channel runs from 0.0 to 1.0. The cycle follows the length of
  the list, so nothing else needs to change.
- Fade from one color to the next instead of jumping:
  `@right out lamp: color = Tween(shown.Value, 0.5)`. A **Tween** moves
  toward its target over 0.5 seconds ([Game Enums](../enums/game-enums.md)).
- Keep the light on with the switch off, frozen on one color: wire the
  Point Light's **Enabled** from a Switch of its own instead of `lit`.

## How it works

The Clock runs its handler every `STEP_SECONDS` while `running` is on
([Reacting to the Game](../reacting-to-the-game.md)). `Cycle(n)` is the
game's **Counter** gate: each pulse moves it on by one, from 0 up to `n - 1`
and then back to 0. Its number picks a color from `COLORS`, a `const`
array: a fixed list that never changes while the game runs, read by index
like any array ([Lists](../values/lists.md)). The handler
stores the color in `shown`, which drives the light's **Color**
([Vectors and Colors](../values/vectors-and-colors.md)).
