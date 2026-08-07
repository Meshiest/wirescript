# Fold gate-semantics certification (dev)

Internal doc for the constant-folding pipeline — how a pure gate becomes
*foldable*, how its behavior is certified against the real game, and the
per-build runbook. Not user-facing (see `docs/wirescript/` for that).

## What folding is

Constant folding evaluates a pure gate with compile-time-constant inputs and
replaces it with a literal. It is **opt-in** (`@fold` module attribute /
`--fold`; `FoldMode` in `src/lower/fold/mod.rs`). Two hard preconditions for a
gate to fold:

1. **Constant inputs** — every operand must be a compile-time constant. A gate
   fed a runtime value (an `entity`, a `var`, an unwired port…) can never fold.
2. **Deterministic, game-matched semantics** — the fold must produce exactly
   what the game would, including its text rendering.

## The model: eval is the law, the probe certifies it

Folding is **not** a table lookup keyed by input value. Instead:

- `src/lower/fold/eval.rs` holds a **hand-written Rust law per gate**
  (`MathAdd => a + b`, `MathSin => a.sin()`, `BitwiseAND => a & b`, …). This is
  what actually computes a folded value.
- `data/gate_semantics.json` is a **certified truth table** of
  `inputs -> output` cases *probed from the running game*. Its job is to
  **verify** the Rust law matches the game and to **gate** which
  `(gate, input-signature)` shapes are allowed to fold at all.

So the table doesn't drive folding — it audits it. Two independent checks keep
`eval.rs` honest:

- **Rust replay** (`eval.rs::replay_every_certified_case`): every certified case
  is fed through `eval()` and its rendered result compared to the table.
- **In-game verifier** (`probes/verify_semantics.ws`): generated from the table,
  pasted into the game, asserts the table still matches the live gates
  (`VERIFY N/N`).

### The coverage gate

`eval()` first calls `CertifiedTable::certified().covers(gate_class, &sig)` and
returns `None` (refuse) for any gate/signature the probe never observed. So an
uncertified gate — or an un-probed edge case (a domain error, a shift ≥ 64, a
NaN operand) — is a **barrier**: it never folds, regardless of the Rust law
below it. This is why a freshly-added `eval` law stays inert until the gate's
cases land in the table via a re-probe.

## The pieces

| File | Role |
|------|------|
| `src/lower/fold/eval.rs` | Rust fold law per gate + `covers()` gate + render laws + the replay test |
| `src/lower/fold/table.rs` | Loads `gate_semantics.json`; `loads_real_table_v<N>` pins gate/render counts |
| `data/gate_semantics.json` | Certified truth table — **generated only**, never hand-edited |
| `probes/gate_semantics.ws` | The probe: paste in-game, prints one `CASE …` line per interaction |
| `probes/verify_semantics.ws` | Generated verifier: paste in-game, asserts table == game |
| `scripts/gen_semantics.mjs` | probe console dump → `gate_semantics.json` (the only writer) |
| `scripts/gen_verifier.mjs` | `gate_semantics.json` → `verify_semantics.ws` |
| `scripts/test/*.test.mjs` | Node tests for both generators |

### Why the probe uses `Opaque()`

`Opaque(x)` is an intentionally-untyped probe value that **defeats folding**, so
the gate actually runs *in-game* instead of being folded away by the compiler.
Every probe operand is `Opaque`-armored (`sin(Opaque(1.0))`,
`Opaque(12) & Opaque(10)`) — the console line then prints the gate's real output,
which is the certified truth. The probe self-runs on paste (`ReadBrickGrid`
fires) and prints `BEGIN <chapter> <count>` / `CASE …` / `END <chapter>` blocks;
the `<count>` is checked by `gen_semantics.mjs`.

## Per-build runbook (re-certify)

When a build ships and gate behavior may have changed:

1. Paste `probes/gate_semantics.ws` into a **local** world. Copy the
   `[Wire Graph] PROBE … CASE …` console dump into e.g. `dumps/<something>.txt`.
2. `node crates/wirescript/scripts/gen_semantics.mjs <dump.txt>` — regenerates
   `data/gate_semantics.json` (the *only* writer; hand edits are rejected by
   convention). It validates chapter counts and the probe version.
3. `node crates/wirescript/scripts/gen_verifier.mjs` — regenerates
   `probes/verify_semantics.ws` from the table.
4. Paste `verify_semantics.ws` into the game → confirm `VERIFY N/N`. A `FAIL`
   line means the Rust law disagrees with the live gate — fix `eval.rs`.
5. `cargo test -p wirescript` — the Rust replay gate rechecks eval == table. If
   the case matrix changed, update the hard-coded tallies in
   `replay_every_certified_case` and the counts in `loads_real_table_v<N>`.

## Adding a new foldable gate (the coupled change)

A new foldable gate is a **four-layer** change that must land together, plus a
version bump. Using `MathSin` as the example:

1. **`eval.rs`** — a Rust law (`"MathSin" => as_float(a)?.sin()`), a dispatch
   arm in `eval()`, and a direct unit test (the law is dead behind `covers()`
   until re-probed, so unit-test it directly).
2. **`probes/gate_semantics.ws`** — a `CASE MathSin float:1.0 -> ${sin(Opaque(1.0))}`
   in a chapter, with **clean, finite, in-domain** constant inputs so no probed
   case hits an edge the law would mishandle (domain error, shift ≥ 64, a
   round-half tie). Bump `let VERSION`.
3. **`scripts/gen_semantics.mjs`** — a `GATE_CLASS` entry (short name → full
   class string). Bump `EXPECTED_PROBE_VERSION` to match the probe. Add to
   `FLOAT_OUTPUT` if the gate is float-typed but can render as a bare integer
   (`min(3,7)->3`, `pow(2,10)->1,024`) — otherwise it'd be misclassified `int`.
4. **`scripts/gen_verifier.mjs`** — how to *spell* the gate in an assertion:
   `BIN_OP` (infix op, `&`), `CALL_FN` (builtin, `sin`), or `UNARY_OP` (prefix,
   `~`/`-`). Add float gates to `FLOAT_RESULT_SHORT_NAMES` so they assert by
   **rendered text**, not value-EQ (see below).

Then re-probe and run the pipeline as above.

## Gotchas

- **Multi-output / record gates are not supported.** `Split*` (3–4 output
  ports) and `FormatDate` (`{ Output, Success }`) don't fit the single
  `CASE … -> value` format or the fold `Value` type. Supporting them needs a
  format + eval extension first.
- **Float rendering must match FormatText exactly.** `render_for_format` (and
  `render`, its replay twin) round to 3 decimals, drop trailing zeros, and
  **group thousands** — `pow(2,10)` renders `1,024`, not `1024`. A float law that
  gets the grouping wrong fails the replay. (This bit us once: `render`'s float
  arm was missing `group_thousands` — no prior float case reached ≥ 1000 to
  expose it until `MathPow`.)
- **Value-EQ vs rendered-text assert.** Exact results (int, exact-binary floats
  like `0.5+0.25`) can assert by value equality. A *rounded* float (`sin(1.0)`
  stored as `0.841`) cannot — the live value is full precision. Those gates must
  assert by rendered-text (`FLOAT_RESULT_SHORT_NAMES` / the
  `COMPOSITE_CHAPTER_FLOAT_SHORT_NAMES` set) in the verifier, and the Rust replay
  compares `render(v)` to the stored text.
- **Non-finite results refuse.** A law producing `NaN`/`inf` returns `None`;
  `fold/mod.rs` also guards this before baking. Keep such inputs unprobed.
- **The table is generated, singular, and hand-edit-free.** Only
  `gen_semantics.mjs` writes it. Everything downstream (the verifier, the Rust
  replay) trusts it, so a hand edit that isn't reproducible from a probe dump is
  a silent divergence.

## Console precision (accepted)

The probe reads gate outputs through the game's **console log**, i.e. through
`FormatText`: discrete outputs (int / bool / string) are exact and asserted by
literal `==` in the verifier, while floats are rounded to **3 decimals** and
asserted by rendered-text equality (composite vector/rotator/color/quat outputs
render blank and are allowlisted refusals / transitively certified). So a folded
float is certified to 3 decimals, not full precision — a low-bit divergence
between the game's math and Rust's below the 3rd decimal is not caught.

This is **accepted**. A full-precision path — wiring outputs into labeled
Variable gates, saving, and reading exact bits back out of the `.brz`/`.brdb`
via the `brdb` crate — was considered and set aside as more machinery than the
gain warrants: 3-decimal float agreement plus exact discrete agreement is
enough for the constants folding bakes.
