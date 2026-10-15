# Card Table

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Shuffling](../random/shuffling.md), [Ready Check](../input/ready-check.md), [Index and Value](../loops/index-and-value.md), [Mods](../abstraction/mods.md), [Safe Text](../strings/safe-text.md), [Keeping It Current](../screen/keeping-it-current.md)

</div>
<!-- /learn-links -->

A card table for two to four players. Each player stands on a seat pad, a
deal button shuffles a deck and deals three cards to everyone, and each
player sees their own hand on their screen and nobody else's. Players take
turns playing a card with three buttons: the highest card takes the trick,
and whoever takes the most tricks wins.

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

- Four bricks with an **Attached Zone** each, the seat pads, small enough
  for one player.
- A **Button** to deal.
- Three **Button**s in the middle of the table, one per card in a hand.
- A brick with a **Text Decal** for the table, where everyone can see it.

```wiring
zone1 "Zone Ref" -> chip seat1
zone2 "Zone Ref" -> chip seat2
zone3 "Zone Ref" -> chip seat3
zone4 "Zone Ref" -> chip seat4
button1 Held -> chip deal
button2 "Press Count" -> chip card1
button2 Character -> chip by1
button3 "Press Count" -> chip card2
button3 Character -> chip by2
button4 "Press Count" -> chip card3
button4 Character -> chip by3
chip sign -> decal Text
```

## Code

```wirescript name=cp3-card-table
import { escape } from "sanitize"

const HAND = 3        // cards per player, one Button per card
const TEXT_ID = 7300  // a number no other circuit on the server uses
const FONT_SIZE = 32  // size of the hand on each screen

@left in seat1: zone
@left in seat2: zone
@left in seat3: zone
@left in seat4: zone
@left in deal: exec
@left in card1: int
@left in by1: character
@left in card2: int
@left in by2: character
@left in card3: int
@left in by3: character

var crowd: character[]
var seated: character[]
var ids: string[]
var names: string[]
var points: int[]
var deck: int[]
var hands: int[]
var playing: bool = false
var turn: int = 0
var trick: int = 0
var played: int = 0
var high: int = -1
var leader: int = 0
var table: string = "Stand on a seat and press deal"
var last: string = ""
var held1: int = 0
var held2: int = 0
var held3: int = 0
var presser: character
var pick: int = 0
@right out sign = "${table.Value}<br>${last.Value}"

let card: exec
let redraw: exec

// true when a button's Press Count went up: a new press, not a release
mod pressed(holding: int, prev: *int) -> bool {
  let up = holding > prev
  prev = holding
  return up
}

mod cardName(c: int) -> string {
  let rank = "23456789TJQKA".Substring(c / 4, 1)
  let suit = "SHDC".Substring(c % 4, 1)
  return "${rank}${suit}"
}

mod sit(pad: zone) {
  crowd.fillFromZonePlayers(pad)
  if crowd.length() > 0 {
    let p = crowd[0]
    let id = p.GetUserId()
    if !ids.find(id).Found {
      seated.push(p)
      ids.push(id)
      names.push(escape(p.GetDisplayName()))
      points.push(0)
    }
  }
}

mod tell(text: string) {
  for p in seated {
    p.ShowStatusMessage(text)
  }
}

mod finish() {
  var top: int = 0
  var tied: bool = false
  for i, n in points {
    if n > points[top] {
      top = i
      tied = false
    } else if i != top && n == points[top] {
      tied = true
    }
  }
  playing = false
  if tied {
    table = "A tie at ${points[top]} tricks!<br>Press deal to play again"
    tell("A tie!")
  } else {
    table = "${names[top]} wins with ${points[top]} tricks!<br>Press deal to play again"
    tell("${names[top]} wins!")
  }
}

mod play(who: character, slot: int) {
  if !playing {
    return
  }
  if who.GetUserId() != ids[turn] {
    who.ShowStatusMessage("Wait for your turn")
    return
  }
  let at = turn * HAND + slot
  let c = hands[at]
  if c < 0 {
    who.ShowStatusMessage("You played that card already")
    return
  }
  hands[at] = -1
  seated[turn] = who
  let sep = if played == 0 then " " else ", "
  table = "${table}${sep}${names[turn]} ${cardName(c)}"
  if c > high {
    high = c
    leader = turn
  }
  played += 1
  if played < seated.length() {
    turn = (turn + 1) % seated.length()
  } else {
    points[leader] += 1
    last = "${table}: ${names[leader]} takes it"
    tell("${names[leader]} takes the trick with ${cardName(high)}")
    trick += 1
    if trick == HAND {
      finish()
    } else {
      turn = leader
      played = 0
      high = -1
      table = "Trick ${trick + 1}:"
    }
  }
  emit redraw
}

on deal {
  for p in seated {
    p.DisplayText("", textId = TEXT_ID)
  }
  seated.clear()
  ids.clear()
  names.clear()
  points.clear()
  sit(seat1)
  sit(seat2)
  sit(seat3)
  sit(seat4)
  if seated.length() < 2 {
    table = "Need two players on the seats"
    playing = false
    return
  }
  deck.clear()
  for i in 0..52 {
    deck.push(i)
  }
  deck.shuffle()
  hands.clear()
  for i in 0..HAND * 4 {
    hands.push(deck[i])
  }
  turn = 0
  trick = 0
  played = 0
  high = -1
  playing = true
  table = "Trick 1:"
  last = ""
  tell("Cards dealt. ${names[0]} goes first")
  emit redraw
}

on Change(card1) {
  if pressed(card1, held1) {
    presser = by1
    pick = 0
    emit card
  }
}
on Change(card2) {
  if pressed(card2, held2) {
    presser = by2
    pick = 1
    emit card
  }
}
on Change(card3) {
  if pressed(card3, held3) {
    presser = by3
    pick = 2
    emit card
  }
}

on card {
  play(presser, pick)
}

on redraw {
  for i, p in seated {
    if !playing {
      p.DisplayText("", textId = TEXT_ID)
    } else {
      var text: string = "Your hand:"
      for k in 0..HAND {
        let c = hands[i * HAND + k]
        let shown = if c < 0 then "--" else cardName(c)
        text = "${text}  [${k + 1}] ${shown}"
      }
      let note = if i == turn then "Your turn: press a card's button" else "Waiting for ${names[turn]}"
      p.DisplayText("${text}<br>${note}<br>Tricks: ${points[i]}",
        textId = TEXT_ID, anchorX = 0.5, anchorY = 1.0, positionY = -180.0,
        fontSize = FONT_SIZE, justify = Center, lifetime = 0.0)
    }
  }
}
```

A card is a number from 0 to 51. `c / 4` is its rank and `c % 4` its suit,
so a higher number is a higher card. `cardName` writes it as two letters:
the rank (`2` to `9`, then `T` for ten, `J`, `Q`, `K`, `A`) and the suit
(`S`, `H`, `D`, `C`). Two cards of the same rank are told apart by suit, in
that order: clubs are highest.

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the four seat zones' **Zone Ref** outputs to `seat1` to `seat4`. The
   seat order is the turn order.
3. Wire the deal Button's **Held** to `deal`.
4. Wire the first card Button's **Press Count** to `card1` and its
   **Character** to `by1`, and the same for the second (`card2`, `by2`)
   and third (`card3`, `by3`).
5. Wire `sign` to the Text Decal's **Text**.

## Check it in game

You need a second player for most of this list.

- [ ] The sign reads "Stand on a seat and press deal".
- [ ] Alone on a seat, press deal: "Need two players on the seats".
- [ ] With two players seated, press deal: each sees "Cards dealt", and
  their own hand at the bottom of the screen: three cards, different for
  each player.
- [ ] The first seat's screen says "Your turn", the other "Waiting for"
  the first player.
- [ ] The second player presses a card button: "Wait for your turn", and
  nothing is played.
- [ ] The first player presses button 2: their second card shows `--`, the
  sign reads "Trick 1:" with their name and card, and the turn passes on.
- [ ] The second player plays: everyone sees who takes the trick, and the
  sign's second line keeps the finished trick.
- [ ] On your turn, press the button for a card you already played: "You
  played that card already".
- [ ] After three tricks the sign names the winner, or says "A tie", and the
  hands vanish from the screens.
- [ ] Press deal again: the old hands are replaced by new ones.

## Change it

- Five cards a hand: `HAND = 5`, two more card buttons with `card4`/`by4`
  and `card5`/`by5` pins, `held4` and `held5` vars, and their
  `on Change(...)` handlers, setting `pick` to 3 and 4. With four players
  that deals 20 of the 52 cards.
- Low card wins: start `high` at 52 instead of -1 (three places), and in
  `play` test `c < high`.
- Follow suit: remember the suit of the first card in a trick, and refuse a
  card of another suit while the player still holds one of that suit.
- Keep a score across games: a `Map<string, int>` of wins keyed by
  `GetUserId()`, added to in `finish` and shown on the sign.

## How it works

The players are fixed when the cards are dealt: `sit` fills an
[array](../values/lists.md) with whoever stands on each pad, as in
[Ready Check](../input/ready-check.md), and keeps the first one. From then
on a player is a number, their seat, and every list (`seated`, `ids`,
`names`, `points`) lines up by that number. A player already in `ids` is
not seated twice. Their hand is three slots in `hands`, starting at
`seat * HAND`, and a played card becomes -1. The deck is the numbers 0 to
51, [shuffled](../random/shuffling.md), and the first cards go to the
hands.

A card button says only "someone pressed card 2". Its **Press Count** is
how many players hold it right now: it goes up on a press and down on a
release. `on Change(card2)` runs on both
([Change](../../builtins.md#edge--change-detectors)), and the `pressed` mod
lets only a rise through, so a release plays nothing. **Held** would miss a
press made while another player still holds the same button, since it stays
on. Each button's handler stores who pressed (its **Character**) and which
card, then fires the chip's own `card` signal, so the three buttons share
one `card` handler and `play` is built once instead of once per button.
`play` compares the presser's `GetUserId()` with the one whose turn it is,
so pressing out of turn does nothing. When every player has
played, the highest card's player gets a point and leads the next trick.
At the end, `finish` finds the most tricks and checks whether anyone else
has as many.

The hands are private because each one is drawn with `DisplayText`, which
shows text on one player's screen, as in [Scoreboard
HUD](scoreboard-hud.md). They are drawn only when something changes: the
deal and every card played end by firing `redraw`, which draws each hand
with the same `textId`, so each draw replaces the last. `lifetime = 0.0`
keeps a hand up until the next draw, and once the game is over `redraw`
draws an empty text instead, which takes the hands down. A new deal first
takes down the hands of the last game's players. Status messages go to
every seated player with the `tell` [mod](../abstraction/mods.md), and the
names are escaped when they are stored ([Safe
Text](../strings/safe-text.md)).

The chip holds each player's character from the deal. A player who dies or
respawns during a game has a new character, so their hand stops updating
until their next turn: playing a card stores the new character in their
seat. Their turns still work, since those go by user id. A player who
leaves never takes their turn: press deal to start over.

Back to [Parts](README.md).
