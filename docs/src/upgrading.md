# Upgrading

Wirescript's syntax and gate catalog evolve as the game updates. This page
collects the **breaking changes** and how to migrate existing `.ws` code. The
complete, per-version log (every change, not only breaking ones) lives in the
repository's [`CHANGELOG.md`](../../CHANGELOG.md).

<!-- toc -->
## Contents

- [2.0.0](#200)
- [1.1.0](#110)
<!-- /toc -->

## 2.0.0

### `DisplayText` centers text unless `justify` says otherwise

The game's EA 3.2 update changed the default justification of a Display Text
gate from `Left` to `Center`, and an unset `justify` takes the game's default.
Text laid out to start at its position now centers on it instead, moving left
by half its width. Say `justify = Left` wherever the layout expects that:

```wirescript ignore
ctrl.DisplayText("Score", positionX = -300.0, positionY = 40.0)                  // before: left-aligned
ctrl.DisplayText("Score", positionX = -300.0, positionY = 40.0, justify = Left)  // after
```

An unset `pivotX` follows the justification (`Left` places the text's start at the
position, `Center` its middle, `Right` its end), so only `justify` needs to change.

### An empty `[]` or `{}` needs a type

A `var` initialized with an empty literal and no annotation is `WS098`: the
literal says nothing about what the container holds. Annotate the declaration.
A literal with elements still takes its type from them.

```wirescript ignore
var hist = []                // before: an array of floats, whatever was pushed
var hist: float[] = []       // after
var names = ["a", "b"]       // unchanged: a string[]
```

### A string method on an unbounded type parameter

A generic body is checked for every type its parameter may be, and an
unbounded `T` may be a `bool`, so `v.Length()` on a `v: T` is `WS003` even
when every call passes a string. It compiled before. Bound the parameter:

```wirescript ignore
mod size<T>(v: T) -> int { return v.Length() }          // before
mod size<T: string>(v: T) -> int { return v.Length() }  // after
```

### A file calls only the mods it declares or imports

Imports merge every module into one program, and a call used to reach any
`mod` or `chip` in it by name. Now a call, `f(x)` or `x.f()`, reaches only
the mods its own file declares or imports. Two programs that compiled before
are errors:

- an importer calling a library's private helper, present only because an
  imported body calls it: `WS002` for `f(x)`, `WS036` for `x.f()`. This
  includes a helper of a named or aliased import (`import { g as gg }` then
  `f(k)`, where `g` calls `f`). Import the helper by name or import the whole
  module.
- a library body calling a mod its importer declares: `WS002` or `WS036`,
  located in the library. A module can't import the file that imports it, so
  move the mod into the library, or into a module both files import.
- a module calling a mod of another module that only the importer imports
  (`main.ws` imports `a.ws` and `b.ws`, and `a.ws` calls `b.ws`'s `bf`):
  `WS002`, located in `a.ws`. Import `b.ws` into `a.ws`.

```wirescript ignore
// lib.ws
mod scale(v: float) -> float { return v.hook() }   // before: the importer's hook
mod hook(self: float) -> float { return self * 9.0 }
mod scale(v: float) -> float { return v.hook() }   // after: the library's own
```

### A record or tuple written to an output with no type

An output with no type is one pin, so a record or tuple written to it drove
nothing. It is now `WS105`. Annotate the output with the record's type, which
the message spells out:

```wirescript ignore
on go { out z = pv }       // before: an undriven pin
on go { out z: P = pv }    // after: a pin per field
```

### An expression the compiler cannot build is an error

An expression the type check accepts but lowering cannot build compiled to a
gate that computes nothing, with a `WSP001` warning. In a program with no other
error it is now a `WSP001` error that names the expression and how to rewrite
it. This applies even when the only thing holding it is a variable nothing
reads, and neither `// ws-ignore-line` nor `// ws-ignore-file` silences it. For
a method the value's type does not list (`k.nosuch()`, `a.ToString()`), the
message suggests one this reference lists for the type (a number becomes text
through an interpolation, `"${a}"`).

A record or tuple held by a `var` with no type (`var v = (a, 1)`,
`var r = m.get(k)` on a map of records) is held as its type, field by field,
the same as in an annotated `var`. Before, it was a placeholder or read its
fields as a vector's.

### A name declared twice at the top level is read by position

A file's top level may declare a name again (`const ch = "a"`, later
`const ch = "b"`, or a `let` beside a `var` of the name), and `WS108` warns at
the later one. Every read now names the declaration written before it, for
values and for constant-only settings alike. This covers a `mod`, `chip` or
`const mod` body (read as if where it is declared, so it gets the first
declaration when it is above all of them) and storage (declared ahead of
everything, so read as if above every declaration). A library imported flat or
by name reads its own duplicates the same way, however the program reaches it.
A library imported with `import * as` reads its last declaration everywhere
(`WS108` says so). A `var` initializer reads the declaration before the `var`,
so `let k = 5` then `var k: int = k` starts the var at 5 again. Before, a
constant-only setting (an event name, a `Sleep` delay, a folded `if`) and
another constant's initializer read the first declaration, while a value read
the latest, so one handler could store `"b"` and send on `"a"`.

```wirescript ignore
const ch = "a"
const ch = "b"                                // WS108
on go { s = ch
 SendCustomEvent(ch, 1) }                     // before: s = "b", sent on "a"; after: both "b"
```

### Constants that read as integers

A negated integer constant (`--2`, `-(-K)`, `-K`) is an integer in both fold
modes. With folding on it was stored as a float
(`MathNegate` gives a float). Negating the least `int` is `WS028`.
A float constant used as an array or string index, a range bound or a `count`
is the integer it truncates to (`WS106`, or `WS028` when no `int` holds it), as
in every other `int` place. Before, `0..2.5` ran three times. A `.step` stride keeps a
float value, and one that is not greater than zero, integer or float, is
`WS077`. An integer literal given to a `bool` parameter is the bool it names,
and a bool literal given to an `int` parameter the integer.

### `const` of a name that hides a constant

A parameter, local or loop variable named like an outer constant hides it, so
`const q = K` of such a name is `WS046`: the name is not a constant there. It
compiled with the outer constant's value. Use `let`, or rename the binding.

```wirescript ignore
const K = 4
mod g(K: int) -> int { const q = K  return q }   // before: q was 4
mod g(K: int) -> int { let q = K  return q }     // after: q is the argument
```

### A signal is declared with `let`

A signal is declared with `let s: exec`, which also accepts an alias of `exec`
(`let s: Signal` for `type Signal = exec`). An initializer on it is `WS101`:
it was dropped, so `s` fired only on `emit s` and never when the signal it
named did. To run code when another signal fires, write `on <signal> { .. }`.
`const s: exec` is a single `WSP001` asking for a value (it was `WS046`). A
`const` is a value known while compiling, never a signal.

```wirescript ignore
let done: exec = go2     // before: `= go2` was dropped
let done: exec           // after
```

### A `var` initializer reads the names around it

A `var` initializer is evaluated before the variable exists, as a `let`'s is.
`var K = K + 1` reads the outer `K`. It used to read the variable being
declared, and the reads after it saw the value from before the initializer ran.
With no outer name of its own, `var x = x + 1` is `WS002`, which says the
initializer cannot read the variable it declares. Declare it without one
(`var x: int`), or initialize it from another value. In a `mod` or `chip` body,
a read above a `var` of the same name, and that `var`'s own initializer, read
what the name means there (an outer binding, a constant, an enum or a bare
variant), as the type check always did. They used to read the body's own
`var`. For example, `var Blue: int = EnumToInt(Blue) + p` reads the variant
`Blue`. The idiom `static var z: int = z`, whose initializer read the variable
being declared, is `WS002` the same way. Write `static var z: int` instead,
which keeps its value across calls with no initializer.

```wirescript ignore
const K = 4
on go { var K: int = K + 1 }   // before: 0 + 1 / after: 4 + 1
static var z: int = z          // before: read z itself / after: WS002
static var z: int              // after
```

### A record where one value goes

A record of the program's own (a record literal, or a parameter, `var`, field,
element or call of a record type) written where one value goes is `WS003`,
and the message names a field to read. This covers a `return`, `emit` or `out`
of one value, an assignment, an argument, an operand, a comparison, a
condition or an interpolated part. It used to compile. A `return` or `emit` to
a mod's `int` output stored the exec wire, `mk(a) + 1` and `"${mk(a)}"` read
the exec wire too, `m.get(k) + 1` on a map of records read one field, and
other places were placeholders. A multi-output read (`x = Edge(b)`,
`n = fs.max()`, a call of a `mod` or `chip` with several outputs, flat,
through `import * as` or as a method) is not a record of the program's own.
Where one value goes, it gives its first output, converted as a value of that
output's type is. This holds whether it is written inline or held in a `let`,
a record field or a tuple (`let t = { m: fs.max() }` then `f = t.m`),
returned, sent or chosen. One whose first output is itself a record
(`-> (p: R, q: int)`) is `WS003` there too, because the first output is read
once, never a field of it. A generic body is checked for every member of its
bound, so one that reads such a value as an operand (`m.get(k) + 1` in
`mod f<T: R | int>(m: Map<string, T>)`) is `WS003` at its definition even if
no call uses the record member. Narrow the bound. A tuple pattern takes a
multi-output read's outputs in the order they are declared
(`let (v, e) = fs.pop()` binds `Value` then `IsEmpty`).

```wirescript ignore
mod C(q: Q) -> int { return q }     // before: the caller stored the exec wire
mod C(q: Q) -> int { return q.a }   // after
```

### New errors for programs that compiled

Each of these compiled before, to something other than what the source says:

- `WS097`: an `if` or `match` expression of a record, tuple or enum with an
  arm that writes, sends or returns. Use an `if` or `match` statement whose
  arms assign a `var`.
- `WS099`: `return <value>` in a top-level handler, which drove the file's one
  `out` from load time. Write `emit o = value`, then a bare `return`.
- `WS100`: a record type read as a value (`let c = Box { v: n }`). Give the
  record literal its type instead: `let c: Box = { v: n }`.
- `WS103`: values no one type holds written to an output with no type
  (`out z = 5` and `out z = "six"`). Annotate the output.
- `WS003`: a string method on a value that is not a string (`vv.Length()` on a
  vector), which read the text the value formats to. A vector's length is
  `Magnitude(vv)`, an array's count `xs.length()`.
- `WS088`: an array assigned a literal that spreads the same array anywhere
  but first (`xs = [0, ...xs]`), which emptied it. Use `xs.insert(0, v)`, or
  put `...xs` first.
- `WS095`: a block arm (`A => { k = 5 }`) of a `match` used as a value, whose
  value was the exec pin. Write the arm as a value, end it in `return`, or make
  the `match` a statement whose arms assign a `var`.
- `WS057`: a bare `emit o` of a value output, at the top level or in a chip,
  which wired the exec into the value. Write `emit o = value`.
- `WS021`: a `mod` or `chip` body reading a top-level `let` declared below
  the call, which read 0. Declare the `let` above the first call.
- `WS044`: changing a constant array or map (a `const`, a `let` bound to
  one, or an array or map literal passed as an argument), also through a
  parameter, or storing its reference where it could be changed. Hold the
  values in a `var` and change that.
- `WS049`: an enum at a `chip` parameter or output, or nested in a record or
  tuple at a port, which was an unwired pin. Only a top-level `in`/`out` of a
  payload-free enum carries one, as its tag. Pass it to a `mod`, pass a chip a
  `*E`, or pass the tag and payload as plain values.
- `WS004`: arithmetic on a `*T` field of a call's result or of a record an
  `if`/`match` chooses (`mk().x + 1`), which read the var the field names.
  The field is a ref there, as it is everywhere. Read through it with `*mk().x`.
- `WS090`: a `mod` or `chip` with a value output and a path that ends before
  the output has a value (`if c { return a }` with nothing after), which gave
  the value another path or the previous call set. Return a value on every
  path, or give the `if` an `else` that returns.
- `WS003`: an annotated `buffer` whose initializer is not of its type
  (`buffer b: int = "abc"`), which buffered the value as it was. Initialize
  it with a value of its type.
- `WS028`: a constant gate setting its field cannot hold, however it is
  written: a number outside the field's range (`SleepTicks(_, delay = 1e30)`,
  `fontSize = 5000000000`), which wrapped, or an enum index the field has no
  value for (`justify = 3.0`), which was stored as it was. Write a value the
  setting holds.
- `WS002`: a library body naming a type alias that only the file importing it
  declares, which read the importer's alias. Declare the alias in the library,
  or import it there.
- `WS107`: a `type` or `enum` named like a builtin type (`type float = int`),
  or named `Array` or `Ref` with one type parameter (`type Ref<V> = V[]`),
  which was ignored. Give it another name.
- `WS002`: a library body naming an enum that only the file importing it
  declares (`var e: Dir`, `Dir.B`), which read the importer's enum. Declare
  the enum in the library, or import it there. A library's own alias now also
  wins over an importer's enum of the same name, which it lost to before.
- `WS028`: a float constant beyond the range of an `int` in a place typed
  `int` (`n = 1e30`), which was stored as the largest `int`.
- `WS002`: a library body reading a top-level value (a `const`, `let`, `var`
  or port) that only the file importing it declares, which read the
  importer's value. Declare the value in the library, or import it there.
- `WS013`: two declarations of one name in one anonymous `chip { }`. Rename
  one. A top-level anonymous `chip { }` may still declare a name the top level
  does (see [Anonymous chips](chips.md#names-a-chip-shares-with-the-top-level)).
- `WS013`: two outputs of one `mod` or `chip` with one name. Rename one.
- `WS003`: a multi-output read returned whole to several outputs, some but
  not all named like the read's outputs (`-> (Found: bool, Value: float)`
  returning `fs.min()`), which filled them by position, so `Value` carried
  `IsEmpty`. Name every output like one of the read's, or none of them to
  fill them in order.

A float constant that is the whole value of an `int` place is the integer it
truncates to, in a `const` and a `let` as in a `var`, with a `WS106` warning.
`const K: int = 2.5` reads 2, and `n = 2.5` for an `int` var writes 2. Both
kept 2.5 before. The constant is an integer wherever it then goes (`var u = K`
is an `int`), and a chip argument and a field of a record or tuple literal
truncate the same way (a record or tuple
literal with such a field was `WS003`). The data of a custom event is not such
a place. A value is sent as the type it has, and a receiver that declares the
other number type still warns `WS030`. A constant gate setting stored as a
different value than the one written (`SleepTicks(_, delay = 0.999)` waits 0
ticks) now warns `WS106` too. Write the stored value to silence either.

A line that starts with `*` begins a new statement (a deref). Before, `n = a`
followed by a line `*h` was `n = a * h`. A deref nothing uses warns (`WSP001`). To
multiply across lines, end the first line with `*`.

A `mod` or `chip` body uses the names where it is declared: a caller's own
local named like a `var` the body uses no longer stands in for it. The call
uses the outer `var`, and the local gets a `WS091` warning. Rename the local,
or pass it to the call.

A failed compile on the command line now ends each error with where it is,
`(file.ws:line:col)`. A script reading the output sees that suffix.

## 1.1.0

### Event handlers bind outputs with `->`

An event's data outputs are captured in a trailing `-> (…)` (tuple, positional;
the cleanest form) or `-> { … }` (record, by field name), not inside the
event call. The event call's parens now hold **config/inputs only** (a
custom-event channel, `isObject`, `zone`, `interval`, a `ChatCommand` name/help).
Binding outputs inside the call is an error.

```wirescript ignore
// before
on CharacterDied(character, killer) { … }
on CustomEvent("dmg", amount: int) { … }
```
```wirescript ignore
// after
on CharacterDied() -> (character, killer) { … }
on CustomEvent("dmg") -> (amount: int) { … }
```

Config stays in the parens; only data moves to `->`:

```wirescript ignore
on ChatCommand("greet", "Greets you") -> (controller, args) { … }
on ZoneEntered(zone = z) -> (character) { … }
```

### Event triggers are calls: `on RoundStart()`

An event trigger is written with `()`, uniform with the `on <call> -> (…)`
model. The no-parens form, a handler head (`on RoundStart { }`) or a
captured-event alias (`let x = on RoundStart`), is now an error. The `()` makes
it clear the event is a gate/call.

```wirescript ignore
on RoundStart { … }        // before   ->   on RoundStart() { … }        // after
let x = on RoundStart      // before   ->   let x = on RoundStart()      // after
```

### Custom-event data types move to the capture (and are inferred)

Write a slot's type in the tuple capture, or omit it and let the compiler infer
it from the matching in-unit `SendCustomEvent`. When no sender supplies a type,
the slot defaults to `float` and warns **WS042** (replacing the old `WS029`
"annotate this param" lint).

```wirescript ignore
on CustomEvent("dmg") -> (amount: int, source) { … }
//                              ^annotated  ^inferred from the sender
on CharacterSpawned() -> (ch) { SendCustomEvent("dmg", 5, ch) }
```

### `emit` is for exec signals, not data

Set a data output with **`out name = value`** (in a handler or a chip/mod body)
or **`return value`** (a single-output `mod`). Reserve `emit` for firing an exec
signal (`emit done`) or ferrying a payload with a local signal you `await`
(`emit loop = value` … `await loop`).

### General-call triggers

`on <call> -> (…)` works for any exec-producing call: a `mod`/`chip` call, or a
gate driven by an `exec =` input (`on Foo(exec = go) -> (out)`). `on`
auto-extracts the call's exec output; a general call with no exec output (or an
`exec =` with nowhere to attach) is **`WS043`**.

### New whole-grid events

`on WholeGridInteracted() -> (character, held)` fires when the grid is interacted
with; `on WholeGridTargeted() -> (character, damage, weapon, weaponName)` fires
when it is hit.

---

For the full, per-version history (features, fixes, and migration notes), see
[`CHANGELOG.md`](../../CHANGELOG.md) at the repository root.
