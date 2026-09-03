# Enums

An `enum` is a named type with a fixed set of **variants**. Each variant can be
a bare unit (like a C enum member), or it can carry a payload -- turning the
type into a tagged union. A value of an enum type always knows which variant
it currently is; `match`, `if let`, and `let ... else` are how you branch on
that and pull the payload back out.

Unlike [`type` aliases](types.md#record-types), which are structural, `enum`
is **nominal**: two enums with identically-shaped variants are still different
types.

<!-- toc -->
## Contents

- [Declaration](#declaration)
- [Construction](#construction)
- [`.Discriminant`](#discriminant)
- [`is`](#is)
- [Enum and int conversion](#enum-and-int-conversion)
- [`match`](#match)
- [`if let` / `let else`](#if-let--let-else)
- [Changing a payload](#changing-a-payload)
- [Payloads cannot hold containers](#payloads-cannot-hold-containers)
- [`unsafe`: unchecked payload access](#unsafe-unchecked-payload-access)
- [Generic enums](#generic-enums)
- [Built-in `Option` and `Result`](#built-in-option-and-result)
- [Built-in game enums](#built-in-game-enums)
<!-- /toc -->

## Declaration

Discriminants (the integer tag backing each variant) auto-number from `0`. An
explicit `= N` resets the counter -- the next unannotated variant continues
from `N + 1`. Two variants that resolve to the same value are a
[`WS064`](diagnostics.md) error.

```wirescript
enum Color { Red, Green, Blue }              // 0, 1, 2
enum Status { Idle = 0, Running = 5, Done }  // 0, 5, 6

enum Shape {
  Empty,                          // unit variant
  Circle(float),                  // positional payload
  Rect(float, float),
  Box { w: float, h: float },     // named payload
}
```

A single enum can freely mix unit, positional, and named variants. Whichever
shape a variant is declared with is the shape it must be constructed and
matched with -- see [Construction](#construction) below.

## Construction

Construct a variant with its qualified `Enum.Variant` path: a unit variant is
the bare path, a positional-payload variant takes `(...)`, and a
named-payload variant takes `{ ... }` (the shorthand `{ x, y }` works in a
value position too, same as a [record literal](types.md#record-types)):

```wirescript
enum Shape { Empty, Circle(float), Box { w: float, h: float } }

let c = Shape.Empty
let s = Shape.Circle(5.0)
let b = Shape.Box { w: 1.0, h: 2.0 }
```

Using the wrong bracket form for a variant's declared shape --
`Shape.Circle { }` for a positional variant, or `Shape.Box(1.0, 2.0)` for a
named one -- is a [`WS065`](diagnostics.md) error, so a variant's shape stays
unambiguous for exhaustiveness checking and the LSP's fill action.

### The prelude variants

`Some`, `None`, `Ok`, and `Err` (the built-in [`Option`/`Result`](#built-in-option-and-result)
variants) are available **bare** -- no `Option.` / `Result.` qualifier needed:

```wirescript
let o = Some(42)                     // T inferred = int
let n: Option<int> = None            // annotation fixes T when payload can't
```

`None` and `Err` carry no payload to infer their type parameters from, so a
bare `let n = None` has nothing to pin `T`. Give the binding a type
annotation (`let n: Option<int> = None`) or that's a [`WS063`](diagnostics.md)
error.

## `.Discriminant`

`.Discriminant` yields the variant's tag as an `int`. On a **variant path**
(`Enum.Variant.Discriminant`) it's a compile-time constant that costs no
gates; on a **value** it reads the tag currently stored in that value:

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

out d = Shape.Circle.Discriminant             // compile-time int, = 1
```

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

static var s: Shape = Shape.Circle(5.0)
out matches = s.Discriminant == Shape.Rect.Discriminant   // runtime read vs const
```

`.Discriminant` (and `match`) on a value that isn't an enum is a
[`WS066`](diagnostics.md) error.

## `is`

`value is Enum.Variant` asks whether the value currently holds that variant and
yields a `bool`. It is the discriminant comparison above, spelled the way it
reads:

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

static var s: Shape = Shape.Circle(5.0)
in ready: bool

out round = s is Shape.Circle                  // same gates as the compare above
out other = !(s is Shape.Circle)               // negate with `!`
out go = s is Shape.Rect && ready              // binds like `==`
```

The test compares tags, so a payload variant answers on its tag alone and binds
nothing; [`match`](#match) and [`if let`](#if-let--let-else) are how a payload
comes back out. The right side must name a variant of the same enum: `s is v`
(two values) and `s is Other.Empty` (a different enum) are errors.

A test whose two sides are both compile-time constants folds away, so testing
a `const` value costs no gates.

## Enum and int conversion

Two spellings move between an enum value and its integer tag.

`value.ToInt()` is an **exact alias** for `.Discriminant`: it yields the tag as
an `int`, on both a value and a variant path, and folds to the same
compile-time constant when the receiver is a variant path.

`Enum.FromInt(n)` goes the other way. It builds a value of `Enum` whose tag is
the int `n`, with **every payload slot defaulted to its zero value**. Here `n`
may be a runtime `int`, not just a constant. It sets only the tag, so it is
meaningful mainly for unit-only enums (C-like enums and the built-in game
enums). For a payload-carrying variant the payload reads back as zero. An `n`
that matches no variant's discriminant leaves a value that no `match` arm
covers except a wildcard `_`.

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

in tag: int

static var s: Shape = Shape.Rect(1.0, 2.0)

// ToInt is Discriminant by another name.
out same = s.ToInt() == s.Discriminant        // always true
out circle = Shape.Circle.ToInt()             // compile-time int, = 1

// FromInt rebuilds a value from a (possibly runtime) tag.
let rebuilt = Shape.FromInt(tag)
out kind = match rebuilt {
  Empty => 0,
  Circle(r) => 1,
  Rect(w, h) => 2,
}
```

`EnumToInt(value)` and `IntToEnum(value, wrap?)` are the **gate-backed
twins** of `.ToInt()` and `Enum.FromInt(n)`, for routing through the game's
"Enum to Integer" / "Integer to Enum" gates.

`EnumToInt(value)` **requires an enum argument** (an `int` or any non-enum
is a type error) and yields an `int`. When `value` is a compile-time-known enum
(a variant literal, or a `const` value) it **folds to the discriminant literal**
and emits no gate; a runtime enum value instead routes through the real
`EnumToInt` gate, fed by the value's tag. For Wirescript's record enums the
runtime gate is just reading the tag, but it is emitted so game/native enums go
through the real gate.

`IntToEnum(value)` is the reverse. Its result is an enum whose **concrete
type comes from the use site** (the annotated target or output type), exactly
like `FromInt` and `null`; with no enum-typed context it can't tell which enum
the integer names, which is an error. A constant `value` folds to the enum
record directly; a runtime `value` routes through the real `IntToEnum` gate.
The optional `wrap` clamps an out-of-range tag into range.

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

in tag: int
static var s: Shape = Shape.Rect(1.0, 2.0)

// EnumToInt is the gate-backed twin of ToInt / Discriminant.
out folded = EnumToInt(Shape.Circle(1.0)) // compile-time: folds to 1, no gate
out live = EnumToInt(s)                    // runtime: routes through the gate

// IntToEnum is the gate-backed twin of FromInt; the result's enum type
// comes from the annotated target.
let back: Shape = IntToEnum(tag)           // runtime int -> Shape via the gate
out kind = match back {
  Empty => 0,
  Circle(r) => 1,
  Rect(w, h) => 2,
}
```

## `match`

`match` branches on an enum value's variant, binding any payload as it goes.
It works both as an **expression** (arms are values, comma-separated, and it
compiles to a `Select` tree) and as a **statement** (arms are blocks, and it
compiles to a `Branch`/`Union` tree). Arm patterns use bare variant names --
the scrutinee's enum type is already known, so there's no `Enum.` qualifier.

### Expression form

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float), Box { w: float, h: float } }

static var s: Shape = Shape.Circle(5.0)

out area = match s {
  Circle(r)    => 3.14159 * r * r,
  Rect(w, h)   => w * h,
  Box { w, h } => w * h,
  Empty        => 0.0,
}
```

The arms' result types follow the same
[widening join](types.md#widening-inference) as an `if`-then-else expression.

### Statement form

Statement arms are blocks (no commas between them) and require exec context:

```wirescript
enum Shape { Empty, Circle(float) }

static var s: Shape = Shape.Circle(5.0)
var lastArea: float = 0.0

on ReadBrickGrid() {
  match s {
    Circle(r) => { lastArea = r * r }
    Empty     => { lastArea = 0.0 }
  }
}
```

### Exhaustiveness

A `match` must cover every variant of the scrutinee's enum, or be capped with
a `_` wildcard arm. An uncovered variant is a [`WS054`](diagnostics.md) error
that names the missing pattern(s) in its message; an arm that can never run
because an earlier arm already covers everything it would match is a
[`WS061`](diagnostics.md) warning.

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

static var s: Shape = Shape.Circle(5.0)

out area = match s {
  Circle(r) => 3.14159 * r * r,
  _         => 0.0,
}
```

### Nested patterns

A pattern can nest into a variant's payload, including into another enum, and
exhaustiveness checking follows it down:

```wirescript
enum Opt { Some(int), None }
enum Tree { Leaf(int), Node(Opt) }

static var t: Tree = Tree.Leaf(0)

out val = match t {
  Node(Some(x)) => x,
  Node(None)    => 0,
  Leaf(n)       => n,
}
```

A named-payload pattern can ignore the fields it doesn't need with `..`:
`Box { w, .. }` binds only `w` and drops `h`.

### Scrutinee must be a value, not a port

The scrutinee of a `match` (and of `if let` / `let else`, below) has to be a
`var`, `let`, `static var`, mod/chip parameter, or `const` -- something the
compiler can see as a compile-time **record** of the tag plus its payload
slots. A top-level enum-typed `in` **input port** does not decompose that way
(it lowers to a single scalar wire, not a record), so matching on one directly
emits an unwired placeholder instead of a real Select/Branch tree. If an enum
value needs to arrive from outside the unit, copy it into a `var` first and
match on that:

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }

var s: Shape = Shape.Empty
in setCircle: exec
in radius: float

on setCircle {
  s = Shape.Circle(radius)
}

out area = match s {
  Circle(r)  => r * r,
  Rect(w, h) => w * h,
  Empty      => 0.0,
}
```

(The same gap applies in the other direction: an enum value can't yet drive a
top-level `out` port directly either -- expose a derived scalar, such as
`.Discriminant` or a `match` result, instead.)

## `if let` / `let else`

These are single-variant refutable binds -- shorthand for a `match` with one
real arm.

`if let PATTERN = scrutinee { ... }` runs the `then` block (with the pattern's
bindings in scope) only when the scrutinee is that variant; the `else` is
optional:

```wirescript
enum Opt { Some(int), None }

static var o: Opt = Opt.Some(5)
var result: int = 0

on ReadBrickGrid() {
  if let Some(x) = o {
    result = x
  } else {
    result = -1
  }
}
```

`let PATTERN = scrutinee else { ... }` binds into the surrounding scope
instead of a nested block, which is why its `else` is **required to
diverge** -- it must end in `return` / `emit`, or be an `if`/`match` whose
arms all diverge. That's what guarantees the binding is always available
after the statement. A non-diverging `else` is a [`WS062`](diagnostics.md)
error.

```wirescript
enum Opt { Some(int), None }

mod unwrapOr(v: Opt, fallback: int) -> int {
  let Some(x) = v else { return fallback }
  return x
}

static var o: Opt = Opt.Some(5)
var result: int = 0

on ReadBrickGrid() {
  result = unwrapOr(o, -1)
}
```

## Changing a payload

A payload field has **no direct write**. `s.field = v` on an enum value is a
[`WS007`](diagnostics.md) error, because an enum value is stored as its tag
plus one slot per variant field, and the surface field name names no slot.

There are two ways to change one, and which you want depends on whether the
value is already the variant you are writing.

**Destructure and assign the capture.** A `match` arm, an `if let`, or a
`let ... else` binds each capture directly to the matched value's payload
slot, so writing the capture writes the payload **in place**. The other
fields, and the tag, are left alone:

```wirescript
type Track = { origin: vector, direction: vector }

enum Nav {
  Idle,
  Moving { track: Track, label: string, active: bool },
}

var nav: Nav = Nav.Moving {
  track: { origin: Vec(4.0, 4.0, 4.0), direction: Vec(0.707, 0.707, 0.0) },
  label: "start",
  active: false,
}

in go: exec

on go {
  if let Moving { track, label, active } = nav {
    track = { origin: Vec(69.0, 69.0, 69.0), direction: Vec(1.0, 0.0, 0.0) }
    label = "moved"
    active = true
  }
}
```

The write costs one `Var_Set` per scalar field, and one per leaf for a
record-typed field, so `track` above costs two, one for `origin` and one for
`direction`. That is the same price as writing a record var. Because the
assignment sits inside the arm, it only runs when the value really is that
variant, which is what makes an in-place payload write safe.

The scrutinee has to be **writable storage** for this: a `var`, a `static var`,
a `*T` parameter, or a `var`-backed record field. Captures off a `let`, a
`const`, or a by-value parameter stay read-only, since there is no storage gate
behind their slots for a write to land on.

A **container element is not storage**. `arr[i]`, `m[k]`, and any field reached
through one (`hs[0].e`) read the element out by value, so a capture off them is
read-only too and writing it is a [`WS007`](diagnostics.md) error. To change an
element's payload, copy it into a `var`, mutate that, and store it back:

```wirescript
enum Slot { Empty, Full { label: string, ready: bool } }

var slots: Slot[]
var cur: Slot = Slot.Empty
in go3: exec

on go3 {
  slots.push(Slot.Full { label: "start", ready: false })
  cur = slots[0]
  if let Full { label, ready } = cur {
    label = "moved"
    ready = true
  }
  slots[0] = cur
}
```

**Rebuild the whole variant.** Assigning the enum itself sets the tag and every
slot of the new variant at once, and is the only option when the value is
changing variant:

```wirescript
enum Nav2 { Idle, Moving { label: string, active: bool } }

var nav2: Nav2 = Nav2.Idle
in go2: exec

on go2 {
  nav2 = Nav2.Moving { label: "moved", active: true }
}
```

Reading works the same way round: pull a payload field out through a `match`
or `if let` capture, not through `value.field`.

## Payloads cannot hold containers

A payload field cannot be an array or a map, and neither can a record used as
one. The declaration is a [`WS069`](diagnostics.md) error:

```wirescript ignore
enum Loadout {
  Empty,
  Carrying { items: int[] },   // WS069
}
```

A payload slot is a single storage gate, and it is filled by **constructing**
the variant. There is no way to construct a container into one: a declaration
initializer bakes an initial value, and an array has none, while a runtime
`Loadout.Carrying { items: [1, 2, 3] }` has no gate that assigns a whole array.
The slot would still be allocated, so a captured `items.push(...)` compiled to a
real gate writing storage that nothing ever filled, and the value read back
empty. Rejecting the declaration stops that at the source.

The same applies to a generic enum instantiated with a container, when the
parameter is one a variant actually stores, so `Option<int[]>` is a `WS069` at
the annotation. A parameter no variant stores is unaffected.

Keep the container in its own `var` and put something scalar in the payload
that refers to it, such as an index, a key, or a length:

```wirescript
var items: int[]

enum Loadout {
  Empty,
  Carrying { first: int, count: int },
}

var loadout: Loadout = Loadout.Empty
in pickUp: exec

on pickUp {
  items.push(7)
  loadout = Loadout.Carrying { first: 0, count: items.length() }
}
```

## `unsafe`: unchecked payload access

`unsafe <value>.<Variant>.<field>` reads or writes one payload slot without
proving the value is that variant. It costs no gate: the access resolves
straight to the slot.

```wirescript
enum Job { Idle, Running { label: string, ticks: int } }

var job: Job = Job.Running { label: "start", ticks: 0 }
var seen: string = ""
in tick: exec

on tick {
  unsafe job.Running.ticks = unsafe job.Running.ticks + 1
  seen = unsafe job.Running.label
}
```

The variant is required and is what selects the slot, so a field two variants
share is never ambiguous. A record-typed payload keeps projecting
(`unsafe job.Running.spec.width`). An unknown variant is a
[`WS060`](diagnostics.md), an unknown field a `WS010`, a missing segment a
`WS070`.

The tag is asserted, never tested and never written:

- Reading a variant the value is **not** returns that slot's stale contents,
  with no error and no default.
- Writing sets the slot only. If `job` is `Idle`, `unsafe job.Running.ticks = 5`
  fills `Running`'s slot while `job` stays `Idle`, and a later `match` still
  takes the `Idle` arm.

Prefer a destructure, which tests the tag first; reach for `unsafe` when you
have already established the variant.

`unsafe` is contextual, so a variable, parameter, or mod named `unsafe` keeps
working. (The tree-sitter grammar cannot express that and treats the word as a
keyword, so an editor using it flags such a name even though it compiles.)

## Generic enums

An enum can take type parameters, instantiated per use exactly like a
[generic type alias](types.md#generic-type-aliases):

```wirescript
enum Box<T> { Value(T), Empty }

static var b: Box<int> = Box.Value(42)

out val = match b {
  Value(x) => x,
  Empty    => 0,
}
```

## Built-in `Option` and `Result`

Wirescript ships `Option<T>` and `Result<T, E>` as prelude enums -- no
declaration needed, and their variants are usable bare (`Some`/`None`/`Ok`/`Err`,
see [Construction](#construction) above). They're built in as if declared:

```text
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }
```

Don't redeclare them yourself -- the prelude already registers both names, so
an `enum Option<T> { ... }` of your own is a [`WS013`](diagnostics.md)
duplicate-declaration error.

```wirescript
static var maybe: Option<int> = Some(7)
static var missing: Option<int> = None

out found = match maybe { Some(x) => x, None => -1 }
```

```wirescript
static var r: Result<int, string> = Ok(200)

out status = match r {
  Ok(code) => code,
  Err(msg) => -1,
}
```

## Built-in game enums

A handful of enum types are built into the compiler with **no `enum`
declaration** at all. `EasingFunction`, `Direction`, `ColorSpace`,
`DisplayTextJustification`, `TextTypeface`, `DisplayTextEasing`, and
`EasingDirection` are the ones shipping today. They are not hand-maintained:
the compiler discovers them from the game's own config enums (the same ones
that back a gate's settings-menu fields), so the exact set and its variants
track whatever build the compiler was generated against.

A built-in game enum behaves like an ordinary unit-only enum (see
[Declaration](#declaration) above): construct a variant with its qualified
path, store it in a `var` or `static var`, and read `.Discriminant`:

```wirescript
static var mode: EasingFunction = EasingFunction.Bounce

out disc = mode.Discriminant
```

The enum value is the default representation; `.Discriminant` gives back the
integer the game's own schema assigns that member. That is the one place a
built-in game enum differs from a user-declared one: a user enum's
discriminants auto-number from 0, but a built-in game enum's discriminant is
the real schema value, since it round-trips through saved component data and
a renumbered tag would write the wrong value to the game.

A built-in enum value also passes directly as the matching
[gate config argument](builtins.md#gate-config-properties):

```wirescript
in t: float

let eased = Easing(0.0, 1.0, t, function = EasingFunction.Bounce, direction = EasingDirection.InOut)
out result = eased
```

The older bare-name form (an unqualified member name) still works side by
side with the enum-qualified form, and both set the same config field:

```wirescript
in t: float

let eased = Easing(0.0, 1.0, t, function = Bounce, direction = InOut)
out result = eased
```
