# Built-in Functions

Wirescript provides built-in functions that map directly to Brickadia's logic gates. Each function is either **pure** (returns a value, no exec context needed) or **exec** (requires exec context and chains into the current execution flow).

<!-- toc -->
## Contents

- [Notation](#notation)
- [Math / Trigonometry (Pure)](#math--trigonometry-pure)
- [Bitwise (Pure)](#bitwise-pure)
- [Vector (Pure)](#vector-pure)
- [Rotation / Quaternion (Pure)](#rotation--quaternion-pure)
- [Color (Pure)](#color-pure)
- [Stateful Exec Values](#stateful-exec-values)
- [Select / Swap (Pure)](#select--swap-pure)
- [Edge / Change Detectors](#edge--change-detectors)
- [Logical XOR (`^^`)](#logical-xor-)
- [String Operations (Pure)](#string-operations-pure)
- [String Formatting (Pure)](#string-formatting-pure)
- [Array Methods (Exec)](#array-methods-exec)
- [Player Input (Exec)](#player-input-exec)
- [Controller / Character Conversions (Exec)](#controller--character-conversions-exec)
- [Camera / Aim (Exec)](#camera--aim-exec)
- [Display (Exec)](#display-exec)
- [Entity Getters (Exec)](#entity-getters-exec)
- [Entity Manipulation (Exec)](#entity-manipulation-exec)
- [Gamemode (Exec)](#gamemode-exec)
- [Character (Exec)](#character-exec)
- [Controller (Exec)](#controller-exec)
- [Broadcast Messaging (Exec)](#broadcast-messaging-exec)
- [Audio (Exec)](#audio-exec)
- [Entity Tags (Exec)](#entity-tags-exec)
- [Entity Custom Properties (Exec)](#entity-custom-properties-exec)
- [Bricks (Exec)](#bricks-exec)
- [Environment and Components (Exec)](#environment-and-components-exec)
- [Misc (Pure / Exec)](#misc-pure--exec)
- [Gate config properties](#gate-config-properties)
- [Clock (Event)](#clock-event)
- [ChatCommand (Event)](#chatcommand-event)
- [Custom Events](#custom-events)
- [Prefab Spawning (Exec)](#prefab-spawning-exec)
- [Raycasting (Exec)](#raycasting-exec)
- [Random (Exec)](#random-exec)
- [Sleep / Delay (Pure)](#sleep--delay-pure)
- [Exec Override](#exec-override)
- [Newer builtins](#newer-builtins)
- [Generic type syntax](#generic-type-syntax)
- [Maps (`var m: Map<K, V>`)](#maps-var-m-mapk-v)
- [Exec-flow gates (Union / Branch)](#exec-flow-gates-union--branch)
- [Callable gate builtins](#callable-gate-builtins)
<!-- /toc -->

## Notation

- **Pure** functions are expressions: they produce a value and can be used anywhere.
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
| `abs(x)` | `(x: T) -> T` | Absolute value; `T` is any math variant, per component |
| `sqrt(x)` | `(x: float) -> float` | Square root |
| `pow(x, exponent)` | `(x: float, exponent: float) -> float` | Power |
| `clamp(x, min, max)` | `(x: T, min: T, max: T) -> T` | Clamp to range |
| `round(x)` | `(x: float) -> float` | Round to nearest integer |
| `floor(x)` | `(x: float) -> float` | Round down |
| `ceil(x)` | `(x: float) -> float` | Round up |
| `min(a, b)` | `(a: T, b: T) -> T` | Minimum of two values |
| `max(a, b)` | `(a: T, b: T) -> T` | Maximum of two values |
| `log(x, base)` | `(x: float, base: float) -> float` | Logarithm with arbitrary base |
| `lerp(a, b, t)` | `(a: T, b: T, t: float) -> T` | Linear interpolation; `T` is any math variant (see Easing/Tween) |
| `Blend(a, b, alpha)` | `(a: T, b: T, alpha: float) -> T` | The same gate as `lerp`, with a `clampAlpha` config (receiver on `a`) |
| `fmod(a, b)` | `(a: float, b: float) -> float` | Floored modulo |
| `Deg2Rad(x)` | `(x: float) -> float` | Degrees to radians |
| `Rad2Deg(x)` | `(x: float) -> float` | Radians to degrees |

```wirescript
let angle = atan2(dy, dx)
let clamped = clamp(value, 0.0, 1.0)
let dist = sqrt(dx * dx + dy * dy)
let radians = Deg2Rad(90.0)
```

`abs`, `min`, `max` and `clamp` give the type of their arguments: an `int` for
`int`s, a `float` once one is a `float`, and a `vector`, `rotator`, `quat` or
`color` per component. A `bool` reads as an `int`. A constant call folds to a
constant of that type, so `max(3, 7)` is the `int` `7`.

## Bitwise (Pure)

| Function | Signature | Description |
|----------|-----------|-------------|
| `BitCount(x)` | `(x: int) -> int` | Count set bits (popcount) |
| `BitNand(a, b)` | `(a: int, b: int) -> int` | Bitwise NAND (same as `~(a & b)`) |
| `BitNor(a, b)` | `(a: int, b: int) -> int` | Bitwise NOR (same as `~(a \| b)`) |

The compiler fuses `~(a & b)` and `~(a | b)` into single NAND/NOR gates.

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
| `v.SplitVec()` | `(v: vector) -> {x, y, z: float}` | Decompose vector (receiver on `vector`) |

### Vector Receiver Methods

`Dot`, `Cross`, `Normalize`, `Magnitude`, `MagnitudeSq`, `Distance`, `DistanceSq`, and `ScaleVec` support receiver syntax on `vector`:

```wirescript
// Receiver form
let dsq = a.DistanceSq(b)
let msq = v.MagnitudeSq()
let n = v.Normalize()

// Traditional form
let dsq = DistanceSq(a, b)
let msq = MagnitudeSq(v)
let n = Normalize(v)
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
| `a.AngleTo(b)` | `(quat, quat) -> float` | Smallest angle between two quaternions (degrees) |
| `a.Slerp(b, alpha)` | `(quat, quat, float) -> quat` | Spherical interpolation |
| `axis.RotationByAngle(angle)` | `(vector, float) -> quat` | Quaternion from axis + angle (degrees) |
| `q.ToAxisAngle()` | `(quat) -> {Axis: vector, Angle: float}` | Decompose into axis + angle (degrees) |
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
| `a.ColorBlend(b, alpha)` | `(color, color, float) -> color` | Blend two colors (color-space aware) |

`SplitColor`, `ToSRGB`, `ToHex`, and `ColorBlend` support receiver syntax on `color`.

`Blend` is a different gate: the math blend (an alias for `lerp`), which takes colors
as one of its variants but has no color-space selection.

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
| `Select(cond, a, b)` | `(cond: bool, a: T, b: T) -> T` | Returns `a` if false, `b` if true |
| `Swap(cond, a, b)` | `(cond: bool, a: T, b: T) -> {Output, OutputB: T}` | Conditionally swap two values |

```wirescript
let bigger = Select(x > y, y, x)
let result = Swap(shouldSwap, left, right)
// result auto-unwraps to Output; result.Output and result.OutputB
// are swapped if shouldSwap is true
```

## Edge / Change Detectors

| Function | Signature | Description |
|----------|-----------|-------------|
| `Edge(input)` | `(input: bool) -> {Rising, Falling: bool}` | Bool pulses on boolean transitions |
| `EdgeExec(input)` | `(input: float) -> {Rising, Falling: exec}` | Exec pulses when a value rises/falls |
| `Changed(input)` | `(input: any) -> bool` | Bool pulse when the input changes |
| `Change(input)` | `(input: any) -> exec` | Exec pulse when the input changes |
| `ContainerChange(container)` | `(container: T[] \| Map<K, V>) -> exec` | Exec pulse on the update after an array or map changes |

`Edge` and `Changed` are pure: they produce a one-tick bool pulse (`Rising` on
false→true, `Falling` on true→false; `Changed` on any change). `EdgeExec` and
`Change` are their exec-flavored siblings. `EdgeExec`'s outputs fire exec
chains directly (use with `on`/`await`, like `Timer(...).Expired`), and
`Change` fires an exec pulse whenever the input changes.

```wirescript
let edges = Edge(button)
on edges.Rising { count = count + 1 }

let health = EdgeExec(hp)
on health.Falling { ctrl.ShowStatusMessage("taking damage!") }
```

`Change` and the other detectors watch a wire value, and an array or map's
wire carries the same reference however its contents move (`WS059`).
`ContainerChange` watches the contents instead: it fires on the update after
the array or map changes in any way.

```wirescript
var scores: Map<string, int>
var shown: int = 0
on ContainerChange(scores) { shown = scores.length() }
```

## Logical XOR (`^^`)

The `^^` operator is boolean XOR, which returns true if exactly one operand is true.

```wirescript
let either = a ^^ b  // true if a or b but not both
```

The compiler fuses `!(a && b)` and `!(a || b)` into single NAND/NOR gates.

## String Operations (Pure)

All string functions support **receiver syntax** on `string`:

| Function | Signature | Description |
|----------|-----------|-------------|
| `s.Length()` | `(s: string) -> int` | String length |
| `s.Contains(search, caseSensitive?)` | `(s: string, search: string, caseSensitive?: bool) -> bool` | Check if string contains substring |
| `s.StartsWith(prefix, caseSensitive?)` | `(s: string, prefix: string, caseSensitive?: bool) -> bool` | Check prefix |
| `s.EndsWith(suffix, caseSensitive?)` | `(s: string, suffix: string, caseSensitive?: bool) -> bool` | Check suffix |
| `s.Find(search, caseSensitive?, start?)` | `(s: string, search: string, caseSensitive?: bool, start?: int) -> int` | Find substring index (-1 if not found) |
| `s.Substring(start, length)` | `(s: string, start: int, length: int) -> string` | Extract substring |
| `s.Replace(search, replacement, caseSensitive?, maxReplacements?, start?)` | `(s: string, search: string, replacement: string, caseSensitive?: bool, maxReplacements?: int, start?: int) -> string` | Replace occurrences |
| `s.Split(delimiter, occurrence?, caseSensitive?)` | `(s: string, delimiter: string, occurrence?: int, caseSensitive?: bool) -> {Left, Right: string, Found: bool}` | Split at the delimiter |
| `s.ToLower()` | `(s: string) -> string` | Convert to lowercase |
| `s.ToUpper()` | `(s: string) -> string` | Convert to uppercase |
| `s.Trim()` | `(s: string) -> string` | Remove leading/trailing whitespace |
| `s.ParseInt()` / `ParseInt(s)` | `(s: string) -> {Value: int, Success: bool}` | Parse an integer from text (auto-unwraps to `Value`) |
| `s.ParseNumber()` / `ParseNumber(s)` | `(s: string) -> {Value: float, Success: bool}` | Parse a number from text (auto-unwraps to `Value`) |

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
| `Fmt(format, a?, b?, c?, d?, e?, f?, g?)` | `(format: string, a-g?: any) -> string` | Format text with placeholders |

The `Fmt` function wraps the FormatText gate. The format string names inputs
`a` through `g` as `{A}` through `{G}`, the form the gate's own tooltip uses.
The numbered form `{0}` through `{6}` also works, and it is what `${}`
interpolation compiles to.

```wirescript
let label = Fmt("{A}: {B}", "Score", score)
let coords = Fmt("({0}, {1}, {2})", x, y, z)

// The format string can be built at runtime, which turns Fmt into a lookup table:
let col = Fmt('{' .. bucket .. '}', 'eee4da', 'f2b179', 'f65e3b')
```

An option after a colon changes how a value prints:

| Placeholder | Prints | Example |
|---|---|---|
| `{A:,}` | Thousands grouped | `1,234,567` |
| `{A:.2}` | Fixed decimals, up to 6 | `12.50` |
| `{A:,.2}` | Both | `1,234.50` |
| `{A:On=Off}` | The first word when true, the second when false | `On` |

A form after a bar picks a word by number. Each form names a plural category
(`one`, `two`, `few`, `other`), and a form's text can insert inputs itself:

```wirescript
let apples = Fmt("{A} {A}|plural(one=apple,other=apples)", n)            // 1 apple, 3 apples
let place = Fmt("{A}{A}|ordinal(one=st,two=nd,few=rd,other=th)", rank)  // 1st, 2nd, 3rd, 4th
let basket = Fmt("{A}|plural(one=one apple,other={A} apples)", n)       // one apple, 3 apples
```

The game applies these options. A `Fmt` that uses lettered inputs, options or
word forms is never folded at compile time, even when its inputs are
constants. Whether the options also apply to the numbered form (`{0:.2}`) is
unverified, so prefer the lettered form with them.

## Array Methods (Exec)

Methods on an `array` variable. All run in exec context (they lower to ArrayVar
exec gates), so call them inside `on` handlers / mods. Declare arrays with
`var name: T[]` (see [statements](statements.md)).

The element type can be a **record** (`var pts: Point[]`): the array is stored as
one parallel array per field and each method fans out. `sort`/`shuffle`, the
aggregates, and the dual-array ops have no per-field meaning there and are
rejected with `WS050`. See [Records as storage](types.md#records-as-storage).

| Method | Signature | Description |
|--------|-----------|-------------|
| `arr.push(value)` | `(value: T)` | Append an element |
| `arr.pop()` | `() -> {Value: T, IsEmpty: bool}` | Remove and return the last element (auto-unwraps to `Value`) |
| `arr.get(index)` | `(index: int) -> {Value: T, OutOfBounds: bool}` | Read the element at `index` (auto-unwraps to `Value`). `.OutOfBounds` flags a bad index. This is the explicit form of `arr[i]` |
| `arr.length()` | `() -> int` | Number of elements |
| `arr.remove(index)` | `(index: int) -> bool` | Remove the element at `index`; gives `true` if `index` was out of range |
| `arr.insert(index, value)` | `(index: int, value: T) -> bool` | Insert before `index`; gives `true` if `index` was out of range |
| `arr.clear()` | `()` | Remove all elements |
| `arr.find(value)` | `(value: T) -> {Index: int, Found: bool}` | Find the first match (auto-unwraps to `Index`). `Index` is `-1` when `Found` is `false` |
| `arr.sort(descending?)` | `(descending?: bool)` | Sort in place |
| `arr.sortMultiple(other, ..., descending?)` | `(other: T[], ..., descending?: bool) -> bool` | Sort in place, reordering at most 7 parallel arrays through the same permutation (more is `WS011`). A record array argument is carried along as one array per field, and each field counts toward the 7. Only a trailing `bool` is `descending` |
| `arr.reverse()` | `()` | Reverse in place |
| `arr.shuffle()` | `()` | Randomly reorder |
| `arr.swap(a, b)` | `(a: int, b: int) -> bool` | Swap two elements; gives `true` if an index was out of range |
| `arr.fill(value)` | `(value: T)` | Set every element to `value` |
| `arr.resize(size, value)` | `(size: int, value: T)` | Grow/shrink, filling new slots with `value` |
| `arr.sum()` | `() -> T` | Sum of elements |
| `arr.min()` / `arr.max()` | `() -> {Value: T, IsEmpty: bool}` | Smallest / largest element (auto-unwraps to `Value`) |
| `arr.average()` | `() -> {Value: float, IsEmpty: bool}` | Mean of elements (auto-unwraps to `Value`) |
| `arr.join(separator?)` | `(separator?: string) -> string` | Join every element into one string, each converted as it would be to a string, with `separator` between them (none when omitted) |
| `arr.append(source)` | `(source: T[])` | Append all elements of another array |
| `arr.copyFrom(source)` | `(source: T[])` | Replace contents with a copy of another array |
| `arr.slice(source, start, count)` | `(source: T[], start: int, count: int) -> bool` | Copy `source[start..start+count]` into this array; gives `true` if the range was out of bounds |
| `arr.fillFromPlayers()` | `()` | Fill with all current players |
| `arr.fillFromTeam(team)` | `(team: entity)` | Fill with the members of a team |

Element access uses bracket syntax: `arr[i]` reads (with `.value` / `.bOutOfBounds`),
`arr[i] = x` writes.

**`exec =`.** Any exec-gate call (an array method, a builtin, or a mod/chip call)
accepts an `exec = <trigger>` argument that drives its exec input, firing the gate each
time the trigger's value changes. A per-index, always-nonzero trigger like `index + 1`
turns an array into a single-gate lookup table read straight from a pure binding:

```wirescript
var lut: color[] = [ /* ...constant entries... */ ]
out c: color = lut.get(i, exec = i + 1).Value
```

```wirescript
var scores: int[]
on RoundEnd() {
  scores.push(currentScore)
  scores.sort(true)          // descending
  let best = scores.max()
  let count = scores.length()
}
```

`sortMultiple` sorts the receiver and drags parallel arrays along, which is how
you sort records by one field and keep the others attached:

```wirescript
var scores: Map<string, int>
var names: string[]
var points: int[]

in show: exec
on show {
  scores.keys(names)     // string[]
  scores.values(points)  // int[]
  points.sortMultiple(names)
  // points is sorted ascending and names[k] still owns points[k]
}
```

Sorting a copy and searching back with `find` is the alternative, and it ties
duplicate values to whichever entry matched first.

## Player Input (Exec)

### InputReader
```
character.InputReader() -> { Forward, Right, Up, Pitch, Yaw, Roll, MouseWheel, PressedC, PressedE, PressedQ, PressedLeftMouse, PressedRightMouse }
InputReader(character: character) -> { ...same fields... }
```

Read player input axes and pressed keys. Receiver on `character`.

`InputReader` is not a gate: it compiles to the **Seat Control Splitter**
component (`Component_Internal_InputSplitter`), with the character wired into
its **Character** input. Its outputs are live: they change while the player
presses keys, with no Clock and no exec step, so `on input.PressedQ { ... }`
runs once per press (on the rising edge) and `on input.Forward > 0.5` once
per push. Any character wire works, not only a Seat's **Sitting Character**:
a Switch's **Character** holds the last player who flipped it and keeps
holding them wherever they go, which hands a remote to a player who is not
seated. One splitter reads one player; to read every player at once, spawn a
small chip per player that wires its own splitter
([One Chip per Player](learn/prefabs/one-chip-per-player.md)).

Returns a record with fields:
- `Forward: float`: forward/backward movement axis (-1 to 1)
- `Right: float`: left/right movement axis (-1 to 1)
- `Up: float`: up/down movement axis (-1 to 1)
- `Pitch: float` / `Yaw: float` / `Roll: float`: look axes
- `MouseWheel: float`: mouse wheel delta
- `PressedC` / `PressedE` / `PressedQ` / `PressedLeftMouse` / `PressedRightMouse: bool`: key/button states

```wirescript
let input = char.InputReader()
let moving = input.Forward != 0.0 || input.Right != 0.0
let interacting = input.PressedE
```

### GetInputs
```
character.GetInputs() -> { ...same fields as InputReader... }
GetInputs(player: character) -> { ...same fields... }
```

Sample the same twelve controls once, at the point the exec chain reaches this
call, rather than reading them continuously. The field names are identical to
`InputReader`'s, so only the context differs. Its operand also accepts a
persistent player, which wires straight into the `character` parameter.
It reads any player, wherever they are: they need not be seated or standing
in a zone, so a list of players from a zone fill, a stored reference or an
event's character all work.

It is an exec gate, so it must sit on an exec chain. In pure position it
reports `WS007`. Use it for a one-time read in a handler that has a player
but no character wire, such as a zone event. To watch keys, wire the
character into [`InputReader`](#inputreader) instead: a `GetInputs` on a
Clock misses any press shorter than the interval.

```wirescript
on ZoneEntered(zone = hall) -> (character) {
  if character.GetInputs().PressedQ { character.ShowStatusMessage("q") }
}
```

## Controller / Character Conversions (Exec)

These functions convert between entity types. They require exec context and support receiver syntax.

### ControllerOf
```
entity.ControllerOf() -> controller
ControllerOf(entity: entity) -> controller
```

Get controller from entity. Receiver on `entity`. Uses the admin-only **Get
Player (Persistent)** gate.

A controller (player state) is the persistent player, not the body: it stays
the same through deaths and respawns, while each respawn makes a new
character. A character and its controller are not equal. A character wired
straight into a `controller` slot (a `Map<controller, V>` key, a
`controller` var) still carries the character, so as a map key it files
under that body and a respawn starts a new key. Custom properties
(`SetCustomProperty`) can be set on a controller, which makes it per-player
storage that survives respawns but not a reconnect.

### CharacterOf
```
controller.CharacterOf() -> character
CharacterOf(controller: controller) -> character
```

Get character from controller. Receiver on `controller`.

```wirescript
on CharacterSpawned() -> (character) {
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
- `Origin: vector`: aim origin position
- `Direction: vector`: aim direction vector

Receiver on `character`. Access the fields with `.Origin` / `.Direction`. Both
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
DisplayText(target: controller, text: string, ...) -> int
```

Display HUD text to a player. Receiver on `controller`. Returns the resolved
`textId` (an `int`) so a later call can update or clear the same on-screen text.

The 2D layout ports are `Vector2D` composites, and the call feeds them one axis
at a time: `positionX` / `positionY`, `anchorX` / `anchorY`, and so on, each a
`float`. There is no `vector`-typed `position` or `anchor`, and passing one is a
`WS041` error. A constant axis bakes into the parent `Vector2D` data field. A
runtime value wires the matching sub-port. The call also exposes the scalar
styling below, plus `typeface` / `font`, which are constant-only data fields
(not wire inputs). `justify` and `easing` are wire inputs: a constant member
name bakes into the gate, and any other value wires in.

#### DisplayText Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `target` | `controller` | Yes | Player to display to |
| `text` | `string` | Yes | Text content (auto-converted to string) |
| `positionX` / `positionY` | `float` | No | 2D screen position, per axis |
| `anchorX` / `anchorY` | `float` | No | 2D anchor point, per axis |
| `scaleX` / `scaleY` | `float` | No | 2D scale, per axis |
| `pivotX` / `pivotY` | `float` | No | 2D pivot point, per axis |
| `shadowOffsetX` / `shadowOffsetY` | `float` | No | 2D drop-shadow offset, per axis |
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
| `fontSize` | `int` | No | Font size |
| `justify` | `DisplayTextJustification` | No | Justification: `DisplayTextJustification.Left` / `.Center` / `.Right`, or bare `Left` / `Center` / `Right`. Defaults to `Center` (the game's default since its EA 3.2 update; it was `Left` before) |
| `easing` | `DisplayTextEasing` | No | Transition curve: `DisplayTextEasing.Linear` / `.EaseIn` / `.EaseOut` / `.EaseInOut`, or bare `Linear` / `EaseIn` / `EaseOut` / `EaseInOut` |
| `typeface` | `TextTypeface` | No | Typeface: `TextTypeface.Regular` / `.Bold` / `.Italic` / `.BoldItalic`, or bare `Regular` / `Bold` / `Italic` / `BoldItalic` (constant only) |
| `font` | asset ref | No | Font asset reference (constant only) |

```wirescript
on RoundStart() {
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

It moves a spawned prefab (the entity `SpawnPrefab` returns, or
`ReadBrickGrid()` inside the copy). It does not move a pasted chip's own
grid: `ReadBrickGrid().SetLocation(...)` in a chip that was pasted, not
spawned, leaves it where it is.

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
entity.Teleport(dest: teleport)
Teleport(entity: entity, dest: teleport)
```

Teleport entity to destination.

### RelativeTeleport
```
entity.RelativeTeleport(source: teleport, dest: teleport)
RelativeTeleport(entity: entity, source: teleport, dest: teleport)
```

Relative teleport between two points.

### SetVelocity
```
entity.SetVelocity(linear?: vector, angular?: vector)
SetVelocity(entity: entity, linear?: vector, angular?: vector)
```

Set velocity. Both `linear` and `angular` are optional, so pass whichever components you want to set.

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

### SetVisible
```
entity.SetVisible(visible: bool)
SetVisible(entity: entity, visible: bool)
```

Show or hide a physics object or dynamic brick grid. A hidden one keeps its
collision and renders only for players holding a tool that shows hidden bricks.

### SetVisibleTo
```
entity.SetVisibleTo(visibleTo?: controller | character | entity)
SetVisibleTo(entity: entity, visibleTo?: controller | character | entity)
```

Show a physics object or dynamic brick grid to one player or one team only.
`visibleTo` takes a player (a `controller` or `character`) or a team (the
`entity` that `GetTeam` gives). Everyone else sees it as hidden, but its
collision stays on for everyone, and builder mode always shows it. Leave
`visibleTo` out to show it to everyone again. `SetVisible(false)` still hides
it from everyone, the chosen player or team included.

```wirescript
@left in reveal: exec
@left in hideAgain: exec
@left in crate: entity
@left in seer: controller

on reveal { crate.SetVisibleTo(seer) }   // only `seer` sees the crate
on hideAgain { crate.SetVisibleTo() }    // everyone sees it again
```

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
controller.SetLeaderboard(key: string, value: int)
SetLeaderboard(controller: controller, key: string, value: int)
```

Set a leaderboard value. Receiver on `controller`.

### IncLeaderboard
```
controller.IncLeaderboard(key: string, value: int)
IncLeaderboard(controller: controller, key: string, value: int)
```

Increment a leaderboard value. Receiver on `controller`.

### GetLeaderboard
```
controller.GetLeaderboard(key: string) -> int
GetLeaderboard(controller: controller, key: string) -> int
```

Get a leaderboard value. Receiver on `controller`.

### GetTeam
```
character.GetTeam() -> entity
GetTeam(character: character) -> entity
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
gate was removed, and a round now ends via a win.) `PlayerWins` declares a
player the winner, or their team if `teamWinsInstead` is true. `TeamWins`
declares a team the winner.

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
on CharacterDied() -> (character) {
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

Display a hint popup to a character. Receiver on `character`. `title` and
`text` are gate settings, so they must be constants: a variable or computed
string is `WS028`.

```wirescript
on CharacterSpawned() -> (character) {
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
character.AddInventoryItemAdv(item, damage?, speed?, scale?, itemName?, projectile?, projectileScale?)
character.SetInventoryItemAdv(item, slot?, damage?, speed?, scale?, itemName?, projectile?, projectileScale?)
```

Give items, procedural bricks, or spawnable entities to a character's
inventory. Asset args are `$Type/Name` references (`$BRItemBase/...` for
items, a brick asset for bricks, an entity type for entities), inlined into
the gate's data. The `Adv` variants add per-item overrides: damage/weapon
speed/scale multipliers, a display-name override, a projectile override, and
a projectile size multiplier (`projectileScale`, default `1.0`).
All receive on `character`.

```wirescript
on CharacterSpawned() -> (character) {
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
on RoundStart() {
  ctrl.ShowStatusMessage("Round started!")
}
```

### ShowChatMessage
```
controller.ShowChatMessage(message: any)
ShowChatMessage(controller: controller, message: any)
```

Send a chat message that only this player sees (a whisper). Receiver on
`controller`.

### Dialogs
```
controller.ShowMessageBox(message: any, title?: any) -> { ClickedBy, OkClicked, Declined }
controller.ShowOkCancelBox(message: any, title?: any) -> { ClickedBy, OkClicked, Declined }
controller.ShowYesNoBox(message: any, title?: any) -> { ClickedBy, YesClicked, Declined }
controller.ShowTextInputBox(message: any, title?: any) -> { ClickedBy, Text, OkClicked, Declined }
```

Pop up a dialog for this player. Receiver on `controller`. The call itself
continues at once, and the answer arrives later. When the player confirms, the
`OkClicked` (or `YesClicked`) exec field pulses, `ClickedBy` is the player who
answered (a `controller`), and `ShowTextInputBox`'s `Text` is the line they
typed. Every other outcome pulses `Declined`: Cancel or No, closing the
dialog, or a dialog the game could not show.

```wirescript
var ready: int = 0
on ControllerJoined() -> (c) {
  let r = c.ShowYesNoBox("Ready to play?", title = "Start")
  on r.YesClicked { ready = ready + 1 }
  on r.Declined { c.ShowChatMessage("Type /ready when you want to join") }
}
```

### Player info
```
controller.GetUserName() -> string
controller.GetUserId() -> string
controller.GetDisplayName() -> string
controller.IsTrusted() -> bool
controller.HasPermission(permission: string) -> bool
controller.HasRole(role: string) -> bool
controller.SetCanRespawn(canRespawn: bool)
controller.ForceRespawn()
controller.SetTeamPinned(pinned: bool)
```

These read a player's account name, persistent user id, or current display
name. They check whether the player is trusted by the brick owner, holds a
named permission, or has a named role (`HasRole`). They toggle the player's
ability to respawn, respawn them immediately (`ForceRespawn`, also callable as
`ForceRespawn(player)`), or pin them to their team. All receive on `controller`.

## Broadcast Messaging (Exec)

```
BroadcastChatMessage(message: any)
BroadcastStatusMessage(message: any, flash?: bool)
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
player.PlayClientAudio(audio, volume?, pitch?)
```

Play a one-shot sound at an entity's location (spatialized by default),
globally for all players, or non-spatially for a single player
(`PlayClientAudio`). `PlayClientAudio` receives on the player, which is a
character or persistent player reference, and is also callable as
`PlayClientAudio(player, audio, ...)`. The `audio`
arg is a `$BrickOneShotAudioDescriptor/...` asset reference. `PlayAudioAt`
receives on `entity` (characters work too).

```wirescript
on ZoneEntered() -> (character) {
  character.PlayAudioAt($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press)
}
```

## Entity Tags (Exec)

```
entity.SetTag(tag: string)
entity.GetTag() -> string
```

Attach an arbitrary string tag to any entity and read it back later. This is handy
for marking players/entities with game state (team, slot index, role). Zone
components can also filter on tags. Receiver on `entity`.

## Entity Custom Properties (Exec)

```
entity.SetCustomProperty(key: string, value)
entity.GetCustomProperty(key: string) -> { Value, Found: bool }
entity.RemoveCustomProperty(key: string) -> bool
entity.CopyCustomProperties(source: entity)
```

A key/value store on an entity, a player or a dynamic grid. On a controller
the values stay with the player through respawns and are lost when the player
leaves and reconnects. On a character they belong to that one body and are
lost when it respawns. `SetCustomProperty`
replaces any value already under the key. `GetCustomProperty` gives the value,
auto-unwrapped like a map read, and `Found` is `false` when the key is not set.
`RemoveCustomProperty` gives whether anything was removed.
`CopyCustomProperties` replaces the receiver's properties with a copy of
`source`'s, for instance to give a spawned replacement the original's data. A
holder keeps up to 256 properties, and a key is at most 64 characters: a longer
key is silently not stored. A dynamic grid such as a spawned prefab holds
properties; a pasted grid does not. The value can be any wire value, refs
included, so `.Value` is an `any`. Give it a type by storing it in a typed
`var` or an annotated `let` (`let hp: int = player.GetCustomProperty("hp").Value`),
or convert it with `as`. `.Value.getType()` says which kind it holds (see
[GetValueType](#value-conversions-pure)).

```wirescript
on CharacterDied() -> (character) {
  let player = character.ControllerOf()
  let lives = player.GetCustomProperty("lives")
  let left: int = if lives.Found then lives.Value else 3
  player.SetCustomProperty("lives", left - 1)
}
```

The count is kept on the controller, so it carries over to the player's next
character. Kept on `character`, every respawn would start without it.

## Bricks (Exec)

### GetMicrochipBrick
```
GetMicrochipBrick() -> brick
```

The program's own microchip brick: the one brick a paste places on the main
grid. It is a reference baked into the save, so it needs no wire or editor
setup, and works anywhere in the program, a `fn` body included. It does not
yet name the right brick inside a spawned prefab.

```wirescript
in go: exec
on go {
  GetMicrochipBrick().SetBrickProperty(Color, Color(0.0, 1.0, 0.0))
}
```

### GetBrickProperty / SetBrickProperty
```
brick.GetBrickProperty(property: BrickProperty) -> bool | color | any
brick.SetBrickProperty(property: BrickProperty, value)
```

Read or write one setting of a brick: recolor it, hide it, change what
collides with it. `property` is a constant `BrickProperty` member (`Visible`,
`Color`, `Material`, `MaterialOpacity`, `AllCollision`, `PlayerCollision`,
`Player2Collision` to `Player4Collision`, `WeaponCollision`,
`InteractionCollision`, `PhysicsCollision`), written bare or qualified
(`BrickProperty.Color`). The value is a `bool` for `Visible` and the collision
flags and a `color` for `Color`, both for what `GetBrickProperty` returns and
for what `SetBrickProperty` accepts. Any other type there is `WS003`.
`Material` and `MaterialOpacity` read and write `any`. Receiver on `brick`.

```wirescript
in go: exec
on go {
  let r = SweepSimple(1000.0, detectBricks = true)
  on r.Hit { r.HitBrick.SetBrickProperty(Color, Color(1.0, 0.0, 0.0)) }
}
```

## Environment and Components (Exec)

### Environment settings
```
Environment.<environment>.<property>            // read
Environment.<environment>.<property> = value    // write
```

Reads and writes a setting of the world's environment: the time of day, the
sky's color, the water's height. Both run on the exec chain. Every
environment and property, with the type each reads and writes as, is listed in
[Environment Settings](environment.md).

```wirescript
in go: exec
on go {
  let t = Environment.Sky.TimeOfDay
  Environment.Sky.TimeOfDay = t + 1.0
  Environment.Sky.SkyColor = Color(1.0, 0.5, 0.0)
}
```

### Components
```
brick.GetComponent($Component/<type>) -> Component<type>
brick.AddComponent($Component/<type>) -> Component<type>
brick.RemoveComponent($Component/<type>)
component.<property>            // read
component.<property> = value    // write
```

`GetComponent` gives a reference to the brick's component of that type (an
empty reference if it has none). `AddComponent` adds one first if it has none.
The reference is typed by its component type, so its properties read and write
with their own types (`light.Brightness` is a `float`). The game converts a
written value to the property's type and leaves a wired property alone.
`RemoveComponent` removes the brick's component of that type after the current
update, unless an `AddComponent` of its type runs first. It takes the brick,
not a reference. No gate gives a reference's brick, so `component.Remove()` is
`WS110`. The component types are the ones a
player can add to a brick (see [Components](expressions.md#components)), and
the editor completes a reference's properties.

A reference can be passed to a `mod` taking `Component<PointLight>`, or
`Component` for a reference of any type. It can be
stored in a `var`, an array (`Component<PointLight>[]`) or a map value, but not
used as a map key (`WS039`). The property lists come from the
save schema, so they may name fields the game's own picker does not offer
(unverified).

```wirescript
in go: exec
on go {
  let r = SweepSimple(1000.0, detectBricks = true)
  on r.Hit {
    let light = r.HitBrick.AddComponent($Component/PointLight)
    light.Brightness = light.Brightness * 2.0
    light.Color = Color(1.0, 0.5, 0.0)
  }
}
```

## Misc (Pure / Exec)

| Function | Signature | Description |
|----------|-----------|-------------|
| `FindPlayer(query)` | `(query: any) -> controller` (exec) | Look up a player by name; emits the player |
| `PrintToConsole(text)` | `(text: string) -> ()` (exec) | Print a value to the game console (debugging) |
| `Opaque(value)` | `(value: any) -> any` (pure) | Identity rerouter that acts as a permanent constant-fold barrier. The wrapped value always stays a real runtime wire and is never folded or seen through. Used in probe/test circuits (see [Constant Folding](folding.md)) |
| `DeltaTime()` | `() -> float` | Seconds elapsed since the previous tick |
| `ServerUptime()` | `() -> float` | Seconds the server has been running |
| `ReadBrickGrid()` | `() -> entity` | The brick grid this gate's microchip is on, as an entity |
| `NearlyEqual(a, b, tolerance)` | `(a: float, b: float, tolerance: float) -> bool` | Approximate float equality |
| `Dampen(target, smoothTime)` | `(target: T, smoothTime: float) -> T` | Critically-damped smoothing toward a target; `T` is any math variant, as for `lerp` |
| `Easing(a, b, blend, function?, direction?)` | `(a: T, b: T, blend: float, function?: EasingFunction, direction?: EasingDirection) -> T` | Ease from `a` to `b` by `blend` |
| `Tween(target, duration, function?, direction?)` | `(target: T, duration: float, function?: EasingFunction, direction?: EasingDirection) -> {Value: T, Arrived: exec}` | Stateful eased value toward `target` (auto-unwraps to `Value`; `Arrived` pulses on arrival) |
| `Timer(limit, restart?, pause?, resume?)` | `(limit: float, restart?/pause?/resume?: exec) -> {Time: float, Expired: exec}` | Stateful countdown timer |

`Blend`/`lerp`/`Easing`/`Tween`/`Dampen` interpolate any one math variant `T`: float|int|vector|rotator|quat|color.
The result is whatever `T` the inputs carry.

`Easing`/`Tween` take an easing `function` and `direction`: an int, a bare enum-name
literal, or an `EasingFunction` / `EasingDirection` value (see
[Built-in game enums](enums.md#built-in-game-enums)). `function =
EasingFunction.Bounce` and `function = Bounce` set the same field. Functions:
`Linear`, `Sine`, `Quad`, `Cubic`, `Quart`, `Quint`, `Expo`, `Circ`, `Back`,
`Elastic`, `Bounce`. Directions: `In`, `Out`, `InOut`. Omitted, they default to
`Linear`/`In`.

`Timer` is a function-call instance. The `restart`/`pause`/`resume` exec
controls are optional. Its outputs are a value (`Time`) and an exec (`Expired`):

```wirescript
in trigger: exec
let t = Timer(10.0, restart = trigger)
out elapsed = t.Time
on t.Expired { /* fired when Time reaches the limit */ }
```

`ReadBrickGrid()` is pure and takes no arguments. It returns the brick grid
that this gate's microchip is placed on as an `entity`, ready to pass to entity
getters/setters or wire into gates that expect a brick grid:

```wirescript
let grid = ReadBrickGrid()
let origin = grid.GetLocation()
```

It is a value, not an event: the game no longer runs `on ReadBrickGrid() { ... }`
when the chip is pasted or loaded. Use [`on Loaded()`](statements.md#built-in-events)
to run something at load.

## Gate config properties

Some gate settings are not wire inputs. They are the checkboxes, dropdowns,
and values in a gate's in-game **settings menu**. Wirescript sets them as
**optional, constant-only call arguments**. A constant is baked into the gate's
data, and anything you omit keeps the game default. (A non-constant value is a
compile error, because these can't be wired.)

**Enum values are bare member names**, validated against the game's own enum
member list at compile time. An unknown name is a `WS028` error. A raw int is
also accepted (and range-checked).

```wirescript
let e = Easing(0.0, 1.0, t, function = Bounce, direction = InOut)
let c = ColorBlend(a, b, t, blendSpace = Oklab, clampAlpha = true)
p.DisplayText("hi", typeface = Bold, justify = Center, easing = EaseInOut)
```

Each enum used this way is also a [built-in game enum](enums.md#built-in-game-enums)
type of its own, so its qualified value form works too, side by side with the
bare name, and the two set the same field:

```wirescript
let e = Easing(0.0, 1.0, t, function = EasingFunction.Bounce, direction = EasingDirection.InOut)
let c = ColorBlend(a, b, t, blendSpace = ColorSpace.Oklab, clampAlpha = true)
p.DisplayText("hi", typeface = TextTypeface.Bold, justify = DisplayTextJustification.Center, easing = DisplayTextEasing.EaseInOut)
```

Every config field is settable, including fields that have no alias below. In
addition to the friendly names, each gate exposes each `bool`/`int`/`float`/`string`/`enum`
settings-menu field under its **raw game name**, so any of the ~60 config gates
works even without a curated alias:

```wirescript
SweepSimple(500.0, Direction = X_Negative, bOnlyHitPlayerBodyParts = true)
p.DisplayText("hi", FontSize = 40, Typeface = Bold, Justification = Center)
```

Completion offers these raw field names, and hovering one shows its type (and
enum members). The friendly alias and the raw name set the same field, so pick
either. The table below lists the aliases for the common gates.

Config attributes by gate:

| Gate | Config attributes |
|------|-------------------|
| `Sweep` | `bodyPartsOnly` |
| `SweepSimple` | `direction` (EBrickDirection), `spreadTowardCenter`, `detectBricks`, `detectPlayers1` to `4`, `bodyPartsOnly`, `detectPhysics`, `detectMap` |
| `Blend` | `clampAlpha` |
| `ColorBlend` | `blendSpace` (EBRColorSpace), `clampAlpha` |
| `Slerp` | `shortestPath`, `clampAlpha` |
| `Easing` | `function` (`EasingFunction`, schema `EBREasingFunction`), `direction` (`EasingDirection`, schema `EBREasingDirection`) |
| `Remap` | `function` (`EasingFunction`), `direction` (`EasingDirection`) |
| `ConvertColor` | `fromSpace`, `toSpace` (EBRColorSpace) |
| `DisplayText` | `typeface` (`TextTypeface`, schema `EBRTextTypeface`), `font` (a `$Font/...` asset ref). `justify` (`DisplayTextJustification`) and `easing` (`DisplayTextEasing`) are wire inputs that also take a bare member name |
| `GetAim` | `localAim` |
| `AddInventoryItemAdv` / `SetInventoryItemAdv` | `overrideColors`, `meshColors` (a color array), `ammoOverride` |

Enum member names: **EBRColorSpace** `Linear Srgb Oklab Hsv`; **EBREasingFunction**
`Linear Sine Quad Cubic Quart Quint Expo Circ Back Elastic Bounce`;
**EBREasingDirection** `In Out InOut`; **EBRTextTypeface** `Regular Bold Italic BoldItalic`;
**EBRDisplayTextJustification** `Left Center Right`; **EBRDisplayTextEasing**
`Linear EaseIn EaseOut EaseInOut`; **EBrickDirection** `X_Positive X_Negative
Y_Positive Y_Negative Z_Positive Z_Negative`.

Each of these schema enum types is also usable directly, under its clean
Wirescript name, as a [built-in game enum](enums.md#built-in-game-enums):
`EBREasingFunction` as `EasingFunction`, `EBREasingDirection` as
`EasingDirection`, `EBRColorSpace` as `ColorSpace`, `EBRTextTypeface` as
`TextTypeface`, `EBRDisplayTextJustification` as `DisplayTextJustification`,
`EBRDisplayTextEasing` as `DisplayTextEasing`, and `EBrickDirection` as
`Direction`.

## Clock (Event)

The Clock gate emits a periodic execution pulse forever. Wirescript uses it as
an event.
It takes `interval` and `enabled`, both **wire inputs**: a constant bakes into
the gate and a variable wires in, so the rate and the on/off state can change at
runtime. The handler body runs on each pulse.

```wirescript
in running: bool
var ticks: int = 0
on Clock(interval = 2.0, enabled = running) {
  ticks = ticks + 1
}
```

## ChatCommand (Event)

Registers a chat command. The call parens hold only **config args** (the
command name and an optional description). The event's data outputs are bound
by a trailing `-> (…)` tuple capture (or `-> { field: local }` record):

- **String literals** fill the config fields in order: `CommandName`, then
  `HelpText`. The description can also be given by name as `Description = "..."`.
- The `->` capture binds the event's data outputs, in order: `controller`
  (the player who typed it), then `arguments` (the command text as a string).

```wirescript
on ChatCommand("greet", "Greets the player") -> (controller, arguments) {
  // CommandName = "greet", HelpText = "Greets the player"
  // controller: the player who typed the command
  // arguments: the command text as a string
  controller.ShowStatusMessage("You said: ${arguments}")
}
```

The description is optional and can use the named form. Binding params are also
optional, so omit the ones you don't need:

```wirescript
on ChatCommand("wave", Description = "Wave at everyone") {
  // no bindings needed
}
```

## Custom Events

A named, cross-gate event channel that carries up to **8 data values**. Each comes
in two flavors: **personal** (same-owner) and **global** (ownership-agnostic), on
**separate** channel namespaces, so a personal `"x"` and a global `"x"` never mix.

### SendCustomEvent (Exec)

`SendCustomEvent(name, data1, … data8, target = …)` takes the channel `name` as a
**constant** string baked into the gate (a variable or computed value is a `WS028`
error), followed by up to 8 optional data values of any type. `target` is an
optional entity whose grid receives the matching **object** events. Delivery is
same-owner. Use `SendGlobalCustomEvent` for the ownership-agnostic version. It
fires all matching receivers.

> **A targeted send needs `isObject = true` on the receiver.** Giving a `target`
> (including the receiver spelling, `ent.SendCustomEvent(...)`) makes it an
> *object* event, and only an object-scoped receiver matches one. A plain
> `on CustomEvent("x")` is scoped grid-wide and **silently never fires** for it.
> There is no error or warning at check or compile. The two spellings pair up:
>
> ```wirescript ignore
> ent.SendCustomEvent("ui.init", who)          // targeted -> object event
> on CustomEvent("ui.init", isObject = true) -> (who: character) { }
>
> SendCustomEvent("ui.init", who)              // untargeted -> grid-wide
> on CustomEvent("ui.init") -> (who: character) { }
> ```
>
> This is the usual way to talk to a spawned prefab: keep the entity
> `SpawnPrefab` returned and target it, with `isObject = true` on the prefab's
> receivers.

```wirescript
on hit {
  SendCustomEvent("damage", 7, attacker)   // send an int + a character
}
```

### on CustomEvent (Event)

The receiver's call parens hold only the channel name (positional) and any
config (`isObject = true`); the **typed data outputs** are bound by a trailing
`-> (…)` tuple capture. Each data slot's type can be given explicitly
(`amount: int`) or **inferred from a matching in-unit
`SendCustomEvent`** on the same channel. When neither is available, the slot
defaults to `float` and a **`WS042`** warning is emitted (the game stores each
data slot as a typed value, not `any`). Unused slots default to `float`.
`isObject = true` is constant config that scopes the receiver to a specific
grid/object (an **object event**) instead of firing grid-wide.

```wirescript
var lastDamage: int = 0   // a top-level var is already persistent, so it needs no `static`

on CustomEvent("damage") -> (amount: int, attacker: character) {
  lastDamage = amount
  attacker.ShowStatusMessage("You took ${amount} damage")
}
```

### Global variants: SendGlobalCustomEvent / on GlobalCustomEvent

`SendGlobalCustomEvent` and `on GlobalCustomEvent` are the **ownership-agnostic**
counterparts: delivery ignores the owner, reaching every matching global receiver.
They have the same shape (constant channel name, up to 8 typed data values,
optional `target` entity, `isObject` config), on a channel namespace that is
**separate** from the personal one: `SendGlobalCustomEvent("x")` reaches
`on GlobalCustomEvent("x")` but never `on CustomEvent("x")`, and vice versa.

```wirescript
var total: int = 0

on GlobalCustomEvent("score") -> (points: int) { total = total + points }
on hit { SendGlobalCustomEvent("score", 10) }
```

> **One-tick delay.** A `CustomEvent` receiver fires on the **tick after** the
> `SendCustomEvent` runs. The pulse is delivered on the next frame, not
> synchronously within the sender's exec chain. Anything that must observe the
> event's effect immediately has to account for that one-tick latency (and a
> send → receive → send round trip costs a tick each hop).

> **Signature checking (`WS030`).** When a send targets a **constant** channel name,
> the compiler compares each data value's wire type against every matching receiver's
> declared param types (and a typed `let v: T = await CustomEvent("c")` capture's `T`)
> and warns (`WS030`) on a mismatch, e.g. sending a `float` where the receiver declared
> `int`. A constant is sent as the type it has, so `2.5` to an `int` param warns too. This runs for both the personal (`SendCustomEvent` /
> `on CustomEvent`) and global (`SendGlobalCustomEvent` / `on GlobalCustomEvent`) pairs,
> each within its own namespace. Types that share a
> wire variant are interchangeable and never flagged (any two entity kinds,
> `character`/`entity`/`controller`/…, are all the same `Object` variant). The channel
> name must be a constant literal, so every send's receiver set is known at compile time.
> In the editor, **go-to-definition** on a send's channel-name string jumps to the receiver.
>
> A *non-constant* data value is still typed as `float` on the wire at emit rather than
> from the value's real type. Full end-to-end typing waits on generics. For now the
> receiver's annotations are the source of truth, and constant sends carry their type.

## Prefab Spawning (Exec)

### SpawnPrefab

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `prefab` | prefab ref | No | The prefab to spawn: a `$./file.brz` archive, a `$./file.ws` source compiled on reference, or an inline `$` triple-backtick block ([prefab reference](expressions.md#prefab-references)). Embedded into the bundle at compile. |
| `offset` | `vector` | No | Spawn position, relative to the Spawn Prefab gate's own position (local, not world space) |
| `rotation` | `rotator` | No | Spawn rotation offset |
| `velocity` | `vector` | No | Initial velocity of the spawned entity |
| `lifetime` | `float` | No | Lifetime in seconds (0 = permanent) |
| `limit` | `int` | No | Max concurrent instances |
| `destroyAll` | `exec` | No | Wire an exec here; pulsing it destroys every entity this gate has already spawned. Independent of the spawn `Exec`, so one gate both spawns and clears. |

Returns: `entity`, the spawned entity. Give the prefab with a `$…brz`
reference; omit `prefab` to configure it on the placed gate in-game instead
(copy a prefab onto the Spawn Prefab brick).

`offset` is measured from the Spawn Prefab gate, so `Vec(0.0, 0.0, 50.0)` is
50 units above the spawner. To place a copy at a world position (a player's
location, a stored spot), spawn it and then move the returned entity with
[`SetLocation`](#setlocation):

```wirescript
in pad: zone
on ZoneEntered(zone = pad) -> (character) {
  let crate = SpawnPrefab(prefab = $./crate.brz, limit = 3)
  crate.SetLocation(character.GetLocation() + Vec(0.0, 0.0, 150.0))
}
```

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
compiled program carries its prefab. A `$./file.ws` reference compiles that
source first and embeds the result, and a prefab can also be written inline as a
`$` followed by a triple-backtick block. See
[Prefab References](expressions.md#prefab-references) and the per-entity
fan-out section in [best practices](best-practices.md).

### SpawnExplosion

Spawns an explosion of a given projectile/explosion class.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `projectileType` | class ref | Yes | The explosion/projectile class: a `$…` asset reference (or a wired value) |
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
| `projectileType` | class ref | Yes | The explosion/projectile class: a `$…` asset reference (or a wired value) |
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
| `distance` | `float` | Yes | Maximum ray distance |
| `radius` | `float` | No | Sphere radius (0 = line trace) |
| `relative` | `bool` | No | Interpret `origin`/`direction` in the owning grid's local frame |
| `ignore` | `entity` | No | A single entity to exclude from hits |
| `ignoreList` | `entity[]` | No | An array var of additional entities to exclude (on top of `ignore`) |
| `detectBricks` | `bool` | No | Detect brick grids, including spawned prefabs (**default false**) |
| `detectPlayers1` to `detectPlayers4` | `bool` | No | Detect players on collision channels 1-4 (**default false**) |
| `detectPhysics` | `bool` | No | Detect physics-simulating objects (**default false**) |
| `detectMap` | `bool` | No | Detect the static world / environment (**default false**) |
| `ignoreOwningGrid` | `bool` | No | Exclude the grid this gate sits on (prevents self-hits) |
| `collisionChannel` | `SweepCollisionChannel` | No | Collision channel to sweep on (`EBRSweepCollisionChannel`: 0 Physics, 1 Weapon, 2 Interaction, 3 Tool, 4-7 Player1-4, 8 NoAdditionalRestriction) |
| `bodyPartsOnly` | `bool` | No | Only hit player body parts (constant only) |

> **Detection is opt-in.** Every `detect*` flag defaults to `false`, so a Sweep
> with none set **detects nothing and always fires `Miss`.** Enable the channel you
> want: `detectBricks` for brick grids / spawned prefabs, `detectPlayers1` for
> players, `detectPhysics` for loose physics objects.

Returns a record with fields:
- `HitDistance: float`: Distance to hit point
- `HitEntity: entity`: Entity that was hit
- `HitLocation: vector`: World position of hit
- `HitNormal: vector`: Surface normal at hit
- `HitColor: color`: Color of the surface hit
- `HitBrick: brick`: Brick that was hit (see [Bricks](#bricks-exec))
- `Hit: exec`: Fires if something was hit
- `Miss: exec`: Fires if nothing was hit

```wirescript
on trigger {
  let aim = char.GetAim()
  // Run the Sweep inside the exec handler and handle it with nested Hit/Miss branches.
  // A top-level `Sweep(..., exec = t)` does not fire.
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
on RoundStart() {
  let r = Random(0, 15)
  if r == 0 { specialEvent = true }
}
```

`Random` uses the same PrimMath variant as the [arithmetic operators](expressions.md#arithmetic-operators), so its `min`/`max` may be a `vector`, `rotator`, `quat`, or `color`. It then rolls each component independently and returns that same type. `Random(Vec(0.0, 0.0, 0.0), Vec(1.0, 1.0, 1.0))` is a random point in the unit cube. `Random(a, b)` on two colors is a random color between them (all four RGBA channels). Both bounds share the type of the result.

`Random` is an exec function because it requires sequential execution to produce a new random value each time.

## Sleep / Delay (Pure)

Buffer gates that delay a value passing through. Most useful with `await` and the `_` armed flag placeholder.

| Function | Signature | Description |
|----------|-----------|-------------|
| `Sleep(input, delay?, hold?)` | `(input: T, delay?: float, hold?: float) -> T` | Delay by seconds (BufferSeconds gate) |
| `SleepTicks(input, delay?, hold?)` | `(input: T, delay?: int, hold?: int) -> T` | Delay by ticks (BufferTicks gate) |

- `input`: the value to delay. Use `_` inside `await` to wire the armed flag.
- `delay`: seconds/ticks to wait before the output follows the input.
- `hold`: seconds/ticks to hold the output after the input drops to zero. Set to -1 to use delay instead.

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
// Outside a handler, provide exec explicitly
let r = Random(0, 10, exec = someTrigger)
```

This wires `someTrigger` as the exec input of the gate, bypassing the requirement for an enclosing handler context.

## Newer builtins

Player-reference gates (`DisplayText`, `ShowChatMessage`, `HasRole`, leaderboard and
team setters, the join/left/chat events, `ControllerOf`/`CharacterOf`) target the
persistent player-state on the current build. The `controller` type is unchanged and
still wires straight into them, so existing scripts keep working.

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

The `CharacterFiredWeapon(character, direction, start, weapon, weaponName)` event fires when a player
fires a weapon (`direction`/`start` are vectors). `Sweep`/`SweepSimple` results also
carry a `HitColor` field (the color of the surface hit).

### Date / time (Pure)
```
GetUnixTime() -> int
FormatDate(unixTime: int, format: string, useUTC?: bool) -> { Output: string, Success: bool }
```

### Value conversions (Pure)
```
Remap(value, inMin, inMax, outMin, outMax, function?, direction?) -> float   // rescale a value between ranges
LogicalShiftRight(a: int, b: int) -> int              // logical (unsigned) >>
EnumToInt(value: enum) -> int                     // enum tag; folds a known variant, else uses the gate
IntToEnum(value: int, wrap?: bool) -> enum        // enum type from context; const folds, runtime uses the gate
ItemToPickup(item: entity) -> entity                  // pickup asset for an item
color.ConvertColor(fromSpace?: ColorSpace, toSpace?: ColorSpace) -> color
"A".ToCharCode() -> { Codepoint: int, Success: bool }
FromCharCode(codepoint: int) -> { Character: string, Success: bool }
GetValueType(value) -> Type                           // which kind of value is on the wire
value.getType() -> Type                                  // the same, as a method of any value
```

`Remap` follows a straight line by default. Its `function` and `direction`
settings bend it along an easing curve, with the same values `Easing` takes: a
bare member name (`Bounce`, `InOut`), an `EasingFunction` / `EasingDirection`
value, or an int. They are settings-menu fields, so each must be a constant
(`WS028` otherwise); the raw names `Function` and `Direction` set the same
fields.

```wirescript
in v: float
out r = Remap(v, 0.0, 1.0, 0.0, 100.0, function = Quad, direction = Out)
```

`GetValueType(v)`, or `v.getType()`, reports which kind of value a wire carries,
as the built-in enum `Type`. Use it to branch on what an untyped value (a
custom property, an `any`) holds. In each arm of a `match` on it, the tested
value has that arm's type, so it needs no `as`:

```wirescript
in e: entity
in go: exec
var total: int = 0
var log: string = ""

on go {
  let p = e.GetCustomProperty("score")
  match p.Value.getType() {
    Int    => { total += p.Value }
    String => { log = p.Value }
    _      => {}
  }
}
```

An `if` narrows the same way, in its block or its `then` branch:

```wirescript
in v: any
in go: exec
var total: int = 0

on go {
  if v.getType() is Type.Int {
    total += v
  }
  total = if v.getType() is Type.Int then total + v else total
}
```

The tested value must be a name or fields read off one (`v`, `p.Value`). A
`let` of the same name inside the arm is a new value and is not narrowed.
An `Object` arm reads the value as an `entity`. `Enum`, `Component` and
`Exec` have no one Wirescript type, so their arms leave the value an `any`.

| Member | Number | Member | Number |
|---|---|---|---|
| `Type.Bool` | 0 | `Type.String` | 7 |
| `Type.Int` | 1 | `Type.Enum` | 8 |
| `Type.Float` | 2 | `Type.Object` | 9 |
| `Type.Color` | 3 | `Type.Brick` | 10 |
| `Type.Vector` | 4 | `Type.Component` | 11 |
| `Type.Rotator` | 5 | `Type.Exec` | 12 |
| `Type.Quat` | 6 | | |

The numbers are the game's own, checked in game for every kind except `Enum`,
`Component` and `Exec`. `Object` covers entities, characters and players. A
custom property that was never set reads as `Float` (its value is 0), and a
game enum value on a wire reads as `Int`. Outside a `match` arm, name a member
through the enum (`Type.Int`): the bare names would clash with type names
and constructors like `Color(...)`.

`EnumToInt` / `IntToEnum` are the gate-backed twins of `.ToInt()` (=
`.Discriminant`) and `Enum.FromInt(n)`. `EnumToInt` requires an enum
argument (a non-enum is a type error). A compile-time-known enum folds to its
discriminant literal, and a runtime enum routes through the gate. `IntToEnum`'s
result is an enum whose concrete type comes from the annotated target (like
`FromInt`). A constant tag folds to the enum record, a runtime tag routes
through the gate, and `wrap` clamps an out-of-range tag. See
[enums.md](enums.md#enum-and-int-conversion).

`ParseInt` / `ParseNumber` likewise now expose a `Success` flag: they auto-unwrap to
their parsed `Value` in arithmetic/comparisons (`ParseInt(s) == 5`), and `.Success` is
`false` when the string wasn't a valid number.

### Self transform + simple raycast (Exec)
```
GetOwnTransform() -> { Location: vector, Rotation: rotator }
SweepSimple(distance: float, radius?: float, spreadConeAngle?: float, ..., collisionChannel?: SweepCollisionChannel)
  -> { HitDistance, HitEntity, HitLocation, HitNormal, HitColor, HitBrick, Hit, Miss }
```
  
`SweepSimple` sweeps **from its own brick** (the containing microchip's brick, or the
gate's own brick if not in a microchip), in the configured `direction`. It has no origin
input and **takes no receiver**. To sweep from an arbitrary point, use
the full `Sweep(origin, direction, distance, ...)` gate, which has a `vector` `origin` input.
`distance` is positional (`SweepSimple(500.0, ...)`).

### Zone array fills (Exec): array methods
```
arr.fillFromZoneEntities(zone, tagFilter?)   // entities inside a zone
arr.fillFromZonePlayers(zone, tagFilter?)    // players inside a zone
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

## Maps (`var m: Map<K, V>`)

A map is a keyed variable collection (the `MapVar` gate family), declared as a `var`
of the generic `Map<K, V>` type. Keys must be `int`, `string`, an object reference
(entity/character/controller) or a `brick`. Any other key type is a **`WS039`** error; values may
be any wire-storable scalar (int/float/bool/string/vector/rotator/quat/color/object).
A map starts empty unless given a constant literal initializer (`= {}` is the explicit
empty form).

A map **value** can also be a **record** (`var m: Map<int, Point>`): the map is stored
as one parallel map per field and `set`/`get`/`has`/`length`/`remove`/`clear`/`keys` fan
out (`values`/`copyFrom` are `WS050`). A record can never be a map *key*. See
[Records as storage](types.md#records-as-storage).

```wirescript
var scores: Map<string, int>
var names: string[]

on tick {
  scores.set("alice", 10)                 // insert / overwrite
  let g = scores.get("alice")             // { Value, Found }, auto-unwraps to Value
  if g.Found { PrintToConsole(g.Value) }
  if scores.has("bob") { ... }
  scores.remove("bob")                    // -> bool (was present)
  let n = scores.length()
  scores.keys(names)                      // fill an array with the keys
  scores.clear()
}
```

Methods (exec context, like array methods): `set(key, value)`, `get(key)`,
`has(key)`, `remove(key)`, `clear()`, `copyFrom(otherMap)`, `length()`,
`keys(destArray, includeDead?)`, `values(destArray)`.

A map keyed by objects or bricks can outlive what a key names. `keys` skips a key
whose brick no longer exists unless `includeDead = true`.

### Map literals

A `var` of `Map<K, V>` type can be given literal contents with `{ ... }`.
Entries use `=>` for any key expression, or `:` for a string / atom / int
*literal* key (or a bracketed `[expr]` computed key):

```wirescript
var m: Map<int, int>    = { :red => 10, 7 => 0 }    // arrow: any key
var s: Map<string, int> = { "red": 1, "blue": 2 }    // colon: string literal key
var a: Map<int, int>    = { :red: 1, :blue: 2 }      // colon: atom literal key
var e: Map<int, int>    = {}                         // explicit empty map
on tick { m = { [runtimeKey] => x } }                // computed key, desugars
```

A **constant** map literal (every key and value a compile-time constant) in a
`var` initializer bakes straight into the map at rest, with no runtime
gates, and the map loads pre-populated. An initializer with any non-constant
entry doesn't bake. Its entries are dropped (with a compiler warning) and
the map loads empty, so build it at runtime instead. Inside an exec handler,
`m = { ... }` (or a literal with runtime keys/values) desugars to `clear()`
followed by one `set(key, value)` per entry, in source order, the same
clear-then-populate shape as [array literal assignment](statements.md).

`{ foo: 1 }` with a **bare identifier** key is a record literal, not a map, because
`:` only introduces a map key for a string/atom/int literal or a `[expr]`
computed key. Use `foo => 1` or `[foo]: 1` to key a map by an identifier's
value.

Assigning a whole map from another map variable (`m = m2`) is not supported
because there is no whole-map-copy gate. Use `m.copyFrom(m2)` instead.

## Exec-flow gates (Union / Branch)

Two exec-signal combinators, callable like any other builtin:

```
Union(a: exec, b: exec) -> exec
Branch(cond: bool, exec: exec) -> (A: exec, B: exec)
```

- **`Union(a, b)`** merges two exec signals into one: the result fires whenever
  *either* input fires. Handy for running one handler from several triggers:

  ```wirescript
  let go = Union(init, Change(team))
  on go { rebuild() }
  ```

- **`Branch(cond, exec)`** routes an incoming exec to `.A` or `.B` depending on
  `cond` at the instant it fires (a runtime `if` for exec flow). Bind or trigger
  on the named outputs:

  ```wirescript
  on Branch(isRed, tick).A { redTick() }
  on Branch(isRed, tick).B { blueTick() }
  ```

## Callable gate builtins

Every variable / array / map wire gate is also exposed as a plain function named
after the in-game gate, alongside its method / operator / assignment form. The
two forms are **identical** (the call desugars to the method or assignment at
parse time), so pick whichever reads better. The names match the game gates so
they are easy to find (and complete).

**Variables**

| Builtin | Same as |
| --- | --- |
| `GetVariable(v)` | `v` |
| `SetVariable(v, x)` | `v = x` |
| `IncrementVariable(v, n)` | `v = v + n` |

**Arrays**: the function name maps to the [array method](#array-methods-exec) of
the same operation; the receiver is the first argument:

| Builtin | Same as | | Builtin | Same as |
| --- | --- | --- | --- | --- |
| `GetArrayElement(a, i)` | `a.get(i)` | | `SortArray(a, desc?)` | `a.sort(desc?)` |
| `SetArrayElement(a, i, x)` | `a[i] = x` | | `ReverseArray(a)` | `a.reverse()` |
| `PushToArray(a, x)` | `a.push(x)` | | `ShuffleArray(a)` | `a.shuffle()` |
| `PopFromArray(a)` | `a.pop()` | | `SwapArrayElements(a, i, j)` | `a.swap(i, j)` |
| `InsertArrayElement(a, i, x)` | `a.insert(i, x)` | | `SliceArray(a, src, s, n)` | `a.slice(src, s, n)` |
| `RemoveArrayElement(a, i)` | `a.remove(i)` | | `AppendArray(a, src)` | `a.append(src)` |
| `GetArrayLength(a)` | `a.length()` | | `CopyArray(a, src)` | `a.copyFrom(src)` |
| `FindArrayElement(a, x)` | `a.find(x)` | | `SumArray(a)` | `a.sum()` |
| `ClearArray(a)` | `a.clear()` | | `AverageArray(a)` | `a.average()` |
| `FillArray(a, x)` | `a.fill(x)` | | `ArrayMaximum(a)` | `a.max()` |
| `ResizeArray(a, n, x)` | `a.resize(n, x)` | | `ArrayMinimum(a)` | `a.min()` |
| `JoinArray(a, sep?)` | `a.join(sep?)` | | | |

**Array fills** (Gamemode / Zone gates, the function-call twins of the
[`fillFrom*` array methods](#zone-array-fills-exec-array-methods)):

| Builtin | Same as |
| --- | --- |
| `FillArrayFromPlayers(a)` | `a.fillFromPlayers()` |
| `FillArrayFromTeamMembers(a, team)` | `a.fillFromTeam(team)` |
| `GetPlayersInZone(a, zone, tagFilter?)` | `a.fillFromZonePlayers(zone, tagFilter?)` |
| `GetEntitiesInZone(a, zone, tagFilter?)` | `a.fillFromZoneEntities(zone, tagFilter?)` |

**Maps**: map to the [map methods](#maps-var-m-mapk-v) of the same name:

| Builtin | Same as | | Builtin | Same as |
| --- | --- | --- | --- | --- |
| `GetMapElement(m, k)` | `m.get(k)` | | `ClearMap(m)` | `m.clear()` |
| `SetMapElement(m, k, v)` | `m.set(k, v)` | | `CopyMap(m, src)` | `m.copyFrom(src)` |
| `HasMapElement(m, k)` | `m.has(k)` | | `GetMapLength(m)` | `m.length()` |
| `RemoveMapElement(m, k)` | `m.remove(k)` | | `GetMapKeys(m, dest)` | `m.keys(dest)` |
| | | | `GetMapValues(m, dest)` | `m.values(dest)` |

All array/map operations remain **exec-context only** (they run on the exec
chain), like their method forms.
