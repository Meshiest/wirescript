# Ref Parameters

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Object References](object-references.md), [Mods](../abstraction/mods.md), [Lists](../values/lists.md)

**Next steps:** [Ref Values](ref-values.md)

</div>
<!-- /learn-links -->

A mod's inputs are copies: changing one inside the mod changes nothing
outside it. Mark an input with `*` and the caller hands over the variable
itself, so the mod can change it. This lesson scores a two-team match with one
mod that changes three variables.

A `*int` is a reference, like the ones in
[Object References](object-references.md). A `character` names a player's
body out in the world, and a `*int` names a variable kept somewhere else.

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
target1 "Just Hit" -> chip redHit
target2 "Just Hit" -> chip blueHit
chip sign -> decal Text
```

## Code

```wirescript name=r2-match
@left in redHit: exec
@left in blueHit: exec
var red: int = 0
var blue: int = 0
var redRounds: int = 0
var blueRounds: int = 0
@right out sign = "Red ${red.Value} - ${blue.Value} Blue<br>Rounds ${redRounds.Value} - ${blueRounds.Value}"

mod point(mine: *int, theirs: *int, rounds: *int) {
  mine += 1
  if mine >= 5 {
    rounds += 1
    mine = 0
    theirs = 0
  }
}

on redHit { point(red, blue, redRounds) }
on blueHit { point(blue, red, blueRounds) }
```

- `mine: *int` reads "a variable holding an int". The caller passes a
  variable, and every use of `mine` inside the mod is that variable.
- `point(red, blue, redRounds)` scores for red: `mine` is `red`, `theirs` is
  `blue`. The blue call passes the same two variables the other way round.
- The first to 5 points wins the round, and both scores go back to 0.
- `<br>` in the sign's text starts a new line.

Red has 4 points and blue has 3. Red scores. What does the sign show?

## Run

1. Compile the chip and paste it in-game. Place two **Target**s, one for
   each team, and a brick with a **Text Decal**.
2. Wire the red Target's **Just Hit** to `redHit`, the blue Target's **Just
   Hit** to `blueHit`, and `sign` to the Text Decal's **Text**.
3. Shoot the targets. Each hit adds a point to its team. The fifth point
   wins the round and clears both scores.

## Under the Hood

Open the chip. There is no gate named `point`. As with every mod, each call
pasted the body in: two copies of seven gates, one copy under each handler.

Follow the red copy. Its **Increment Variable** has a **Var Ref** input,
wired from the `red` **Var**. That wire is what `mine` became. The copy's two
**Set Variable** gates are wired to `red` and `blue`. In the blue copy the
same gates are wired the other way round. A `*` parameter costs no gate. It
only decides where the mod's gates are wired.

Without the `*`, `mine` is a copy of a number, and a number cannot be
changed:

```wirescript name=r2-no-star expect=WS007
@left in redHit: exec
var red: int = 0

mod point(mine: int) {
  mine += 1
}

on redHit { point(red) }
```

```text
Error: [WS007] 'mine' isn't a writable target
```

The other way round, a `*` parameter needs a variable to point at. A number
or a sum is not one:

```wirescript name=r2-not-a-var expect=WS008
@left in redHit: exec
var red: int = 0

mod point(mine: *int) {
  mine += 1
}

on redHit { point(red + 1) }
```

```text
Error: [WS008] cannot pass a non-lvalue to ref parameter 'mine': `*T`/`ref` needs a variable or ref parameter, not a literal, expression, `let`, `arr[i]`, or call
```

## Modify

1. Make the target score a parameter. Add `to: int = 5` to `point` and
   compare `mine >= to`. Play a short match with `point(red, blue,
   redRounds, 3)` and `point(blue, red, blueRounds, 3)`.
2. Add a button that makes the teams swap ends, and swaps their scores with
   them:

   ```wirescript nocompile
   mod swap(a: *int, b: *int) {
     let before = a
     a = b
     b = before
   }
   ```

   Call `swap(red, blue)` and `swap(redRounds, blueRounds)` from
   `on swapEnds { ... }`, with `@left in swapEnds: exec` wired from a
   **Button**'s **Held**. `let before = a` reads `a` before the next line
   overwrites it.
3. Keep a log of red's score after every hit. Arrays are already passed as themselves, with
   no `*`. A mod with a plain `int[]` parameter pushes into the caller's
   array.

   ```wirescript name=r2-log
   @left in redHit: exec
   var red: int = 0
   var log: int[]
   var logged: int = 0
   @right out sign = "${logged.Value} hits logged"

   mod note(list: int[], score: int) {
     list.push(score)
   }

   on redHit {
     red += 1
     note(log, red)
     logged = log.length()
   }
   ```

## Make

Make a shop counter. Each player area has a **Button** that earns a coin and a
**Button** that buys a lamp for 3 coins. Write one
`mod buy(coins: *int, lamps: *int, cost: int)` that takes the cost from
`coins` and adds a lamp only if there are enough coins. Use it for two
players' buttons, and show both players' coins and lamps on a sign.

```wiring
button1 Held -> chip earn1
button2 Held -> chip buy1
button3 Held -> chip earn2
button4 Held -> chip buy2
chip sign -> decal Text
```

## Good to Know

- A `*` parameter adds no gates, but the mod's body is still copied at every
  call. Three calls of `point` are 21 gates.
- The writes happen in order, one step after another, so a later line inside
  the mod sees an earlier line's change: `mine >= 5` reads the score after
  the `+= 1`.
- Two handlers that write the same variable on the same tick both run, in no
  set order. If both targets are hit at once, either team's point can land
  first.

See [Reference Parameters](../../chips.md#reference-parameters-ref-t--t) and
[Ref and Deref](../../expressions.md#ref-and-deref) in the reference.

Next: [Ref Values](ref-values.md).
