# Testing

A Wirescript program can check itself. The pattern below compiles to a circuit
that runs its own assertions when the grid loads, says nothing when everything
passes, and prints what it needs to diagnose the failure when something does
not.

Type checking cannot do this job. It proves a program is well formed, not that
a gate computes what you expected, and the gates are the part with the surprises.

## The shape

```wirescript
var pass: int = 0
var total: int = 0

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

on ReadBrickGrid() {
  var xs: int[]
  xs.push(10)
  xs.push(20)

  assert(2, xs.length(), "length after two pushes")
  assert(20, xs[1], "second element")

  BroadcastChatMessage("array checks: ${pass}/${total}")
}
```

The file is [`examples/assert.ws`](../../examples/assert.ws), verified against
`just check`. The later examples reuse this `assert`, `pass`, and `total` without
repeating them. Five things are doing work.

**`on ReadBrickGrid()` as the trigger.** It fires when the grid loads, so the
test runs by pasting it in. No separate binding: the handler reads the grid
event directly.

**Module-level counters.** `pass` and `total` live at module scope, so `assert`
updates them in place and each call is a bare `assert(want, got, label)`. The
summary line at the end is the whole result. `pass = pass + ok` relies on `bool`
coercing to `int`, so there is no `if ok then 1 else 0`.

**One generic `assert`.** `<T: int | float | string>` lets the same mod check
ints, floats, and strings; the compiler monomorphizes each call to its argument
type. `==` compares any variant, but the failure line stringifies the values
with `${...}`, and only those three types support that, which is what the bound
names. Widen it to plain `<T>` for a check that never prints the value.

**It carries the value out.** A wrong value and a value that never arrived look
identical when all you print is a label. `want 1 got 0` tells them apart, and
`assert` gives you that on every check without writing the interpolation by hand.

**Silence on success.** Only failures print. A run that says
`array checks: 12/12` and nothing else is a pass you can read at a glance;
twelve lines of `OK: ...` is noise you have to scan. `BroadcastChatMessage`,
not `PrintToConsole`, so it lands in chat where you are already looking.

## Compare two paths, not one path against a constant

A check against a hardcoded expected value only proves the program agrees with
what you typed. The stronger form computes the same answer two ways and compares
those, so the test fails when the two disagree even if you were wrong about both:

<!-- doc-check-prelude:
var pass: int = 0
var total: int = 0
mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}") }
}
-->
```wirescript
mod byAddition(n: int) -> int { return n + n }

on ReadBrickGrid() {
  assert(byAddition(21), 21 * 2, "addition agrees with multiplication")

  BroadcastChatMessage("math checks: ${pass}/${total}")
}
```

This is how to test anything with two implementations: an optimized path against
an obvious one, a compile-time answer against the runtime gates, a lookup table
against the formula that generated it.

## Include a control

A test that reads `0` when a value fails to arrive will pass any check whose
expected answer is also `0`. Give at least one check a value nothing else in the
program produces, so a whole channel going dead cannot slip through:

<!-- doc-check-prelude:
var pass: int = 0
var total: int = 0
mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}") }
}
-->
```wirescript
on ReadBrickGrid() {
  var xs: int[]
  xs.push(12345)

  assert(12345, xs[0], "control literal survives the round trip")

  BroadcastChatMessage("checks: ${pass}/${total}")
}
```

`12345` cannot be confused with a default, an empty read, or an off-by-one.

## What an in-game test cannot prove

Running the circuit proves the values are right. It cannot prove how they were
produced, and for anything about compilation that is the whole question. A
compile-time evaluation that quietly fell back to emitting gates still computes
the correct answer and still passes every check above.

So the in-game program is one half. The other half is a compiler-side test that
asserts the structure: that the gate count is what it should be, that a
particular gate class is absent, that a value was baked into a component rather
than wired. Together they cover both questions, and neither covers both alone.
