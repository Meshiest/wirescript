# Execution Context

Understanding exec vs pure context is fundamental to writing correct Wirescript. This distinction reflects how Brickadia's wire graph engine actually executes: some gates define continuous signal relationships (pure), while others execute imperatively in response to events (exec).

## Two Contexts

### Pure Context

Pure context is the default. Code in pure context defines **continuous signal-flow relationships** -- like wiring gates together. Pure expressions re-evaluate whenever their inputs change.

What runs in pure context:
- Top-level `let` bindings
- `var` initializers
- `out` bindings
- `buffer` expressions
- Chip output expressions

```wirescript
// All of these are pure context:
var count: int = 0                              // initializer is pure
let doubled = count.Value * 2                   // let binding is pure
out result = doubled                            // out binding is pure
buffer prev = count.Value                       // buffer expression is pure
```

### Exec Context

Exec context represents **imperative, sequential execution** triggered by an event. Code in exec context runs once per trigger, in order, along an exec chain.

What runs in exec context:
- `on` handler bodies
- Code after `on` handlers at the same scope level (exec union)
- Named chip bodies when the chip has `ref` parameters
- Exec function calls (`DisplayText`, `Random`, `SetLocation`, etc.)

```wirescript
// This is exec context:
on RoundStart {
  count = count + 1          // assignment requires exec
  let r = Random(0, 10)      // Random is an exec call
  if r == 0 {                // if statement requires exec
    emit special             // emit requires exec
  }
  DisplayText(ctrl, "Go!")   // DisplayText is an exec call
}
```

## What Requires Exec Context

The following operations are **only valid in exec context**:

| Operation | Why |
|-----------|-----|
| `var = expr` (assignment) | Writing to a variable gate requires an exec chain |
| `if { ... }` (statement) | Conditional execution requires an exec branch gate |
| `emit event` | Firing an event requires an exec signal |
| Exec function calls | Functions like `Random`, `DisplayText`, `SetLocation` have exec pins |
| `*var` (explicit deref) | Reads the variable's current-tick value via `Var_Get`; not allowed in pure context (WS006) |

The typechecker emits error **WS007** when any of these appear outside exec context:

```wirescript
var n: int = 0
n = 1              // ERROR: WS007 -- var write 'n' outside an exec context
```

## How to Enter Exec Context

### 1. Handler Bodies

The primary way to enter exec context is through an `on` handler:

```wirescript
on RoundStart {
  // Everything in here is exec context
  count = 0
  score = 0
}
```

### 2. Exec Union After Handlers

After a handler at the same scope level, subsequent statements automatically enter exec context. This is called "exec union" -- the combined exit of all preceding handlers flows into later statements.

```wirescript
var count: int = 0

on RoundStart {
  count = 0
}

on CharacterDied(c) {
  count = count + 1
}

// This code runs in exec context -- after EITHER handler fires,
// the exec chain continues here:
if count > 10 {
  // ...
}
```

This models how the wire graph works: the exec output of each handler merges into a union that feeds into subsequent exec nodes.

### 3. Chips with Ref Parameters

When a named chip has any `ref T` parameters, its body runs in exec context automatically. This is because reading/writing variable references requires exec:

```wirescript
chip Increment(n: ref int) {
  // Body is automatically exec context because 'n' is ref
  n = n + 1
}
```

### 4. Explicit Exec Argument

Exec functions called outside a handler can receive an explicit `exec` named argument:

```wirescript
// No enclosing handler, but providing exec explicitly
let r = Random(0, 10, exec = myTrigger)
```

This wires `myTrigger` as the exec input of the Random gate.

The same convention applies to user-defined exec chips and mods: outside an
exec context, pass their trigger as `exec = ...`. The call then also returns
the completion exec as an `exec` field on the result record, so callers can
`await r.exec` or `on r.exec { }`. See
[Exec Chips](chips.md#exec-chips).

## Reading Variables: Exec vs Pure

The behavior of a bare variable name depends on context:

### In Exec Context

A `var x: int` has type `ref int` internally, but in exec context, the bare name `x` **auto-dereferences** to type `int` when used in expressions. When passed to a `*T` parameter (e.g., `inc(x)` where `inc` takes `*int`), it stays as a reference:

```wirescript
on RoundStart {
  // x auto-derefs: reads as int, writes as int
  x = x + 1
  let doubled = x * 2   // x is int here
}
```

> **Note:** Even pure expressions like `x * 2` use a `Var_Get` gate (exec) to read `x` when inside an exec context. This ensures the read is sequenced correctly in the exec chain — the `Var_Get` fires at the right point and its value output feeds the pure `*` gate. In pure context, `x` reads directly from the PseudoVar's `Value` port (no exec gate).

You can also use `*x` as an **explicit deref** — it compiles to the same `Var_Get` gate and is equivalent to the bare name in exec context:

```wirescript
on tick {
  let a = x    // implicit deref via Var_Get
  let b = *x   // explicit deref — identical result
}
```

`*x` in pure context is an error (**WS006**): `use .Value for pure reads`.

### In Pure Context

In pure context, the bare name `x` refers to the **variable reference** (`ref int`), not the value. To read the value, use `.Value` or `.prev`:

```wirescript
var count: int = 0

// Pure context:
out current = count.Value    // .Value reads the current int value
out previous = count.prev    // .prev reads the previous tick's value

// This would be an error (WS006) if the context expected int:
// out bad = count + 1       // count is ref int, not int, in pure context
```

### `.Value` vs `.prev` vs `*var`

| Access | Context | Gate | Meaning |
|--------|---------|------|---------|
| `x` (bare) | Exec | `Var_Get` | Current tick's value (auto-deref) |
| `*x` | Exec | `Var_Get` | Current tick's value (explicit deref, same as bare) |
| `*x` | Pure | — | **Error WS006** — use `.Value` |
| `x` (bare) | Pure | — | Variable reference (`ref T`) |
| `x.Value` | Either | `.Value` port | Previous tick's value (delayed read) |
| `x.prev` | Either | `.Value` port | Previous tick's value (same as `.Value`) |

`.prev` is essential for change detection:

```wirescript
// Detect when count changes
chip let changed = count != count.prev

on changed {
  DisplayText(ctrl, "Count changed!", fontSize = 24)
}
```

## Handler Exec Chains

Handlers create exec chains -- sequences of exec gates connected by their exec output pins. Each statement in a handler body is a link in the chain:

```wirescript
on RoundStart {
  count = 0            // Exec node 1
  score = 0            // Exec node 2 (chained after 1)
  let r = Random(0,5)  // Exec node 3 (chained after 2)
  if r == 0 {          // Exec branch node (chained after 3)
    bonus = 100        //   Then-branch exec
  }
  // Exec continues after the if
}
```

The wire graph executes these in sequence: event fires, then node 1, then node 2, then node 3, then the branch.

## Exec Context in Conditional Expressions vs Statements

There are two different `if` constructs with different context requirements:

### `if-then-else` Expression (Pure)

The `if-then-else` expression is pure -- it selects between two values based on a condition and produces a value. No exec context needed.

```wirescript
// Pure -- works anywhere
let abs = if x < 0 then -x else x
let label = if count == 1 then "item" else "items"
```

### `if { }` Statement (Exec)

The `if` statement conditionally executes a block. It requires exec context.

```wirescript
// Exec -- must be inside a handler or after one
on trigger {
  if score > highScore {
    highScore = score
  }
}
```

## Summary of Context Rules

| Construct | Context | Notes |
|-----------|---------|-------|
| `var x = expr` initializer | Pure | Initializer is pure even though var is mutable |
| `let x = expr` | Pure (at top level) | Inside handlers: shares handler's exec context |
| `out x = expr` | Pure | Always pure -- outputs are continuous signals |
| `buffer x = expr` | Pure | Always pure |
| `on trigger { ... }` body | Exec | Primary way to enter exec |
| After `on` at same level | Exec | Exec union from all preceding handlers |
| `chip { ... }` body (anon) | Inherits parent | Shares parent's context |
| `chip Name(ref params) { ... }` | Exec | Auto-exec from ref params |
| `chip Name(value params) { ... }` | Pure (by default) | Unless handlers inside create exec |
| `mod Name(params) { ... }` | Inherits call site | Inlined, takes caller's context |
| `await expr` | Exec | Rewires exec continuation to expr |
| `emit name` | Exec | Bare emit requires exec |
| `emit name = expr` | Either | Value emit works in pure or exec |
| `let name: exec` | Either | Declares local exec signal |

## Await

`await` suspends the current exec chain and resumes when the awaited expression fires. It rewires `ctx.current_exec` -- no state machine, just exec redirection.

```wirescript
on start {
  doSetup()
  await ready              // pause until 'ready' fires
  doMain()                 // resumes here
}
```

### Capture Values

```wirescript
on start {
  let pos = await entity.GetLocation() on moveComplete
  use(pos)
}
```

`let x = await val on trigger` sets exec continuation to `trigger` and binds `val` to `x`.

### Race (first trigger wins)

```wirescript
on start {
  await timeout || userInput || cancel
}
```

`||` in the expression creates a Union gate -- first exec wins.

### Local Exec Signals

`let name: exec` declares a local synchronization point. `emit name` fires it from any handler. `await name` or `on name` listens for it — the listener runs whenever the signal fires, regardless of source order or which handler emits it.

```wirescript
let done: exec

on compute { emit done }
on start { await done }
```

An `on name` handler is the fan-out form — its body runs every time the signal
fires, from any emitter (handy for menu-style actions):

```wirescript
let up: exec

on tick { if doubleTapped() { emit up } }
on up { ctrl.DisplayText("menu up") }   // runs on every `emit up`
```

### Sleep / SleepTicks

`Sleep` and `SleepTicks` are buffer gates that delay a value. Combined with `await` and `_`, they create timed delays:

```wirescript
on start {
  await Sleep(_, delay = 2.0)        // wait 2 seconds
  doAfterDelay()
}

on start {
  await SleepTicks(_, delay = 120)   // wait 120 ticks (~2s at 60Hz)
  doAfterDelay()
}
```

`_` inside an `await` expression is the armed flag -- a `bool` that becomes `true` when the await is armed. The buffer gate delays this bool, and the await resumes when the delayed output transitions to true.

### Important Notes

- `await` only works in exec context -- it modifies the exec chain.
- `await` works inside `if` blocks naturally.
- Each `await` creates an armed flag (static bool) that guards the continuation. The continuation fires exactly once per arming.
- `_` inside `await` is typed as `bool` (the armed flag). It is only valid inside `await` expressions.
- `let foo = await 1` is valid but dangerous: the pure value pulses once, so the continuation runs immediately and never again.
- Multiple sequential awaits chain: each one gets its own armed flag and redirects exec.
