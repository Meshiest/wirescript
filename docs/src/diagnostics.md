# Diagnostics

Every problem the compiler reports carries a stable `WSxxx` code. This page
lists every code the type-checker and lowering passes emit, grouped by the kind
of problem. Codes are **errors** (they stop compilation) unless the entry marks
them *(warning)*. Warnings compile but flag something likely unintended.

Diagnostics run in a fixed order (parse, resolve/import, type-check, lower,
circuit analysis), so an early error can suppress later ones on the same
construct.

> Some numbers in the `WS0xx` range are **not used**: `WS009` and `WS018` have
> no emit site. `WS034` was once a generic-chip cross-wiring guard and has
> since been removed.

<!-- toc -->
## Contents

- [Execution context](#execution-context)
- [Names, declarations & imports](#names-declarations--imports)
- [Types & operators](#types--operators)
- [Calls & arguments](#calls--arguments)
- [Generics](#generics)
- [Ports, outputs & labels](#ports-outputs--labels)
- [Collections & shapes](#collections--shapes)
- [Loops](#loops)
- [Functions (`fn`)](#functions-fn)
- [Compile-time constants (`const`)](#compile-time-constants-const)
- [Gate & event config](#gate--event-config)
- [`any`](#any)
- [Internal errors](#internal-errors)
- [Silencing a warning](#silencing-a-warning)
<!-- /toc -->

## Execution context

Wirescript has a *pure* context (continuous signal-flow) and an *exec* context
(imperative code inside `on` handlers). These codes fire when a construct is
used in the wrong one, or when a feedback loop has no tick barrier. See
[Execution Context](exec-context.md).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS005` | Gate cycle with no barrier: a feedback loop must cross a Buffer/Queue/EdgeDetector; break it with `buffer emit`. | a gate loop with no buffer on any edge |
| `WS006` | `*x` deref used in pure context. Use `x.Value` for a pure read. | `out v = *count` |
| `WS007` | Exec-only construct outside an exec context: assignment, `emit`, `await`, an array index read, an exec-returning call, or a `return` inside a branch of a mod or chip called with no enclosing exec chain. A named `chip` whose output writes must choose between paths (several `return`s, a `return` beside one in a nested `on` handler, or a `return` beside an `emit`/`out` of an output) needs that chain too. Also an `emit` of a local exec signal (`let sig: exec`) where no exec chain runs, such as the body of a named `chip` outside its handlers, so nothing would ever fire it. Also a write to something that is not writable storage: a `let`, an enum payload field named directly (destructure it instead), a misspelled record field, or a field of a scalar. | `count = 1` at the top level / `state.field = v` on an enum |
| `WS101` | An initializer on `let s: exec`. That form declares a new signal, fired only by `emit s`, so the value would be dropped and an `on s` handler would never run. Use `on go { .. }` with the existing signal, or name it again with `let s = go` (no annotation). | `let s: exec = go` |
| `WS116` | *(warning)* An `on` trigger that joins an object (an entity, character or player) with `||`. `||` makes one value, and an object is true while it exists, so the handler fires once when the object appears and never on the other side. To fire on either, use `on Union(a, b)`. `&&` with an object (an "is set" guard) and `exec` operands are fine. | `on reset || ReadBrickGrid()` |

## Names, declarations & imports

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS001` | Unknown event or trigger: the name after `on` isn't a known event, input, `let`, `buffer`, `var`, or param. | `on Nope { }` |
| `WS002` | Unknown name or type: an undefined variable, an unknown type, an undefined namespace base, a namespace base shadowed by a local binding of the same name, or a misused generic alias (bare, wrong arity, or recursive). Also a call to a `mod` or `chip` the calling file neither declares nor imports, such as a library body calling its importer's mod or an importer calling a library's private helper (see [Imports](syntax.md#imports)). Likewise a type alias is read in the file that writes it: a library's body sees the aliases the library declares or imports, never one its importer happens to declare, so a library naming an alias it does not have is this error whatever the importer declares. Enums are read the same way: a library names the enums it declares or imports, its own alias wins over an importer's enum of the same name, and a library naming an enum only its importer declares (in a type or in `Dir.B`) is this error. So are the top-level values: a library body reading a `const`, `let`, `var` or port only its importer declares (a value, an event name, a `Sleep` delay, a constant's initializer) is this error. Also a `var` initializer that reads the name it declares when nothing before the `var` has that name (`var y = y + 1`): the initializer reads the names around the declaration, never the variable it declares, and the message says so, with the declaration to write instead when the `var`'s type is known. | `let x = undefinedVar` / `var x: Widget` / `import * as u` then `mod g(u: int) { u.f() }` |
| `WS012` | Import error: a circular import, an unresolvable file, or a named binding not found in the target module. | `import { nope } from "utils"` |
| `WS013` | Duplicate declaration, or an output that is never assigned. In one anonymous `chip { }` a name is declared once: a second `const`, `let` or storage of it is this error, as are two storage declarations (`var`, `buffer`, `array`, `map`) of one name at the top level of a file. A second top-level `const` or `let` of a name, or a `let` beside top-level storage of it, is read by position instead (`WS108`). Also an `enum` of an `import * as` module named like another enum of the program: a program has one enum of a name. A top-level anonymous `chip { }` may declare a name the top level does too (see [Anonymous chips](chips.md)), and a `let` in a body may shadow one declared before it. Also two outputs of one `mod` or `chip` with one name, of which a read of the call could take only one. Also an output driven continuously by a non-constant `out name = expr` and also written in a handler or branch (by `emit` or another `out`): the two would fight over it, so give the continuous one a constant default or remove it. | two `var x: int = 0` in one scope |
| `WS103` | An output no site gives a type (`out z = ...` in a handler or a `mod` body, or `emit z = ...`) is stored as one type that holds every value written to it: `int` and `float` give `float` (an integer constant written there is that float), and a bare `pop`, `get`, `find`, `min`, `max` or `average` counts as the value it reads. This error is for values no one type holds: a `string`, `bool` or enum beside a number, a record beside a single value, or two different records. The message names both writes. Annotate the output (`out z: int = ...`) or write values of one kind everywhere. | `if c { out z = 5 } else { out z = "six" }` |
| `WS105` | A record or tuple written to an output that no site gives a type (`out z = rec` in a handler or at the top level, `emit z = rec`, or an `out` in an anonymous `chip`). Such a port is one pin, and only an annotation gives each field its own, so nothing would drive it. Annotate the output with the record's type: the message spells it, by its alias when there is one (the one declared first when several name it), and puts it on the output's declaration when the record is written elsewhere. A gate with named outputs (`Edge(b)`) is a record of them here; written to an output with a single-value type, it drives the output with its first one when that output is of the output's type (`WS003` otherwise). | `on go { out z = pv }` with `pv: P` |
| `WS107` | A `type` or `enum` declared under the name of a builtin type (`int`, `float`, `string`, `vector`, `any`, ...), or as `Array` or `Ref` with one type parameter (`Array<T>` and `Ref<T>` are how `T[]` and `*T` are written). The builtin is what that name means wherever a type is written, so the declaration would never be used. Give it another name. Any other builtin generic name, such as `Map`, or `Array`/`Ref` with another number of type parameters, is the program's own type wherever the program declares one. | `type float = int` |
| `WS108` | *(warning)* A top-level declaration of a name the top level of the same file declares already: a second `const` or `let` of it, or a `let` beside a `var`, `buffer`, `array` or `map` of it. The file is read by position: a read names the latest declaration visible where it is written (a `let` or `const` from its end, so `let k = k + 1` reads the earlier `k`; storage from its start, though a `var` initializer reads the declaration before the `var`, as `let k = 5` then `var k: int = k` starts the var at 5, while a `buffer` initializer reads the buffer, its value the tick before), so the later declaration hides the earlier one from its position on, for values and constant-only settings (an event name, a `Sleep` delay, a folded `if`) alike. Storage is declared ahead of everything, so a read above every declaration of the name names the storage. The body of a `mod`, `chip`, generic or `const mod` reads as code written where that declaration is, wherever it is called from; above every declaration of the name (and with no storage of it) it names the first. A file imported flat or by name reads its own duplicates the same way, in the order it writes them, however the program reaches it (a named import beside a flat one, several named imports, several paths), and an importer reads the last; a named import of the name binds the last. A module imported `import * as` is the exception: its namespace holds the last declaration only, so every read there names the last one, and its reads through a flat import in the same program too; it gets one warning, which says so. The message names both lines. Rename one to read both. | `buffer tick: int = tick + 1`, later `let tick = ServerUptime()` |
| `WS109` | *(warning)* A conversion `as` cannot make yet: an array or map, an enum, a ref, `exec`, a `zone`/`teleport`/`prefab`, a brick to another type, a value type to an object, or a record or tuple whose fields differ from the target's. The value keeps its own type, so a use that needs the target type reports there. | `xs as float[]` for `var xs: int[]` |
| `WS110` | A name the property gates don't know: a `$Component/<name>` or `Component<name>` that is not a component type the component gates take, a property a component or environment does not have (`light.Volume`, `Environment.Sky.Brightness`), an environment that does not exist, an `Environment.<environment>` with no property, a `GetComponent`/`AddComponent` whose type is not a `$Component/...`, or `Remove()` on a component reference (the game removes by brick: `brick.RemoveComponent($Component/<type>)`). The component types are the components a player can add to a brick, spelled as the display name without spaces. | `$Component/SetVariable`, `Environment.Moon.TimeOfDay` |
| `WS014` | *(warning)* Unused import, or a named import that leaves behind the `on` handlers / anonymous chips the module installs (write `import "mod"` to install those too). | `import { clamp } from "u"`, `clamp` never used |
| `WS021` | Use before declaration: a chip/mod is called above the point where it's declared (declarations register in source order), or a mod or chip body reads a top-level `let` declared below the call (the body reads it where it is called). | `helper()` above `mod helper() { }` / `out b = f(n)` above `let A = n * 2`, with `f` reading `A` |
| `WS091` | *(warning)* A local shadows a `var`, array, map or buffer that a `mod` or `chip` called in the local's scope uses, itself or through the mods it calls. The body resolves its names where it is declared, so the call uses the outer one, never the local. Reported at the local. Rename one of them, or pass the local to the call. | `mod bump() { q = q + 1 }` then `on go { var q: int = 5  bump() }` |
| `WS093` | A name that starts with `__`: a variable, `let`, parameter, `mod`/`chip`, type, enum variant, or record field (declared, written in a literal, or read). The prefix is reserved for names the compiler makes for its own storage, such as an enum's tag slot, so a user name could stand in for one. A `__` later in a name (`a__b`) is fine. | `var __x: int = 0` / `type T = { __tag: int }` |
| `WS043` | An `on <call> -> <pattern>` general (non-event) trigger's call has no exec-typed output for `on` to auto-extract. It needs an event, or a call whose result includes an exec field (e.g. via `exec = ...`). Also `.exec` on a call that runs no exec step, such as an inline `mod` whose body is all values. | `on pair(5) -> (p, q) { }` where `pair` has two plain (non-exec) outputs and no `exec =` arg; `inc(1).exec` for `mod inc(x: int) -> int { return x + 1 }` |
| `WS060` | Unknown variant on an enum path (`Enum.Variant`): the enum has no variant with that name. | `enum Shape { Empty }` then `Shape.Nope` |
| `WS064` | Duplicate discriminant value in an enum declaration. Each variant's tag must be unique. | `enum E { A = 1, B = 1 }` |
| `WS065` | Wrong bracket form for an enum variant's payload: a named-payload variant called with `(...)`, or a positional-payload variant called with `{ ... }`. | `enum S { Box { w: float } }` then `S.Box(1.0)` |
| `WS054` | Non-exhaustive `match`: the arms don't cover every value of the scrutinee enum. Each uncovered pattern is named in the message. Add the missing arm(s) or a `_` catch-all. | `enum S { A, B, C }` then `match s { A => 1, B => 2 }` |
| `WS061` | *(warning)* Unreachable `match` arm: an earlier arm already matches every value this one could, so it never runs. | `match s { _ => 0, A => 1 }` |
| `WS092` | A `match` used as a statement has an arm that is only a value, such as a record literal: its arms run for their effects, so the value does nothing. Write the arm as a block, or use the `match` as a value. | `match e { A => { x: 1 }, B => { n = 1 } }` |
| `WS095` | A `match` used as a value has a block arm (`A => { ... }`) that does not end in `return` on every path. A block has no value, and the `match` reads its value after the arms rejoin. Write the arm as a value (`A => expr`), or make the `match` a statement whose arms assign a `var`. Also a record, tuple or enum-valued `match` with a block arm, and any block arm of a value `match` outside an exec context (a top-level `out` or `let`), where its statements and its `return` never run. | `n = match e { A => { k = 5 }, B => 2 }` |
| `WS097` | An `if` or `match` expression whose value is a record, tuple or enum has an arm that does more than give its value: it writes a variable or an output, sends, mutates a container, runs an action builtin, or `return`s, directly or through a `mod` it calls. A scalar `if` or `match` runs such an arm only when it is taken, by storing the value; a record has no single store to choose through. Use an `if` or `match` statement whose arms assign a `var`. A field read of one (`(if c then mk() else p).a`) is reported in its own words: read the field in each arm instead (`if c then mk().a else p.a`), which is a single value. | `p = if c then { x: bump(), y: 0 } else { x: 0, y: 0 }` |
| `WS102` | *(warning)* An `if` or `match` expression arm on an exec chain calls a `mod` or `chip` whose body has its own `on` handler that acts (writes outside the handler, including a `var` or container the body keeps from one trigger to the next, sends, mutates a container). That handler runs whenever its trigger fires, whether or not the arm is taken: the condition only chooses which value is read. To run it only when taken, call it from an `if` statement whose branch fires its trigger. | `n = if c then Cx(go, 3) else 0`, where `Cx`'s `on t` handler writes a top-level `cnt` |
| `WS062` | A `let <pattern> = ... else { }` whose `else` block can fall through. It must diverge on every path (`return`/`emit`, or an `if`/`match` whose arms all diverge), since the binding is unavailable when the pattern doesn't match. | `let Some(x) = o else { let y = 1 }` |
| `WS063` | A generic enum's type parameter can't be inferred: a variant with no payload to pin it (`Option.None`) is constructed with no annotation supplying it. Annotate the target (`out n: Option<int> = Option.None`) or use a variant whose payload determines it. | `enum Option<T> { Some(T), None }` then `out n = Option.None` |

## Types & operators

`WS003` is the general type-mismatch code. The others are narrower.

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS104` | *(warning)* An integer constant in a place typed `float` (an integer literal, or a name or expression that is one) that a float cannot hold exactly: beyond 2^53 (9007199254740992) in magnitude floats skip integers, so it is stored as the nearest float. Keep it in an `int` to keep every digit. | `var x: float = 9007199254740993` |
| `WS106` | *(warning)* A constant stored as a different value than the one written. A float constant with a fraction that is the whole value of a place typed `int` (an annotated `let`, `const`, `var`, `static var` or `buffer`, an assignment, a `mod` or `chip` argument, a `return`, an output, a field of a record or tuple literal wherever the literal goes (a `var`, a `mod` argument, an enum payload, an array push), an array or map element, an enum payload such as `Opt<int>`, an array or string index, a range bound or a `count`) is stored as the integer it truncates to, held as an integer wherever it then goes, for a `const` exactly as for a `var`. A float constant giving one branch of an `if` or `match` keeps the join's type, and a write through a ref (`*h = 2.5`) stays `WS003`. A float constant beyond the range of an `int` is `WS028`. A constant given to a gate setting the gate keeps in its own data (a tick count, a font size, a justification, a flag) is stored as that setting's field holds it: a dropped fraction, a number other than 0 or 1 made a bool, or a bool made a number for a setting that does not take a bool are each reported, naming the setting, the value written and the value stored. Write the stored value to silence it. A value the field cannot hold at all is `WS028`. | `var n: int = 2.5` / `SleepTicks(_, delay = 0.999)` |
| `WS003` | Type mismatch: a value doesn't coerce to the expected type (assignment, argument, output, array element, an annotated `buffer`'s initializer, `if`-branch join, event input). Also a `return` from a `mod`/`chip` with several outputs whose value does not line up with them: a single value, a tuple of the wrong length, or a record with a field naming no output or missing a value output. Also an `any` value assigned, returned or emitted as a record or enum, which is one wire with no fields to split. Also a record of the program's own (a record literal, or a parameter, `var`, field, element or one-output `mod` call of a record type) where one value goes: a `return`, `emit` or `out` to a single-value output, an assignment, an argument, a record field, an array element, an operand, a comparison, a condition or an interpolated part (`mk(a) + 1`, `"${pv}"`). A multi-output read (`x = Edge(b)`, `n = fs.max()`, a call of a `mod` or `chip` with several outputs, flat, namespaced or as a method), written inline or held in a `let`, a record field or a tuple, gives such a place its first output instead, converted as a value of that output's type is; a multi-output read whose first output is itself such a record is the same error, as the first output is read once, never a field of it. Also a string method called on a value that is not a string (`vv.Length()` on a vector), which would read the text the value formats to: a vector's length is `Magnitude(vv)`, an array's or map's count is `xs.length()`, and `Length(n)` formats a value on purpose. In a generic body the value may be of an unbounded type parameter, which is checked as every type it may be (a `bool` among them); the message names the parameter and the bound to add (`<T: string>`), also for a field or element of a value declared with it (`v[0]` for `v: T[]`), a `let` or `for` name taken from one, and a call's result of its type (`id(v)`). A record a bounded type parameter stands for (`r` in `mod gf<T: R \| int>` reading `m.get(k)`) names the bound to narrow (`<T: int>`), as a field of it is no field of the bound's other members. An `if`, `match` or `Select` of two multi-output reads joins them output by output, an `int` output widening to `float`, when they declare the same outputs in the same order, or by their first outputs otherwise (an argument of one value included); with no common type it is this error, naming both. A record a bounded type parameter stands for names the members of the bound whose check of the body found no error, a class in the bound (`Scalar`, `Numeric`) read as its members, so following it compiles: `<T: R \| Scalar>` suggests `<T: int \| float>`. Also a multi-output read returned whole to several outputs when some, not all, of the outputs are named like the read's: an output named like one takes that one, so every output must be, and the message names the ones that match none; outputs named like none of the read's take them in order. | `var n: int = "hi"` / `vv.Length()` / `-> (Found: bool, Value: float) { return fs.min() }` |
| `WS004` | No operator overload for the operand type(s) (arithmetic / comparison / logical), named as the program spells them. Also two records compared or combined with each other. | `"a" + 1` |
| `WS008` | Taking `&`/`ref` of a non-reference, or passing one to a `*T` parameter: only a variable, a ref parameter or a record field over one can be referenced, not a temporary. An element of a value array has no ref of its own (`&xs[i]`, `&pts[i].x`), and neither does a plain field of a record value (`rr.n` of a `let` literal, `m.get(k).Value` on a map of values). | `&(a + b)` / `&xs[0]` |
| `WS011` | No overload for a bitwise/shift operator (`&`, `\|`, `^`, `~`, `<<`, `>>`), or a builtin call with the wrong number of positional arguments (including more than seven parallel arrays passed to `sortMultiple`, whose gate has seven array ports). Bitwise/shift accept `int`, and `float`/`bool` coerce to `int`; only non-numeric operands (`string`, `vector`, …) have no overload. | `"a" & 2` |
| `WS016` | *(warning)* `let` / `out` annotation doesn't match the inferred type (a checked assertion; string-format coercion is exempt). | `let n: int = s` where `s: string` |
| `WS025` | A type with no storable form used as storage: `zone`, `teleport`, `prefab`, a ref to one of those, a container of records inside another container, or a ref, array or map as a map key. Also a handler-local `*T` / `brick` var with no initializer. | `var h: *zone` / `on go { var h: *int }` |
| `WS031` | A reference-only value (`zone`, `teleport`, prefab) used in an `if`-then-else. A `*T`, an array ref and a `brick` pass through the Select gate and are fine. | `if c then zoneA else zoneB` |
| `WS066` | `.Discriminant`, `.ToInt()`, or a `match` targets a value that isn't an enum. | `var x: int = 0` then `x.Discriminant` |
| `WS067` | A bare variant name for a variant that has a payload (`Circle` instead of `Circle(_)`) in a `match` arm binds the whole value like a catch-all, which can leave later arms unreachable. | `match s { Circle => .., Empty => .. }` |
| `WS068` | A custom-event data param is a record or an enum. A data slot is one wire and those span several, so the value cannot travel through it. Pass the fields as separate params, or send a key and read the value from shared storage on the other side. | `on CustomEvent("c") -> (p: Point) { }` |
| `WS069` | An enum payload field would need container storage. A payload slot is filled by constructing the variant, and a container cannot be constructed into one, so the value would read back empty. Fires on the declaration, or on a generic instantiation that binds a stored parameter to one (`Option<int[]>`). | `enum E { B { xs: int[] } }` |
| `WS070` | An `unsafe` payload access does not name both a variant and a field. The form is `unsafe <value>.<Variant>.<field>`; the variant is what selects the slot. | `unsafe e.B = x` |
| `WS071` | A record value used where a single value is expected that the type check could not see. A record is several wires, so it cannot drive one port. Read a field, or concatenate the fields individually. Also reported for an `if`/`match` choosing between records that hold an array or map of records, tuples or enums, since that field is several containers with no single reference to choose. | `let w = if c then h else h2` for records `h`, `h2` holding a `P[]` |
| `WS072` | An expression that produces no value read as one: a void container mutation (`a.push(x)`, `m.set(k, v)`) or a `mod`/`chip` with no output. There is no wire to pass, so the consumer would read its port default. Use the call as a statement, or give the `mod` an output. Also reported for a named output of a mod or chip call (`f(c).r`) that nothing in the callee sets. | `mod f(x: int) { }` then `o = f(1)` |
| `WS084` | A ref or brick compared with something that is not a ref of the same kind. The game treats `1 == <ref>` as true, so the comparison would mean nothing. Compare two refs, or read through one with `*`. | `h == 1` where `h: *int` |
| `WS085` | `*` applied to an array ref, a map ref or a brick. An array or map ref is used directly (`rows[y].push(v)`), and nothing reads through a brick. | `*rows[0]` / `*anchor` |
| `WS086` | A ref to a ref: a `**T` type, or `&` of a value that already holds a ref. The game has no storage for one. | `var g: **int` / `&h` where `h: *int` |
| `WS100` | A `type` name (a record or other alias, not an enum) read as a value. A record type has no constructor call: `Box { v: 1 }` reads as `Box` followed by a separate `{ v: 1 }`. Give the record literal its type instead: `let c: Box = { v: 1 }`. An alias of another type (`type Score = int`) is worded as one: write a value and give it the alias as its type. | `type Box = { v: int }` then `let c = Box { v: n }` |
| `WS117` | *(warning)* A comparison used directly as an operand of `&`, `|` or `^`. Those bind looser than `==`, `!=`, `<`, `<=`, `>`, `>=` and `is`, so the comparison would run first and the bitwise operator would get its true or false. Parenthesize the part you meant. | `code & 4 == 4` (write `(code & 4) == 4`) |

## Calls & arguments

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS020` | Recursive chip/mod/fn call. Chips and mods inline at compile time, and the game does not run a recursive Function Call, so none can call itself, directly or mutually. | `mod f() { f() }`, `fn f(n: int) { f(n) }` |
| `WS022` | Wrong argument count in a user mod/chip call. | `mod f(a: int) {}` then `f(1, 2)` |
| `WS035` | A `self`-receiver mod shadows a builtin receiver-method of the same name/receiver, so it could never be reached as a method. Rename it. | `mod Dot(self: vector, o: vector) -> float { ... }` |
| `WS036` | A non-`self` mod called with method syntax `x.f(…)`. Call `f(x, …)` directly, or rename its first param to `self`. Also a builtin that takes no receiver called as `x.f(…)`, and a `self` mod the calling file neither declares nor imports: a module's private `self` mod answers only its own module's calls, even when a named import brings in a body that calls it, and an importer's `self` mod is not visible in the bodies it imports. Import it by name or import the whole module, or, from a library, move it into the library (see [Imports](syntax.md#imports)). | `mod f(a: int) {}` then `x.f()` |
| `WS038` | The callee isn't callable: it's a var/let/array/param, not a mod/chip/fn (often an index typo). | `xs(i)` for `xs[i]` |
| `WS041` | Unknown named argument: it matches no parameter and no settings-menu config field, so it does nothing (`exec =` and a variadic call's trailing options are exempt). | `p.DisplayText(t, positionX = 0.0)` |

## Generics

See [Generics](types.md#generics).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS033` | Generic inference failure: `T` can't be inferred (conflicting args, unpinnable, or out of its bound), an explicit type-arg count/bound is wrong, or type args were given to a non-generic function. | `pick(true, anInt, aVector)` |
| `WS037` | *(warning)* Explicit type arguments on a builtin are ignored, because a builtin's result type comes from its arguments. | `Random<int>(0, 5)` |
| `WS096` | Too many distinct instantiations. A generic body is checked for every combination of its type parameters' bound members, or, past 64 combinations, for a sample of them plus each combination a call uses. Calls using more than 1024 combinations beyond those, counting the generics their bodies call, stop being checked. The first call left out is reported, with the number of combinations the calls demand in all and the generics that demand the most. Give the calls fewer distinct types, or narrow the bounds. | one generic called with 1025 different combinations of type arguments |

## Ports, outputs & labels

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS017` | *(warning)* Ambiguous variable output type: `out foo = someVar` with an untyped var. Annotate `out foo: T = someVar` (value) or `out foo: *T = &someVar` (ref). A var that holds a ref offers only `out foo: *T = someVar`, since a ref to it would be a `**T` (`WS086`). | `out o = myVar` |
| `WS019` | A prefab reference must end in `.brz`. | `$./level` |
| `WS023` | A side annotation (`@left`/`@right`/`@top`/`@bottom`) is only valid on a top-level port of the compiled file, not inside a chip/mod body. | `chip { @left in go: exec }` |
| `WS040` | A `@label(<expr>)` isn't a compile-time constant, in a position that requires a baked label (a port, chip, or nested `var`). The same holds for a `@color` or `@material` value on a chip or a nested `var`; only the module and a top-level `var` take a runtime one. | `@label(hp) in x: int` |
| `WS073` | An `out` binding has no port to bind to, so its value is dropped. Declare the port at the top level of the file, in a chip body, or in the chip's signature. | `chip { on Clock { out x = Toggle() } }` |
| `WS090` | A `mod` or `chip` with a value output uses `return`, and some path ends (at a bare `return`, or at the end of the body) before every value output has a value on it. That path would output a value from another path (the only `return`'s, or the previous call's). An output gets a value on a path from `return <value>` or `emit r = ...`; a continuous `out r = ...` counts for every path, and an exec output never stands in for a value one. Return a value on that path, or give the `if` an `else` that returns. | `mod f(c: bool, q: int) -> int { if c { return q } }` |
| `WS099` | `return <value>` in a top-level `on` handler or anonymous chip, outside every `mod` and `chip`. Only a mod or chip has outputs a `return` gives a value to; the file's own `out` ports are written by name. Write `emit o = value` (or `out o = value`), then a bare `return`. | `out o: int` then `on go { return 1 }` |

## Collections & shapes

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS010` | Destructure / field-access shape mismatch: wrong destructure arity, no such record field, or a tuple index out of range. | `rec.missingField` |
| `WS024` | *(warning)* An asset/prefab reference inlined into a constant array initializer is silently dropped. Build the array with `.push(...)` in an exec handler. | `var s: entity[] = [$Type/Name]` |
| `WS026` | A map literal used somewhere other than initializing or assigning a Map variable, or filling a `Map` field of a record literal that initializes or is assigned to a record variable. `{}` where a map is expected is the empty map literal, valid in those places and as a map argument. | `foo({ "a": 1 })` |
| `WS027` | Assigning a whole array or map from a non-literal is unsupported; copy it with `.copyFrom(src)`. This covers a record's container field (`rec.xs = ys`, `rec = { xs: ys, n: 1 }`) and a handler-local record `var` initialized that way. A record array or map field copied from another record copies column by column instead. | `m = otherMap` |
| `WS039` | Invalid map key type: a `Map<K, V>` key must be `int`, `string`, an object (`entity`/`character`/`controller`) or a `brick`. | `var m: Map<float, int>` |
| `WS044` | An array/map method (`.push`, `.set`, `.remove`, …) called on something that isn't an array or map. The receiver didn't resolve to a container, so the operation would otherwise be silently dropped. Also reported for an array/map argument (`append`, `copyFrom`, `slice`, `keys`/`values`, `sortMultiple`) that does not resolve to a container; for a mutating operation on a constant array or map (a `const`, a `let` bound to one, or a literal argument), including through a `mod` or `chip` parameter (`T[]` or `*T[]`), an `if` choosing it, or as a `sortMultiple` parallel array; and for storing a constant container's reference where it could be read back and changed (an array or map element, a `*T[]` var, an output port, event data, a chip that changes a parameter it was passed), for an array literal with a non-constant element used as a value, which names no storage; and for an index (`r[0]`) on a record, tuple or enum. A constant container is immutable, so its compile-time value and its runtime contents can never disagree; copy it into a `var` to store or change it. | `let x = 5` then, in an exec handler, `x.push(1)`; or `const t = [1, 2]` then `t.push(3)` |
| `WS087` | A whole-record assignment whose container fields each copy from another container field the same assignment replaces, in a cycle. Scalar fields are read before any field is written, but a container is copied when its write runs, so one of them would copy contents already overwritten. Copy one side into a separate variable first. | `h = { pts: h.qs, qs: h.pts }` |
| `WS088` | An array literal assigned to an array spreads that same array anywhere but first (`xs = [0, ...xs]`). By then the rebuild has cleared or added to the array, so the spread would not add the old contents. A leading `...xs` keeps them and appends the rest; to put values in front, use `xs.insert(0, v)`. | `xs = [1, ...xs]` |
| `WS089` | A whole-record assignment copies (or spreads) a container into a field when that field is written, after every source is evaluated, and a later field's code writes that container first, or writes through a ref, an array element or a parameter pin, which may reach any container of its kind. A call that writes nothing the literal copies is fine. Put the field with the call first, or run the call in a `let` before the assignment. | `h = { ps: src, n: popper() }` |
| `WS098` | A `var` declared with an empty `[]` or `{}` and no type, or with an array literal whose only elements are spreads of empty literals (`[...[]]`). An empty literal says nothing about what the container holds, and its gate stores one element type, so annotate the declaration (`var x: int[] = []`, `var m: Map<string, int> = {}`). An array literal with elements needs no annotation: it takes the join of its elements. An error that follows from the unknown element type is not reported again; any other error, in the same statement too, is. | `var x = []` |

## Loops

`for` iterates a range or an array/map variable, but only on an exec chain.
See [Statements](statements.md).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS074` | A `for` loop outside an exec context. The loop shell has an `Exec` input, so it must sit inside an `on` handler or an exec `mod`/`chip` body. | `if true { for v in arr { } }` at the top level |
| `WS075` | A `for` loop's collection source is not an array/map variable. It binds by reference, and a computed value (a call, an expression) has no storage to bind. Take the collection as a `T[]`/`Map<K, V>` parameter, or fill a declared `var` array/map and iterate that. | `for v in make() { }` where `make()` returns `int[]` by value |
| `WS076` | A `for` loop binding that doesn't fit its loop: more names than the loop has slots, a `{ .. }` pattern on an array or map (whose slots are never records, so there is nothing to destructure), or, on a range where `{ .. }` names the loop's own slots, a name other than `index`. | `for a, b, c in arr { }` |
| `WS077` | A loop's `.step(...)` (or `count(..., step = ...)`) stride is a constant that is not greater than zero: an integer or a float (a float stride steps by its value, `.step(0.5)`), written or computed from constants (`0.0`, `-1.5`, a `const` of one, `S - 0.5`, or `0.0 / 0.0`, which evaluates to 0 as every constant division by zero does). The message quotes a literal as written. Use `.rev()` to count down instead. | `for i in (0..10).step(0) { }` |
| `WS078` | Control flow that cannot cross a loop plane: `break` outside a loop, `return` inside one, or `continue`, which has no gate to lower to. | `for i in 0..3 { return }` |
| `WS079` | An `await` inside a loop body, including `await Sleep(...)`/`await SleepTicks(...)`. A body is one synchronous sweep within a single tick, so it cannot suspend. Use a back-edge loop (`emit` + `await`) for work that needs to spread across ticks. | `for i in 0..3 { await Sleep(_, delay = 1.0) }` |
| `WS080` | `@flat` over a program containing a `for`. A loop's plane is an execution boundary, not a visual grouping, so flattening it would compile a different program. | `@flat` with any `for` in the file |
| `WS081` (warning) | A statement after `break` in the same body never runs, because the `break` already ended the block's exec chain. Reported at the dead statement, not the `break`, and in place of the generic WS058 that construct would otherwise get blamed with. | `for i in 0..3 { break\n t = t + 1 }` |
| `WS082` | A `for` loop's array/map source has a record-typed element, key or value. A record is stored as one container per field, so there is no single reference for the loop's driver pin, in either binding form. Iterate the index instead (`for i in 0..arr.length()`) and read fields in the body. | `for p in pts` or `for { x, y } in pts` where `pts: Point[]` |
| `WS083` | A range method used where it cannot apply: `.step(...)`/`.rev()` on an array or map source, or a second `.step(...)`/`.rev()` on one range (the outer one replaces the inner rather than combining with it). Reported at the method itself. | `for v in xs.rev() { }` / `for i in (0..10).step(2).step(3) { }` |

## Functions (`fn`)

A `fn` compiles once to a function definition plane, run by a Function Call
gate at each call site. See [`fn`](statements.md#fn-function).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS111` | A `fn` called outside an exec context. A Function Call fires on its `Exec` input, so the call must sit in a handler or a `mod`/`fn` body, or pass `exec = trigger`. | `out o = f()` |
| `WS112` | A record parameter of a `fn` given an argument with no record fields to pass. | `f(5)` for `fn f(p: { a: int })` |
| `WS113` | A `fn` body that reaches across its plane's wall with something other than outer storage or another `fn`: a computed value (an `in` port, a `let`), or a value or exec leaving the plane. Storage (`var`, array, map) goes through handles inside the plane, and a call reaches its `fn` by a wire from the definition; anything else has no way in or out. Pass it as a parameter, or return it. | `in k: int` then `fn f() -> int { return k }` |
| `WS114` | A declaration one function plane cannot hold, or a misplaced default value: a `fn` with type parameters, `...rest`, a `const` parameter, an `exec` parameter, a tuple or enum parameter, a record parameter holding a record, or a record, tuple, enum or `exec` result; a default value on a `chip` parameter, or a parameter without a default after one with a default. | `fn g<T>(x: T) -> T`, `fn h(a: int = 1, b: int)` |
| `WS115` | A `fn` with more than 8 inputs or 8 results. A Function Call has eight of each, and a record parameter takes one per field. | a `fn` with nine `int` parameters |

## Compile-time constants (`const`)

See [`const` -- Compile-Time Binding](statements.md#const-compile-time-binding).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS046` | Not a compile-time constant: the value names a runtime value, a call to a mod that isn't `const mod`, an unsupported syntactic form, an out-of-range compile-time array index, or a missing compile-time map key/record field. The message names the actual offender. A `const mod` the file does not declare or import is the call's `WS002` alone. | `in live: int` then `const n = live + 1` |
| `WS047` | The certified evaluator refuses to compute the value even though every operand is constant: integer overflow, a non-ASCII string operand, or an uncertified gate/operand combination. The value is computable in principle, but the compiler will not guess it. | `const s = "café".ToUpper()` |
| `WS048` | Const evaluation gave up: the call chain is too deep or took too many steps (guards a runaway or self-referential `const mod` chain against a stack overflow, since a `const mod` calling itself type-checks fine). | a `const mod` that calls itself, reached from a constant-only position such as a custom-event channel name |
| `WS049` | A record or enum can't be stored or passed in this shape. A stored record decomposes into one backing gate per field, so four shapes are rejected. A field whose type has no storable value: a reference (`*T`), `zone`, `teleport`, prefab reference, or `exec`. A record or tuple stored in an array or map (`var hs: H[]`, `Map<K, H>`, `(int, int[])[]`, or a record variable's `H[]` field) that contains an array or map at any depth, since each field becomes one column and a column can't hold a container. An array or map of records (or tuples, or enums with payloads), or a record holding an array or map of records, tuples or enums at any depth, crossing a port (an `in`/`out` port or a `chip` parameter or output), since it is several columns and a pin carries one reference; a `mod` parameter is fine. An enum crossing a port where it would need more than one wire: an enum with a payload at any port, or any enum inside a record or tuple port or at a `chip` parameter or output (only a payload-free enum, a built-in game enum included, crosses a top-level `in`/`out` port, as its tag, alone or as an array or map element); a `*E` chip parameter and a `mod` parameter are fine. | `type T = { z: zone }` then `var t: T`; `type H = { xs: int[] }` then `var hs: H[]`; `in ps: P[]`; `in e: E` for an `enum E { A, B(int) }` |
| `WS050` | A method with no per-field meaning was called on a record array or map. A record container is stored as parallel per-field gates, so `sort`/`shuffle` (would desync the fields), the aggregates (`sum`/`min`/`max`/`average`), `find`, and the dual-container ops (`append`/`copyFrom`/`slice`/`values`) are not available. Use `pts[i]` / `m[k]` for element access. | `var pts: Point[]` then `pts.sort()` |
| `WS051` | `null` was used for a type that has no null value. `null` adopts its target type and produces that type's zero / unset, which is valid for a number, `bool`, `string`, `vector`/`rotator`/`quat`/`color`, or an entity/character/controller, but not for a container, record, or reference-only type (which have their own empty forms). | `var a: int[] = null` |
| `WS052` | A `...rest` variadic parameter was declared on a `chip`. Only a `mod` may be variadic: a mod inlines per call site (so the trailing args are captured into a compile-time tuple), while a physical microchip instantiates once and cannot vary its pin count per call. Change the `chip` to a `mod`. | `chip C(a: int, ...rest)` |
| `WS053` | A plain `emit X` is followed, in the same chain, by an `await X` on the same signal. The emit fires in the current tick, but the `await` is what arms the resume and only arms afterward, so the await never catches that kick and the chain parks forever. Use `buffer emit X`, which lands the next tick after the await is armed. | `emit go` then `await go` |
| `WS055` (warning) | `let x = await CustomEvent(...)` captures the event's data but its type could not be determined, so the wire defaults to a float and mis-delivers non-float data. Annotate the binding (`let x: int = await CustomEvent("c")`). A tuple `let (p, t) = ...` positional capture has no annotation surface, so capture typed values one at a time or receive them with a handler (`on CustomEvent("c") -> (p: T, ...)`). | `let foo = await CustomEvent("c")` |
| `WS056` | `let x = await sig` binds a value from a signal that carries no payload. A bare `emit sig` fires the signal without a value, so there is nothing to capture and the binding would read the signal's exec pin as garbage. Emit a value (`emit sig = ...`), capture a live value at resume (`let x = await expr on sig`), or drop the `let`. | `let sig: exec` then `let v = await sig` |
| `WS057` | `emit X` targets something that is not an `out` port or a `let ...: exec` signal reachable here, so it would compile to nothing. An input port, a `var`, or an `out`/signal declared outside an enclosing named `chip` (whose fresh scope can't see it) is not a valid target, and neither is a bare `emit` of a value output (any `out` or output that is not `exec` or `any`, at the top level, in a `chip` or in a `mod`), which carries only its value: write `emit X = value`. | `in go: exec` then `emit go` |
| `WS058` (warning) | An exec statement never runs because nothing triggers it. An assignment (or other exec work) in a pure position is dropped: a mod or chip body used as a value (`let x = f()`), or a statement outside any `on` handler. So is one after a `buffer emit` on its chain (earlier in the block, ending both branches of an `if`, or ending a mod it calls), since the emit closes the chain; an `await` right after it still arms. The structural form of the same problem is a gate whose `Exec` trigger input has no incoming wire. Run it in an exec context, or pass `exec = <trigger>` to the call. | `mod f() { x = x + 1 }` then `let d = f()` |
| `WS118` (warning) | An `await` inside an `if` whose signal is declared outside that branch, with an emit of the signal after it on the same chain. Only an emit of a signal declared in the await's own branch re-arms the await, so that back-edge finds it unarmed and the loop runs once. Declare the signal inside the branch, or move the loop to a handler of its own. | `let s: exec` then `on go { if c { buffer emit s\n await s\n buffer emit s } }` |
| `WS059` | `Change`/`Changed` (or an `Edge` detector) is watching a value it can't observe. A change/edge detector watches a single wire value. A reference or container (a `Map`, an array, a `*T` ref, a `zone`/`teleport`) has none, so `Change(m)` compiled to a dead gate. Watch a scalar the container produces instead. | `in m: Map<int,int>` then `on Change(m) {}` |

## Gate & event config

Some gates and events take **config args** that bake into gate data rather than
wiring in as ports (see [Built-in Events](statements.md#built-in-events) and
[Custom Events](builtins.md)).

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS028` | Invalid gate/event config: an unknown enum member, an out-of-range int, a missing required config field, or a non-constant value for constant-only config. Also a constant gate setting the gate's data field cannot hold, however it is spelled: a number outside the field's range (`SleepTicks(_, delay = 1e30)`, `fontSize = 5000000000`) or an enum index the field has no value for (`justify = 3.0` as `justify = 3`); the message names the setting and what its field holds. Likewise a float constant that is the whole value of a place typed `int` (see `WS106`) but lies beyond the range of an `int` (`n = 1e30`, `Opt<int>` given `-1e30`, or `9223372036854775807.0`, which as a float is 9223372036854775808): no `int` holds it. Likewise the negation of the least `int` (`-(-9223372036854775808)`, `-K` of a constant holding it), which makes 9223372036854775808. A destructuring `let` binds runtime names even when its source is itself constant, so only the `const` spelling of one satisfies a constant-only slot; the `let` is rejected here rather than silently baking an empty value. | `on ChatCommand("greet", description = someVar)`; `let { chan } = src` then `SendCustomEvent(chan, n)` |
| `WS030` | *(warning)* Custom-event sender/receiver type mismatch: a send's data value type disagrees with the receiver's type on the same channel. A receiver type written through an alias is the type the alias names. A constant is sent as the type it has, so a number constant of the other number type is a mismatch too (`SendCustomEvent("c", 2.5)` or `if c then 2.5 else 1.5` to `amount: int`). The send is checked against every receiver of the channel, a typed `let v: T = await CustomEvent("c")` capture included (it takes the first data value as `T`). A receiver param with no annotation takes its type from the first send on the channel in source order, so two sends of different types disagree with each other; the message names that first send. | sender sends `float`, receiver declares `amount: int`; or `SendCustomEvent("c", 1)` then `SendCustomEvent("c", 2.5)` to an unannotated receiver |
| `WS042` | *(warning)* A `CustomEvent`/`GlobalCustomEvent` handler param has no type annotation and no in-unit sender to infer its type from, so its data defaults to `float`. Annotate the param, or add a matching `SendCustomEvent`/`SendGlobalCustomEvent` in the same unit, to silence. | `on CustomEvent("dmg") -> (amount) { }` with no in-unit `SendCustomEvent("dmg", …)` |
| `WS045` | *(warning)* A custom-event send's data argument has no concrete type (`any`, or an `Opaque(...)` that erased its input's type). The send emits the float variant regardless, so it won't match a receiver that declares a real type. Give the value a typed binding (e.g. a `var` of that type) instead of `any`/`Opaque(...)`. Unlike WS030 this needs no in-unit receiver to visit. | `SendGlobalCustomEvent("a", who, Opaque(true))` |

## `any`

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS032` | *(warning)* An `any` annotation on a non-storage position (port, param, `let`, output). Prefer a [generic type parameter](types.md#any-vs-a-generic-parameter), which keeps the type. | `mod f(a: any) -> any { return a }` |

## Internal errors

| Code | Meaning | Trigger |
|------|---------|---------|
| `WS094` | Internal compiler error: after lowering, a wire names a node that no longer exists, or reaches a node in another chip without going through a boundary pin (a constant read from another chip included). The message names the compiler pass that left it. Also an exec pin (an `ExecOut`, an `exec` input or pin) wired straight into an input that takes a value other than a `bool`: the message names both gates and the line, as the gate would read 0. An input whose type is `bool` (a branch or select condition, a logical gate's input, the value written to a `bool` variable, array or map) reads an exec as true for the tick it fires, as [Logical Operators](expressions.md#logical-operators) documents (type checking still rejects storing an exec, or passing it as a `bool`, written directly: `WS003`), and a custom-event data pin takes one with `WS045`. Also a write the checker accepted that lowered to nothing, or to fields its target does not have, such as a returned or assigned record or enum value, and a call that instantiates a generic with type arguments the checker never checked its body with. These are compiler bugs, not the program's fault: please report them with the program that caused them. Compiling would otherwise ship a gate that reads 0 or fail at emit with `dropped wire`. One case is the program's own error instead: a rerouter pin of no type passes an exec on, and the check follows it to the input it reaches; where that is a value input other than a `bool` one (`out o: any` emitted as an exec, then read as a number, text or color), the program reads an exec as a value, and the message says so rather than calling it a compiler bug. Give the pin a type, or test the exec with `if`. Every such wire is reported. Every `WS094`, the program's case included, is dropped when the compile reports any other error. | Compiler bug: none, no program should reach it. The program's error: `chip C() { out o: any` with `on go { emit o } }`, then `let k = C()` and `n = k.o` |

## Silencing a warning

Sometimes a warning reports something the program does on purpose. Two
comment directives turn one off by code:

```wirescript
import { helper } from "utils" // ws-ignore-line:WS014
```

`ws-ignore-line` covers the line the comment sits on. Written on its own line it
covers the line below instead, so a long statement can carry the note above it
rather than past its right edge:

```wirescript
// ws-ignore-line:WS014
import { helper } from "utils"
```

`ws-ignore-file` covers the whole file no matter where it sits, and by
convention goes at the top:

```wirescript
// ws-ignore-file:WS014
import { helper } from "utils"
```

Both forms take a comma-separated list (`ws-ignore-line:WS014,WS032`), and both
silence every warning on their line or in their file when written bare
(`// ws-ignore-file`). The directive is recognized as any whitespace-separated
word of the comment, so prose saying why it is there can sit on either side of
it, and appending one to a line that already ends in a comment works. A trailing
`:` with no code after it is read as a typo, not as the bare form.

A directive travels with the file it is written in. A warning reported against
an imported module is silenced by a directive in *that* module, not by one in
the file that imported it.

**Errors are never silenced.** A directive naming one has no effect. The program
still does not compile, and hiding the reason would only make the failure
harder to find. The same goes for the editor, whose "Ignore WSxxx on
this line" and "Ignore WSxxx in this file" quick fixes write these directives
for you, and are offered on warnings only.

---

Parse- and lexer-level problems (an unexpected token, an unterminated string or
comment, a construct lowering doesn't yet support) are reported under the
separate code **`WSP001`**, outside the `WS0xx` numbering above.

An expression the type check accepts but lowering cannot build yet would
compile to a placeholder gate that computes nothing. In a program with no other
error that placeholder is a `WSP001` error naming the expression ("this
expression is not supported by the compiler yet: the method call `.twice()`"),
so nothing is emitted, even when only a variable nothing reads holds it. The
message says how to rewrite it: a method the value's type does not list in this
reference is replaced by one it does. Beside another error it is a warning,
since the placeholder is most likely that error's consequence. Neither
`ws-ignore` directive silences it.
