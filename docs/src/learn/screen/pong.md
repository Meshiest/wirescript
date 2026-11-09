# Pong

<!-- learn-links -->
<div class="learn-links">

**Before this:** [A Game on a Screen](a-game-on-a-screen.md), [Ref Parameters](../references/ref-parameters.md)

</div>
<!-- /learn-links -->

Two seats, one screen, one ball. This lesson builds the classic two-player
paddle game: each player steers a paddle up and down, the ball bounces off
the walls and the paddles, and a ball that gets past a paddle scores for the
other side.

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

```wirescript name=sc6-pong
@display(width = 24, height = 13, size = 0.4)
var screen

@seat(label = "Left paddle")
in left

@seat(label = "Right paddle")
in right

const W = 24
const H = 13
const TOP = 1
const PADDLE = 3
const FIRST_TO = 7
var leftTop: int = 6
var rightTop: int = 6
var ballX: int = 12
var ballY: int = 7
var dx: int = 1
var dy: int = 1
var leftPoints: int = 0
var rightPoints: int = 0
var playing: bool = false
var steps: int = 0

let leftKeys = left.character.InputReader()
let rightKeys = right.character.InputReader()

mod paint(x: int, top: int, c: color) {
  for i in 0..PADDLE {
    screen[(top + i) * W + x] = c
  }
}

mod steer(top: *int, x: int, push: float, c: color) {
  var next: int = top
  if push > 0.5 {
    next = max(top - 1, TOP)
  } else if push < -0.5 {
    next = min(top + 1, H - PADDLE)
  }
  paint(x, top, Color(0.0, 0.0, 0.0, 0.0))
  top = next
  paint(x, top, c)
}

mod chase(top: int) -> float {
  if steps % 2 == 1 {
    return 0.0
  }
  if ballY < top + 1 {
    return 1.0
  }
  if ballY > top + 1 {
    return -1.0
  }
  return 0.0
}

mod serve(toward: int) {
  ballX = W / 2
  ballY = (TOP + H) / 2
  dx = toward
}

mod step_ball() {
  screen[ballY * W + ballX] = Color(0.0, 0.0, 0.0, 0.0)
  if ballY + dy < TOP || ballY + dy >= H {
    dy = -dy
  }
  let nx = ballX + dx
  let ny = ballY + dy
  if nx == 0 {
    if ny >= leftTop && ny < leftTop + PADDLE {
      dx = 1
    } else {
      rightPoints += 1
      screen[W - rightPoints] = Color(1.0, 0.4, 0.2)
      serve(1)
      return
    }
  } else if nx == W - 1 {
    if ny >= rightTop && ny < rightTop + PADDLE {
      dx = -1
    } else {
      leftPoints += 1
      screen[leftPoints - 1] = Color(0.2, 0.6, 1.0)
      serve(-1)
      return
    }
  }
  ballX += dx
  ballY += dy
  screen[ballY * W + ballX] = Color(1.0, 1.0, 1.0)
}

on left.occupied || right.occupied {
  screen.fill(Color(0.0, 0.0, 0.0, 0.0))
  leftPoints = 0
  rightPoints = 0
  serve(1)
  playing = true
}

on !left.occupied && !right.occupied {
  playing = false
}

on Clock(interval = 0.1, enabled = playing.Value) {
  steps += 1
  if left.occupied {
    steer(leftTop, 0, leftKeys.Forward, Color(0.2, 0.6, 1.0))
  } else {
    steer(leftTop, 0, chase(leftTop), Color(0.2, 0.6, 1.0))
  }
  if right.occupied {
    steer(rightTop, W - 1, rightKeys.Forward, Color(1.0, 0.4, 0.2))
  } else {
    steer(rightTop, W - 1, chase(rightTop), Color(1.0, 0.4, 0.2))
  }
  step_ball()
  if leftPoints == FIRST_TO || rightPoints == FIRST_TO {
    playing = false
  }
}

@test mod a_paddle_sends_the_ball_back() {
  leftTop = 5
  ballX = 1
  ballY = 5
  dx = -1
  dy = 1
  step_ball()
  testing.assertEq(dx, 1)
  testing.assertEq(ballX, 2)
  testing.assertEq(rightPoints, 0)
}

@test mod a_miss_scores_for_the_other_side() {
  leftTop = TOP
  ballX = 1
  ballY = 5
  dx = -1
  dy = 1
  step_ball()
  testing.assertEq(rightPoints, 1)
  testing.assert(screen[W - 1].r > 0.9, "the right end of the score row is lit")
  testing.assertEq(ballX, W / 2)
}

@test mod a_paddle_stops_below_the_score_row() {
  leftTop = TOP
  steer(leftTop, 0, 1.0, Color(0.2, 0.6, 1.0))
  testing.assertEq(leftTop, TOP)
  steer(leftTop, 0, -1.0, Color(0.2, 0.6, 1.0))
  testing.assertEq(leftTop, TOP + 1)
  testing.assert(screen[TOP * W].a < 0.5, "the cell the paddle left is cleared")
  testing.assert(screen[(TOP + 3) * W].b > 0.9, "the paddle is drawn one lower")
}

@test mod an_empty_seat_chases_the_ball() {
  ballY = 9
  steps = 0
  testing.assert(chase(4) < -0.5, "it moves down toward the ball")
  steps = 1
  let rest = chase(4)
  testing.assert(rest > -0.5 && rest < 0.5, "it rests every other step")
}
```

Both seats sit in one row in front of the chip, the left paddle's seat on
the left as you face the screen. `leftKeys` and `rightKeys` read each
sitter's keys, as in [Seat Controls](../input/seat-controls.md).

The top row of the screen is the score row; the court is the rows below it,
from `TOP` down.

The game runs on a **Clock**: every tenth of a second it moves both paddles
and then the ball. A **Clock** suits a game loop because the ball has to
move whether or not anyone presses a key. `enabled = playing.Value` wires the
Clock's **Enabled** to the variable's live value, so the game stops the
moment `playing` turns false.

- `steer` takes the paddle's position as a `*int`, so one mod moves either
  paddle (see [Ref Parameters](../references/ref-parameters.md)). Holding W
  pushes `Forward` to 1 and moves the paddle up a row; S moves it down. `max`
  and `min` keep it on the court.
- A paddle whose seat is empty plays itself: `chase` pushes it toward the
  ball's row, but only on every other step, so a player can get the ball
  past it. One person can play alone, and a friend who sits down takes over
  the other paddle.
- `step_ball` erases the ball, bounces it off the court's top and bottom, and
  looks one step ahead. A ball about to reach column 0 either meets the left
  paddle, which turns it round, or gets past and scores for the right side.
  Column `W - 1` works the same way for the right paddle.
- A point lights one pixel of the score row: the left side's from the left
  end, the right side's from the right end. `serve` puts the ball back in the
  middle, heading toward the side that just scored.
- The game starts when someone sits down and stops when both seats are empty
  or a side reaches 7 points.

The ball is at column 2, row 4, moving left and down, and the left paddle
covers rows 1 to 3. Where is the ball after two steps, and which side has
scored?

## Run

1. Compile the file and paste it in-game. The screen stands behind the
   chip; the two seats face it.
2. Sit in the left seat. The screen clears, both paddles appear, and the
   ball starts toward the right paddle, which moves by itself.
3. Hold W and S to steer. Get the ball past the right paddle and your end of
   the score row lights one more pixel.
4. Have a friend sit in the right seat. From then on their keys steer it.

**Play** in the playground or in VS Code runs the same game without
Brickadia: click a seat's button to sit in it.

## Under the Hood

A **Clock** gate fires the handler on every pulse while its **Enabled** input
is on. Its **Enabled** is wired straight from the `playing` variable's
**Value** output, so the Clock turns off as soon as `playing` turns false.

`if left.occupied` reads the seat's **Is Occupied** output on every step, so
a paddle switches between its player and `chase` the step after someone
sits or stands.

`paint` holds a `for` loop, and a mod's body is copied into every place it
is called. Each `steer` call paints twice, once to erase and once to draw,
so the handler holds eight small loops: two for each of its four `steer`
calls.

The screen draws the same way as in
[A Game on a Screen](a-game-on-a-screen.md): a microchip behind the panel
turns the pixels into one line of text per row, a tick after they change.

## Modify

1. Make the paddles longer: set `PADDLE` to 4. Which lines change how the
   game plays, and which just follow along?
2. Make the computer harder to beat: let `chase` move on two steps out of
   three instead of one out of two.
3. Let the ball leave a paddle at an angle: when it hits the top cell of a
   paddle set `dy = -1`, and when it hits the bottom cell set `dy = 1`.

## Make

Make a brick-breaking game for one seat. The paddle runs along the bottom
row, steered with A and D (`Right` instead of `Forward`), and a row of
bricks fills the top of the screen. The ball bounces off the paddle and the
walls, and a brick it hits disappears and turns the ball around. Keep the
bricks in a `bool[]` filled with `push` when the game starts.

## Good to Know

- `on left.occupied || right.occupied` fires when the condition turns true.
  After a game ends, everyone stands up and someone sits down again to start
  the next.
- A bare `playing` in the Clock's `enabled` would read the variable once,
  when it was last set; `playing.Value` follows it. Reach for `.Value`
  whenever a variable feeds a gate outside a handler.
- A pixel written with alpha 0 shows the panel behind it, which is how both
  the ball and the paddles erase themselves.
