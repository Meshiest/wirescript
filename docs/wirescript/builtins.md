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
| `lerp(a, b, t)` | `(a: float, b: float, t: float) -> float` | Linear interpolation |
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
| `a.Blend(b, alpha)` | `(color, color, float) -> color` | Blend two colors |

`SplitColor`, `ToSRGB`, `ToHex`, and `Blend` support receiver syntax on `color`.

```wirescript
let red = Color(1.0, 0.0, 0.0)
let orange = ColorSRGB(255, 128, 0, 255)
let hex = orange.ToHex()
let parts = red.SplitColor()  // parts.r = 1.0, parts.g = 0.0, ...
let mixed = red.Blend(orange, 0.5)
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

```wirescript
array scores: int[]
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
character.InputReader() -> {Forward, Right, Jump}
InputReader(character: character) -> {Forward, Right, Jump}
```

Read player input axes. Receiver on `character`.

Returns a record with fields:
- `Forward: float` -- Forward/backward axis (-1 to 1)
- `Right: float` -- Left/right axis (-1 to 1)
- `Jump: bool` -- Jump button pressed

```wirescript
let input = char.InputReader()
let moving = input.Forward != 0.0 || input.Right != 0.0
let jumping = input.Jump
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
target.DisplayText(text, ...)
DisplayText(target: controller, text: any, ...)
```

Display HUD text to a player. Receiver on `controller`.

#### DisplayText Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `target` | `controller` | Yes | Player to display to |
| `text` | `any` | Yes | Text content (auto-converted to string) |
| `positionX` | `float` | No | Horizontal position |
| `positionY` | `float` | No | Vertical position |
| `anchorX` | `float` | No | Horizontal anchor (0-1) |
| `anchorY` | `float` | No | Vertical anchor (0-1) |
| `scaleX` | `float` | No | Horizontal scale |
| `scaleY` | `float` | No | Vertical scale |
| `angle` | `float` | No | Rotation angle |
| `fontSize` | `float` | No | Font size |
| `outlineSize` | `float` | No | Text outline size |
| `justify` | `int` | No | Text justification |
| `lifetime` | `float` | No | Display duration (seconds) |
| `transition` | `float` | No | Transition animation duration |
| `textId` | `int` | No | Unique ID for updating text in-place |

```wirescript
on RoundStart {
  ctrl.DisplayText("Round Start!",
    positionX = 0.0,
    positionY = -200.0,
    fontSize = 48,
    lifetime = 3.0
  )
}

// Updating text in-place using textId
on trigger {
  ctrl.DisplayText("Score: ${score}",
    positionX = 100.0,
    positionY = -50.0,
    fontSize = 24,
    lifetime = 10.0,
    textId = 1
  )
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

Set entity position.

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
character.GetDamage() -> float
character.SetDamage(damage: float)
character.IncDamage(amount: float)
```

Read, set, or add to a character's accumulated damage. Receiver on `character`.

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
| `FindPlayer(query)` | `(query: string) -> entity` | Look up a player entity by name (pure) |
| `PrintToConsole(text)` | `(text: any) -> ()` (exec) | Print a value to the game console (debugging) |
| `DeltaTime()` | `() -> float` | Seconds elapsed since the previous tick |
| `ServerUptime()` | `() -> float` | Seconds the server has been running |
| `NearlyEqual(a, b, tolerance)` | `(a: float, b: float, tolerance: float) -> bool` | Approximate float equality |
| `Dampen(target, smoothTime)` | `(target: float, smoothTime: float) -> float` | Critically-damped smoothing toward a target |
| `Easing(a, b, blend, fn?, dir?)` | `(a: float, b: float, blend: float, fn?: any, dir?: any) -> float` | Ease from `a` to `b` by `blend` |
| `Tween(target, duration, fn?, dir?)` | `(target: float, duration: float, fn?: any, dir?: any) -> float` | Stateful eased value toward `target` |
| `Timer(limit, restart?, pause?, resume?)` | `(limit: float, restart?/pause?/resume?: exec) -> {Time: float, Expired: exec}` | Stateful countdown timer |

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

## Prefab Spawning (Exec)

### SpawnPrefab

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `offset` | `vector` | No | Spawn position offset |
| `rotation` | `rotator` | No | Spawn rotation offset |
| `lifetime` | `float` | No | Lifetime in seconds (0 = permanent) |
| `limit` | `int` | No | Max concurrent instances |

Returns: `entity` -- the spawned entity.

```wirescript
on trigger {
  let spawned = SpawnPrefab(
    offset = Vec(0.0, 0.0, 50.0),
    lifetime = 10.0,
    limit = 5
  )
  spawned.SetVelocity(linear = launchDir)
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
| `relative` | `bool` | No | Use relative coordinates |
| `ignore` | `entity` | No | Entity to ignore |

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
  let result = Sweep(aim.Origin, aim.Direction, 10000.0, radius = 5.0, ignore = char)

  // result.HitDistance, result.HitEntity, etc.
}
```

## Random (Exec)

| Function | Signature | Description |
|----------|-----------|-------------|
| `Random(min, max)` | `(min: int, max: int) -> int` | Generate a random integer in [min, max] |

```wirescript
on RoundStart {
  let r = Random(0, 15)
  if r == 0 { specialEvent = true }
}
```

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
