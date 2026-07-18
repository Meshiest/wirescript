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
| `ref`/`*` params | Yes | Allowed, but they force it to inline (it stops being a microchip) |

**Constant arguments are free.** `F(1)` folds the `1` into the instance itself and drops
the input pin it would have crossed, so the constant lands as inline gate data on whatever
consumes it -- exactly what the `mod` version does. Arguments that are already a wire (an
input, a var, another gate's output) cross the boundary through a rerouter as usual.

Captured outer variables work normally through the boundary -- a chip that writes an outer
`var` wires to that one real variable gate, it does not get a private copy per instance.

**Choose between them on organization, never on gate count.** A `chip` buys you a visible
microchip boundary in-game and named outputs; a `mod` keeps the gates in the parent grid
and supports `ref`/`*` params. Neither one shares logic between call sites.

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
array queue: int[]

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

## Gotchas worth knowing

- **An expression-`if` is a `Select` gate, so BOTH arms evaluate.** Guard
  possibly-out-of-bounds array reads with a statement-`if`, never a ternary.
- **A `mod`-local `static var` is per-copy, not shared** -- each inlined instance gets its
  own. Hoist shared state to a root `var`.
- **`.brz` file size is a good proxy for gate count** while iterating; you don't need to
  count gates to see whether a refactor helped.
- **Don't optimize prematurely.** Gate count only matters once something is instantiated
  many times. A leaf helper called twice is fine as a `mod`.

## Checklist

When a build is unexpectedly huge, walk this list:

1. How many call sites reach the biggest `mod`? Multiply -- that's your bill.
2. Can many producers be funneled through one queued dispatch site?
3. Can near-identical entry points collapse into one parameterized `mod`?
4. Can a hot shared routine be deferred behind a dirty flag to one call per tick?
5. Is any per-slot boolean state an array that should be a bitmask?
6. Is anything being re-derived inside a callee that could be passed in?

## See also

- [Chips](chips.md) -- `mod` vs `chip` semantics, `ref`/`*` params, nested chips
- [Execution Context](exec-context.md) -- pure vs exec, and why array reads need exec
- [Builtin Functions](builtins.md) -- `BitCount` and the other cheap primitives
