# Built-in Functions

Wirescript provides built-in functions that map directly to Brickadia wire graph gates. Each function is either **pure** (returns a value, no exec context needed) or **exec** (requires exec context and chains into the current execution flow).

## Notation

- **Pure** functions are expressions -- they produce a value and can be used anywhere.
- **Exec** functions require an active exec context (inside an `on` handler). They are called as statements or in exec expressions.
- Parameters marked with `?` are optional.

### Receiver Method Syntax

Many functions support **receiver method syntax**, where the first parameter is written before the dot instead of as a positional argument. Both forms are equivalent:

```wirescript
// Receiver form (preferred)
entity.SetLocation(pos)

// Traditional form
SetLocation(entity, pos)
```

Functions that support receiver syntax show both forms in the documentation below.

---

## Math / Trigonometry (Pure)

All trig functions take and return `float`. Angles are in **radians** unless converted.

| Function | Signature | Description |
|----------|-----------|-------------|
| `sin(x)` | `(x: float) -> float` | Sine |
| `cos(x)` | `(x: float) -> float` | Cosine |
| `tan(x)` | `(x: float) -> float` | Tangent |
| `asin(x)` | `(x: float) -> float` | Arc sine |
| `acos(x)` | `(x: float) -> float` | Arc cosine |
| `atan(x)` | `(x: float) -> float` | Arc tangent |
| `atan2(y, x)` | `(y: float, x: float) -> float` | Two-argument arc tangent |
| `sinh(x)` | `(x: float) -> float` | Hyperbolic sine |
| `cosh(x)` | `(x: float) -> float` | Hyperbolic cosine |
| `tanh(x)` | `(x: float) -> float` | Hyperbolic tangent |
| `asinh(x)` | `(x: float) -> float` | Inverse hyperbolic sine |
| `acosh(x)` | `(x: float) -> float` | Inverse hyperbolic cosine |
| `atanh(x)` | `(x: float) -> float` | Inverse hyperbolic tangent |
| `exp(x)` | `(x: float) -> float` | e^x |
| `ln(x)` | `(x: float) -> float` | Natural logarithm |
| `sign(x)` | `(x: float) -> float` | Sign (-1, 0, or 1) |
| `abs(x)` | `(x: float) -> float` | Absolute value |
| `sqrt(x)` | `(x: float) -> float` | Square root |
| `pow(x, exponent)` | `(x: float, exponent: float) -> float` | Power |
| `clamp(x, min, max)` | `(x: float, min: float, max: float) -> float` | Clamp to range |
| `round(x)` | `(x: float) -> float` | Round to nearest integer |
| `floor(x)` | `(x: float) -> float` | Round down |
| `ceil(x)` | `(x: float) -> float` | Round up |
| `min(a, b)` | `(a: float, b: float) -> float` | Minimum of two values |
| `max(a, b)` | `(a: float, b: float) -> float` | Maximum of two values |
| `log(x, base)` | `(x: float, base: float) -> float` | Logarithm with arbitrary base |
| `lerp(a, b, t)` | `(a: T, b: T, t: float) -> T` | Linear interpolation; `T` is any math variant (see Easing/Tween) |
| `fmod(a, b)` | `(a: float, b: float) -> float` | Floored modulo |
| `Deg2Rad(x)` | `(x: float) -> float` | Degrees to radians |
| `Rad2Deg(x)` | `(x: float) -> float` | Radians to degrees |

```wirescript
let angle = atan2(dy, dx)
let clamped = clamp(value, 0.0, 1.0)
let dist = sqrt(dx * dx + dy * dy)
let radians = Deg2Rad(90.0)
```

## Bitwise (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
| `BitCount(x)` | `(x: int) -> int` | Count set bits (popcount) |
| `BitNand(a, b)` | `(a: int, b: int) -> int` | Bitwise NAND (same as `~(a & b)`) |
| `BitNor(a, b)` | `(a: int, b: int) -> int` | Bitwise NOR (same as `~(a \| b)`) |

Note: `~(a & b)` and `~(a | b)` are automatically fused into single NAND/NOR gates by the compiler.

```wirescript
let bits = BitCount(flags)
```

## Vector (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
| `Vec(x, y, z)` | `(x: float, y: float, z: float) -> vector` | Construct a vector |
| `Dot(a, b)` | `(a: vector, b: vector) -> float` | Dot product |
| `Cross(a, b)` | `(a: vector, b: vector) -> vector` | Cross product |
| `Normalize(v)` | `(v: vector) -> vector` | Normalize to unit length |
| `Magnitude(v)` | `(v: vector) -> float` | Length of vector |
| `MagnitudeSq(v)` | `(v: vector) -> float` | Squared length (avoids sqrt) |
| `Distance(a, b)` | `(a: vector, b: vector) -> float` | Distance between two points |
| `DistanceSq(a, b)` | `(a: vector, b: vector) -> float` | Squared distance (avoids sqrt) |
| `ScaleVec(v, s)` | `(v: vector, scalar: float) -> vector` | Scale vector by scalar |
| `RotToDir(rot)` | `(rot: vector) -> vector` | Convert rotation to direction |
| `v.SplitVec()` | `(v: vector) -> {x, y, z: float}` | Decompose vector (receiver on `vector`) |

### Vector Receiver Methods

`DistanceSq`, `MagnitudeSq`, and `RotToDir` support receiver syntax on `vector`:

```wirescript
// Receiver form
let dsq = a.DistanceSq(b)
let msq = v.MagnitudeSq()
let dir = rot.RotToDir()

// Traditional form
let dsq = DistanceSq(a, b)
let msq = MagnitudeSq(v)
let dir = RotToDir(rot)
```

```wirescript
let pos = Vec(1.0, 2.0, 3.0)
let dir = Normalize(target - origin)
let dist = Distance(posA, posB)
let scaled = ScaleVec(velocity, 0.5)
```

## Rotation / Quaternion (Pure)

Two rotation types: `rotator` is euler (pitch/yaw/roll, used by entity rotation),
`quat` is a quaternion produced by the conversion gates. Methods use the concise
receiver form.

| Function | Signature | Description |
|----------|-----------|-------------|
| `Rotation(pitch, yaw, roll)` | `(float, float, float) -> rotator` | Construct an euler rotator |
| `r.ToEuler()` | `(rotator) -> {Pitch, Yaw, Roll: float}` | Split a rotator into components |
| `dir.ToRotation()` | `(vector) -> quat` | Quaternion that points along `dir` |
| `q.ToDirection()` | `(quat) -> vector` | Forward direction of `q` |
| `v.Rotate(q)` | `(vector, quat) -> vector` | Rotate a vector by a quaternion |
| `q.Invert()` | `(quat) -> quat` | Inverse rotation |
| `from.RotationTo(to)` | `(vector, vector) -> quat` | Quaternion rotating `from` onto `to` |
| `a.AngleTo(b)` | `(quat, quat) -> float` | Angle between two quaternions |
| `a.Slerp(b, alpha)` | `(quat, quat, float) -> quat` | Spherical interpolation |
| `axis.RotationByAngle(angle)` | `(vector, float) -> quat` | Quaternion from axis + angle (radians) |
| `q.ToAxisAngle()` | `(quat) -> {Axis: vector, Angle: float}` | Decompose into axis + angle |
| `Quat(x, y, z, w)` | `(float, float, float, float) -> quat` | Construct a quaternion from raw components |
| `q.SplitQuat()` | `(quat) -> {X, Y, Z, W: float}` | Decompose into raw components |
| `a.QuatDot(b)` | `(quat, quat) -> float` | Quaternion dot product |

```wirescript
let q = forward.ToRotation()
let spun = velocity.Rotate(q)
let mid = a.Slerp(b, 0.5)
let r = Rotation(0.0, 90.0, 0.0)   // euler rotator
let yaw = r.ToEuler().Yaw
```

## Color (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
| `Color(r, g, b, a?)` | `(r: float, g: float, b: float, a?: float) -> color` | Construct a color (linear RGBA, 0-1 range) |
| `ColorSRGB(r, g, b, a)` | `(int, int, int, int) -> color` | Construct from sRGB bytes (0-255) |
| `ColorHex(hex)` | `(string) -> color` | Construct from a hex string (`"#ff8800"`) |
| `c.SplitColor()` | `(c: color) -> {r, g, b, a: float}` | Decompose into linear components |
| `c.ToSRGB()` | `(color) -> {R, G, B, A: int}` | Decompose into sRGB bytes |
| `c.ToHex()` | `(color) -> string` | Hex string |
| `a.ColorBlend(b, alpha)` | `(color, color, float) -> color` | Blend two colors (colour-space aware) |

`SplitColor`, `ToSRGB`, `ToHex`, and `ColorBlend` support receiver syntax on `color`.

`Blend` is a different gate -- the math blend (an alias for `lerp`), which takes colours
as one of its variants but has no colour-space selection.

```wirescript
let red = Color(1.0, 0.0, 0.0)
let orange = ColorSRGB(255, 128, 0, 255)
let hex = orange.ToHex()
let parts = red.SplitColor()  // parts.r = 1.0, parts.g = 0.0, ...
let mixed = red.ColorBlend(orange, 0.5)
```

## Stateful Exec Values

| Function | Signature | Description |
|----------|-----------|-------------|
| `Cycle(count)` | `(count: int) -> int` exec | Returns 0,1,…,count-1 advancing each exec pulse |
| `Toggle()` | `() -> bool` exec | Flips between false/true each exec pulse |

## Select / Swap (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
| `Select(cond, a, b)` | `(cond: bool, a: any, b: any) -> any` | Returns `a` if false, `b` if true |
| `Swap(cond, a, b)` | `(cond: bool, a: any, b: any) -> {a, b: any}` | Conditionally swap two values |

```wirescript
let bigger = Select(x > y, y, x)
let result = Swap(shouldSwap, left, right)
// result.a and result.b are swapped if shouldSwap is true
```

## Edge / Change Detectors

| Function | Signature | Description |
|----------|-----------|-------------|
| `Edge(input)` | `(input: bool) -> {Rising, Falling: bool}` | Bool pulses on boolean transitions |
| `EdgeExec(input)` | `(input: float) -> {Rising, Falling: exec}` | Exec pulses when a value rises/falls |
| `Changed(input)` | `(input: any) -> bool` | Bool pulse when the input changes |
| `Change(input)` | `(input: any) -> any` | Pulse the input value through when it changes |

`Edge` and `Changed` are pure: they produce a one-tick bool pulse (`Rising` on
false→true, `Falling` on true→false; `Changed` on any change). `EdgeExec` and
`Change` are their exec-flavored siblings — `EdgeExec`'s outputs fire exec
chains directly (use with `on`/`await`, like `Timer(...).Expired`), and
`Change` pulses the new value through whenever the input changes.

```wirescript
let edges = Edge(button)
on edges.Rising { count = count + 1 }

let health = EdgeExec(hp)
on health.Falling { ctrl.ShowStatusMessage("taking damage!") }
```

## Logical XOR (`^^`)

The `^^` operator is boolean XOR — returns true if exactly one operand is true.

```wirescript
let either = a ^^ b  // true if a or b but not both
```

Note: `!(a && b)` and `!(a || b)` are automatically fused into single NAND/NOR gates.

## String Operations (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
All string functions support **receiver syntax** on `string`:

| Function | Signature | Description |
|----------|-----------|-------------|
| `s.Length()` | `(s: string) -> int` | String length |
| `s.Contains(search)` | `(s: string, search: string) -> bool` | Check if string contains substring |
| `s.StartsWith(prefix)` | `(s: string, prefix: string) -> bool` | Check prefix |
| `s.EndsWith(suffix)` | `(s: string, suffix: string) -> bool` | Check suffix |
| `s.Find(search, caseSensitive?)` | `(s: string, search: string, caseSensitive?: bool) -> int` | Find substring index (-1 if not found) |
| `s.Substring(start, length)` | `(s: string, start: int, length: int) -> string` | Extract substring |
| `s.Replace(search, replacement)` | `(s: string, search: string, replacement: string) -> string` | Replace occurrences |
| `s.Split(delimiter)` | `(s: string, delimiter: string) -> {Left, Right: string}` | Split at first delimiter |
| `s.ToLower()` | `(s: string) -> string` | Convert to lowercase |
| `s.ToUpper()` | `(s: string) -> string` | Convert to uppercase |
| `s.Trim()` | `(s: string) -> string` | Remove leading/trailing whitespace |
| `s.ParseInt()` / `ParseInt(s)` | `(s: string) -> int` | Parse an integer from text |
| `s.ParseNumber()` / `ParseNumber(s)` | `(s: string) -> float` | Parse a number from text |

```wirescript
let name = "Hello World"
let len = name.Length()              // 11
let has = name.Contains("World")     // true
let low = name.ToLower()             // "hello world"
let sub = name.Substring(6, 5)      // "World"
let parts = name.Split(" ")         // parts.Left = "Hello", parts.Right = "World"
```

## String Formatting (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
| `Fmt(format, a?, b?, c?, d?, e?, f?, g?)` | `(format: any, a-g?: any) -> string` | Format text with placeholders |

The `Fmt` function wraps the FormatText gate. The format string uses `{0}` through `{6}` placeholders corresponding to inputs `a` through `g`.

```wirescript
let label = Fmt("{0}: {1}", "Score", score)
let coords = Fmt("({0}, {1}, {2})", x, y, z)

// Also works for palette selection:
let col = Fmt('{' .. bucket .. '}', 'eee4da', 'f2b179', 'f65e3b')
```

## Array Methods (Exec)

Methods on an `array` variable. All run in exec context (they lower to ArrayVar
exec gates), so call them inside `on` handlers / mods. Declare arrays with
`array name: T[]` (see [statements](statements.md)).

| Method | Signature | Description |
|--------|-----------|-------------|
| `arr.push(value)` | `(value: T)` | Append an element |
| `arr.pop()` | `() -> T` | Remove and return the last element |
| `arr.length()` | `() -> int` | Number of elements |
| `arr.remove(index)` | `(index: int)` | Remove the element at `index` |
| `arr.insert(index, value)` | `(index: int, value: T)` | Insert before `index` |
| `arr.clear()` | `()` | Remove all elements |
| `arr.find(value)` | `(value: T) -> int` | Index of first match (-1 if absent) |
| `arr.sort(descending?)` | `(descending?: bool)` | Sort in place |
| `arr.reverse()` | `()` | Reverse in place |
| `arr.shuffle()` | `()` | Randomly reorder |
| `arr.swap(a, b)` | `(a: int, b: int)` | Swap two elements |
| `arr.fill(value)` | `(value: T)` | Set every element to `value` |
| `arr.resize(size, value)` | `(size: int, value: T)` | Grow/shrink, filling new slots with `value` |
| `arr.sum()` | `() -> T` | Sum of elements |
| `arr.min()` / `arr.max()` | `() -> T` | Smallest / largest element |
| `arr.average()` | `() -> float` | Mean of elements |
| `arr.append(source)` | `(source: T[])` | Append all elements of another array |
| `arr.copyFrom(source)` | `(source: T[])` | Replace contents with a copy of another array |
| `arr.slice(source, start, count)` | `(source: T[], start: int, count: int)` | Copy `source[start..start+count]` into this array |
| `arr.fillFromPlayers()` | `()` | Fill with all current players |
| `arr.fillFromTeam(team)` | `(team: entity)` | Fill with the members of a team |

Element access uses bracket syntax: `arr[i]` reads (with `.value` / `.bOutOfBounds`),
`arr[i] = x` writes.

**`exec =`.** Any exec-gate call — an array method, a builtin, or a mod/chip call —
accepts an `exec = <trigger>` argument that drives its exec input, firing the gate each
time the trigger's value changes. A per-index, always-nonzero trigger like `index + 1`
turns an array into a single-gate lookup table read straight from a pure binding:

```wirescript
var lut: color[] = [ /* ...constant entries... */ ]
out c: color = lut.get(i, exec = i + 1).Value
```

```wirescript
var scores: int[]
on RoundEnd {
  scores.push(currentScore)
  scores.sort(true)          // descending
  let best = scores.max()
  let count = scores.length()
}
```

## Player Input (Exec)

### InputReader
```
character.InputReader() -> { Forward, Right, Up, Pitch, Yaw, Roll, MouseWheel, PressedC, PressedE, PressedQ, PressedLeftMouse, PressedRightMouse }
InputReader(character: character) -> { ...same fields... }
```

Read player input axes and pressed keys. Receiver on `character`.

Returns a record with fields:
- `Forward: float` -- forward/backward movement axis (-1 to 1)
- `Right: float` -- left/right movement axis (-1 to 1)
- `Up: float` -- up/down movement axis (-1 to 1)
- `Pitch: float` / `Yaw: float` / `Roll: float` -- look axes
- `MouseWheel: float` -- mouse wheel delta
- `PressedC` / `PressedE` / `PressedQ` / `PressedLeftMouse` / `PressedRightMouse: bool` -- key/button states

```wirescript
let input = char.InputReader()
let moving = input.Forward != 0.0 || input.Right != 0.0
let interacting = input.PressedE
```

## Controller / Character Conversions (Exec)

These functions convert between entity types. They require exec context and support receiver syntax.

### ControllerOf
```
entity.ControllerOf() -> controller
ControllerOf(entity: entity) -> controller
```

Get controller from entity. Receiver on `entity`.

### CharacterOf
```
controller.CharacterOf() -> character
CharacterOf(controller: controller) -> character
```

Get character from controller. Receiver on `controller`.

```wirescript
on CharacterSpawned(character) {
  let ctrl = character.ControllerOf()
  ctrl.DisplayText("Welcome!", fontSize = 24)
}
```

## Camera / Aim (Exec)

### GetAim
```
character.GetAim() -> { Origin: vector, Direction: vector }
GetAim(character: character) -> { Origin: vector, Direction: vector }
```

Reads the character's camera/aim in a single gate. Returns a record:
- `Origin: vector` — aim origin position
- `Direction: vector` — aim direction vector

Receiver on `character`. Access the fields with `.Origin` / `.Direction`; both
share one gate, so reading both costs a single GetAim.

```wirescript
on trigger {
  let aim = char.GetAim()
  let origin = aim.Origin
  let dir = aim.Direction
}
```

## Display (Exec)

### DisplayText
```
target.DisplayText(text, ...) -> int
DisplayText(target: controller, text: any, ...) -> int
```

Display HUD text to a player. Receiver on `controller`. Returns the resolved
`textId` (an `int`) so a later call can update or clear the same on-screen text.

The gate's position/anchor/scale are now composite (2D) properties and are not
settable through the call form; layout is controlled in-game. The call exposes the
scalar styling below (plus `fontSize` / `justify` / `easing`, which remain as
constant-only data fields).

#### DisplayText Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `target` | `controller` | Yes | Player to display to |
| `text` | `any` | Yes | Text content (auto-converted to string) |
| `angle` | `float` | No | Rotation angle |
| `outlineSize` | `int` | No | Text outline size |
| `outlineColor` | `color` | No | Outline color |
| `fontColor` | `color` | No | Font color |
| `shadowColor` | `color` | No | Drop-shadow color |
| `miteredOutline` | `bool` | No | Sharp (mitered) outline corners |
| `letterSpacing` | `float` | No | Extra spacing between letters |
| `lineHeight` | `float` | No | Line-height multiplier |
| `wrapWidth` | `float` | No | Wrap width (0 = no wrap) |
| `skew` | `float` | No | Italic-style skew |
| `zOrder` | `int` | No | Draw order |
| `lifetime` | `float` | No | Display duration (seconds) |
| `transition` | `float` | No | Seconds to interpolate to the new state when re-emitted with the same `textId` |
| `textId` | `int` | No | Unique ID for updating text in-place |
| `fontSize` | `float` | No | Font size (constant only) |
| `justify` | `int` | No | Justification — `"Left"` / `"Center"` / `"Right"` (constant only) |
| `easing` | `int` | No | Transition curve — `"Linear"` / `"EaseIn"` / `"EaseOut"` / `"EaseInOut"` (constant only) |

```wirescript
on RoundStart {
  ctrl.DisplayText("Round Start!", fontSize = 48, lifetime = 3.0)
}

// Update text in-place: capture the id, then re-display with the same textId.
on trigger {
  let id = ctrl.DisplayText("Score: ${score}", fontSize = 24, lifetime = 10.0)
  ctrl.DisplayText("Score: ${score}", textId = id, transition = 0.25)
}
```

## Entity Getters (Exec)

All entity getter functions require exec context and support receiver syntax on `entity`.

### GetLocation
```
entity.GetLocation() -> vector
GetLocation(entity: entity) -> vector
```

Get entity's world position.

### GetRotation
```
entity.GetRotation() -> rotator
GetRotation(entity: entity) -> rotator
```

Get entity's world rotation.

### GetLocationRotation
```
entity.GetLocationRotation() -> {Vector: vector, Rotation: rotator}
GetLocationRotation(entity: entity) -> {Vector: vector, Rotation: rotator}
```

Get both position and rotation at once.

### GetLinearVelocity
```
entity.GetLinearVelocity() -> vector
GetLinearVelocity(entity: entity) -> vector
```

Get entity's linear velocity.

### GetAngularVelocity
```
entity.GetAngularVelocity() -> vector
GetAngularVelocity(entity: entity) -> vector
```

Get entity's angular velocity.

### GetVelocity
```
entity.GetVelocity() -> {Vector: vector, Rotation: rotator}
GetVelocity(entity: entity) -> {Vector: vector, Rotation: rotator}
```

Get both linear and angular velocity at once.

```wirescript
on trigger {
  let pos = entity.GetLocation()
  let rot = entity.GetRotation()
  let vel = entity.GetLinearVelocity()
}
```

## Entity Manipulation (Exec)

All entity manipulation functions require exec context and support receiver syntax on `entity`.

### SetLocation
```
entity.SetLocation(pos: vector)
SetLocation(entity: entity, pos: vector)
```

Set entity position. Use this (not `Teleport`) to move an entity to world
coordinates. `pos` is a real `vector` port.

### SetRotation
```
entity.SetRotation(rot: rotator)
SetRotation(entity: entity, rot: rotator)
```

Set entity rotation.

### SetLocationRotation
```
entity.SetLocationRotation(pos: vector, rot: rotator)
SetLocationRotation(entity: entity, pos: vector, rot: rotator)
```

Set both position and rotation.

### AddLocationRotation
```
entity.AddLocationRotation(pos: vector, rot: rotator)
AddLocationRotation(entity: entity, pos: vector, rot: rotator)
```

Add to position and rotation.

### Teleport
```
entity.Teleport(dest: any)
Teleport(entity: entity, dest: any)
```

Teleport entity to destination.

### RelativeTeleport
```
entity.RelativeTeleport(source: any, dest: any)
RelativeTeleport(entity: entity, source: any, dest: any)
```

Relative teleport between two points.

### SetVelocity
```
entity.SetVelocity(linear?: vector, angular?: vector)
SetVelocity(entity: entity, linear?: vector, angular?: vector)
```

Set velocity. Both `linear` and `angular` are optional -- pass whichever components you want to set.

### AddVelocity
```
entity.AddVelocity(linear?: vector, angular?: vector)
AddVelocity(entity: entity, linear?: vector, angular?: vector)
```

Add to velocity. Both `linear` and `angular` are optional.

### SetLinearVelocity
```
entity.SetLinearVelocity(vel: vector)
SetLinearVelocity(entity: entity, vel: vector)
```

Set linear velocity only.

### SetAngularVelocity
```
entity.SetAngularVelocity(vel: vector)
SetAngularVelocity(entity: entity, vel: vector)
```

Set angular velocity only.

### SetGravityDirection
```
entity.SetGravityDirection(rot: rotator)
SetGravityDirection(entity: entity, rot: rotator)
```

Set gravity direction for entity.

### SetFrozen
```
entity.SetFrozen(frozen: bool)
SetFrozen(entity: entity, frozen: bool)
```

Freeze or unfreeze an entity's physics.

```wirescript
on trigger {
  entity.SetLocation(Vec(0.0, 0.0, 100.0))
  entity.SetVelocity(linear = Vec(0.0, 0.0, 500.0))
  entity.AddVelocity(linear = direction, angular = Vec(0.0, 90.0, 0.0))
}
```

## Gamemode (Exec)

### SetLeaderboard
```
controller.SetLeaderboard(key: string, value: any)
SetLeaderboard(controller: controller, key: string, value: any)
```

Set a leaderboard value. Receiver on `controller`.

### IncLeaderboard
```
controller.IncLeaderboard(key: string, value: any)
IncLeaderboard(controller: controller, key: string, value: any)
```

Increment a leaderboard value. Receiver on `controller`.

### GetLeaderboard
```
controller.GetLeaderboard(key: string) -> any
GetLeaderboard(controller: controller, key: string) -> any
```

Get a leaderboard value. Receiver on `controller`.

### GetTeam
```
character.GetTeam() -> any
GetTeam(character: character) -> any
```

Get a character's team. Receiver on `character`.

### IsBuilderTeam / IsUnaffiliatedTeam
```
team.IsBuilderTeam() -> bool
IsBuilderTeam(team: entity) -> bool
team.IsUnaffiliatedTeam() -> bool
IsUnaffiliatedTeam(team: entity) -> bool
```

Pure predicates over a `team` entity: `IsBuilderTeam` is true for the builder
team, `IsUnaffiliatedTeam` for the unaffiliated (no-team) group. Receiver on the
team entity.

### PlayerWins / TeamWins
```
player.PlayerWins(teamWinsInstead?: bool)
PlayerWins(player: controller, teamWinsInstead?: bool)
team.TeamWins()
TeamWins(team: entity)
```

End the current round by declaring a winner. (The old imperative `EndRound`
gate was removed; a round now ends via a win.) `PlayerWins` declares a player
the winner, or their team if `teamWinsInstead` is true; `TeamWins` declares a
team the winner.

### GetCurrentRound
```
GetCurrentRound() -> int
```

The current round number.

### GetTeamByName / GetTeamName
```
GetTeamByName(name: string) -> entity
team.GetTeamName() -> string
GetTeamName(team: entity) -> string
```

Look up a team by name, or get a team's display name.

### SetTeam
```
controller.SetTeam(team: entity, pin?: bool)
SetTeam(controller: controller, team: entity, pin?: bool)
```

Assign a player to a team, optionally pinning them to it.

### Team leaderboards
```
team.GetTeamLeaderboardValue(key: string) -> int
team.SetTeamLeaderboardValue(key: string, value: int)
team.IncrementTeamLeaderboardValue(key: string, value: int)
```

Read, set, or add to a team-scoped leaderboard value. Receiver on the team
`entity` (also callable as free functions with `team` as the first argument).

```wirescript
on CharacterDied(character) {
  let ctrl = character.ControllerOf()
  ctrl.IncLeaderboard("deaths", 1)
  let score = ctrl.GetLeaderboard("score")
}
```

## Character (Exec)

### ShowHint
```
character.ShowHint(title: string, text: string)
ShowHint(character: character, title: string, text: string)
```

Display a hint popup to a character. Receiver on `character`.

```wirescript
on CharacterSpawned(character) {
  character.ShowHint("Welcome", "Press E to interact")
}
```

### Damage
```
character.GetDamage() -> { Damage: float, DamageLimit: float }
character.SetDamage(damage: float)
character.IncDamage(amount: float)
```

Read, set, or add to a character's accumulated damage. Receiver on `character`.
`GetDamage()` auto-unwraps to `Damage` where a `float` is expected (e.g.
`if char.GetDamage() > 50.0`), and `.DamageLimit` gives the death threshold.

### SetTempPermission
```
character.SetTempPermission(permission: string, enable: bool)
```

Grant or revoke a temporary permission tag on a character. Receiver on `character`.

### Inventory
```
character.GiveWeapon(weapon, slot?)                  // set a slot to an item asset
character.AddInventoryItem(item)                     // append an item
character.SetInventoryItem(item, slot?)              // set a slot to an item
character.AddInventoryBrick(brick, size?)            // append a placeable brick
character.SetInventoryBrick(brick, slot?, size?)
character.AddInventoryEntity(entityType)             // append a spawnable entity
character.SetInventoryEntity(entityType, slot?)
character.AddInventoryItemAdv(item, damage?, speed?, scale?, itemName?, projectile?)
character.SetInventoryItemAdv(item, slot?, damage?, speed?, scale?, itemName?, projectile?)
```

Give items, procedural bricks, or spawnable entities to a character's
inventory. Asset args are `$Type/Name` references — `$BRItemBase/...` for
items, a brick asset for bricks, an entity type for entities — inlined into
the gate's data. The `Adv` variants add per-item overrides: damage/weapon
speed/scale multipliers, a display-name override, and a projectile override.
All receive on `character`.

```wirescript
on CharacterSpawned(character) {
  character.GiveWeapon($BRItemBase/Weapon_Pistol, 0)
  character.AddInventoryItemAdv($BRItemBase/Weapon_Bow,
    damage = 2.0, itemName = "Longbow of Doom")
}
```

## Controller (Exec)

### ShowStatusMessage
```
controller.ShowStatusMessage(message: string)
ShowStatusMessage(controller: controller, message: string)
```

Display a status bar message to a player. Receiver on `controller`.

```wirescript
on RoundStart {
  ctrl.ShowStatusMessage("Round started!")
}
```

### ShowChatMessage
```
controller.ShowChatMessage(message: string)
ShowChatMessage(controller: controller, message: string)
```

Send a chat message that only this player sees (a whisper). Receiver on
`controller`.

### ShowMessageBox
```
controller.ShowMessageBox(message: string, title?: string)
```

Pop up a modal message box for this player. Receiver on `controller`.

### Player info
```
controller.GetUserName() -> string
controller.GetUserId() -> string
controller.GetDisplayName() -> string
controller.IsTrusted() -> bool
controller.HasPermission(permission: string) -> bool
controller.SetCanRespawn(canRespawn: bool)
controller.SetTeamPinned(pinned: bool)
```

Read a player's account name, persistent user id, or current display name;
check whether they are trusted by the brick owner or hold a named permission;
toggle their ability to respawn; or pin them to their team. All receive on
`controller`.

## Broadcast Messaging (Exec)

```
BroadcastChatMessage(message: string)
BroadcastStatusMessage(message: string, flash?: bool)
```

Send a chat message or status-bar message to **every** player. `flash`
re-flashes the status message even when its text is unchanged.

```wirescript
on roundEnd {
  BroadcastChatMessage("Red team wins!")
  BroadcastStatusMessage("Round over", flash = true)
}
```

## Audio (Exec)

```
entity.PlayAudioAt(audio, volume?, pitch?, innerRadius?, maxDistance?, spatialized?)
PlayGlobalAudio(audio, volume?, pitch?)
```

Play a one-shot sound at an entity's location (spatialized by default) or
globally for all players. The `audio` arg is a
`$BrickOneShotAudioDescriptor/...` asset reference. `PlayAudioAt` receives on
`entity` (characters work too).

```wirescript
on ZoneEntered(character) {
  character.PlayAudioAt($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press)
}
```

## Entity Tags (Exec)

```
entity.SetTag(tag: string)
entity.GetTag() -> string
```

Attach an arbitrary string tag to any entity and read it back later — handy
for marking players/entities with game state (team, slot index, role). Zone
components can also filter on tags. Receiver on `entity`.

## Misc (Pure / Exec)

| Function | Signature | Description |
|----------|-----------|-------------|
| `FindPlayer(query)` | `(query: string) -> character` (exec) | Look up a player by name; emits their character |
| `PrintToConsole(text)` | `(text: any) -> ()` (exec) | Print a value to the game console (debugging) |
| `Opaque(value)` | `(value: any) -> any` (pure) | Identity rerouter; the permanent constant-fold barrier — the wrapped value always stays a real runtime wire, never folded or seen through (probe/test circuits; see [Constant Folding](folding.md)) |
| `DeltaTime()` | `() -> float` | Seconds elapsed since the previous tick |
| `ServerUptime()` | `() -> float` | Seconds the server has been running |
| `ReadBrickGrid()` | `() -> entity` | The brick grid this gate's microchip is on, as an entity |
| `NearlyEqual(a, b, tolerance)` | `(a: float, b: float, tolerance: float) -> bool` | Approximate float equality |
| `Dampen(target, smoothTime)` | `(target: float, smoothTime: float) -> float` | Critically-damped smoothing toward a target |
| `Easing(a, b, blend, fn?, dir?)` | `(a: T, b: T, blend: float, fn?: any, dir?: any) -> T` | Ease from `a` to `b` by `blend` |
| `Tween(target, duration, fn?, dir?)` | `(target: T, duration: float, fn?: any, dir?: any) -> T` | Stateful eased value toward `target` |
| `Timer(limit, restart?, pause?, resume?)` | `(limit: float, restart?/pause?/resume?: exec) -> {Time: float, Expired: exec}` | Stateful countdown timer |

`Blend`/`lerp`/`Easing`/`Tween` interpolate any one math variant `T`: float|int|vector|rotator|quat|color.
The result is whatever `T` the inputs carry.

`Easing`/`Tween` take an easing `fn` and `dir`: either an int or an enum-name
literal. Functions: `Linear`, `Sine`, `Quad`, `Cubic`, `Quart`, `Quint`,
`Expo`, `Circ`, `Back`, `Elastic`, `Bounce`. Directions: `In`, `Out`, `InOut`.
Omitted, they default to `Linear`/`In`.

`Timer` is a function-call instance. The `restart`/`pause`/`resume` exec
controls are optional; its outputs are a value (`Time`) and an exec (`Expired`):

```wirescript
in trigger: exec
let t = Timer(10.0, restart = trigger)
out elapsed = t.Time
on t.Expired { /* fired when Time reaches the limit */ }
```

`ReadBrickGrid()` is pure and takes no arguments — it returns the brick grid
that this gate's microchip is placed on as an `entity`, ready to pass to entity
getters/setters or wire into gates that expect a brick grid:

```wirescript
let grid = ReadBrickGrid()
let origin = grid.GetLocation()
```

## Gate config properties

Some gate settings are not wire inputs — they are the checkboxes, dropdowns,
and values in a gate's in-game **settings menu**. Wirescript sets them as
**optional, constant-only call arguments**; a constant is baked into the gate's
data, and anything you omit keeps the game default. (A non-constant value is a
compile error — these can't be wired.)

**Enum values are bare member names**, validated against the game's own enum
member list at compile time. An unknown name is a `WS028` error; a raw int is
also accepted (and range-checked).

```wirescript
let e = Easing(0.0, 1.0, t, function = Bounce, direction = InOut)
let c = ColorBlend(a, b, t, blendSpace = Oklab, clampAlpha = true)
p.DisplayText("hi", typeface = Bold, justify = Center, easing = EaseInOut)
```

**Every** config field is settable — not just the aliases below. In addition to
the friendly names, each gate exposes each `bool`/`int`/`float`/`string`/`enum`
settings-menu field under its **raw game name**, so any of the ~60 config gates
works even without a curated alias:

```wirescript
SweepSimple(500.0, Direction = X_Negative, bOnlyHitPlayerBodyParts = true)
p.DisplayText("hi", FontSize = 40, Typeface = Bold, Justification = Center)
```

Completion offers these raw field names, and hovering one shows its type (and
enum members). The friendly alias and the raw name set the same field, so pick
either; the table below lists the ergonomic aliases for the common gates.

Config attributes by gate:

| Gate | Config attributes |
|------|-------------------|
| `Sweep` | `bodyPartsOnly` |
| `SweepSimple` | `direction` (EBrickDirection), `spreadTowardCenter`, `detectBricks`, `detectPlayers1`–`4`, `bodyPartsOnly`, `detectPhysics`, `detectMap` |
| `Blend` | `clampAlpha` |
| `ColorBlend` | `blendSpace` (EBRColorSpace), `clampAlpha` |
| `Slerp` | `shortestPath`, `clampAlpha` |
| `Easing` | `function` (EBREasingFunction), `direction` (EBREasingDirection) |
| `ConvertColor` | `fromSpace`, `toSpace` (EBRColorSpace) |
| `DisplayText` | `fontSize`, `justify` (EBRDisplayTextJustification), `easing` (EBRDisplayTextEasing), `typeface` (EBRTextTypeface), `font` (a `$Font/…` asset ref) |
| `GetAim` | `localAim` |
| `AddInventoryItemAdv` / `SetInventoryItemAdv` | `overrideColors`, `meshColors` (a color array), `ammoOverride` |

Enum member names: **EBRColorSpace** `Linear Srgb Oklab Hsv`; **EBREasingFunction**
`Linear Sine Quad Cubic Quart Quint Expo Circ Back Elastic Bounce`;
**EBREasingDirection** `In Out InOut`; **EBRTextTypeface** `Regular Bold Italic BoldItalic`;
**EBRDisplayTextJustification** `Left Center Right`; **EBRDisplayTextEasing**
`Linear EaseIn EaseOut EaseInOut`; **EBrickDirection** `X_Positive X_Negative
Y_Positive Y_Negative Z_Positive Z_Negative`.

## Clock (Event)

The Clock gate emits a periodic execution pulse forever; it reads as an event.
`interval` and `enabled` are **wire inputs** — they may be constants (baked) or
dynamic (a variable wires in), so you can toggle the clock at runtime. `pulseOn`,
`onTime`, and `offTime` are constant-only settings-menu config. The handler body
runs on each pulse.

```wirescript
in running: bool
var ticks: int = 0
on Clock(interval = 2.0, enabled = running, pulseOn = false, onTime = 0.5, offTime = 0.5) {
  ticks = ticks + 1
}
```

## ChatCommand (Event)

Registers a chat command. The trigger takes both **config args** (the command
name and an optional description) and **binding params** (the event's data
outputs), distinguished by form:

- **String literals** fill the config fields in order: `CommandName`, then
  `HelpText`. The description can also be given by name as `Description = "..."`.
- **Bare identifiers** bind the event's data outputs, in order: `controller`
  (the player who typed it), then `arguments` (the command text as a string).

```wirescript
on ChatCommand("greet", "Greets the player", controller, arguments) {
  // CommandName = "greet", HelpText = "Greets the player"
  // controller: the player who typed the command
  // arguments: the command text as a string
  controller.ShowStatusMessage("You said: ${arguments}")
}
```

The description is optional and can use the named form. Binding params are also
optional — omit the ones you don't need:

```wirescript
on ChatCommand("wave", Description = "Wave at everyone") {
  // no bindings needed
}
```

## Custom Events

A named, cross-gate event channel that carries up to **8 data values**. Each comes
in two flavours: **personal** (same-owner) and **global** (ownership-agnostic), on
**separate** channel namespaces — a personal `"x"` and a global `"x"` never mix.

### SendCustomEvent (Exec)

`SendCustomEvent(name, data1, … data8, target = …)` — `name` is the channel, a
**constant** string baked into the gate (a variable or computed value is a `WS028`
error), followed by up to 8 optional data values of any type. `target` is an
optional entity whose grid receives the matching **object** events. Delivery is
same-owner; use `SendGlobalCustomEvent` for the ownership-agnostic version. Fires
all matching receivers.

```wirescript
on hit {
  SendCustomEvent("damage", 7, attacker)   // send an int + a character
}
```

### on CustomEvent (Event)

The receiver's first argument is the channel name (positional), and the
remaining params are the **typed data outputs**. The type annotations are
**required** — the game stores each data slot as a typed value, not `any`, so a
receiver param without a type is a **`WS029`** lint. Unused slots default to
`float`. `objectEvent = true` is constant config that scopes the receiver to a
specific grid/object (an **object event**) instead of firing grid-wide.

```wirescript
var lastDamage: int = 0   // a top-level var is already persistent — no `static`

on CustomEvent("damage", amount: int, attacker: character) {
  lastDamage = amount
  attacker.ShowStatusMessage("You took ${amount} damage")
}
```

### Global variants — SendGlobalCustomEvent / on GlobalCustomEvent

`SendGlobalCustomEvent` and `on GlobalCustomEvent` are the **ownership-agnostic**
counterparts: delivery ignores the owner, reaching every matching global receiver.
They have the same shape (constant channel name, up to 8 typed data values,
optional `target` entity, `objectEvent` config), on a channel namespace that is
**separate** from the personal one — `SendGlobalCustomEvent("x")` reaches
`on GlobalCustomEvent("x")` but never `on CustomEvent("x")`, and vice versa.

```wirescript
var total: int = 0

on GlobalCustomEvent("score", points: int) { total = total + points }
on hit { SendGlobalCustomEvent("score", 10) }
```

> **One-tick delay.** A `CustomEvent` receiver fires on the **tick after** the
> `SendCustomEvent` runs — the pulse is delivered on the next frame, not
> synchronously within the sender's exec chain. Anything that must observe the
> event's effect immediately has to account for that one-tick latency (and a
> send → receive → send round trip costs a tick each hop).

> **Signature checking (`WS030`).** When a send targets a **constant** channel name,
> the compiler compares each data value's wire type against the matching receiver's
> declared param types and warns (`WS030`) on a mismatch — e.g. sending a `float` where
> the receiver declared `int`. This runs for both the personal (`SendCustomEvent` /
> `on CustomEvent`) and global (`SendGlobalCustomEvent` / `on GlobalCustomEvent`) pairs,
> each within its own namespace. Types that share a
> wire variant are interchangeable and never flagged (any two entity kinds —
> `character`/`entity`/`controller`/… — are all the same `Object` variant). The channel
> name must be a constant literal, so every send's receiver set is known at compile time.
> In the editor, **go-to-definition** on a send's channel-name string jumps to the receiver.
>
> A *non-constant* data value is still typed as `float` on the wire at emit rather than
> from the value's real type — full end-to-end typing waits on generics. For now the
> receiver's annotations are the source of truth, and constant sends carry their type.

## Prefab Spawning (Exec)

### SpawnPrefab

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `prefab` | prefab ref | No | The prefab to spawn — a `$./file.brz` / `$/abs.brz` [prefab reference](expressions.md#prefab-references). Embedded into the bundle at compile. |
| `offset` | `vector` | No | Spawn position offset |
| `rotation` | `rotator` | No | Spawn rotation offset |
| `velocity` | `vector` | No | Initial velocity of the spawned entity |
| `lifetime` | `float` | No | Lifetime in seconds (0 = permanent) |
| `limit` | `int` | No | Max concurrent instances |
| `destroyAll` | `exec` | No | Wire an exec here; pulsing it destroys every entity this gate has already spawned. Independent of the spawn `Exec`, so one gate both spawns and clears. |

Returns: `entity` -- the spawned entity. Give the prefab with a `$…brz`
reference; omit `prefab` to configure it on the placed gate in-game instead
(copy a prefab onto the Spawn Prefab brick).

```wirescript
on trigger {
  let spawned = SpawnPrefab(
    prefab = $./turret.brz,
    offset = Vec(0.0, 0.0, 50.0),
    lifetime = 10.0,
    limit = 5
  )
  spawned.SetVelocity(linear = launchDir)
}
```

`destroyAll` is a secondary exec trigger (like `Timer`'s `restart`): wire a reset /
round-start signal into it to remove every entity this spawner has produced, without
re-spawning. The gate spawns when its own exec chain fires and clears when
`destroyAll` fires:

```wirescript
in reset: exec
on trigger {
  let cube = SpawnPrefab(prefab = $./msg.brz, limit = 64, destroyAll = reset)
  cube.SetTag(payload)
}
// pulsing `reset` destroys every cube this gate has spawned
```

A `$./file.brz` reference reads the `.brz` at compile and embeds it into the
output bundle (content-addressed at `Prefabs/Uploads/<hash>.brz`), so the
compiled program carries its prefab. See
[Prefab References](expressions.md#prefab-references).

### SpawnExplosion

Spawns an explosion of a given projectile/explosion class.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `projectileType` | class ref | Yes | The explosion/projectile class — a `$…` asset reference (or a wired value) |
| `instigator` | `entity` | No | The character/entity that caused it (kill credit, etc.) |
| `offset` | `vector` | No | Spawn position offset |
| `scale` | `float` | No | Explosion scale multiplier |
| `damage` | `float` | No | Damage multiplier |

```wirescript
on hit {
  SpawnExplosion($BRWeaponProjectile/Grenade, instigator = attacker, scale = 2.0, damage = 1.5)
}
```

### SpawnExplosionAt

Like `SpawnExplosion`, but at an absolute **world position** instead of an offset
from the gate's brick.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `worldPosition` | `vector` | Yes | Absolute world position to spawn the explosion at |
| `projectileType` | class ref | Yes | The explosion/projectile class — a `$…` asset reference (or a wired value) |
| `instigator` | `entity` | No | The character/entity that caused it (kill credit, etc.) |
| `scale` | `float` | No | Explosion scale multiplier |
| `damage` | `float` | No | Damage multiplier |

```wirescript
on hit {
  SpawnExplosionAt(Vec(0.0, 0.0, 200.0), $BRWeaponProjectile/Grenade, instigator = attacker)
}
```

## Raycasting (Exec)

### Sweep

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `origin` | `vector` | Yes | Ray start position |
| `direction` | `vector` | Yes | Ray direction |
| `Distance` | `float` | Yes | Maximum ray distance |
| `radius` | `float` | No | Sphere radius (0 = line trace) |
| `relative` | `bool` | No | Interpret `origin`/`direction` in the owning grid's local frame |
| `ignore` | `entity` | No | A single entity to exclude from hits |
| `ignoreList` | `entity[]` | No | An array var of additional entities to exclude (on top of `ignore`) |
| `ignoreOwningGrid` | `bool` | No | Exclude the grid this gate sits on (prevents self-hits) |
| `collisionChannel` | `int` | No | Collision channel to sweep on (`EBRSweepCollisionChannel`: 0 Physics, 1 Weapon, 2 Interaction, 3 Tool, 4–7 Player1–4, 8 NoAdditionalRestriction) |
| `detectBricks` | `bool` | No | Detect brick grids, including spawned prefabs — **default false** |
| `detectMap` | `bool` | No | Detect the static world / environment — **default false** |
| `detectPhysics` | `bool` | No | Detect physics-simulating objects — **default false** |
| `detectPlayers1`–`detectPlayers4` | `bool` | No | Detect players on collision channels 1–4 — **default false** |

> **Detection is opt-in.** Every `detect*` flag defaults to `false`, so a Sweep
> with none set **detects nothing and always fires `Miss`.** Enable the channel you
> want: `detectBricks` for brick grids / spawned prefabs, `detectPlayers1` for
> players, `detectPhysics` for loose physics objects.

Returns a record with fields:
- `HitDistance: float` -- Distance to hit point
- `HitEntity: entity` -- Entity that was hit
- `HitLocation: vector` -- World position of hit
- `HitNormal: vector` -- Surface normal at hit
- `Hit: exec` -- Fires if something was hit
- `Miss: exec` -- Fires if nothing was hit

```wirescript
on trigger {
  let aim = char.GetAim()
  // Run the Sweep INSIDE the exec handler; handle it with nested Hit/Miss branches.
  // A top-level `Sweep(..., exec = t)` does NOT fire.
  let r = Sweep(aim.Origin, aim.Direction, 10000.0,
    radius = 5.0, ignore = char, detectPlayers1 = true)
  on r.Hit  { r.HitEntity.ShowStatusMessage("hit!") }
  on r.Miss { /* nothing in range */ }
  // If should also work here
  if r.Hit  { r.HitEntity.ShowStatusMessage("hit!") }
  if r.Miss { /* nothing in range */ }
}
```

## Random (Exec)

| Function | Signature | Description |
|----------|-----------|-------------|
| `Random(min, max)` | `(min: int, max: int) -> int` | Random integer in `[min, max]` |
| `Random(min, max)` | `(min: T, max: T) -> T`, `T` ∈ `vector`/`rotator`/`quat`/`color` | Per-component random of the same type |

```wirescript
on RoundStart {
  let r = Random(0, 15)
  if r == 0 { specialEvent = true }
}
```

`Random` rides the same PrimMath variant as the [arithmetic operators](expressions.md#arithmetic-operators), so its `min`/`max` may be a `vector`, `rotator`, `quat`, or `color` — it then rolls each component independently and returns that same type. `Random(Vec(0.0, 0.0, 0.0), Vec(1.0, 1.0, 1.0))` is a random point in the unit cube; `Random(a, b)` on two colors is a random color between them (all four RGBA channels). Both bounds share the type of the result.

Note: `Random` is an exec function because it requires sequential execution to produce a new random value each time.

## Sleep / Delay (Pure)

Buffer gates that delay a value passing through. Most useful with `await` and the `_` armed flag placeholder.

| Function | Signature | Description |
|----------|-----------|-------------|
| `Sleep(input, delay?, hold?)` | `(input: any, delay?: float, hold?: float) -> any` | Delay by seconds (BufferSeconds gate) |
| `SleepTicks(input, delay?, hold?)` | `(input: any, delay?: int, hold?: int) -> any` | Delay by ticks (BufferTicks gate) |

- `input` -- the value to delay. Use `_` inside `await` to wire the armed flag.
- `delay` -- seconds/ticks to wait before the output follows the input.
- `hold` -- seconds/ticks to hold the output after the input drops to zero. Set to -1 to use delay instead.

```wirescript
// Sleep 2 seconds using await
on start {
  await Sleep(_, delay = 2.0)
  doAfterDelay()
}

// Sleep 60 ticks (~1 second at 60Hz)
on start {
  await SleepTicks(_, delay = 60)
  doAfterDelay()
}

// Pure usage: delay a signal by 5 ticks
let delayed = SleepTicks(rawSignal, delay = 5)
```

## Exec Override

Exec functions that are called outside of an exec context can be given an explicit `exec` named argument to provide the execution trigger:

```wirescript
// Outside a handler -- provide exec explicitly
let r = Random(0, 10, exec = someTrigger)
```

This wires `someTrigger` as the exec input of the gate, bypassing the requirement for an enclosing handler context.

## Newer builtins

Player-reference gates (`DisplayText`, `ShowChatMessage`, `HasRole`, leaderboard and
team setters, the join/left/chat events, `ControllerOf`/`CharacterOf`) target the
persistent player-state on the current build. The `controller` type is unchanged and
still wires straight into them — existing scripts keep working.

### Entity (Exec)
```
entity.GetSpeed() -> float                    // scalar speed
entity.GetVelocityAtPoint(point: vector) -> vector
entity.GetEntityTeam() -> entity              // team of any entity (grid/prefab)
entity.SetEntityTeam(team: entity)
entity.IsFrozen() -> bool                     // whether the entity / brick grid is frozen
entity.DestroySpawned()                       // despawn a spawned entity
entity.DestroySpawnedPrefab()                 // despawn a spawned prefab
```

### Character ammo (Exec)
```
character.GetAmmo(resource: entity) -> int
character.GrantAmmo(resource: entity, amount: int)
character.SetAmmo(resource: entity, amount: int)
character.GetInventoryEntry(slot: int) -> { Item, BrickAsset, EntityType }
character.GetCurrentInventorySlot() -> int
character.GetWeaponChamberAmmo(resource: entity, slot: int) -> int
character.IncWeaponChamberAmmo(resource: entity, slot: int, amount: int)
character.SetWeaponChamberAmmo(resource: entity, slot: int, amount: int)
```

The `CharacterFiredWeapon(character, direction, start)` event fires when a player
fires a weapon (`direction`/`start` are vectors). `Sweep`/`SweepSimple` results also
carry a `HitColor` field (the color of the surface hit).

### Date / time (Pure)
```
GetUnixTime() -> int
FormatDate(unixTime: int, format: string, useUTC?: bool) -> { Output: string, Success: bool }
```

### Value conversions (Pure)
```
Remap(value, inMin, inMax, outMin, outMax) -> float   // rescale a value between ranges
LogicalShiftRight(a: int, b: int) -> int              // logical (unsigned) >>
EnumToInteger(value) -> int
IntegerToEnum(value: int, wrap?: bool)
ItemToPickup(item: entity) -> entity                  // pickup asset for an item
color.ConvertColor(fromSpace?: int, toSpace?: int) -> color
"A".ToCharCode() -> { Codepoint: int, Success: bool }
FromCharCode(codepoint: int) -> { Character: string, Success: bool }
```

`ParseInt` / `ParseNumber` likewise now expose a `Success` flag: they auto-unwrap to
their parsed `Value` in arithmetic/comparisons (`ParseInt(s) == 5`), and `.Success` is
`false` when the string wasn't a valid number.

### Self transform + simple raycast (Exec)
```
GetOwnTransform() -> { Location: vector, Rotation: rotator }
SweepSimple(distance: float, radius?: float, spreadConeAngle?: float)
  -> { HitDistance, HitEntity, HitLocation, HitNormal, Hit, Miss }
```

### Zone array fills (Exec) — array methods
```
arr.fillFromZoneEntities(zone, tagFilter?)   // entities inside a zone
arr.fillFromZonePlayers(zone, tagFilter?)    // players inside a zone
arr.sortMultiple(other, ..., descending?)    // sort this + up to 7 parallel arrays together
```

The character/entity zone enter/leave events also accept a `tagFilter =` argument
(alongside `zone =`) to restrict them to tagged entities.

## Generic type syntax

Types may be written in generic form:

```wirescript
var nums: Array<int> = [1, 2, 3]   // same as int[]
mod inc(v: Ref<int>) { v = v + 1 }   // same as *int
```

`Array<V>` and `Ref<V>` are exact aliases of `V[]` and `*V`.

## Dicts (`var m: Dict<K, V>`)

A dict is a keyed variable collection (the `MapVar` gate family), declared as a `var`
of the generic `Dict<K, V>` type. Keys must be `int`, `string`, or an object reference
(entity/character/controller) — any other key type is a **`WS039`** error; values may
be any wire-storable scalar (int/float/bool/string/vector/rotator/quat/color/object).
A dict starts empty unless given a constant literal initializer (`= {}` is the explicit
empty form).

```wirescript
var scores: Dict<string, int>
var names: string[]

on tick {
  scores.set("alice", 10)                 // insert / overwrite
  let g = scores.get("alice")             // { Value, Found } — auto-unwraps to Value
  if g.Found { PrintToConsole("${g.Value}") }
  if scores.has("bob") { ... }
  scores.remove("bob")                    // -> bool (was present)
  let n = scores.length()
  scores.keys(names)                      // fill an array with the keys
  scores.clear()
}
```

Methods (exec context, like array methods): `set(key, value)`, `get(key)`,
`has(key)`, `remove(key)`, `clear()`, `copyFrom(otherDict)`, `length()`,
`keys(destArray)`, `values(destArray)`.

### Dict literals

A `var` of `Dict<K, V>` type can be given literal contents with `{ ... }`.
Entries use `=>` for any key expression, or `:` for a string / atom / int
*literal* key (or a bracketed `[expr]` computed key):

```wirescript
var m: Dict<int, int>    = { :red => 10, 7 => 0 }    // arrow -- any key
var s: Dict<string, int> = { "red": 1, "blue": 2 }    // colon -- string literal key
var a: Dict<int, int>    = { :red: 1, :blue: 2 }      // colon -- atom literal key
var e: Dict<int, int>    = {}                         // explicit empty dict
on tick { m = { [runtimeKey] => x } }                // computed key -- desugars
```

A **constant** dict literal (every key and value a compile-time constant) in a
`var` initializer bakes straight into the dict at rest -- no runtime
gates, the dict loads pre-populated. An initializer with any non-constant
entry doesn't bake -- its entries are dropped (with a compiler warning) and
the dict loads empty; build it at runtime instead. Inside an exec handler,
`m = { ... }` (or a literal with runtime keys/values) desugars to `clear()`
followed by one `set(key, value)` per entry, in source order -- the same
clear-then-populate shape as [array literal assignment](statements.md).

`{ foo: 1 }` with a **bare identifier** key is a record literal, not a dict --
`:` only introduces a dict key for a string/atom/int literal or a `[expr]`
computed key. Use `foo => 1` or `[foo]: 1` to key a dict by an identifier's
value.

Assigning a whole dict from another dict variable (`m = m2`) is not supported
-- there is no whole-dict-copy gate. Use `m.copyFrom(m2)` instead.
