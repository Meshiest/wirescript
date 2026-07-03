# Wirescript SDK

Local development tools for Wirescript — a language that compiles to Brickadia wire graphs.

## Tools

```bash
# Type-check a file (reports errors and warnings)
node check.mjs myfile.ws

# Compile to .brz (loadable in Brickadia via BR.World.LoadAdditive)
node compile.mjs myfile.ws [output.brz]

# Format a file in-place (or --stdout to print)
node format.mjs myfile.ws [--stdout]

# Get type/hover info at a position (1-based line:col)
node hover.mjs myfile.ws 5 10

```

Requires Node.js 18+.

## Playground

The `playground/` directory contains the full browser-based editor. Serve it locally:

```bash
# Python
cd playground && python3 -m http.server 8080

# Node.js
cd playground && npx serve -p 8080
```

Then open http://localhost:8080.

## Files

```
├── check.mjs        # Type checker
├── compile.mjs      # Compiler (.ws → .brz)
├── format.mjs       # Code formatter
├── hover.mjs        # Type info lookup
├── docs/            # Language documentation (markdown)
├── pkg/             # WASM runtime (do not modify)
│   ├── wasm.js
│   └── wasm_bg.wasm
├── playground/      # Browser-based editor (serve with any HTTP server)
├── vscode/          # VS Code extension (WASM-powered, no native binary)
└── examples/        # Example programs
```

## Language Overview

Wirescript compiles to Brickadia's visual wire graph system. Every `var` becomes a Variable gate, every operator becomes an expression gate, and every `on` handler becomes an exec chain.

### Two Contexts
- **Pure context** — expressions that define signal-flow. Evaluated whenever inputs change.
- **Exec context** — imperative code inside `on` handlers. Runs sequentially when triggered.

### Core Syntax
```wirescript
var x: int = 0                                    // mutable variable
let y = x + 1                                     // computed binding (pure)
buffer prev = x                                   // one-tick delayed value
in trigger: bool                                  // input port
out result = x                                    // output port
on RoundStart { x = 0 }                           // event handler (exec)
chip { var a: int = 0 }                           // visual microchip grouping
mod inc(v: *int) { v = v+1 }                      // inline macro with ref params
return                                            // early return from handler/mod
type Point = { x: int, y: int }                   // record type
let p: Point = { x: 1, y: 2 }                     // record literal
let { x, y } = p                                  // destructuring
let q = { ...p, y: 99 }                           // spread
mod dist({ x, y }: Point) -> int { return x + y } // destructured params
```

### Imports
```wirescript
import "utils"                          // import all from utils.ws
import { clamp, swap } from "utils"     // selective import
import * as u from "utils"              // namespace — u.clamp()
```

### Receiver Syntax
Functions with entity/controller/character/vector first param support method calls:
```wirescript
entity.SetLocation(Vec(0.0, 0.0, 100.0))
ctrl.DisplayText("Hello!", fontSize = 40)
let d = a.Dot(b)
let aim = char.GetAim()   // record: aim.Origin / aim.Direction (camera aim)
```

### Built-in Events
`RoundStart`, `RoundEnd`, `CharacterSpawned(character)`, `CharacterDied(character)`,
`ControllerJoined(controller)`, `ControllerLeft(controller)`,
`ZoneEntered(character)`, `ZoneLeft(character)`, `BrickChanged(brick)`, `BrickRemoved(brick)`,
`ChatCommand("cmd", "help", controller, arguments)` — leading string literals set the command name + help text (help can be `Description = "..."`); the `controller`/`arguments` identifiers bind the outputs

### Types
`int` (64-bit), `float` (64-bit), `bool`, `string`, `entity`, `controller`, `character`,
`vector`, `rotator`, `color`, `brick`, `prefab`, `exec`, `ref T` / `*T`, `T[]`

Record and tuple types:
```wirescript
type Point = { x: int, y: int }
type Pair = (int, bool)
```

### Math Functions
`sin`, `cos`, `tan`, `atan`, `atan2`, `abs`, `sqrt`, `pow`, `clamp`, `min`, `max`, `round`, `floor`, `ceil`, `exp`, `ln`, `log`, `sign`, `lerp`, `fmod`, `Deg2Rad`, `Rad2Deg`

### String Methods
`s.Length()`, `s.Contains(search)`, `s.StartsWith(prefix)`, `s.EndsWith(suffix)`, `s.Find(search)`, `s.Substring(start, len)`, `s.Replace(search, repl)`, `s.Split(delim)`, `s.ToLower()`, `s.ToUpper()`, `s.Trim()`

### Array Methods
Arrays start empty unless initialized. A top-level initializer must be all literals (`array a: int[] = [1, 2, 3]`); `var a = [1, 2, 3]` infers `int[]`. To build one from runtime values, assign an array literal inside a handler — it desugars to clear + push/append, and `...other` spreads another array: `a = [n, 1, ...base, 5]`.
`arr.push(val)`, `arr.pop()`, `arr.length()`, `arr.remove(i)`, `arr.find(v)`, `arr.sort()`, `arr.clear()`, `arr.shuffle()` (and more — see docs)

### Vector/Color
`Vec(x, y, z)`, `Color(r, g, b, a)`, `v.SplitVec()`, `c.SplitColor()`, `v.Normalize()`, `v.Magnitude()`, `a.Dot(b)`, `a.Cross(b)`, `a.Distance(b)`. Read components directly with `v.x`/`v.y`/`v.z` and `c.r`/`c.g`/`c.b`/`c.a` on any vector/color value (variable, `let`, or literal).
- **sRGB / hex color**: `ColorSRGB(r,g,b,a)` (0-255), `ColorHex("#ff8800")`, `c.ToSRGB()`, `c.ToHex()`, `a.Blend(b, alpha)`.

### Rotation / Quaternion
`quat` is a quaternion (distinct from the euler `rotator`). `Rotation(pitch, yaw, roll)` makes a rotator; `dir.ToRotation()`, `q.ToDirection()`, `v.Rotate(q)`, `q.Invert()`, `from.RotationTo(to)`, `a.AngleTo(b)`, `a.Slerp(b, alpha)`, `axis.RotationByAngle(angle)`, `q.ToAxisAngle()`, `r.ToEuler()`.

### Stateful exec
`Cycle(count) -> int` and `Toggle() -> bool` advance each exec pulse.

## VS Code Extension

The `vscode/` directory contains a WASM-powered VS Code extension. To use:
1. Copy or symlink `vscode/` into `~/.vscode/extensions/wirescript-wasm`
2. Reload VS Code

Features: diagnostics, completions, hover, go-to-definition, formatting, rename, and **Compile and Copy Path** (`Ctrl+Shift+B`) — compiles to a temp `.brz` and copies the path to clipboard for Brickadia's load dialog.

## Using with Claude

1. **Check before compiling** — share `node check.mjs` output for error context
2. **Use hover for types** — `node hover.mjs file.ws LINE COL` shows type info
3. **Read docs** — language documentation is in the `docs/` directory
4. **Look at examples/** — working programs for reference
