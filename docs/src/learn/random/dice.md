# Dice

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Random Colors](random-colors.md)

**Parts that use this:** [Whack-a-Light](../parts/arcade-game.md)

</div>
<!-- /learn-links -->

`Random(1, 6)` rolls a die and gives a whole number from 1 to 6.
This lesson rolls two dice, and shows why two dice are not the same as one
roll from 2 to 12.

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
button Held -> chip roll
chip text -> decal Text
```

## Code

```wirescript name=rn1-two-dice
@left in roll: exec
var a: int = 1
var b: int = 1
@right out text = "${a.Value} + ${b.Value} = ${a.Value + b.Value}"

on roll {
  a = Random(1, 6)
  b = Random(1, 6)
}
```

- `Random(min, max)` gives a whole number from `min` to `max`, and both ends
  can come up: `Random(1, 6)` gives 1, 2, 3, 4, 5 or 6.
- Each `Random` is a separate roll. `a` and `b` get two different rolls, so
  they can match or not.
- `Random` is a step, like a variable write, so it runs in a handler. The sign
  reads `a` and `b` from their variables.

Press the button many times. Which total comes up most often? Would
`Random(2, 12)` give totals in the same way?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with
   a **Text Decal**.
2. Wire the Button's **Held** to `roll`, and `text` to the Text Decal's
   **Text**.
3. Press the button and watch the totals. 7 comes up most. 2 and 12 are
   rare.

## Under the Hood

Open the chip. Each `Random(1, 6)` is one **Random** gate, with 1 typed into
**Min** and 6 into **Max**. Its **Exec** input runs it, and it puts out a new
number on **Output** each time. The two Random gates run one after the other
in the chain, each followed by a **Set Variable**.

There are 36 ways two dice can land, and six of them add to 7 (1 + 6, 2 + 5,
and so on), but only one adds to 2. `Random(2, 12)` has no such pattern.
Each of its 11 totals is as likely as the others.

A `Random` outside a handler has nothing to run it:

```wirescript name=rn1-pure expect=WS007
@left in roll: exec
@right out face = Random(1, 6)
```

```text
Error: [WS007] exec call 'Random' outside an exec context (pass exec = ... to override)
```

Move the roll into `on roll { ... }` and keep the result in a variable, as
the lesson's code does.

## Modify

1. Flip a coin. `Random(0, 1)` gives 0 or 1. Store `Random(0, 1) == 1` in a
   `bool` variable and show `Heads` or `Tails` with
   `if heads.Value then "Heads" else "Tails"`, which gives the first text
   when `heads` is true and the second when it is false.
2. Count the sevens. Roll both dice into one `let`, and keep two counts:

   ```wirescript name=rn1-sevens
   @left in roll: exec
   var rolls: int = 0
   var sevens: int = 0
   @right out text = "${sevens.Value} sevens in ${rolls.Value} rolls"

   on roll {
     let total = Random(1, 6) + Random(1, 6)
     rolls += 1
     if total == 7 {
       sevens += 1
     }
   }
   ```

   A `let` is one roll, however often you read it, so `total` is the same number
   in the `if` as on the line above. Over many rolls, about one in six is a 7.
3. Roll a twenty-sided die, `Random(1, 20)`, and light a **Point Light** on a
   20.

## Make

Make a doubles game. Each press rolls two dice and shows them. Doubles (both
dice the same) light a Point Light. Three doubles in a row light a second
light, and any roll that is not a double resets the streak.

```wiring
button Held -> chip roll
chip text -> decal Text
chip double -> light1 Enabled
chip streak -> light2 Enabled
```

## Good to Know

- `Random(1, 6)` includes both 1 and 6. `Random(0, 6)` would give seven
  different numbers.
- Every `Random` call rolls again. To use one roll in two places, roll it once
  into a `let` or a variable and read that.
- Each roll is new. The dice do not remember earlier rolls, and a 6 is as
  likely right after a 6 as at any other time.

See [Random](../../builtins.md#random-exec) in the reference.

Next: [Random Colors](random-colors.md).
