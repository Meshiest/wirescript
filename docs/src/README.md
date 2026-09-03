# Wirescript Language Reference

Wirescript is a high-level language that compiles to Brickadia wire graphs. It replaces manual gate-by-gate wiring with a readable, imperative syntax while preserving the underlying execution model of Brickadia's wire system.

## Table of Contents

1. **[Syntax](syntax.md)** -- Language syntax reference: declarations, statements, blocks, statement terminators, comments, and doc comments.
2. **[Types](types.md)** -- The type system: primitives (`int`, `float`, `bool`, `string`, `entity`, `controller`, `character`, `vector`, `rotator`, `color`, `exec`), compound types (`ref T`, `T[]`, `Map<K, V>`, tuples, unions, records, enums), and type coercion rules.
3. **[Expressions](expressions.md)** -- Operators (arithmetic, comparison, logical, bitwise, string concatenation), operator precedence, string interpolation, conditional expressions, field access, index access, and function calls.
4. **[Statements](statements.md)** -- `var`, `let`, `buffer`, arrays, maps, `in`, `out`, `if`, `match`/`if let`/`let else`, `on` (handlers), `event`, `emit`, assignment, and expression statements; plus the module-level annotations a file opens with (`@fold`/`@nofold`, `@layout("code")`/`@layout("cube")`, `@flat`).
5. **[Builtin Functions](builtins.md)** -- All built-in functions grouped by category: math/trig, vector, entity, controller/character, display, gamemode, raycasting, random, string formatting, and color.
6. **[Chips](chips.md)** -- Anonymous chips (`chip {}`), `chip let`, `chip on`, named chips with parameters, `mod` (inline expansion), `ref`/`*` params, nested chips, the `open` modifier, and compiling without microchips (`@flat`).
7. **[Execution Context](exec-context.md)** -- Pure vs exec context, what requires exec, handler exec chains, exec unions after handlers, and explicit exec parameters.
8. **[Enums](enums.md)** -- Tagged unions: declaring `enum` variants (unit, positional, named payload), construction, `.Discriminant`, `match`, `if let` / `let else`, generic enums, and the built-in `Option`/`Result`.
9. **[Best Practices](best-practices.md)** -- Gate count and scaling: why every call site is a copy (for `mod` and `chip` alike), the call-site multiplier, single-dispatch event queues, deferred flags, and bitmask state.
10. **[Constant Folding](folding.md)** -- Compile-time evaluation of pure gates with constant inputs, guarded by an in-game-certified semantics table; fold barriers; the certification story and reproducibility guarantees.
11. **[Testing](testing.md)** -- Writing a program that checks itself in game: the `ReadBrickGrid()` trigger, a `check` mod over reference counters, staying silent unless something fails, making the failure line diagnostic, comparing two paths rather than one path against a constant, and what an in-game run cannot prove.
12. **[Game Knowledge](game-knowledge.md)** -- Brickadia behaviour the language does not define but programs depend on: rich text markup, the fonts the game ships, input action and axis glyphs, and every action name.
13. **[Diagnostics](diagnostics.md)** -- Every `WSxxx` diagnostic code the compiler emits, grouped by category (context, names, types, calls, generics, config, ...), with a one-line meaning and trigger for each.
14. **[Upgrading](upgrading.md)** -- Breaking changes and how to migrate existing `.ws` code across versions; links the full [`CHANGELOG.md`](../../CHANGELOG.md).

## Quick Example

```wirescript
// A simple counter that increments on each round start
var count: int = 0

on RoundStart() {
  count = count + 1
}

out total = count
```

## How It Works

Wirescript compiles down to Brickadia wire graph gates and wires. Every `var` becomes a variable gate, every operator becomes an expression gate, and every `on` handler becomes an exec chain rooted at an event gate. The compiler handles gate placement, port wiring, and type coercion automatically.

The key mental model: Wirescript has two execution contexts:

- **Pure context** -- Expressions that define continuous signal-flow relationships (like wiring gates together). These evaluate whenever their inputs change.
- **Exec context** -- Imperative code that runs in response to events (like a handler body). These execute sequentially when triggered.

Understanding this distinction is fundamental to writing correct Wirescript. See [Execution Context](exec-context.md) for details.
