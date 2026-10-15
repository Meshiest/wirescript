# Option and Result

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Payloads](payloads.md), [Mods](../abstraction/mods.md)

**Next steps:** [Enums from Pins](enums-from-pins.md)

</div>
<!-- /learn-links -->

Some questions have no answer. No number is 7 divided by 0.
Two enums come built in for this. `Option` is either `Some(value)` or
`None`, and `Result` is either `Ok(value)` or `Err(reason)`. A mod that might
fail returns one, and the caller has to look before using the value.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Result](#result)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button1 Held -> chip addA
button2 Held -> chip addB
button3 Held -> chip go
chip text -> decal1 Text
chip lastGood -> decal2 Text
```

## Code

```wirescript name=en3-safe-div
mod safeDiv(a: int, b: int) -> Option<int> {
  if b == 0 {
    return None
  }
  return Some(a / b)
}

@left in addA: exec
@left in addB: exec
@left in go: exec
var a: int = 0
var b: int = 0
var shown: string = "Press to divide"
var kept: int = 0
@right out text = shown.Value
@right out lastGood = "Last answer: ${kept.Value}"

on addA { a += 1 }
on addB { b += 1 }

on go {
  let r = safeDiv(a, b)
  shown = match r {
    Some(q) => "${a} / ${b} = ${q}",
    None => "${a} / 0 has no answer",
  }
  if let Some(q) = r {
    kept = q
  }
}
```

- `a` and `b` count the presses of two buttons, and `go` divides them.
- `-> Option<int>` says the mod gives back an `Option` that holds an `int`
  when there is one.
- `Some`, `None`, `Ok` and `Err` are written bare, with no `Option.` in
  front.
- The `match` must handle both, so the caller cannot reach the number
  without saying what happens when there is none.
- `if let Some(q) = r { ... }` runs its block only when `r` is `Some`, with
  the number named `q`. It is a `match` with one arm that matters.

Press the first button eight times and the second twice, then press `go`.
What do the two signs say? What do they say if you press `go` before ever
touching the second button?

## Run

1. Compile the chip and paste it in-game. Place three **Button**s and two
   bricks with a **Text Decal** each.
2. Wire the three Buttons' **Held** to `addA`, `addB` and `go`, and `text`
   and `lastGood` to the two Text Decals' **Text**.
3. Press `go` before touching the second button. The sign says "0 / 0 has
   no answer", and the last answer stays 0. Press the first button eight
   times, the second twice, and `go`. The sign says "8 / 2 = 4", and the last
   answer becomes 4.

## Under the Hood

Open the chip. The mod's answer is held in two **Var** gates, `__ret.__disc`
and `__ret.__Some_0`. An `Option` is an enum like any other, with a number
for the variant (`Some` is 0, `None` is 1) and a slot for the value. Each
`return` writes them, and the two paths meet at a **Union** before the
`match` reads them.

The **Divide** is a pure gate, so it works out `a / b` all the time, even
when `b` is 0. Only the `Some` path stores its answer.

Call a mod that returns an enum inside a handler, as here. Its `return`s
are steps that write those Var gates, and a handler is where steps run.

A `fn`, a function compiled once and shared by every caller, cannot return
an enum at all:

```wirescript name=en3-fn-enum expect=WS114
fn safeDiv(a: int, b: int) -> Option<int> {
  if b == 0 {
    return None
  }
  return Some(a / b)
}
```

```text
Error: [WS114] `fn safeDiv` returns a record, tuple, enum or exec; return its values instead. A `fn` compiles once to one plane; use a `mod` for this
```

## Result

`Option` says only that there is no answer. `Result` also says why:

```wirescript name=en3-result
mod safeDiv(a: int, b: int) -> Result<int, string> {
  if b == 0 {
    return Err("cannot divide by zero")
  }
  return Ok(a / b)
}

@left in addA: exec
@left in addB: exec
@left in go: exec
var a: int = 0
var b: int = 0
var shown: string = "Press to divide"
@right out text = shown.Value

on addA { a += 1 }
on addB { b += 1 }

on go {
  shown = match safeDiv(a, b) {
    Ok(q) => "${a} / ${b} = ${q}",
    Err(why) => why,
  }
}
```

`Result<int, string>` holds an `int` when it works and a `string` when it
does not. Here the `match` reads the call directly, with no `let`.

## Modify

1. Use `let ... else` instead of `if let`: `let Some(q) = r else { return }`
   leaves the handler when there is no answer, and the lines after it can use
   `q` freely.
2. Add a third counted button `c` and divide twice: `a / b`, then that answer by `c`.
   Show "no answer" if either division fails.
3. In the `Result` version, also refuse a `b` above 10, with its own `Err`
   message.

## Make

Make a vending machine. Each press of one button puts in a coin, and a
second button buys. A mod `buy(coins: int, price: int) -> Result<int,
string>` gives back the change, or the reason the sale failed. Show the
answer on a sign.

```wiring
button1 Held -> chip coin
button2 Held -> chip buy
chip text -> decal Text
```

## Good to Know

- `b` starts at 0, so it is 0 until its button is pressed. That is why the
  first `go` has no answer.
- `kept` is a **Var** gate, so the last good answer survives a failed division,
  and a save and load.

See [Built-in `Option` and `Result`](../../enums.md#built-in-option-and-result)
and [`if let` / `let else`](../../enums.md#if-let--let-else).

Next: [Enums from Pins](enums-from-pins.md).
