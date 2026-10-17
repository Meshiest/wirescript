# Ref Values

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Ref Parameters](ref-parameters.md)

**Next steps:** [Per-Player State](per-player-state.md)

</div>
<!-- /learn-links -->

A `*int` parameter points a mod at a variable when you write the call. A ref
can also be a value: kept in a variable, and pointed somewhere else while the
game runs. This lesson keeps three counters and a button that chooses which
one the other button adds to.

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
button1 Held -> chip next
button2 Held -> chip add
chip sign -> decal Text
```

## Code

```wirescript name=r3-picker
@left in next: exec
@left in add: exec
var a: int = 0
var b: int = 0
var c: int = 0
var picked: *int
var name: string = "none"
@right out sign = "A ${a.Value}  B ${b.Value}  C ${c.Value}<br>Picked: ${name.Value}"

on next {
  if name == "A" {
    picked = &b
    name = "B"
  } else if name == "B" {
    picked = &c
    name = "C"
  } else {
    picked = &a
    name = "A"
  }
}

on add {
  *picked += 1
}
```

- `var picked: *int` is a variable that holds a ref to an `int` variable.
  It starts empty.
- `&b` is `b`'s ref, as a value. `picked = &b` stores it.
- `*picked` is the variable `picked` points at. `*picked += 1` adds one to
  whichever of `a`, `b` or `c` that is.
- `next` steps `picked` from A to B to C and round again. `name` keeps the
  letter for the sign.

The `add` handler never mentions `a`, `b` or `c`. Press `next` twice, then
`add` three times. Which counter goes up? What happens if you press `add`
before you have ever pressed `next`?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire one Button's **Held** to `next`, the other's **Held** to `add`, and
   `sign` to the Text Decal's **Text**.
3. Press `add` first: nothing happens, since nothing is picked yet. Press
   `next`: A is picked. Press `add`: A goes up. Press `next` and `add`: B
   goes up.

## Under the Hood

Open the chip. `picked` is a **Var** like the others, but what it holds is a
ref to another Var. The three **Set Variable** gates that write `picked` each
take their **Value** from a different counter's **Var Ref** output. That wire
is `&a`, `&b` or `&c`.

The `add` chain is two gates. A **Get Variable** reads `picked`, and its
**Value** output is wired into the **Var Ref** input of an **Increment
Variable**. In [Ref Parameters](ref-parameters.md) the Var Ref input was wired
straight from one counter when you compiled. Here it comes from a variable,
so which counter it reaches is decided while the game runs.

A ref is not the number it points at. Leave out the `*` and the compiler
stops you:

```wirescript name=r3-no-deref expect=WS004
@left in add: exec
var a: int = 0
var picked: *int

on add {
  picked = &a
  picked += 1
}
```

```text
Error: [WS004] operator '+' cannot be applied to a reference (*int, int): refs and bricks only compare with `==` and `!=`
```

A ref can start out pointing somewhere. The `= &a` is baked into the save,
so `picked` already holds `a`'s ref when the program is pasted, with no gate
to set it:

```wirescript name=r3-start
var a: int = 0
var picked: *int = &a
```

The initializer has to name a var, array or map declared above it. A ref that
is only known while the game runs, such as one read from another var, is
still dropped with a `WSP001` warning.

## Modify

1. Add a third button that clears the picked counter: `*picked = 0`. One
   handler clears whichever counter is picked.
2. Keep the refs in a list instead of an `if` chain. `(*int)[]` is an array
   of refs, filled the first time `next` runs:

   ```wirescript name=r3-lanes
   @left in next: exec
   @left in add: exec
   var a: int = 0
   var b: int = 0
   var c: int = 0
   var lanes: (*int)[]
   var lane: int = -1
   var picked: *int
   @right out sign = "A ${a.Value}  B ${b.Value}  C ${c.Value}<br>Picked: ${lane.Value + 1}"

   on next {
     if lanes.length() == 0 {
       lanes.push(&a)
       lanes.push(&b)
       lanes.push(&c)
     }
     lane = (lane + 1) % 3
     picked = lanes[lane]
   }

   on add {
     *picked += 1
   }
   ```

   Add a fourth counter `d`. Only the list and the `% 3` change.
3. A ref value can go to a `*int` parameter. The mod then works on the
   variable the ref points at. Add this mod and call it with `picked`:

   ```wirescript nocompile
   mod bump(count: *int, by: int) {
     count += by
   }

   on add {
     bump(picked, 5)
   }
   ```

## Make

Make a switchboard. Three **Point Light**s are each driven by a `bool`
variable. One button picks the next light, and another flips the picked one
on or off (`*sel = !*sel`). A sign says which light is picked.

```wiring
button1 Held -> chip next
button2 Held -> chip flip
chip lamp1 -> light1 Enabled
chip lamp2 -> light2 Enabled
chip lamp3 -> light3 Enabled
chip sign -> decal Text
```

## Good to Know

- An empty ref stops the chain. Reading or writing through `picked` before
  anything was stored in it ends the handler at that line, with no
  message.
- Refs compare only with refs. `picked == &a` asks whether `picked` points at
  `a`. Comparing a ref with a number is an error (`WS084`), because the game
  treats `1 == <ref>` as true.
- Shown as text, a ref prints as `Var`, not as the value it points at. Read
  through it with `*` first.
- Arrays and maps are already passed as themselves: an `int[]` mod
  parameter needs no `*`, as in [Ref Parameters](ref-parameters.md).

See [Reference values](../../types.md#reference-values) and
[Ref and Deref](../../expressions.md#ref-and-deref) in the reference.

Next: [Per-Player State](per-player-state.md).
