# Types

Wirescript has a static type system that maps directly to Brickadia's wire graph port types. The type checker validates that wires connect compatible ports and inserts coercion gates where needed.

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

The `exec` type represents an execution trigger signal. It is not a data value -- it represents "this event fired" or "this code path should execute." Inputs of type `exec` are used as handler triggers:

```wirescript
in reset: exec

on reset {
  count = 0
}
```

### Special Types

| Type | Description |
|------|-------------|
| `any` | Universal type -- compatible with everything, but can't back a variable gate's storage. See [`any` Type](#any-type) below. |
| `never` | Bottom type -- no value inhabits this type. Used internally. |

### `any` Type

`any` is a wildcard annotation for a value that flows through a wire without the checker pinning down (or caring about) its concrete type: `test & 1`, `test == "x"`, and every other operator overload still resolve against whatever operand type is on the other side, instead of erroring the way an actually-unknown type would. The tradeoff is spelled out by the name -- an `any` value works anywhere, but its side effects are on you: the checker can't warn you if the operator that ends up selected wasn't the one you meant.

For a `mod` parameter that just passes a value through, prefer a [generic type parameter](#generics) (`<T>`) over `any` -- it keeps the checker's help instead of erasing the type. See [`any` vs. a Generic Parameter](#any-vs-a-generic-parameter).

`any` is valid wherever a value just passes through:

```wirescript
in test: any             // input port
let value = test & 1     // let binding
mod f(v: any) { ... }    // mod parameter
chip C(v: any) -> (r: any) { out r = v }  // chip parameter / output
```

It is **not** valid as a variable gate's storage type, because a Variable gate needs one concrete wire variant to hold -- `any` has none:

```wirescript
var foo: any = 0          // ERROR: 'any' cannot be stored
static var foo: any = 0   // ERROR: same
var foo: any[]          // ERROR: same
buffer foo: any = 0       // ERROR: same
```

An **unannotated** `var`/`array`/`buffer` is unaffected -- its placeholder type is refined from the initializer (or left as the internal "unknown" fallback), never `any`, so it never trips this rejection.

### Object references & assets

`entity`, `character`, `controller` are all **object references** -- a wire carries a handle to a game object, not a copy of it. **Asset references** (`$AssetType/AssetName`, e.g. `$BrickAudioDescriptor/BA_MUS_…`) are object references too: each lowers to its own reference gate (an `AudioReference` brick, and so on) whose output is wired wherever you use it.

Because they share the same underlying object wire variant, an `entity[]` array (or an object-typed `var`) can hold any of them -- including asset references:

```wirescript
var songs: entity[]

on load {
  songs.push($BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop)
}
```

**References can't be inlined into an initializer.** A constant `array` / `var` initializer (`= [...]`) only bakes *value* literals (`int` / `float` / `bool` / `string` / `vector` / …) into the gate. An object reference must be wired in from its own brick, so it can't sit in a constant initializer -- build the array with `.push(...)` inside an exec handler instead. Writing `array songs: entity[] = [$Asset/…]` silently drops the elements, and the compiler warns (`WS024`).

### `zone` & `teleport` references

| Type | Description | Produced by | Consumed by |
|------|-------------|-------------|-------------|
| `zone` | Reference to a Zone brick | a Zone brick's output (wire it into an `in z: zone` port) | `zone = …` on the zone events; `fillFromZone*` |
| `teleport` | Reference to a Teleport Destination (a "teleport point") | a Teleport Destination brick (wire it into an `in p: teleport` port) | `Teleport` / `RelativeTeleport` `dest`/`source` |

These are **reference-only** types, exactly like a variable ref (`ref T`): a wire carries a handle to a component, not a value. They can be passed as `in` ports, mod/chip parameters, and rerouted anywhere -- but, like a var ref, they can **not** be:

- **stored** in a `var` / `array` / `buffer` (`WS025`) -- a storage gate needs a concrete wire variant;
- **selected** with an if-then-else (`WS031`) -- the Select gate routes a *value*, not a reference;
- operated on (arithmetic, comparison, string-format).

```wirescript
in z: zone
in e: entity
in p: teleport

on ZoneEntered(character, zone = z) {   // wire the zone into the event
  e.Teleport(p)                         // teleport `e` to the teleport point `p`
}
```

To teleport an entity to a raw **position** (a vector), use `SetLocation` -- the `Teleport` gates require a teleport point, not a coordinate.

## Compound Types

### Reference Types (`ref T`)

A `ref T` is a reference to a mutable variable of type `T`. Variables declared with `var` have type `ref T` internally -- this is how the wire graph tracks that they are mutable storage rather than pure signal values.

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

### Array Types (`T[]`)

Arrays hold multiple values of the same element type. Declare them with a `var` whose type ends in `[]`:

```wirescript
var scores: int[]
```

Array access uses bracket syntax and returns the element type directly:

```wirescript
let result = scores[i]  // result: int
if result > 100 { }     // works directly, no .value needed
```

Assignment also works directly: `scores[i] = 42`.

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

Access tuple elements with `.0`, `.1`, `.2` etc:

```wirescript
let pair = someTuple
let first = pair.0
let second = pair.1
```

### Record Types

Record types are named structural types with labeled fields. Define them with the `type` keyword:

```wirescript
type Point = { x: int, y: int }
type State = { counter: *int, label: string }
```

Records are a compile-time abstraction -- they do not generate wire graph gates. Each field resolves directly to its underlying binding (variable reference, local value, array, etc.).

**Interior mutability with `*T` fields**: A record field of type `*int` (or `ref int`) holds a reference to a mutable variable. Writing through the field mutates the original variable:

```wirescript
type State = { val: *int }
var n: int = 0
let s: State = { val: n }
on RoundStart { s.val = 42 }  // writes to n
```

Nested records work as expected -- field access chains resolve through each level:

```wirescript
type Inner = { x: *int }
type Outer = { inner: Inner }
var x: int = 0
let i: Inner = { x }
let o: Outer = { inner: i }
on RoundStart { o.inner.x = 42 }  // writes to x
```

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

Access tuple elements with `.0`, `.1`, `.2` etc:

```wirescript
let pair = someTuple
let first = pair.0
let second = pair.1
```

Both `Type::Record` and `Type::Tuple` exist in the type system. Records use named fields (`{ x: int, y: int }`), while tuples use positional access (`(int, float)`).

### Union Types (`A | B`)

Union types represent a value that can be one of several types. Write one
directly in a type annotation:

```wirescript
let x: int | float = 42
```

Union syntax is also how a [generic bound](#constraint-classes-bounds) names
an ad hoc set of types (`<T: int | vector>`).

Note that an `if`-then-else expression does **not** produce a union of its
branch types -- it *widens* them to a single common type instead (see
[Widening Inference](#widening-inference) and the
[if-expression note](expressions.md#conditional-expressions-if-then-else)).
`if condition then 42 else 3.14` has type `float`, not `int | float`.

## Generics

`mod` and `chip` declarations can take type parameters -- one implementation
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

`Scalar ⊆ Numeric ⊆ Variant`. Note `bool` is only a member of `Variant` --
`Scalar` and `Numeric` are strictly numeric-math types, and a bounded call
with a `bool` argument is rejected:

```wirescript
mod addOne<T: Scalar>(v: T) -> T {
  return v + 1
}

in flag: bool
let bad = addOne(flag)   // ERROR WS033: 'T' = bool, which isn't allowed by its bound
```

An **anonymous union bound** (`<T: A | B>`) restricts `T` to exactly that
set of types instead of a named class:

```wirescript
mod pickAxis<T: int | vector>(v: T) -> T {
  return v
}
```

### Widening Inference

`T` is inferred as the **join** (least upper bound) of all the arguments'
types, over a widening-only lattice -- there's no narrowing:

- Numeric types widen toward the wider type: `int` widens to `float`, so
  `pick(flag, 1, 2.0)` infers `T = float` (the `int` argument casts up).
- Object types widen toward `entity`: `character` and `controller` both
  widen to `entity`, so `pick(flag, aCharacter, aController)` infers
  `T = entity`.
- Incompatible operands -- e.g. `int` and `vector` -- have no common
  widening and are a compile error (`WS033`):

```wirescript
in n: int
in v: vector
let bad = pick(true, n, v)
// ERROR WS033: cannot infer 'T': it's int from one argument but vector
// from another -- all 'T' arguments must be the same type
```

The same join is used by the built-in blend-family gates (`Blend`, `lerp`,
`Easing`) and by [`if`-then-else expressions](expressions.md#conditional-expressions-if-then-else) --
see the widening note there.

### Body Checking Is Per-Mask-Member

A generic mod's body is type-checked against **every** type in its bound's
mask -- not just the types it happens to be called with -- so the body must
be valid for the whole bound, not only your call sites. This is the most
common gotcha: `<T: Numeric>` includes `rotator`, and an operator that isn't
defined between `rotator` and a bare `int` literal fails the definition
itself, even if you never call the mod with a `rotator`:

```wirescript
mod addOne<T: Numeric>(v: T) -> T {
  return v + 1
}
// ERROR WS004: no overload for '+' on Rotator, Int -- rejected at the
// DEFINITION, because `Numeric` includes `rotator` and `rotator + int`
// has no overload, even though every actual call site below uses `int`.
```

Narrow the bound to `Scalar` when the body only needs `int`/`float`
semantics -- `v + 1` is valid for every `Scalar` member, so this is clean:

```wirescript
mod addOne<T: Scalar>(v: T) -> T {
  return v + 1     // OK -- valid for both int and float
}
```

An operation that genuinely is valid across the whole `Numeric` family (a
same-type operator, for instance) is fine to write against `Numeric`
directly:

```wirescript
mod square<T: Numeric>(v: T) -> T {
  return v * v     // OK -- same-type multiply is defined for the whole family
}
```

### Ref Parameters

A `*T` (or `ref T`) parameter infers `T` through the reference:

```wirescript
mod swap<T>(a: *T, b: *T) {
  let tmp = a
  a = b
  b = tmp
}
```

The same per-mask-member body checking applies -- a ref-param body that only
assigns is fine unbounded (valid for every `Variant` member), but one that
does arithmetic on the referenced value needs a `Scalar` bound for the same
reason `addOne` does above:

```wirescript
mod inc<T: Scalar>(v: *T) {
  v = v + 1
}
```

### `any` vs. a Generic Parameter

[`any`](#any-type) erases the type entirely -- the checker can't validate
operators against it and can't tell you when you've made a mistake. A
generic parameter keeps the type information and validates the body against
the bound, so prefer a generic `mod` (`<T>` or `<T: Bound>`) over `any` for
a value that flows through unchanged. Reach for `any` only when you
genuinely don't care what flows through and don't need the checker's help.

### Generic type aliases

A `type` alias can take type parameters and is instantiated by substitution:

```wirescript
type Pair<T> = { a: T, b: T }
let p: Pair<int> = { a: 1, b: 2 }   // resolves to { a: int, b: int }
```

An alias must be **fully applied** (`Pair` alone, or `Pair<int, float>` on a
one-parameter alias, is a `WS002` error) and **non-recursive** (`type L<T> =
{ tail: L<T> }` is rejected, not hung).

### Explicit type arguments

`T` is normally inferred from the arguments, but you can pin it explicitly with
a `<...>` type-argument list at the call site:

```wirescript
let x = pick<int>(flag, a, b)   // same as the inferred pick(flag, a, b)
let z = zero<vector>()          // REQUIRED: T appears only in the return
```

Explicit type arguments are the only way to call a mod whose type parameter
can't be inferred from its arguments -- e.g. a `T` that appears only in the
return type (`mod zero<T>() -> T`). They are checked like inferred ones: the
count must match the type parameters, and each must satisfy its bound
(`zero<string>()` on a `<T: Numeric>` is a `WS033` error); type arguments on a
non-generic function are an error too, and on a builtin they are ignored with a
`WS037` warning (a builtin's result type is derived from its arguments).

Parsing note: `f<int>(...)` is read as a type-argument list only when the
`<...>` is a valid list of types immediately followed by `(` -- a plain `a < b`
comparison (or `a < b > c`) is never mistaken for it.



Wirescript automatically inserts coercion gates when types don't match exactly but are compatible. The coercion rules mirror Brickadia's `PortsAreCompatible` behavior.

### Numeric Coercion (Bidirectional)

All numeric types (`bool`, `int`, `float`) coerce to each other freely:

```wirescript
var x: float = 1      // int -> float: OK
var y: int = true      // bool -> int: OK (true=1, false=0)
var z: float = false   // bool -> float: OK
```

Because `bool` coerces to `int` automatically, you do not need `if x then 1 else 0` -- just use the bool directly where an int is expected. The `if/then/else` form is only needed when you want specific non-0/1 scalar values:

```wirescript
let count = a + b + c            // bools coerce to 0/1 automatically
let weight = if heavy then 10 else 1  // need if/then for non-0/1 values
```

### Rotation Coercion (Bidirectional)

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

### String Coercion to Bool -- empty is false

A `string` coerces to `bool` wherever a bool is expected -- a condition, a `bool`-typed `let`/`var`, a bool-typed port or chip/mod param. The semantics are exactly `s != ""`, and the compiler inserts a real `CompareNotEqual(s, "")` gate at every such coercion point:

```wirescript
in name: string

if name {
  // taken whenever `name` is non-empty — compiles as `if name != ""`
}

let hasName: bool = name   // also `name != ""`
```

| String value | Bool |
|---------------|------|
| `""` | `false` |
| anything else (including `"0"`, `"false"`, `" "`) | `true` |

String *literals* in bool positions (`var v: bool = "0"`, `array flags: bool[] = ["x", ""]`, `Select("0", a, b)`) are converted at compile time by the same `!= ""` law -- the baked value is already a bool, so `"0"` bakes as `true`.

The rule is deliberately simple: **only the empty string is false**. This differs from the game's *native* bool-port behavior, which is content-aware -- a string wired manually into a bool port (e.g. via an [`any`](#any-type)-typed value, whose erased type skips the coercion, or through the logical operators' native string overloads) is read by the gate itself, where `""`, `"0"`, and `"false"` are all falsy. That native law is certified per build against the in-game gate-semantics table -- the same certification that drives constant folding of conditions, see [folding.md](folding.md). If you want the content-aware behavior, wire it manually; if you write `if someString`, you get the deterministic `!= ""`.

This direction is one-way -- `bool` to `string` still goes through the format gate above and renders `"true"`/`"false"` text, not the other way around.

### Pulsing Coercion to Exec

Value types that "pulse" (change over time) can trigger exec inputs. This means `bool`, `int`, `float`, `vector`, `entity`, `character`, and `controller` values can be connected to `exec` inputs -- the exec fires whenever the value changes:

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

// This would be an error -- ref int != ref float
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
| `entity` | `character` | Coerce (wired directly — an entity wire can carry a player, e.g. a sweep hit) |
| `entity` | `controller` | Coerce (wired directly) |
| `character` | `controller` | Coerce (wired directly) |
| `controller` | `character` | Coerce (wired directly) |
| `rotator` | `quat` | Coerce (interchangeable rotation values) |
| `quat` | `rotator` | Coerce |
| any primitive | `string` | Via format gate |
| `string` | `bool` | Coerce (compiler inserts `!= ""` -- only empty is false) |
| `exec` | `bool` | Coerce (true for one frame) |
| `bool`/`int`/`float`/`vector`/`entity`/`character`/`controller` | `exec` | Pulsing coerce |
| `ref T` | `ref U` (T != U) | **Mismatch** |
| `any` | anything | Same |
| anything | `any` | Same |

## Type Annotations

Type annotations appear after a colon in declarations:

```wirescript
var x: int = 0
in trigger: exec
var data: float[]
```

Type annotations are optional on `var` when an initializer is present (the type is inferred), but they are required on `in` declarations and `array` declarations.

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

count.Value  // Current value (type T) -- delayed read, usable in pure context
count.prev   // Previous tick's value (type T) -- useful for change detection
```

See [Execution Context](exec-context.md) for when to use `.Value` vs direct access.
