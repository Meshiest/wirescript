# Syntax Reference

## Source Structure

A Wirescript file (`.ws`) is a sequence of top-level declarations. There is no required entry point or wrapper -- declarations appear at the top level of the file.

```wirescript
in trigger: exec
var count: int = 0

on trigger {
  count = count + 1
}

out total = count
```

## Imports

Import declarations bring symbols from other `.ws` files into scope. The `.ws` extension is implicit.

```wirescript
import "lib"                            // import all exportable declarations
import { swap, clamp } from "lib"       // selective import
import { swap as mySwap } from "lib"    // aliased import
import * as utils from "lib"            // namespace import — utils.swap()
```

Only stateless declarations can be imported: `mod`, `chip`, `let`, and `type`. Variables, arrays, buffers, I/O ports, and handlers are not importable.

Paths are resolved relative to the importing file. Circular imports are an error.

## Comments

### Line Comments

Line comments start with `//` and extend to the end of the line.

```wirescript
// This is a line comment
var x: int = 0  // inline comment
```

### Block Comments

Block comments are delimited by `/*` and `*/`. They may be nested.

```wirescript
/* This is a block comment */

/* Block comments
   can span
   multiple lines */

/* And they /* can be */ nested */
```

### Doc Comments

Doc comments start with `///` (three slashes) and are attached to the declaration that immediately follows them. They are preserved by the compiler for documentation generation.

```wirescript
/// The player's current score.
/// Resets to zero each round.
var score: int = 0
```

Multiple consecutive doc comment lines are joined together. A single space after `///` is consumed automatically.

## Statement Terminators

Statements are terminated by **newlines** or **semicolons**. Both are interchangeable -- you can use whichever style you prefer.

```wirescript
// Newline-terminated (typical style)
var x: int = 0
var y: int = 1

// Semicolon-terminated (compact style)
var x: int = 0; var y: int = 1

// Mixed
var x: int = 0; var y: int = 1
var z: int = 2
```

Multiple consecutive newlines and semicolons are consumed as a single statement boundary.

## Line Continuation

Expressions can span multiple lines when split at an operator. The parser skips newlines when it encounters an infix operator, allowing natural line wrapping:

```wirescript
let total = a +
  b +
  c

let check = condition1 &&
  condition2 &&
  condition3
```

Newlines are also allowed inside delimited groups — call arguments `( ... )`,
array literals `[ ... ]`, and record literals `{ ... }` — after the opener,
around commas, and before the closer, with an optional trailing comma:

```wirescript
array names: string[] = [
  "alice",
  "bob",
]

let point = {
  x: 1,
  y: 2,
}
```

## Blocks

Blocks are enclosed in curly braces `{ }` and contain a sequence of statements. They are used for handler bodies, chip bodies, `if`/`else` branches, and named chip declarations.

```wirescript
on RoundStart {
  count = count + 1
  score = 0
}
```

Newlines inside blocks are consumed freely -- blank lines are fine.

## Identifiers

Identifiers start with a letter or underscore and continue with letters, digits, or underscores.

```
valid_name
_private
counter2
myVar
```

Identifiers are case-sensitive. `count` and `Count` are different names.

## Keywords

The following words are reserved and cannot be used as identifiers:

| Keyword | Purpose |
|---------|---------|
| `var` | Mutable variable declaration |
| `let` | Immutable binding |
| `buffer` | Buffered value (delayed one tick) |
| `array` | Array declaration |
| `chip` | Chip declaration (anonymous or named) |
| `mod` | Inline chip (expanded at call sites) |
| `on` | Event handler |
| `in` | Input port declaration |
| `out` | Output port binding |
| `emit` | Emit a user-defined event |
| `if` | Conditional (statement or expression) |
| `else` | Else branch |
| `then` | Used in `if-then-else` expressions |
| `match` | Pattern matching expression |
| `return` | Early return from handler |
| `import` | Import declarations from another file |
| `from` | Used with `import { } from "path"` |
| `as` | Alias in imports or namespace |
| `true` | Boolean literal |
| `false` | Boolean literal |
| `ref` | Reference type or ref-of expression |
| `open` | Modifier for anonymous chips (start expanded) |
| `type` | Record type declaration |
| `static` | Persistent-variable modifier (inside handlers/mods) |
| `await` | Suspend an exec chain until a signal fires |

Using a reserved word as an identifier (eg. `from`) as a variable or parameter
name produces a **cascade of confusing `WSP001 expected Ident, got '<word>'
(Kw)` parse errors** that mask the real cause.

## Literals

### Integer Literals

Decimal, hexadecimal, binary, and octal integer literals are supported. Underscores may be used as digit separators.

```wirescript
42
1_000_000
0xff          // hexadecimal
0b1010        // binary
0o77          // octal
0xFF
0B1100_0011
```

### Float Literals

Floating-point literals use decimal notation with an optional exponent.

```wirescript
3.14
0.5
1e10
2.5e-3
1_000.0
```

A float literal requires a digit after the decimal point -- `1.` alone is not a float literal (it would be parsed as integer `1` followed by a dot).

### String Literals

String literals are delimited by double quotes `"` or single quotes `'`.

```wirescript
"hello world"
'hello world'
```

#### Escape Sequences

| Escape | Character |
|--------|-----------|
| `\\` | Backslash |
| `\"` | Double quote (in double-quoted strings) |
| `\'` | Single quote (in single-quoted strings) |
| `\n` | Newline |
| `\t` | Tab |
| `\r` | Carriage return |
| `\$` | Literal dollar sign (prevents interpolation) |
| `\0` | Null character |

#### String Interpolation

Both single- and double-quoted strings support `${expr}` interpolation. Any expression can be embedded:

```wirescript
"Hello, ${name}!"
"Score: ${score + bonus}"
'Position: ${pos.x}, ${pos.y}'
```

Interpolated expressions are converted to strings. Use `\$` to include a literal dollar sign.

### Boolean Literals

```wirescript
true
false
```

## Operators

Operators are listed here for reference. See [Expressions](expressions.md) for full details on precedence and behavior.

### Arithmetic
`+`, `-`, `*`, `/`, `%`, `**` (power)

### Comparison
`==`, `!=`, `<`, `<=`, `>`, `>=`

### Logical
`&&`, `||`, `!`

### Bitwise
`&`, `|`, `^`, `~`, `<<`, `>>`

### String
`..` (concatenation)

### Other
`=` (assignment), `->` (return type / outputs), `=>` (fat arrow, reserved)

## Top-Level Declarations

The following forms are valid at the top level of a script:

- `var name: type = expr` -- Mutable variable
- `let name = expr` -- Immutable binding
- `buffer name = expr` -- Buffered value
- `array name: type[]` -- Array
- `in name: type` -- Input port
- `out name = expr` -- Output port
- `chip name(params) -> outputs { body }` -- Named chip
- `chip { body }` -- Anonymous chip
- `chip let name = expr` -- Anonymous chip with let bindings
- `chip on trigger { body }` -- Anonymous chip with handler
- `mod name(params) { body }` -- Inline chip (macro-like)
- `on trigger { body }` -- Event handler
- `let name = on trigger { body }` -- Captured event with handler
- `import "path"` -- Import all declarations from file
- `import { names } from "path"` -- Selective import
- `import * as ns from "path"` -- Namespace import
- `if cond { body }` -- Conditional (in exec context)
- `return` -- Early return from handler
- `target = expr` -- Assignment (in exec context)
- `expr` -- Expression statement
