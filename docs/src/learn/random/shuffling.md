# Shuffling

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Random Colors](random-colors.md), [Index and Value](../loops/index-and-value.md)

**Next steps:** [Weighted Loot](weighted-loot.md), [Teams](../admin/teams.md)

**Parts that use this:** [Card Table](../parts/card-game.md)

</div>
<!-- /learn-links -->

`shuffle()` puts an array in a random order. Every element stays, and none is
repeated, which is what you want for a turn order, a team draw or a deck of
cards.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip deal
chip board -> decal Text
```

## Code

```wirescript name=rn3-turn-order
@left in deal: exec
var names: string[] = ["Ann", "Bo", "Cy", "Dee", "Eve"]
var shown: string = "Press to pick the order"
@right out board = shown.Value

on deal {
  names.shuffle()
  var text: string = ""
  for i, name in names {
    text = "${text}${i + 1}. ${name}<br>"
  }
  shown = text
}
```

- `names` starts with five names typed in, so you can try it alone.
- `names.shuffle()` puts the elements in a random order, in place, so `names`
  itself changes.
- The loop from [Index and Value](../loops/index-and-value.md) writes the new
  order as a numbered list, one name per line.

Press the button a few times. Can a name ever appear twice? Can one be left
out?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with
   a **Text Decal**.
2. Wire the Button's **Held** to `deal`, and `board` to the Text Decal's
   **Text**.
3. Press the button. The five names come out in a new order each time, all
   five, once each.

## Under the Hood

Open the chip. `names.shuffle()` is one **Shuffle Array** gate, wired to the
`names` **Array Var**. Then comes the **For Each Array** loop that builds the
list.

Compare picking each place with `Random` instead:

```wirescript name=rn3-with-random
@left in deal: exec
var names: string[] = ["Ann", "Bo", "Cy", "Dee", "Eve"]
var shown: string = "Press to pick the order"
@right out board = shown.Value

on deal {
  var text: string = ""
  for i in 0..5 {
    text = "${text}${i + 1}. ${names[Random(0, 4)]}<br>"
  }
  shown = text
}
```

Each place rolls on its own here, so a name can come up twice and another not
at all. A shuffle only moves the names around, so that cannot happen.

## Modify

1. Split into teams after the shuffle. Use `i % 2` as in
   [Index and Value](../loops/index-and-value.md): even places on Red, odd
   places on Blue. The teams now change on every press.
2. Shuffle real players. Take the lobby from Index and Value, where a zone
   pushes each player's id into `ids` and escaped name into `names`. Do not shuffle
   `names` itself, or it no longer lines up with `ids`. Shuffle a copy:

   ```wirescript name=rn3-lobby
   import { escape } from "sanitize"

   @left in lobby: zone
   @left in deal: exec
   var ids: string[]
   var names: string[]
   var order: string[]
   var shown: string = "Walk in, then press deal"
   @right out board = shown.Value

   on ZoneEntered(zone = lobby) -> (character) {
     let id = character.GetUserId()
     if !ids.find(id).Found {
       ids.push(id)
       names.push(escape(character.GetDisplayName()))
     }
   }

   on deal {
     order.copyFrom(names)
     order.shuffle()
     var text: string = ""
     for i, name in order {
       text = "${text}${i + 1}. ${name}<br>"
     }
     shown = text
   }
   ```

   `copyFrom` replaces everything in `order` with the elements of `names`,
   and the loop then walks `order`. Wire an **Attached Zone**'s **Zone Ref**
   to `lobby`.
3. Deal from a deck. Each press draws one card, and the deck is shuffled
   again only when it runs out:

   ```wirescript name=rn3-deck
   const CARDS = ["Ace", "King", "Queen", "Jack", "Ten"]

   @left in draw: exec
   var deck: string[]
   var shown: string = "Press to draw"
   @right out card = shown.Value

   on draw {
     if deck.length() == 0 {
       deck.copyFrom(CARDS)
       deck.shuffle()
     }
     let top = deck.pop()
     shown = "${top} (${deck.length()} left)"
   }
   ```

   `deck.pop()` takes the last card off the deck. Five presses show all five
   cards, and the sixth starts a new shuffled deck.

## Make

Make a raffle. Players walk into a zone to enter, once each. A button shuffles
the entries and shows the first three as 1st, 2nd and 3rd prize. A second
button clears the entries for the next raffle.

```wiring
zone "Zone Ref" -> chip entry
button1 Held -> chip draw
button2 Held -> chip clear
chip board -> decal Text
```

## Good to Know

- `shuffle()` changes the array itself. If another array has to stay lined up
  with it, shuffle a copy.
- A shuffle of an empty array, or of one element, leaves it as it was.
- With one player you cannot see a random team split. Test with names typed
  in, as this lesson's code does.

See [Array Methods](../../builtins.md#array-methods-exec) in the reference.

Next: [Weighted Loot](weighted-loot.md).
