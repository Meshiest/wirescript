# Chips

Chips are Wirescript's primary mechanism for organizing and reusing wire graph logic. They map to physical microchip bricks in Brickadia: a separate grid of gates contained within a single brick. (A module-level [`@flat`](#compiling-without-microchips-flat) compiles the same program onto one grid instead, with no microchip bricks at all.)

<!-- toc -->
## Contents

- [Anonymous Chips](#anonymous-chips)
- [Named Chips](#named-chips)
- [`const` Parameters and `const mod`](#const-parameters-and-const-mod)
- [Open and Closed Chips](#open-and-closed-chips)
- [Compiling Without Microchips (`@flat`)](#compiling-without-microchips-flat)
- [Chip Labels and Headers](#chip-labels-and-headers)
- [`mod`: Inline Chips](#mod-inline-chips)
- [Boundary Pins](#boundary-pins)
- [Nested Chips](#nested-chips)
- [Inline `mod` with Mixed Parameters](#inline-mod-with-mixed-parameters)
- [Complete Example: Named Chip with Handlers](#complete-example-named-chip-with-handlers)
<!-- /toc -->

## Anonymous Chips

An anonymous chip creates a physical microchip for visual organization while sharing the parent scope. Variables, buffers, and arrays declared inside an anonymous chip are accessible from outside it (and vice versa).

### Basic Anonymous Chip

```wirescript
chip {
  var x: int = 0
  var y: int = 0

  on trigger {
    x = x + 1
  }
}

// x and y are accessible here because anon chips share parent scope
out xValue = x.Value
```

Anonymous chips group related gates into a microchip brick and share the scope around them, except for a name the chip declares itself where the top level declares it too.

### Names a Chip Shares With the Top Level

A top-level anonymous chip may declare a name (a `var`, array, map, buffer, `const` or `let`) that the top level declares as well. Inside the chip the name is the chip's declaration: in the chip's statements, and in the body of every `mod` or `chip` written inside it, wherever that `mod` or `chip` is called from. Everywhere else the top-level declaration is what the name means: before the chip and after it, in a constant-only setting such as an event name or a `Sleep` delay, and in a `mod` or `chip` written outside the chip even when it is called inside.

```wirescript
in tick: exec
in go: exec
let limit = 10
var hits: int = 0
chip {
  var limit: int = 3
  mod over(n: int) -> bool { return n > limit }  // the chip's `limit`
  on tick { limit += 1 }
}
on go {
  if over(hits) { hits = 0 }  // still the chip's `limit`
  hits = hits + limit         // the top-level `limit`, 10
}
```

A name only the chip declares is visible across the file. A top-level declaration is named from where it is written on, so a read above it reads the chip's declaration of the name. Two declarations of one name in one chip are `WS013`. Two at the top level itself are read by position (`WS108`): a read names the latest one visible where it is written (storage, declared ahead of everything, above both), as described in [Declaring a Name Again](statements.md#declaring-a-name-again). A chip nested in another names its storage from where it is written on, past the enclosing chip.

### `chip let`: Compact Let Bindings

`chip let` is shorthand for an anonymous chip containing one or more `let` bindings, separated by commas:

```wirescript
chip let score = c0 + c1 + c2 + c3
chip let doubled = score * 2, halved = score / 2

// Equivalent to:
chip {
  let score = c0 + c1 + c2 + c3
}
chip {
  let doubled = score * 2
  let halved = score / 2
}
```

This is commonly used to wrap pure computations in their own microchip for layout clarity.

```wirescript
// From the 2048 example: each chip let gets its own microchip
chip let moved = c0 != c0.prev || c1 != c1.prev || c2 != c2.prev
chip let score = c0 + c1 + c2 + c3 + c4 + c5
chip let emptyCount = (if c0 == 0 then 1 else 0) + (if c1 == 0 then 1 else 0)
```

### `chip on`: Compact Handler

`chip on` is shorthand for an anonymous chip containing a single handler:

```wirescript
chip on trigger {
  count = count + 1
}

// Equivalent to:
chip {
  on trigger {
    count = count + 1
  }
}
```

This is the most common pattern for associating a handler with its own microchip:

```wirescript
chip on goLeft {
  slide(c0, c1, c2, c3)
  slide(c4, c5, c6, c7)
}

chip on goRight {
  slide(c3, c2, c1, c0)
  slide(c7, c6, c5, c4)
}
```

### Negated and Combined Triggers in `chip on`

All trigger forms work with `chip on`:

```wirescript
// Negated trigger
chip on !running {
  DisplayText(ctrl, "Game Over", fontSize = 48)
}

// Regular trigger
chip on moved {
  // ...
}
```

## Named Chips

Named chips define reusable components with explicit inputs and outputs. Each call is a separate instance with its own local state. Names in the body resolve where the chip is declared, so a chip declared at module level can read and write module-level vars; each such read crosses the chip wall through a boundary pin (see [Boundary Pins](#boundary-pins)). Values from the call site come in through parameters.

```wirescript ignore
chip Name(params) -> outputs {
  // body
}
```

### Basic Named Chip

```wirescript
chip Counter(bump: exec, reset: exec) -> (value: int, overflow: bool) {
  var n: int = 0

  on bump {
    n = n + 1
  }

  on reset {
    n = 0
  }

  out value = n.Value
  out overflow = n > 255
}
```

### Parameters

Parameters are declared as `name: type` pairs in parentheses:

```wirescript
chip Adder(a: int, b: int) -> (sum: int) {
  out sum = a + b
}
```

### Destructured Parameters

Parameters can destructure record or tuple values directly in the signature, using the same `{ field, ... }: Type` syntax as `let` destructuring. This is most useful for `mod` declarations that work with record types:

```wirescript
// Destructured parameters: fields are directly in scope in the body
mod distance({ x, y }: Point) -> int { return x + y }
mod process({ data, ...opts }: Config) { ... }
```

Each destructured name is bound directly, just as if written `let { x, y } = param` at the start of the body. The synthetic parameter name (`_p0`, `_p1`, ...) is generated by the compiler and is not accessible in the body.

Tuple destructuring is also supported:

```wirescript
mod sum((a, b): (int, int)) -> int { return a + b }
```

### Outputs

Outputs are declared after `->`. A single output can omit the parentheses, in which case it defaults to the name `_`:

```wirescript
// Single output (defaults to name "_")
chip Double(x: int) -> int {
  out _ = x * 2
}

// As a mod with return
mod Double(x: int) -> int {
  return x * 2
}

// Single-output chips auto-unwrap: f is directly an int
let f = Double(21)    // f == 42, no .result needed

// Multiple named outputs (still use record access)
chip MinMax(a: int, b: int) -> (min: int, max: int) {
  out min = if a < b then a else b
  out max = if a > b then a else b
}
let mm = MinMax(3, 7)  // mm.min == 3, mm.max == 7
```

### Exec Outputs

Chips can have exec-typed outputs for signaling completion. Use `out name: exec` in the signature and `emit name` inside a handler:

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

### Using Named Chips

Call named chips like functions. Each call creates a new physical microchip instance with independent internal state:

```wirescript
chip Double(x: int) -> int {
  out _ = x * 2
}

let result = Double(21)  // result = 42

// Multi-output chips return a record with a field per output
chip MinMax(a: int, b: int) -> (min: int, max: int) {
  out min = if a < b then a else b
  out max = if a > b then a else b
}

let mm = MinMax(3, 7)
let lo = mm.min  // 3
let hi = mm.max  // 7

// Multiple calls create separate instances
let mm1 = MinMax(1, 10)
let mm2 = MinMax(5, 20)  // independent from mm1
```

> **Note:** The `-> (...)` outputs declare the chip's public interface. Use `out name = expr` in the body to wire values to those outputs. Do not redeclare the same name in both the signature and the body. Doing so produces a warning.

### Exec Chips

A chip whose body contains statement-level exec calls (array methods,
`DisplayText`, `emit signal`, ...) is an **exec chip**: the body runs
as one exec chain. How it gets its trigger depends on the call site:

- **Called inside a handler**, the body joins the caller's exec chain
  automatically and execution continues after it.
- **Called outside an exec context** (e.g. a top-level `let`), pass the
  trigger as an `exec = ...` named argument, the same convention as builtin
  exec calls like `Random(0, 10, exec = trigger)`.

```wirescript
in reset: exec

var vals: int[]

chip InitTables() -> (count: int) {
  vals.clear()
  vals.push(1)
  out count = 1
}

let t = InitTables(exec = reset)  // body runs whenever reset fires
```

A call with `exec =` also returns the chip's completion exec as an `exec`
field on the result record (unless the chip declares its own `exec` output),
so callers can sequence on the body having run:

```wirescript
on start {
  emit reset
  await t.exec       // resumes after InitTables finished
}

on t.exec { }        // or treat it as an event
```

To bind body work to a specific exec **parameter** instead, declare the param
and use an `on` handler, because the param does not implicitly invoke the body:

```wirescript
chip Counter(bump: exec) -> (value: int) {
  var n: int = 0
  on bump { n = n + 1 }
  out value = n.Value
}
```

> **Note:** chip bodies reference top-level `var`s, `array`s, buffers, and
> record bindings freely. Wire refs cross chip boundaries, so the body's
> gates connect to the outer nodes directly (`vals` above).

### Reference Parameters (`ref T` / `*T`)

When a chip parameter has type `ref T` (or equivalently `*T`), the caller passes a variable by reference. The chip can both read and write the variable.

```wirescript
chip Increment(counter: ref int, amount: int) {
  on trigger {
    counter = counter + amount
  }
}
```

When a chip has `ref` parameters, its body is automatically placed in exec context (because ref access implies mutable state operations).

The `*T` syntax is an alternative shorthand:

```wirescript
// These declarations are equivalent:
chip Swap(a: ref int, b: ref int) { ... }
chip Swap(a: *int, b: *int) { ... }
```

At call sites, pass variables directly, and the compiler handles the reference wiring:

```wirescript
var x: int = 10
var y: int = 20
Swap(x, y)  // x and y are passed by reference
```

## `const` Parameters and `const mod`

A `mod` or `chip` parameter can be marked `const`, requiring the caller's
argument to be a compile-time constant. Inside the body the parameter reads
as one, usable anywhere a `const` binding is (see
[`const` -- Compile-Time Binding](statements.md#const-compile-time-binding)):

```wirescript
mod ping(channel: const string, v: int) {
  SendCustomEvent(channel, v)
}

in go: exec
var hp: int = 0
on go { ping("died", hp) }   // "died" bakes as the event's channel name
```

A non-constant argument in a `const` slot is **WS046**:

```wirescript ignore
in name: string
mod ping(channel: const string) { SendCustomEvent(channel) }
on go { ping(name) }   // WS046: 'name' is a runtime value
```

`v` above stays an ordinary wired parameter: a `mod`/`chip` can freely mix
`const` and non-`const` parameters. A `const` parameter costs no pin: it
never becomes a `MicrochipInput`, so it doesn't shift the pin index of any
parameter that follows it. For a `chip` (not a `mod`), each distinct
constant value passed to a `const` parameter gets its own compiled instance.
Two call sites passing the same constant share one instance, since the
body is cached by a template key that includes the const value.

### `const mod`

`const mod` marks every parameter of an inlined chip `const` in one step,
equivalent to writing `const` on each parameter individually:

```wirescript
const mod double(n: int) -> int { return n * 2 }
```

A call to a `const mod` is itself const-evaluable (see
[What's const-evaluable](statements.md#whats-const-evaluable)), so it can be
used to build a `const` value out of one or more calls, including calls to
other `const mod`s and reads of module-level constants.

`const chip` is a parse error. A `chip` compiles to one shared physical
microchip template reused across call sites, so "every parameter is const"
is not a property the declaration can have, as it is for an inlined `mod`.
Mark the parameters you need constant individually instead:

```wirescript ignore
const chip C(v: int) -> (r: int) { out r = v }   // error: use a mod, or const params

chip C(name: const string, v: int) -> (r: int) { out r = v }   // fine
```

A `const` binding inside a named `chip`'s body is compile-time throughout that
chip (its handlers, its `out`s, and its constant-only config slots), the
same as inside a `mod`. Two places it is not: the body of an **anonymous**
`chip { }`, and a chip *declared inside* another chip, which does not inherit
the enclosing chip's body constants (pass them in as `const` parameters). Both
are reported rather than silently dropped (see
[Where `const` is allowed](statements.md#where-const-is-allowed)).

A `const mod` may declare multiple outputs, and its result destructures the same way
as an ordinary multi-output mod's does. One rule is specific to `const`: the
wire graph keeps whichever assignment to an output comes first in source order.
A valued `return` placed after an `out` to the same output would disagree with
the wire graph, so it is rejected (**WS046**) instead of compiling to a
mismatch.

```wirescript ignore
const mod pick(n: const int) -> (r: int) {
  out r = 111
  if n > 0 { return 222 }   // WS046: disagrees with the wire graph
}
```

### The mixed-parameter macro pattern

Because `const` and ordinary parameters mix freely, a `mod` can behave like a
small macro that both configures a gate at compile time and wires a runtime
value through it in the same call:

```wirescript
mod tagged(label: const string, amount: int) {
  SendCustomEvent("evt_" .. label, amount)
}

in go: exec
var score: int = 0
on go {
  tagged("score", score)   // "evt_score" bakes; score stays a live wire
}
```

### const `if` and tree-shaking

An `if` whose condition is const-evaluable is resolved at compile time: the
taken branch is checked and lowered normally, and the untaken branch is
dropped before type-checking reaches it.

```wirescript
const MODE = 1
var x: int = 0
in go: exec

on go {
  if MODE == 1 {
    x = 1
  } else {
    x = someModeTwoOnlyFunction()   // never type-checked: MODE is always 1
  }
}
```

This lets one `mod`/`chip` serve call sites that share no common API. A
branch that would not compile for a given constant value is never checked
for it. The editor's hover on a dropped block explains why it
was dropped (e.g. `` `MODE == 1` is true here ``).

**This does not work for a `const` parameter.** A `mod`/`chip` body is
type-checked once, before any call site exists, so there is no real
argument yet to decide the branch with. During that check a `const`
parameter holds a placeholder of its type, and the placeholder is never
allowed to decide a branch. If it could, the compiler would accept the
branch the placeholder happened to select and leave the other branch (the
one a real call takes) unchecked. **Both branches of an `if` on a const
parameter (or a `const` derived from one) are type-checked**, and each must
be independently valid code, even though only one of them lowers per call
site:

```wirescript
var x: int = 0

mod f(mode: const int) {
  if mode == 1 {
    x = 1
  } else {
    x = someOtherModesFunction()   // still type-checked, even if every call site passes mode = 1
  }
}
```

Both arms must compile for every possible constant value, not just the ones
a call site passes. A top-level `const` (or one read from inside a mod body
that is not itself a parameter) does not have this restriction. Only the
parameter itself, and anything derived from it, carries the placeholder.

## Open and Closed Chips

Non-root chips compile **open** by default: each chip's inner grid renders
as an upright plane facing the same side of the chip as its `@bottom`
rerouter pins, stacked in a wall above the placed microchip brick. The
root module's plane sits at the bottom, with its bottom edge just above the
brick. Directly nested chips occupy a row above it, and deeper nesting stacks
higher. Rows are centered and packed side by side, ordered first by
parent position, then by source order within a parent. Within a plane the
dataflow axis runs bottom-to-top: input gates sit at the bottom edge,
outputs at the top.

Annotate a chip with `@closed` to collapse it instead. A closed chip still
reserves its slot in the wall, so opening it later in-game reveals it in
place:

```wirescript
@closed chip Counter(bump: exec) -> (value: int) {
  var n: int = 0
  on bump { n = n + 1 }
  out value = n.Value
}

@closed chip {
  var scratch: int = 0
}
```

`@closed` works on every chip form: `chip Name(...)`, `chip { ... }`,
`chip on`, and `chip let`.

## Compiling Without Microchips (`@flat`)

A module-level `@flat` at the top of the entry file inlines every chip body
into the module that instantiates it. The program then emits no microchip
bricks and no nested grids. Every gate lands on one grid, and a wire that
would have crossed a chip wall becomes an ordinary same-grid wire instead of a
pair of wires through a boundary pin.

Flattening does not change the program's behavior. A chip is not a scoping or
timing boundary, so a flattened program computes the same results as the nested
one. What it removes is the tree of planes to open and the pin at each crossing.

Because the microchip brick and its plane no longer exist, `@closed` and
`@label` have nothing to describe under `@flat`. They are inert rather than an
error, so a file can be compiled both ways without editing its chips.

See [Statements](statements.md) for `@flat`'s placement rules and how it
composes with `@layout(...)`.

## Chip Labels and Headers

`@label("text")` overrides the display text on a chip's shell-brick label
and its plane header:

```wirescript
@label("Score Tracker") chip {
  var score: int = 0
}
```

`@label` also applies to `in`/`out` declarations at any nesting level, and
stacks with a `@left`/`@right`/`@top`/`@bottom` side annotation in either
order (see [Outer Rerouter Pins](statements.md#left--right--top--bottom-outer-rerouter-pins)).
There it overrides the port's floating display label (and its rerouter pin
label); the port's wiring-UI name is unaffected either way.

The `@label` argument may also be an expression: a constant folds to baked
text, and on a top-level `var` a runtime expression becomes a **dynamic
label** that shows the value live (see
[Expression labels](statements.md#expression-labels-labelexpr)). A chip
label must be a constant (its shell brick has no wired text component).

Each opened plane with a title or doc comment gets a header at its top
edge: the title rendered as `<size="96">...</>`, with the chip's `///` doc
comment on the line below. The title is the chip's `@label` text if
present, otherwise its name (the root chip's title is the module name); an
anonymous, undocumented chip gets no header at all. Header text is passed
through raw, so rich-text tags inside a name or doc comment render as rich
text:

```wirescript
/// Counts ticks forever.
chip Counter(bump: exec) -> (value: int) {
  var n: int = 0
  on bump { n = n + 1 }
  out value = n.Value
}
```

`@color(...)` and `@material(...)` style a chip's shell brick the same way:
`@material(Glass) chip { ... }` makes it Glass. On a chip the value must be a
constant; see
[`@color` / `@material`](statements.md#color--material-brick-color-and-material).

`mod` declarations always inline into their callers and never become a
physical chip, so any annotation (`@closed`, `@label`, `@color`,
`@material`, or a side pin) on a `mod` is an error.

## `mod`: Inline Chips

The `mod` keyword declares a chip that is always **inlined** at call sites. Instead of creating a separate microchip brick, the `mod`'s gates are expanded directly into the caller's grid.

Only the gates are expanded into the caller. Every name in the body resolves where the `mod` is declared, the same way a `chip` body's names do: a `var`, `let`, parameter or nested `mod` the caller declares under the same name is never the one the body reads, writes or calls.

A `fn` takes the same signature but compiles once, into a function definition plane that every call runs through a Function Call gate. Use a `fn` when one body is called from many places. Use a `mod` when the call should cost no extra gate, needs type parameters, or must run in a pure context. A `for` loop is the third kind of plane: compiled once per loop, swept once per iteration. See [`fn`](statements.md#fn-function).

```wirescript ignore
mod Name(params) {
  // body: no output
}

mod Name(params) -> (result: type) {
  // body: single output, use `return expr` to set it
}
```

### Mod Outputs and `return value`

Mods can declare a single output with `-> (name: type)`. Use `return expr` to set the output and exit:

```wirescript
mod clamp(v: int, lo: int, hi: int) -> (result: int) {
  if v < lo { return lo }
  if v > hi { return hi }
  return v
}

let clamped = clamp(x, 0, 255)  // clamped is directly an int
```

> **Note:** A single `return expr` wires the value directly (pure, zero-tick). Multiple `return expr` statements cause the compiler to insert a hidden variable: each branch does a `Var_Set`, and a `Var_Get` after the return union reads the result. This adds exec gates but is correct within the same tick's exec chain.

An output set by name (`emit name = v` or `out name = v`) works the same way when it is set in more than one place, under a branch or loop, or beside a `return`: every write goes to the hidden variable, and the call reads the value the path that ran left there. A `mod` output behaves like a `chip` output here, per call site for a `mod` and per instance for a `chip`. A path that does not write it leaves it holding its last value, which starts at the type's default (`0`, `""` or `false`). A write from an `on` handler nested in the mod lands whenever that handler fires, and the caller sees it until the caller's own chain writes the output again. A `return` inside such a handler is one of those writes: it sets the outputs and ends that handler's run, not the call. A `return` inside an anonymous `chip { }` is an ordinary return of the body.

### Default and Named Arguments

A trailing `mod` or `fn` parameter may have a default value, a constant a call uses when it leaves the argument out. A call can also pass a defaulted parameter by name after its positional arguments, which skips the ones before it.

```wirescript
mod scale(v: int, by: int = 2, offset: int = 0) -> int {
  return v * by + offset
}

in go: exec
in n: int
on go {
  let a = scale(n)              // by = 2, offset = 0
  let b = scale(n, 3)           // by = 3
  let c = scale(n, offset = 1)  // by keeps its default
  BroadcastChatMessage("${a} ${b} ${c}")
}
```

A parameter without a default is always passed by position (`WS022`), and a name that is not a parameter is `WS041`. A namespaced call (`u.scale(n, offset = 1)`) takes defaults and names the same way. A `chip` parameter cannot have a default (`WS114`).

### When to Use `mod`

Use `mod` when:
- You want to reuse logic without the overhead of a physical microchip
- The logic is small and called many times
- You need the expanded gates visible in the parent grid

```wirescript
mod slide(a: *int, b: *int, c: *int, d: *int) {
  if a == 0 { a = b; b = c; c = d; d = 0 }
  if b == 0 { b = c; c = d; d = 0 }
  if c == 0 { c = d; d = 0 }
  if a == b { a = a + b; b = 0 }
  if b == c { b = b + c; c = 0 }
  if c == d { c = c + d; d = 0 }
}
```

### Variadic Parameters (`...rest`)

A `mod` (only a `mod`, never a `chip`) can end its parameter list with a `...rest`
variadic that captures every argument past the fixed parameters. Because a mod is
inlined at each call site, `rest` is a compile-time tuple of the actual extra
arguments. There is no runtime variadic. A `...rest` in the body then splats that
tuple onward into another call, one element per positional slot:

```wirescript
mod broadcast(name: const string, ...rest) {
  SendGlobalCustomEvent(name, ...rest)
}

in amount: int
in kind: float
in go: exec
on go {
  broadcast("hit", amount, kind)   // -> SendGlobalCustomEvent("hit", amount, kind)
}
```

The fixed parameters are still required (passing fewer is an arity error). Only the
trailing count is open. The same `...tuple` splat works on any call: spread a bound
tuple, a tuple literal, or a multi-output result into consecutive arguments:

```wirescript
mod add3(a: int, b: int, c: int) -> (r: int) { return a + b + c }

in n: int
let pair = (2, 3)
out total = add3(n, ...pair)         // -> add3(n, 2, 3)
```

Because the body is type-checked once (at the declaration, where the captured arity
isn't known yet), a `...rest` forward is not statically arity-checked against a
fixed-arity target. It is validated when the mod inlines at each call. Forwarding
into `SendCustomEvent`/`SendGlobalCustomEvent` (whose data slots accept any type) is
the common case and always sound.

### `mod` vs `chip`

| Feature | `chip` | `mod` |
|---------|--------|-------|
| Physical microchip | Yes | No (inlined) |
| Gates | Own grid | Expanded into caller |
| Names in the body | Resolved where declared | Resolved where declared |
| Outputs with `->` | Yes (multi) | Yes (single or named, with `return`) |
| `ref`/`*` params | Yes | Yes |
| Reusable | Yes (separate instance per call) | Yes (expanded per call) |

A `mod` declares outputs with `->`: one (`-> int`, set by `return expr`) or several named ones (`-> (lo: int, hi: int)`, set by `return (a, b)`). It can also act through `ref` parameters.

## Boundary Pins

Every wire that crosses a chip wall (a value, an exec trigger, or a
variable reference alike) routes through a labeled `MicrochipInput` or
`MicrochipOutput` rerouter at that wall, one pin per external source, even
if the source isn't a declared parameter. Multiple consumers inside the
chip fan out from the same pin rather than each getting their own. A
constant argument is the one exception: it's inlined directly into the
gate that uses it rather than wired in, so it never costs a pin.

## Nested Chips

Chips can be nested inside handler bodies and other chip bodies:

```wirescript
on trigger {
  chip {
    // This anonymous chip is inside the handler
    let localCalc = a + b
    // ...
  }

  chip {
    // Another chip in the same handler
    if condition { x = localCalc }
  }
}
```

Nested anonymous chips still share their enclosing scope. Named chip declarations inside blocks create locally-scoped chip types.

```wirescript
// From the 2048 example: deeply nested chips
if running && moved {
  chip {
    chip {
      let b0 = 0
      let b1 = b0 + if c0 == 0 then 1 else 0
      // ...
    }
    let r = Random(0, emptyCount - 1)
    chip {
      let p0 = c0 == 0 && r == b0
      // ...
    }
    chip {
      if p0 { c0 = 2 }
      // ...
    }
  }
}
```

## Inline `mod` with Mixed Parameters

A `mod` can take both reference and value parameters. Value parameters are pure inputs; reference parameters allow mutation:

```wirescript
mod renderCell(c: *int, tid: *int, p: bool, px: float, py: float) {
  let t = if p || c == 0 || c == c.prev then 0.0 else 0.15
  let bucket = if c <= 4 then 0
    else if c <= 16 then 1
    else if c <= 64 then 2
    else 3
  let col = Fmt('{' .. bucket .. '}', 'eee4da', 'f2b179', 'f65e3b', 'edcf72')
  let txt = '<color="${col}">${c}</>'
  DisplayText(ctrl, txt, positionX = px, positionY = py, fontSize = 40, transition = t, textId = tid)
}
```

## Complete Example: Named Chip with Handlers

```wirescript
chip Timer(tick: exec, reset: exec, limit: int) -> (value: int, expired: bool) {
  var elapsed: int = 0

  on tick {
    if elapsed < limit {
      elapsed = elapsed + 1
    }
  }

  on reset {
    elapsed = 0
  }

  out value = elapsed.Value
  out expired = elapsed >= limit
}
```
