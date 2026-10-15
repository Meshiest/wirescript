# Weighted Loot

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Shuffling](shuffling.md)

**Parts that use this:** [Paint Gun](../parts/paint-gun.md)

</div>
<!-- /learn-links -->

Some prizes should be rarer than others. This lesson gives each prize a
weight, rolls one number, and walks a running total until the roll falls
inside a prize's share.

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
button Held -> chip pull
chip text -> decal Text
```

## Code

```wirescript name=rn4-loot
const NAMES = ["Common", "Rare", "Epic"]
const WEIGHTS = [60, 30, 10]

@left in pull: exec
var prize: string = "Pull the lever"
@right out text = prize.Value

on pull {
  let roll = Random(1, 100)
  var total: int = 0
  var pick: int = 0
  for i, w in WEIGHTS {
    total += w
    if roll <= total {
      pick = i
      break
    }
  }
  prize = "${NAMES[pick]} (rolled ${roll})"
}
```

- `NAMES` and `WEIGHTS` are two `const` arrays, one entry per prize, in the
  same order. The weights add up to 100, so each is the chance in a hundred.
- `roll` is a number from 1 to 100.
- The loop adds each weight to `total` in turn: 60, then 90, then 100. The
  first time `roll` is at or under `total`, that prize wins and `break` stops
  the loop.
- So 1 to 60 is Common, 61 to 90 is Rare, and 91 to 100 is Epic.

The roll is 75. Which prize wins? What about 91?

## Run

1. Compile the chip and paste it in-game. Place a **Button** (the lever) and a
   brick with a **Text Decal**.
2. Wire the Button's **Held** to `pull`, and `text` to the Text Decal's
   **Text**.
3. Pull a few times. Most pulls are Common, about one in three is Rare, and
   Epic turns up about once in ten.

## Under the Hood

Open the chip. A `const` array that the circuit reads while it runs becomes
an **Array Var** with its values typed in. `WEIGHTS` feeds the
**For Each Array** loop. `NAMES` is read once, after the loop, by a
**Get Array Element** with `pick` as its index.

Inside the loop, the roll comes in on a wire from the **Random** gate outside.
It is rolled once, before the loop, so every iteration compares against the
same number. A `Random` inside the loop would roll again on every iteration, and
the weights would no longer mean what they say.

The arrays are `const`, so they cannot change while the game runs, and
`WEIGHTS.push(5)` does not compile. Declare one with `var` to make it
changeable.

## Modify

1. Add a Legendary prize. Add `"Legendary"` to `NAMES` and a weight of 2 to
   `WEIGHTS`, and take 2 off Epic so the weights still add up to 100. Then
   check by hand which rolls give Legendary.
2. Color the prize by its rarity. A map literal, `{ key: value, ... }`, can be
   a `const` too:

   ```wirescript name=rn4-colors
   const NAMES = ["Common", "Rare", "Epic"]
   const WEIGHTS = [60, 30, 10]
   const COLORS = { "Common": "aaaaaa", "Rare": "4080ff", "Epic": "c050ff" }

   @left in pull: exec
   var prize: string = "Pull the lever"
   @right out text = prize.Value

   on pull {
     let roll = Random(1, 100)
     var total: int = 0
     var pick: int = 0
     for i, w in WEIGHTS {
       total += w
       if roll <= total {
         pick = i
         break
       }
     }
     let won = NAMES[pick]
     prize = '<color="${COLORS[won]}">${won}'
   }
   ```

   `COLORS[won]` looks up the color for the prize's name, the way
   `NAMES[pick]` looks up a name by its index.
3. Keep each prize and its weight together in one map, and loop over it:

   ```wirescript name=rn4-table
   const LOOT = { "Common": 60, "Rare": 30, "Epic": 10 }

   @left in pull: exec
   var prize: string = "Pull the lever"
   @right out text = prize.Value

   on pull {
     let roll = Random(1, 100)
     var total: int = 0
     var won: string = ""
     for name, w in LOOT {
       total += w
       if roll <= total {
         won = name
         break
       }
     }
     prize = "${won} (rolled ${roll})"
   }
   ```

   The loop may visit the prizes in any order. That does not change the
   odds, because each prize still gets a stretch of the numbers as wide as
   its weight.

## Make

Make a loot chest that never gives the same prize twice in a row. When the
roll lands on the prize from last time, roll again, at most three times, and
take whatever the third roll gives. Keep the last prize in a variable.

```wiring
button Held -> chip pull
chip text -> decal Text
```

## Good to Know

- The weights must add up to the top of the roll. If they add up to 90 and
  the roll goes to 100, a roll of 91 or more matches nothing, and the sign
  shows the first prize, since `pick` was never changed.
- Roll once, outside the loop. A roll inside the loop changes on every
  iteration.
- Weights set only the chances. Ten pulls can bring two Epics, or none.

See [`const` containers at runtime](../../statements.md#const-containers-at-runtime)
in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
