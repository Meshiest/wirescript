# Wirescript Changelog

## 0.13.0 - 2026-07-10

- **~4x faster compiles on large projects** (gba: 5.9s -> 1.5s):
  - Each chip is laid out exactly once - the pre-emit layout pass no longer recurses into children that emit re-lays-out anyway
  - Layout: one toposort bucketed per connected component (was one full-graph sort *per* component); source-to-consumer alignment uses a prebuilt consumer map + O(1) occupancy checks
  - Emit: gate data fields' schema classification (variant/asset/enum/str) and interned names resolved once per gate class instead of per brick; brick assets no longer clone a String per brick
  - Lower: dead exec-union pruning is a single incremental worklist pass instead of a full graph rebuild per spliced union
- brdb 0.8.0: unset component fields skip a linear defaults scan + two error-String allocations per field; brz index compression actually works (its size guard was dead code)
- Fixed Inline field access on a call result dropping the call (`arr.find(x).Found` / `.Index`)
- Fixed a standalone `chip` whose exec-bearing body ends in `return <value>` shipping without its exec output
- Fixed `out X = X` emitting no wire when the output shares its name with a var/array
- LSP: hovering a field on a call result (`ids.find(x).Found`) resolves its type from the call's recor
- LSP: goto-definition on a namespaced call (`card.drawCard` with `import * as card`) resolves in the imported file instead of jumping to a same-named local decl
- Chip exec I/O gates are labeled `exec` and the anonymous `-> type` return output is labeled `return` (previously synthesized `_exec_in`/`_exec_out`/`_` ports had no label)
- `ControllerJoined`/`ControllerLeft` expose the player's id: `on ControllerLeft(controller, userId)` (`string`) - stable when the controller is torn down on disconnect
- **Calling a chip/mod before it is declared is now a hard error (WS021)** in both the compiler and the LSP, instead of silently lowering to a placeholder that reads its default (`0`) at runtime
- **`in X: T[]` array inputs are first-class** - an array-typed `in` port now supports array methods (`X.length()`, `X.push(v)`, ...) and can be passed to a mod/chip's `T[]` parameter
- **Namespaced (`import * as ns`) module members resolve inside their own mods**. Named imports (`import { f }`) were unaffected.
- **Chip/mod calls check their argument count (WS022)** in both the compiler and the LSP. User chips/mods have no default parameters, so a wrong positional-argument count left a param unbound (silently reading 0 / an empty value) or dropped an extra arg; it is now a hard error. An `exec =` trigger isn't counted as a parameter; a spread arg skips the check.

## 0.12.3 - 2026-07-10

- Fix literal constants not reaching anonymous chips

## 0.12.2 - 2026-07-09


- Add `ReadBrickGrid()` builtin

## 0.12.1 - 2026-07-09


- **Zone events bind their `Zone` input** - `on ZoneEntered(character, zone = z)` wires the value `z` into the event gate's `Zone` input port, so an `in` port wired to a zone brick selects the watched zone instead of hand-wiring each internal gate. Covers all zone events: `ZoneEntered`/`ZoneLeft`, `EntityZoneEntered`/`Left`, `ProjectileZoneEntered`/`Left`, `BrickChanged`/`BrickRemoved`.
- Fix false recursion flag on imported namespaced identifier conflicting with local identifier

## 0.12.0 - 2026-07-09

### Language / Compiler

- **Emitted saves label their elements with text decals** - the top-level chip is titled with the entry file's stem (or the `--name` override); named chips, variables/arrays, and microchip I/O gates get their names as diagonal floating labels. Synthesized `_`-prefixed ports stay unlabeled.
- **Var/array exec gates tag their variable** - `Var_Get`/`Var_Set`/`Var_Increment` and the array-var gates carry a smaller tag naming the variable they access, traced through the gate's ref wire (works across chip boundaries for captured vars).

## 0.11.0 - 2026-07-08

### Language / Compiler

- **Gate data mappings derive from game data** - the hand-maintained gate->data-struct table is gone; struct names and field lists come from the game-extracted pair table + schema, so new gates need no table edits. Stale entries for components the game doesn't have were dropped.
- **Vector/Rotation literals embed into gate data** - `e.SetLocation(Vec(0.0, 0.0, 100.0))` bakes the vector into the gate instead of spawning a wired `MakeVector`. `Split*` inputs still materialize.
- **Exhaustive gate-data write audit** - a test serializes a literal into every representable field of every game component through the real writer; a failure names the gate and field.
- **Record literals as call args bind their fields** - passing `{ a: 1, b: 2 }` to a destructured (`f({ a, b }: P)`) or whole-record (`f(p: P)`) param now lowers the fields.
- String constants inline as wire variants. Ports that can't hold an inline variant keep the real gate
- **Chips capture the whole enclosing scope** - Previously `let`/`in`/event-param references were dropped. Constants additionally clone into the chip so `let K = 2` used as `arr.push(K)` bakes `2` into the gate

### Bug Fixes

- **`min`/`max` and 14 more expression gates embed literals** - `min`, `max`, `sign`, `round`, `exp`, `ln`, the hyperbolics, `Deg2Rad`/`Rad2Deg`, `BitCount`, and `ScaleVec` share data structs that had no mapping, so literal args (`min(a, 3.0)`) were dropped.
- **`ScaleVec` wires to the real ports** - `Input`/`Scalar`, not `InputA`/`InputB` (which don't exist on the gate).
- Destructuring record literals now properly lowered to bindings.


## 0.10.2 - 2026-07-08

### Language / Compiler

- **emit/await loops with `buffer emit`** - a buffered back-edge `emit` after an `await` now forms a working loop: `buffer emit sig` (1 tick), `buffer(N)`, `buffer(0.5s)`, `buffer(myVar)`, or `buffer(delay, hold)` inserts a `Buffer(Ticks|Seconds)` gate — the tick barrier a wire-graph cycle needs. Constants bake into the gate; variables wire into the duration port.
- **Payload ferrying** - `emit sig = value` on a local signal stores the value in hidden per-signal vars (one per record field); `let x = await sig` / `let { a, b } = await sig` reads them back, so loop state can ride the signal. Cost: one `Var_Set` per field per emit, one `Var_Get` per field at the await. Previously the value was dropped.
- **Body-level `let x: exec` wires correctly** - signals declared inside a body got no hub, so `await x` lowered to a dead `_Unsupported` placeholder.
- **Signals are scoped per declaration** - two mods each declaring `let loop: exec` shared one signal (keyed by bare name): the second's `await` went dead and its emits cross-wired into the first's loop. Hubs are now keyed per declaration and resolved through the scope.
- **Handler-local array vars re-init correctly** - `var nums = [1,2,3]` in a body emitted a `Var_Set` wired from the ArrayVar's nonexistent `VarRef` port (in-game: "Wire source port VarRef does not exist in source component"); arrays now rebuild via clear + push.
- **Layout no longer panics on multi-cycle SCCs** - one feedback edge was dropped per SCC, so two loops sharing a chain crashed layout; it now iterates until acyclic.

### Language / Compiler (types)

- **`entity` coerces to `character`/`controller`** - an entity wire can carry a player (e.g. `Sweep`'s `HitEntity`), so character/controller receiver methods and typed params now accept entity values, wiring directly with no adapter gate — same rule as `character` ↔ `controller`.

### Bug Fixes

- **`CharacterDamaged` attacker is `character`-typed** - the attacker binding was `entity`, so character/controller receiver methods and typed params rejected it; attackers are always player characters. The weapon binding stays `entity` (an item, matched by entity-typed asset refs).
- `ShowStatusMessage` and 12 more gates persist their literal args
- **Recursive chip/mod calls error instead of crashing** - Now a `WS020` error

### Editor / IDE

- **Named-arg hovers only fire on the arg name** - in `delay = delay`, hovering the value showed the param docs instead of the symbol; the param hover now requires arg-name position (followed by a single `=`).
- **Method/call hovers only fire on the actual access** - hovering a variable/let/param whose name matches an array method (`var sum = 0`) or a builtin receiver-method (`var Teleport = 0`) showed the method/call hover instead of the symbol's own declaration. Array-method hovers now require a `.method` access, and builtin call/method hovers require actual call/method position (`recv.method` or `name(`); a bare identifier hovers as itself.

## 0.10.1 - 2026-07-07

### Language / Compiler

- **Asset references are `entity`-typed and usable as values** - `$Type/Name` now has type `entity` (was `any`), so `weapon == $BRItemBase/Weapon_Pickaxe` type-checks against an entity value (e.g. a `CharacterDamaged` weapon). As a value it materializes into the matching `*Reference` gate (`ItemReference`, `AudioReference`, `EntityTypeReference`, … chosen by asset type) — which holds the asset in its class/object field and outputs it as an entity wire — since assets can't be inlined into arbitrary ports like a Compare gate's input. Previously it errored (`WS004`).
- **`DisplayText` gained an `easing` param** - the interpolation curve for `transition` (`"Linear"` / `"EaseIn"` / `"EaseOut"` / `"EaseInOut"`), a property-only enum like `justify`. Animating a slot's position now eases instead of only running linearly.

## 0.10.0 - 2026-07-07

### Bug Fixes

- **`character` and `controller` wire directly** - no longer inserts a `GetFromEntity` adapter (an admin-only gate that got blocked on paste for non-admins, breaking every wire through it); the two types connect straight into each other's ports.
- **Gate brick colours no longer double-darkened** - they were pre-multiplied by γ=2.2 for a linear decode the game doesn't do (it renders colour bytes as sRGB), so every gate rendered too dark. Now the intended sRGB values.
- **Multi-byte string chars survive emit** - the lexer read each byte as its own `char`, mangling chars like `█`/`é` into garbage bytes; it now reads whole UTF-8 chars.
- **Long templates no longer drop values** - `FormatText` has only 7 substitution inputs, so a template with `>7` `${...}` values silently dropped the extras; templates now split across chained `FormatText` gates.
- **`on <local exec signal>` fires across handlers** - `emit sig` in one handler now triggers `on sig` in another, regardless of source order (the signal's binding was created after handlers lowered, so `on sig` was silently dropped).
- **Lexer no longer panics on stray multi-byte chars** - a non-ASCII char outside a string (e.g. `▲`) hit a mid-codepoint byte-slice and crashed the LSP; now UTF-8-safe.
- **`FindPlayer` is an exec gate returning `character`** - it has Exec/ExecOut ports and emits the found player's character; it was mis-declared as a pure gate returning `entity`.

### Editor / IDE

- **`$` reference highlighting + hovers** - prefab (`$./x.brz`) and asset (`$Type/Name`) refs get TextMate scopes and hovers; prefab hovers show the resolved path and (in the LSP) whether the file exists.
- **Prefab refs are navigable** - a resolvable `$./file.brz` is a clickable link / go-to-definition target (Ctrl/Cmd-click or F12 opens it).
- **Missing prefab files warn** - the LSP flags a `$./file.brz` that isn't on disk, or lacks the `.brz` extension.
- **Playground uploads `.brz` prefabs** - the sandbox has a Prefabs panel (upload button + drag-drop); uploaded files are stored as browser blobs (IndexedDB), offered by `$./` completion, and embedded at compile so `$./file.brz` works in the web playground, not just the CLI.
- **Named-arg completion + hover work in multi-line calls** - `find_enclosing_call` scanned only the current line, so a call whose args span multiple lines lost param-name completion and param-type hovers on its continuation lines; it now scans the whole call (skipping strings/comments).
- **Enum-valued args complete their values** - a named arg backed by a schema enum (e.g. DisplayText's `justify`) completes the enum's variant names (`Left` / `Center` / `Right`), auto-quoted when no quote is open yet.

### Language / Compiler

- **Prefab references embed a `.brz` into `SpawnPrefab`** - `$./file.brz` (relative) / `$/abs.brz` (absolute) read the archive at compile and embed it content-addressed (brdb 0.7 `add_prefab`), setting the gate's `Prefab` path. `.brz` required (`WS019`); resolution is pluggable via `EmitOptions::prefab_resolver`.

## 0.9.0 - 2026-07-07

### New Builtins

- **Split edge/change detectors** - `Edge(bool) -> {Rising, Falling: bool}`, `EdgeExec(float) -> {Rising, Falling: exec}`, `Changed(any) -> bool`, `Change(any) -> any`

### Bug Fixes

- `SpawnPrefab` gained the gate's `velocity` param (the `SpawnVelocity` input) - previously only settable via `SetVelocity` on the returned entity.
- `SpawnPrefab()` compiles again

### Gate Catalog / Data

- Gate inventory regenerated (314 -> 316 entries: the two exec detectors)
- Fixed the plain Edge Detector's emit data mapping key (was missing `Type` in the class name, so its component data was never written).

### Language / Compiler

- **`exec =` named arg on chip and mod calls** - pass a trigger when calling exec chips/mods outside an exec context (previously typechecked but lowered as dead gates). The call also returns the completion exec as an `exec` result field: `await r.exec` / `on r.exec { }`.
- **Import dependency pulling fixed** - imported declarations now pull same-file deps referenced from record/array literals, `emit` values, `await` exprs, and buffer inits; type aliases inline into imported `let`/`var`/`out`/`buffer`/`in` annotations (previously only chip/mod params).
- **WS013 understands `emit`** - the unassigned-output check counts `emit x (= expr)` and plain assigns anywhere in the body, per-output.
- **Named chip bodies capture top-level state** - free references to outer vars, arrays, buffers, and record bindings inside a named chip body now resolve against the caller's scope (previously they compiled as dead references).

### Parser

- **Multi-line array literals** - newlines allowed after `[`, around commas, and before `]`, with optional trailing comma - mirroring the call-arg rules. Covers top-level `array` initializers and runtime `foo = [...]` rebuilds.

### Editor / IDE

- **Formatter indents multi-line array literals** - both formatters (native `format_wirescript`, prettier plugin) track `[`/`]` depth like `(`/`)`; delimiter scanning stops at `//` so comments no longer skew indentation.
- **Formatter: one indent level per line** - a line opening several groups (`f(x, {`) indents its continuation once, not once per delimiter; the closing `})` returns to the opener's level.
- **One "Wirescript" entry in the formatter picker** - the extension keeps its prettier formatter and sends `provideFormatting: false` so the LSP doesn't register a duplicate.
- **Prefab path completion** - typing `$./` (or `$/`) completes `.brz` prefab references: the native LSP scans the document's directory; the wasm playground offers dragged-in files (new optional `prefabs_json` registry on `wirescript_compile` / `wirescript_completions`).

## 0.8.0 - 2026-07-06

### New Builtins and Methods

- **Chat / messaging** - `ctrl.ShowChatMessage(msg)` (a whisper only that player sees), `ctrl.ShowMessageBox(msg, title?)` (modal popup), and global `BroadcastChatMessage(msg)` / `BroadcastStatusMessage(msg, flash?)`. The old plugin-side whisper/broadcast is now fully expressible in wires.
- **Audio** - `entity.PlayAudioAt($BrickOneShotAudioDescriptor/..., volume?, pitch?, innerRadius?, maxDistance?, spatialized?)` plays a one-shot at an entity (characters work), and `PlayGlobalAudio(audio, volume?, pitch?)` plays for everyone. The audio descriptor is a `$` asset reference inlined into the gate's data.
- **Entity tags** - `entity.SetTag("...")` / `entity.GetTag() -> string` attach an arbitrary string to any entity and read it back (mark players with team/slot/role state; zones can filter on tags).
- **`FindPlayer(name)`** - pure value gate: look up a player entity by name.
- **`Change(input)`** - any-typed companion to `Edge`: pulses the input value through when it changes.
- **Quaternion raw components** - `Quat(x, y, z, w)`, `q.SplitQuat() -> {X, Y, Z, W}`, `a.QuatDot(b)`.
- **Inventory family** - `char.AddInventoryItem(item)` / `SetInventoryItem(item, slot?)`, `AddInventoryBrick(brick, size?)` / `SetInventoryBrick(...)`, `AddInventoryEntity(entityType)` / `SetInventoryEntity(...)`, and `AddInventoryItemAdv` / `SetInventoryItemAdv` with per-item overrides (`damage`, `speed`, `scale`, `itemName`, `projectile`). Asset args are `$Type/Name` references, like `GiveWeapon`.

### New Events

- **`CharacterDamaged(character, damage, attacker, attackerWeapon, attackerWeaponName)`** - a character took damage.
- **`EntityZoneEntered` / `EntityZoneLeft` (`entity`)** and **`ProjectileZoneEntered` / `ProjectileZoneLeft` (`character`, `projectile`, `weapon`, `weaponName`)** - zone events beyond characters; the projectile events' `character` is the shooter.

### Compiler / Output

- **Generic asset-field emission** - gates whose data struct has a `class`/`object` field (PlayAudioAt's `AudioDescriptor`, the inventory gates' `Item`/`EntityType`/`BrickAsset`/`ProjectileOverride`) now register an inlined `$` asset reference in the world's external-asset table automatically; `GiveWeapon`'s hand-built special case is no longer the only path. _Like GiveWeapon, the new gates' binary encoding needs in-game verification._

### Gate Catalog / Data

- Gate inventory regenerated (288 -> 314 entries, 26 new classes; the messaging/tag/zone-event/quaternion/inventory gates above are wired into the language).
- brdb component `_max` schema (286 structs) and `component_db.rs` (296 type mappings) regenerated from the same build's dump. The dump world only referenced 14 external assets, so `assets/external.rs` was left at the previous full catalog.
- Deliberately not exposed as builtins: the `*Reference` gates (`$Type/Name` syntax covers them), `Convert`/`ColorConvert` (implicit coercions cover the use cases), and `AddInventoryEntry` (opaque nested entry struct; `GiveWeapon` covers it).

## 0.7.0 - 2026-07-05

### Language Features

- **Scalar var type inference** - `var foo = ""` is a string var, `var n = 0` an int var, `var f = 1.5` a float var (also bools, negatives, and interpolated strings) - no annotation needed. A non-literal initializer refines the type from its expression (`var v = Vec(1.0, 2.0, 3.0)` is a `vector` var), same as buffers. Previously an unannotated var stayed `any`: every operator use failed with WS004 "no overload", and real mistakes (assigning a vector into an int var) passed silently - both now behave like the annotated form.
- **Everything casts to string** - all variant-able primitives (numbers, floats, bools, vectors, rotators, colors, entities, characters, controllers, bricks, prefabs) coerce to `string` wherever one is expected: `let s: string = 5` is a cast, not a WS016 warning, and `..` concat accepts any of them on either side (`"hi " .. player`). Unannotated array vars also infer constructor elements (`var pts = [Vec(1.0, 1.0, 1.0)]` is `vector[]`).
- **`Color()` returns `color`** - was `any`; matches `ColorSRGB`/`ColorHex`/`Blend`.

### Constant Folding

- **`Vec`/`Rotation`/`Color` on literal args fold to constants** - `var v = Vec(1.0, 2.0, 3.0)` bakes the value into the Variable gate's initial value (top-level constructor initializers were previously dropped to zero), and constant constructors are legal top-level array initializer elements: `array pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]` loads pre-populated.
- **Folded constants inline into consumers** - in expressions, a constant `Vec(...)` is a literal, not a `MakeVector` gate: it lands directly in the consuming gate's component data (`v = Vec(...)` on the Var_Set, math operands, select branches, `arr.push(Vec(...))`). Consumers that can only take wired inputs (entity `SetLocation`/`Teleport`, `.x` component splits, chip inputs) get a real `Make*` gate materialized automatically, so a constant is never silently zeroed.
- **Vars and arrays of every wire variant** - `var`/`static var` of `rotator` (zero), `quat` (identity), and `color` (opaque white) get type-matched initial values, and `rotator[]`/`quat[]`/`color[]` arrays back onto the game's typed array variants (`WireGraphRotatorArray`/`QuatArray`/`LinearColorArray`) instead of falling back to doubles.

### Dependencies

- Requires `brdb` 0.6.3 - the wire variant gained `Rotator`/`Quat`/`LinearColor` members (plus the matching typed array variants) mirroring the current game schema; only `WireGraphEnumWrapper` remains unmapped (no enum-typed vars in the language yet).

## 0.6.0 - 2026-06-30

- Gate inventory (285 -> 288: new `Convert` / `FindPlayer` gates), the brdb component `_max` schema (258 structs), and `component_db.rs`.
- **Sweep upgrade** - the raycast `Sweep(...)` gate gained per-channel detection flags: optional `detectBricks`, `detectPlayers1`–`detectPlayers4`, `detectPhysics`, `detectMap`, and `ignoreOwningGrid`.

## 0.5.0 - 2026-06-29

### Language Features

- **`quat` type + rotation/quaternion builtins** - a new `quat` primitive (quaternion, distinct from the euler `rotator`) plus concise receiver methods: `dir.ToRotation()`, `q.ToDirection()`, `v.Rotate(q)`, `q.Invert()`, `from.RotationTo(to)`, `a.AngleTo(b)`, `a.Slerp(b, alpha)`, `axis.RotationByAngle(angle)`, `q.ToAxisAngle()`, plus `Rotation(p, y, r)` / `r.ToEuler()` for the euler rotator.
- **sRGB / hex color builtins** - `ColorSRGB(r, g, b, a)` and `ColorHex("#rrggbb")` constructors, and `c.ToSRGB()` / `c.ToHex()` / `a.Blend(b, alpha)` receivers.
- **`Cycle(count)` / `Toggle()`** - stateful exec value gates (advance a counter / flip a bool each exec pulse).
- **User definitions shadow builtins** - a `chip`/`mod`/`fn` named like a builtin (e.g. `chip Toggle`) now takes precedence at the call site instead of resolving to the builtin.
- **Asset references** - `$AssetType/AssetName` (e.g. `$BRItemBase/Weapon_Pistol`) references an external asset the world embeds by name. Lexer/parser/typecheck support plus editor completion: typing `$` offers the asset types, `$Type/` offers that type's asset names (sourced from the brdb external-asset catalog). Assets register into the world's external-asset table and encode as an index on emit.
- **`HasRole` / `GiveWeapon`** - `ctrl.HasRole("Admin") -> bool` (role is a config string); `char.GiveWeapon($BRItemBase/Weapon_Pistol, slot)` sets an inventory slot to an item asset (the first asset-consuming gate - it registers the weapon and builds the nested `EntryPlan`). _The give-weapon binary encoding needs in-game verification._

### Gate Catalog / Output

- Refreshed the gate inventory added 26 new gate classes; the rotation/quaternion, sRGB-color, and cycle/toggle ones above are now wired into the language. Component data structs for all of them are registered for `.brdb` output.

## 0.4.0 - 2026-06-28

### Language Features

- **Pre-initialized arrays** - `array foo: int[] = [1, 2, -3]` declares an array with constant initial contents. At the top level every element must be a literal (numbers incl. negatives, strings, bools); the values are written straight into the array gate, so it loads pre-populated with no runtime setup. A non-literal element at the top level is now a clear error instead of being silently dropped.
- **Inferred array-typed vars** - `var foo = [1, 2, 3]` (no annotation) infers `int[]` from the literal and lowers to the same array gate as an `array` declaration; the var indexes and iterates as a real array.
- **Runtime array assignment + spread** - inside an exec handler, `foo = [a, 1, ...other, 5]` rebuilds an array variable's contents: it desugars to clear -> push each item -> append each `...spread`, so elements can be any runtime value and a spread splices another array in place. Spreads (`...`) are now parseable inside `[ ... ]` (`ArrayElem::Item` / `ArrayElem::Spread`).
- **Array methods, one source of truth** - every array method now derives from a single `catalog::arrays` table: completion offers the full set on any array-typed value (incl. `var ids: string[]`), and return types are derived from each method's gate output ports. `find` returns `{ Index, Found, Value }` that auto-unwraps to the int Index; `pop`/`min`/`max` expose `.IsEmpty`, and `insert`/`swap`/`slice` expose `.OutOfBounds`.
- **`GetAim` replaces `AimOrigin`/`AimDirection`** - a character's camera/aim is now one gate returning a record: `char.GetAim().Origin` / `.Direction`. Reading both fields shares a single GetAim gate instead of emitting two. The separate `AimOrigin`/`AimDirection` calls are removed.
- **Chat command config** - `on ChatCommand(...)` now accepts config args that set the gate's command name and help text. String literals fill `CommandName` then `HelpText` in order (`on ChatCommand("greet", "Greets the player")`), and the description can be named (`Description = "..."`). Bare identifiers still bind the event's data outputs (`controller`, `arguments`), so config and bindings can be mixed: `on ChatCommand("greet", "Greets the player", player, args) { ... }`.

### Bug Fixes

- **Vector components on stored values** - `.x`/`.y`/`.z` (and color `.r`/`.g`/`.b`/`.a`) now work on a vector held in a variable or `let` binding, not just an inline `Vec(...)`. Previously `let s = a + b; s.x` returned the whole vector instead of the component; the field-access lowering now falls through to the SplitVector/SplitColor gate for component names on a local.
- **`Vec(...)` literal arguments** - constant `Vec(1.0, 2.0, 3.0)` components are no longer dropped at emit (the `MakeVector` gate was missing its component-data struct mapping, so literal X/Y/Z defaulted to `0`). Vectors built from literals now hold their real values.

### Compiler / Output

- **Gate defaults resolve from component_db** - unspecified data-struct fields are no longer force-written with a schema type-zero; they're omitted so the brdb writer fills them from component_db's `STRUCT_DEFAULTS`. Fixes DisplayText emitting `FontSize = 0` / `Lifetime = 0`; they now resolve to the game defaults (`16` / `5`).

## 0.3.0 - 2026-06-27

### Language Features

- **String variables** - `var`/`static var` of type `string` are now stored in a Variable gate (the WireGraphVariant gained a `str` member). The `WS018` "strings can't be stored in vars" diagnostic is gone.
- **Native string equality** - `==` / `!=` on strings lower directly to the `CompareEqual` / `CompareNotEqual` gates, which now accept string-typed variant wires. Removed the old `contains(a,b) && length(a) == length(b)` workaround.
- **Vector arithmetic** - `+ - * / %` operate component-wise on two `vector` operands, and mixing a vector with a scalar (`v * 2.0`, `10.0 * v`, `v / 4`) broadcasts the scalar across the components - all on the same `MathAdd`/`Subtract`/`Multiply`/`Divide`/`Modulo` gates (their inputs accept the vector, `f64` and `i64` wire variants). The dedicated `Scale` helper still works too.
- **Any-variant variables** - a `var` can hold any WireGraphVariant member (`int`, `float`, `bool`, `string`, `vector`, and object types). Typed vars are emitted with a type-matched initial value instead of always defaulting to a number.
- **Typed arrays** - an array's declared element type now selects the backing `WireGraphArrayVariant` member (`int` -> Int64, `float` -> Double, plus Bool/String/Vector/Object arrays), so elements keep their declared type instead of all being stored as doubles.

### Gate Catalog

- **Regenerated inventory** - `data/logic_gate_inventory.simple.json` rebuilt from the latest in-game dump via a new checked-in generator (`scripts/gen_inventory.mjs`). Adds 76 gate classes (ArrayVar exec gates, new Gamemode/Controller/Character gates, string `ParseInt`/`ParseNumber`, reference gates) and gives 86 previously-`any` physical-brick ports concrete types. 175 -> 260 entries.
- **Refreshed brdb component tables** - `brdb` `component_db.rs` regenerated from the same dump so the new gates can be written (emit registers their component types). The removed `Gamemode_EndRound` gate is gone.

### New Builtins and Methods

- **Array methods** - `insert`, `find`, `sort(desc?)`, `reverse`, `sum`, `min`, `max`, `average`, `swap`, `fill`, `resize`, `append`, `copyFrom`, `slice`, `fillFromPlayers`, `fillFromTeam` joined the existing `push`/`pop`/`length`/`remove`/`clear`/`shuffle`. Every ArrayVar gate is now reachable.
- **Easing** - `Easing(a, b, blend, fn?, dir?)` and `Tween(target, duration, fn?, dir?)`, with easing function/direction passed as an int or an enum-name literal (`"Quad"`, `"InOut"`, ...) resolved against the engine's `EBREasingFunction`/`EBREasingDirection` enums.
- **Timer** - `Timer(limit, restart?, pause?, resume?)` function-call instance returning `{ Time: float, Expired: exec }`; the controls are optional exec inputs and `Expired` works with `on`/`await`.
- **String parsing** - `ParseInt(s) -> int` and `ParseNumber(s) -> float` (also `s.ParseInt()` / `s.ParseNumber()`).
- **Controller** - `GetUserName`, `GetUserId`, `GetDisplayName`, `IsTrusted`, `HasPermission`, `SetCanRespawn`, `SetTeamPinned`.
- **Character** - `GetDamage`, `SetDamage`, `IncDamage`, `SetTempPermission`.
- **Entity** - `SetFrozen`.
- **Gamemode** - `PlayerWins` / `TeamWins` (the new way to end a round; the imperative `EndRound` gate and builtin were removed), `GetCurrentRound`, `SetTeam`, `GetTeamName`, `GetTeamLeaderboardValue` / `SetTeamLeaderboardValue` / `IncrementTeamLeaderboardValue`.
- **Misc** - `PrintToConsole`, `DeltaTime`, `ServerUptime`, `NearlyEqual`, `Dampen`.

### Compiler / Output

- **Prefab output** - Compiled programs now emit a Brickadia **prefab** (`type: "Prefab"` with a `Meta/Prefab.json` carrying brick bounds computed from the microchip shell) instead of a world, so the `.brz` pastes like a native copied selection (Ctrl+V) with a correct preview.
- **Loads on current builds** - A compiled bundle now embeds only the component data structs the program actually uses, plus their transitive schema dependencies, and writes them dependency-first (a referenced struct is always defined before the struct using it) - matching how the game writes its own bundles. This replaces embedding the full gate catalog, which recent builds reject (`While building schema: While reading struct count` -> "failed to capture thumbnail / cache prefab"). Real programs stay well within the game's per-schema struct limit and load again.

## 0.2.0 - 2026-05-15

### Language Features

- **`emit target = expr`** - Set output value and fire exec in one statement. Works in both pure and exec contexts.
- **`await expr`** - Suspend exec chain and resume when expression fires. Armed-flag guard ensures one-shot execution (~7 gates per await).
- **`let name: exec`** - Local exec signals. `emit name` fires them from any handler; `await name` or `on name` listens.
- **`let x = await val on trigger`** - Capture a value when a trigger fires.
- **`await a || b`** - Race semantics via normal binary expressions.
- **`_` placeholder in await** - Resolves to the armed flag (`bool`). Enables `await Sleep(_, delay = 1.0)`.
- **Logical/comparison operator coercion** - `&&`, `||`, `^^`, `!`, `==`, `!=`, `<`, `>`, `<=`, `>=` now accept all wire variant types (bool, int, float, exec, string, entity, controller, character, brick, prefab).

### Builtin Functions

- **`Sleep(input, delay?, hold?)`** - BufferSeconds gate. Delays a value by seconds.
- **`SleepTicks(input, delay?, hold?)`** - BufferTicks gate. Delays a value by ticks.

### Compiler

- **`compile_to_world`** - New compile path returning `brdb::World` for `.brdb` output.
- **CLI `.brdb` support** - `just compile file.ws -o file.brdb` emits SQLite saves.
- **Compile progress** - LSP sends `wirescript/compileProgress` notifications; VS Code extension shows step counter in status bar.
- **`**` (pow) fix** - Now wires to `Input`/`Exponent` ports instead of `InputA`/`InputB`.
- **BRZ double-write fix** - Fixed `to_brz_vec` writing the archive twice (exact 2x file size).

### Editor / IDE

- **Inlay type hints** - Ctrl+Alt shows inferred types for `let`/`buffer` bindings. Works in VS Code and the web playground.
- **Hover gate estimates** - Hovering chips/mods/handlers/if-blocks shows estimated gate and microchip counts. Call-graph expansion sums callee costs recursively.
- **Record field hover fix** - `cpu.regs`, `cpu.cpsr` and nested field access now show types correctly.
- **`on` handler hover** - Shows gate estimate for the handler scope.
- **`if` hover estimates** - Shows gate cost for the if/else scope.
- **Tuple display** - Records with numeric keys show as `(bool, int)` instead of `{0: bool, 1: int}`.
- **`await` keyword highlighting** - Added to VS Code tmLanguage and Monaco monarch tokenizer.

### Playground

- **Inlay hints provider** - `wirescript_inlay_hints` WASM binding + Monaco `InlayHintsProvider`. Hidden by default, shown on Ctrl+Alt.
- **New `async_signals.ws` example** - Demonstrates emit-value, await, local exec signals, Sleep.

### Documentation

- Updated `statements.md` with emit-value, local exec signals, await, Sleep/SleepTicks.
- Updated `exec-context.md` with await section, `_` placeholder, Sleep examples.
- Updated `builtins.md` with Sleep/Delay section.
- Updated `expressions.md` with operator coercion for all wire variant types.
- Updated `types.md` with exec->bool coercion.
- Removed `fn` keyword references from docs.
- `just compile-brdb` recipe added.

### Test Files

- `projects/tests/src/` - New in-game test suite: `test_await_emit.ws`, `test_variables.ws`, `test_operators.ws`, `test_control_flow.ws`, `test_chips_mods.ws`, `test_strings.ws`.
- `crates/wirescript/tests/` - Integration tests: `await_test.rs`, `emit_value.rs`, `local_exec.rs`.

## 0.1.0 - 2026-05-07

### Language Features

- **Records & tuples** - User-defined record types (`type Point = { x: int, y: int }`), record literals, destructuring (`let { x, y } = p`), spread operator (`{ ...p, y: 99 }`), tuple types and literals
- **Spread in call args** - Pass record fields as named parameters: `foo({ ...defaults, x: 1 })`
- **Destructured params** - `mod dist({ x, y }: Point) -> int { ... }` in mods and chips
- **`on expr` syntax** - Trigger handlers on arbitrary exec expressions, not just named events
- **Exportable vars/buffers/arrays** - `var`, `buffer`, and `array` declarations are now importable across files
- **String `var` error (WS018)** - `var s: string` now errors at typecheck time (Brickadia runtime doesn't support string variables)
- **Ref/deref improvements** - LSP completions on arrays and refs, output ref/deref fixes

### Editor / IDE

- **Record field hovers** - Hover shows `State.counter: *int` for record fields and type declaration fields
- **Spread type validation** - Extra fields from spread are caught with errors pointing at the `...expr` span
- **Chip/mod context hover** - Hovering `chip`/`mod` keywords shows whether the block is pure or exec
- **Event parameter hovers** - Hover on event handlers shows parameter types
- **Mod/chip return type hovers** - Hover shows `-> (result: int)` return types
- **Formatter fixes** - Multi-line function call args indented correctly; operator continuation lines indented
- **`type` keyword highlighting** - VS Code extension highlights user-defined type names

### Playground

- **Docs panel refactor** - Docs fetched from `docs/*.md` instead of inline JS (~1900 lines removed from `docs.js`)
- **Examples loaded from files** - Playground examples loaded from `sdk/examples/*.ws` via fetch instead of hardcoded JS
- **New `records.ws` example** - Demonstrates records, destructuring, spread, and tuples

### Bug Fixes

- Fix branch scoping - variables declared in `if`/`else` branches no longer leak across branches
- Fix string comparison gate using wrong variant
- Fix inline modules adding extra microchip outputs
- Fix `return expr` in pure mods
- Fix `on var.value` not lowering handler body
- Fix emit not chaining union gates for multiple `emit` paths
- Fix import not pulling in same-file dependencies of imported declarations
- Fix array index access requiring exec context
- Fix array `.length()` / `.pop()` returning `Any` type
- Fix string wire port emits with literal variant values

## 0.0.0 - 2026-04-30

### Language Features

- **Standalone chip instantiation** - Named chips with `-> (outputs)` now compile to real child microchips, one per call site. Cross-chip wires resolve automatically.
- **`static var`** - Variables that persist across rounds: `static var highScore: int = 0`
- **`return expr`** - Return values from chips and mods
- **Single-output auto-unwrap** - `chip Foo() -> (result: int)` returns `int` directly instead of `{result: int}`
- **Block expressions** - `{ stmts; expr }` as expressions
- **Compound assignment** - `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`
- **`^^` logical XOR operator** - `a ^^ b` is true when exactly one operand is true
- **`let` type annotations** - `let x: int = expr`
- **Array params are always pass-by-reference** - `mod init(arr: int[])` passes the array by reference without needing `*`
- **`fn` deprecation** - `fn` declarations emit a warning (WS015) suggesting `let` instead

### Builtin Functions (30 new)

- **Select/Swap** - `Select(cond, a, b)`, `Swap(cond, a, b) -> {a, b}`
- **String ops** (all receiver on `string`) - `s.Length()`, `s.Contains(search)`, `s.StartsWith(prefix)`, `s.EndsWith(suffix)`, `s.Find(search)`, `s.Substring(start, len)`, `s.Replace(search, repl)`, `s.Split(delim) -> {Left, Right}`, `s.ToLower()`, `s.ToUpper()`, `s.Trim()`
- **Math** - `tan`, `log(x, base)`, `lerp(a, b, t)`, `fmod(a, b)`
- **Vector/Color** - `v.SplitVec() -> {x, y, z}`, `c.SplitColor() -> {r, g, b, a}`
- **Edge detector** - `Edge(input) -> {rising, falling}`
- **Gamemode** - `EndRound(winner?)`, `GetTeamByName(name)`
- **Character** - `ShowHint(char, title, text)`
- **Controller** - `ShowStatusMessage(ctrl, message)`
- **Bitwise** - `BitNand(a, b)`, `BitNor(a, b)`
- **Renamed** `MakeColor` -> `Color`
- **93% gate coverage** - 163 of 175 Brickadia gates supported

### Events

- **ChatCommand** - `on ChatCommand(controller, arguments) { ... }`

### Compiler Optimizations

- **NAND/NOR gate fusion** - `!(a && b)` compiles to a single NAND gate instead of NOT + AND. Same for `!(a || b)` -> NOR, `~(a & b)` -> BitwiseNAND, `~(a | b)` -> BitwiseNOR.
- **7.2x faster chip compile** - Schema parse caching + lower zstd level cuts chip program compile from 334ms to 46ms
- **Receiver syntax on all vector ops** - `v.Normalize()`, `a.Distance(b)`, `v.Magnitude()`, etc. now work as chained calls with correct type inference

### Editor / IDE

- **Cross-file go-to-definition** - Clicking an imported symbol jumps to its declaration in the source file. Clicking an import path opens that file.
- **Hover on `if` keywords** - Shows whether the block is in exec or pure context
- **Unused import/output warnings** - Warnings for imported symbols and outputs that aren't used
- **`wirescript-check` CLI** - Standalone type checker binary
- **VS Code extension auto-reload** - Extension reloads when the LSP binary changes

### Removals

- `ArrayRef` type removed - arrays are always references, use `int[]` everywhere
- `event` keyword removed (was already deprecated)
