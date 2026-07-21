# Wirescript Language Reference

Wirescript is a high-level language that compiles to Brickadia wire graphs. It replaces manual gate-by-gate wiring with a readable, imperative syntax while preserving the underlying execution model of Brickadia's wire system.

## Table of Contents

1. **[Syntax](syntax.md)** -- Language syntax reference: declarations, statements, blocks, statement terminators, comments, and doc comments.

2. **[Types](types.md)** -- The type system: primitives (`int`, `float`, `bool`, `string`, `entity`, `controller`, `character`, `vector`, `rotator`, `color`, `exec`, `brick`, `prefab`), compound types (`ref T`, `T[]`, tuples, unions, records), and type coercion rules.

3. **[Expressions](expressions.md)** -- Operators (arithmetic, comparison, logical, bitwise, string concatenation), operator precedence, string interpolation, conditional expressions, field access, index access, and function calls.

4. **[Statements](statements.md)** -- `var`, `let`, `buffer`, `array`, `in`, `out`, `if`, `on` (handlers), `event`, `emit`, assignment, and expression statements.

5. **[Builtin Functions](builtins.md)** -- All built-in functions grouped by category: math/trig, vector, entity, controller/character, display, gamemode, raycasting, random, string formatting, and color.

6. **[Chips](chips.md)** -- Anonymous chips (`chip {}`), `chip let`, `chip on`, named chips with parameters, `mod` (inline expansion), `ref`/`*` params, nested chips, and the `open` modifier.

7. **[Execution Context](exec-context.md)** -- Pure vs exec context, what requires exec, handler exec chains, exec unions after handlers, and explicit exec parameters.

8. **[Best Practices](best-practices.md)** -- Gate count and scaling: why every call site is a copy (for `mod` and `chip` alike), the call-site multiplier, single-dispatch event queues, deferred flags, and bitmask state.

9. **[Constant Folding](folding.md)** -- Compile-time evaluation of pure gates with constant inputs, guarded by an in-game-certified semantics table; fold barriers; the certification story and reproducibility guarantees.

## Quick Example

```wirescript
// A simple counter that increments on each round start
var count: int = 0

on RoundStart {
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
