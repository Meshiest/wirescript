# Expressions

Expressions compute values. In Wirescript, expressions map to wire graph gates -- each operator or function call becomes one or more gates with their ports wired together.

## Operator Precedence

Operators are listed from **lowest** (loosest binding) to **highest** (tightest binding):

| Precedence | Operators | Associativity | Description |
|-----------|-----------|---------------|-------------|
| 2 | `\|\|` `^^` | Left | Logical OR, Logical XOR |
| 3 | `&&` | Left | Logical AND |
| 4 | `\|` | Left | Bitwise OR |
| 5 | `^` | Left | Bitwise XOR |
| 6 | `&` | Left | Bitwise AND |
| 7 | `==` `!=` | Left | Equality |
| 8 | `<` `<=` `>` `>=` | Left | Comparison |
| 9 | `<<` `>>` | Left | Bitwise shift |
| 10 | `+` `-` `..` | Left | Addition, subtraction, string concat |
| 11 | `*` `/` `%` | Left | Multiplication, division, modulo |
| 12 | `**` | **Right** | Exponentiation |
| -- | `-` `!` `~` `*` `ref` | -- | Unary prefix operators |
| -- | `.field` `[i]` `(args)` | Left | Postfix: field access, index, call |

Parentheses `( )` override precedence as usual.

```wirescript
// ** is right-associative:
let x = 2 ** 3 ** 2   // = 2 ** (3 ** 2) = 2 ** 9 = 512

// Standard math precedence:
let y = 1 + 2 * 3     // = 1 + (2 * 3) = 7

// String concat at same level as +/-:
let s = "x=" .. x + 1  // = "x=" .. (x + 1) -- careful!
```

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
components (`v * 2.0`, `10.0 * v`, `v / 4`); the result is a vector. The
`Scale` helper still works for explicit vector–scalar scaling.

Colors work the same way — the math gate's variant set also covers `color`
(`LinearColor`), so `+ - * / %` operate **RGBA channel-wise** on two `color`
operands, and mixing a color with a **number** broadcasts that number across the
channels: add one to lighten (`c + 0.1`), multiply to scale (`tint * 0.5` dims
every channel including alpha), either direction (`0.1 + c`, `2 * c`). The result
is a color.

The same gates also accept the rotation family (`quat` / `rotator`), so `q1 * q2`
composes two rotations. Same-type operands keep their type; a `quat`/`rotator`
mix yields a `quat` (freely coercible back to a `rotator` — see
[types](types.md)).

An **object** operand (a `controller`/`character`/`entity`/`brick`/`prefab` —
e.g. a player) no longer coerces directly to an `int` on a math gate, so it is
routed through `(obj || false)` first: `1 + player` lowers to
`add(1, or(player, false))`. The `||` gate coerces the object to a value the math
gate accepts. This is automatic — just write the arithmetic.

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

Comparison accepts all wire variant types (`int`, `float`, `bool`, `string`, `entity`, `controller`, `character`, `brick`, `prefab`) in any combination.

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

Wire variant types are: `bool`, `int`, `float`, `exec`, `string`, `entity`, `controller`, `character`, `brick`, `prefab`. The engine coerces all of these to bool on bool ports (truthy/falsy). This means `exec` values (true for one frame) work directly in logical expressions:

```wirescript
in reset: bool
in start: exec
on reset || start { ... }

let canMove = isAlive && !isFrozen
let either = a ^^ b
```

`!(a && b)` and `!(a || b)` are automatically fused into single NAND/NOR gates by the compiler.

## Bitwise Operators

All bitwise operators work on `int` operands and produce `int` results.

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

`let` bindings inside a block expression are scoped to that block — they are not accessible outside. The block's value is its last expression.

Block expressions stay pure (Select gate) as long as they only contain `let` bindings. They can be used anywhere an expression is expected:

```wirescript
let norm = { let len = sqrt(x*x + y*y); len }
```

The result type is the common type of both branches. If the branches have different types, the result is a union type.

```wirescript
let value = if flag then 42 else 3.14  // type: int | float
```

Conditional expressions can be nested and used anywhere an expression is valid:

```wirescript
let score = baseScore + (if hasBonus then 100 else 0)
chip let emptyCount = (if c0 == 0 then 1 else 0) + (if c1 == 0 then 1 else 0)
```

## Match Expressions

Match expressions branch on an event or value, selecting one of several arms. Each arm names an event and optionally binds its data to a variable:

```wirescript
// Syntax: match <scrutinee> { EventName(binding) => expr, ... }
```

Match arms use `=>` to separate the pattern from the body. The body can be an expression or a block.

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

Records are a compile-time abstraction -- they produce no wire graph gates. Each field resolves directly to the underlying binding of its value expression.

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

### Vector / color components

`.x` `.y` `.z` extract the components of any **vector** value, and `.r` `.g`
`.b` `.a` extract the channels of any **color** value (both cases are also
accepted upper-case). They work on any expression of that type — a `Vec(...)`
literal, an input, a stored variable, or a `let` binding — and lower to a
SplitVector / SplitColor gate that outputs the single `float` component:

```wirescript
let sum = a + b          // vector
let height = sum.z       // float — the z component

in tint: color
let red = tint.r         // float
```

### Variable Fields

Variables have special `.Value` and `.prev` fields:

```wirescript
var count: int = 0

// .Value -- reads the current value (delayed read, works in pure context)
out currentCount = count.Value

// .prev -- reads the previous tick's value (change detection)
chip let changed = count != count.prev
```

In exec context, a bare variable name auto-dereferences to its value when used in expressions (arithmetic, comparisons, etc.). When passed to a `*T` parameter, the variable stays as a reference. In pure context, the bare name refers to the `ref T` (the variable reference itself). Use `.Value` to read the value in pure context.

### Record Fields

Functions and chips that return records allow field access on the result:

```wirescript
let input = InputReader(char)
let fwd = input.Forward    // float
let rgt = input.Right      // float
let jmp = input.Jump       // bool
```

## Index Access

Use bracket notation to index into arrays:

```wirescript
let item = myArray[i]
// item.value -- the element value
// item.bOutOfBounds -- bool, true if index was out of range
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

```wirescript
// First positional args, then named
let v = Vec(1.0, 2.0, 3.0)

// All named
DisplayText(target = ctrl, text = msg, fontSize = 30)
```

## Ref and Deref

The `ref` keyword creates a reference to a value. The `*` prefix operator dereferences a reference.

```wirescript
// ref creates a reference
let r = ref someVar

// * dereferences
let val = *r
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

### `*var` — Explicit Deref in Exec Context

Inside exec context, prefixing a variable with `*` explicitly reads its current-tick value via a `Var_Get` gate. This is identical to the implicit auto-deref that happens with a bare variable name in exec context — it exists for clarity or to disambiguate:

```wirescript
var x: int = 0
on tick {
  let a = x    // implicit deref (Var_Get)
  let b = *x   // explicit deref (same Var_Get, same result)
}
```

`*var` is **not allowed in pure context** — it produces error **WS006** ("use `.Value` for pure reads"). In pure context, use `x.Value` instead.

### Variable Read Modes Summary

| Expression | Context | Gate | Meaning |
|------------|---------|------|---------|
| `x` (bare) | Exec | `Var_Get` | Current tick's value |
| `*x` | Exec | `Var_Get` | Current tick's value (explicit) |
| `*x` | Pure | — | **Error WS006** — use `.Value` |
| `x.Value` | Pure or Exec | `.Value` port | Previous tick's value (delayed) |
| `x.prev` | Pure or Exec | `.Value` port | Previous tick's value (same as `.Value`) |
| `x` (bare) | Pure | — | Variable reference (`ref T`, not the value) |

## Parenthesized Expressions

Parentheses group expressions to override precedence:

```wirescript
let result = (a + b) * c
let check = !(x && y)
```

## Gotchas

### Bitwise `&` is lower precedence than `==`/`!=`

This matches C. `x >> 31 & 1 != 0` parses as `x >> 31 & (1 != 0)`. Always parenthesize:

```wirescript
let bit = ((x >> 31) & 1) != 0  // correct
let bad = x >> 31 & 1 != 0      // wrong — compares 1 != 0 first
```

### Chips encapsulate state

Chip internal `var`/`array` are not accessible from outside. Only declared `->` outputs are visible. For shared mutable state, use top-level declarations and `mod` macros.

The reverse works, though: a chip body can reference top-level `var`s,
`array`s, and buffers freely — wire refs cross chip boundaries.

## Asset References

`$AssetType/AssetName` references an external asset the world embeds by name —
weapons, pickups, projectiles, and audio/font descriptors:

```wirescript
let weapon = $BRItemBase/Weapon_Pistol
let beep = $BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press
```

The editor completes asset references: typing `$` offers the asset types, and
`$Type/` offers that type's asset names (from the embedded asset catalog). An
asset reference is meant to be passed to a gate that takes an asset (e.g. an
inventory or audio gate).

## Prefab References

A `$` reference whose path begins with `.` or `/` is a **prefab file
reference** — it points at a `.brz` prefab archive rather than a named catalog
asset:

- `$./file.brz` — relative to the current source file's directory.
- `$/abs/path/file.brz` — a filesystem-absolute path.

The `.brz` extension is required. Pass one to [`SpawnPrefab`](builtins.md#spawnprefab):

```wirescript
on spawn {
  SpawnPrefab(prefab = $./turret.brz, offset = Vec(0.0, 0.0, 50.0))
}
```

At compile the referenced `.brz` is read and **embedded into the output
bundle** (content-addressed at `Prefabs/Uploads/<hash>.brz`), and the gate's
`Prefab` property is set to that embedded path — so the compiled program is
self-contained. Typing `$./` completes available `.brz` files (the editor scans
the project directory; the web playground has a Prefabs panel where you upload
or drag in `.brz` files).
