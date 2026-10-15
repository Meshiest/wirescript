# Easing

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Blending Values](blending-values.md), [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Over Time](over-time.md)

</div>
<!-- /learn-links -->

A blend moves at one steady speed. Real things rarely do: a door picks up
speed and slows down again before it stops, and a ball lands and bounces. An
**easing curve** bends the progress of a blend so its speed changes on the
way. This lesson plays three curves side by side.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Directions](#directions)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip go
chip linear -> light1 Brightness
chip quad -> light2 Brightness
chip bounce -> light3 Brightness
switch1 Enabled -> light1 Enabled
switch2 Enabled -> light2 Enabled
switch3 Enabled -> light3 Enabled
```

## Code

```wirescript name=mo2-curves
const FRAMES = 30 // steps in one run: 30 steps of 0.05 seconds is 1.5 seconds

@left in go: exec
var frame: int = FRAMES
let t = frame.Value / (FRAMES as float)
@right out linear = Easing(0.0, 4.0, t)
@right out quad = Easing(0.0, 4.0, t, function = Quad, direction = In)
@right out bounce = Easing(0.0, 4.0, t, function = Bounce, direction = Out)

on go {
  frame = 0
}

on Clock(interval = 0.05, enabled = frame.Value < FRAMES) {
  frame += 1
}
```

`t` is the **progress**: 0.0 at the start of the run and 1.0 at the end. The
chip drives it itself. A press sets `frame` back to 0, and the Clock counts
it up by one every 0.05 seconds until it reaches 30. `t` divides `frame` by
`FRAMES as float`: `as float` turns the whole number 30 into 30.0, so the
division keeps its fraction.

`Easing(a, b, t)` is a blend from `a` to `b` like `Blend`, but first it bends
`t` along a curve. `function` picks the curve and `direction` picks which
end it shapes:

- `Linear` does not bend: the same as `lerp`. It is the default.
- `Quad` with `In` squares the progress, so it starts slowly and speeds up.
- `Bounce` with `Out` rushes to the end and bounces there, like a dropped
  ball.

Halfway through a run, `t` is 0.5. How bright is each light?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and three
   **Point Light**s in a row, each with its **Enabled** on a **Switch** that
   is flipped on.
2. Wire the Button's **Held** to `go`, and `linear`, `quad` and `bounce` to
   the three lights' **Brightness**.
3. At load all three are at full: `frame` starts at 30, the end of a run.
4. Press the button. All three go dark and climb back over a second and a
   half. The first climbs steadily. The second hardly moves at first and
   catches up at the end. The third shoots up, then dips and rises a few
   times before it settles.

Halfway, the first light is at 2.0. The second is at 1.0: a quarter of the
way, since 0.5 squared is 0.25. The third is at about 3.06: Bounce Out is
already past three quarters of the way.

## Under the Hood

Open the chip. The Clock's **Enabled** is wired from a **Less** gate
comparing `frame` with 30, so it stops by itself at the end of a run. A
**Divide** turns `frame` into `t`, and `t` feeds three **Easing** gates.

Each Easing gate has its curve in its settings: **Function** and
**Direction**. They are the game enums `EasingFunction` and
`EasingDirection` ([Game Enums](../enums/game-enums.md) shows the full lists
and the numbers the game saves). Like `clampAlpha` on a Blend, they are
fixed when the chip is built, so they take only a constant. A variable here
is the `WS028` error from [Blending Values](blending-values.md), and so is a
name that is not on the list:

```wirescript name=mo2-unknown expect=WS028
@left in t: float
@right out level = Easing(0.0, 4.0, t, function = Quad, direction = Both)
```

```text
Error: [WS028] unknown enum member 'Both' for EBREasingDirection; expected one of: In, Out, InOut
```

## Directions

`In` puts the curve's shape at the start, `Out` at the end, and `InOut` at
both ends, meeting in the middle. The table gives the eased values of a few
curves at three points of progress, for a blend from 0 to 1.

| Curve | t = 0.25 | t = 0.5 | t = 0.75 |
|---|---|---|---|
| `Linear` | 0.25 | 0.5 | 0.75 |
| `Quad`, `In` | 0.0625 | 0.25 | 0.5625 |
| `Quad`, `Out` | 0.4375 | 0.75 | 0.9375 |
| `Quad`, `InOut` | 0.125 | 0.5 | 0.875 |
| `Bounce`, `Out` | about 0.47 | about 0.77 | about 0.97 |

`Quad` `Out` is `Quad` `In` turned around: the slow part moves to the end.
Its value at 0.25 is 1 minus `In`'s value at 0.75. `InOut` is slow at both
ends and quick in the middle.

`Sine`, `Cubic`, `Quart`, `Quint`, `Expo` and `Circ` are the same idea,
from gentle to sharp. `Back` and `Elastic` go past the end and come back, and
`Bounce` bounces.

## Modify

1. Change the second light to `direction = InOut`. It now starts slowly and
   ends slowly too.
2. Ease a color. `Easing` takes the same kinds of value as `Blend`. Add
   `@right out lamp = Easing(Color(0.0, 0.2, 1.0), Color(1.0, 0.1, 0.0), t, function = Elastic, direction = Out)`
   and wire it to a fourth light's **Color** (with **Use Brick Color** off).
   The color swings past red, then wobbles back.
3. Drive the progress from the time instead of a counter. `ServerUptime()`
   is the server's running time in seconds, and the time since a press is
   the uptime now minus the uptime at the press:

   ```wirescript name=mo2-uptime
   const SECONDS = 1.5 // how long one run takes

   @left in go: exec
   var started: float = -10.0
   let t = clamp((ServerUptime() - started.Value) / SECONDS, 0.0, 1.0)
   @right out level = Easing(0.0, 4.0, t, function = Quad, direction = Out)

   on go {
     started = ServerUptime()
   }
   ```

   No Clock: the subtraction follows the time on its own. `clamp` holds `t`
   at 1.0 once the run is over.

## Make

Make a light that breathes. It rises from dark to bright over a second, falls
back over a second, and repeats forever. Count `frame` from 0 to 39 on a
0.05-second Clock and wrap it back to 0, turn the first half of the count
into a rising `t` and the second half into a falling one, and ease both with
`Sine` and `InOut`.

```wiring
chip glow -> light Brightness
switch Enabled -> light Enabled
```

## Good to Know

- The progress must be a `float` from 0.0 to 1.0. `frame.Value / FRAMES`
  divides two `int`s and is 0 until the very end, and the compiler accepts
  it without a warning. Turn one side into a `float` first, as
  `FRAMES as float` does in the chip.
- Keep the progress between 0 and 1 with `clamp`. A curve is only shaped for
  that range.
- `ServerUptime()` starts again from 0 when the server restarts, but a
  stored `started` survives. After a restart the stored time can be ahead of
  the uptime, and `t` stays at 0 until the uptime catches up or someone
  presses again. Use it for short runs, not for stored deadlines.
- `GetUnixTime()` counts whole seconds, too coarse to animate with.
- A Clock-driven run moves in steps: twenty a second here. A shorter
  interval is smoother. The next lesson's `Tween` moves every tick by itself.
- `Remap` has the same curves. Set them with the gate's own setting names:
  `Remap(position, 0.0, 100.0, 0.0, 4.0, Function = Quad, Direction = Out)`.

See [Misc](../../builtins.md#misc-pure--exec) and
[Gate config properties](../../builtins.md#gate-config-properties) in the
reference.

Next: [Over Time](over-time.md).
