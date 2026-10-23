# Looping Over a List

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Counting Loops](counting-loops.md)

**Next steps:** [Looping Over a Map](looping-over-a-map.md), [Ready Check](../input/ready-check.md), [Persistent Players](../admin/persistent-players.md), [Testing](../mastery/testing.md), [Rotations](../space/rotations.md)

**Parts that use this:** [Healing Zone](../parts/healing-zone.md)

</div>
<!-- /learn-links -->

A `for` loop can walk through an array, one element per iteration. `break`
can stop it as soon as it finds what it is looking for, and `continue` can
skip an element and go on to the next. This lesson keeps a list of dice rolls
and searches it.

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
button1 Held -> chip roll
button2 Held -> chip search
chip text -> decal Text
```

## Code

```wirescript name=lp2-first-six
@left in roll: exec
@left in search: exec
var rolls: int[]
var shown: string = "No rolls yet"
@right out text = shown.Value

on roll {
  rolls.push(Random(1, 6))
  shown = "${rolls.length()} rolls"
}

on search {
  var place: int = 0
  var found: int = 0
  for r in rolls {
    place += 1
    if r == 6 {
      found = place
      break
    }
  }
  if found > 0 {
    shown = "First six: roll ${found}"
  } else {
    shown = "No six in ${rolls.length()} rolls"
  }
}
```

- `for r in rolls { ... }` runs its block once for each element of `rolls`,
  first to last, with `r` holding that element. An empty array runs it zero
  times.
- `place` counts the iterations, so it says which roll is being looked at.
- `break` stops the loop at once. The handler carries on after the loop, with
  `found` holding the place of the first six.
- `found` stays 0 when no roll was a six, and the sign says so.

Say the rolls so far are 4, 6, 2 and 6. Which roll does the search report?
How many iterations run?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire the Buttons' **Held** outputs to `roll` and `search`, and `text` to the
   Text Decal's **Text**.
3. Roll a few times, then search. Also search before any six comes up. The
   sign then says how many rolls had no six.

## Under the Hood

Open the chip. The loop is a **For Each Array** microchip. Its **Array Var
Ref** input is wired from the `rolls` **Array Var**, and it runs once per
element.

Inside, the **For Each Array Entry** gate has three outputs: **Body** starts
each iteration, **Value** is `r`, and **Index** is the element's position
([Index and Value](index-and-value.md) uses it). After them come an
**Increment Variable** for `place`, an **Equal**, a **Branch**, the write to
`found`, and a **Break** gate at the end of the chain. When an iteration reaches **Break**, the loop ends and the For Each
Array's **Completed** output fires.

The loop reads the array straight from its gate, so there is no
**Get Array Element** in the chip.

`continue` skips the rest of one iteration and goes on to the next element.
This counts the sixes by skipping every roll that is not one:

```wirescript name=lp2-continue
@left in search: exec
var rolls: int[]
var sixes: int = 0

on search {
  for r in rolls {
    if r != 6 { continue }
    sixes += 1
  }
}
```

Open this chip and there is no gate for `continue`. The **Branch** for
`r != 6` has nothing wired to its true side, so on a roll that is not a six
the iteration's chain stops there, and the loop starts the next one. The
**Increment Variable** for `sixes` hangs off the false side only.

Here `if r == 6 { sixes += 1 }` would do the same. `continue` pays off when
several checks come first and the rest of the body is long: each check skips
the roll, and the body does not need to nest inside all of them.

## Modify

1. Show the total of all rolls. In the `roll` handler, after the push, add a
   loop with no `break`:

   ```wirescript name=lp2-total
   @left in roll: exec
   var rolls: int[]
   var shown: string = "No rolls yet"
   @right out text = shown.Value

   on roll {
     rolls.push(Random(1, 6))
     var total: int = 0
     for r in rolls {
       total += r
     }
     shown = "${rolls.length()} rolls, total ${total}"
   }
   ```

2. Count the sixes instead of finding the first one: drop the `break`, and add
   one to a `sixes` variable each time `r == 6`.
3. Search for any number. Add a third button that steps a `target` variable
   through 1 to 6 (`target = target % 6 + 1`), show it on the sign, and search
   for `target` instead of 6.

## Make

Make a streak finder. After every roll, search the list for two sixes in a
row, and light a **Point Light** when there are. Keep the previous element in
a variable declared before the loop, and compare it with `r`.

```wiring
button Held -> chip roll
chip streak -> light Enabled
```

## Good to Know

- A loop over an empty array runs zero times and goes straight to the code
  after it. Plan for "nothing found".
- `break` ends the whole loop, not only the current iteration. Nothing
  after the `break` in the block runs.
- `continue` ends only the current iteration. The loop goes on with the next
  element, and nothing after the `continue` in the block runs this time.
- A read like `rolls[i]` past the end of the array gives a stale value, but
  `for r in rolls` visits only the elements that are there, so it never reads
  past the end.
- When the test is plain equality, an array can answer in one gate.
  `rolls.find(6).Index` is the index of the first 6 (`-1` when there is
  none), and `rolls.sum()` is the total. A loop is for tests one gate cannot
  do, such as the streak in Make.

See [`break`](../../statements.md#break) and
[`continue`](../../statements.md#continue) in the reference.

Next: [Looping Over a Map](looping-over-a-map.md).
