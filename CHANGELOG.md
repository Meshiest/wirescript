# Wirescript Changelog

## 0.18.0

- **Certified constant folding** - pure gates whose inputs are known constants are
  evaluated at compile time against the in-game-certified semantics table,
  constant-selector `Select`s short-circuit, constant-condition branches
  truncate their dead side (including across chip boundaries), all before
  layout. `Opaque(...)` and `@nofold` exempt code. The pass is opt-in while it
  stabilizes: enable it with a module-level `@fold` (or `--fold`); `--no-fold`
  (or a module-level `@nofold`) disables it and always wins over `@fold`.

## 0.17.1

- **Fixed a constant on a data-only port failing the build** - `DisplayText.fontSize` and 13 other params name settable fields with no wire input; binding one emitted a wire emit rejects. Now written as data, with a test pinning the list.
- **Fixed destructuring a builtin multi-output call binding nothing** - `let { Forward, Right } = c.InputReader()` left every name an unwired placeholder. Fields now bind to the gate's ports, and an unknown field errors with a suggestion instead of binding silently.
- **LSP reports lowering and emit errors on save** - Live analysis stays typecheck-only, so lowering problems only surfaced on an explicit Compile. Saving now runs the full pipeline and publishes its diagnostics.
- **New `arr.get(index)`** - A checked read giving `{ Value, OutOfBounds }`; used bare it is the element. Completion now offers those fields on `arr[i].` too.
- **`Blend` is the math blend gate** - An alias for `lerp`, accepting any math variant (float/int/vector/rotator/quat/color), as do `lerp`, `Easing`, and `Tween`. The colour-space gate is now `ColorBlend`.
- **`Opaque` hovers with its own docs** - It showed the Rerouter gate's blurb, which says nothing about the fold-hiding and type-erasing behaviour it exists for.
- **Fixed `.Value` on a multi-output result** - `a.pop().Value` typed as the whole record, so every use of it mismatched.
- **Fixed a type alias not resolving through a namespace import** - `import * as T` with `mod f() -> MyType` failed with "unknown type". Aliases now inline as they do for a named import, and `T.MyType` parses as a qualified type.
- **`GetLeaderboard` returns `int`** - It was typed `any`, so arithmetic on its result had no operator overload.
- **`bool` arithmetic with two bools** - `bool + bool` (and `- * / %`) now promotes to `int`, matching `bool`/`int` mixes and the bitwise ops; `(a && b) + (c && d)` compiles.

## 0.17.0

- **`Opaque(x)` builtin + `@nofold` annotation** - `Opaque` passes a value through a rerouter and hides it from constant folding; `@nofold` suppresses folding for a declaration, or for the whole file when placed at the top separated by a blank line. No-op placements warn. Groundwork for gate-semantics verification circuits.
- **Gate semantics probe** - `probes/gate_semantics.ws` prints every probed gate interaction to the console on paste; `scripts/gen_semantics.mjs` turns the dump into `data/gate_semantics.json`, and `scripts/gen_verifier.mjs` generates `probes/verify_semantics.ws`, which re-asserts every recorded case in-game.
- **Fixed a chip output named `x`/`y`/`z`/`r`/`g`/`b`/`a` reading garbage** - Those names collide with vector/color component access, so reading one split the scalar and returned a component instead of the output. Field access now splits only when the value really is a vector or color.
- **Fixed a `chip` called from inside a nested anon chip never firing** - Its exec trigger stayed at the root, and an exec pulse cannot cross into an instance grid nested inside another anon chip. Partition now routes boundary-pin wires into the module that directly contains the instance.
- **A constant argument to a `chip` no longer costs a gate per instance** - `F(1)` materialized a `_Var` in the caller and wired it across the boundary. The constant now folds into the instance itself and its input pin is dropped, matching what the equivalent `mod` emits.
- **Fixed a tuple-destructured `mod` parameter binding nothing** - `mod f((a, b): (int, int))` left every name unbound, so the body silently computed on zeros.
- **Fixed `let (a, b) = t` on a tuple value** - Both names bound nothing, and the shape was rejected as a non-tuple (WS010).
- **`out f(x)` is now a parse error** - The trailing call was dropped and re-parsed as a separate declaration, leaving a bare port.
- **Fixed a namespace import lost through a re-export** - `Ns` didn't travel with the imported declarations calling through it, so every `Ns.f(...)` silently did nothing at runtime.
- **Fixed a namespaced call losing its return type** - `Ns.f(x)` typed as `any`, so `Ns.f(x) + 1` failed operator resolution (WS004) and dropped the expression.
- **New tree-sitter grammar** - `editors/tree-sitter-wirescript/`, with highlight/locals/indent queries.
- **Docs: dropped `match` expressions** - Reserved keyword, but the parser has no expression form for it.
- **New docs page: [Best Practices](docs/wirescript/best-practices.md)** - Gate count and scaling: why every call site is a copy (for `mod` and `chip` alike), the call-site multiplier, single-dispatch event queues, deferred flags, and bitmask state.

## 0.16.4 - 2026-07-17

- **Fixed a constant shared across two chips reading 0 in one of them** - A literal used as a wired operand (e.g. `x * 4`) inside two separate `chip { ... }` blocks was merged by constant-deduplication into a single gate *before* anon-chip partitioning, leaving the second chip's operand wired across the chip boundary — where emit's per-module literal inlining can't reach it, so the operand silently read its port default (0). Deduplication now groups by owning chip, keeping a shared constant once per chip.
- **Fixed `on` handlers bound to `Change(x)`** - `Change`'s `OnChanged` output is now typed `exec` (was `any`), so `let c = Change(x)` + `on c { ... }` fires on the change pulse.
- **`then` may start its own line in an if expression** - `let x = if cond` followed by indented `then ...` / `else ...` lines now parses; the formatter indents both keywords one level as expression continuations.
- **Fixed transitive imports resolving in the wrong order** - When an imported file had imports of its own, its declarations were placed *before* the ones it imported, so any call into a deeper module was a use-before-declaration (WS021) or lowered against a missing declaration. This surfaced through a file that only re-exports another (`import "b"` alone in `a.ws`). Nested imports now resolve ahead of the importing file's own declarations, matching how the entry file already behaved.

## 0.16.3 - 2026-07-16

- **Compiler is ~2x faster on large projects** - mimalloc in the native binaries, thin LTO, a single-pass anon-chip partition, a quadratic wire-scan fix in inline chip calls, and Arc-shared ports/templates: lowering −69%, end-to-end −42%, lowering allocations −46%.
- **Compiled output is deterministic** - Anon-chip partitioning iterated a randomly-ordered set, so emitted gate/wire structure varied run to run; chips now partition in sorted order and repeated compiles produce identical graphs.
- **New `fuzz_programs` example** - Seeded grammar fuzzer that hunts silent miscompiles: programs with no error diagnostics whose output has `_Unsupported` gates, duplicate/fan-in wires, or dangling endpoints. Findings write to a gitignored `fuzz_findings/`.

## 0.16.2 - 2026-07-15

- **Fixed LSP crash (stack overflow) on large programs** - The cycle-analysis SCC walk is now iterative, and every `compile*` entry point runs on a worker thread with a 256 MiB reserved stack.
- **Fixed LSP crash on multi-byte text** - Hover/completion word scanners now step past characters by their real width, and member-receiver lookup converts the cursor column from chars to bytes.
- **Stale compile-command diagnostics clear on edit** - Editing or saving any `.ws` file clears the previous Compile command's diagnostics; the next explicit compile repopulates them.
- **Rename applies to every reference find-references sees** - Three `textDocument/rename` fixes:
  - Open files match by canonical path and references are deduplicated, so edits are no longer doubled and rejected.
  - `import { foo }` rewrites to `import { bar }`; shorthand expands to `{ foo: bar }`, and value-position names are untouched.
  - Rename works from any reference site (`u.foo`, record fields); built-in event names refuse rename.
- **Compiler is ~15% faster end-to-end** - Internal tables use the Fx hasher (`crate::collections`) instead of SipHash: lowering −15%/−26%, cycle analysis −45%/−55%, layout −36%, world building −21%/−33%. Map iteration is now deterministic, so output is more stable run-to-run.
- **~30% fewer allocations during lowering** - Chip declarations are shared via `Arc` instead of deep-cloned per call, and scope keys ride the interner; mod-heavy programs lower ~14% faster. New `count_allocs` example reports per-stage allocation counts/bytes.

## 0.16.1 - 2026-07-15

- **`chip let` labels the chip with its binding name** - `chip let x = ...` now shows the binding name(s) as its display label; an explicit `@label(...)` still overrides.
- **Wider vertical gap between chip-pane rows** - The wall layout's row-to-row gutter was widened so stacked chip planes read as separated.
- **Fixed repeated chip calls sharing wire endpoints** - Later instances of the same `chip` (`foo(0)`, `foo(1)`) wired their boundaries to the first instance and failed to load ("Failed to connect wire"). Boundary wires now remap to each instance's own nodes.
- **Hover on a namespace member shows its signature** - `u.foo` (via `import * as u`) now shows the full signature and exec-ness, matching a direct call's hover.
- **Unresolved namespace/method call is now a hard error (WS002)** - `ns.foo(...)` whose base is not in scope errors at the dangling identifier instead of lowering to a silent `_Unsupported` gate.
- **Organize Imports preserves namespace, bare, and multi-line imports** - Alt+Shift+O now keeps every import form (namespace/bare imports are never pruned; unused named imports still are) and sorts a namespace import before a named import from the same module.

## 0.16.0 - 2026-07-14

- **Fixed field triggers on a local in handlers** - `on x.field` (and negated `on !x.field`) now fires the matching output port instead of the local's default port.
- **Duplicate constant gates merged per chip** - A repeated constant is emitted once and fanned out. Pure gates with no wired input only; `Random`/stateful detectors are never merged; cut ~1200 gates on a large project.
- **LSP: member completion after `receiver.` wins inside a call arg** - `Call(arg = recv.<here>` completes `recv`'s members (records complete their fields, including in `on` handlers); `Call(<here>` still completes params.
- **LSP: more completion contexts** -
  - `import * as u` then `u.<here>` lists the module's members.
  - `pos.<here>` on a `var pos: vector` offers type methods + swizzle (`x`/`y`/`z`, `r`/`g`/`b`/`a`) alongside `.Value`/`.prev`; `static var` gets `.Value`/`.prev`.
  - Values typed by a `type Foo = { ... }` alias complete `Foo`'s fields.
  - User `mod`/`chip`/`fn` calls complete their param names instead of the global list.
  - All-required calls (`Vec(<here>)`) offer their params; method calls drop the bound receiver param.
  - `@`-annotation list adds `@label` and `@closed`.
  - Native LSP and web playground share these paths.
- **Doc comments on record-type fields** - A `///` on a field inside `type T = { ... }` now parses (was a parse error) and shows on hover of that field.
- **Fixed hover on a namespace alias** - Hovering `u` in `import * as u` shows `namespace u` and lists its members (was `namespace u: unknown`).
- **VS Code formatter (Prettier plugin)** - Adds a space after commas; splits long braced imports (fill) and binary-op statements (one operator per line, lowest precedence first) at 100 cols; joins `} else {`; honors `// fmt-ignore` (standalone guards the next line, trailing its own). `///` doc comments auto-continue on Enter.
- **Opened-plane headers space the doc off the title** - A blank line now separates the size-96 title from the chip/module doc comment.
- **Warn on asset/reference values in an array initializer (WS024)** - Assets (`$Type/Name`) and prefab refs can't bake into a constant `array`/`var` initializer; build with `.push(...)` in an exec handler. All reference types (`entity`/`character`/`controller`/`brick`/`prefab`/assets) share the object wire and can't be inlined.
- **Module doc comments stay separate from the first declaration** - A top-of-file `///` block followed by a blank line (or `//` comment) is the module doc (root plane header); a block directly above a declaration still documents it.

## 0.15.0 - 2026-07-13

- **Color arithmetic** - `+ - * / %` operate RGBA channel-wise on two `color` operands; a scalar broadcasts across channels (`tint * 0.5`). Same PrimMath gate as vectors/rotations.
- **`Random` is polymorphic** - `min`/`max` may be `vector`, `rotator`, `quat`, or `color`; each component rolls independently and the same type is returned (`Random(Vec(0,0,0), Vec(1,1,1))` → point in the unit cube). Scalar `int` form unchanged.
- **Fixed anonymous-record mod returns** - A `mod` returning a record literal (`return { head: ..., rest: ... }`) now destructures into per-field sources, so each field wires to its own value (was one `_Unsupported` gate).
- **Non-root chips compile open by default** - Opened planes stack as a wall above the compiled microchip (root at bottom, deeper nesting higher). New `@closed` collapses a chip but keeps its wall slot; `open chip` is now a no-op.
- **New `@label("text")` annotation** - Display-text override for chip labels/headers and `in`/`out` port labels (stacks with `@side` in any order); the wiring-UI port name is unchanged.
- **Opened planes render a header** - A size-96 title (the `@label` text, else the chip name) plus the chip's `///` doc comment, on an invisible brick at the plane's top edge.

## 0.14.1 - 2026-07-13

- **LSP: fixed a `return <expr>` mod mislabeled `exec` on hover** - `return` alone no longer forces the exec label; only an exec op in the returned expression (e.g. an array read) does.
- **Pruned duplicate constants from dual imports** - A module imported via both `import * as x` and a named import no longer ships its top-level `let` constants twice; fully-disconnected pure gates and orphan literals are pruned.

## 0.14.0 - 2026-07-12

- **Port-side rerouter pins** - `@left`/`@right`/`@top`/`@bottom` on a top-level port (same line or the line above) emits a pre-wired rerouter brick flush against that side of the microchip. Ports keep declaration order per side (ins/outs interleave), each pin is labeled with its port name; annotations inside `chip {}`/`mod` bodies error (WS023).

## 0.13.1 - 2026-07-11

- **Fixed `array.pop()` returning `0`** - Both gate outputs are now declared: `.Value` reads the popped element and `.IsEmpty` reads `bIsEmpty` (true once the array is empty after the pop).
- **Fixed `buffer` initializers inside chip/mod/handler bodies never wiring** - The initializer expression was silently dropped, leaving the buffer's input dangling.
- **Silently-dropped `var` initializers now warn (WSP001)** - Warns on a non-constant init in pure position, any non-constant `static var` init, and an exec-context array-var init that isn't an array literal. Use a `let` for a pure computed binding, or assign inside an exec handler.

## 0.13.0 - 2026-07-10

- **~4x faster compiles on large projects** (5.9s → 1.5s):
  - Each chip is laid out exactly once; the pre-emit layout pass no longer recurses into children.
  - Layout: one toposort bucketed per connected component; prebuilt consumer map + O(1) occupancy checks.
  - Emit: gate-data schema classification and interned names resolved once per gate class; no per-brick String clones.
  - Lower: dead exec-union pruning is a single incremental worklist pass.
- **brdb 0.8.0** - Unset component fields skip a defaults scan and two error-String allocations per field; brz index compression actually works (its size guard was dead code).
- **Fixed field access on a call result dropping the call** - `arr.find(x).Found` / `.Index` now keep the call.
- **Fixed a standalone `chip` losing its exec output** - An exec-bearing body ending in `return <value>` now ships the output.
- **Fixed `out X = X` emitting no wire** - Applies when the output shares its name with a var/array.
- **LSP: hover on a call-result field resolves its type** - `ids.find(x).Found` resolves from the call's record.
- **LSP: goto-definition on a namespaced call resolves in the imported file** - `u.foo` with `import * as u` no longer jumps to a same-named local decl.
- **Chip exec I/O gates are labeled** - Exec gates say `exec`; the anonymous `-> type` return output says `return` (synthesized ports had no label).
- **`ControllerJoined`/`ControllerLeft` expose the player's id** - `on ControllerLeft(controller, userId)` (`string`); stable when the controller is torn down on disconnect.
- **Calling a chip/mod before it is declared is a hard error (WS021)** - In both the compiler and the LSP (was a silent placeholder reading its default `0`).
- **`in X: T[]` array inputs are first-class** - An array-typed `in` port supports array methods (`X.length()`, `X.push(v)`, ...) and passes to a mod/chip's `T[]` parameter.
- **Namespaced module members resolve inside their own mods** - `import * as ns` only; named imports were unaffected.
- **Chip/mod calls check their argument count (WS022)** - Hard error in both the compiler and the LSP (a wrong count silently left a param unbound or dropped an arg). An `exec =` trigger isn't counted; a spread arg skips the check.

## 0.12.3 - 2026-07-10

- **Anonymous-chip constants** - Fixed literal constants not reaching anonymous chips.

## 0.12.2 - 2026-07-09

- **`ReadBrickGrid()`** - New builtin.

## 0.12.1 - 2026-07-09

- **Zone events bind their `Zone` input** - `on ZoneEntered(character, zone = z)` wires `z` into the event gate's `Zone` port, so a wired `in` port selects the watched zone. Covers `ZoneEntered`/`ZoneLeft`, `EntityZoneEntered`/`Left`, `ProjectileZoneEntered`/`Left`, `BrickChanged`/`BrickRemoved`.
- **Fixed a false recursion flag** - An imported namespaced identifier no longer triggers it when conflicting with a local identifier.

## 0.12.0 - 2026-07-09

### Language / Compiler

- **Emitted saves label their elements with text decals** - The top-level chip is titled with the entry file's stem (or `--name`); named chips, variables/arrays, and microchip I/O gates get diagonal floating name labels. `_`-prefixed ports stay unlabeled.
- **Var/array exec gates tag their variable** - `Var_Get`/`Var_Set`/`Var_Increment` and array-var gates carry a smaller tag naming the accessed variable, traced through the ref wire (works across chip boundaries for captured vars).

## 0.11.0 - 2026-07-08

### Language / Compiler

- **Gate data mappings derive from game data** - Struct names and field lists come from the game-extracted pair table + schema, so new gates need no table edits. Stale entries for components the game lacks were dropped.
- **Vector/Rotation literals embed into gate data** - `e.SetLocation(Vec(0.0, 0.0, 100.0))` bakes the vector into the gate instead of spawning a wired `MakeVector`. `Split*` inputs still materialize.
- **Exhaustive gate-data write audit** - A test serializes a literal into every representable field of every game component through the real writer; a failure names the gate and field.
- **Record literals as call args bind their fields** - `{ a: 1, b: 2 }` passed to a destructured (`f({ a, b }: P)`) or whole-record (`f(p: P)`) param now lowers the fields.
- **String constants inline as wire variants** - Ports that can't hold an inline variant keep the real gate.
- **Chips capture the whole enclosing scope** - `let`/`in`/event-param references now resolve; constants clone into the chip, so `let K = 2` used as `arr.push(K)` bakes `2` into the gate.

### Bug Fixes

- **`min`/`max` and 14 more expression gates embed literals** - `min`, `max`, `sign`, `round`, `exp`, `ln`, the hyperbolics, `Deg2Rad`/`Rad2Deg`, `BitCount`, and `ScaleVec` no longer drop literal args like `min(a, 3.0)`.
- **`ScaleVec` wires to the real ports** - `Input`/`Scalar` instead of the nonexistent `InputA`/`InputB`.
- **Destructuring record literals** - Now properly lowered to bindings.

## 0.10.2 - 2026-07-08

### Language / Compiler

- **emit/await loops with `buffer emit`** - `buffer emit sig` (1 tick), `buffer(N)`, `buffer(0.5s)`, `buffer(myVar)`, or `buffer(delay, hold)` inserts the `Buffer(Ticks|Seconds)` gate a wire-graph cycle needs. Constants bake into the gate; variables wire the duration port.
- **Payload ferrying** - `emit sig = value` stores the value in hidden per-signal vars (one per record field); `let x = await sig` / `let { a, b } = await sig` reads it back. Cost: one `Var_Set` per field per emit, one `Var_Get` per field at the await.
- **Body-level `let x: exec` wires correctly** - `await x` on a body-declared signal no longer lowers to a dead placeholder.
- **Signals are scoped per declaration** - Two mods each declaring `let loop: exec` no longer share one signal; hubs are keyed per declaration and resolved through the scope.
- **Handler-local array vars re-init correctly** - `var nums = [1,2,3]` in a body rebuilds via clear + push instead of wiring a nonexistent `VarRef` port.
- **Layout no longer panics on multi-cycle SCCs** - Feedback-edge removal iterates until acyclic, so two loops sharing a chain lay out.

### Language / Compiler (types)

- **`entity` coerces to `character`/`controller`** - Character/controller receiver methods and typed params accept entity values (e.g. `Sweep`'s `HitEntity`), wiring directly with no adapter gate.

### Bug Fixes

- **`CharacterDamaged` attacker is `character`-typed** - Was `entity`, which receiver methods and typed params rejected. The weapon binding stays `entity`.
- **`ShowStatusMessage` and 12 more gates** - Literal args now persist.
- **Recursive chip/mod calls error instead of crashing** - Now a `WS020` error.

### Editor / IDE

- **Named-arg hovers only fire on the arg name** - In `delay = delay`, hovering the value shows the symbol, not the param docs.
- **Method/call hovers only fire on the actual access** - Array-method hovers require a `.method` access; builtin call/method hovers require `recv.method` or `name(`. A bare identifier (e.g. `var sum = 0`) hovers as itself.

## 0.10.1 - 2026-07-07

### Language / Compiler

- **Asset references are `entity`-typed and usable as values** - `$Type/Name` is `entity` (was `any`), so `weapon == $BRItemBase/Weapon_Pickaxe` type-checks instead of erroring (`WS004`). As a value it materializes into the matching `*Reference` gate (`ItemReference`, `AudioReference`, `EntityTypeReference`, ... by asset type), which outputs the asset as an entity wire.
- **`DisplayText` gained an `easing` param** - The interpolation curve for `transition` (`"Linear"` / `"EaseIn"` / `"EaseOut"` / `"EaseInOut"`), a property-only enum like `justify`.

## 0.10.0 - 2026-07-07

### Bug Fixes

- **`character` and `controller` wire directly** - No more `GetFromEntity` adapter, an admin-only gate that got blocked on paste for non-admins.
- **Gate brick colours no longer double-darkened** - Colours emit as the intended sRGB values instead of being pre-multiplied by γ=2.2.
- **Multi-byte string chars survive emit** - The lexer reads whole UTF-8 chars, so `█`/`é` no longer mangle.
- **Long templates no longer drop values** - `FormatText` has only 7 substitution inputs; templates with more `${...}` values split across chained gates.
- **`on <local exec signal>` fires across handlers** - `emit sig` in one handler triggers `on sig` in another, regardless of source order.
- **Lexer no longer panics on stray multi-byte chars** - A non-ASCII char outside a string (e.g. `▲`) is now UTF-8-safe instead of crashing the LSP.
- **`FindPlayer` is an exec gate returning `character`** - Has Exec/ExecOut ports and emits the found player's character; was mis-declared pure returning `entity`.

### Editor / IDE

- **`$` reference highlighting + hovers** - Prefab (`$./x.brz`) and asset (`$Type/Name`) refs get TextMate scopes and hovers; prefab hovers show the resolved path and (in the LSP) whether the file exists.
- **Prefab refs are navigable** - A resolvable `$./file.brz` is a clickable link / go-to-definition target (Ctrl/Cmd-click or F12).
- **Missing prefab files warn** - The LSP flags a `$./file.brz` that isn't on disk or lacks the `.brz` extension.
- **Playground uploads `.brz` prefabs** - A Prefabs panel (upload + drag-drop) stores files as browser blobs (IndexedDB), offers them in `$./` completion, and embeds them at compile.
- **Named-arg completion + hover work in multi-line calls** - The enclosing-call scan covers the whole call (skipping strings/comments), not just the current line.
- **Enum-valued args complete their values** - A named arg backed by a schema enum (e.g. `justify`) completes its variants (`Left` / `Center` / `Right`), auto-quoted when no quote is open.

### Language / Compiler

- **Prefab references embed a `.brz` into `SpawnPrefab`** - `$./file.brz` (relative) / `$/abs.brz` (absolute) embeds the archive content-addressed (brdb 0.7 `add_prefab`) and sets the gate's `Prefab` path. `.brz` required (`WS019`); resolution pluggable via `EmitOptions::prefab_resolver`.

## 0.9.0 - 2026-07-07

### New Builtins

- **Split edge/change detectors** - `Edge(bool) -> {Rising, Falling: bool}`, `EdgeExec(float) -> {Rising, Falling: exec}`, `Changed(any) -> bool`, `Change(any) -> any`.

### Bug Fixes

- **`SpawnPrefab` gained a `velocity` param** - The gate's `SpawnVelocity` input.
- **`SpawnPrefab()`** - Compiles again.

### Gate Catalog / Data

- **Gate inventory regenerated** - 314 -> 316 entries (the two exec detectors).
- **Edge Detector emit mapping key fixed** - The class name was missing `Type`, so its component data was never written.

### Language / Compiler

- **`exec =` named arg on chip and mod calls** - Pass a trigger when calling exec chips/mods outside an exec context. The call returns the completion exec as an `exec` result field: `await r.exec` / `on r.exec { }`.
- **Import dependency pulling fixed** - Imports pull same-file deps in record/array literals, `emit` values, `await` exprs, and buffer inits; type aliases inline into imported `let`/`var`/`out`/`buffer`/`in` annotations, not just chip/mod params.
- **WS013 understands `emit`** - The unassigned-output check counts `emit x (= expr)` and plain assigns anywhere in the body, per-output.
- **Named chip bodies capture top-level state** - Free references to outer vars, arrays, buffers, and record bindings resolve against the caller's scope.

### Parser

- **Multi-line array literals** - Newlines allowed after `[`, around commas, and before `]`, with optional trailing comma - mirroring call-arg rules. Covers top-level `array` initializers and runtime `foo = [...]` rebuilds.

### Editor / IDE

- **Formatter indents multi-line array literals** - Both formatters (native, prettier plugin) track `[`/`]` depth like `(`/`)`; delimiter scanning stops at `//` so comments don't skew indentation.
- **Formatter: one indent level per line** - A line opening several groups (`f(x, {`) indents its continuation once, not once per delimiter; the closing `})` returns to the opener's level.
- **One "Wirescript" entry in the formatter picker** - The extension keeps its prettier formatter and sends `provideFormatting: false` so the LSP doesn't register a duplicate.
- **Prefab path completion** - `$./` (or `$/`) completes `.brz` refs: the native LSP scans the document's directory; the wasm playground offers dragged-in files via a new optional `prefabs_json` registry.

## 0.8.0 - 2026-07-06

### New Builtins and Methods

- **Chat / messaging** - `ctrl.ShowChatMessage(msg)` (per-player whisper), `ctrl.ShowMessageBox(msg, title?)` (modal popup), and global `BroadcastChatMessage(msg)` / `BroadcastStatusMessage(msg, flash?)`.
- **Audio** - `entity.PlayAudioAt($BrickOneShotAudioDescriptor/..., volume?, pitch?, innerRadius?, maxDistance?, spatialized?)` plays a one-shot at an entity (characters work); `PlayGlobalAudio(audio, volume?, pitch?)` plays for everyone. The descriptor is an inlined `$` asset reference.
- **Entity tags** - `entity.SetTag("...")` / `entity.GetTag() -> string` attach an arbitrary string to any entity and read it back; zones can filter on tags.
- **`FindPlayer(name)`** - Pure value gate looking up a player entity by name.
- **`Change(input)`** - Any-typed companion to `Edge`: pulses the input value through when it changes.
- **Quaternion raw components** - `Quat(x, y, z, w)`, `q.SplitQuat() -> {X, Y, Z, W}`, `a.QuatDot(b)`.
- **Inventory family** - `char.AddInventoryItem(item)` / `SetInventoryItem(item, slot?)`, `AddInventoryBrick(brick, size?)` / `SetInventoryBrick(...)`, `AddInventoryEntity(entityType)` / `SetInventoryEntity(...)`, and `AddInventoryItemAdv` / `SetInventoryItemAdv` with overrides (`damage`, `speed`, `scale`, `itemName`, `projectile`). Asset args are `$Type/Name` references.

### New Events

- **`CharacterDamaged(character, damage, attacker, attackerWeapon, attackerWeaponName)`** - A character took damage.
- **`EntityZoneEntered` / `EntityZoneLeft` (`entity`)** and **`ProjectileZoneEntered` / `ProjectileZoneLeft` (`character`, `projectile`, `weapon`, `weaponName`)** - Zone events beyond characters; the projectile events' `character` is the shooter.

### Compiler / Output

- **Generic asset-field emission** - Gates with a `class`/`object` data field (`AudioDescriptor`, `Item`, `EntityType`, `BrickAsset`, `ProjectileOverride`) register inlined `$` asset references in the world's external-asset table automatically. _Binary encoding needs in-game verification._

### Gate Catalog / Data

- **Gate inventory regenerated** - 288 -> 314 entries (26 new classes); the messaging/tag/zone-event/quaternion/inventory gates are wired into the language.
- **brdb data regenerated** - Component `_max` schema (286 structs) and `component_db.rs` (296 type mappings). `assets/external.rs` kept the previous full catalog (the dump referenced only 14 assets).
- **Deliberately not exposed as builtins** - The `*Reference` gates (`$Type/Name` covers them), `Convert`/`ColorConvert` (implicit coercions cover them), and `AddInventoryEntry` (opaque nested struct; `GiveWeapon` covers it).

## 0.7.0 - 2026-07-05

### Language Features

- **Scalar var type inference** - `var foo = ""` is a string var, `var n = 0` an int var, `var f = 1.5` a float var (also bools, negatives, interpolated strings). A non-literal initializer refines from its expression (`var v = Vec(1.0, 2.0, 3.0)` is `vector`), same as buffers.
- **Everything casts to string** - All variant-able primitives (numbers, floats, bools, vectors, rotators, colors, entities, characters, controllers, bricks, prefabs) coerce to `string`: `let s: string = 5` is a cast, not a WS016 warning, and `..` accepts any of them (`"hi " .. player`). Unannotated array vars also infer constructor elements (`var pts = [Vec(1.0, 1.0, 1.0)]` is `vector[]`).
- **`Color()` returns `color`** - Was `any`; matches `ColorSRGB`/`ColorHex`/`Blend`.

### Constant Folding

- **`Vec`/`Rotation`/`Color` on literal args fold to constants** - `var v = Vec(1.0, 2.0, 3.0)` bakes into the Variable gate's initial value, and constant constructors are legal top-level array initializer elements (loads pre-populated).
- **Folded constants inline into consumers** - A constant `Vec(...)` lands as a literal in the consuming gate's data (`Var_Set`, math operands, select branches, `arr.push`); wire-only consumers (`SetLocation`/`Teleport`, component splits, chip inputs) get a `Make*` gate materialized, never silently zeroed.
- **Vars and arrays of every wire variant** - `rotator` (zero), `quat` (identity), and `color` (opaque white) vars get type-matched initial values; `rotator[]`/`quat[]`/`color[]` back onto the typed array variants (`WireGraphRotatorArray`/`QuatArray`/`LinearColorArray`) instead of doubles.

### Dependencies

- **Requires `brdb` 0.6.3** - The wire variant gained `Rotator`/`Quat`/`LinearColor` members plus matching typed array variants; only `WireGraphEnumWrapper` remains unmapped.

## 0.6.0 - 2026-06-30

- **Data regenerated** - Gate inventory (285 -> 288: new `Convert` / `FindPlayer` gates), the brdb component `_max` schema (258 structs), and `component_db.rs`.
- **Sweep upgrade** - The raycast `Sweep(...)` gate gained optional per-channel flags: `detectBricks`, `detectPlayers1`–`detectPlayers4`, `detectPhysics`, `detectMap`, and `ignoreOwningGrid`.

## 0.5.0 - 2026-06-29

### Language Features

- **`quat` type + rotation/quaternion builtins** - A `quat` primitive (distinct from the euler `rotator`) plus `dir.ToRotation()`, `q.ToDirection()`, `v.Rotate(q)`, `q.Invert()`, `from.RotationTo(to)`, `a.AngleTo(b)`, `a.Slerp(b, alpha)`, `axis.RotationByAngle(angle)`, `q.ToAxisAngle()`, and `Rotation(p, y, r)` / `r.ToEuler()` for the euler rotator.
- **sRGB / hex color builtins** - `ColorSRGB(r, g, b, a)` and `ColorHex("#rrggbb")` constructors; `c.ToSRGB()` / `c.ToHex()` / `a.Blend(b, alpha)` receivers.
- **`Cycle(count)` / `Toggle()`** - Stateful exec value gates (advance a counter / flip a bool each exec pulse).
- **User definitions shadow builtins** - A `chip`/`mod`/`fn` named like a builtin (e.g. `chip Toggle`) takes precedence at the call site.
- **Asset references** - `$AssetType/AssetName` (e.g. `$BRItemBase/Weapon_Pistol`) references an external asset embedded by name, encoded as an external-asset-table index on emit. Completion: `$` offers asset types, `$Type/` that type's names (from the brdb catalog).
- **`HasRole` / `GiveWeapon`** - `ctrl.HasRole("Admin") -> bool` (role is a config string); `char.GiveWeapon($BRItemBase/Weapon_Pistol, slot)` sets an inventory slot to an item asset (builds the nested `EntryPlan`). _Binary encoding needs in-game verification._

### Gate Catalog / Output

- **Gate inventory refreshed** - Adds 26 new gate classes; the rotation/quaternion, sRGB-color, and cycle/toggle ones are wired into the language, with component data structs registered for `.brdb` output.

## 0.4.0 - 2026-06-28

### Language Features

- **Pre-initialized arrays** - `array foo: int[] = [1, 2, -3]` writes literal contents (numbers incl. negatives, strings, bools) straight into the array gate, loading pre-populated. A non-literal top-level element is a clear error.
- **Inferred array-typed vars** - `var foo = [1, 2, 3]` infers `int[]` and lowers to the same array gate as an `array` declaration; it indexes and iterates as a real array.
- **Runtime array assignment + spread** - In an exec handler, `foo = [a, 1, ...other, 5]` rebuilds an array var: clear -> push each item -> append each `...spread`. Elements can be any runtime value; a spread splices another array in place.
- **Array methods, one source of truth** - Every method derives from a single `catalog::arrays` table: completion offers the full set on any array-typed value, return types come from gate output ports. `find` returns `{ Index, Found, Value }` (auto-unwraps to Index); `pop`/`min`/`max` expose `.IsEmpty`; `insert`/`swap`/`slice` expose `.OutOfBounds`.
- **`GetAim` replaces `AimOrigin`/`AimDirection`** - A character's camera/aim is one gate returning `char.GetAim().Origin` / `.Direction`; reading both fields shares a single gate. The separate calls are removed.
- **Chat command config** - `on ChatCommand("greet", "Greets the player", player, args)`: string literals fill `CommandName` then `HelpText` in order (or named `Description = "..."`), and bare identifiers still bind the event outputs (`controller`, `arguments`).

### Bug Fixes

- **Vector components on stored values** - `.x`/`.y`/`.z` (and color `.r`/`.g`/`.b`/`.a`) work on a vector held in a variable or `let` binding via the SplitVector/SplitColor gates, not just an inline `Vec(...)`.
- **`Vec(...)` literal arguments** - Constant components are no longer dropped to `0` at emit; `MakeVector` gained its component-data mapping.

### Compiler / Output

- **Gate defaults resolve from component_db** - Unspecified data fields are omitted so the brdb writer fills them from `STRUCT_DEFAULTS`; DisplayText's `FontSize`/`Lifetime` now resolve to the game defaults (`16` / `5`) instead of `0`.

## 0.3.0 - 2026-06-27

### Language Features

- **String variables** - `var`/`static var` of type `string` store in a Variable gate (the WireGraphVariant gained a `str` member). The `WS018` "strings can't be stored in vars" diagnostic is gone.
- **Native string equality** - `==` / `!=` on strings lower directly to the `CompareEqual` / `CompareNotEqual` gates. The `contains(a,b) && length(a) == length(b)` workaround is removed.
- **Vector arithmetic** - `+ - * / %` operate component-wise on two vectors, and a scalar operand (`v * 2.0`, `10.0 * v`, `v / 4`) broadcasts - all on the same `MathAdd`/`Subtract`/`Multiply`/`Divide`/`Modulo` gates. The `Scale` helper still works.
- **Any-variant variables** - A `var` can hold any WireGraphVariant member (`int`, `float`, `bool`, `string`, `vector`, object types); typed vars get a type-matched initial value instead of a number default.
- **Typed arrays** - The declared element type selects the backing `WireGraphArrayVariant` member (`int` -> Int64, `float` -> Double, plus Bool/String/Vector/Object), so elements keep their declared type.

### Gate Catalog

- **Regenerated inventory** - Rebuilt from the in-game dump via a new checked-in generator (`scripts/gen_inventory.mjs`): adds 76 gate classes (ArrayVar exec, Gamemode/Controller/Character, string `ParseInt`/`ParseNumber`, reference gates) and types 86 previously-`any` ports. 175 -> 260 entries.
- **Refreshed brdb component tables** - `component_db.rs` regenerated from the same dump so the new gates emit; the removed `Gamemode_EndRound` gate is gone.

### New Builtins and Methods

- **Array methods** - `insert`, `find`, `sort(desc?)`, `reverse`, `sum`, `min`, `max`, `average`, `swap`, `fill`, `resize`, `append`, `copyFrom`, `slice`, `fillFromPlayers`, `fillFromTeam` join `push`/`pop`/`length`/`remove`/`clear`/`shuffle`. Every ArrayVar gate is reachable.
- **Easing** - `Easing(a, b, blend, fn?, dir?)` and `Tween(target, duration, fn?, dir?)`; function/direction pass as an int or enum-name literal (`"Quad"`, `"InOut"`, ...) resolved against the engine's `EBREasingFunction`/`EBREasingDirection` enums.
- **Timer** - `Timer(limit, restart?, pause?, resume?)` returns `{ Time: float, Expired: exec }`; the controls are optional exec inputs and `Expired` works with `on`/`await`.
- **String parsing** - `ParseInt(s) -> int` and `ParseNumber(s) -> float` (also `s.ParseInt()` / `s.ParseNumber()`).
- **Controller** - `GetUserName`, `GetUserId`, `GetDisplayName`, `IsTrusted`, `HasPermission`, `SetCanRespawn`, `SetTeamPinned`.
- **Character** - `GetDamage`, `SetDamage`, `IncDamage`, `SetTempPermission`.
- **Entity** - `SetFrozen`.
- **Gamemode** - `PlayerWins` / `TeamWins` (replace the removed imperative `EndRound` gate and builtin), `GetCurrentRound`, `SetTeam`, `GetTeamName`, `GetTeamLeaderboardValue` / `SetTeamLeaderboardValue` / `IncrementTeamLeaderboardValue`.
- **Misc** - `PrintToConsole`, `DeltaTime`, `ServerUptime`, `NearlyEqual`, `Dampen`.

### Compiler / Output

- **Prefab output** - Compiled programs emit a Brickadia prefab (`type: "Prefab"` + `Meta/Prefab.json` with brick bounds from the microchip shell) instead of a world, so the `.brz` pastes like a native copied selection (Ctrl+V) with a correct preview.
- **Loads on current builds** - A bundle embeds only the component structs the program uses plus transitive schema deps, written dependency-first - matching game bundles. Replaces the full-catalog embed recent builds reject; real programs stay within the per-schema struct limit.

## 0.2.0
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

## 0.1.0
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

## 0.0.0
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
