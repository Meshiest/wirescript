# Constant Folding

The compiler constant-folds pure gates before layout, guarded by an in-game-certified
semantics table so nothing folds on a guess. Folding sees through chip boundaries and
iterates to a fixpoint -- a fold can unlock another fold, so a chain of constant math
or nested conditionals collapses in a single compile.

## Enabling folding

Folding is **on by default** -- every compile folds unless you opt out. An unannotated
program folds:

```wirescript
let x = 2 + 3 * 4          // folds to the literal 14
```

Turn it off for a whole program with a module-level `@nofold`: on the first line of the
entry file (after any module doc), separated from the first declaration by a blank line.

```wirescript
@nofold

let x = 2 + 3 * 4          // stays as gates; nothing folds or is elided
```

- `--no-fold` on the CLI's `compile` command disables folding for that compile,
  equivalent to a module-level `@nofold`.
- `--fold`, and a module-level `@fold`, still parse but are redundant now that folding
  is the default. They never override a `@nofold`.
- If both a module-level `@fold` and `@nofold` are present, `@nofold` wins and the
  parser warns that the annotations conflict.
- A `@nofold` in an *imported* file has no effect -- only the entry file's module-level
  annotation is consulted.

## What folds

**Value folding.** A gate whose class and input-variant combination is certified in
the semantics table, with every live input a known constant (int/float/bool/string),
is replaced by a literal carrying the computed result. Unwired inputs count as known
-- their certified variant default (`0`, `0.0`, `false`, `""`).

```wirescript
let x = 2 + 3 * 4          // folds to the literal 14
```

**Constant-selector `Select`.** An `if`-expression compiles to a `Select` gate; when
its condition folds to a known `bool`, the gate is removed and consumers rewire
directly to the chosen arm's source -- even when that arm is not itself a constant.

```wirescript
let y = if true then f() else g()    // folds to f()'s output; g() is dropped
```

**Dead exec-branch truncation.** An `if` statement compiles to a `Branch` gate; when
its condition folds to a known `bool`, the branch is removed and incoming exec wires
rewire straight to the taken side. This happens across chip boundaries too -- a
constant fed into a chip's input can truncate a branch inside it. Anything on the dead
side still exec-reachable from elsewhere survives; a follow-up sweep also removes pure
gates left feeding only the deleted branch.

```wirescript
if false { heavy() }       // heavy()'s whole exec chain is dropped
```

**Annihilators.** `&&` with either side certified `false` folds to `false`; `||` with
either side certified `true` folds to `true` -- even when the other side is unknown at
compile time. This is the only case where a non-constant operand still lets a gate
fold, and it draws solely from the table's whitelisted rules (nothing derived).

## Constant inlining into gate data (always on)

Independent of the fold pass (it runs even under `@nofold`): a **bare** constant wired
into a gate's data field whose type matches the constant's type is written straight into that field and the
wire dropped — no separate carrier gate. This runs before the fold pass and only
on source literals (never fold-produced ones), so folding stays a structural
no-op.

- A constant gate *setting* bakes in: `on Clock(interval = 0.25)` stores `0.25`
  in the Clock's `IntervalSeconds` data, and a constant `Sweep` distance,
  `DisplayText` `fontSize`, etc. do the same. A **variable** value still wires
  (so the setting stays dynamic).
- A matching-type constant *operand* inlines too: the `0` in `x | 0`, a `false`
  operand of `||`, and string literals into `str` fields (`Contains`'s search
  text, an interpolation template) all bake into the gate instead of spawning a
  throwaway carrier gate.
- A **type mismatch** (e.g. a float constant into an `i64` field) does *not*
  inline — it keeps a converting carrier gate. Wire-variant math inputs
  (`+`/`-`/`*`) and `Vector`/`Rotator`/`Quat`/`Color` fields inline as wire
  variants instead.

## Strings and composite values (wave 2)

Folding now also covers `string` operators/methods and `vector`/`rotator`/`color`/`quat`
math and constructors -- certified the same way as everything else: an in-game probe
records the real output, and only a (gate class, input-variant) pair the probe actually
observed is eligible to fold.

**Strings.** `..` concatenation, `Length`/`Contains`/`StartsWith`/`EndsWith`/`ToLower`/
`ToUpper`/`Trim`/`Substring`/`Find`/`Replace`/`ParseInt`/`ParseNumber`, and
`${...}`-interpolated templates (`FormatText`) all fold when every operand is a known
constant.

```wirescript
let s = "hello".ToUpper() .. "!"     // folds to the literal "HELLO!"
let n = "  42  ".Trim().Length()     // folds to the literal 2
```

Interpolation folding is held to a **render-exactness guarantee**: the folded literal
text must be byte-identical to what `FormatText` would print in-game for the same
values, not just numerically equivalent. The certified render law: ints comma-group
every 3 digits from `1,000` up; floats round to 3 decimals (ties to even), comma-group
their integer part, and trim trailing fractional zeros (and a bare trailing `.`); bools
print `1`/`0`; vectors print `X=%.3f Y=%.3f Z=%.3f`. `..` concatenation's own operand
stringification differs on one point -- a bool operand prints `true`/`false`, not `1`/`0`
-- matching the game's generic to-string conversion there instead of `FormatText`'s.

**Vector/rotation/color/quaternion math.** `Vec(...)` construction, component-wise and
scalar-broadcast `+ - * /` on vectors, `.Scale()`/`.Dot()`/`.Cross()`, and `Quat(...)`
construction all fold when every component is known.

```wirescript
let v = Vec(1.0, 2.0, 0.0) + Vec(0.0, 0.0, 3.0) * 2.0   // folds to Vec(1.0, 2.0, 6.0)
```

`Quat(...)` and the 3-argument (RGB, no alpha) form of `Color(...)` fold too, but their
own certification is **transitive**, not direct: a quaternion or an RGB color never
renders through `FormatText` (its probe case prints blank), so nothing directly proves
the constructor packs its fields correctly. Instead, each is certified through a
*different*, value-bearing gate that consumes it and produces an observable result --
`RotateVector` and a quaternion dot product for `Quat(...)`, hex-string conversion for
3-argument `Color(...)`. A wrong field order or a transposed component would visibly
diverge those gates' certified outputs, so their exact replay certifies the constructor
by proxy. The 4-argument (alpha-carrying) form of `Color(...)` has no such transitive
proof -- alpha never survives into any value-bearing consumer the probe covers -- and
does not fold.

**Refusals specific to this family:**

- Any **non-ASCII** string operand or result -- the certified behavior was only ever
  probed with ASCII text, so it never folds regardless of which model (character count
  vs. UTF-16 code units) the game actually uses.
- A string result longer than **8,192 characters**.
- Any float operand whose magnitude exceeds **`1e15`** in a string/interpolation
  context -- the game cannot print a float that large at all (the console line is
  silently dropped), so folding refuses past that bound rather than bake unreproducible
  text. `1e15` itself is certified to print fine; the bound is exclusive.
- A handful of composite-constructor gates whose *only* table evidence is a blank
  render and that have no transitive proof either (unlike `Quat(...)`/3-argument
  `Color(...)` above): a bare rotator constructor, the sRGB-byte and hex-string color
  constructors, and rotation inversion. These hard-refuse regardless of how determined
  their inputs are, until a future probe wave chains them through a value-bearing gate.
- A fixed list of gates the probe recorded but deliberately never folds: vector
  magnitude/normalize/distance, spherical interpolation, axis-angle construction,
  angle- and rotation-between queries, direction/rotation conversions, and color
  blending. Their math is meaningfully harder to reproduce exactly (square roots,
  trig, arbitrary-axis construction) and wasn't certified for folding.

Per-build recertification is unchanged from the workflow below -- these families ride
the same probe/table/replay/verifier loop as everything else.

## What never folds (barriers)

These are always treated as unknown, and folding never elides or sees through them:

- `Opaque(x)` -- the permanent, explicit fold barrier.
- `@nofold`-annotated declarations, including a module-level `@nofold` at the top of a
  file, which disables the whole fold pass for that compile.
- Rerouters, `var`/`Var_Get` reads, arrays, buffers, events, `ReadBrickGrid()`, and any
  wire carrying an object-typed value.
- Any gate class or input-variant combination the probe never certified — absent from
  the table means permanently unknown, never folded.
- Certified signatures whose specific VALUES the evaluator declines to compute, as a
  safety net layered on top of coverage: math involving a `string` operand (the
  recorded observations rule out every parsing model), integer overflow, mixed-sign
  division/modulo with a nonzero remainder (truncation direction was never probed),
  and any result that would be non-finite. These are refused rather than guessed at,
  even when every input is constant.

## The certification story

Every fold decision traces back to `data/gate_semantics.json`, a table built by
probing each gate combination in-game and recording its real output. Two things hold
the compiler to that table:

- **Replay gate** -- a build-time test feeds every table case through the compiler's
  evaluator and asserts the recorded output; the table cannot drift from what the
  compiler does without breaking the build.
- **Coverage gate** -- a gate only folds when its (gate class, input-variant
  signature) pair is present in the table. A combination the probe never ran stays
  unknown, even if the evaluator could compute an answer for it.

A companion **probe invariant** test compiles the certification circuits themselves
with folding on and off and asserts identical gate and wire counts -- proof the pass
cannot touch the instruments that certify it.

When a game build changes gate behavior: re-run the probe, regenerate the table, and
re-paste the generated verifier in-game. The replay gate then re-certifies (or fails
to) the evaluator against the fresh table automatically, so a stale assumption fails
the build instead of silently folding wrong.

## Disabling the pass

- A module-level `@nofold`, placed at the top of the entry file and separated from the
  first declaration by a blank line, turns folding off for the whole program -- and
  always wins over a module-level `@fold`.
- `--no-fold` on the CLI's `compile` command has the same effect for that compile.
- Folding is on by default everywhere it runs -- CLI compile, the LSP, and the wasm
  build -- so all tools fold consistently unless a program opts out with `@nofold`.

## Guarantee

A gate is only value-folded when its class and input-variant signature are certified,
and only using a law the evaluator implements for it; anything else is left as a real
gate. `Opaque(...)`, `@nofold`, rerouters, variables, arrays, buffers, events,
`ReadBrickGrid()`, and object-carrying wires are never folded, elided, or seen
through. Nothing is removed that the certified table does not license.

## See also

- [Statements](statements.md) -- `@nofold` syntax and placement rules
- [Builtin Functions](builtins.md) -- `Opaque`
