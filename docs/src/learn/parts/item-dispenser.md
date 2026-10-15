# Item Dispenser

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Maps](../values/maps.md)

</div>
<!-- /learn-links -->

Press a button and an item lands in your inventory. The dispenser holds a
limited stock, each player may take only so many, and a second button
restocks it. A sign shows how many are left.

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

- a **Button**, to take an item
- a second **Button**, to restock (put it somewhere only you can reach)
- a brick with a **Text Decal**, for the stock

```wiring
button1 "Press Count" -> chip holding
button1 Character -> chip taker
button2 Held -> chip restock
chip sign -> decal Text
```

## Code

```wirescript name=pp7-dispenser
// The item each press hands out.
const ITEM = $BRItemBase/Weapon_HealthPotion
// How many the dispenser holds until it is restocked.
const STOCK = 10
// How many one player may take until it is restocked.
const PER_PLAYER = 1

@left in holding: int
@left in taker: character
@left in restock: exec

var stock: int = STOCK
var taken: Map<string, int>
var last: int = 0
@right out sign = "${stock.Value} left"

on Change(holding) {
  if holding > last {
    let id = taker.GetUserId()
    let had = taken.get(id)
    let mine = if had.Found then had.Value else 0
    if stock <= 0 {
      taker.ShowStatusMessage("Sold out")
    } else if mine >= PER_PLAYER {
      taker.ShowStatusMessage("You already took yours")
    } else {
      taker.AddInventoryItem(ITEM)
      taken.set(id, mine + 1)
      stock -= 1
    }
  }
  last = holding
}

on restock {
  stock = STOCK
  taken.clear()
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the take Button's **Press Count** to `holding` and its
   **Character** to `taker`.
3. Wire the restock Button's **Held** to `restock`.
4. Wire `sign` to the Text Decal's **Text**.

## Check it in game

- Before anyone presses: the sign says `10 left`.
- Press the take button: a health potion appears in your inventory, and the
  sign says `9 left`.
- Press it again: nothing is added, and a status message says you already
  took yours.
- With a second player: they get one too, and the sign goes down again.
- Hold the take button down while a second player presses it: they still
  get theirs.
- Press restock: the sign is back to `10 left`, and everyone may take one
  more.

## Change it

- Hand out something else: change `ITEM`. The editor lists the item names
  after you type `$BRItemBase/`.
- Change `STOCK` and `PER_PLAYER`. A large `STOCK` makes a free dispenser.
- Give each player a random item: put three items in a `const` array and
  pick one with `Random` ([Weighted Loot](../random/weighted-loot.md)).

## How it works

A Button's **Character** is the player who pressed it last, so the handler
knows whom to give the item to. **Press Count** is how many players hold the
button right now. The handler runs whenever it changes
([Change](../../builtins.md#edge--change-detectors)), and `holding > last`
lets only a rise through: one new press, by anyone. A release lowers the
count and gives nothing. **Held** stays on while anyone holds the button, so
it would miss a second player pressing during that time.
`$BRItemBase/...` names one of the game's items.
It is written into the gate, so it needs no wire. How many items each player
took is a map keyed by user id, and `.Found` covers a player who has not
taken any yet ([Maps](../values/maps.md)). `if ...
else if ... else` checks the stock first, then the player's share
([Reacting to the Game](../reacting-to-the-game.md)). Restocking clears the
map, so everyone starts over.
