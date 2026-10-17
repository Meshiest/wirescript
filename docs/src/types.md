# Types

Wirescript has a static type system that maps directly to Brickadia's wire graph port types. The type checker validates that wires connect compatible ports and inserts coercion gates where needed.

<!-- toc -->
## Contents

- [Primitive Types](#primitive-types)
- [Compound Types](#compound-types)
- [Generics](#generics)
- [Type Coercion](#type-coercion)
- [Type Annotations](#type-annotations)
- [Field Access on Types](#field-access-on-types)
<!-- /toc -->

## Primitive Types

| Type | Description | Default Value |
|------|-------------|---------------|
| `bool` | Boolean (`true` / `false`) | `false` |
| `int` | 64-bit signed integer | `0` |
| `float` | 64-bit floating point | `0.0` |
| `string` | Text string | `""` |
| `vector` | 3D vector (x, y, z floats) | `(0, 0, 0)` |
| `rotator` | Euler rotation (pitch, yaw, roll floats) | `(0, 0, 0)` |
| `quat` | Quaternion (x, y, z, w); produced by the rotation conversion gates (`dir.ToRotation()`, …) | identity |
| `color` | RGBA color (r, g, b, a floats) | `(0, 0, 0, 0)` |
| `entity` | Reference to a game entity | null |
| `character` | Reference to a player character | null |
| `controller` | Reference to a player controller | null |
| `exec` | Execution signal (trigger) | -- |

### `exec` Type

The `exec` type represents an execution trigger signal. It is not a data value. It represents "this event fired" or "this code path should execute." Inputs of type `exec` are used as handler triggers:

```wirescript
in reset: exec

on reset {
  count = 0
}
```

### Special Types

| Type | Description |
|------|-------------|
| `any` | Universal type, compatible with everything, but can't back a variable gate's storage. See [`any` Type](#any-type) below. |
| `never` | Bottom type: no value inhabits this type. Used internally. |

### `any` Type

`any` is a wildcard annotation for a value that flows through a wire without the checker fixing its concrete type. `test & 1`, `test == "x"`, and every other operator overload still resolve against the operand type on the other side, instead of erroring as an unknown type would. An `any` value is accepted anywhere, but the checker can't warn you if the operator it selects isn't the one you meant.

For a `mod` parameter that passes a value through, prefer a [generic type parameter](#generics) (`<T>`) over `any`, because it keeps the type and the checker's help with it. See [`any` vs. a Generic Parameter](#any-vs-a-generic-parameter).

`any` is valid wherever a value passes through:

```wirescript
in test: any             // input port
let value = test & 1     // let binding
mod f(v: any) { ... }    // mod parameter
chip C(v: any) -> (r: any) { out r = v }  // chip parameter / output
```

It is **not** valid as a variable gate's storage type, because a Variable gate needs one concrete wire variant to hold, and `any` has none:

```wirescript
var foo: any = 0          // ERROR: 'any' cannot be stored
static var foo: any = 0   // ERROR: same
var foo: any[]          // ERROR: same
buffer foo: any = 0       // ERROR: same
```

An **unannotated** `var` or `buffer` is unaffected: its placeholder type is refined from the initializer (or left as the internal "unknown" fallback), never `any`, so this rejection never applies to it.

### Object references & assets

`entity`, `character`, `controller` are all **object references**: a wire carries a handle to a game object, not a copy of it. **Asset references** (`$AssetType/AssetName`, e.g. `$BrickAudioDescriptor/BA_MUS_…`) are object references too: each lowers to its own reference gate (an `AudioReference` brick, and so on) whose output is wired wherever you use it.

Because they share the same underlying object wire variant, an `entity[]` array (or an object-typed `var`) can hold any of them, including asset references:

```wirescript
var songs: entity[]

on load {
  songs.push($BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop)
}
```

**References can't be inlined into an initializer.** A constant `var` initializer (`= [...]`) only bakes *value* literals (`int` / `float` / `bool` / `string` / `vector` / …) into the gate, and refs to storage (see [Reference values](#reference-values)). An object reference must be wired in from its own brick, so it can't sit in a constant initializer. Build the array with `.push(...)` inside an exec handler instead. Writing `var songs: entity[] = [$Asset/…]` silently drops the elements, and the compiler warns (`WS024`).

### `zone` & `teleport` references

| Type | Description | Produced by | Consumed by |
|------|-------------|-------------|-------------|
| `zone` | Reference to a Zone brick | a Zone brick's output (wire it into an `in z: zone` port) | `zone = …` on the zone events; `fillFromZone*` |
| `teleport` | Reference to a Teleport Destination (a "teleport point") | a Teleport Destination brick (wire it into an `in p: teleport` port) | `Teleport` / `RelativeTeleport` `dest`/`source` |

These are **reference-only** types: a wire carries a handle to a component, not
a value. They can be passed as `in` ports, mod/chip parameters, and rerouted
anywhere. But unlike a [`*T` ref value](#reference-values), which can be
stored, selected and compared, a `zone`, `teleport` or prefab reference can
**not** be:

- **stored** in a `var` / `buffer` (`WS025`), because there is no wire variant that holds one;
- **selected** with an if-then-else (`WS031`), because the Select gate routes a *value*, and these have none;
- operated on (arithmetic, comparison, string-format).

```wirescript
in z: zone
in e: entity
in p: teleport

on ZoneEntered(zone = z) -> (character) {   // wire the zone into the event
  e.Teleport(p)                         // teleport `e` to the teleport point `p`
}
```

To teleport an entity to a raw **position** (a vector), use `SetLocation`. The `Teleport` gates require a teleport point, not a coordinate.

### The `null` literal

`null` is a polymorphic literal that adopts its target type and produces that
type's zero value: an unset object for `entity`/`character`/`controller`, `0` for
a number, `false` for `bool`, `""` for `string`, the zero vector / rotation /
color. It needs a known target type: a `var`/`out` annotation, an assignment, a
call argument, or a record field:

```wirescript
var target: entity = null      // an unset object reference
var score: int = null          // 0
in go: exec
on go { target = null }        // clear the reference

type Slot = { owner: entity, count: int }
var slot: Slot = { owner: null, count: null }
```

`null` is only valid for a value type. A container, record, or reference-only
type (`int[]`, `Map<K, V>`, a record, `zone`) has no null value and reports
`WS051`. Use its own empty form (`[]`, `{}`) instead. A `*T` var or element
that was never assigned is null (see [Reference values](#reference-values)).
A bare `let x = null` with no target types as `any`.

Compared with `==` or `!=`, `null` takes the other operand's type, so
`x == null` tests for an unset reference on an `entity`, `character`,
`controller`, `brick` or `*T`: an Equal (or Not Equal) gate against the empty
reference. On a value type it compares with that type's zero (`n == null` is
`n == 0`).

```wirescript
var holder: character = null
in go: exec
on go {
  if holder != null { BroadcastChatMessage("taken") }
}
```

An object is `false` on a `bool` port when it is unset, so `!holder` reads the
same as `holder == null`.

## Compound Types

### Reference Types (`ref T`)

A `ref T` is a reference to a mutable variable of type `T`. Variables declared with `var` have type `ref T` internally. This is how the wire graph tracks that they are mutable storage rather than pure signal values.

```wirescript
var count: int = 0  // count has type ref int internally
```

You can write `ref T` explicitly in type annotations, particularly for chip parameters that need to mutate a caller's variable:

```wirescript
chip Counter(n: ref int, step: int) {
  on trigger {
    n = n + step
  }
}
```

The `*` prefix is an alternative syntax for `ref`:

```wirescript
// These are equivalent:
mod slide(a: ref int, b: ref int) { ... }
mod slide(a: *int, b: *int) { ... }
```

`Ref<V>` is an alias spelling of `*V` / `ref V` (`Ref<int>` == `*int` == `ref int`).

#### A ref to a record, tuple, or enum refs its parts

A record has no single wire to point at. When stored, it becomes one backing gate
per field, so `*T` on a record **distributes over the fields**: `*{ a: float, b:
float }` means `{ a: *float, b: *float }`. This lets a chip or mod write through
to the caller's record:

```wirescript
type Player = { score: int, lives: int }

chip AwardPoint(p: *Player) {
  on trigger {
    p.score = p.score + 1
  }
}
```

The caller passes the record variable itself, and each field is wired as its own
reference, so the write lands on the caller's storage.

Tuples and enums work the same way, because they are stored the same way: `*(A,
B)` refs each element, and `*SomeEnum` refs the enum's discriminant and payload
slots, so a chip can reassign the caller's enum outright.

A field that is *already* a reference stays as it is rather than becoming a
double reference. A record whose fields are references still cannot be stored in
a variable, array, or map (`WS049`), because storage needs a real value per
field.

### Reference values

A `*T` is also a value: a ref to one var of type `T`, which can be stored,
copied, compared and sent like any other value.

```wirescript
var a: int = 0
var b: int = 0
var h: *int                 // holds a ref; null until assigned
var refs: (*int)[]          // an array of refs

on ReadBrickGrid() {
  h = &a                    // `&x` is x's ref as a value
  *h = 5                    // write through it: a is now 5
  let n = *h + 1            // read through it
  refs.push(&a)
  refs.push(&b)
  *refs[1] += 2             // b is now 2
}
```

- `&x` makes a ref and `*e` reads or writes through one, in an exec context.
- A ref never converts to its target type: `h + 1` is an error, `*h + 1` is the read.
- `==` and `!=` compare two refs of the same type. A ref equals its copy and differs from a ref to any other var. Comparing a ref with a number is `WS084`: the game treats `1 == <ref>` as true.
- Passing a `*T` value to a `*T` parameter binds the parameter to the var the ref points at. Passing a plain var still binds that var.
- Write an array of refs with parentheses, `(*int)[]`. A plain `*int[]` is a var that holds an array ref (below).
- A ref to a ref (`**int`, or `&h` where `h` holds a ref) is `WS086`.
- An initializer can hold refs: `var h: *int = &a` or `var refs: (*int)[] = [&a, &b]` starts out pointing at `a` and `b`. The refs are baked into the save, so no gate runs. Each one must name a var, array or map declared above it (`WS003`).

**A null ref halts the chain.** A `*T` var or element that was never assigned holds a null ref. Reading or writing through it stops the exec chain in game, with no message. A `*T` var declared inside a handler must have an initializer (`WS025`), since a local var resets every run and a ref has no literal to reset to.

#### Places and ref values in a record

A record's `*T` field is one of two things, fixed by where the record comes from:

- A **place** names a var. A field filled from a var (`{ cpsr }`, `{ p: n }`) or from a `*T` parameter is a place, and so is every `*T` field of a record parameter. A place reads as its var's value (`cpu.cpsr + 1`) and binds a `*T` parameter to that var, and `&` of it is that var's ref. It stays a place through a `let`, a field (`let s = o.c`), a tuple element, a destructuring pattern and an `import * as` namespace.
- A **ref value** carries a ref, as a `*T` var does. A field filled from `&x`, from a var holding a ref, or from an array element, map read or event field is a ref value, and so is every `*T` field of a call's result and of a record an `if` or `match` chooses: a place that a `mod` or `chip` returns leaves it as that place's ref. A ref value reads as the ref (`*r.p` reads through it, `r.p + 1` is `WS004`) and binds a `*T` parameter to the var it points at.

`&r` of a whole record is a record of refs, one per field, as `*` on a record type distributes over its fields: after `let x = &r`, `*x.a` reads `r.a`.

A record parameter's fields stay places whatever the caller passes. A ref value there makes the field the var the ref points at, so it reads and writes that var in an exec context. A pure read of it is `WS006`, as for any runtime ref.

```wirescript
type R = { p: *int }
var a: int = 0
var n: int = 0
mod bump(q: R) { q.p = q.p + 1 }
mod pick() -> R { return { p: &a } }

on ReadBrickGrid() {
  bump({ p: a })            // a place: bumps a
  bump({ p: &a })           // a ref value: bumps a too
  let r = pick()
  n = *r.p + 1              // a call's result carries the ref
}
```

#### Arrays of arrays

An array or map is only ever handled by reference, so an array value is its own ref. It is used directly: `&` is optional and `*` is an error (`WS085`).

```wirescript
var row0: int[]
var row1: int[]
var rows: int[][]
var grid: int[][] = [row0, row1]  // starts holding both
var cur: *int[]             // a var holding an array ref

on ReadBrickGrid() {
  rows.push(row0)
  rows.push(row1)
  rows[1].push(7)           // pushes into row1
  rows[1][0] = 8
  cur = rows[0]
  cur.push(1)               // pushes into row0
}
```

Maps work the same way, as values (`Map<string, int[]>`, `Map<string, *color>`), though not as keys.

A map nested in a map closes with `>>`, as in `Map<string, Map<string, int>>`, and the closing `>` may run straight into an initializer's `=` (`var m: Map<string, int>= {}`).

### Bricks

`brick` is a value type. It can be stored in a `var`, a `brick[]` or a map value, copied, compared with `==` / `!=`, printed, and passed through ports, parameters and custom events.

```wirescript
var anchor: brick           // point it at a brick in the editor after pasting
var seen: brick[]

on ReadBrickGrid() {
  seen.push(anchor)
  if seen[0] == anchor {
    BroadcastChatMessage("same brick")
  }
}
```

Nothing in the language creates a brick, and no gate reads one. A brick comes from the editor, a port or an event.

### Array Types (`T[]`)

Arrays hold multiple values of the same element type. Declare them with a `var` whose type ends in `[]`:

```wirescript
var scores: int[]
```

`Array<V>` is an alias spelling of `V[]` (`Array<int>` == `int[]`).

Array access uses bracket syntax and returns the element type directly:

```wirescript
let result = scores[i]  // result: int
if result > 100 { }     // works directly, no .value needed
```

Assignment also works directly: `scores[i] = 42`.

### Map Types (`Map<K, V>`)

A `Map<K, V>` is a keyed collection, backed by a `MapVar` gate. Like an array,
it is stored in a `var` and starts empty. Build it at runtime from an exec
handler:

```wirescript
var scores: Map<string, int>
```

The **key type `K` must be `int`, `string`, or an object reference**
(`entity` / `character` / `controller`), because a map is keyed by a hashed slot, and
only those types have a slot representation. Any other key type is a **`WS039`**
error. The value type `V` may be any storable variant.

Maps are read and written through their methods (`get`, `set`, `has`, `remove`,
`clear`, `copyFrom`, `length`, `keys`, `values`), which run in exec context.
See the map-method table in [builtins.md](builtins.md) and the
[Maps statement section](statements.md#maps-var-name-mapk-v).

### Tuple Types (`(A, B, C)`)

Tuples are fixed-size ordered collections of potentially different types:

```wirescript
// A chip returning multiple outputs produces a record/tuple
chip Split(v: vector) -> (x: float, y: float, z: float) {
  out x = v.x
  out y = v.y
  out z = v.z
}
```

Access the elements of a tuple value with `.0`, `.1`, `.2` etc. (A multi-output `mod`'s result is read by its output names instead, `r.lo`; `.0` on it is `WSP001`.)

```wirescript
let pair = someTuple
let first = pair.0
let second = pair.1
```

Both `Type::Record` and `Type::Tuple` exist in the type system. Records use named fields (`{ x: int, y: int }`), while tuples use positional access (`(int, float)`).

### Record Types

Record types are named structural types with labeled fields. Define them with the `type` keyword:

```wirescript
type Point = { x: int, y: int }
type State = { counter: *int, label: string }
```

A record **value** (a `let` binding, a record literal, a chip's multi-output result) is a compile-time abstraction: it generates no gates, and each field resolves directly to its underlying binding (variable reference, local value, array, etc.). A record used as **storage** (a `var`, array, or map) is the exception (see [Records as storage](#records-as-storage) below).

**Interior mutability with `*T` fields**: A record field of type `*int` (or `ref int`) holds a reference to a mutable variable. Writing through the field mutates the original variable:

```wirescript
type State = { val: *int }
var n: int = 0
let s: State = { val: n }
on RoundStart() { s.val = 42 }  // writes to n
```

Nested records work the same way. Field access chains resolve through each level:

```wirescript
type Inner = { x: *int }
type Outer = { inner: Inner }
var x: int = 0
let i: Inner = { x }
let o: Outer = { inner: i }
on RoundStart() { o.inner.x = 42 }  // writes to x
```

### Records as storage

A record can back a `var`, an array, or a map. It decomposes into one storage
gate **per field** (recursing through nested records), and every operation fans
out across those per-field gates. This is what lets a record be mutated, indexed,
and kept across ticks, which a plain record *value* cannot.

```wirescript
type Point = { x: int, y: int }

on RoundStart() {
  // A record variable: one Variable gate per field.
  var p: Point = { x: 1, y: 2 }
  p.x = 10                  // writes the x gate only, y untouched
  p = { x: 7, y: 8 }        // whole-record assignment writes every field

  var q: Point = { x: 0, y: 0 }
  q = p                     // copies each field into q's own gates, not an alias
  p.x = 99                  // ...so this does not change q.x

  // A record array: parallel arrays, one per field.
  var pts: Point[]
  pts.push({ x: 3, y: 4 })  // pushes x into pts' x-array, y into its y-array
  let first = pts[0].x      // reads the x-array at index 0
  pts[0] = { x: 9, y: 9 }   // writes every field's array at index 0
  let n = pts.length()      // the fields share a length; length reads the first

  // A record map: parallel maps, one per field (same key type).
  var m: Map<int, Point>
  m.set(0, { x: 5, y: 6 })
  let v = m.get(0).x        // reads the x-map at key 0
}
```

A constant initializer bakes per field, so a record array or map can be
constructed up front:

```wirescript
type Point = { x: int, y: int }
var pts: Point[] = [{ x: 1, y: 2 }, { x: 3, y: 4 }]   // x-array [1,3], y-array [2,4]
var grid: Map<int, Point> = { 0 => { x: 5, y: 6 } }   // x-map {0:5}, y-map {0:6}
```

**Struct-of-arrays access.** Because a record array is *stored* as one array per
field, that field's array is directly reachable as `pts.field`, a real
`T[]` you can index (`pts.x[i]`), read (`pts.x.length()`, `pts.x.sum()`,
`pts.x.min()`, `pts.x.find(v)`), or pass on. Sorting is a special case:
`pts.field.sort(descending?)` sorts the whole record by that field and reorders
every sibling column to match, so rows stay intact. (Wide records sort in groups
against a copy of the key, so there is no field-count limit.) Every other
operation on a column acts on that column alone. Mutating one field's array by
itself (`pts.x.push(1)` without a matching `pts.y.push(...)`) breaks the row
correspondence that the whole-array operations rely on. Prefer the whole-record
operations (`push`/`pop`/`pts[i]`) unless you want to work on a single column.

**Choosing between two records.** A record has no single wire, so an `if`
expression or a `match` expression over records makes the choice **per leaf
field**: one `Select` per leaf, all reading the same lowered condition (or, for
a `match`, the same `__disc` read). In source it is written like any other
conditional value:

```wirescript
type Point = { x: int, y: int }
enum Pick { First, Second }

var a: Point = { x: 1, y: 2 }
var b: Point = { x: 3, y: 4 }
var chosen: Point = { x: 0, y: 0 }
var which: Pick = Pick.First
in go: exec

on go {
  chosen = if which is Pick.First then a else b   // 2 Selects, one per field
  chosen = match which { First => a, Second => b }
  let alias = match which { First => a, Second => b }
  chosen.x = alias.y
}
```

The same holds for an enum value, whose leaves are its discriminant and payload
slots, so `match outer { A => Shape.Circle(r), B => Shape.Empty }` chooses the
tag and each slot alongside it.

An arm that is itself a **call** splits the same way. A record-returning `mod`
contributes its record's fields, and a multi-output gate contributes one leaf
per output port, so `let c = if hit then m.get(a) else m.get(b)` gives `c` a
real `Value` and a real `Found`, each chosen by its own `Select`.

**Which fields are allowed.** Every leaf field must be a value the wire graph can
store: a number, `bool`, `string`, `vector`/`rotator`/`color`, an entity type,
or a nested record/array/map. A reference-only field (`*T`, `zone`, `teleport`, a
prefab reference) or an `exec` field cannot be stored, and a record with one is
rejected with `WS049`. (A record *value* may still carry a `*T` field for interior
mutability, as above. Only *storage* is restricted.)

A record variable's array or map field is itself a container. When its elements
are records (`type H = { pts: Point[] }`), `h.pts` is stored the same way as a
top-level `var pts: Point[]`, one column per field, and supports the same
operations (`h.pts.push(p)`, `h.pts[i].y`, `h.pts.x[i]`). Two shapes are
rejected with `WS049` because a column holds one value per element. First, a
record or tuple stored in an array or map cannot contain an array or map at any
depth (`type H = { xs: int[] }` then `var hs: H[]`). Second, an array or map of
records, or a record holding one, cannot cross a port (an `in`/`out` port or a
`chip` parameter or output), since a pin carries one reference. Pass it to a
`mod` instead. A record literal that initializes or is assigned to a record
variable fills its container fields the same way the field assignments would
(`h = { pts: [{ x: 1, y: 2 }], m: { "a": 1 }, n: 0 }`), and a `...spread`
supplies every field it names.

A record assignment is a parallel assignment. Its sources are evaluated in
source order and every field sees the values from before the assignment, so
`p = { x: p.y, y: p.x }` swaps, and a value computed from a `let` that reads the
record is held before the first field is written. As everywhere (see
[Evaluation Order](expressions.md#evaluation-order)), a record literal's fields
are evaluated in source order and each keeps its value, so `{ x: q, y: bump() }`
keeps the `q` from before `bump()`. A container field is copied when its write
runs, not read up front. A container copied from one that the assignment also
replaces is copied first. Two containers copied from each other
(`h = { pts: h.qs, qs: h.pts }`) are `WS087`. One that a later field's code may
change first (`h = { ps: src, n: popper() }`, where `popper` clears `src`) is
`WS089`, and a write through a ref, an array element or a parameter pin may
change any container of its kind. Inside a handler, a record `var` with an
initializer is reset to it every run, like a scalar or container `var`. One
without an initializer resets each scalar field to its zero value, as
`var x: int` does. A `static var` keeps its baked initializer.

**Container operations.** A record array supports `push`, `pop`, `insert`,
`remove`, `fill`, `resize`, `swap`, `reverse`, `clear`, `length`, and element
access (`pts[i]`, `pts[i].field`, `pts[i] = rec`, `p = pts[i]`). A nested-record
field is reachable to any depth, since the storage decomposes to leaf columns:
`pts[i].inner.a` reads or writes one leaf, and `pts[i].inner` reads or writes the
whole sub-record at that index. Operations that
reorder elements by *value* (`sort`, `shuffle`), fold *over* whole records
(`sum`/`min`/`max`/`average`), or need a matching second container
(`append`/`copyFrom`/`slice`) have no per-field meaning and are rejected with
`WS050`. Index a scalar field instead. A record map supports `set`, `get`,
`has`, `remove`, `clear`, `length`, `keys`, and `m[k]` access, with the same
depth of nested-field access (`m[k].inner.a`, `m[k].inner`). A map **key**
cannot be a record (`WS039`). Keys must be a single wire value.

### Union Types (`A | B`)

Union types represent a value that can be one of several types. Write one
directly in a type annotation:

```wirescript
let x: int | float = 42
```

Union syntax is also how a [generic bound](#constraint-classes-bounds) names
an ad hoc set of types (`<T: int | vector>`).

An `if`-then-else expression does **not** produce a union of its branch
types. It *widens* them to a single common type instead (see
[Widening Inference](#widening-inference) and the
[if-expression note](expressions.md#conditional-expressions-if-then-else)).
`if condition then 42 else 3.14` has type `float`, not `int | float`.

### Enum Types

An `enum` is a **nominal** type with a fixed set of named variants. Unlike
`Type::Union` above (which is structural: any value of a matching type
qualifies), two enums stay distinct types even if their variants look the
same. A variant can be a bare unit or carry a payload, which makes `enum` how
Wirescript expresses a tagged union:

```wirescript
enum Shape { Empty, Circle(float), Rect(float, float) }
```

An enum value is represented at compile time as a record: a hidden
discriminant field plus one slot per payload field, so it follows the same
[Records as storage](#records-as-storage) rules when it backs a `var`.
`match`, `if let`, and `let ... else` are how you branch on a value's variant
and pull its payload back out. See [Enums](enums.md) for the full reference:
declaration, construction, `.Discriminant`, `match`, `if let` / `let else`,
generic enums, and the built-in `Option`/`Result`.

## Generics

`mod` and `chip` declarations can take type parameters: one implementation
that specializes per call site instead of one copy per concrete type. A
generic `mod` **inlines and monomorphizes** at each call: the compiler infers
the concrete type(s) from the arguments and emits concrete gates for that
type, same as a hand-written non-generic mod would.

Generic **chips** work too: a generic `chip` is monomorphized *per distinct
type instantiation* into its own microchip template (`Box<int>` and
`Box<vector>` become two separate grids; two `Box<int>` calls share one), so
the wire-level behavior mirrors a hand-written non-generic chip at each type.

```wirescript
mod pick<T>(c: bool, a: T, b: T) -> T {
  return if c then a else b
}

in go: exec
in i: int
on go {
  let x = pick(true, i, i)   // T = int, inferred from the arguments
}
```

Multiple type parameters are declared as `<T, U, ...>`, each inferred
independently from its own arguments:

```wirescript
mod first<T, U>(a: T, b: U) -> T {
  return a
}
```

See [`examples/generics.ws`](../../examples/generics.ws) for a complete,
`just check`-clean file exercising every form on this page.

### Constraint Classes (Bounds)

A type parameter can be bounded to a named class of types with `<T: Class>`.
There are three built-in classes, and unbounded `<T>` means `<T: Variant>`:

| Class | Members |
|-------|---------|
| `Scalar` | `int`, `float` |
| `Numeric` | `int`, `float`, `vector`, `rotator`, `quat`, `color` |
| `Variant` | `Numeric` + `bool`, `string`, `entity`, `character`, `controller` (all value variants) |

`Scalar ⊆ Numeric ⊆ Variant`. `bool` is a member of `Variant` only.
`Scalar` and `Numeric` contain only numeric-math types, and a bounded call
with a `bool` argument is rejected:

```wirescript
mod addOne<T: Scalar>(v: T) -> T {
  return v + 1
}

in flag: bool
let bad = addOne(flag)   // ERROR WS033: 'T' = bool, which isn't allowed by its bound
```

An **anonymous union bound** (`<T: A | B>`) restricts `T` to that set of
types instead of a named class:

```wirescript
mod pickAxis<T: int | vector>(v: T) -> T {
  return v
}
```

A union bound may name an [enum](enums.md) or a record type, which an unbounded
`<T>` never admits. The body is still checked for every member, so `match`,
`is` or `.Discriminant` on a `T` that may be `int` is `WS066`, and `==` on one
that may be an enum is `WS004`.

### Widening Inference

`T` is inferred as the **join** (least upper bound) of all the arguments'
types, over a widening-only lattice, with no narrowing:

- Numeric types widen toward the wider type: `int` widens to `float`, so
  `pick(flag, 1, 2.0)` infers `T = float` (the `int` argument casts up).
- Object types widen toward `entity`: `character` and `controller` both
  widen to `entity`, so `pick(flag, aCharacter, aController)` infers
  `T = entity`.
- Incompatible operands (e.g. `int` and `vector`) have no common
  widening and are a compile error (`WS033`):

```wirescript
in n: int
in v: vector
let bad = pick(true, n, v)
// ERROR WS033: cannot infer 'T': it's int from one argument but vector
// from another -- all 'T' arguments must be the same type
```

The same join is used by the built-in blend-family gates (`Blend`, `lerp`,
`Easing`) and by [`if`-then-else expressions](expressions.md#conditional-expressions-if-then-else).
See the widening note there.

### Body Checking Is Per-Mask-Member

A generic mod's body is type-checked against **every** type in its bound's
mask, not only the types it is called with. The body must be valid for the
whole bound. This is the most common mistake with generics: `<T: Numeric>`
includes `rotator`, and an operator that isn't defined between `rotator` and
a bare `int` literal fails the definition itself, even if you never call the
mod with a `rotator`:

```wirescript
mod addOne<T: Numeric>(v: T) -> T {
  return v + 1
}
// ERROR WS004: no overload for '+' on rotator, int. Reported at the
// definition, because `Numeric` includes `rotator` and `rotator + int`
// has no overload, even though every actual call site below uses `int`.
```

Narrow the bound to `Scalar` when the body only needs `int`/`float`
semantics: `v + 1` is valid for every `Scalar` member, so this is clean:

```wirescript
mod addOne<T: Scalar>(v: T) -> T {
  return v + 1     // OK: valid for both int and float
}
```

An operation that is valid across the whole `Numeric` family (a
same-type operator, for instance) is fine to write against `Numeric`
directly:

```wirescript
mod square<T: Numeric>(v: T) -> T {
  return v * v     // OK: same-type multiply is defined for the whole family
}
```

The same holds for a method: an unbounded `T` may be a `bool`, so a string
method on it is rejected at the definition however the mod is called. Bound
`T` to the type the body needs:

```wirescript
mod size<T>(v: T) -> int {
  return v.Length()
}
// ERROR WS003: `.Length()` is a string method, and `T` is unbounded, so `v`
// may be a `bool`; bound it: `<T: string>`

mod textSize<T: string>(v: T) -> int {
  return v.Length()   // OK
}
```

With several type parameters the combinations multiply. Past 64 of them the
body is checked for a sample (each parameter varied across its bound while the
others stay fixed) and for every combination a call uses, including calls from
inside other generic bodies, so an error only a called combination has is
still reported at the definition. Calls that use more than 1024 combinations
beyond the ones the bounds cover are `WS096`.

### Ref Parameters

A `*T` (or `ref T`) parameter infers `T` through the reference:

```wirescript
mod swap<T>(a: *T, b: *T) {
  let tmp = a
  a = b
  b = tmp
}
```

The same per-mask-member body checking applies: a ref-param body that only
assigns is fine unbounded (valid for every `Variant` member), but one that
does arithmetic on the referenced value needs a `Scalar` bound for the same
reason `addOne` does above:

```wirescript
mod inc<T: Scalar>(v: *T) {
  v = v + 1
}
```

### `any` vs. a Generic Parameter

[`any`](#any-type) erases the type entirely, so the checker can't validate
operators against it and can't tell you when you've made a mistake. A
generic parameter keeps the type information and validates the body against
the bound, so prefer a generic `mod` (`<T>` or `<T: Bound>`) over `any` for
a value that flows through unchanged. Use `any` only when the type of what
flows through doesn't matter and you don't need the checker's help.

### Generic type aliases

A `type` alias can take type parameters and is instantiated by substitution:

```wirescript
type Pair<T> = { a: T, b: T }
let p: Pair<int> = { a: 1, b: 2 }   // resolves to { a: int, b: int }
```

An alias must be **fully applied** (`Pair` alone, or `Pair<int, float>` on a
one-parameter alias, is a `WS002` error) and **non-recursive** (`type L<T> =
{ tail: L<T> }` is rejected rather than hanging the compiler).

Every alias body, generic or not, is read where the alias is declared. A
generic `mod` whose type parameter is named `T` hides an alias `T` only in what
the `mod` itself writes, never inside another alias it uses, and a library's
aliases are the library's own: a body imported from it reads its `F`, whatever
the importing file calls `F`. Enums are read the same way: a library names the
enums it declares or imports, and never one only its importer declares.

A program's own type named like a builtin generic is the program's wherever it
is declared: `type Map<K, V> = { k: K, v: V }` makes `Map<int, int>` that
record, in every stage of the compiler. `Array<T>` and `Ref<T>` are how `T[]`
and `*T` are written, so a `type` or `enum` named `Array` or `Ref` with one type
parameter could never be named, and is a `WS107` error. With another number of
type parameters it is the program's own type, as `Map` is.

### Explicit type arguments

`T` is normally inferred from the arguments, but you can pin it explicitly with
a `<...>` type-argument list at the call site:

```wirescript
let x = pick<int>(flag, a, b)   // same as the inferred pick(flag, a, b)
let z = zero<vector>()          // required: T appears only in the return
```

Explicit type arguments are the only way to call a mod whose type parameter
can't be inferred from its arguments, e.g. a `T` that appears only in the
return type (`mod zero<T>() -> T`). They are checked like inferred ones: the
count must match the type parameters, and each must satisfy its bound
(`zero<string>()` on a `<T: Numeric>` is a `WS033` error). Type arguments on a
non-generic function are an error too. On a builtin they are ignored with a
`WS037` warning, because a builtin's result type is derived from its arguments.

Parsing note: `f<int>(...)` is read as a type-argument list only when the
`<...>` is a valid list of types immediately followed by `(`. A plain `a < b`
comparison (or `a < b > c`) is never mistaken for it.

## Type Coercion

Wirescript automatically inserts coercion gates when types don't match exactly but are compatible. The coercion rules mirror Brickadia's `PortsAreCompatible` behavior.

### Numeric Coercion (Bidirectional)

All numeric types (`bool`, `int`, `float`) coerce to each other freely:

```wirescript
var x: float = 1      // int -> float: OK
var y: int = true      // bool -> int: OK (true=1, false=0)
var z: float = false   // bool -> float: OK
```

A float constant that is the whole value of an `int` place is the integer it
truncates to: `var n: int = 2.5` and `const K: int = 2.5` both hold 2, with a
`WS106` warning that the fraction is dropped. The constant is then an integer
wherever it goes, so `var u = K` declares an `int` and `u / 4` divides as
integers. The same holds for a chip argument, a field of a record or tuple
literal, an element and a return. Custom-event data is not an `int` place. A
float constant sent to a receiver that declares `int` is sent as the float, and
`WS030` warns of the mismatch. A float constant that no `int` can hold (`1e30`)
is a `WS028` error.

Because `bool` coerces to `int` automatically, you do not need `if x then 1 else 0`. Use the bool directly where an int is expected. The `if/then/else` form is only needed when you want specific non-0/1 scalar values:

```wirescript
let count = a + b + c            // bools coerce to 0/1 automatically
let weight = if heavy then 10 else 1  // need if/then for non-0/1 values
```

### Rotation Coercion (Bidirectional)

Both carry their components as `float` fields (`.pitch` `.yaw` `.roll` on a
`rotator`, `.x` `.y` `.z` `.w` on a `quat`), read through the split gates
described in [Field Access](expressions.md#vector--color--rotation-components).

A `rotator` (euler) and a `quat` (quaternion) are interchangeable rotation values
at the wire level, so they coerce to each other freely. This is how a rotation
converts to a quaternion: feed an entity's `GetRotation()` rotator straight into a
quaternion gate, or call quaternion methods on a `Rotation(...)` result.

```wirescript
let r = Rotation(0.0, 90.0, 0.0)   // rotator (Make Rotation from euler degrees)
let back = r.Invert()              // rotator coerces to quat for the gate → quat
let spun = aim.Rotate(r.Invert())  // rotate a vector by the inverse rotation
```

### String Coercion (One-Way)

All primitive types can be coerced to `string` via an implicit format gate:

```wirescript
var label: string = 42         // int -> string: "42"
var pos: string = someVector   // vector -> string: formatted
```

The following types format to string: `bool`, `int`, `float`, `string`, `vector`, `rotator`, `color`, `entity`, `character`, `controller`.

### String Coercion to Bool (empty is false)

A `string` coerces to `bool` wherever a bool is expected: a condition, a `bool`-typed `let`/`var`, a bool-typed port or chip/mod param. The coercion means `s != ""`, and the compiler inserts a `CompareNotEqual(s, "")` gate at every such coercion point:

```wirescript
in name: string

if name {
  // taken whenever `name` is non-empty (compiles as `if name != ""`)
}

let hasName: bool = name   // also `name != ""`
```

| String value | Bool |
|---------------|------|
| `""` | `false` |
| anything else (including `"0"`, `"false"`, `" "`) | `true` |

String *literals* in bool positions (`var v: bool = "0"`, `array flags: bool[] = ["x", ""]`, `Select("0", a, b)`) are converted at compile time by the same `!= ""` rule: the baked value is already a bool, so `"0"` bakes as `true`.

**Only the empty string is false.** This differs from the game's *native* bool-port behavior, which is content-aware. A string wired manually into a bool port (e.g. via an [`any`](#any-type)-typed value, whose erased type skips the coercion, or through the logical operators' native string overloads) is read by the gate itself, where `""`, `"0"`, and `"false"` are all falsy. That native rule is certified per build against the in-game gate-semantics table, the same certification that drives constant folding of conditions (see [folding.md](folding.md)). If you want the content-aware behavior, wire it manually. If you write `if someString`, you get the deterministic `!= ""`.

This direction is one-way: `bool` to `string` still goes through the format gate above and renders `"true"`/`"false"` text, not the other way around.

### Pulsing Coercion to Exec

Value types that "pulse" (change over time) can trigger exec inputs. This means `bool`, `int`, `float`, `vector`, `entity`, `character`, and `controller` values can be connected to `exec` inputs: the exec fires whenever the value changes:

```wirescript
// A bool value can trigger a handler
chip let moved = position != position.prev

on moved {
  // Fires whenever 'moved' transitions
}
```

### Reference Invariance

Reference types (`ref T`) do **not** coerce. A `ref int` cannot be passed where a `ref float` is expected, even though `int` and `float` coerce freely. This prevents accidentally wiring incompatible variable storage:

```wirescript
var x: int = 0
var y: float = 0.0

// This would be an error: ref int != ref float
// someChip(x, y)  // if both params expect ref int
```

### Coercion Summary Table

| From | To | Rule |
|------|----|------|
| `int` | `float` | Coerce |
| `float` | `int` | Coerce |
| `bool` | `int` | Coerce |
| `bool` | `float` | Coerce |
| `int` | `bool` | Coerce |
| `float` | `bool` | Coerce |
| `character` | `entity` | Coerce (subtype) |
| `controller` | `entity` | Coerce |
| `entity` | `character` | Coerce (wired directly, since an entity wire can carry a player, e.g. a sweep hit) |
| `entity` | `controller` | Coerce (wired directly) |
| `character` | `controller` | Coerce (wired directly) |
| `controller` | `character` | Coerce (wired directly) |
| `rotator` | `quat` | Coerce (interchangeable rotation values) |
| `quat` | `rotator` | Coerce |
| any primitive | `string` | Via format gate |
| `string` | `bool` | Coerce (compiler inserts `!= ""`, so only empty is false) |
| `exec` | `bool` | Coerce (true for one frame) as a condition, a logical operator's operand or an `on` trigger. Stored or passed as a `bool`, it is `WS003` (see [Logical Operators](expressions.md#logical-operators)) |
| `bool`/`int`/`float`/`vector`/`entity`/`character`/`controller` | `exec` | Pulsing coerce |
| `ref T` | `ref U` (T != U) | **Mismatch** |
| `any` | anything | Same |
| anything | `any` | Same |

"Wired directly" converts nothing in game. A character wired into a
`controller` slot is still the character: it is not equal to that player's
controller, and as a `Map<controller, V>` key it files under the body, so a
respawn starts a new key. Only `ControllerOf` (an admin gate) gives the
persistent player.

## Type Annotations

Type annotations appear after a colon in declarations:

```wirescript
var x: int = 0
in trigger: exec
var data: float[]
```

Type annotations are optional on `var` when an initializer is present (the type is inferred), but they are required on `in` declarations, and on an array or map `var` that has no initializer to infer its element/key-value types from.

## Field Access on Types

Certain types have built-in fields accessible with dot notation:

### Vector Fields

```wirescript
let v = Vec(1.0, 2.0, 3.0)
v.x   // or v.X -> float
v.y   // or v.Y -> float
v.z   // or v.Z -> float
```

### Color Fields

```wirescript
let c = Color(1.0, 0.5, 0.0, 1.0)
c.r   // or c.R -> float
c.g   // or c.G -> float
c.b   // or c.B -> float
c.a   // or c.A -> float
```

### Rotator Fields

```wirescript
let r = someRotator
r.pitch  // -> float
r.yaw    // -> float
r.roll   // -> float
```

### Variable Fields

Variables (type `ref T`) have special fields:

```wirescript
var count: int = 0

count.Value  // Current value (type T), a delayed read usable in pure context
count.prev   // Previous tick's value (type T), useful for change detection
```

See [Execution Context](exec-context.md) for when to use `.Value` vs direct access.
