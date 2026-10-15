# Countdown Sign

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Exec Pins](../events/exec-pins.md)

</div>
<!-- /learn-links -->

Press a button and a sign counts down the seconds. At zero the chip sends a
pulse: open a door, ring a sound, start a round.

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

- a **Button**
- a brick with a **Text Decal**
- something to happen at zero, such as a **One Shot Audio** brick
- optional: a brick with a **Point Light**, lit while the count runs

```wiring
button Held -> chip start
chip text -> decal Text
chip done -> speaker Play
chip counting -> light Enabled
```

## Code

```wirescript name=bp3-countdown-sign
const SECONDS = 10 // where the count starts

@left in start: exec
@right out done: exec
var remaining: int = SECONDS
var running: bool = false
@right out text = "${remaining.Value}"
@right out counting = running.Value

on start {
  if !running {
    remaining = SECONDS
    running = true
  }
}

on Clock(interval = 1.0, enabled = running) {
  remaining -= 1
  if remaining == 0 {
    running = false
    emit done
  }
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the **Button**, the Text
   Decal brick and the One Shot Audio brick. Open the One Shot Audio's
   settings and pick a sound.
2. Wire the Button's **Held** to `start`.
3. Wire `text` to the Text Decal's **Text**.
4. Wire `done` to the One Shot Audio's **Play**.
5. Optional: wire `counting` to the Point Light's **Enabled**.

## Check it in game

- Before any press, the sign shows `10`.
- Press the button: the sign counts down by one each second, and the light
  is on.
- At `0` the sound plays once and the light goes off.
- Press the button during a count: nothing changes, and the count carries on.
- Press it after the count has ended: the sign starts again from `10`.

## Change it

- Count from another number: `const SECONDS = 30`.
- Show minutes and seconds: `"${remaining.Value / 60}:${remaining.Value % 60}"`.
  `/` on two whole numbers drops the remainder, and `%` gives the remainder.
  65 seconds shows as `1:5`. The `pad2` helper in
  [Number Options](../templates/number-options.md) makes it `1:05`.
- Let a press restart the count at any time: remove the `if !running`
  check, keeping the two lines inside it.
- Add a stop button: `@left in stop: exec` and
  `on stop { running = false }`. The sign keeps the number it stopped at.

## How it works

`running` turns the Clock on and off: while it is on, the Clock runs its
handler once a second ([Reacting to the Game](../reacting-to-the-game.md)),
and each run takes one off `remaining`. At zero the handler turns `running`
off, which stops the Clock, and fires the `done` pin
([Exec Pins](../events/exec-pins.md)). The `if !running` check in the start
handler ignores presses while a count is going. `text` turns the number
into text for the sign, and follows `remaining` all the time.
