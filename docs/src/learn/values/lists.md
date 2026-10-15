# Lists

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Converting Values](converting-values.md), [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Maps](maps.md), [Records](../data/records.md), [Counting Loops](../loops/counting-loops.md), [Ref Parameters](../references/ref-parameters.md)

**Parts that use this:** [Combination Lock](../parts/combination-lock.md), [Color-Cycling Light](../parts/color-cycling-light.md)

</div>
<!-- /learn-links -->

A variable holds one value. A list holds many: one value per slot, in order.
The slots are numbered from 0, so the first value is in slot 0, the second in
slot 1, and so on. In code a list is called an array. It is declared with
`var` and keeps its contents between presses.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Reading past the end](#reading-past-the-end)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button1 Held -> chip roll
button2 Held -> chip step
chip text -> decal Text
```

## Code

```wirescript name=v5-history
@left in roll: exec
@left in step: exec
var rolls: int[]
var count: int = 0
var index: int = 0
var shown: int = 0
@right out text = "${count.Value} rolls. Roll ${index.Value + 1} was ${shown.Value}"

on roll {
  rolls.push(Random(1, 6))
  count = rolls.length()
}

on step {
  index = (index + 1) % count
  shown = rolls[index]
}
```

`var rolls: int[]` is an array of `int`s. The `[]` after the type makes it a
list. It starts empty. `rolls.push(...)` adds a value to the end, and
`rolls.length()` says how many there are.

`rolls[index]` reads one element. The first is at index 0, the second at 1,
and so on. Each press of `step` moves `index` on by one, and `% count` wraps it
back to 0 after the last roll.

Array reads, pushes and lengths are steps, so they all happen inside
handlers. The sign reads from ordinary variables that the handlers fill in.

Roll three times, then press `step` four times. Which roll does the sign show?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire the Buttons' **Held** outputs to `roll` and `step`, and `text` to the
   Text Decal's **Text**.
3. Roll a few times, then step through them. The sign walks through the rolls
   in order and wraps around.

## Under the Hood

Open the chip. The array is one **Array Var** gate. The `roll` chain is a
Random, a **Push To Array** and a **Get Array Length**. The `step` chain ends
in a **Get Array Element**. Every one of them has an exec input, because the
game has no pure gate that reads an array.

So the compiler refuses an array read outside a handler:

```wirescript name=v5-pure-read expect=WS007
var rolls: int[] = [4, 2, 6]
@right out first = rolls[0]
```

```text
Error: [WS007] array index read 'rolls[...]' outside an exec context
```

`= [4, 2, 6]` makes the array start with those values. It works only for
values known when you compile.

## Reading past the end

Press `step` before any roll. There is no element at index 0 yet, but the
read does not fail. A read past the end of an array gives back the
**previous** value that gate read, not 0, so a wrong index can look like real
data.

Each read also reports `.OutOfBounds`, which is `true` when the index was not
in the list.

```wirescript name=v5-bounds
@left in roll: exec
@left in step: exec
var rolls: int[]
var index: int = 0
var shown: int = 0
var missing: bool = false
@right out text = "slot ${index.Value}: ${shown.Value} (missing: ${missing.Value})"

on roll {
  rolls.push(Random(1, 6))
}

on step {
  index += 1
  let item = rolls[index]
  shown = item
  missing = item.OutOfBounds
}
```

Roll twice, then step. Slot 1 is real. Step again: slot 2 says missing, and
still shows slot 1's roll.

## Modify

1. Show the biggest roll so far. In the `roll` handler, add
   `best = rolls.max().Value` with a new `var best: int = 0`, and show it on
   the sign. `max()` is one gate however long the list is. Like an element
   read, it gives `.Value` and a second part, `.IsEmpty`.
2. Add a third button that clears the list: `rolls.clear()`, then set `count`
   and `index` back to 0.
3. Keep only the last five rolls. After the push, drop the oldest one when
   there are too many:

   ```wirescript nocompile
   if rolls.length() > 5 {
     rolls.remove(0)
   }
   ```

   `remove(0)` takes out slot 0, and every later value moves down a slot.

## Make

Make a waiting line. One button hands out tickets numbered 1, 2, 3 and so on,
and pushes each number onto the end of a list. A second button serves the
ticket at the front: it shows that number on a sign and removes it from the
list. The sign also shows how many tickets are waiting. Pressing serve with
nobody waiting should not show an old ticket again.

```wiring
button1 Held -> chip ticket
button2 Held -> chip serve
chip text -> decal Text
```

## Good to Know

- An array starts empty unless its declaration lists constant values.
- A read past the end of an array gives the gate's previous value, not 0.
  Check `.OutOfBounds` whenever the index might be wrong.
- Every `rolls[i]` is its own gate and its own read. To use one read twice,
  bind it to a `let`, as `item` is above.
- To find a value by a name instead of a slot number, such as a score per
  player, use a map: the next lesson.

Next: [Maps](maps.md).
