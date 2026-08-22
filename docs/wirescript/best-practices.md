# Best Practices: Gate Count & Scaling

Wirescript makes it easy to write logic that reads like a normal program but compiles
into an enormous number of gates. The patterns below came out of shrinking a real game
circuit from roughly **300,000 gates to about 8,000** -- the logic was unchanged, only
its shape.

Everything here follows from one fact, so start there.

## The one thing to internalize: every call site is a copy

A **`mod` is inlined**. Its entire body is copy-pasted into the caller's grid at every
call site, and that expansion is **transitive** -- everything the `mod` reaches is
copied too.

```wirescript
mod heavy(x: int) { /* 500 gates of logic */ }

mod a() { heavy(1) }
mod b() { heavy(2) }
mod c() { heavy(3) }
// heavy is now built THREE times: 1500 gates
```

**A `chip` does not fix this.** A chip is not a shared subroutine you jump into -- each
call builds a **new instance**. It emits *the same gates a `mod` would*, plus an
input/output rerouter per boundary port and the microchip container itself. Chips can be
pure (no exec involved at all); they are a structural and visual boundary, not a
deduplication mechanism.

So three calls cost three copies either way:

```wirescript
chip F(n: int) -> (y: int) { out y = n * 2 + 1 }
let c1 = F(a)
let c2 = F(a)
let c3 = F(a)
// Three F instances. Same six logic gates the mod version emits,
// plus 3 input rerouters + 3 output rerouters + 3 microchip containers.
```

| | `mod` | `chip` |
|---|---|---|
| Compiles to | Inline gates in the caller's grid | The same gates, in a microchip instance |
| N call sites | N copies of the whole subtree | N copies of the whole subtree |
| Extra gates | None | One rerouter per boundary port, plus the container |
| Pure (no exec) | Yes | Yes |
| Named multi-outputs | Via a returned record | Yes (`-> (a: int, b: int)`) |
| `ref`/`*` params | Yes | Yes |

**Constant arguments are free.** `F(1)` folds the `1` into the instance itself and drops
the input pin it would have crossed, so the constant lands as inline gate data on whatever
consumes it -- exactly what the `mod` version does. Arguments that are already a wire (an
input, a var, another gate's output) cross the boundary through a rerouter as usual.

Captured outer variables work normally through the boundary -- a chip that writes an outer
`var` wires to that one real variable gate, it does not get a private copy per instance.

**Choose between them on organization, never on gate count.** A `chip` buys you a visible
microchip boundary in-game and named outputs; a `mod` keeps the gates in the parent grid.
Both support `ref`/`*` params, and a ref crosses a chip boundary as a direct wire to the
one real variable gate. Neither one shares logic between call sites.

That means there is no keyword that rescues you from a gate explosion. The only lever is
**reducing the number of call sites** -- which is what the rest of this page is about.

## The call-site multiplier

The damage is multiplicative, not additive. If some heavy shared subsystem is reachable
from `N` call sites, you pay for it `N` times:

```wirescript
// 10 slots x 3 inputs = 30 call sites, each inlining the ENTIRE state machine
mod onInput(slot: int, code: int) { /* whole phase machine */ }

if (mask & BIT_0) { if a0 { onInput(0, 0) } if b0 { onInput(0, 1) } if c0 { onInput(0, 2) } }
if (mask & BIT_1) { if a1 { onInput(1, 0) } if b1 { onInput(1, 1) } if c1 { onInput(1, 2) } }
// ... x10
```

That single shape is what produced the 300k-gate build. The fixes below, in order of
impact:

## 1. Funnel many producers through ONE dispatch site

Do not call the logic from every producer. Have producers push a small encoded integer
into a queue, and dequeue **one per tick** at a single call site. The state machine then
inlines exactly once.

```wirescript
var queue: int[]

// Producers are now trivial -- they inline almost nothing.
mod enqueue(slot: int, code: int) {
  if queue.length() < 32 {
    queue.push(phase * 64 + slot * 4 + code) // pack: phase | slot | code
  }
}

on tick {
  // THE only dispatch site: everything downstream is built once.
  if queue.length() > 0 {
    let ev = queue[0]
    queue.remove(0)
    if ev / 64 == phase {            // stale-intent guard, see below
      handle((ev % 64) / 4, ev % 4)
    }
  }
}
```

Two details that matter:

- **Tag events with the phase at enqueue and drop mismatches at dequeue.** An input
  queued during one phase must not execute a tick later in the next one.
- **Cap the queue** (`length() < 32`) so a burst can't grow it without bound. One event
  per tick at 60 Hz drains fast enough for human input.

## 2. Merge per-variant mods into one parameterized mod

Three near-identical entry points each inline their whole downstream tree:

```wirescript
// Before: 3 call trees
mod onA(slot: int) { /* ... */ }
mod onB(slot: int) { /* ... */ }
mod onC(slot: int) { /* ... */ }
```

Collapse them into one and make the variant a *computed argument*, so each downstream
mod is instantiated once instead of two or three times:

```wirescript
// After: 1 call tree; the variant is data, not a separate code path
mod onInput(slot: int, code: int) {
  if phase == PHASE_PICK {
    // the per-variant difference becomes an argument, not another call site
    pick(if code == CODE_A then -1 else 1)
    return
  }
  // ...
}
```

## 3. Defer hot shared work behind a flag

If a heavy shared routine is called from many mutation sites, each site inlines it.
Instead, set a boolean and make **one** real call per tick:

```wirescript
var dirty: bool = false

// 18 different mutation sites do only this:
dirty = true

on tick {
  if dirty {
    dirty = false
    refresh()      // built ONCE
  }
}
```

This also removes a class of bug: the deferred call runs after the exec chain settles,
so consumers never observe mid-update state.

> **Ordering caveat:** if you defer more than one thing, run them in the order the state
> machine requires. A deferred *advance* should typically run **before** a queued-event
> dequeue, so an event queued for the old state doesn't re-trigger the thing that just
> advanced.

## 4. Bitmasks instead of per-slot arrays

Every `arr[i]` compiles to an array-get gate, and array reads are exec-only. Per-slot
*boolean* state is far cheaper as a single integer bitmask.

```wirescript
// Instead of: array flagged: bool[]   (an array-get per read, per slot)
var flagged: int = 0                   // bit i = slot i

flagged = flagged | (1 << i)           // set
flagged = flagged & ~(1 << i)          // clear
if (flagged & (1 << i)) { /* ... */ }  // test -- already truthy, no `!= 0` needed
let n = BitCount(flagged)              // popcount builtin, not a 10-way sum
```

This was the single biggest late win. It compounds:

- **Derived sets are free:** `BitCount(active & ~disabled)` replaces a loop-and-count.
- **Two masks beat a tri-state array:** store `votedMask` and `yesMask` rather than an
  array of `-1/0/1`; "voted no" is `votedMask & ~yesMask`.
- **Pass masks (plain `int`) to pure helpers** instead of arrays -- helpers stay pure and
  cheap to inline.
- **Bit outputs drive hardware directly.** If an output expects one bit per slot, publish
  the mask itself; no pack loop needed.
- **Entity-ish ports coerce to 0/1 in arithmetic**, so `a0 + a1 + a2 + ...` is a cheap
  *pure* occupancy count with no array and no exec.

## 5. Resolve once, pass down

Re-deriving the same handle inside a callee means re-deriving it in *every inlined copy*.
Resolve it once at the top and pass it as a parameter:

```wirescript
// Before: each callee re-derives the same thing
mod draw(i: int) { let e = lookup(i)  /* ... */ }
mod tag(i: int)  { let e = lookup(i)  /* ... */ }

// After: derived once, handed down
mod service(i: int) {
  let e = lookup(i)
  draw(i, e)
  tag(i, e)
}
```

A free running counter (`buffer tick`) also makes a good round-robin cursor -- `tick % 10`
services one slot per tick instead of building ten service chains.

## 6. Prefer pure `let` chains over exec ladders

A predicate written as an early-return ladder becomes exec gates; the same predicate
written as boolean `let`s stays pure:

```wirescript
// Prefer
mod allowed(i: int, mask: int, blocked: int) -> bool {
  let live = (mask & (1 << i)) != 0
  let free = (blocked & (1 << i)) == 0
  return live && free
}
```

## 7. Reduce with a native array aggregate, not an unrolled fold

`arr.sum()`, `arr.max()`, `arr.min()`, and `arr.average()` are **one gate each**. An
unrolled max/sum over `N` slots is `N` compares/adds plus the accumulator plumbing.

```wirescript
// Before: an N-way unrolled scan
mod maxTotal(t: int[]) -> int {
  var m = t[0]
  if t[1] > m { m = t[1] }   // ... repeated per slot
  return m
}

// After: one gate
let over = totals.max().Value >= 100
```

- **Gotcha:** `.max()` / `.min()` return a record `{ IsEmpty: bool, Value: int }` -- read
  `.Value` (and check `IsEmpty` when the array can be empty). `.sum()` / `.average()`
  return a bare scalar.
- If the slots you want to *exclude* already hold the identity value (`0` for a sum, or a
  value below every real one for a max), just reduce the whole array -- no masking needed.

## 8. Keep a derived array to unlock an aggregate

To reduce a *computed* value over a sub-range, don't decode-and-add per element every time.
Maintain a parallel array holding the precomputed per-element value in lockstep with the
source, then `slice` the window into a scratch array and `sum()` it -- 2 gates instead of
~`N`.

```wirescript
var packed: int[]   // source (e.g. state+value packed per element)
var vals: int[]     // derived mirror: the summable value per element
var scratch: int[]  // reusable slice target

// keep the mirror in lockstep at EVERY write to `packed`
mod setCell(i: int, v: int) { packed[i] = encode(v)  vals[i] = v }

// reduce a 12-wide window in 2 gates, not 12 decodes + 11 adds
mod windowSum(base: int) -> int {
  scratch.slice(vals, base, 12)   // slice REPLACES scratch with vals[base..base+12]
  return scratch.sum()
}
```

The cost is one extra write per source mutation; it pays back at every reduction -- and
reductions are usually called from many slots. Audit that *every* write to the source has a
paired write to the mirror, or the aggregate silently drifts.

## 9. Derive in pure output bindings, not on the exec chain

Array reads are exec-only, so an `on <update>` handler that reads a buffer and *also*
computes display values inlines the whole decode/format on the exec chain (a `Union` /
`Branch` / `Var_Set` per step) and needs a cached-result var per output. Cache only the raw
read; derive everything else in the **pure** output binding.

```wirescript
// Before: decode + format run on the exec chain, cached into a result var
var vC: color = ...
on update { vC = colorOf(buf[0]) }
out color0: color = vC.Value

// After: cache only the raw cell; the binding derives it, pure
var vP0: int = 0
on update { vP0 = buf[0] }                // the only exec-only step (the array read)
out color0: color = colorOf(vP0.Value)    // pure: no exec chain, no result var
```

Requirement: a mod called from a pure binding must be **expression-`if`** (a single
`return if ... then ... else ...`), not a statement-`if` with early `return`s.

## 10. Per-entity fan-out: one chip each, not one loop over all

A loop advances one iteration per tick. So a central chip that sweeps a roster
of N entities every tick does not cost "a loop" -- it costs N ticks per pass,
and it gets slower exactly as the thing succeeds and N grows. Anything the
player perceives as continuous (a ticking timer, a follow camera, a per-player
HUD) is unusable built that way.

Give each entity its own chip instance instead. The per-tick work then happens
in parallel gate instances, one per entity, and the wall-clock cost stops
depending on N:

```wirescript
var minions: Map<character, entity>

on CharacterSpawned() -> (who) {
  let e = SpawnPrefab(prefab = $./minion.ws, lifetime = 0.0, limit = 64)
  minions.set(who, e)
  owner = who              // a var: a constant argument emits no wire
  e.SendCustomEvent("minion.init", owner)
}
```

and in `minion.ws`, the per-tick handler nests inside the init handler so it
closes over a reference that is set:

```wirescript
var owner: character
var ready: bool = false

on CustomEvent("minion.init") -> (who: character) {
  owner = who
  ready = true
  on ServerUptime() {
    if ready {
      // per-tick work for exactly this one entity
      if owner.GetUserId() == "" { ReadBrickGrid().DestroySpawnedPrefab() }
    }
  }
}
```

Three things that bite:

- **The nested handler is not registered by the outer one.** The wire graph is
  static, so its trigger is live from the moment the prefab spawns, a tick or
  more before the init event lands. It needs a guard on an init-set value.
  Nesting scopes the reference readably; it does not sequence the two.
- **Each instance should own only its own state.** Shared state stays in the
  central chip, which stays event-driven. The moment an instance needs to read
  another's data you have rebuilt the roster sweep by mail.
- **Instances must clean themselves up.** A spawned chip whose subject is gone
  keeps running; check for that in the tick handler and
  `DestroySpawnedPrefab()`.

Keep the *bookkeeping* central and event-driven, and push only the per-tick
rendering or sampling out to the instances.

## What actually costs anything: measured

Numbers below were measured in game, not reasoned. They matter because most
optimisation instinct here aims at the wrong target.

### Almost nothing costs what you think

At **512 operations per tick** against a 4.2ms frame, only one thing moved the
tick time at all:

| Operation | Cost per op |
|---|---|
| Building a string (`"a" .. pad(x) .. "b"`) | **~1us** |
| Reading a var | free |
| `Map.get` + `Found` check | free |
| Reading a pre-built string from a map | free |
| `DisplayText` call | free |

**512 `DisplayText` calls per tick cost 3us.** If a HUD redraws twenty elements
at 10Hz, drawing is not your problem and rate-limiting it will not help.

**Map reads are free and O(1)** -- a 1024-entry map reads no slower than an
8-entry one. A permanent record store can grow forever without pruning, and
keying per-entity state by `character` or account id costs nothing at runtime.

So: **the only operation worth caching is a built string.** If the same text is
rebuilt every frame and only occasionally changes, store it in a map and read it
back. Everything else is noise -- and a cache with one reader is a net loss.

### Gates and ticks are one currency

There is no construction that gives concurrency from a single body:

| Construction | Bodies (gates) | Ticks for N items |
|---|---|---|
| Unrolled calls | N | **1** |
| Back-edge loop (`await`) | 1 | N |
| Self-event fan-out | 1 | N |
| One send to N distinct receivers | N | **1** |

**Firing an event N times gives one invocation per tick**, at any tick rate --
measured at 240Hz and 60Hz, and the tick COUNT was identical. Fan-out is a loop
with nicer ergonomics, not a way to parallelise.

But **one send reaching many receivers costs one tick total.** Send count is
expensive; receiver count is free. So broadcast one packed payload and let each
receiver filter, rather than tailoring a message per receiver.

Practical rule: **unroll hot paths** (N gates buys one tick), **loop or fan out
cold ones** (one gate body, and nothing is waiting).

### Where gates actually go

- **A `mod` inlines per call site**, so its cost is roughly `body x call sites`.
  That is an upper bound -- the compiler shares loop-invariant subexpressions
  across the copies, so hand-hoisting a repeated read out of an unrolled mod
  typically buys very little.
- **A `mod`'s LOCAL `var` becomes a storage gate at every call site.** Three
  locals in a helper called from a 30-wide unroll is ninety gates. Hoisting them
  to module scope collapses that to three -- but only do it when every call site
  is alone in its statement, because module scratch is shared and two calls in
  one expression will race.
- **A `mod` with a return type costs a storage gate per call site** for the
  returned value. Unavoidable short of writing to a module var instead.
- **Storage survives everything.** Guarding a feature behind a false constant
  removes its logic but not its `var` declarations. Nothing prunes unused state,
  and the compiler never warns about it.

### Feature flags fold completely

A body guarded by a compile-time-false constant is **deleted, not skipped**:

```wirescript
const FEATURE = false
on tick { if FEATURE { expensiveThing() } }   // compiles to nothing
```

Measured: a 1056-gate body behind `const FEATURE = false` compiled to 3 gates.
Shipping two builds of a chip -- one with a feature, one without -- is therefore
close to free, and cuts both gate count and artifact size for anyone who does
not need it.

### Size is usually the real constraint

Runtime headroom is large; artifact size is not.

- **Every `.brz` has a ~25KB floor**, whatever it contains. A 10-gate helper
  chip and a 500-gate chip cost about the same. Splitting work across many small
  chips pays that floor every time.
- **A `SpawnPrefab` reference embeds the whole prefab** into the spawning chip.
  Shrinking the prefab shrinks every chip that spawns it, and each spawner
  carries its own copy.
- **On-screen text is capped at roughly 32 elements per player**, shared across
  every circuit that player has loaded -- not per chip. Past the cap, one element
  silently never renders, and which one shifts as you change unrelated things.
  Budget elements, not draw calls.

### Before optimising, check you have a problem

Compute what your circuit actually does per second and compare it to the numbers
above. A full 30-player HUD system doing ~8,000 string builds per second spends
under 1% of wall clock on the only operation that costs anything.

Gate count still matters for **paste size, world load and the text-element
budget**. It mostly does not matter for per-tick CPU. Optimise for the one that
is actually binding.

## Profiling: find the hot spots

`--dump-ir` prints, to **stderr**, a node count per module and every gate with its
`@ line:col` source anchor. Measure before you refactor -- attack the dominant gate kind or
source line, not a guess.

```bash
# per-module node counts + the full node list (IR is on stderr)
cargo run -p wirescript-cli -- compile foo.ws --dump-ir 2>&1 1>/dev/null

# total node (gate) count
... 2>&1 1>/dev/null | grep -cE '^\s*\[(Input|Output|Gate)\]'

# which gate KINDS dominate
... 2>&1 1>/dev/null | grep -oE 'BrickComponentType[A-Za-z_]+' | sort | uniq -c | sort -rn | head

# which SOURCE LINES emit the most gates (the @ line anchor)
... 2>&1 1>/dev/null | grep -oE '@ [0-9]+:' | grep -oE '[0-9]+' | sort -n | uniq -c | sort -rn | head
```

A dominant `MathModulo` / `MathDivide` count usually means a decode called too often; a
large `ArrayVar_Get` count means reads that could be cached or aggregated; a single source
line with a big share is your first refactor target.

## Gotchas worth knowing

- **An expression-`if` is a `Select` gate, so BOTH arms evaluate.** Guard
  possibly-out-of-bounds array reads with a statement-`if`, never a ternary.
- **Delete dead arithmetic.** If a value's range makes an op a no-op, drop it: `(x / 16) % 4`
  where `x / 16` is already proven `<= 2` is just `x / 16`; a mask that can never clear a
  set bit is nothing. Every op you remove is a gate you don't build.
- **Don't recompute inside one expression.** Bind a repeated (possibly expensive) call to a
  `let` once and reuse it -- each call is its own gate subtree, so `f(x) + f(x)` builds `f`
  twice.
- **A `mod`-local `static var` is per-copy, not shared** -- each inlined instance gets its
  own. Hoist shared state to a root `var`.
- **Hover a `mod`, `chip`, `on`, or `if` for its gate count.** The estimate covers the
  whole construct (following its calls), and a `mod` reports that it is inlined per call
  site -- so you can see what a refactor costs without compiling.
- **Don't optimize prematurely.** Gate count only matters once something is instantiated
  many times. A leaf helper called twice is fine as a `mod`.

## Checklist

When a build is unexpectedly huge, walk this list:

0. Profile with `--dump-ir` first -- refactor the dominant gate kind / source line, not a guess.
1. How many call sites reach the biggest `mod`? Multiply -- that's your bill.
2. Can many producers be funneled through one queued dispatch site?
3. Can near-identical entry points collapse into one parameterized `mod`?
4. Can a hot shared routine be deferred behind a dirty flag to one call per tick?
5. Is any per-slot boolean state an array that should be a bitmask?
6. Is anything being re-derived inside a callee that could be passed in?
7. Is an unrolled max/sum/count really a one-gate `.max()` / `.sum()` / `.average()`?
8. Would a derived parallel array turn a per-element decode-and-add into a `slice` + `sum`?
9. Is display/derived logic running on the exec chain when it could be a pure output binding?
10. Is a per-tick roster sweep really one chip instance per entity?

## See also

- [Chips](chips.md) -- `mod` vs `chip` semantics, `ref`/`*` params, nested chips
- [Execution Context](exec-context.md) -- pure vs exec, and why array reads need exec
- [Builtin Functions](builtins.md) -- `BitCount` and the other cheap primitives
