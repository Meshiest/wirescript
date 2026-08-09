# Diagnostics

Every problem the compiler reports carries a stable `WSxxx` code. This page
lists every code the type-checker and lowering passes emit, grouped by the kind
of problem. Codes are **errors** (they stop compilation) unless the entry marks
them *(warning)* — warnings compile but flag something likely unintended.

Diagnostics run in a fixed order — parse, resolve/import, type-check, lower,
wire-graph analysis — so an early error can suppress later ones on the same
construct.

> Some numbers in the `WS0xx` range are **not used**: `WS008`, `WS009`, `WS018`,
> and `WS034` have no emit site. `WS034` was once a generic-chip cross-wiring
> guard and has since been removed.

## Execution context

Wirescript has a *pure* context (continuous signal-flow) and an *exec* context
(imperative code inside `on` handlers). These codes fire when a construct is
used in the wrong one, or when a feedback loop has no tick barrier. See
[Execution Context](exec-context.md).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS005` | Wire-graph cycle with no barrier — a feedback loop must cross a Buffer/Queue/EdgeDetector; break it with `buffer emit`. | a gate loop with no buffer on any edge |
| `WS006` | `*x` deref used in pure context — use `x.Value` for a pure read. | `out v = *count` |
| `WS007` | Exec-only construct outside an exec context — assignment, `emit`, `await`, an array index read, or an exec-returning call with no enclosing exec chain. | `count = 1` at the top level |

## Names, declarations & imports

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS001` | Unknown event or trigger — the name after `on` isn't a known event, input, `let`, `buffer`, `var`, or param. | `on Nope { }` |
| `WS002` | Unknown name or type — an undefined variable, an unknown type, an undefined namespace base, or a misused generic alias (bare, wrong arity, or recursive). | `let x = undefinedVar` / `var x: Widget` |
| `WS012` | Import error — a circular import, an unresolvable file, or a named binding not found in the target module. | `import { nope } from "utils"` |
| `WS013` | Duplicate declaration, or an output that is never assigned. | two `var x: int = 0` in one scope |
| `WS014` | *(warning)* Unused import. | `import { clamp } from "u"`, `clamp` never used |
| `WS015` | *(warning)* `fn` is deprecated — use `mod name(...) -> T { return <body> }`. | `fn f(a: int) -> int { a }` |
| `WS021` | Use before declaration — a chip/mod is called above the point where it's declared (declarations register in source order). | `helper()` above `mod helper() { }` |

## Types & operators

`WS003` is the general type-mismatch code; the others are narrower.

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS003` | Type mismatch — a value doesn't coerce to the expected type (assignment, argument, output, array element, `if`-branch join, event input). | `var n: int = "hi"` |
| `WS004` | No operator overload for the operand type(s) (arithmetic / comparison / logical). | `"a" + 1` |
| `WS011` | No overload for a bitwise/shift operator (`&`, `\|`, `^`, `~`, `<<`, `>>`), or a builtin call with the wrong number of positional arguments. | `1.5 & 2` |
| `WS016` | *(warning)* `let` / `out` annotation doesn't match the inferred type (a checked assertion; string-format coercion is exempt). | `let n: int = s` where `s: string` |
| `WS025` | A non-storable type used as storage — `any`, `zone`, or `teleport` in a `var` / `buffer` / array / map. | `var x: any = 0` |
| `WS031` | A reference (`zone` / `teleport` / var ref) used in an `if`-then-else — a Select routes a value, not a reference. | `if c then zoneA else zoneB` |

## Calls & arguments

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS020` | Recursive chip/mod call — chips and mods inline at compile time and can't call themselves, directly or mutually. | `mod f() { f() }` |
| `WS022` | Wrong argument count in a user mod/chip/fn call. | `mod f(a: int) {}` then `f(1, 2)` |
| `WS035` | A `self`-receiver mod shadows a builtin receiver-method of the same name/receiver — it could never be reached as a method; rename it. | `mod Dot(self: vector, o: vector) -> float { ... }` |
| `WS036` | A non-`self` mod called with method syntax `x.f(…)` — call `f(x, …)` directly, or rename its first param to `self`. | `mod f(a: int) {}` then `x.f()` |
| `WS038` | The callee isn't callable — it's a var/let/array/param, not a mod/chip/fn (often an index typo). | `xs(i)` for `xs[i]` |

## Generics

See [Generics](types.md#generics).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS033` | Generic inference failure — `T` can't be inferred (conflicting args, unpinnable, or out of its bound), an explicit type-arg count/bound is wrong, or type args were given to a non-generic function. | `pick(true, anInt, aVector)` |
| `WS037` | *(warning)* Explicit type arguments on a builtin are ignored — a builtin's result type comes from its arguments. | `Random<int>(0, 5)` |

## Ports, outputs & labels

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS017` | *(warning)* Ambiguous variable output type — `out foo = someVar` with an untyped var; annotate `out foo: T` (value) or `out foo: *T` (ref). | `out o = myVar` |
| `WS019` | A prefab reference must end in `.brz`. | `$./level` |
| `WS023` | A side annotation (`@left`/`@right`/`@top`/`@bottom`) is only valid on a top-level port of the compiled file, not inside a chip/mod body. | `chip { @left in go: exec }` |
| `WS040` | A `@label(<expr>)` isn't a compile-time constant, in a position that requires a baked label (a port, chip, or nested `var`). | `@label(hp) in x: int` |

## Collections & shapes

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS010` | Destructure / field-access shape mismatch — wrong destructure arity, no such record field, or a tuple index out of range. | `rec.missingField` |
| `WS024` | *(warning)* An asset/prefab reference inlined into a constant array initializer is silently dropped — build the array with `.push(...)` in an exec handler. | `var s: entity[] = [$Type/Name]` |
| `WS026` | A map literal used somewhere other than initializing or assigning a Map variable. | `foo({ "a": 1 })` |
| `WS027` | Assigning a whole map from a non-literal is unsupported — use `m.copyFrom(src)`. | `m = otherMap` |
| `WS039` | Invalid map key type — a `Map<K, V>` key must be `int`, `string`, or an object (`entity`/`character`/`controller`). | `var m: Map<float, int>` |

## Gate & event config

Some gates and events take **config args** that bake into gate data rather than
wiring in as ports — see [Built-in Events](statements.md#built-in-events) and
[Custom Events](builtins.md).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS028` | Invalid gate/event config — an unknown enum member, an out-of-range int, a missing required config field, or a non-constant value for constant-only config. | `on Clock(pulseOn = someVar)` |
| `WS029` | *(warning)* A `CustomEvent`/`GlobalCustomEvent` handler param has no type annotation — untyped data has no wire type and defaults to float. | `on CustomEvent("dmg", amount) { }` |
| `WS030` | *(warning)* Custom-event sender/receiver type mismatch — a send's data value type disagrees with the receiver's declared type on the same channel. | sender sends `float`, receiver declares `amount: int` |

## `any`

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS032` | *(warning)* An `any` annotation on a non-storage position (port, param, `let`, output) — prefer a [generic type parameter](types.md#any-vs-a-generic-parameter), which keeps the type. | `mod f(a: any) -> any { return a }` |

---

Parse- and lexer-level problems (an unexpected token, an unterminated string or
comment, a construct lowering doesn't yet support) are reported under the
separate code **`WSP001`**, outside the `WS0xx` numbering above.
