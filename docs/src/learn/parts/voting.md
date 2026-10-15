# Voting

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Looping Over a Map](../loops/looping-over-a-map.md), [Mods](../abstraction/mods.md), [Exec Pins](../events/exec-pins.md), [Per-Player State](../references/per-player-state.md)

</div>
<!-- /learn-links -->

A yes-or-no vote. A start button opens it for 30 seconds, players press a
yes or a no button, and when time is up the circuit fires `passed` or
`failed`. Each player has one vote, and pressing the other button changes
it.

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

- Three **Button**s: start the vote, yes, no.
- A brick with a **Text Decal** for the question and the count.
- Whatever the result should do: a door, a light, a round manager.

```wiring
button1 Held -> chip begin
button2 "Press Count" -> chip yes
button2 Character -> chip yesVoter
button3 "Press Count" -> chip no
button3 Character -> chip noVoter
chip sign -> decal Text
chip passed -> circuit "exec input"
chip failed -> circuit "exec input"
```

## Code

```wirescript name=gs7-voting
const QUESTION = "Play another round?" // shown on the sign
const VOTE_SECONDS = 30                // how long a vote stays open

@left in begin: exec
@left in yes: int
@left in yesVoter: character
@left in no: int
@left in noVoter: character

var votes: Map<string, bool>
var counting: bool = false
var yesCount: int = 0
var noCount: int = 0
var timeLeft: int = 0
var lastYes: int = 0
var lastNo: int = 0
@right out sign = if counting.Value
  then "${QUESTION}<br>Yes ${yesCount.Value} - No ${noCount.Value}<br>${timeLeft.Value}s left"
  else "${QUESTION}<br>Yes ${yesCount.Value} - No ${noCount.Value}"
@right out passed: exec
@right out failed: exec

// true when a button's Press Count went up: a new press, not a release
mod pressed(holding: int, prev: *int) -> bool {
  let up = holding > prev
  prev = holding
  return up
}

mod cast(who: character, choice: bool) {
  if !counting {
    who.ShowStatusMessage("No vote is open")
    return
  }
  votes.set(who.GetUserId(), choice)
  var y: int = 0
  var n: int = 0
  for id, v in votes {
    if v {
      y += 1
    } else {
      n += 1
    }
  }
  yesCount = y
  noCount = n
  who.ShowStatusMessage(if choice then "You voted yes" else "You voted no")
}

on begin {
  if !counting {
    votes.clear()
    yesCount = 0
    noCount = 0
    timeLeft = VOTE_SECONDS
    counting = true
  }
}

on Change(yes) {
  if pressed(yes, lastYes) { cast(yesVoter, true) }
}
on Change(no) {
  if pressed(no, lastNo) { cast(noVoter, false) }
}

on Clock(interval = 1.0, enabled = counting) {
  timeLeft -= 1
  if timeLeft == 0 {
    counting = false
    if yesCount > noCount {
      emit passed
    } else {
      emit failed
    }
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the start Button's **Held** to `begin`.
3. Wire the yes Button's **Press Count** and **Character** to `yes` and
   `yesVoter`, and the no Button's to `no` and `noVoter`.
4. Wire `sign` to the Text Decal's **Text**, and `passed` and `failed` to
   what each result should do.

## Check it in game

- [ ] Press yes before any vote: "No vote is open".
- [ ] Press start: the sign shows the question, "Yes 0 - No 0" and "30s
  left", counting down.
- [ ] Press yes: "Yes 1 - No 0". Press yes again: still 1.
- [ ] Press no: "Yes 0 - No 1". Your vote moved.
- [ ] A second player votes yes: "Yes 1 - No 1".
- [ ] Two players press yes at once, one still holding it down: both votes
  count.
- [ ] Let the time run out: the countdown line goes, and with a tie `failed`
  fires once.
- [ ] Press start again: the count starts from zero.

## Change it

- Vote by standing: replace the buttons with two zones and
  `on ZoneEntered(zone = yesZone) -> (character) { cast(character, true) }`.
- Pass only with two thirds of the votes, and at least one:
  `if yesCount > 0 && yesCount * 3 >= (yesCount + noCount) * 2`.
- Ask each player in a pop-up box instead: `ShowYesNoBox` does that, but
  every dialog gate needs admin to load, so a chip using it is blocked for
  anyone else who pastes it.

## How it works

Each vote is one entry in a [map](../values/maps.md) keyed by
`GetUserId()`, holding `true` for yes. Setting the entry again replaces the
old vote, so each player has one vote. After each vote, `cast`
counts the whole map with `for id, v in votes` ([Looping Over a
Map](../loops/looping-over-a-map.md)), so a changed vote moves from one
count to the other with no extra code. `cast` is a
[mod](../abstraction/mods.md) shared by both buttons. A Button's **Press
Count** is how many players hold it right now, and each handler runs on a
`Change` of it ([Change](../../builtins.md#edge--change-detectors)). The
`pressed` mod lets it through only when the count went up: once per press,
even while someone else is still holding the button and **Held** stays on,
and never on a release. While `counting` is on, a Clock takes one second
off `timeLeft` each second, the same countdown as the [Countdown
Sign](countdown-sign.md). At zero it closes the vote, and the result leaves
the chip as one of two exec pulses ([Exec Pins](../events/exec-pins.md))
from that one handler.

Back to [Parts](README.md).
