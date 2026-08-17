# Diagnostics

Every problem the compiler reports carries a stable `WSxxx` code. This page
lists every code the type-checker and lowering passes emit, grouped by the kind
of problem. Codes are **errors** (they stop compilation) unless the entry marks
them *(warning)* — warnings compile but flag something likely unintended.

Diagnostics run in a fixed order — parse, resolve/import, type-check, lower,
wire-graph analysis — so an early error can suppress later ones on the same
construct.

> Some numbers in the `WS0xx` range are **not used**: `WS009` and `WS018` have
> no emit site. `WS034` was once a generic-chip cross-wiring guard and has
> since been removed.

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
| `WS021` | Use before declaration — a chip/mod is called above the point where it's declared (declarations register in source order). | `helper()` above `mod helper() { }` |
| `WS043` | An `on <call> -> <pattern>` general (non-event) trigger's call has no exec-typed output for `on` to auto-extract — it needs an event, or a call whose result includes an exec field (e.g. via `exec = ...`). | `on pair(5) -> (p, q) { }` where `pair` has two plain (non-exec) outputs and no `exec =` arg |

## Types & operators

`WS003` is the general type-mismatch code; the others are narrower.

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS003` | Type mismatch — a value doesn't coerce to the expected type (assignment, argument, output, array element, `if`-branch join, event input). | `var n: int = "hi"` |
| `WS004` | No operator overload for the operand type(s) (arithmetic / comparison / logical). | `"a" + 1` |
| `WS008` | Taking `&`/`ref` of a non-reference — only a variable, ref parameter, or array/map element can be referenced, not a temporary. | `&(a + b)` |
| `WS011` | No overload for a bitwise/shift operator (`&`, `\|`, `^`, `~`, `<<`, `>>`), or a builtin call with the wrong number of positional arguments. | `1.5 & 2` |
| `WS016` | *(warning)* `let` / `out` annotation doesn't match the inferred type (a checked assertion; string-format coercion is exempt). | `let n: int = s` where `s: string` |
| `WS025` | A non-storable type used as storage — `any`, `zone`, `teleport`, or `prefab` in a `var` / `buffer` / array / map. | `var x: any = 0` |
| `WS031` | A reference (`zone` / `teleport` / var ref) used in an `if`-then-else — a Select routes a value, not a reference. | `if c then zoneA else zoneB` |

## Calls & arguments

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS020` | Recursive chip/mod call — chips and mods inline at compile time and can't call themselves, directly or mutually. | `mod f() { f() }` |
| `WS022` | Wrong argument count in a user mod/chip call. | `mod f(a: int) {}` then `f(1, 2)` |
| `WS035` | A `self`-receiver mod shadows a builtin receiver-method of the same name/receiver — it could never be reached as a method; rename it. | `mod Dot(self: vector, o: vector) -> float { ... }` |
| `WS036` | A non-`self` mod called with method syntax `x.f(…)` — call `f(x, …)` directly, or rename its first param to `self`. | `mod f(a: int) {}` then `x.f()` |
| `WS038` | The callee isn't callable — it's a var/let/array/param, not a mod/chip/fn (often an index typo). | `xs(i)` for `xs[i]` |
| `WS041` | Unknown named argument — it matches no parameter and no settings-menu config field, so it does nothing (`exec =` and a variadic call's trailing options are exempt). | `p.DisplayText(t, positionX = 0.0)` |

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
| `WS044` | An array/map method (`.push`, `.set`, `.remove`, …) called on something that isn't an array or map — the receiver didn't resolve to a container, so the operation would otherwise be silently dropped. Also reported for a MUTATING method whose receiver is a `const` array or map: a `const` container is immutable, so its compile-time value and its runtime contents can never disagree. | `let x = 5` then, in an exec handler, `x.push(1)`; or `const t = [1, 2]` then `t.push(3)` |

## Compile-time constants (`const`)

See [`const` -- Compile-Time Binding](statements.md#const----compile-time-binding).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS046` | Not a compile-time constant — the value names a runtime value, a call to a mod that isn't `const mod`, an unsupported syntactic form, an out-of-range compile-time array index, or a missing compile-time map key/record field. The message names the actual offender. | `in live: int` then `const n = live + 1` |
| `WS047` | The certified evaluator refuses to compute the value even though every operand IS constant — integer overflow, a non-ASCII string operand, or an uncertified gate/operand combination. The value is computable in principle; the compiler will not guess it. | `const s = "café".ToUpper()` |
| `WS048` | Const evaluation gave up — the call chain is too deep or took too many steps (guards a runaway or self-referential `const mod` chain against a stack overflow, since a `const mod` calling itself type-checks fine). | a `const mod` that calls itself, reached from a constant-only position such as a custom-event channel name |

## Gate & event config

Some gates and events take **config args** that bake into gate data rather than
wiring in as ports — see [Built-in Events](statements.md#built-in-events) and
[Custom Events](builtins.md).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS028` | Invalid gate/event config — an unknown enum member, an out-of-range int, a missing required config field, or a non-constant value for constant-only config. Note that a DESTRUCTURING `let` binds runtime names even when its source is itself constant, so only the `const` spelling of one satisfies a constant-only slot; the `let` is rejected here rather than silently baking an empty value. | `on Clock(pulseOn = someVar)`; `let { chan } = src` then `SendCustomEvent(chan, n)` |
| `WS030` | *(warning)* Custom-event sender/receiver type mismatch — a send's data value type disagrees with the receiver's declared type on the same channel. | sender sends `float`, receiver declares `amount: int` |
| `WS042` | *(warning)* A `CustomEvent`/`GlobalCustomEvent` handler param has no type annotation and no in-unit sender to infer its type from — data defaults to `float`. Annotate the param, or add a matching `SendCustomEvent`/`SendGlobalCustomEvent` in the same unit, to silence. | `on CustomEvent("dmg") -> (amount) { }` with no in-unit `SendCustomEvent("dmg", …)` |
| `WS045` | *(warning)* A custom-event send's data argument has no concrete type (`any`, or an `Opaque(...)` that erased its input's type) — the send emits the float variant regardless, so it won't match a receiver that declares a real type. Give the value a typed binding (e.g. a `var` of that type) instead of `any`/`Opaque(...)`. Unlike WS030 this needs no in-unit receiver to visit. | `SendGlobalCustomEvent("a", who, Opaque(true))` |

## `any`

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS032` | *(warning)* An `any` annotation on a non-storage position (port, param, `let`, output) — prefer a [generic type parameter](types.md#any-vs-a-generic-parameter), which keeps the type. | `mod f(a: any) -> any { return a }` |

---

Parse- and lexer-level problems (an unexpected token, an unterminated string or
comment, a construct lowering doesn't yet support) are reported under the
separate code **`WSP001`**, outside the `WS0xx` numbering above.
