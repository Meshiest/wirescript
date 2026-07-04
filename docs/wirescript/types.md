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
| `brick` | Reference to a brick | null |
| `prefab` | Reference to a prefab | null |
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
| `any` | Universal type -- compatible with everything. Used internally. |
| `never` | Bottom type -- no value inhabits this type. Used internally. |

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

Arrays hold multiple values of the same element type. Declare them with the `array` keyword:

```wirescript
array scores: int[]
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

Union types represent values that can be one of several types:

```wirescript
// A chip output may produce different types depending on the branch
let result = if condition then 42 else 3.14
// result: int | float
```

Union types primarily arise from conditional expressions where branches produce different types.

## Type Coercion

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

The following types format to string: `bool`, `int`, `float`, `string`, `vector`, `rotator`, `color`, `entity`, `character`, `controller`, `brick`, `prefab`.

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
| `character` | `controller` | Coerce (auto ControllerOf) |
| `controller` | `character` | Coerce (auto CharacterOf) |
| `rotator` | `quat` | Coerce (interchangeable rotation values) |
| `quat` | `rotator` | Coerce |
| any primitive | `string` | Via format gate |
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
array data: float[]
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
