# Statements

Statements are the building blocks of Wirescript programs. They declare data, define behavior, and control execution flow.

## `var` -- Mutable Variable

Declares a mutable variable backed by a wire graph variable gate. In exec context (inside handlers or mods), the variable is **reset to its initial value** each time the code path executes.

```wirescript
var name: type = initializer
var name: type              // default-initialized
var name = initializer      // type inferred from annotation or usage
```

The type annotation and initializer are both optional (but at least one should be provided for the typechecker to determine the type).

```wirescript
var count: int = 0       // resets to 0 each handler invocation
var score: float = 0.0
var alive: bool = true
var label: string = "hi" // strings persist in vars too
var dir: vector = Vec(0.0, 0.0, 1.0)
```

A variable is backed by a wire-graph `Variable` gate, whose value is a wire
variant, so a `var` can hold any variant member type: `int`, `float`, `bool`,
`string`, `vector`, and object types (`entity`, `controller`, `character`,
`brick`, `prefab`).

### `static var` -- Persistent Variable

A `static var` keeps its value across handler/mod invocations. The initial value is set once when the save loads. Use this for accumulators, counters, or state that must survive across calls.

```wirescript
static var total: int = 0     // persists across calls
static var highScore: int = 0

on RoundStart {
  total = total + 1           // accumulates over time
}
```

Top-level (module-scope) `var` declarations are always persistent — `static` is only meaningful inside handlers and mods, so **`static var` at top/root level is a no-op** (just use `var`).

### Variable Identity

Internally, a `var x: T` has type `ref T`. This means:

- **In exec context** (inside `on` handlers): `x` auto-dereferences to type `T` when used in expressions. When passed to a `*T` parameter, it remains a reference.
- **In pure context** (outside handlers): `x` refers to the variable reference itself (`ref T`). Use `x.Value` or `x.prev` to read the value.

```wirescript
var count: int = 0

// Pure context -- use .Value for the current value
out currentCount = count.Value

// Exec context -- direct access auto-derefs
on RoundStart {
  count = count + 1    // reads and writes the int value directly
}
```

See [Execution Context](exec-context.md) for full details.

## `let` -- Immutable Binding

Binds a name to a computed value. Unlike `var`, a `let` binding is not mutable storage -- it is a pure wire connection to an expression's output.

```wirescript
let name = expression
let name: type = expression
```

```wirescript
let doubled = count * 2
let isAlive = hp > 0
let greeting = "Hello, " .. playerName
```

An optional type annotation can follow the name. The annotation does not change the binding's type -- it is a checked assertion. If the expression's inferred type does not match, the compiler emits a **WS016** warning.

```wirescript
let x: int = 42           // ok — types match
let y: float = 42         // ok — int coerces to float
let z: string = 42        // WS016 warning — int does not match string
```

`let` bindings can appear at the top level, inside blocks, and inside chip bodies. They are evaluated in pure context.

```wirescript
// Top-level let
let maxScore = 100

// Let inside a handler (evaluated in the exec context of the handler)
on RoundStart {
  let r = Random(0, 15)
  if r == 0 { count = count + 1 }
}
```

### Record Destructuring

Destructure a record into individual bindings with `let { field1, field2 } = record`:

```wirescript
type Point = { x: int, y: int }
let p: Point = { x: 10, y: 20 }
let { x, y } = p
let sum = x + y  // 30
```

Each destructured name becomes an independent `let` binding that resolves to the same underlying value as the original record field.

### Tuple Destructuring

Destructure a tuple into named bindings:

```wirescript
let (first, second) = someTuple
```

A rest pattern captures remaining elements:

```wirescript
let (head, ...rest) = longTuple
```

### Spread in Call Arguments

Spread a tuple or record into a function's positional arguments:

```wirescript
let args = (1, 2, 3)
foo(...args)  // equivalent to foo(1, 2, 3)
```

## `buffer` -- Buffered Value

Declares a value that is delayed by one tick. Buffers are useful for creating feedback loops where a value depends on its own previous state without creating a circular dependency.

```wirescript
buffer name = expression
buffer name: type = expression
```

```wirescript
buffer prevScore = score
buffer delayed: int = count
```

The optional type annotation is useful when the expression type needs clarification (e.g., for self-referential buffers).

## `array` -- Array Declaration

Declares an array that holds multiple values of the same element type.

```wirescript
array name: elementType[]
```

```wirescript
array scores: int[]
array positions: vector[]
array names: string[]
array flags: bool[]
```

The type annotation must end with `[]` to indicate it is an array type. The
element type selects the backing array variant (`int` -> Int64 array, `float` ->
double array, `bool`, `string`, `vector`, and object types each map to their
matching array kind), so elements keep their declared type rather than all
being stored as doubles.

An array can be given **constant initial contents** with an `= [ ... ]`
initializer. At the top level (outside an exec handler) the contents are baked
straight into the array gate, so **every element must be a literal** (numbers —
including negatives — strings, and bools). The array loads pre-populated with no
runtime setup:

```wirescript
array scores: int[] = [100, 50, -10]
array names: string[] = ["alice", "bob"]
```

Initializers may span multiple lines — newlines are allowed after `[`, around
commas, and before `]`, with an optional trailing comma:

```wirescript
array names: string[] = [
  "alice",
  "bob",
]
```

A non-literal element at the top level (an identifier, a call, or a `...spread`)
is an error — there is no exec context in which to populate it. Build the array
from runtime values inside a handler instead (see below).

### Array-typed `var` and inferred element type

A `var` whose value is an array literal is also an array — it desugars to the
same gate as `array`. The element type is taken from the annotation, or inferred
from the literal when there's no annotation:

```wirescript
var queue: int[] = [1, 2, 3]   // annotated
var queue = [1, 2, 3]          // element type inferred -> int[]
```

### Building an array at runtime (assignment + spread)

Inside an exec handler you can assign an array literal to an array variable. It
desugars to **clear -> push each item -> append each spread**, so the elements may
be any runtime value, and a `...spread` splices another array's contents in
place:

```wirescript
array base: int[] = [3, 4]
var work: int[]

on tick {
  let n = score + 1
  work = [n, 1, ...base, 5]   // clear, push n, push 1, append base, push 5
                              // -> [n, 1, 3, 4, 5]
}
```

The assignment always clears first, so it replaces (not appends to) the previous
contents. Spreads are only valid here, not in a top-level initializer.

Access elements with bracket notation:

```wirescript
let item = scores[i]
// item.value: int (the element)
// item.bOutOfBounds: bool (bounds check)
```

## `in` -- Input Port

Declares an input port for the current scope. At the top level, `in` creates an external input that other wire graphs can connect to. Inside a chip, `in` creates a chip input port.

```wirescript
in name: type
```

```wirescript
in trigger: exec
in player: character
in speed: float
in enabled: bool
```

Input values are read-only within the script. They are provided by the external wire graph environment.

## `out` -- Output Port

Declares an output port that exposes a value externally.

### Value outputs

The value form is a pure expression -- continuously computed from its inputs.

```wirescript
out name = expression
```

```wirescript
out score = count
out isAlive = hp > 0
out doubled = value * 2
out greeting = "Score: ${count}"
```

### Typed value outputs

An output port can have both a type annotation and a value expression. The annotation is a checked assertion (like on `let`):

```wirescript
out name: type = expression
```

```wirescript
out score: int = count.Value     // type asserted + value
out ratio: float = hits / total
out ref: *int = myVar            // ref output — exposes the variable reference
```

This form is required when you want to expose a variable reference (`*T`) rather than its computed value, or to disambiguate the type when the compiler would otherwise warn.

### Exec outputs

The typed form without a value declares an exec output port. Use `emit` inside a handler to connect the current exec chain to it.

```wirescript
out done: exec

on RoundStart {
  count = count + 1
  emit done  // fires the 'done' output after incrementing
}
```

This is useful for chips that need to signal completion:

```wirescript
chip Counter(bump: exec) -> (value: int, done: exec) {
  var n: int = 0
  on bump {
    n = n + 1
    emit done
  }
  out value = n.Value
}
```

Value output bindings are evaluated in pure context. Exec outputs are wired via `emit` in exec context.

### WS017 -- Ambiguous variable output type

When `out foo = someVar` is used and `someVar` has no explicit type annotation, the compiler emits **WS017** because it cannot determine whether you want the variable's value or a reference to it:

```
warning WS017: output type inferred from untyped variable
  suggest: `out foo: T = var` for value or `out foo: *T = var` for ref
```

Fix by adding a type annotation:

```wirescript
out foo: int = myVar      // exposes the value (uses .Value)
out foo: *int = myVar     // exposes the variable reference
```

## `@left` / `@right` / `@top` / `@bottom` -- Outer Rerouter Pins

Annotating a top-level `in` or `out` with a side places a physical Rerouter
brick on the outside of the compiled microchip, pre-wired to that port.
Placed chips can then be wired up like an IC: wire **into** an input pin's
rerouter, and **from** an output pin's rerouter.

```wirescript
@left in go: exec          // same line
@left
out done: exec             // or on the line directly above
@right out score = 1
@top in players: int
```

Rules:

- Valid sides are exactly `left`, `right`, `top`, `bottom`; one annotation
  per declaration.
- Only **top-level** `in`/`out` of the compiled file may be annotated.
  Inside `chip {}` or `mod` bodies the annotation is an error (WS023).
- Unannotated ports get no rerouter — the feature is fully opt-in.

Placement:

- Rerouters sit flush against the chosen side of the chip brick,
  bottom-aligned with it, spaced 2 grid units apart and starting from the
  top corner (left/right sides) or left corner (top/bottom sides) of the edge.
- Ports on the same side appear in **declaration order**, with `in` and
  `out` freely interleaved. Left/right sides run top to bottom; top/bottom
  sides run left to right.
- Each rerouter is coloured by its port's value type and carries a floating
  label with the port's name; a side's input and output labels read opposite
  ways so the two are easy to tell apart.

```
                @top ports (left to right)
                ┌──[d]────────────┐
   @left ports  │                 │  @right ports
(top to bottom) │                 │  (top to bottom)
        [a] ────┤    microchip    ├──── [c]
        [b] ────┤                 │
                └─────────────────┘
                @bottom ports (left to right)
```

### `@label` -- Port Display Label

`@label("text")` overrides the floating display label on a port's gate
(and its rerouter pin label, if the port also has a side annotation). The
port's wiring-UI name always stays the declared identifier -- `@label`
only changes what's shown floating in the world.

Unlike `@left`/`@right`/`@top`/`@bottom`, which are top-level only,
`@label` works on `in`/`out` declarations at **any** nesting level, and it
stacks with a side annotation in either order:

```wirescript
@left @label("Fire!") in trigger: exec
@label("Fire!") @left in trigger: exec   // order doesn't matter
```

### `@nofold` -- Suppress Constant Folding

- `@nofold` — suppress constant folding/elision for everything lowered from this
  declaration (`let`/`out`/`var`/`chip`/`on`, including captured events
  `let e = on trigger { … }` and await bindings); legal at any nesting depth.
  Placed at the very top of the file (after any module doc comment) and
  separated from the first declaration by a blank line, it applies to the
  whole module — the same blank-line rule as module doc comments. Sites where
  it can have no effect (anonymous chips, `in` declarations) emit a warning.
  Used by semantics-verification circuits that need real gates for known
  values.
- Two module-level gotchas: leave a **blank line between a module doc block and
  a module-level `@nofold`** (a directly-adjacent pair registers as neither),
  and a module-level `@nofold` applies only to the file compiled as the
  **entry** — an imported library's own module-level `@nofold` does not carry
  into the importer (annotate the individual declarations instead).
- A module-level `@nofold` disables the entire constant-fold pass for that
  compile — the same effect as `--no-fold` on the CLI.
- `@nofold` also preserves literal-condition `if` branches. Normally an `if`
  whose condition is a literal `true`/`false` has its dead side stripped
  during lowering as a shortcut, ahead of the fold pass proper — but under
  `@nofold` (including a module-level one) that shortcut is suppressed too,
  so both branches stay real gates. See [Constant Folding](folding.md) for
  the full pass.

### `@fold` -- Opt Into Constant Folding

- The fold pass is currently opt-in: `@fold` at the very top of the **entry**
  file (after any module doc comment), separated from the first declaration
  by a blank line, enables the whole pass for that compile — the same
  blank-line rule as a module-level `@nofold` above, and module-level only
  (there's no decl-scoped `@fold`).
- Entry-file-only, same as module-level `@nofold`: an `@fold` at the top of
  an *imported* file does not carry into the importer and has no effect.
- If both a module-level `@fold` and `@nofold` are present, `@nofold` wins
  and the parser warns that the two conflict. A module-level `@nofold` also
  overrides `--fold` on the CLI.
- Two module-level gotchas: leave a **blank line between a module doc block and
  a module-level `@fold`** (a directly-adjacent pair registers as neither,
  and now produces a module-level-only error), and a module-level `@fold`
  applies only to the file compiled as the **entry** — an imported library's
  own module-level `@fold` does not carry into the importer.
- `--fold` on the CLI has the same effect as a module-level `@fold`, without
  editing the source; `--no-fold` overrides either. See
  [Constant Folding](folding.md) for the full enable/disable story.

## `if` -- Conditional Statement

The `if` statement executes a block conditionally. It **requires exec context** -- you can only use `if` statements inside `on` handlers or after handlers in the exec chain.

```wirescript
if condition {
  // then branch
}

if condition {
  // then branch
} else {
  // else branch
}
```

```wirescript
on RoundStart {
  if score > highScore {
    highScore = score
  }

  if lives == 0 {
    gameOver = true
  } else {
    lives = lives - 1
  }
}
```

For pure conditional values, use the `if-then-else` **expression** instead:

```wirescript
// Expression (pure, no exec needed)
let clamped = if x > max then max else x

// Statement (exec required)
on trigger {
  if x > max { x = max }
}
```

## `on` -- Event Handler

Handlers run code in response to events or triggers. The handler body executes in exec context.

```wirescript
on trigger {
  // body (exec context)
}
```

### Triggering on Built-in Events

```wirescript
on RoundStart {
  score = 0
}

on CharacterDied(character) {
  lives = lives - 1
}
```

Event parameters are bound by listing names in parentheses after the event name. The number and types of parameters are determined by the event (see [Built-in Events](#built-in-events) below).

Some events also accept **config args** that configure the event gate itself. String literals (and `Name = value` named args) set the gate's config fields, while bare identifiers still bind data outputs. `ChatCommand` uses this for its command name and help text:

```wirescript
on ChatCommand("greet", "Greets the player", player, args) {
  // "greet" -> command name, "Greets the player" -> help text
  // player -> controller output, args -> arguments output
  player.DisplayText("Hello ${args}")
}

// the help text can also be named, and bindings are optional:
on ChatCommand("wave", Description = "Wave at everyone") { }
```

The zone events — `ZoneEntered`, `ZoneLeft`, `EntityZoneEntered`, `EntityZoneLeft`, `ProjectileZoneEntered`, `ProjectileZoneLeft`, `BrickChanged`, `BrickRemoved` — accept a `zone = <value>` named arg that **wires** its value into the gate's `Zone` input port (rather than setting a static config field). Pass an `in` port bound to a zone brick so one wire selects the zone the gate watches — and the same port can drive several of these gates:

```wirescript
in room: entity                             // wire to a Zone brick in-game
on ZoneEntered(character, zone = room) { }  // room feeds the gate's Zone input
on ZoneLeft(character, zone = room) { }
```

> **Frozen entities still fire entity zone events** — `SetFrozen(true)` does not
> suppress `EntityZoneEntered`. But an entry only fires on a **boundary crossing**:
> `SetLocation`-ing an entity to a zone it is *already* inside does **not** re-fire
> the event. To force a fresh entry, move it out of the zone and back in.

### Triggering on Input Ports

```wirescript
in trigger: exec

on trigger {
  count = count + 1
}
```

### Triggering on Boolean/Pulsing Values

Any `bool`, `int`, `float`, or `vector` value can trigger a handler when its value changes:

```wirescript
chip let moved = position != position.prev

on moved {
  // Fires whenever the 'moved' signal transitions
}
```

### Triggering on Let Bindings and Buffers

```wirescript
let signal = someExpression

on signal {
  // Fires when signal changes
}
```

### Triggering on Chip Result Execs

A chip call result's exec fields work as triggers — including the `exec`
completion field returned by a call with an `exec = ...` trigger (see
[Exec Chips](chips.md#exec-chips)):

```wirescript
let r = InitTables(exec = reset)

on r.exec {
  // Fires after the chip body ran
}
```

### Negated Triggers

Prefix a trigger with `!` to trigger on the negation (falling edge for booleans):

```wirescript
on !running {
  // Fires when 'running' becomes false
}
```

### Union Triggers

Multiple triggers can be combined with `|` to fire the handler on any of them:

```wirescript
on eventA | eventB {
  // Fires on either event
}
```

### Field Triggers

Trigger on a field of an object using dot notation:

```wirescript
on obj.field {
  // Fires when obj.field changes
}
```

## `let on` -- Event Declaration

Event declarations create named triggers using `let ... = on ...`. The `event` keyword is also accepted as a legacy alias.

### Event Alias

Creates a new name for an existing event or trigger:

```wirescript
let died = on CharacterDied
```

The alias can then be used as a trigger:

```wirescript
on died(character) {
  // ...
}
```

### Captured Event

Wraps a trigger with a handler body that defines the event's behavior:

```wirescript
let bumped = on Bumped {
  // This body executes when Bumped fires
  // 'bumped' becomes a trigger in its own right
}
```

## `emit` -- Emit Signal

Fires an exec signal to an output port or local exec signal. Bare `emit` requires exec context; `emit target = expr` also works in pure context.

```wirescript
emit eventName              // bare exec signal (exec context only)
emit target = expr          // set value + fire exec (any context)
```

```wirescript
out scored: exec

on CharacterDied(c) {
  score = score + 1
  emit scored
}

on scored {
  DisplayText(ctrl, "Score!", fontSize = 24)
}
```

### Value Emit

`emit target = expr` wires `expr` to the output's value input and fires the exec signal:

```wirescript
out result: int

on trigger {
  emit result = computed_value    // exec context: wires value + routes exec
}

emit status = some_expr           // pure context: continuous wire
```

### Local Exec Signals

`let name: exec` declares a local synchronization point that can be targeted by `emit` and used with `await` or `on`:

```wirescript
let ready: exec

on compute { emit ready }      // fires the signal
on start { await ready }        // continues when ready fires
```

### Buffered Emit

`buffer emit sig` routes the emit's exec through a **Buffer** gate, delaying delivery by one tick. This is the tick-crossing barrier that makes emit/await **loops** legal: a back-edge `emit` after an `await` closes a wire-graph cycle, and every cycle must cross a Buffer or the compile errors (**WS005**).

```wirescript
buffer emit loop            // 1 tick (default)
buffer(3) emit loop         // 3 ticks (BufferTicks)
buffer(0.5s) emit loop      // 0.5 seconds (BufferSeconds)
buffer(d) emit loop         // variable delay — wired into TicksToWait
buffer(0, 1s) emit sig      // delay 0, hold output 1s after the input drops
```

- The first duration is the **delay** (`TicksToWait` / `SecondsToWait`); the optional second is the **hold** (`ZeroTicksToWait` / `ZeroSecondsToWait` — how long the output stays up after the input drops; omitted = `-1` = same as delay).
- An `s` suffix selects the seconds gate; unadorned durations are ticks.
- Constant durations bake into the gate; variables/expressions wire into the duration port.

### Payload Ferrying

`emit sig = value` on a **local** exec signal ferries the value with the signal: each emitted value is written into a hidden per-signal store var on the emit chain, and `await sig` reads it back on the resumed chain — so the value survives the buffered tick crossing.

```wirescript
let loop: exec

emit loop = 0                        // scalar payload
let index = await loop               // read it back

emit loop = { sum: 0, index: 0 }     // record payload: one store per field
let { sum, index } = await loop      // destructure the fields
```

### Loops

`emit`/`await` on a local signal plus a buffered back-edge forms a loop that advances one iteration per buffer period. Loop state can live in `var`s (they persist across iterations; non-static vars reset on the entry chain, once per call):

```wirescript
mod sumItems(arr: int[]) -> int {
  var sum = 0
  var index = 0
  let loop: exec
  emit loop
  await loop
  if index < arr.length() {
    sum += arr[index]
    index += 1
    buffer emit loop        // back-edge: crosses 1 tick, re-arms the await
  } else {
    return sum
  }
}
```

or ride the signal as a ferried payload (no mutable vars):

```wirescript
mod sumItems(arr: int[]) -> int {
  let loop: exec
  emit loop = { sum: 0, index: 0 }
  let { sum, index } = await loop
  if index < arr.length() {
    buffer(1) emit loop = { sum: sum + arr[index], index: index + 1 }
  } else {
    return sum
  }
}
```

Semantics worth knowing:

- An emit on the **same exec chain** as an unconditional `await` of that signal is sequenced through a `Var_Set(armed = true)` **before** entering the signal's union — so the awaiting `Var_Get` can never race the arm, and a loop back-edge re-arms the await every iteration.
- Emits from **other handlers** enter the signal directly and are guarded by the armed flag: the continuation only runs if the awaiting chain has reached the `await`.
- An `await` inside an `if` branch keeps pure flag semantics (its arm only fires when the branch is taken).

### Gate Cost

| Construct | Gates added |
|-----------|-------------|
| `emit sig` (bare) | 0 — joins the signal's union |
| `buffer(...) emit sig` | 1 Buffer (Ticks/Seconds) |
| `emit sig = scalar` | 1 Var_Set per emit (+1 hidden store var per signal) |
| `emit sig = { F fields }` | F Var_Set per emit (+F store vars per signal) |
| `await sig` (per await) | ~5: armed-flag var, arm + reset Var_Set, Var_Get, Branch |
| `let { F fields } = await sig` | +F Var_Get |
| per signal | 1 Union hub (+1 arm Var_Set when same-chain emits exist); a single-input hub is spliced away |

## `await` -- Suspend Exec Chain

Suspends the current exec chain and resumes from the awaited expression's exec output. Everything after the `await` runs when that exec fires. Only valid in exec context.

```wirescript
await signal                         // resume when signal fires
let val = await signal               // capture the signal's ferried payload
let { a, b } = await signal          // destructure a record payload
let val = await value on trigger     // capture value when trigger fires
await a || b                         // race -- first signal wins
await Sleep(_, delay = 1.0)          // sleep 1 second using _ armed flag
await SleepTicks(_, delay = 5)       // sleep 5 ticks
```

Each `await` creates an armed flag (`static var bool`) that guards the continuation. The continuation only fires once per arming, preventing repeated triggers.

### The `_` Placeholder

Inside an `await` expression, `_` refers to the await's armed flag -- a `bool` that becomes `true` when the exec chain reaches the await point. Use `_` with `Sleep`/`SleepTicks` to wire the armed flag as the buffer gate's input:

```wirescript
on start {
  doSetup()
  await SleepTicks(_, delay = 60)  // _ = armed flag, delayed 60 ticks (~1s)
  doAfterDelay()                    // runs after the delay
}
```

### Sleep / SleepTicks

`Sleep(input, delay, hold)` and `SleepTicks(input, delay, hold)` are buffer gates that delay a value passing through.

| Function | Gate | Delay unit | Params |
|----------|------|-----------|--------|
| `Sleep` | BufferSeconds | seconds (float) | `input`, `delay`, `hold` |
| `SleepTicks` | BufferTicks | ticks (int) | `input`, `delay`, `hold` |

- `input` -- the value to delay (use `_` for the await armed flag)
- `delay` -- how long to wait before the output follows the input (optional)
- `hold` -- how long to hold the output after the input drops to zero (optional, set to -1 to use delay instead)

### Examples

```wirescript
in start: exec
in done: exec
var count: int = 0

on start {
  count = 1
  await done          // exec chain pauses here
  count = 2           // runs when 'done' fires
}

// Capture a value when a signal fires
on start {
  let val = await score on done
  use(val)
}

// Sleep for 2 seconds
on start {
  await Sleep(_, delay = 2.0)
  count = 99
}
```

## Assignment

Assigns a new value to a mutable variable. Requires exec context.

```wirescript
target = expression
```

```wirescript
on RoundStart {
  count = count + 1
  score = 0
  name = "Player " .. playerId
}
```

Only `var` declarations are valid assignment targets. Attempting to assign to a `let` binding, parameter, or other non-variable produces a type error.

Indexed assignment works for arrays:

```wirescript
on trigger {
  scores[i] = newScore
}
```

### Compound Assignment

Compound assignment operators combine an operation with assignment:

| Operator | Equivalent | Gate Used |
|----------|-----------|-----------|
| `+=` | `x = x + expr` | `IncVar` |
| `-=` | `x = x - expr` | `Var_Get` + Sub + `Var_Set` |
| `*=` | `x = x * expr` | `Var_Get` + Mul + `Var_Set` |
| `/=` | `x = x / expr` | `Var_Get` + Div + `Var_Set` |
| `%=` | `x = x % expr` | `Var_Get` + Mod + `Var_Set` |
| `&=` | `x = x & expr` | `Var_Get` + AND + `Var_Set` |
| `\|=` | `x = x \| expr` | `Var_Get` + OR + `Var_Set` |
| `^=` | `x = x ^ expr` | `Var_Get` + XOR + `Var_Set` |
| `<<=` | `x = x << expr` | `Var_Get` + Shl + `Var_Set` |
| `>>=` | `x = x >> expr` | `Var_Get` + Shr + `Var_Set` |

`+=` compiles to the dedicated `IncVar` gate (one gate instead of three). All others desugar to `x = x OP expr` (Var_Get + operation + Var_Set).

```wirescript
on tick {
  counter += 1       // IncVar gate
  health -= damage   // Var_Get + Sub + Var_Set
  mask &= 0xFF       // Var_Get -> BitAnd -> Var_Set
  bits <<= 1         // Var_Get -> Shift -> Var_Set
}
```

## Expression Statement

Any expression can be used as a statement. This is primarily useful for calling exec functions that have side effects:

```wirescript
on RoundStart {
  DisplayText(ctrl, "Round Started!", fontSize = 30)
  SetLocation(entity, newPos)
}
```

## Built-in Events

These events are available as handler triggers. Parameters listed can be bound using the `on Event(param)` syntax.

| Event | Parameters | Description |
|-------|-----------|-------------|
| `RoundStart` | (none) | Game round started |
| `RoundEnd` | (none) | Game round ended |
| `CharacterSpawned` | `character: character` | A character spawned |
| `CharacterDied` | `character: character` | A character died |
| `ControllerJoined` | `controller: controller`, `userId: string` | A player joined |
| `ControllerLeft` | `controller: controller`, `userId: string` | A player left (`userId` stays valid even as the controller is torn down on disconnect) |
| `ZoneEntered` | `character: character` | A character entered a zone |
| `ZoneLeft` | `character: character` | A character left a zone |
| `EntityZoneEntered` | `entity: entity` | An entity entered a zone |
| `EntityZoneLeft` | `entity: entity` | An entity left a zone |
| `ProjectileZoneEntered` | `character: character`, `projectile: entity`, `weapon: entity`, `weaponName: string` | A projectile entered a zone (`character` is the shooter) |
| `ProjectileZoneLeft` | `character: character`, `projectile: entity`, `weapon: entity`, `weaponName: string` | A projectile left a zone |
| `CharacterDamaged` | `character: character`, `damage: float`, `attacker: entity`, `attackerWeapon: entity`, `attackerWeaponName: string` | A character took damage |
| `BrickChanged` | `brick: brick` | A brick was changed in a zone |
| `BrickRemoved` | `brick: brick` | A brick was removed from a zone |
| `ChatCommand` | `controller: controller`, `arguments: string` | A registered chat command was run. Takes config args for the command name + help text — see [above](#triggering-on-built-in-events) |

## `return`

The `return` statement terminates the current exec chain early. It can be used inside:

- `on` handlers
- `chip on` handlers
- `if` blocks within handlers
- `mod` bodies (when called from exec context)

```wirescript
on RoundStart {
  if score > 100 {
    return  // skip the rest of this handler
  }
  score = score + 1
}

chip on CharacterDied(character) {
  lives = lives - 1
  if lives <= 0 {
    return  // don't process further
  }
  health = 100
}

mod process(v: *int) {
  if v < 0 { return }  // early exit from mod
  v = v * 2
}
```

### `return expr` -- Return with Value

For mods with a single output (declared with `-> (name: type)`), `return expr` sets the output value and exits:

```wirescript
mod clamp(v: int, lo: int, hi: int) -> (result: int) {
  if v < lo { return lo }
  if v > hi { return hi }
  return v
}

let clamped = clamp(x, 0, 255)  // clamped is directly an int
```

Single-output chips and mods auto-unwrap: `let f = Foo(5)` gives `f` the output type directly (e.g. `int`), no `.result` field access needed.

### How Multiple Returns Compile

A single `return expr` wires the value directly to the output port (pure, zero-tick).

When a mod has **multiple `return expr` statements**, the compiler inserts a variable to hold the return value. Each `return expr` becomes a `Var_Set` before jumping to the return union, and a `Var_Get` after the union reads the result. This means multi-return mods have a one-tick latency on the return value (the var write is visible on the next tick for pure reads, but available immediately for subsequent exec-chain reads via `Var_Get`).

`return` is not allowed in pure context (outside exec chains).
