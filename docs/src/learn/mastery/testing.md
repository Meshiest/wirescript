# Testing

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Mods](../abstraction/mods.md), [Looping Over a List](../loops/looping-over-a-list.md)

**Next steps:** [Const and Folding](const-and-folding.md)

</div>
<!-- /learn-links -->

A test is code that checks other code. This lesson writes checks for a rule
as `@test`s you run in the editor with one click, then builds the same
checks into a chip that runs them the moment it is pasted. Both stay quiet
about what passed and show enough about a failure to find it.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Testing in Game](#testing-in-game)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

## Code

```wirescript name=ma1-medal-tests
var hits: int[] = [0, 4, 5, 9, 10, 12345]
var wants: string[] = ["bronze", "bronze", "silver", "silver", "gold", "gold"]

mod medal(n: int) -> string {
  return if n >= 10 then "gold" else if n >= 5 then "silver" else "bronze"
}

@test mod medal_table() {
  testing.expect(hits.length()).toBe(wants.length())
  for i in 0..hits.length() {
    testing.assertEq(medal(hits[i]), wants[i], "${hits[i]} hits")
  }
}
```

- `medal` is the code under test: the medal rule from
  [Counting the Cost](../abstraction/cost.md).
- `hits` and `wants` are a table of cases: each number of hits, and the medal
  it should give. The cases sit on the edges, where a rule usually breaks: 4
  and 5, 9 and 10.
- `@test` marks `medal_table` as a test. Nothing calls it, so it adds no
  gates to the chip. A test runner calls it once, the way `on Loaded()`
  would.
- `testing.expect(a).toBe(b)` checks that `a` equals `b`, and
  `testing.assertEq(a, b, message)` does the same with a message for the
  failure. A check that holds prints nothing. One that fails prints a chat
  line starting with `FAIL`, and the test fails.
- It is a `@test mod` because it reads the file's `var`s. A `@test fn` runs on
  its own and cannot see them.

What does a run of the test say? Change `>= 5` to `> 5` in `medal`: what
does it say then?

## Run

1. Open the code in the playground. A **Run test** link sits above
   `@test mod medal_table`, and **Run all tests (1)** above the first line.
2. Click **Run test**. The playground runs the test in its gate simulator for
   ten seconds of ticks, and the simulator panel shows `PASS medal_table`.
3. In VS Code, the same **Run test** link sits above each `@test`, and the
   Testing view lists every test in the workspace.

A test fails when it prints a `FAIL` line, when the run stops with an error,
or when it does not reach its end in time, such as an `await` that never
resumes. The panel then shows `FAIL medal_table`, the chat lines the test
printed, and the reason, and the playground marks the test's line in red.

## Testing in Game

The simulator runs the gates your code compiles to, not the game itself. To
check a chip in a world, give it checks that run when it is pasted and a sign that
shows the result:

```wiring
chip report -> decal Text
```

```wirescript name=ma1-medal-test
var pass: int = 0
var total: int = 0
var fails: string = ""
var hits: int[] = [0, 4, 5, 9, 10, 12345]
var wants: string[] = ["bronze", "bronze", "silver", "silver", "gold", "gold"]
@right out report = "medal checks: ${pass.Value}/${total.Value}${fails.Value}"

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total += 1
  let ok = want == got
  pass += ok
  if !ok {
    fails = "${fails}<br>FAIL ${label}: want ${want} got ${got}"
  }
}

mod medal(n: int) -> string {
  return if n >= 10 then "gold" else if n >= 5 then "silver" else "bronze"
}

on Loaded() {
  pass = 0
  total = 0
  fails = ""
  assert(hits.length(), wants.length(), "table sizes")
  for i in 0..hits.length() {
    assert(wants[i], medal(hits[i]), "${hits[i]} hits")
  }
}
```

- `assert` counts one check, and adds a `FAIL` line when the two values
  differ. `<T: int | float | string>` lets the same mod compare ints, floats
  or text. Both values must be the same type. `pass += ok` adds 1 for a
  true `ok` and 0 for a false one.
- `on Loaded()` runs once, when the chip appears. The checks run as you
  paste it, with nothing to press.
- The handler sets the counters back to zero first, so a second run does not
  add to the first.

1. Compile the chip and paste it in-game. Place a brick with a **Text
   Decal** and wire `report` to its **Text**.
2. The checks ran when the chip appeared, before the wire was there, so the
   sign shows the result as soon as it is wired: `medal checks: 7/7`.
3. To run them again, paste the chip again.

The sign is the whole result. `7/7` with no other line means every check
passed. A failure adds a line saying what was wanted and what came out.
`want silver got bronze` tells a wrong answer apart from an answer that never
arrived, which would read `got` and nothing after it.

## Under the Hood

**Run test** compiles the file as if it ended with
`on Loaded() { medal_table() }` and a line that marks the end, then runs
that program in the gate simulator, gate by gate. A normal compile leaves the
test out: a `@test` nothing calls compiles to nothing, and so does each
`testing` line. The tests can sit beside the code they check at no cost in
game.

Open the chip from Testing in Game. The loop is a **For Loop**, and inside it is one copy of
`medal`: two **Greater/Equal** and two **Select** gates. Each pass reads a
case with **Get Array Element**, runs it through those gates and compares
the answer with an **Equal**. The `FAIL` line is a **Format String** behind
a **Branch**, so it is only written when the check fails.

The cases come from **Array Var**s on purpose. This version of the same
test types the cases into the calls instead:

```wirescript name=ma1-folded
var pass: int = 0
var total: int = 0
var fails: string = ""
@right out report = "medal checks: ${pass.Value}/${total.Value}${fails.Value}"

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total += 1
  let ok = want == got
  pass += ok
  if !ok {
    fails = "${fails}<br>FAIL ${label}: want ${want} got ${got}"
  }
}

mod medal(n: int) -> string {
  return if n >= 10 then "gold" else if n >= 5 then "silver" else "bronze"
}

on Loaded() {
  pass = 0
  total = 0
  fails = ""
  assert("bronze", medal(4), "4 hits")
  assert("silver", medal(5), "5 hits")
  assert("gold", medal(10), "10 hits")
}
```

It also shows `3/3` in game. Open it: there is no Greater/Equal, no Select
and no Equal. There are only the counters, the **Set Variable** gates that
reset them, and **Increment Variable** gates that add 1 to each. Every
input was a constant, so the compiler worked out each medal and each
comparison itself while compiling, and kept only the answers. The test still
catches a mistake in the rule, but it checks the compiler's arithmetic, not
the gates the game will run when real hits arrive. The chip has 14 gates,
and the table version has 50.

A `@test` with typed-in inputs is folded the same way. So feed test inputs
from storage: an array, a `var`, or a pin. A value read
from storage is only known when the game runs, so the compiler has to build
the gates. [`const` and Folding](const-and-folding.md) covers what the
compiler works out ahead of time, and how to tell.

## Modify

1. Break `medal` on purpose: change `n >= 5` to `n > 5`. **Run test** now
   fails, and the panel shows the check that broke:
   `FAIL <file>:11: medal(hits[i]) expected silver, got bronze (5 hits)`,
   with your file's name for `<file>`. In game, the sign
   says `medal checks: 6/7` and `FAIL 5 hits: want silver got bronze`. Make
   every new test fail once like this. A test that has never failed may not
   be checking anything.

2. Check one answer against another. A loop that adds up a list must agree
   with **Sum Array** (`scores.sum()`), which adds up the whole array in one
   gate:

   ```wirescript name=ma1-two-paths
   var scores: int[] = [3, 12345, 7, 0, 40]

   @test mod loop_agrees_with_sum() {
     var sum: int = 0
     for s in scores {
       sum += s
     }
     testing.assertEq(sum, scores.sum(), "loop agrees with sum()")
     testing.expect(scores[1]).toBe(12345)
   }
   ```

   Neither answer is typed in, so the check fails if either way is wrong,
   even if you would have typed the wrong number yourself. `12345` is a
   control: a value nothing else in the chip could produce by accident. An
   unwired or empty read gives 0, and a check that expects 0 cannot tell.

3. Test a chip from outside. A test bench is a second chip whose outputs
   drive the first chip's input pins and whose inputs read its outputs. The
   first chip below is the one under test, and the second is its bench:

   ```wirescript name=ma1-medal-chip
   @left in hits: int
   @right out medal = if hits >= 10 then "gold" else if hits >= 5 then "silver" else "bronze"
   ```

   ```wirescript name=ma1-bench bench-for=ma1-medal-chip
   @top in go: exec
   @left in medal: string
   var pass: int = 0
   var total: int = 0
   var fails: string = ""
   var sent: int = 0
   @right out hits = sent.Value
   @bottom out report = "bench: ${pass.Value}/${total.Value}${fails.Value}"

   mod assert<T: int | float | string>(want: T, got: T, label: string) {
     total += 1
     let ok = want == got
     pass += ok
     if !ok {
       fails = "${fails}<br>FAIL ${label}: want ${want} got ${got}"
     }
   }

   mod check(n: int, want: string) {
     sent = n
     await Sleep(_, delay = 0.1)
     assert(want, medal, "${n} hits")
   }

   on go {
     pass = 0
     total = 0
     fails = ""
     check(4, "bronze")
     check(5, "silver")
     check(10, "gold")
   }
   ```

   Paste both, with the bench to the left of the medal chip. Wire the
   bench's `hits` to the medal chip's `hits`, the medal chip's `medal` to the
   bench's `medal`, a **Button**'s **Held** to the bench's `go`, and the
   bench's `report` to a Text Decal. Press the button: `bench: 3/3` after
   about a third of a second. Each check sets the bench's output, waits a
   tenth of a second for the answer to come back through the wires, then
   compares. `await Sleep(_, delay = 0.1)` is that wait: the handler stops
   there and carries on 0.1 seconds later ([Waiting](../events/waiting.md)
   covers it). The bench's own controls sit on its top and bottom, so its
   left and right pins pair up with the chip under test.

## Make

Turn the search from [Looping Over a List](../loops/looping-over-a-list.md)
into a mod that takes a list of rolls and gives back the place of the first
six, or 0 for none. Test it with a `@test mod`: an empty list, a list with
no six, a six first, a six last, and two sixes. Give it a control value, and
make every check fail once before you trust it. Then check it in game with a
self-checking chip and a sign.

```wiring
chip report -> decal Text
```

## Good to Know

- `on Loaded()` runs once, when the chip appears, and again each time the
  world loads. Each pasted copy has its own counters and runs its own checks.
- A read past the end of an array gives the previous value read, not 0. If
  `wants` were shorter than `hits`, the last checks would compare stale
  values. The length check before each loop catches that.
- A test that never ran reads `0/0`. Show the total, not only the passes, so
  a test that did nothing cannot look like a pass.
- `testing` lines also work in game. A test build
  (`wirescript compile --tests`) turns each one into a real check, so a chip
  that calls `medal_table()` from `on Loaded()` posts each failure to chat
  as a `FAIL` line. Those lines use `BroadcastChatMessage`, which needs
  admin to load, so these lessons use a Text Decal.
- A `@test fn` cannot `await`, and runs apart from the file's `var`s. A
  `@test mod` can do both, so a test that waits, or checks state, is a
  `@test mod`.

See also: [Testing](../../testing.md) in the reference.

Next: [`const` and Folding](const-and-folding.md).
