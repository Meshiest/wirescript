# A Game on a Screen

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Seat Controls](../input/seat-controls.md), [Lists](../values/lists.md)

**Next steps:** [Pong](pong.md)

</div>
<!-- /learn-links -->

`@display` gives a program a screen of pixels and `@seat` gives it a seat, so
a whole game fits in one file: no hand-built screen, no wiring. This lesson
makes a dot you steer around a screen to catch pellets.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

## Code

```wirescript name=sc5-dot-chase
@display(width = 12, height = 8, size = 0.4)
var screen

@display(width = 12, height = 1, size = 0.4)
var points

@seat(label = "Play")
in player

const W = 12
const H = 8
var x: int = 0
var y: int = 0
var goal: int = 40
var score: int = 0

mod draw_goal() {
  screen[goal] = Color(1.0, 0.8, 0.1)
}

mod step(dx: int, dy: int) {
  screen[y * W + x] = Color(0.0, 0.0, 0.0, 0.0)
  x = (x + dx + W) % W
  y = (y + dy + H) % H
  let here = y * W + x
  if here == goal {
    score += 1
    points[(score - 1) % W] = Color(0.3, 1.0, 0.3)
    goal = (goal * 7 + 5) % (W * H)
    draw_goal()
  }
  screen[here] = Color(0.2, 0.6, 1.0)
}

on player.occupied {
  screen.fill(Color(0.0, 0.0, 0.0, 0.0))
  points.fill(Color(0.0, 0.0, 0.0, 0.0))
  x = 0
  y = 0
  score = 0
  draw_goal()
  step(0, 0)
}

let keys = player.character.InputReader()
on keys.Forward > 0.5 { step(0, -1) }
on keys.Forward < -0.5 { step(0, 1) }
on keys.Right > 0.5 { step(1, 0) }
on keys.Right < -0.5 { step(-1, 0) }

@test mod a_point_lights_the_score() {
  x = 3
  y = 3
  step(1, 0)
  testing.assertEq(score, 1)
  testing.assert(points[0].g > 0.9, "the first point is lit")
}

@test mod a_step_moves_the_dot() {
  step(1, 0)
  testing.assertEq(x, 1)
  testing.assert(screen[1].b > 0.9, "the dot is drawn at its new cell")
  testing.assert(screen[0].a < 0.5, "its old cell is cleared")
}

@test mod the_dot_wraps_at_the_edge() {
  step(-1, 0)
  testing.assertEq(x, W - 1)
  step(0, -1)
  testing.assertEq(y, H - 1)
}
```

`@display(width = 12, height = 8)` makes `screen` a list of 96 colors, one per
pixel, row by row: the pixel at column `x`, row `y` is `screen[y * W + x]`.
Every pixel starts transparent. Writing a color into the list draws it.

`@seat` makes `player` a seat with two fields: `occupied` is `true` while
someone sits in it, and `character` is who. The keys come from
`player.character.InputReader()`, as in [Seat Controls](../input/seat-controls.md).

Each key press runs `step`, which clears the dot's old pixel, moves it (the
`% W` and `% H` wrap it round the edges), and draws it again. Landing on the
yellow pellet scores a point, lights the next pixel of `points`, and moves the
pellet. `points` is a second display, one row high; displays stand side by
side in the order they are declared.

The `@test` mods check the moving and scoring without a seat: `just test-ws`
runs each once and fails on any `FAIL` line.

## Run

1. Compile the file and paste it in-game. The screen and the score strip
   stand behind the chip, facing the seat in front of it.
2. Sit in the seat. The screen clears, a blue dot appears in the top-left
   corner, and a yellow pellet in the middle.
3. Steer with W, A, S and D. Catch the pellet and the score strip beside
   the screen lights one more pixel.

In the playground or in VS Code, **Play** runs the same file without
opening Brickadia: click the seat to sit, then use the keys.

## Under the Hood

The program's own chip holds no drawing gates. Each row of the screen is a
**Text Decal** on its own strip of the panel, and a second microchip behind
the panel turns pixels into text: one `█` per pixel, a color tag where the
color changes, and a space for a transparent pixel. A change detector on
`screen` runs that chip whenever a pixel changes, so a write shows on the
next tick.

The seat is a real **Seat** brick. `occupied` and `character` are its
**Is Occupied** and **Sitting Character** outputs, wired into the chip.

## Modify

1. Give the screen a red border instead of the near-black one:
   `@color(Color(0.4, 0.0, 0.0))` above `var screen`.
2. Leave a trail: delete the line that clears the old pixel.
3. Add a second seat with `@top @seat in rival`, and a second dot it
   steers, in another color.

## Make

Make a drawing toy: the seat's keys move a cursor, and holding E paints the
pixel under it in the current color. Q cycles the color through a list of
four, set up with `push` in the `player.occupied` handler.

## Good to Know

- A screen draws one tick after a write, however many pixels changed.
- A pixel whose alpha is under one half shows the panel behind it.
- `size` is a pixel's edge in studs; `size = 0.4` makes this screen about
  five studs wide.
- `screen` is an ordinary `color[]`: `fill`, `swap` and `copyFrom` draw
  too, and a `fn` can take it as a `color[]` parameter.
