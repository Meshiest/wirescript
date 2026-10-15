# Combination Lock

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Mods](../abstraction/mods.md), [Lists](../values/lists.md)

</div>
<!-- /learn-links -->

A door that opens when four buttons are pressed in the right order. A wrong
press starts the code over, and any press while the door is open locks it
again.

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

- four **Button**s
- a door: a few bricks, one with a **Brick Property Changer**
- optional: a brick with a **Point Light**, to show when the lock is open

```wiring
button1 Held -> chip b1
button2 Held -> chip b2
button3 Held -> chip b3
button4 Held -> chip b4
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
chip unlocked -> light Enabled
```

## Code

```wirescript name=bp2-combination-lock
const CODE = [3, 1, 4, 2] // the buttons to press, in order

@left in b1: exec
@left in b2: exec
@left in b3: exec
@left in b4: exec
var step: int = 0
@right out unlocked = step.Value == CODE.length()
@right out solid = step.Value != CODE.length()

mod press(key: int) {
  if step == CODE.length() {
    step = 0
  } else if CODE[step] == key {
    step += 1
  } else {
    step = 0
  }
}

on b1 { press(1) }
on b2 { press(2) }
on b3 { press(3) }
on b4 { press(4) }
```

## Wire it

1. Compile the chip and paste it in-game. Build the door and place the four
   **Button**s in a row beside it.
2. Wire the Buttons' **Held** outputs to `b1`, `b2`, `b3` and `b4`, left to
   right.
3. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility**. One output can feed both.
4. Optional: wire `unlocked` to the Point Light's **Enabled**, and give the
   light a green color.

## Check it in game

- Press buttons 3, 1, 4, 2: the door vanishes after the last press, and the
  light comes on.
- With the door open, press any button: the door is back and the light is
  off.
- Press 3, 1, 2: nothing opens. Then press 3, 1, 4, 2 again: the door opens,
  because the wrong press started the code over.
- Press 3, 3, 1, 4, 2: the door stays shut. The second 3 was wrong, so the
  code started over, and that 3 did not count as its first press.

## Change it

- Set your own code: `const CODE = [2, 2, 1, 4, 3]`. The code can be any
  length, and a button can appear more than once.
- Use more buttons: add `@left in b5: exec` and `on b5 { press(5) }`.
- Lock the door by itself. Add a handler that runs when the lock opens:

  ```wirescript nocompile
  on step.Value == CODE.length() {
    await Sleep(_, delay = 5.0)
    step = 0
  }
  ```

  Five seconds after the door opens, it locks again.

## How it works

`step` counts how many presses of the code have been right so far
([Remembering Values](../state.md)). Each button's handler calls the mod `press`
([Mods](../abstraction/mods.md)) with that button's number. A mod can read
and write the chip's own variables, as `press` does with `step`. `CODE` is a
`const` array: a fixed list that never changes while the game runs, read by
index like any array ([Lists](../values/lists.md)). If the
next number in the code matches, `step` goes up. If not, it goes back to 0.
When `step` reaches the length of the code, the lock is open: `solid` turns
off and `unlocked` turns on. The mod is copied into each of the four
handlers, so the chip holds four copies of its gates.
