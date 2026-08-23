# Statements

Statements are the building blocks of Wirescript programs. They declare data, define behavior, and control execution flow.

## `var` -- Mutable Variable

Declares a mutable variable backed by a wire graph variable gate. In exec context (inside handlers or mods), the variable is **reset to its initial value** each time the code path executes.

```wirescript ignore
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
`string`, `vector`, and object types (`entity`, `controller`, `character`).

### `static var` -- Persistent Variable

A `static var` keeps its value across handler/mod invocations. The initial value is set once when the save loads. Use this for accumulators, counters, or state that must survive across calls.

```wirescript
static var total: int = 0     // persists across calls
static var highScore: int = 0

on RoundStart() {
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
on RoundStart() {
  count = count + 1    // reads and writes the int value directly
}
```

See [Execution Context](exec-context.md) for full details.

## `let` -- Immutable Binding

Binds a name to a computed value. Unlike `var`, a `let` binding is not mutable storage -- it is a pure wire connection to an expression's output.

```wirescript ignore
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
on RoundStart() {
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

## `const` -- Compile-Time Binding

`const` binds a name to a value, exactly like `let`:

```wirescript ignore
const name = expression
const name: type = expression
```

```wirescript
const width = 8
const area = width * width          // 64
const greeting = "Score: " .. "0"   // string concatenation folds too
```

### `const` vs `let`

`let` folds its initializer **opportunistically**: when the value happens to
be computable at compile time it becomes a literal, and when it isn't, `let`
falls back to an ordinary runtime wire. Either way the program compiles.
`const` makes the opposite promise: the initializer **must** evaluate at
compile time, and one that can't is a compile error (**WS046**) instead of a
silent runtime wire.

```wirescript
in live: int

let n = live + 1     // fine -- falls back to a runtime wire
```

```wirescript ignore
in live: int

const n = live + 1   // WS046 -- 'live' is a runtime value, not a compile-time constant
```

Reach for `const` when a value's shape drives something that has to be known
at compile time: a gate config field, a custom-event channel name, or the
contents of a baked array. A typo that turns it into a runtime read is
caught immediately instead of shipping a build where that position silently
went empty or default.

### Where `const` is allowed

Anywhere `let` is allowed: at the top level, inside a block, and inside any
`mod`/`chip` body, at any nesting depth.

```wirescript
const TOTAL_SLOTS = 4    // top level

mod f() -> int {
  const doubled = TOTAL_SLOTS * 2  // inside a mod body
  return doubled
}
```

One scope limit is worth knowing, and it is reported, never silent. A NAMED
chip's body constants reach that chip's own code, including its handlers, its
`out`s and its constant-only config slots. A chip *declared inside* it is built
against a constant scope of its own, so a name it would inherit from the outer
body does not resolve there. Rather than drop the value, that reports
**WS028**; pass the value in as a `const` parameter instead.

```wirescript ignore
chip Outer(t: exec) {
  const ch = "evt"
  chip Inner(u: exec) { on u { send(ch) } }                // WS028 -- ch does not reach Inner
  chip Ok(u: exec, c: const string) { on u { send(c) } }   // pass it in instead
  let q = Ok(t, ch)
}
```

### What's const-evaluable

- Literals, and any other `const`/named constant.
- Arithmetic, bitwise, shift, comparison, logical, and `..` string
  concatenation over constant operands, with the same semantics as the
  gates they would otherwise have compiled to: 64-bit wrapping integers,
  divide-by-zero as `0`, and a non-finite float as `0`.
- String interpolation (`"a${1 + 1}b"`) and the certified string/math builtin
  methods (`.ToUpper()`, `.Trim()`, `.Length()`, `sin`, `sqrt`, ...), when
  every operand is constant.
- **`abs`, `min`, `max`, `clamp` fold on FLOAT constants but not INT ones.**
  `min(3.0, 7.0)` folds; `min(3, 7)` emits a gate. This is a coverage gap, not a
  language rule: the certified fold pass only folds a gate on an input shape the
  in-game probe actually recorded, and these four were only ever probed with
  float inputs (unlike `+`/`-`/`*`, which were probed with ints and fold). Until
  the probe is rerun with int inputs, either use a float literal (`max(n, 0.0)`)
  where a float result is fine, or write the constant directly. A single
  non-folding op strands everything downstream of it.
- The `Vec`/`Rotation`/`Color` constructors.
- Array and map literals, indexing (`arr[i]`, `m[k]`), and `.length()`.
- Record literals and field access (`.field`), including nested records.
- A call to a `const mod` (see
  [`const` Parameters and `const mod`](chips.md#const-parameters-and-const-mod)),
  including one nested inside an operator, a unary operator, or a
  `Vec`/`Rotation`/`Color` constructor argument (positional or named):

  ```wirescript
  const mod double(n: int) -> int { return n * 2 }

  const seven = double(3) + 1                 // 7 -- nested in an operator
  const negSix = -double(3)                   // -6 -- nested in a unary operator
  const v = Vec(double(1), y = 2.0, z = 3.0)  // nested in a named constructor argument
  ```

  A call nested one level deeper than that, as an argument to a call whose
  own callee is not itself a `const mod`, still is not evaluated, because
  that callee has no compile-time form of its own to descend through:

  ```wirescript ignore
  const mod double(n: int) -> int { return n * 2 }
  mod scaleUp(n: int) -> int { return n }

  const total = scaleUp(double(3))   // WS046 -- scaleUp is not itself a `const mod`
  ```

  Bind the call first, then pass the result: `const d = double(3)`, then
  `scaleUp(d)`.

An out-of-range array index, and a missing map key or record field, are
refused outright rather than falling back to a stale or default value --
unlike a *runtime* out-of-range array read, which keeps the gate's previous
value, there is no previous value to fall back on at compile time.

A const value reaches every position that requires a literal, not just
another `const` binding: gate config fields, a custom event's channel name
on both the sending and the receiving side, and the contents of a baked array
or map:

```wirescript
const CHANNEL = "evt_" .. "died"

in go: exec
var n: int = 0

on go { SendCustomEvent(CHANNEL, n) }
on CustomEvent(CHANNEL) -> (v: int) { n = v }
```

A `const` array built at the top level can be indexed there too, in both a
baked initializer and a runtime wire operand:

```wirescript
const t = [10, 20]
const z = t[1]        // 20

var counts: int[] = [z, 12345]   // baked into the array's initial contents

var rv: int = 0
in go: exec
on go {
  if z == rv { BroadcastChatMessage("match") }   // baked as a literal operand
}
```

### `const` containers at runtime

A `const` array or map is a compile-time value **and** a runtime container. It
folds wherever the answer is known at compile time (`t[1]` above costs nothing),
and the first runtime read builds a real container gate with the constant
contents baked into its initial value:

```wirescript
const t = [10, 20, 30]
const m = { "a": 1, "b": 2 }

mod pick(ys: int[], at: int) -> int { return ys[at] }

var i: int = 1
var k: string = "b"
in go: exec

on go {
  BroadcastChatMessage(t[i])        // 20, read at a runtime index
  BroadcastChatMessage(t.length())  // 3
  BroadcastChatMessage(pick(t, i))  // 20, passed as a `T[]` argument
  BroadcastChatMessage(m[k])        // 2, read at a runtime key
}
```

The container is built only where something needs it, and only once, so a
`const` table used purely at compile time costs no gates and many runtime reads
of one table share a single gate.

Outside a `const mod` body a `const` container is **immutable**: `t.push(4)`,
`t.clear()` and `t[0] = 4` are all rejected, so the compile-time value and the
runtime contents can never disagree. Declare it `var` to make it mutable.

### Compile-time mutation

Inside a `const mod` body, a `const` array or map can be **mutated in
place** using `push`/`set`/`clear`/`append` on an array and
`set`/`remove`/`clear` on a map, so a collection can be assembled conditionally and still bake
with zero gates:

```wirescript
const mod rooms(n: int) -> int[] {
  const t = [10]
  if n >= 2 { t.push(20) }
  if n >= 3 { t.push(30) }
  return t
}

const layout = rooms(2)   // [10, 20], computed entirely at compile time
```

See [`const` Parameters and `const mod`](chips.md#const-parameters-and-const-mod)
for calling `const mod`s, `const` parameters, and how a const-evaluable `if`
condition drops its untaken branch.

### Diagnostics

| Code | Meaning |
|------|---------|
| **WS046** | Not a compile-time constant. The value names a runtime value, a call to a mod that isn't `const`, an unsupported syntactic form, an out-of-range index, or a missing map key/record field. The message names the actual offender. |
| **WS047** | The certified evaluator refuses to compute the value even though every operand IS constant: overflow, a non-ASCII string operand, or a constructor declining its arguments. The fix is different from WS046: the value isn't a stray runtime read, the evaluator just won't guess it. |
| **WS048** | Const evaluation gave up because the call chain is too deep or took too many steps. Guards a runaway or self-referential `const mod` call chain, which fails with this diagnostic rather than a stack overflow. |
| **WS028** | Reused from ordinary constant-config checking: a value that IS fully constant but has no scalar form for the slot it's used in, such as a const **record** used as a gate's config field, which has no wire representation and must be consumed at compile time (read a field off it instead of handing the whole record to the slot). |
| **WS044** | Reused from the container-method backstop: a mutating method (`push`, `clear`, `set`, `sort`, ...) called on a `const` array or map, which is immutable. Declare it `var`, or do the mutation inside a `const mod` body, where it happens at compile time. |
| **WS007** | Reused from the writable-target check: an index write (`t[0] = 4`) to a `const` array or map, rejected for the same reason as a mutating method. |

## `buffer` -- Buffered Value

Declares a value that is delayed by one tick. Buffers are useful for creating feedback loops where a value depends on its own previous state without creating a circular dependency.

```wirescript ignore
buffer name = expression
buffer name: type = expression
```

```wirescript
buffer prevScore = score
buffer delayed: int = count
```

The optional type annotation is useful when the expression type needs clarification (e.g., for self-referential buffers).

## Arrays -- `var name: elementType[]`

An array holds multiple values of the same element type. Declare one as a `var`
whose type ends in `[]` (there is no separate `array` keyword):

```wirescript
var name: elementType[]
```

```wirescript
var scores: int[]
var positions: vector[]
var names: string[]
var flags: bool[]
```

The type annotation must end with `[]` to indicate it is an array type. The
element type selects the backing array variant (`int` -> Int64 array, `float` ->
double array, `bool`, `string`, `vector`, and object types each map to their
matching array kind), so elements keep their declared type rather than all
being stored as doubles.

An array can be given **constant initial contents** with an `= [ ... ]`
initializer. At the top level (outside an exec handler) the contents are baked
straight into the array gate, so **every element must be a compile-time
constant**. The array loads pre-populated with no runtime setup:

```wirescript
var scores: int[] = [100, 50, -10]
var names: string[] = ["alice", "bob"]
```

A constant is a literal (numbers — including negatives — strings, and bools), or
any expression built from literals and top-level `let` constants. So a table can
name its constants instead of restating their values:

```wirescript
let C_FROZEN = 3
let WIDTH = 8

var masks: int[] = [1 << C_FROZEN, 1 << C_FROZEN | 1]
var cells: int[] = [WIDTH * WIDTH, WIDTH - 1]
```

Constants resolve through chains (`let B = A + 1`) and in any declaration order.
Arithmetic, bitwise, shift, comparison, logical and `..` string concatenation all
fold, using the same semantics as the gates they would otherwise have compiled to
— 64-bit wrapping integers, divide-by-zero as `0`, and a non-finite float as `0`.

Initializers may span multiple lines — newlines are allowed after `[`, around
commas, and before `]`, with an optional trailing comma:

```wirescript
var names: string[] = [
  "alice",
  "bob",
]
```

An element that is **not** a compile-time constant — a runtime value such as an
`in` port, a call, or a `...spread` — is an error at the top level, because there
is no exec context in which to populate it. Build the array from runtime values
inside a handler instead (see below).

### Inferred element type

The element type is taken from the annotation, or inferred from the literal
when there's no annotation:

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
var base: int[] = [3, 4]
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

## Maps -- `var name: Map<K, V>`

A map is a keyed collection backed by a `MapVar` gate. Declare one as a `var`
whose type is `Map<K, V>` (there is no separate `map` keyword):

```wirescript
var scores: Map<string, int>
var owners: Map<int, entity>
```

The **key type `K` must be `int`, `string`, or an object reference
(`entity` / `character` / `controller`)** — a map is keyed by a hashed slot, and
only those types have a slot representation. Any other key type is a **WS039**
error. The value type `V` may be any storable variant.

Like an array, a map **starts empty** and is built at runtime from an exec
handler — read/write access goes through its methods, which are exec-only:

```wirescript
in tick: exec

on tick {
  scores.set("alice", 10)      // insert / overwrite
  let r = scores.get("alice")  // r.Value + r.Found (auto-unwraps to Value)
  if scores.has("bob") { }
}
```

A map literal can seed a map at declaration or in an assignment
(`scores = { "a": 1, "b": 2 }`); a map literal used anywhere else is a **WS026**
error, and assigning a whole map from another map (`m = m2`) is unsupported —
use `m.copyFrom(src)` (**WS027**). See
[Builtin Functions](builtins.md) for the full map-method table (`get`, `set`,
`has`, `remove`, `clear`, `copyFrom`, `length`, `keys`, `values`).

## `in` -- Input Port

Declares an input port for the current scope. At the top level, `in` creates an external input that other wire graphs can connect to. Inside a chip, `in` creates a chip input port.

```wirescript ignore
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

```wirescript ignore
out name: type = expression
```

```wirescript ignore
out score: int = count.Value     // type asserted + value
out ratio: float = hits / total
out ref: *int = myVar            // ref output — exposes the variable reference
```

This form is required when you want to expose a variable reference (`*T`) rather than its computed value, or to disambiguate the type when the compiler would otherwise warn.

### Exec outputs

The typed form without a value declares an exec output port. Use `emit` inside a handler to connect the current exec chain to it.

```wirescript
out done: exec

on RoundStart() {
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

#### Expression labels (`@label(<expr>)`)

The argument may be an expression, not just a string literal. A
**compile-time constant** expression is folded and its value baked as the
label text (a float renders the same 3-decimal way `FormatText` shows one):

```wirescript
let title = "Score"
@label(title) out v: int = 0      // baked "Score"
@label(1 + 2) out w: int = 0      // baked "3"
```

`@label` also applies to a **`var`**, overriding the name it would otherwise
show. On a **top-level `var`** the expression may be a *runtime* value — this
is a **dynamic label**: the value is coerced to text and wired live into the
variable's floating label, so the label updates as the value changes. The
common form is a variable labelling itself with its own value:

```wirescript
@label(score) var score: int = 0     // the label shows score's live value
@label(hp * 2) var shown: int = 0    // any runtime expression works
```

A runtime expression is only dynamic on a top-level `var` (the one element
that carries a wireable text component). A runtime `@label` on a port
(`in`/`out`), a chip, or a nested `var` has nowhere to host the wire and is
a compile error — use a constant there.

#### Module-level `@label` (the root microchip)

A `@label(<expr>)` at the **top of the file**, separated from the first
declaration by a blank line, labels the **root microchip** itself rather than
any declaration — the same blank-line placement rule as `@invisible`/`@nofold`.
A constant bakes the chip's title text; a runtime value labels the chip
dynamically (wired into the root shell's label). The expression may
forward-reference declarations below it, so a chip can label itself with one of
its own variables:

```wirescript
@label(status)          // labels the whole chip with `status`'s live value

var status: string = "idle"
on tick { status = "running" }
```

Without the blank line, `@label(status)` would instead attach to the `status`
declaration directly (a variable self-label, above).

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

### `@fold` -- Constant Folding (on by default)

- Constant folding runs on every compile by default, so `@fold` is now
  **redundant** — it is still accepted for backward compatibility but enables
  nothing that isn't already on. To turn folding *off*, use `@nofold` (above)
  or `--no-fold`.
- Placement, if you do write it: `@fold` at the very top of the **entry** file
  (after any module doc comment), separated from the first declaration by a
  blank line — the same module-level, blank-line rule as `@nofold`, and
  module-level only (there's no decl-scoped `@fold`). A directly-adjacent
  module-doc / `@fold` pair registers as neither and produces a
  module-level-only error.
- If both a module-level `@fold` and `@nofold` are present, `@nofold` wins and
  the parser warns that the two conflict.
- `--fold` on the CLI likewise just re-affirms the default; `--no-fold`
  disables folding. See [Constant Folding](folding.md) for the full
  enable/disable story.

### `@layout("code")` -- Source-Shaped Gate Layout

- By default, gates are placed with a flat topological layout: nodes are
  ordered by dependency depth into columns, with no relationship to where
  they appear in the source. `@layout("code")` at the very top of the
  **entry** file (after any module doc comment), separated from the first
  declaration by a blank line, switches the whole compile to a source-shaped
  layout instead: each occupied source line becomes a row (earlier lines sit
  higher), and a node's horizontal position follows its source column, so
  indentation is visible in the placed gates. Same blank-line rule as
  module-level `@fold`/`@nofold`, and it participates in the same top-of-file
  annotation run -- `@fold` and `@layout("code")` can appear together.
- Entry-file-only, same as module-level `@fold`/`@nofold`: an
  `@layout("code")` at the top of an *imported* file does not carry into the
  importer and has no effect.
- Nodes with no real position in the entry file (values from an imported
  file, or synthetic nodes the compiler generates without a source range)
  adopt the row of whichever node consumes or produces them; a node with no
  such neighbor lands on an overflow row below the last source line.
- Three wrapping tiers keep large modules inside their placement budgets: a
  line wider than the line-width budget soft-wraps into an indented
  continuation row; lines stack into vertical bands capped at a height
  budget; a band that would push a page past its width budget starts a new
  page, stacked above the previous one. Each page is centered and flipped
  independently, so it reads top-down on its own.
- Nested chips inherit the mode from their parent, so a chip's interior also
  renders source-shaped when the entry file has `@layout("code")`.
- **Values that many rows read run down a gutter bus** rather than fanning out
  as one long diagonal wire per reader. Such a value -- a variable, an input
  port, a value handed into or read back out of a chip -- gets a *lane*: a
  column of rerouter bricks standing in the gutter between the input pins and
  the code body, chained downward from beside the value's own producer. At
  every row that reads the value the lane branches off sideways and runs
  straight across into that row's gates, so a read reads as a right angle
  instead of a diagonal. Lanes are packed so a value only holds a column for
  as long as it is read, and the longest-lived, most-read values take the
  outermost columns. A value read on a single row keeps its direct wire unless
  it is stored state, a port, or crosses a chip wall. An exec chain running
  from one statement to the next stays a direct wire either way -- it belongs
  on the spine of its own rows, not out in the gutter.
- **Own-line `//` comments render into the plane** as floating text, on a row
  of their own between the surrounding code rows and left-aligned at the
  comment's own indentation -- so the notes read where they sit in the source.
  A comment lands on exactly one plane: the innermost chip whose own rows
  bracket its line, or the outermost plane for a comment no chip's rows
  bracket (a file's leading or closing notes). Only entry-file comments
  render, and only on planes whose own rows come from the entry file -- an
  imported chip's interior numbers its rows against its own file, so it
  carries no comments and takes its indentation from its nodes' columns.
  **Trailing comments** -- code and then `//` on the same line -- are **not**
  rendered; the code already occupies that row. Doc comments (`///`) are
  unaffected: they keep rendering on the plane header of what they document.
- There is no CLI-flag equivalent (unlike `@fold`/`--fold`) -- `@layout` is a
  source annotation only.
- `"code"` and `"cube"` are the accepted arguments; an unknown name
  (`@layout("grid")`) or a missing/malformed argument (`@layout`,
  `@layout(5)`) is a compile error. Naming a layout twice warns and the last
  one wins.
- **`@layout("cube")` emits no per-gate labels.** A cube packs gates shoulder
  to shoulder, so the floating name on a var, an I/O gate, a var tag, or a chip
  brick cannot be read there, and each one costs a text component. Dropping
  them typically removes most of the components in the save. The shell label
  and every plane header stay, so the block is still identifiable and a chip
  still shows its title when opened, as does a runtime `@label(expr)`, whose
  text is a value the program computes rather than decoration.

```wirescript
@layout("code")

var total: int = 0
in tick: exec

// This note gets a row of its own in the plane.
on tick {
  total = total + 1 // ...but this one is not rendered.
}
```

### `@layout("cube")` -- Compact 3D Packing

`@layout("cube")` packs gates into a cube -- rows of bricks, stacked into
layers along the plane's depth axis -- without analysing the wire graph at
all. The compiler already falls back to this arrangement on modules too large
for the default layout to place economically; the annotation asks for it at
any size.

It is the opposite trade from `@layout("code")`: wires are ignored entirely,
so nothing about the picture tells you how signals flow, but the brick mass is
as small as it can be and placement cost does not grow with the number of
wires. Reach for it when a module is too big to read anyway and you want it to
occupy as little space as possible.

Same placement rules as the other module annotations: at the very top of the
entry file, separated from the first declaration by a blank line, and inert in
an imported file. It applies to nested chip interiors too.

```wirescript
@layout("cube")

var total: int = 0
in tick: exec

on tick {
  total = total + 1
}
```

### `@flat` -- Inline Every Chip

`@flat` compiles the program onto a single grid. Every gate that would have
lived inside a chip is placed alongside the rest, and every wire that would
have crossed a chip wall becomes an ordinary same-grid wire. The result has no
microchip bricks and no nested planes to open.

This is a placement decision, not a semantic one. Chips have never been a
scoping boundary -- wire references cross them freely either way -- so a
flattened program computes exactly what the nested one computed. What changes
is that you get one plane to look at instead of a tree of them, and the
per-boundary rerouter pins a crossing would otherwise need are gone.

It is independent of `@layout(...)` and composes with it. `@flat` with
`@layout("cube")` is the natural pairing: one plane, packed as tightly as the
gates allow. `@flat` on its own, or with `@layout("code")`, works too.

Because the chip bricks no longer exist, `@label` and `@closed` on a chip have
nothing to apply to under `@flat`. They are not an error -- they simply have
no effect.

Same placement rules as the other module annotations: at the very top of the
entry file, separated from the first declaration by a blank line, and inert in
an imported file.

```wirescript
@flat
@layout("cube")

chip Step(a: int) -> (r: int) {
  return a * 2
}

var total: int = 0
in tick: exec

on tick {
  total = Step(total)
}
```

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
on RoundStart() {
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
on RoundStart() {
  score = 0
}

on CharacterDied() -> (character) {
  lives = lives - 1
}
```

Event data is bound with a trailing `-> (…)` tuple capture (or `-> { field: local }` record capture) after the call — see [Binding Event Data](#binding-event-data) below for the full capture model. The number and types of values available are determined by the event (see [Built-in Events](#built-in-events) below).

Some events also accept **config args** that configure the event gate itself, written *inside* the call parens. String literals (and `Name = value` named args) set the gate's config fields — the parens hold config/inputs only, never event data. `ChatCommand` uses this for its command name and help text:

```wirescript
on ChatCommand("greet", "Greets the player") -> (player, args) {
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
on ZoneEntered(zone = room) -> (character) { }  // room feeds the gate's Zone input
on ZoneLeft(zone = room) -> (character) { }
```

> **Frozen entities still fire entity zone events** — `SetFrozen(true)` does not
> suppress `EntityZoneEntered`. But an entry only fires on a **boundary crossing**:
> `SetLocation`-ing an entity to a zone it is *already* inside does **not** re-fire
> the event. To force a fresh entry, move it out of the zone and back in.

### Binding Event Data

`on <call>(config…) -> <pattern> { }` is the general capture form: the call's
parens hold **config/inputs only**, and a trailing `->` binds whatever data
the call produces. It works the same way whether `<call>` is a built-in event,
a custom event, or an ordinary `mod`/`chip` call.

**Tuple capture** — `-> (a, b)` — binds outputs positionally under local
names of your choosing. It works for *any* call that produces data, built-in
or custom, and is the cleanest form when you don't need to rename or skip
fields:

```wirescript
on CharacterSpawned() -> (who) {
  who.ShowStatusMessage("Welcome!")
}
```

When you're binding a *single, untyped* output, the parens are optional —
`-> who` is shorthand for `-> (who)`:

```wirescript
on CharacterSpawned() -> who {
  who.ShowStatusMessage("Welcome!")
}
```

(Annotating the slot still needs the parenthesized form, `-> (who: character)`.)

**Record capture** — `-> { field: local }` — binds outputs *by field name*
instead of position, for events with named data. Rename a field with
`field: local`, or write the bare name (`field`) to keep it as-is; list only
the fields you need — it's fine to bind a subset:

```wirescript
on CharacterDied() -> { character: victim, killer } {
  victim.ShowStatusMessage("You were killed")
  killer.ShowStatusMessage("You got a kill!")
}
```

Record capture only works for events whose data outputs are named fields
(built-in events); a call with positional-only data (e.g. a custom event or a
plain chip/mod) requires tuple capture.

**Binding inside the call parens is an error.** The old `on Event(a, b) { }`
form — where identifiers inside the parens bound data — no longer parses;
the parser points you at `->` instead. Parens are reserved for config and
wired inputs: a literal/named config arg (`"greet"`, `Description = "..."`),
or a `name = value` wired input (`zone = room`, `interval = secs`).

**Custom events** write their data types in the tuple capture:

```wirescript
on CustomEvent("dmg") -> (amount: int, source: character) {
  // ...
}
```

A slot's type can be omitted and is then **inferred** from a matching in-unit
`SendCustomEvent`/`SendGlobalCustomEvent` on the same channel; when no sender
supplies a type either, the slot defaults to `float` and emits `WS042`. See
[Custom Events](builtins.md#custom-events) for the full send/receive contract.

**General triggers** extend the same `-> <pattern>` capture to any
exec-producing call — not just built-in/custom events. `on` auto-extracts the
call's exec output, so this works for a `mod`/`chip` call outside an exec
context, driven by an explicit `exec = ...` input (the same convention as
`Random(0, 10, exec = trigger)`):

```wirescript
var log: string[]

chip Note(msg: string) -> (count: int) {
  log.push(msg)
  out count = log.length()
}

in go: exec
var last: int = 0

on Note("hello", exec = go) -> (count) {
  last = count
}
```

If the callee has no exec-typed output for `on` to auto-extract — a plain
value-only call with no `exec = ...` — that's `WS043`. See
[Exec Chips](chips.md#exec-chips) for how `exec = ...` and the resulting
`.exec` field work on user-defined chips/mods.

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

### Triggering on Arbitrary Expressions

The trigger can be any expression, not just a bare name — a comparison, a
method or index result, or a builtin call. It desugars to a hidden `let` bound
to the expression, and the handler fires when that value changes:

```wirescript
on health <= 0 { respawn() }            // comparison
on a.Dot(b) > 0.0 { faceTarget() }      // method call inside the expression
on arr[i] > 0 { ... }                   // index result
on ServerUptime() > 5.0 { ... }         // builtin call in an expression
```

A builtin call that returns an `exec` (`on ServerUptime()`, `on Change(v)`)
fires the handler on that exec — distinct from an event with config args
(`on Clock(...)`), whose name resolves as an event and keeps its config form.

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

To fire a handler on any of several execs, prefer the `Union(...)` builtin — it
reads as an ordinary call in the unified `on <expr>` model and composes with
`->` output capture and `exec =` inputs:

```wirescript
on Union(eventA, eventB) {
  // Fires on either exec
}
```

The older `|` trigger-union syntax still parses but is discouraged in favor of
`Union(...)`:

```wirescript
on eventA | eventB {   // deprecated — use `on Union(eventA, eventB)`
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
let died = on CharacterDied()
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

```wirescript ignore
emit eventName              // bare exec signal (exec context only)
emit sig = value            // fire a signal carrying a value (payload for `await`)
```

```wirescript
out scored: exec

on CharacterDied() -> (c) {
  score = score + 1
  emit scored
}

on scored {
  DisplayText(ctrl, "Score!", fontSize = 24)
}
```

### Setting an output value

To set a data output's value, use **`out name = value`** (in a handler or a
chip/mod body) or **`return value`** (a single-output `mod`). `emit` is for exec
signals, not data — don't use it to assign a plain output.

```wirescript
out result: int

on trigger {
  out result = computed_value    // set the output's value
}
```

The `emit name = value` form is reserved for carrying a **payload alongside an
exec signal** — a local signal you later `await` (see below): it fires the signal
*and* ferries the value, so use it only when you actually want the exec to route.

### Local Exec Signals

`let name: exec` declares a local synchronization point that can be targeted by `emit` and used with `await` or `on`:

```wirescript
let ready: exec

on compute { emit ready }      // fires the signal
on start { await ready }        // continues when ready fires
```

### Buffered Emit

`buffer emit sig` routes the emit's exec through a **Buffer** gate, delaying delivery by one tick. This is the tick-crossing barrier that makes emit/await **loops** legal: a back-edge `emit` after an `await` closes a wire-graph cycle, and every cycle must cross a Buffer or the compile errors (**WS005**).

```wirescript ignore
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

```wirescript ignore
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
- **A back-edge loop whose `await` sits inside an `if` runs exactly ONE iteration.** This follows from the rule above and is the single easiest way to write a loop that looks fine and is not. The first pass consumes the arm; the buffered back-edge arrives on the next tick, finds the branch untaken and the await unarmed, and the continuation never runs. There is no error, no warning, and no hang - the program carries on with one iteration's worth of work done, so a loop meant to fill 25 entries leaves 1.

  Measured, same loop body both ways: 5 of 5 iterations with the `await` at its handler's top level, 1 of 5 with it inside a branch.

  This bites hardest when a loop lives in a `mod` called from a step machine, because the `mod` inlines into the caller and inherits its branch:

  ```wirescript ignore
  // BROKEN - the call site puts the await inside an `if`
  on tick {
    if step == 4 { fill() }        // fill()'s await inlines into this branch
  }

  // WORKS - the await is at its own handler's top level
  let fillSig: exec
  on fillSig {
    idx = 0
    let loop: exec
    emit loop
    await loop
    if idx < 25 { dest.push(idx) idx += 1 buffer emit loop }
  }
  on tick {
    if step == 4 { emit fillSig }  // pulse it instead of calling it
  }
  ```

  The `if` guarding the loop's own continuation (`if idx < 25`) is fine and required - it is the exit test. What must not be branched is the `await` itself.
- **Loop state must outlive the tick.** A back-edge buffer crosses a tick, and a
  tick is a new call of whatever handler the loop sits in - so a non-static `var`
  declared inside the loop's own scope is reset before the next iteration reads
  it. A counter declared there sticks at its initial value forever, and the loop
  rewrites element 0 on every pass while looking like it is running. Put the
  counter (and any accumulator) at **module level**, or make it `static`, and
  reset it upstream of the `emit`:

  ```wirescript
  var idx: int = 0          // module level: survives the tick boundary

  mod fill(dest: int[]) {
    idx = 0                 // reset BEFORE the emit, not inside the loop body
    let loop: exec
    emit loop
    await loop
    if idx < 25 {
      dest.push(idx)
      idx += 1
      buffer emit loop
    }
  }
  ```

  The failure is silent: the program runs, the loop terminates, and the
  collection simply holds one element.
- **A loop advances one iteration per tick.** The back-edge is a buffer, and a buffer crosses a tick, so walking N elements takes N ticks. That is fine for work that runs once (a reset sweep, a one-off rebuild) and wrong for work that has to happen every tick for every element: a per-tick sweep over a roster of N costs N ticks per pass and degrades as the roster grows. When you need per-tick work per entity, give each entity its own chip instance instead of looping a central one -- see [per-entity fan-out](best-practices.md#10-per-entity-fan-out-one-chip-each-not-one-loop-over-all).

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

```wirescript ignore
await signal                         // resume when signal fires
let val = await signal               // capture the signal's ferried payload
let { a, b } = await signal          // destructure a record payload
let val = await value on trigger     // capture value when trigger fires
let n: int = await CustomEvent("c")  // wait for an event, capture its data
await a || b                         // race -- first signal wins
await Sleep(_, delay = 1.0)          // sleep 1 second using _ armed flag
await SleepTicks(_, delay = 5)       // sleep 5 ticks
```

Each `await` creates an armed flag (`static var bool`) that guards the continuation. The continuation only fires once per arming, preventing repeated triggers.

### Awaiting a Custom Event

`await CustomEvent("chan")` (or `GlobalCustomEvent`) suspends until a matching `SendCustomEvent` fires, and a binding captures its data:

```wirescript
in go: exec
var last: int = 0
on go {
  let amount: int = await CustomEvent("dmg")   // resume + capture DataOut1
  last = amount
}
```

Annotate the binding's type (`let amount: int = ...`): the event's data ports are untyped in-game, so the annotation is what makes the wire carry the right variant. Without it the value defaults to a float and mis-delivers non-float data ([`WS055`](diagnostics.md)). A **tuple** `let (p, t) = await CustomEvent("c")` captures the data outputs positionally (`p` = DataOut1, `t` = DataOut2) but has no place to annotate them, so prefer one typed binding per value, or the handler form `on CustomEvent("c") -> (p: int, t: float) { ... }` when you want several typed at once. A bare `await CustomEvent("c")` with no binding just waits for the event.

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
on RoundStart() {
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
on RoundStart() {
  DisplayText(ctrl, "Round Started!", fontSize = 30)
  SetLocation(entity, newPos)
}
```

## Built-in Events

These events are available as handler triggers. Parameters listed can be bound using the `on Event() -> (param)` tuple capture (or `-> { field: local }` record capture) — see [Binding Event Data](#binding-event-data).

| Event | Parameters | Description |
|-------|-----------|-------------|
| `RoundStart` | `roundNumber: int` | Game round started |
| `RoundEnd` | `roundNumber: int` | Game round ended |
| `CharacterSpawned` | `character: character` | A character spawned |
| `CharacterDied` | `character: character`, `killer: character`, `killerWeapon: entity`, `killerWeaponName: string` | A character died (`killer` / `killerWeapon` are who/what killed it) |
| `ControllerJoined` | `controller: controller`, `userId: string`, `userName: string` | A player joined |
| `ControllerLeft` | `controller: controller`, `userId: string`, `userName: string` | A player left (`userId` / `userName` stay valid even as the controller is torn down on disconnect) |
| `ControllerJoinedTeam` | `controller: entity`, `team: entity`, `userId: string`, `userName: string` | A player joined a team (`team` is the team they joined) |
| `ControllerLeftTeam` | `controller: entity`, `team: entity`, `userId: string`, `userName: string` | A player left a team |
| `ZoneEntered` | `character: character` | A character entered a zone |
| `ZoneLeft` | `character: character` | A character left a zone |
| `EntityZoneEntered` | `entity: entity` | An entity entered a zone |
| `EntityZoneLeft` | `entity: entity` | An entity left a zone |
| `ProjectileZoneEntered` | `character: character`, `projectile: entity`, `weapon: entity`, `weaponName: string` | A projectile entered a zone (`character` is the shooter) |
| `ProjectileZoneLeft` | `character: character`, `projectile: entity`, `weapon: entity`, `weaponName: string` | A projectile left a zone |
| `CharacterDamaged` | `character: character`, `damage: float`, `attacker: character`, `attackerWeapon: entity`, `attackerWeaponName: string` | A character took damage |
| `CharacterFiredWeapon` | `character: character`, `direction: vector`, `start: vector`, `weapon: entity`, `weaponName: string` | A character fired a weapon (`start` / `direction` are the shot ray's origin and aim) |
| `BrickChanged` | (none) | A brick was changed in a zone |
| `BrickRemoved` | (none) | A brick was removed from a zone |
| `ChatCommand` | `controller: controller`, `arguments: string` | A registered chat command was run. Takes config args for the command name + help text — see [above](#triggering-on-built-in-events) |

## `return`

The `return` statement terminates the current exec chain early. It can be used inside:

- `on` handlers
- `chip on` handlers
- `if` blocks within handlers
- `mod` bodies (when called from exec context)

```wirescript
on RoundStart() {
  if score > 100 {
    return  // skip the rest of this handler
  }
  score = score + 1
}

chip on CharacterDied() -> (character) {
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
