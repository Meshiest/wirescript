# Toggle Light

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

</div>
<!-- /learn-links -->

One button turns a light on, and the next press turns it off again. Put it
by a door to light a room.

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
- a brick with a **Point Light**

```wiring
button Held -> chip pressed
chip light -> light Enabled
```

## Code

```wirescript name=bp1-toggle-light
const START_ON = false // whether the light is on when the chip is pasted

@left in pressed: exec
var lit: bool = START_ON
@right out light = lit.Value

on pressed {
  lit = !lit
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the **Button** and the
   **Point Light**.
2. Wire the Button's **Held** to `pressed`.
3. Wire `light` to the Point Light's **Enabled**.

## Check it in game

- Press the button: the light comes on, and stays on after you let go.
- Press it again: the light goes off.
- Hold the button down: nothing more happens until you let go and press
  again.

## Change it

- Start with the light on: set `START_ON = true`.
- Add a second button at the other end of the room. Add
  `@left in pressed2: bool`, wire the second Button's **Held** to it, make
  `pressed` a `bool` too, and change the handler to `on pressed || pressed2`.
  The OR reads both pins' values, which is why they are `bool`s. A press on
  one button while the other is held down does nothing: the OR is already on.
- Dim the light instead of switching it off:
  `@right out light: float = if lit.Value then 4.0 else 0.5`.
  `if A then X else Y` gives X while A is true and Y otherwise. Wire `light`
  to the Point Light's **Brightness**, and leave its **Enabled** on.

## How it works

`lit` is a variable the chip remembers ([Remembering Values](../state.md)). `on pressed`
runs once each time the button's **Held** turns on, and `lit = !lit` flips
the variable ([Pure and Exec](../pure-and-exec.md)). `light` follows the
variable all the time, so the light shows whatever was stored last.
[Toggles and Latches](../logic/toggles-and-latches.md) builds the same part
from the game's **Toggle** gate, and a latch with separate on and off
buttons.
