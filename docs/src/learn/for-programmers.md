# Wirescript for Programmers

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Tools](tools.md)

</div>
<!-- /learn-links -->

If you already program, most of Wirescript reads like a small typed language.
This page lists what works differently, then gives a short test for every
lesson so you can skip the ones you already understand.

<!-- toc -->
## Contents

- [What is different](#what-is-different)
- [Test out](#test-out)
- [Where to go next](#where-to-go-next)
<!-- /toc -->

## What is different

- **The program is a circuit.** A compiled file is one microchip of gates
  and wires, and every gate evaluates every tick, 60 times a second. There is
  no `main` and no instruction pointer: `@right out lit = a && b` is an AND
  gate that is always computing.
- **Two contexts, pure and exec.** Pure expressions are wiring: they follow
  their inputs. Exec code lives in handlers and runs as a chain of steps, in
  order. Writes, array reads, `Random` and `for` are steps, so they only
  exist in a handler. See [Pure and Exec](pure-and-exec.md).
- **`let` is a wire, `var` is storage.** A `let` names a gate's output and
  costs nothing. A `var` is a Var gate that only handlers write. Every `var`
  survives a world save and load. Nothing resets unless you build a reset.
- **Two ways to read a `var`.** `count.Value` is a pure wire holding the value
  as of the start of this tick. Bare `count` inside a handler reads it at
  that step, so it sees a write made earlier in the same chain.
- **Handlers fire on edges.** A pin that only starts handlers is declared
  `exec` (`@left in pressed: exec`). Wire a bool output such as a Button's
  **Held** into it and `on pressed { ... }` runs once when Held turns on.
  Holding a button is one press. `on !pressed` runs when it turns off. It
  reads the value, so declare `pressed: bool` for that. The game starts
  handlers too: `on Clock(interval = 0.5)`,
  `on ZoneEntered(zone = room) -> (character)`.
- **Handlers have no order between them.** Two `on` roots that fire in the
  same tick, a Clock included, run in no fixed order. Order exists only
  inside one chain.
- **No short-circuiting.** Both sides of `&&` and `||` are wires into one
  gate, so both are always computed. In a handler, a call with side effects
  on the right still runs when the left already decides: guard it with a
  nested `if`.
- **Time is explicit.** `await Sleep(_, delay = 1.0)` pauses a chain, and
  `await hit` waits for a pulse (one that came earlier is dropped).
  `buffer before = x` is `x` one tick ago, and a loop of pure wires needs
  one.
- **No loops in pure code.** `for i in 0..n { }` is a step. It compiles to a
  loop plane, a chip of its own that runs every iteration within the current
  tick, about 100,000 iterations per tick at most. `break` and `continue`
  work as usual, but a body cannot `return` or `await`. Work spread over many
  ticks is a `buffer emit` and an `await`.
- **`mod` inlines, `fn` calls.** A mod's body is copied into every call: 50
  gates called from 10 places is 500 gates. A `fn` is one callable plane
  with a Function Call gate per call. It reads a pin or a `let` only as a
  parameter (`WS113`) and cannot return a record or an enum (`WS114`).
- **Failures are silent numbers.** Divide or remainder by zero gives 0. An
  unwired input reads 0, false or empty. A read past the end of an array
  gives that gate's previous value, not 0: check `.OutOfBounds`, and `.Found`
  for a map key.
- **Players are keyed by user id.** A death makes a new character, so
  per-player state lives in a `Map<string, T>` keyed by `GetUserId()`.
- **Some gates need admin.** Broadcasts, chat commands, player join, leave,
  spawn and death events, and environment writes only load for an admin.
  The rest of the chip loads without them.

This chip uses most of the list:

```wirescript name=t0p-tour
@left in pressed: exec
@left in reset: exec
var count: int = 0
var squares: int[]
var total: int = 0
var big: int = 0
buffer before: int = count.Value
@right out sign = "count ${count.Value}, a tick ago ${before}, sum ${total.Value}, ${big.Value} over 10"

fn square(n: int) -> int {
  return n * n
}

on pressed {
  count += 1
  squares.push(square(count))
  total = squares.sum()
  var n: int = 0
  for s in squares {
    if s > 10 {
      n += 1
    }
  }
  big = n
  await Sleep(_, delay = 1.0)
  count += 1
}

on reset {
  count = 0
  total = 0
  big = 0
  squares.clear()
}
```

`squares.sum()` is one gate. A `for` runs its body once per element, so keep
it for work no single array method does, like the count here. An array read
in pure code is an error, not a wire:

```wirescript name=t0p-pure-read expect=WS007
var squares: int[]
@left in i: int
@right out pick = squares[i]
```

## Test out

Answer the questions for a lesson without looking. If you get them right,
skip the lesson. Open the answers to check.

### Basics

1. [Tools](tools.md): why does `@left in on: bool` fail to compile?
2. [First Circuit](first-circuit.md): how many gates does
   `@right out b = a` make?
3. [Gates and Wires](gates-and-wires.md): `let both = a && b`, then two
   outputs `both` and `!both`. How many gates?
4. [Pure and Exec](pure-and-exec.md): what does `on pressed` fire on, level
   or edge? How many times while the button is held?
5. [Remembering Values](state.md): in `on pressed { count += 1; last = count }`, does
   `last` get the old count or the new one?
6. [Reacting to the Game](reacting-to-the-game.md): run a handler every half
   second, and greet the player who walks into a zone.
7. [Is It Working?](is-it-working.md): name three reasons a circuit can do
   nothing with no error.

<details><summary>Answers</summary>

1. `on` is a keyword (`WSP001`). Pick another name.
2. One: the rerouter that puts `b` on the chip's right side. Without `@right`
   it makes none, just a wire from `a` to `b`.
3. Two, an AND and a NOT. A `let` adds no gate.
4. The rising edge: once per press, however long it is held.
5. The new one. A bare name in a handler reads the variable at that step.
6. `on Clock(interval = 0.5) { ... }`, and
   `on ZoneEntered(zone = room) -> (character) { ... }` with a `zone` pin.
7. An unwired pin, a wire to the wrong port, a stale array read, a chain
   parked at an `await` that never resumes.

</details>

### Branches

- **State.** [Reading a Variable](state/reading-a-variable.md): in a handler,
  right after `count += 1`, what does `count.Value` read?
  [Buffers](state/buffers.md): build a set/reset latch from pure wires.
  [Variables](state/variables.md), [Resetting](state/resetting.md): what
  does a `var` hold after the world is saved and loaded?
- **Values.** [Numbers](values/numbers.md): what is `7 / 0`?
  [Lists](values/lists.md): what does `xs[5]` give on a
  three-element array? [Maps](values/maps.md): how do you tell a missing key
  from one that holds 0? [Converting Values](values/converting-values.md):
  what type does `round(2.6)` return?
- **Logic.** [Choosing a Value](logic/choosing-a-value.md): what does a pure
  `if c then x else y` compile to, and does a divide by zero in the arm not
  taken do harm? [Toggles and Latches](logic/toggles-and-latches.md): a
  button that turns a light on and off.
- **Strings.** [Text Values](strings/text-values.md): is `"0"` true or false?
  [Safe Text](strings/safe-text.md): why escape a display name?
- **Templates.** [Interpolation](templates/interpolation.md): what does
  `"${true}"` print? [Word Forms](templates/word-forms.md): print a bool as
  "Yes" or "No".
- **Events.** [Custom Events](events/custom-events.md): what does the
  receiver get for a constant argument? [Waiting](events/waiting.md): what
  happens to a pulse that arrives before its `await`?
- **Abstraction.** [Mods](abstraction/mods.md) and
  [Functions](abstraction/functions.md): when is a `fn` smaller than a
  `mod`? [Imports](abstraction/imports.md): what happens to a library's `on`
  handlers under `import { name } from "lib"`?
- **Data Shapes.** [Records in Storage](data/records-in-storage.md): how many
  gates does a `var` holding `{ x: int, y: int }` cost?
- **Enums.** [Enums from Pins](enums/enums-from-pins.md): what does a `match`
  give for an int that names no variant?
  [Option and Result](enums/option-and-result.md): read the value out of an
  `Option`.
- **Loops.** [Counting Loops](loops/counting-loops.md): how long does a
  1,000-iteration `for` take? [Loops Across Ticks](loops/loops-across-ticks.md):
  a back-edge loop sits inside an `if`. Where must its `let x: exec` signal
  be declared?
- **Random.** [Dice](random/dice.md): can `Random(1, 6)` give 6? How do you
  use one roll in two places?
- **Input.** [Seat Controls](input/seat-controls.md): does
  `on input.Forward > 0.5` run every tick the key is held?
  [Remote Control](input/remote-control.md): how does a circuit read the
  keys of a player who is not sitting anywhere?
- **References.** [Per-Player State](references/per-player-state.md): why
  not key a score map by character? [Ref Values](references/ref-values.md):
  what happens when a handler reads through an empty ref?
- **Prefabs.** [Lifecycle and Storage](prefabs/lifecycle.md): do a spawned
  copy and its spawner share variables? [Talking to a
  Copy](prefabs/talking-to-a-copy.md): can a copy's handlers run before the
  spawner's first event arrives?
- **Admin.** [Admin Gates](admin/admin-gates.md): what happens when a player
  without admin rights pastes a chip with one admin gate in a handler?

<details><summary>Answers</summary>

- **State.** The old value: `.Value` is as of the start of the tick, so read
  bare `count`. A latch: `let state = set || (held && !reset)` with
  `buffer held: bool = state`. After a load, the value it had: build a reset.
- **Values.** 0. The value that read gate gave last time, so check
  `.OutOfBounds`. A `float`. Add `as int`.
- **Logic.** A Select gate. Both arms compute, and the divide gives 0, so no
  harm. `on pressed { lit = !lit }`.
- **Strings.** True: only `""` is false. A name can contain `<`, which starts
  a tag and restyles your text.
- **Templates.** `1`. `Fmt("{A:Yes=No}", flag)`.
- **Events.** It is not wired, so it can arrive as 0 or empty: send a `var`.
  It is dropped, not saved.
- **Abstraction.** When it is called from several places and its body costs
  more than calling it. A one-line body is cheaper as a `mod`. They are left
  out (`WS014`): use `import "lib"` to keep them.
- **Data Shapes.** Two: one Var gate per field.
- **Enums.** The default: 0, or empty text. Keep the number in range with `%`.
  `if let Some(v) = r { ... }`, or `let Some(v) = r else { return }`.
- **Loops.** One tick. Inside that same `if`. Declared outside it, the loop
  runs once and stops, and the compiler warns `WS118` at the `await`.
- **Random.** Yes, both ends are included. Roll once into a `let` or a `var`.
- **Input.** No, once when it turns on. Wire a Switch's **Character** into
  `InputReader`: the Switch keeps the last player who flipped it, and the
  splitter follows their keys live. `GetInputs` reads keys only at the step
  that runs it, so keep it for one-time reads, not for watching keys.
- **References.** A respawn makes a new character, so the score starts over.
  The chain stops there, silently.
- **Prefabs.** No: each copy has its own storage, and values travel by
  events. Yes: guard on a value the first event sets.
- **Admin.** The rest of the chip loads. That handler's chain stops at the
  missing gate.

</details>

## Where to go next

- [Syntax](../syntax.md), [Types](../types.md) and
  [Statements](../statements.md): the language in full.
- [Execution Context](../exec-context.md): pure and exec in detail.
- [Best Practices](../best-practices.md): gate counts and how programs scale.
- [Parts](parts/README.md): ready-made circuits to paste and read.
- The [Learn](README.md) tree, for any lesson you did not test out of.
