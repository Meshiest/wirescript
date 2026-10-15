# Hold to Use

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Waiting](../events/waiting.md)

</div>
<!-- /learn-links -->

A door that opens only for a player who holds its button down for two
seconds. A status message tells them to keep holding. If they let go early,
nothing happens.

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

- a **Button** beside the door
- a door: a few bricks, one of them with a **Brick Property Changer**

```wiring
button Held -> chip held
button Character -> chip holder
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Code

```wirescript name=pp9-hold-to-use
// Seconds the button must be held.
const HOLD = 2.0
// Seconds the door stays open.
const OPEN_SECONDS = 5.0

@left in held: bool
@left in holder: character

var opened: bool = false
@right out solid = !opened.Value

let unlock: exec

on held {
  holder.ShowStatusMessage("Keep holding...")
  await !held || Sleep(_, delay = HOLD)
  if held {
    holder.ShowStatusMessage("Done!")
    emit unlock
  } else {
    holder.ShowStatusMessage("Too short")
  }
}

on unlock {
  opened = true
  await Sleep(_, delay = OPEN_SECONDS)
  opened = false
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the Button's **Held** to `held` and its **Character** to `holder`.
3. Wire `solid` to the Brick Property Changer's **Collision Enabled** and
   **Visibility** inputs.

## Check it in game

- Tap the button: the message says "Keep holding...", then "Too short",
  and the door stays shut.
- Hold the button: after two seconds the message says "Done!" and the door
  vanishes.
- Five seconds later the door is back.
- Hold for one second, let go, and hold again: the two seconds start over
  from the second press.

## Change it

- Change `HOLD` for a longer or shorter hold, and `OPEN_SECONDS` for how
  long the door stays open.
- Count the uses on a sign: add `var uses: int = 0`, put `uses += 1` at
  the top of `on unlock`, and add
  `@right out text = "Used ${uses.Value} times"`, wired to a **Text
  Decal**'s **Text**.
- Put a second button on the far side of the door: add a second pair of
  pins and a copy of the `on held` handler that reads them, and fire the
  same `unlock`.

## How it works

The Button's **Held** is on for as long as the button is held down, and
`on held` runs when it turns on ([Pure and Exec](../pure-and-exec.md)). The
handler then races two waits
([Waiting](../events/waiting.md)): `!held` turns on the moment the button is
let go, and the `Sleep` after two seconds. Whichever comes first resumes the
chain, and `if held` tells them apart: still held means the full two
seconds passed. The `unlock` signal starts a second handler that holds the
door open with another `await`. **Character** is the player who pressed, so
the messages go to them alone.
