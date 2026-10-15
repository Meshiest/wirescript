# Index and Value

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Looping Over a Map](looping-over-a-map.md)

**Next steps:** [Loops Across Ticks](loops-across-ticks.md), [Shuffling](../random/shuffling.md), [A HUD for Everyone](../screen/a-hud-for-everyone.md)

**Parts that use this:** [Card Table](../parts/card-game.md)

</div>
<!-- /learn-links -->

A loop over an array can name both the element's position and the element
itself. This lesson uses the position to split the players in a lobby
into two teams, in the order they walked in.

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
zone "Zone Ref" -> chip lobby
button Held -> chip split
chip board -> decal Text
```

## Code

```wirescript name=lp4-teams
import { escape } from "sanitize"

@left in lobby: zone
@left in split: exec
var ids: string[]
var names: string[]
var shown: string = "Walk in, then press split"
@right out board = shown.Value

on ZoneEntered(zone = lobby) -> (character) {
  let id = character.GetUserId()
  if !ids.find(id).Found {
    ids.push(id)
    names.push(escape(character.GetDisplayName()))
  }
}

on split {
  var red: string = ""
  var blue: string = ""
  for i, name in names {
    if i % 2 == 0 {
      red = "${red} ${name}"
    } else {
      blue = "${blue} ${name}"
    }
  }
  shown = '<color="ff5050">Red:${red}<br><color="5090ff">Blue:${blue}'
}
```

- The zone handler signs players up. `ids.find(id).Found` is true when the
  id is in the list already, so each player is added once, however often
  they walk in. `ids` holds user ids, and `names` holds the matching display
  names in the same order.
- `for i, name in names` names two slots. `i` is the element's index (0 for
  the first, 1 for the second) and `name` is the element. With one name, as
  in [Looping Over a List](looping-over-a-list.md), you get only the element.
  An array loop puts the index first, and a map loop puts the key first and
  the index third.
- `i % 2 == 0` is true for even indexes, so the first, third and fifth
  players go to Red, and the others to Blue.
- The sign is rich text. `<color="...">` colors the text after it, and `<br>`
  starts a new line. The text is in single quotes, so the double quotes
  inside the tags need no escaping.

Four players walk in, in the order A, B, C, D. Who is on Blue?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**,
   and place a **Button** and a brick with a **Text Decal**.
2. Wire the Attached Zone's **Zone Ref** to `lobby`, the Button's **Held** to
   `split`, and `board` to the Text Decal's **Text**.
3. Walk into the zone and press the button. Alone, you are the only one on
   Red. With friends, each new player alternates between the teams.

## Under the Hood

Open the chip and the **For Each Array** inside it. Its **For Each Array
Entry** gate has an **Index** output beside **Value**. `i % 2` is a
**Modulo** gate wired from **Index**, then an **Equal** and a **Branch**.
Naming `i` costs nothing extra, because the output is always there.

Each player's id and name are pushed together, so `ids` and `names` stay in
the same order. Index 2 of `names` is the name of the player whose id is
index 2 of `ids`.

`.step(2)` looks like a way to take every other player, but it only works on
a range:

```wirescript name=lp4-step expect=WS083
@left in split: exec
var names: string[] = ["A", "B", "C", "D"]
var red: string = ""
@right out board = red.Value

on split {
  red = ""
  for name in names.step(2) {
    red = "${red} ${name}"
  }
}
```

```text
Error: [WS083] `.step()` applies to a range, not to an array or map. Give the loop a range source (`for i in 0..n`), or drop the `.step()`.
```

Use the index to choose instead, as the lesson's code does.

## Modify

1. Test alone with made-up players. Start the list with names in it:
   `var names: string[] = ["A", "B", "C"]`. Walk in, and you are the fourth.
2. Make three teams. Use `i % 3`, with a third color and a third line on the
   sign.
3. Show a numbered sign-up list of the last five players instead of teams.
   Work out the length once, before the loop, and skip the earlier ones:

   ```wirescript name=lp4-last-five
   import { escape } from "sanitize"

   @left in lobby: zone
   var ids: string[]
   var names: string[]
   var shown: string = "Nobody yet"
   @right out board = shown.Value

   on ZoneEntered(zone = lobby) -> (character) {
     let id = character.GetUserId()
     if !ids.find(id).Found {
       ids.push(id)
       names.push(escape(character.GetDisplayName()))
     }
     let total = names.length()
     var text: string = ""
     for i, name in names {
       if i >= total - 5 {
         text = "${text}${i + 1}. ${name}<br>"
       }
     }
     shown = text
   }
   ```

   `total` is read once, above the loop. Written inside the `if`, the length
   would be read again on every iteration.

## Make

Make a relay order board. Players sign up at a zone, and the sign shows them
as legs of a relay: "Leg 1: A", "Leg 2: B" and so on. Add a second button that
removes the first player in the list (`names.remove(0)`, and the same for
`ids`), so the next player moves up to leg 1.

```wiring
zone "Zone Ref" -> chip lobby
button1 Held -> chip show
button2 Held -> chip next
chip board -> decal Text
```

## Good to Know

- The index starts at 0, not 1. Add 1 before showing it to people.
- Two arrays kept side by side stay matched only while every change is made
  to both. Remove from one and not the other, and every name after that point
  lines up with the wrong id.
- One player cannot test a two-team split. Seed the list with test names, or
  ask a friend to walk in.

See [Bindings](../../statements.md#bindings) in the reference.

Next: [Loops Across Ticks](loops-across-ticks.md).
