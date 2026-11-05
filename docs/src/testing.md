# Testing

A Wirescript program can check itself. The pattern below compiles to a circuit
that runs its own assertions when the grid loads, says nothing when everything
passes, and prints what it needs to diagnose the failure when something does
not.

Type checking cannot do this job. It proves a program is well formed, not that
a gate computes what you expected, and most unexpected behavior comes from the gates.

<!-- toc -->
## Contents

- [The shape](#the-shape)
- [Compare two paths, not one path against a constant](#compare-two-paths-not-one-path-against-a-constant)
- [Include a control](#include-a-control)
- [What an in-game test cannot prove](#what-an-in-game-test-cannot-prove)
- [Assertions](#assertions)
- [Running tests in the simulator](#running-tests-in-the-simulator)
<!-- /toc -->

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

on Loaded() {
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
repeating them. Five parts of the example matter.

**`on Loaded()` as the trigger.** The Load Event fires once when the chip is
placed, and again whenever the world loads with it, so the test runs by
pasting it in.

**Module-level counters.** `pass` and `total` live at module scope, so `assert`
updates them in place and each call is a bare `assert(want, got, label)`. The
summary line at the end is the whole result. `pass = pass + ok` relies on `bool`
coercing to `int`, so there is no `if ok then 1 else 0`.

**One generic `assert`.** `<T: int | float | string>` lets the same mod check
ints, floats, and strings. The compiler monomorphizes each call to its argument
type. `==` compares any variant, but the failure line stringifies the values
with `${...}`. Only those three types support that, so the bound names them.
Widen it to plain `<T>` for a check that never prints the value.

**The failure line prints the values.** A wrong value and a value that never arrived look
identical when all you print is a label. `want 1 got 0` tells them apart, and
`assert` gives you that on every check without writing the interpolation by hand.

**Silence on success.** Only failures print. A run that says
`array checks: 12/12` and nothing else is a pass you can read at a glance.
Twelve lines of `OK: ...` would have to be scanned. The checks print with
`BroadcastChatMessage` rather than `PrintToConsole`, so the output lands in chat
where you are already looking.

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

on Loaded() {
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
on Loaded() {
  var xs: int[]
  xs.push(12345)

  assert(12345, xs[0], "control literal survives the round trip")

  BroadcastChatMessage("checks: ${pass}/${total}")
}
```

`12345` cannot be confused with a default, an empty read, or an off-by-one.

## What an in-game test cannot prove

Running the circuit proves the values are right. It cannot prove how they were
produced, which is what a test of a compiler feature needs to show. A compile-time evaluation that fell back to emitting gates
without any diagnostic still computes the correct answer and still passes every
check above.

So the in-game program is one half. The other half is a compiler-side test that
asserts the structure: that the gate count is what it should be, that a
particular gate class is absent, that a value was baked into a component rather
than wired. Together they cover both questions, and neither covers both alone.

## Assertions

The `testing` statements check a program as it runs. Each one is a statement
in an exec context (an `on` handler, or a `mod` or `fn` body), says nothing
when its check holds, and otherwise prints a chat line that starts with
`FAIL` and names the file and line it is written on:

```wirescript
var xs: int[]

on Loaded() {
  xs.push(10)
  testing.assertEq(xs.length(), 1)
  testing.expect(xs[0]).toBeGreaterThan(5)
  testing.expect(xs).not.toContain(3)
}
```

| Statement | Fails when | Prints |
|---|---|---|
| `testing.fail(args...)` | always | `FAIL <file>:<line>: <args>` |
| `testing.log(args...)` | never | `LOG <file>:<line>: <args>` |
| `testing.assert(cond, msg...)` | `cond` is false | `FAIL ...: <msg>`, or `assert failed: <cond>` with no message |
| `testing.assertEq(a, b, msg...)` | `a != b` | `FAIL ...: <a> expected <b's value>, got <a's value> (<msg>)` |
| `testing.expect(a).toBe(b)` | `a != b` | as `assertEq` |
| `.toEqual(b)` | a field of record `a` differs from the same field of `b`, nested records included; any other value as `toBe` | one line per differing field, with both values |
| `.toBeGreaterThan(b)` / `.toBeLessThan(b)` | `a > b` / `a < b` does not hold | both values |
| `.toBeCloseTo(b, digits = 2)` | `abs(a - b) >= 10^-digits / 2` | both values |
| `.toContain(b)` | string `a` does not contain `b` (case counts), or array `a` has no element equal to `b` | `b`, and the string |

`.not` before any matcher reverses it: `testing.expect(a).not.toBe(b)` fails
when `a == b`. `<file>` is the file's name without its folders, `<line>`
counts from 1, `<a>` is the operand as written, and the arguments of `fail`,
`log` and a message are joined by spaces. Each operand is evaluated once.
Two records compare only with `toEqual`, since `==` on records is WS004.

**Only a test build runs them.** `wirescript test`, `wirescript sim`, the
editor's Run test, the playground's simulator and `wirescript compile --tests`
make a test build, where each statement becomes the check it describes;
`compile --tests` gives an in-game self-checking save. Every other build
type-checks the statements and compiles them to nothing, so their arguments are
not evaluated there: an argument with a side effect, such as `xs.pop()`, has
it only in a test build.

A file that declares `testing` at its top level, or imports something under
that name, gets its own `testing` instead. A misused statement is
[WS126](diagnostics.md).

## Running tests in the simulator

`wirescript test` runs tests without the game, in a gate simulator that
schedules gates the way the game was measured to (`just test-ws <paths>`).
It gives every file or directory it is handed two kinds of check.

**`@test`.** Each [`@test` fn, mod or chip](statements.md#tests-test) runs on
its own: the program is compiled as if it held `on Loaded() { name() }`
followed by a closing chat line, and simulated for ten seconds of ticks. The
test fails when it prints a chat line starting with `FAIL`, which a failed
[assertion](#assertions) does, when the simulation stops with an error, or when
it never reaches its end (the closing line) within those ticks. A test that
passes says nothing, as the in-game pattern above does. A `@test mod` can read module state and `await`; a
`@test fn` is isolated and synchronous.

```wirescript
fn clampTo(n: int, lo: int, hi: int) -> int {
  if n < lo { return lo }
  if n > hi { return hi }
  return n
}

@test fn clamps_both_ends() {
  testing.assertEq(clampTo(-5, 0, 10), 0)
  testing.assertEq(clampTo(50, 0, 10), 10)
  testing.expect(clampTo(5, 0, 10)).toBe(5)
}
```

In VS Code and in the playground, a **Run test** lens above each `@test` runs
it the same way and shows its verdict and its chat lines. The playground adds
**Run all tests** at the top of a file that has tests, and marks the line of a
test that failed.

**Expected output.** A program with a `.expected` file beside it
(`arrays.ws` and `arrays.expected`) runs whole, and passes when its chat lines
are exactly the file's lines. An in-game self-check like the ones above, with
its summary line as the expected file, runs the same way.

A `.script` file beside a program drives what the simulator cannot invent:
players joining, chat commands, what a sweep hits. One action per line, after
the tick it happens on:

```text
5 join alice            # alice joins (ControllerJoined)
12 chat alice /roll 3   # alice types /roll 3
0 sweep 120             # the next sweep hits 120 units away
0 player alice name Sam # alice's display name (user name: `user`, id: `id`)
8 move alice 10 0 -20   # alice's character moves there (tick 0: where it starts)
0 team Red alice bob    # a team and its members (no teams unless declared)
0 player alice role Mod # also `permission <name>`, `trusted yes`, `damagelimit 50`
3 input alice Forward 1 # held until set again (axes, and C E Q LeftMouse RightMouse)
6 damage alice 30 bob   # bob hits alice; also `spawn`, `die`, `fire`
9 answer alice yes      # alice answers her open box: `ok`, `no`, `cancel`, `text <words>`
```

The other lines are `aim`, `slot`, `zone`, `teleport`, `jointeam`,
`leaveteam`, `roundend`, `interact` and `target`; a mistyped one stops the run
with an error naming its line.

A player the script does not name is called `Wirescript` and gets a user id
made from their script name, the same on every run. What a program shows one
player prints among the chat with the player's script name in front:
`[alice text 1] Score: 5` when `DisplayText` changes text 1, and
`[alice chat] hi` for `ShowChatMessage`. Other effects print the same way:
`[status] ...`, `[audio] ...`, `[console] ...` for `PrintToConsole`.

To try a program by hand, `wirescript sim file.ws --interactive` runs the Load
Event and then reads commands: any script line without its tick (`join alice`,
`input alice Forward 1`) happens on the next tick, `step 10` runs ten ticks,
`screen alice` prints what alice's screen shows, and `quit` ends it.

The simulator runs logic exactly: values, storage, exec chains, buffers,
timers, events, functions and loops. A setting or property it was never told
reads as `0` (white for a color), and a world gate it does not model (physics,
brick components such as buttons and zones) stops the run with an error naming the source line,
rather than guessing. Where the game leaves an order open (two handlers fired
by one change), `--seed` picks one; a test whose result changes with the seed
depends on an order the game does not promise.
