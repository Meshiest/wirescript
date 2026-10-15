# Shop

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Mods](../abstraction/mods.md), [Maps](../values/maps.md), [Per-Player State](../references/per-player-state.md)

</div>
<!-- /learn-links -->

A coin counter for every player and two shop buttons. Players earn coins
with one button and spend them on items with the others. An item goes
straight into the buyer's inventory.

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

- A **Button** that pays out coins (or a **Target**: see Change it).
- Two **Button**s for the shop, one per item.

```wiring
button1 "Press Count" -> chip earn
button1 Character -> chip earner
button2 "Press Count" -> chip buy1
button2 Character -> chip buyer1
button3 "Press Count" -> chip buy2
button3 Character -> chip buyer2
```

## Code

```wirescript name=gs6-shop
const EARN = 1                                // coins per press of the earn button
const PRICE_1 = 3                             // price of item 1
const PRICE_2 = 10                            // price of item 2
const ITEM_1 = $BRItemBase/Weapon_Bandage     // item 1
const ITEM_2 = $BRItemBase/Weapon_Pickaxe     // item 2

@left in earn: int
@left in earner: character
@left in buy1: int
@left in buyer1: character
@left in buy2: int
@left in buyer2: character

var coins: Map<string, int>
var lastEarn: int = 0
var last1: int = 0
var last2: int = 0

// true when a button's Press Count went up: a new press, not a release
mod pressed(holding: int, prev: *int) -> bool {
  let up = holding > prev
  prev = holding
  return up
}

mod balance(who: character) -> int {
  let had = coins.get(who.GetUserId())
  return if had.Found then had.Value else 0
}

mod pay(who: character, price: int) -> bool {
  let have = balance(who)
  if have < price {
    who.ShowStatusMessage("You need ${price} coins, you have ${have}")
    return false
  }
  coins.set(who.GetUserId(), have - price)
  who.ShowStatusMessage("Bought! ${have - price} coins left")
  return true
}

on Change(earn) {
  if pressed(earn, lastEarn) {
    let now = balance(earner) + EARN
    coins.set(earner.GetUserId(), now)
    earner.ShowStatusMessage("Coins: ${now}")
  }
}

on Change(buy1) {
  if pressed(buy1, last1) {
    if pay(buyer1, PRICE_1) {
      buyer1.AddInventoryItem(ITEM_1)
    }
  }
}

on Change(buy2) {
  if pressed(buy2, last2) {
    if pay(buyer2, PRICE_2) {
      buyer2.AddInventoryItem(ITEM_2)
    }
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the earn Button's **Press Count** to `earn` and its **Character**
   to `earner`.
3. Wire the first shop Button's **Press Count** and **Character** to `buy1`
   and `buyer1`, and the second's to `buy2` and `buyer2`.
4. Label the shop buttons with their item and price, on a sign or in each
   Button's **Prompt Custom Label** setting.

## Check it in game

- [ ] Press the first shop button with no coins: "You need 3 coins, you have
  0", and nothing is added to your inventory.
- [ ] Press the earn button three times: "Coins: 1", "Coins: 2", "Coins: 3".
  Letting go gives nothing.
- [ ] Press the first shop button: "Bought! 0 coins left", and a bandage is
  in your inventory.
- [ ] A second player earns and buys: their coins are their own.
- [ ] Hold the earn button down while a second player presses it: they
  still get their coin.
- [ ] Die and respawn: your coins are still there.

## Change it

- Sell other items: the editor lists every item name after
  `$BRItemBase/`. Change `ITEM_1` and `PRICE_1`.
- Earn coins from a **Target** instead of a button: make `earn` an
  `exec` pin, change its handler to `on earn` without the `pressed` check,
  and wire **Just Hit** to
  `earn` and **Character That Just Hit** to `earner`. Or give coins at the
  end of a course, in a zone's `on ZoneEntered` handler.
- Add a third item: two more pins, a `last3` var and an `on Change(buy3)`
  handler. `pressed` and `pay` work for it unchanged.

## How it works

Every player's coins are one entry in a [map](../values/maps.md)
keyed by `GetUserId()`, so they survive a respawn and are saved with the
bricks. Two [mods](../abstraction/mods.md) hold the rules. `balance` reads a
player's coins, 0 when they have no entry yet. `pay` checks the price and
takes the coins, and its `-> bool` result tells the handler whether to hand
over the item. Each Button's **Character** output says who pressed it, so
one button serves every player. A Button's **Press Count** is how many
players hold it right now. The handlers run on every `Change` of it
([Change](../../builtins.md#edge--change-detectors)), and the `pressed` mod
lets one through only when the count went up since the last change: a new
press, never a release. Each button keeps its own last count (`lastEarn`,
`last1`, `last2`). **Held** stays on while anyone holds the button, so a
second player's press during that time would not turn it on again, though
**Press Count** and **Character** both change. `AddInventoryItem` adds an
item to the buyer's inventory. The item is a constant `$BRItemBase/...`
asset reference, chosen when the chip is compiled.

Back to [Parts](README.md).
