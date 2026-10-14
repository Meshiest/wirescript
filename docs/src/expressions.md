# Expressions

Expressions compute values. In Wirescript, expressions map to wire graph gates: each operator or function call becomes one or more gates with their ports wired together.

<!-- toc -->
## Contents

- [Operator Precedence](#operator-precedence)
- [Evaluation Order](#evaluation-order)
- [Arithmetic Operators](#arithmetic-operators)
- [Comparison Operators](#comparison-operators)
- [Logical Operators](#logical-operators)
- [Bitwise Operators](#bitwise-operators)
- [String Concatenation](#string-concatenation)
- [String Interpolation](#string-interpolation)
- [Conditional Expressions (if-then-else)](#conditional-expressions-if-then-else)
- [Atom Literals](#atom-literals)
- [Record Literals](#record-literals)
- [Tuple Literals](#tuple-literals)
- [Field Access](#field-access)
- [Index Access](#index-access)
- [Tuple Pick](#tuple-pick)
- [Function Calls](#function-calls)
- [Ref and Deref](#ref-and-deref)
- [Type Conversion (`as`)](#type-conversion-as)
- [Parenthesized Expressions](#parenthesized-expressions)
- [Common mistakes](#common-mistakes)
- [Asset References](#asset-references)
- [Prefab References](#prefab-references)
<!-- /toc -->

## Operator Precedence

Operators are listed from **lowest** (loosest binding) to **highest** (tightest binding):

| Precedence | Operators | Associativity | Description |
|-----------|-----------|---------------|-------------|
| 2 | `\|\|` `^^` | Left | Logical OR, Logical XOR |
| 3 | `&&` | Left | Logical AND |
| 4 | `\|` | Left | Bitwise OR |
| 5 | `^` | Left | Bitwise XOR |
| 6 | `&` | Left | Bitwise AND |
| 7 | `==` `!=` `is` | Left | Equality, enum variant test |
| 8 | `<` `<=` `>` `>=` | Left | Comparison |
| 9 | `<<` `>>` | Left | Bitwise shift |
| 10 | `+` `-` `..` | Left | Addition, subtraction, string concat |
| 11 | `*` `/` `%` | Left | Multiplication, division, modulo |
| 12 | `**` | **Right** | Exponentiation |
| 13 | `as` | Left | Type conversion (`x as float`) |
| -- | `-` `!` `~` `*` `ref` | -- | Unary prefix operators |
| -- | `.field` `[i]` `(args)` | Left | Postfix: field access, index, call |

Parentheses `( )` override precedence as usual.

```wirescript
// ** is right-associative:
let x = 2 ** 3 ** 2   // = 2 ** (3 ** 2) = 2 ** 9 = 512

// Standard math precedence:
let y = 1 + 2 * 3     // = 1 + (2 * 3) = 7

// String concat is at the same level as +/- and groups left to right,
// so `"x=" .. x + 1` means ("x=" .. x) + 1, which is a WS004 error.
let s = "x=" .. (x + 1)
```

## Evaluation Order

Wherever an expression has several parts, they are evaluated in source order,
and each part keeps the value it had when it was evaluated. That covers an
operator's operands, a call's arguments (named ones too), a container method's
arguments, an index or ref and the value written through it (`xs[i] = v`,
`*r = v`), the slots of an interpolated string, an if-expression's condition and
branches, and the fields of a record, tuple or enum payload. So a part that reads storage (a var, a `let`
computed from one, an `out`) is read before a later part runs code that writes
that storage:

```wirescript
var q: int = 0
var o: int = 0
let l = q * 3
mod bump() -> int {
  q = q + 1
  return 1
}
in go: exec
on go {
  o = l + bump()     // `l` from before bump()
  o = bump() + l     // `l` from after it
}
```

`q = q + bump()` likewise reads `q` before `bump()` runs. An array or map is a
reference, read where it is used, except where a whole-record assignment copies it
(see [Records as storage](types.md#records-as-storage)).

## Arithmetic Operators

| Operator | Operation | Operand Types | Result Type |
|----------|-----------|---------------|-------------|
| `+` | Addition | `int, int` | `int` |
| `+` | Addition | `float, float` | `float` |
| `+` | Addition | `int, float` or `float, int` | `float` |
| `+` | Addition | `int, bool` or `bool, int` | `int` |
| `-` | Subtraction | (same as `+`) | (same as `+`) |
| `*` | Multiplication | (same as `+`) | (same as `+`) |
| `/` | Division | (same as `+`) | (same as `+`) |
| `%` | Modulo | (same as `+`) | (same as `+`) |
| `+` `-` `*` `/` `%` | Vector math | `vector, vector` (or `vector` + scalar) | `vector` |
| `+` `-` `*` `/` `%` | Color math | `color, color` (or `color` + scalar) | `color` |
| `+` `-` `*` `/` `%` | Rotation math | `quat, quat` / `rotator, rotator` (or a mix) | `quat`/`rotator` |
| `+` `-` `*` `/` `%` | Object operand | `int`/`float` + an object (player, entity, …) | numeric |
| `**` | Exponentiation | `int`/`float` (not vectors) | (same as `+`) |
| `-x` | Negation (unary) | `int` | `int` |
| `-x` | Negation (unary) | `float` | `float` |

Mixed `int`/`float` arithmetic promotes the result to `float`. `bool` values are treated as `0`/`1` when mixed with `int`.

`+ - * / %` also operate component-wise on two `vector` operands, lowering to
the same math gates (whose inputs accept the vector, `f64` and `i64` wire
variants). Mixing a vector with a scalar broadcasts the scalar across the
components (`v * 2.0`, `10.0 * v`, `v / 4`), and the result is a vector. The
`Scale` helper still works for explicit vector-scalar scaling.

Colors work the same way. The math gate's variant set also covers `color`
(`LinearColor`), so `+ - * / %` operate RGBA channel-wise on two `color`
operands. Mixing a color with a number broadcasts that number across the
channels. Add a number to lighten (`c + 0.1`) or multiply to scale (`tint * 0.5`
dims every channel including alpha), with the number on either side (`0.1 + c`,
`2 * c`). The result is a color.

The same gates also accept the rotation family (`quat` / `rotator`), so `q1 * q2`
composes two rotations. Operands of the same type keep their type. A
`quat`/`rotator` mix yields a `quat`, which coerces freely back to a `rotator`
(see [types](types.md)).

An object operand (a `controller`/`character`/`entity`,
e.g. a player) no longer coerces directly to an `int` on a math gate, so it is
routed through `(obj || false)` first: `1 + player` lowers to
`add(1, or(player, false))`. The `||` gate coerces the object to a value the math
gate accepts. The compiler inserts this for you, so write the arithmetic as usual.

```wirescript
let a = 10 + 3       // 13: int
let b = 10.0 + 3     // 13.0: float
let c = 2 ** 10      // 1024: int
let d = -42           // -42: int (negative literal folded at parse time)
let p = Vec(1.0, 2.0, 3.0) + Vec(4.0, 5.0, 6.0)  // (5, 7, 9): vector
let q = Vec(1.0, 2.0, 3.0) * 2.0                 // (2, 4, 6): vector × scalar
let blend = c1 * 0.5 + c2 * 0.5                  // RGBA channel-wise color blend: color
let lighter = tint + 0.1                          // add a number to every channel: color
let dimmer = tint * 0.5                           // multiply every channel: color
let spin = a1.ToRotation() * a2.ToRotation()     // compose two rotations: quat
let n = 1 + player                                // object → (player || false)
```

## Comparison Operators

All comparison operators return `bool`.

| Operator | Operation | Operand Types |
|----------|-----------|---------------|
| `==` | Equal | any wire variant pair |
| `!=` | Not equal | (same as `==`) |
| `<` | Less than | (same as `==`) |
| `<=` | Less or equal | (same as `==`) |
| `>` | Greater than | (same as `==`) |
| `>=` | Greater or equal | (same as `==`) |

Comparison accepts all wire variant types (`int`, `float`, `bool`, `string`, `entity`, `controller`, `character`) in any combination.

`value is Enum.Variant` tests which variant an enum value holds and also
returns `bool`; see [enums](enums.md#is).

```wirescript
let isZero = count == 0
let isPositive = score > 0
let sameTeam = teamA == teamB
```

## Logical Operators

| Operator | Operation | Operand Types | Result |
|----------|-----------|---------------|--------|
| `&&` | Logical AND | any wire variant pair | `bool` |
| `\|\|` | Logical OR | any wire variant pair | `bool` |
| `^^` | Logical XOR | any wire variant pair | `bool` |
| `!` | Logical NOT (unary) | any wire variant | `bool` |

Wire variant types are: `bool`, `int`, `float`, `exec`, `string`, `entity`, `controller`, `character`. The engine coerces all of these to bool on bool ports (truthy/falsy). An `exec` value is true for the tick it fires, and works directly as an operand of the logical operators (`&&`, `||`, `^^`, `!`), as an `if` or `if ... then ... else` condition and as an `on` trigger, read directly or through a `let`. Otherwise it is not a `bool` value. Storing it (in a `bool` variable, array, map, record field or output), passing it to a `bool` parameter or returning it from a `-> bool` mod is `WS003`, and comparing it with `==` is `WS004`. An exec carried through an output pin of no type (`out o: any`) reads as true where a `bool` is read or written. Read as any other type (a number, text, a color), it carries no value, which is `WS094`:

```wirescript
in reset: bool
in start: exec
on reset || start { ... }

let canMove = isAlive && !isFrozen
let either = a ^^ b
```

`!(a && b)` and `!(a || b)` are automatically fused into single NAND/NOR gates by the compiler.

## Bitwise Operators

All bitwise operators produce `int` results. Operands are `int`. A `float` or `bool` operand coerces to `int` first (`1.5 & 2` truncates `1.5` to `1`), so only a non-numeric operand (`string`, `vector`, …) is a **WS011** error.

| Operator | Operation |
|----------|-----------|
| `&` | Bitwise AND |
| `\|` | Bitwise OR |
| `^` | Bitwise XOR |
| `~` | Bitwise NOT (unary) |
| `<<` | Left shift |
| `>>` | Right shift |

`~(a & b)` and `~(a | b)` are automatically fused into single NAND/NOR gates.

```wirescript
let mask = 0xFF
let high = (value >> 8) & mask
let combined = a | b
let flipped = ~flags
let shifted = 1 << bitIndex
```

## String Concatenation

The `..` operator concatenates strings. It automatically converts numeric types to their string representation.

| Left | Right | Result |
|------|-------|--------|
| `string` | `string` | `string` |
| `string` | `int` | `string` |
| `int` | `string` | `string` |
| `string` | `float` | `string` |
| `float` | `string` | `string` |
| `int` | `int` | `string` |

```wirescript
let greeting = "Hello, " .. name
let label = "Score: " .. score
let coords = x .. ", " .. y .. ", " .. z
```

## String Interpolation

Strings (both `"double"` and `'single'` quoted) support `${expr}` interpolation. The embedded expression is evaluated and converted to a string.

```wirescript
let msg = "Player ${name} scored ${points} points"
let debug = 'pos=(${pos.x}, ${pos.y}, ${pos.z})'
let nested = "result: ${a + b * c}"
```

Interpolated expressions can be arbitrarily complex:

```wirescript
let status = "Health: ${if hp > 50 then "OK" else "LOW"}"
```

Use `\$` to include a literal `$`:

```wirescript
let price = "Cost: \$${amount}"
```

> **Both arms evaluate.** An `if` expression compiles to a `Select` fed by both
> results, so the arm that is not chosen is computed and discarded. The
> condition only picks which value is used and skips no work. Two consequences: an
> expensive expression in the untaken arm still costs its gates every tick, and
> an arm that would fail on bad input (an out-of-range slice, a divide by zero)
> still runs. Gates yield a default rather than faulting, which is why this is
> safe, but it means an arm cannot be used to guard another.
>
> The exception is an arm that does more than give its value. On an exec chain
> (a handler or a `mod` it calls), an arm that writes a variable or an output,
> sends, mutates a container, runs an action builtin, or `return`s (directly,
> through a `match` block arm, or inside a `mod` it calls) runs only when it
> is taken. In that case the `if` compiles like an `if` statement whose
> branches store the value, and the value is read after they rejoin. An `if`
> whose value is a record, tuple or enum can't do this (`WS097`), so write an
> `if` statement instead. A `match` used as a value treats its arms the same
> way. This applies only to what the arm itself runs. An `on` handler inside a
> `mod` or `chip` the arm calls runs whenever its own trigger fires, whether or
> not the arm is taken (`WS102`).

## Conditional Expressions (if-then-else)

The `if-then-else` expression evaluates to one of two values based on a condition. It is a pure expression that compiles to a Select gate.

```wirescript
let abs = if x < 0 then -x else x
let label = if count == 1 then "item" else "items"
let clamped = if v > max then max else if v < min then min else v
```

Syntax: `if <condition> then <true-expr> else <false-expr>`

`then` and `else` may also start their own continuation lines:

```wirescript
let intel = if playerCount <= 6
  then "You have a teammate"
  else "You are alone"
```

### Block Expressions

Branches can be block expressions `{ stmts...; value }` with locally-scoped `let` bindings:

```wirescript
let result = if x > 0 then {
  let doubled = x * 2
  let offset = doubled + 1
  offset
} else {
  0
}
```

`let` bindings inside a block expression are scoped to that block and are not accessible outside. The block's value is its last expression.

Block expressions stay pure (Select gate) as long as they only contain `let` bindings. They can be used anywhere an expression is expected:

```wirescript
let norm = { let len = sqrt(x*x + y*y); len }
```

The result type is the **widening join** of both branches, the same rule
that infers a [generic](types.md#generics) type parameter from its call-site
arguments: numeric branches widen to a common numeric type (`int` widens to
`float`), and object branches widen toward `entity`. The branches are not
required to already match:

```wirescript
let value = if flag then 42 else 3.14  // type: float (int widens to float)
```

If the branches have no common widening (e.g. `int` and `vector`), it's a
compile error:

```wirescript
in v: vector
let bad = if flag then 1 else v
// ERROR WS003: if-then-else branch type mismatch: then is int, else is
// vector (no common widening)
```

Conditional expressions can be nested and used anywhere an expression is valid:

```wirescript
let score = baseScore + (if hasBonus then 100 else 0)
chip let emptyCount = (if c0 == 0 then 1 else 0) + (if c1 == 0 then 1 else 0)
```

## Atom Literals

An atom literal `:name` is a compile-time `int` constant: the deterministic
xxHash64 (seed 0) hash of `name`. It's a readable stand-in for a hand-picked
magic number: a key for an `int`-keyed map, or an enum-like tag.

```wirescript
var scores: Map<int, int> = { :red => 10, :blue => 20 }
if team == :red { ... }
```

The name may contain letters, digits, `_`, and `-` (the first character must
be a letter or `_`, since a leading digit or `-` doesn't lex as an atom start).
`:my-text` is a single atom, not `:my` followed by a subtraction. Write
`:a - b`, with spaces, when you mean `:a` minus `b`. Atoms only lex where a value is expected
(after `=`, an operator, `(`, `,`, `[`, `=>`, ...). A `:` immediately after
something that already reads as a value stays a plain colon, as in a type annotation (`x: int`), a
record field (`{ x: 1 }`), or a map's string/atom key separator (`"red": 1`,
`:red: 1`).

There's no runtime string-to-hash gate: an atom's value is always resolved
at compile time, so `:name` can only appear as a literal, never built from a
runtime string.

## Record Literals

Record literals construct values of a named record type. Fields are specified as `name: expr` pairs inside braces:

```wirescript
type Point = { x: int, y: int }
let p: Point = { x: 1, y: 2 }
```

**Shorthand syntax**: when a field name matches a variable in scope, you can omit the value:

```wirescript
var x: int = 0
var y: int = 0
let p: Point = { x, y }  // equivalent to { x: x, y: y }
```

**Spread operator**: copy all fields from an existing record, then override specific fields:

```wirescript
let a: Point = { x: 1, y: 2 }
let b: Point = { ...a, y: 99 }  // b.x == 1, b.y == 99
```

Later fields override spread fields. Multiple spreads are allowed.

A record *value* is a compile-time abstraction: it produces no wire graph gates, and each field resolves directly to the underlying binding of its value expression. A record used as storage (a `var`, array, or map) is the exception: it decomposes into one gate per field. See [Records as storage](types.md#records-as-storage).

## Tuple Literals

Tuple literals construct tuple values with positional elements:

```wirescript
let pair = (1, 2)
let triple = (true, 3.14, "hello")
```

Access elements with `.0`, `.1`, `.2`, etc.

## Field Access

Use dot notation to access fields on values:

```wirescript
let x = position.x       // vector field
let r = myColor.r         // color field
let p = myRotator.pitch   // rotator field
```

### Vector / color / rotation components

Every component-typed value can be read one component at a time:

| Type | Components |
|------|------------|
| `vector` | `.x` `.y` `.z` |
| `color` | `.r` `.g` `.b` `.a` |
| `rotator` | `.pitch` `.yaw` `.roll` |
| `quat` | `.x` `.y` `.z` `.w` |

The names are also accepted upper-case. They work on any expression of that
type (a `Vec(...)` literal, an input, a stored variable, or a `let` binding)
and lower to the matching Split gate (SplitVector, SplitColor, SplitRotation,
SplitQuaternion), which outputs the single `float` component:

```wirescript
in a: vector
in b: vector
let sum = a + b          // vector
let height = sum.z       // float: the z component

in tint: color
let red = tint.r         // float

in facing: rotator
let turn = facing.yaw    // float, in degrees

let spin = a.ToRotation() // quat
let scalar = spin.w       // float
```

Reading several components of the same value costs a single gate, not one per
component, because the split gates are identical and merge.

A component name belongs to only one type, so using one on another type (`v.w`,
`facing.x`, `tint.z`) is a `WS010` error. It is never silently read as some
other component.

### Variable Fields

Variables have special `.Value` and `.prev` fields:

```wirescript
var count: int = 0

// .Value: reads the current value (delayed read, works in pure context)
out currentCount = count.Value

// .prev: reads the previous tick's value (change detection)
chip let changed = count != count.prev
```

In exec context, a bare variable name auto-dereferences to its value when used in expressions (arithmetic, comparisons, etc.). When passed to a `*T` parameter, the variable stays as a reference. In pure context, a bare name reads the var's value (the same as `.Value`); `&x` is the var's ref as a value. See [Reference values](types.md#reference-values).

### Record Fields

Functions and chips that return records allow field access on the result:

```wirescript
let input = InputReader(char)
let fwd = input.Forward    // float
let rgt = input.Right      // float
let jmp = input.Jump       // bool
```

### Exec Field (`.exec`)

`.exec` names the **exec output** of an exec-returning call. Use it whenever you
need to reference that completion signal: to sequence on it (`await`), handle it
(`on`), or wire it into another gate's `exec =`.

Four cases resolve through `.exec`:

- **A call that returns a bare `exec`.** Some builtins produce an exec whose
  underlying port has a gate-specific name (e.g. a `Change` detector's exec port
  is `OnChanged`). `.exec` denotes that exec directly:

  ```wirescript
  let ch = Change(score)     // exec: fires when `score` changes
  on ch.exec {               // handle it
    ctrl.DisplayText("score changed")
  }
  ```

- **A chip or mod called with `exec =`.** An exec chip/mod call returns its
  completion exec as an `.exec` field on the result record, so callers can
  sequence on the body having finished:

  ```wirescript
  let t = InitTables(exec = reset)
  on start {
    emit reset
    await t.exec             // resumes after InitTables ran
  }
  ```

- **A call inside a handler.** `.exec` binds that call's exec output without
  advancing the current chain, so the call runs as a branch and the statement
  after it runs right away. `emit` in value position names the chain point the
  branch forks from. See
  [Forking the Exec Chain](exec-context.md#forking-the-exec-chain):

  ```wirescript
  var m: int = 0
  var n: int = 0
  in start: exec

  chip Later() -> (count: int) {
    m = m + 1
    out count = m.Value
  }

  on start {
    let d = Later().exec     // Later runs; the chain carries on
    n = 1                    // runs alongside Later, not after it
    await d                  // rejoins once Later has finished
  }
  ```

- **An event that also carries data.** A data-carrying event expression (a
  custom event with data outputs) is a record whose exec output comes first, so
  `.exec` names that trigger explicitly and lets the event compose with other
  exec signals:

  ```wirescript
  in reset: exec
  on Union(reset, CustomEvent("ping").exec) {   // fires on reset or the event
    ctrl.DisplayText("announced")
  }
  ```

`Union` also takes an exec receiver, so `a.Union(b)` is `Union(a, b)` and a wide
fan-in reads as a left-associative chain instead of nested `Union(Union(...))`:

```wirescript
in reset: exec
on reset.Union(CustomEvent("ping").exec).Union(Change(score)) {
  ctrl.DisplayText("announced")
}
```

See [Explicit Exec Argument](exec-context.md#4-explicit-exec-argument) and
[Exec Chips](chips.md#exec-chips).

## Index Access

Use bracket notation to index into arrays:

```wirescript
let item = myArray[i]
// item.value: the element value
// item.bOutOfBounds: bool, true if index was out of range
```

## Tuple Pick

Use `.N` (dot followed by an integer) to pick an element from a tuple:

```wirescript
let pair = someTupleExpr
let first = pair.0
let second = pair.1
```

## Function Calls

Call functions and chips by name with parenthesized arguments:

```wirescript
// Positional arguments
let dist = Distance(posA, posB)
let s = sin(angle)

// Named arguments (kwargs)
DisplayText(ctrl, "Hello",
  positionX = 0.0,
  positionY = -100.0,
  fontSize = 24
)
```

Named arguments use `name = value` syntax. They can be mixed with positional arguments, but positional arguments must come first (matching the parameter order).

Arguments are evaluated in source order, named ones included, and each keeps the value it had when it was evaluated (see [Evaluation Order](#evaluation-order)). A record argument naming a var's storage is bound to that storage, as a `*T` parameter is.

**Running an exec gate with `exec = <trigger>`.** Any exec-gate call (a builtin
like `Random`, an array/map method, or a user-defined exec chip/mod) accepts a
reserved `exec = <trigger>` named argument that drives its exec input. This lets
an exec gate run outside a handler. It fires each time the trigger's value
changes instead of running as part of the surrounding exec chain, so the call
becomes a leaf that is legal in a pure binding.

```wirescript
// Pure output binding: the get fires whenever `i` changes (i + 1 is never 0):
out c: color = lut.get(i, exec = i + 1).Value

// Outside any handler, provide the trigger explicitly:
let r = Random(0, 10, exec = someTrigger)
```

Pair it with [`.exec`](#exec-field-exec) to sequence on the result. See
[Explicit Exec Argument](exec-context.md#4-explicit-exec-argument).

```wirescript
// First positional args, then named
let v = Vec(1.0, 2.0, 3.0)

// All named
DisplayText(target = ctrl, text = msg, fontSize = 30)
```

## Ref and Deref

`ref` (or `&`) makes a ref value: `let r = ref someVar` and `let r = &someVar` are the same. The `*` prefix operator reads or writes through a ref, in an exec context. See [Reference values](types.md#reference-values) for the full rules.

```wirescript
in go: exec
var someVar: int = 0

on go {
  // ref (or &) makes a ref value
  let r = ref someVar

  // * reads through it
  let val = *r
}
```

In practice, `ref` and `*` are primarily used in chip parameter passing. When a chip parameter has type `ref T` (or `*T`), you pass a variable and the chip can read/write it:

```wirescript
mod increment(counter: *int) {
  counter = counter + 1
}

var n: int = 0
on tick {
  increment(n)  // passes ref to n; n is mutated
}
```

### `*var`: Explicit Deref in Exec Context

Inside exec context, prefixing a variable with `*` explicitly reads its current-tick value via a `Var_Get` gate. This is identical to the implicit auto-deref that happens with a bare variable name in exec context. It exists for clarity or to disambiguate:

```wirescript
var x: int = 0
on tick {
  let a = x    // implicit deref (Var_Get)
  let b = *x   // explicit deref (same Var_Get, same result)
}
```

`*var` is **not allowed in pure context**. It produces error **WS006** ("use `.Value` for pure reads"). In pure context, use `x.Value` instead.

### Variable Read Modes Summary

| Expression | Context | Gate | Meaning |
|------------|---------|------|---------|
| `x` (bare) | Exec | `Var_Get` | Current tick's value |
| `*x` | Exec | `Var_Get` | Current tick's value (explicit) |
| `*x` | Pure | none | **Error WS006**: use `.Value` |
| `x.Value` | Pure or Exec | `.Value` port | Previous tick's value (delayed) |
| `x.prev` | Pure or Exec | `.Value` port | Previous tick's value (same as `.Value`) |
| `x` (bare) | Pure | `.Value` port | The same read as `x.Value`; an unannotated `out` of it is `WS017` |
| `&x` | Pure or Exec | `VarRef` port | `x`'s ref, as a value |

## Type Conversion (`as`)

`value as T` converts a value to another type with the game's Convert gate. It
binds tighter than every binary operator: `a + b as float` converts `b`, and
`-x as int` converts `-x`.

```wirescript
in count: int
out ratio = count as float / 3.0
out shown = count as string
```

Converts `bool`, `int`, `float` and `string` into one another; a number into a
`vector`, `rotator` or `color` (it fills every channel, a color's clamped to
0..1); a `rotator` into a `quat` and back; and a `vector`, `rotator`, `quat` or
`color` into a `string` (`X=1.000 Y=2.000 Z=3.000`, `FFBC00FF`). An `any` value
converts to any type. These are the conversions measured in game. Every other
pair (a vector as a number or a color, a string as a vector, anything to or
from an object) comes out of the gate as the target type's default, so it is
`WS109`.

A float converts to an `int` by truncating toward zero (`-2.7 as int` is `-2`),
a number is `true` when it is not zero, and a string reads as its leading
number (`"42abc" as int` is `42`, `"1e3" as int` is `1`, `"abc"` is `0`). A
constant whose result is known this way is converted at compile time, so
`const half = 3 as float` works and costs no gate.

A record or tuple converts field by field, and both sides need the same fields.
A field already of its target type is not converted, and a conversion nothing
reads costs nothing, so `(a, b) as (bool, int)` is one gate while `b` is an
`int`, and none when the result goes unread.

```wirescript
type Cell = { x: int, y: int }
type Pos = { x: float, y: float }
in at: int
var last: Pos = { x: 0.0, y: 0.0 }
in go: exec
on go {
  let c: Cell = { x: at, y: 2 }
  last = c as Pos
}
```

A conversion `as` cannot make yet (an array or map, an enum, a ref, `exec`,
records whose fields differ) is warning `WS109`, and the value keeps its own
type. Inside a `for` source, put a conversion in parentheses:
`for i in 0..(n as int)`.

## Parenthesized Expressions

Parentheses group expressions to override precedence:

```wirescript
let result = (a + b) * c
let check = !(x && y)
```

## Common mistakes

### Bitwise `&` is lower precedence than `==`/`!=`

This matches C. `x >> 31 & 1 != 0` parses as `x >> 31 & (1 != 0)`. Always parenthesize:

```wirescript
let bit = ((x >> 31) & 1) != 0  // correct
let bad = x >> 31 & 1 != 0      // wrong: compares 1 != 0 first
```

### Chips encapsulate state

Chip internal `var`/`array` are not accessible from outside. Only declared `->` outputs are visible. For shared mutable state, use top-level declarations and `mod` macros.

The reverse works, though: a chip body can reference top-level `var`s,
`array`s, and buffers freely, because wire refs cross chip boundaries.

## Asset References

`$AssetType/AssetName` references an external asset the world embeds by name
(weapons, pickups, projectiles, and audio/font descriptors):

```wirescript
let weapon = $BRItemBase/Weapon_Pistol
let beep = $BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press
```

The editor completes asset references: typing `$` offers the asset types, and
`$Type/` offers that type's asset names (from the embedded asset catalog). An
asset reference is meant to be passed to a gate that takes an asset (e.g. an
inventory or audio gate).

### Components

`$Component/<name>` names a component type: the setting of the Add Component,
Get Component and Get/Set Component Property gates. The name is the
component's display name without its spaces, and the editor completes it
after `$Component/`. `$Component/<name>/<property>` names one of that
component's properties as well, completed after the second `/`:

```
$Component/PointLight
$Component/OneShotAudioPlayer
$Component/PointLight/Brightness
```

Only the types the gates' Component Type setting offers are accepted: the
components a player can add to a brick (lights, audio, buttons, spawners,
zones and the like). A logic gate, Input Splitter, a component's class name,
a misspelled name or a property the component does not have is `WS110`.
Property names come from the component's fields in the save schema, so the
list may hold fields the game's own picker does not offer (unverified). The
type goes to `GetComponent` and `AddComponent` (see
[Components](builtins.md#components)). It is a constant setting, not a value
a program can hold.

## Prefab References

A `$` reference whose path begins with `.` or `/` is a **prefab file
reference**: it points at a `.brz` prefab archive rather than a named catalog
asset:

- `$./file.brz`: relative to the current source file's directory.
- `$/abs/path/file.brz`: a filesystem-absolute path.

A reference ending in `.ws` is a **source prefab**: the file is compiled at
reference time and the result embedded, so a prefab can be kept as readable
source next to the program that spawns it. Its own `$./…` references resolve
relative to that `.ws` file. Source prefabs may nest 8 deep.

Pass either form to [`SpawnPrefab`](builtins.md#spawnprefab):

```wirescript
on spawn {
  SpawnPrefab(prefab = $./turret.brz, offset = Vec(0.0, 0.0, 50.0))
  SpawnPrefab(prefab = $./minion.ws, offset = Vec(0.0, 0.0, 50.0))
}
```

A prefab can also be written **inline**, as a `$` followed by a triple-backtick
block of Wirescript compiled as a nested prefab. The spawned chip's source sits
at the spawn site, and it closes over nothing. It communicates by targeted
custom events like any other prefab. Inline blocks may nest 8 deep.

```wirescript
var subject: character

on CharacterSpawned() -> (who) {
  let e = SpawnPrefab(prefab = $```
    var owner: character
    on CustomEvent("init") -> (p: character) {
      owner = p
      on ServerUptime() {
        if owner.GetUserId() == "" { ReadBrickGrid().DestroySpawnedPrefab() }
      }
    }
  ```, lifetime = 0.0, limit = 64)
  subject = who               // a var: a constant argument emits no wire
  e.SendCustomEvent("init", subject)
}
```

> **Why this form exists.** A back-edge loop advances one iteration per tick, so
> a central chip sweeping N entities with one costs N ticks per pass. A `for`
> sweep finishes in one tick but runs its body N times. Spawning one chip per
> entity moves that work into parallel gate instances, and the per-tick cost
> stops depending on N. The nested `on ServerUptime()` above is not
> registered by the init handler. The wire graph is static, so its trigger is
> live from the moment the prefab spawns, before `init` lands, which is why real
> code guards it on an init-set value. See
> [per-entity fan-out](best-practices.md#10-per-entity-work-a-for-sweep-or-one-chip-each)
> and [Loops](statements.md#loops).

At compile the referenced `.brz` is read and embedded into the output
bundle (content-addressed at `Prefabs/Uploads/<hash>.brz`), and the gate's
`Prefab` property is set to that embedded path, so the compiled program is
self-contained. Typing `$./` completes available `.brz` files (the editor scans
the project directory; the web playground has a Prefabs panel where you upload
or drag in `.brz` files).
