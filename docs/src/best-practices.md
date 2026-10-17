# Best Practices: Gate Count & Scaling

Wirescript makes it easy to write logic that reads like a normal program but compiles
into an enormous number of gates. The patterns below came out of shrinking a real game
circuit from roughly **300,000 gates to about 8,000**. The logic did not change, only
its shape.

Most of that work was done before `fn` and `for` existed. Those two now handle the most
common cause of a large build, so they are covered first. The older patterns follow, and
most of them still apply.

<!-- toc -->
## Contents

- [Every `mod` call is a copy; a `fn` call is not](#every-mod-call-is-a-copy-a-fn-call-is-not)
- [The call-site multiplier](#the-call-site-multiplier)
- [When to use `fn`, and its limits](#when-to-use-fn-and-its-limits)
- [When to use `for`, and what it costs](#when-to-use-for-and-what-it-costs)
- [1. Funnel many producers through one dispatch site](#1-funnel-many-producers-through-one-dispatch-site)
- [2. Merge per-variant mods into one parameterized mod](#2-merge-per-variant-mods-into-one-parameterized-mod)
- [3. Defer hot shared work behind a flag](#3-defer-hot-shared-work-behind-a-flag)
- [4. Bitmasks instead of per-slot arrays](#4-bitmasks-instead-of-per-slot-arrays)
- [5. Resolve once, pass down](#5-resolve-once-pass-down)
- [6. Prefer pure `let` chains over exec ladders](#6-prefer-pure-let-chains-over-exec-ladders)
- [7. Reduce with a native array aggregate, not an unrolled fold](#7-reduce-with-a-native-array-aggregate-not-an-unrolled-fold)
- [8. Keep a derived array to unlock an aggregate](#8-keep-a-derived-array-to-unlock-an-aggregate)
- [9. Derive in pure output bindings, not on the exec chain](#9-derive-in-pure-output-bindings-not-on-the-exec-chain)
- [10. Per-entity work: a `for` sweep, or one chip each](#10-per-entity-work-a-for-sweep-or-one-chip-each)
- [Measured costs](#measured-costs)
- [Profiling: find the hot spots](#profiling-find-the-hot-spots)
- [Common mistakes](#common-mistakes)
- [Checklist](#checklist)
- [See also](#see-also)
<!-- /toc -->

## Every `mod` call is a copy; a `fn` call is not

A **`mod` is inlined**. Its entire body is copy-pasted into the caller's grid at every
call site, and that expansion is **transitive**: everything the `mod` reaches is
copied too.

```wirescript
mod heavy(x: int) { /* 500 gates of logic */ }

mod a() { heavy(1) }
mod b() { heavy(2) }
mod c() { heavy(3) }
// heavy is now built three times: 1500 gates
```

**A `chip` does not fix this.** A chip is not a shared subroutine you jump into. Each
call builds a **new instance**. It emits *the same gates a `mod` would*, plus an
input/output rerouter per boundary port and the microchip container itself. Chips can be
pure (no exec involved at all). They are a structural and visual boundary and do not
deduplicate anything.

So three calls cost three copies either way:

```wirescript
chip F(n: int) -> (y: int) { out y = n * 2 + 1 }
let c1 = F(a)
let c2 = F(a)
let c3 = F(a)
// Three F instances. Same six logic gates the mod version emits,
// plus 3 input rerouters + 3 output rerouters + 3 microchip containers.
```

**A `fn` is compiled once.** Its body becomes one function definition plane, and every
call is a single Function Call gate wired to it. The call runs the body to the end, in
the same tick, and the caller's chain continues with the results:

```wirescript
var total: int = 0
var last: int = 0

fn heavy(x: int) -> int {
  total += x
  return total * 2 + x
}

in a: exec
in b: exec
in c: exec
on a { last = heavy(1) }
on b { last = heavy(2) }
on c { last = heavy(3) }
// heavy is built once; three Function Call gates point at it
```

**A `for` loop is compiled once too**, and runs every iteration within the tick it
starts. Repeated steps that differ only by an index can be written once:

```wirescript
var fuse: int[]

in tick: exec
on tick {
  for i in 0..fuse.length() {
    if fuse[i] > 0 { fuse[i] -= 1 }
  }
}
// one copy of the body, however many slots there are
```

| | `mod` | `chip` | `fn` | `for` |
|---|---|---|---|---|
| Compiles to | Inline gates in the caller's grid | The same gates, in a microchip instance | One function definition plane | One loop plane |
| N call sites / iterations | N copies of the whole subtree | N copies of the whole subtree | One copy, plus one Function Call gate per site | One copy, swept N times |
| Runs | As part of the caller | As part of the caller | When its call fires, to the end | Every iteration in one tick (`@sliced`: several) |
| Pure (no exec) | Yes | Yes | No: an exec chain or `exec = trigger` | No: exec only |
| Reads outer `in` ports and `let`s | Yes | Yes | No (`WS113`) | Yes |
| `emit` | Yes | Yes | No (`WS113`) | Yes |
| Outer `var`, array, map | Wired directly | Wired directly | Through a handle var in the plane | Through the plane's pins |
| Named multi-outputs | Via a returned record | Yes (`-> (a: int, b: int)`) | Yes (`-> (a: int, b: int)`) | Write a `var` before the loop |
| `ref`/`*` params | Yes | Yes | Yes | n/a |

**Constant arguments are free.** `F(1)` folds the `1` into the instance itself and drops
the input pin it would have crossed, so the constant lands as inline gate data on whatever
consumes it. The `mod` version does the same. Arguments that are already a wire (an
input, a var, another gate's output) cross the boundary through a rerouter as usual.

Captured outer variables work normally through the boundary. A chip that writes an outer
`var` wires to that one real variable gate and does not get a private copy per instance.

**Choose between `mod` and `chip` on organization, never on gate count.** A `chip` gives
you a visible microchip boundary in-game and named outputs. A `mod` keeps the gates in the
parent grid. Both support `ref`/`*` params, and a ref crosses a chip boundary as a direct
wire to the one real variable gate. Neither one shares logic between call sites.

Use a `fn` for a large body called from many places, and a `for` for repeated steps
that differ only by an index. When neither works (the body has to `emit`, read a port,
or run in a pure position), reduce the number of call sites instead. The numbered
sections below show how.

## The call-site multiplier

The cost multiplies. If some heavy shared subsystem is reachable from `N` call sites,
you pay for it `N` times:

```wirescript
// 10 slots x 3 inputs = 30 call sites, each inlining the entire state machine
mod onInput(slot: int, code: int) { /* whole phase machine */ }

if (mask & BIT_0) { if a0 { onInput(0, 0) } if b0 { onInput(0, 1) } if c0 { onInput(0, 2) } }
if (mask & BIT_1) { if a1 { onInput(1, 0) } if b1 { onInput(1, 1) } if c1 { onInput(1, 2) } }
// ... x10
```

This is the shape that produced the 300k-gate build. The simplest fix now is to make
`onInput` a `fn`: the thirty call sites then cost thirty Function Call gates instead of
thirty copies of the state machine. If the state machine has to `emit` or read ports,
see the workarounds in the next section. If those don't work for it, use the numbered
fixes further down, which are listed in order of impact.

## When to use `fn`, and its limits

A `fn` costs its body once, plus a Function Input per parameter slot, a Function Output
per result, a handle var per outer `var`, array or map its body touches, and one Function
Call gate at each call site. A tiny body (a compare, a lookup) called from two or three
places is cheaper as a `mod`, because inlining it costs about as much as the call. Use
a `fn` when the body is large or the call sites are many.

A `fn`'s local `var` is one gate however many sites call it, and it resets when a call
starts. A `mod`'s local `var` is one gate per call site.

A `fn` body receives only its parameters and sends back only its results. The one
exception is outer storage, which it reaches through handles. Each of the limits below
has a workaround.

**Ports and computed `let`s do not reach a `fn` body (`WS113`).** Pass the value as a
parameter, or sample the ports into storage at the top of the tick and read the storage
inside:

```wirescript
in sit0: bool
in sit1: bool
var seated: bool[]
var count: int = 0

fn seatedCount() -> int {
  var n: int = 0
  for o in seated { if o { n += 1 } }
  return n
}

on Clock(interval = 0.1) {
  seated = [sit0, sit1]   // one sample, read by every fn this tick
  count = seatedCount()
}
```

**A `fn` cannot `emit` (`WS113`).** Return a flag and let the caller fire the output:

```wirescript
@top out placed: exec
var bombs: int[]

fn tryPlace(c: int) -> bool {
  if bombs.length() >= 4 { return false }
  bombs.push(c)
  return true
}

in press: exec
on press {
  if tryPlace(3) { emit placed }
}
```

If one call can cause several events that each need their own pulse, count them in a
`var` inside the `fn`. The caller then fires the output that many times from a loop:
`for n in 0..hits { emit hit }`.

**Signature limits.** A call carries at most 8 values in and 8 out, and
a record parameter takes one slot per field (`WS115`). A `fn` cannot return a record,
tuple or enum, or take a type parameter, a `const`, `exec` or `...rest` parameter
(`WS114`), and it cannot call itself, directly or through another `fn` (`WS020`). If a
helper needs any of these, keep it as a `mod`. A `mod` can still call `fn`s.

**Inside a spawned prefab, a `fn` can't use outer storage.** Measured in game: its reads
and writes don't reach the prefab's storage. Pass values in as parameters and return
them as results.

**Planes also save runtime.** Pure gates outside a plane are evaluated even when a
runtime `if` around them is not taken. Gates inside a plane are evaluated only when the
plane is entered. Measured in game with a loop plane: an expression behind a runtime
`if` cost 121 evaluations per iteration, and the same expression inside the plane cost
39, about 8 of them for entering it. So heavy pure math that is only needed sometimes
runs faster inside a `fn` or `for` body.

## When to use `for`, and what it costs

Use a `for` for repeated steps that differ only by an index: per-slot ticks, per-seat
updates, a fixed number of steps along a line. To stop at the first match, use `break`.
A loop body can't `return`, so store the result in a `var` declared before the loop:

```wirescript
var cell: int[]
var found: int = -1

in go: exec
on go {
  found = -1
  for i in 0..cell.length() {
    if cell[i] < 0 {
      found = i
      break
    }
  }
}
```

When the test is plain equality, `cell.find(-1)` is one gate and needs no loop.

**Chain reactions can finish in one tick.** When one step can trigger others (a blast
setting off nearby bombs, a flood fill), run passes in a loop. Each pass handles
everything that is ready, the next pass catches whatever it triggered, and a pass that
does nothing ends the loop. Limit the number of passes to the element count, because
each pass that continues has used up at least one element:

```wirescript
var fuse: int[]
var fired: int = 0

fn detonate(i: int) {
  fuse[i] = -1
  // light the blast here; set the fuse of any bomb it reaches to 0
}

in tick: exec
on tick {
  for pass in 0..fuse.length() {
    fired = 0
    for i in 0..fuse.length() {
      if fuse[i] == 0 {
        detonate(i)
        fired += 1
      }
    }
    if fired == 0 { break }
  }
}
```

**The runtime cost.** A loop's gates are placed once, but its body runs once per
iteration:

- **Hoist invariant work.** Anything in the body that does not depend on the binding is
  recomputed every pass. Compute it above the loop and read it inside.
- **A runtime bound costs a few always-live gates.** `for i in 0..n` emits the
  iteration-count arithmetic (about five gates) outside the plane, evaluated whether or
  not the loop runs. A constant bound folds away.
- **Entering the plane costs about 8 evaluations** per iteration, measured in game. For
  a two-gate body, unrolling is faster at runtime, though it uses more gates.
- **The frame budget runs out before the loop cap.** The game aborts a tick's loops past
  100,000 invocations, but at a measured ~44.5 ns per gate evaluation a 60 Hz frame has
  room for about 375,000 evaluations. A 2,300-gate body gets roughly 160 iterations per
  frame.

**Body restrictions.** A loop body can't use `return`, `continue` or `await` (`WS078`,
`WS079`). Set a `var` and `break` instead. A `mod` called inside the body can still
`return`, which leaves the `mod`, not the loop. A file with a `for` cannot be `@flat` (`WS080`), and an array of
records is iterated by index (`WS082`). See [`for` Loops](statements.md#for-loops).

To spread work across ticks on purpose, mark the loop `@sliced`, which lets the game
split its iterations over several ticks, or use a back-edge loop, which takes one tick
per iteration.

## 1. Funnel many producers through one dispatch site

With `fn`, many call sites no longer multiply the gate count, so try that first. A
queue is still useful when events must be handled one at a time, tagged with the phase
they were raised in, or limited to one per tick, or when the handler can't be a `fn`.

Do not call the logic from every producer. Have producers push a small encoded integer
into a queue, and dequeue **one per tick** at a single call site. The state machine then
inlines once.

```wirescript
var queue: int[]

// Producers are now trivial: they inline almost nothing.
mod enqueue(slot: int, code: int) {
  if queue.length() < 32 {
    queue.push(phase * 64 + slot * 4 + code) // pack: phase | slot | code
  }
}

on tick {
  // The only dispatch site: everything downstream is built once.
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

The same approach works for a `fn`, or for the body of a `for`.

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
    refresh()      // built once
  }
}
```

With a `fn`, calling `refresh()` from all eighteen sites is cheap in gates, but each
call still runs it. The flag is what limits it to once per tick.

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
if (flagged & (1 << i)) { /* ... */ }  // test: already truthy, no `!= 0` needed
let n = BitCount(flagged)              // popcount builtin, not a 10-way sum
```

This was the biggest single saving late in the work. Bitmasks also help in other ways:

- **Derived sets are free:** `BitCount(active & ~disabled)` replaces a loop-and-count,
  including one written as a `for`. It is one gate, evaluated once, instead of a loop
  over every slot.
- **Two masks beat a tri-state array:** store `votedMask` and `yesMask` rather than an
  array of `-1/0/1`. "Voted no" is then `votedMask & ~yesMask`.
- **Pass masks (plain `int`) to pure helpers** instead of arrays, so the helpers stay
  pure and cheap to inline.
- **Bit outputs drive hardware directly.** If an output expects one bit per slot, publish
  the mask itself. No pack loop is needed.
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

If the callee is a `fn`, the lookup is built once but still runs on every call, so
passing the result down still saves runtime.

A free-running counter (`buffer tick`) also makes a good round-robin cursor. With it,
`tick % 10` services one slot per tick instead of building ten service chains.

## 6. Prefer pure `let` chains over exec ladders

A predicate written as an early-return ladder becomes exec gates. The same predicate
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
unrolled max/sum over `N` slots is `N` compares/adds plus the accumulator plumbing. A
`for` that folds into a `var` emits the body once but still evaluates it `N` times. The
aggregate is evaluated once.

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

- **Gotcha:** `.max()` / `.min()` return a record `{ IsEmpty: bool, Value: int }`. Read
  `.Value` (and check `IsEmpty` when the array can be empty). `.sum()` / `.average()`
  return a bare scalar.
- If the slots you want to *exclude* already hold the identity value (`0` for a sum, or a
  value below every real one for a max), reduce the whole array. No masking is needed.
- The other whole-array methods work the same way: `find`, `fill`, `copyFrom` and
  `resize` each do in one gate what would otherwise take a loop.

## 8. Keep a derived array to unlock an aggregate

To reduce a *computed* value over a sub-range, don't decode-and-add per element every time.
Maintain a parallel array holding the precomputed per-element value in lockstep with the
source, then `slice` the window into a scratch array and `sum()` it. That is 2 gates
instead of ~`N`.

```wirescript
var packed: int[]   // source (e.g. state+value packed per element)
var vals: int[]     // derived mirror: the summable value per element
var scratch: int[]  // reusable slice target

// keep the mirror in lockstep at every write to `packed`
mod setCell(i: int, v: int) { packed[i] = encode(v)  vals[i] = v }

// reduce a 12-wide window in 2 gates, not 12 decodes + 11 adds
mod windowSum(base: int) -> int {
  scratch.slice(vals, base, 12)   // slice replaces scratch with vals[base..base+12]
  return scratch.sum()
}
```

The cost is one extra write per source mutation. Every reduction is cheaper in return, and
reductions are usually called from many slots. Check that *every* write to the source has a
paired write to the mirror, or the aggregate silently goes out of sync.

## 9. Derive in pure output bindings, not on the exec chain

Array reads are exec-only, so an `on <update>` handler that reads a buffer and *also*
computes display values inlines the whole decode/format on the exec chain (a `Union` /
`Branch` / `Var_Set` per step) and needs a cached-result var per output. Cache only the raw
read, and derive everything else in the **pure** output binding.

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
`return if ... then ... else ...`), not a statement-`if` with early `return`s. That mod
can't be a `fn`, because a `fn` call needs an exec chain, or an explicit
`exec = trigger`.

## 10. Per-entity work: a `for` sweep, or one chip each

A back-edge loop advances one iteration per tick, so it can't sweep a roster every
tick: N entities take N ticks per pass, and it gets slower as N grows. A `for` loop
can. The whole sweep finishes in the tick it starts, and its body is built once:

```wirescript
var hp: int[]
var regen: int = 1

on Clock(interval = 0.1) {
  for i in 0..hp.length() {
    if hp[i] < 100 { hp[i] = min(100, hp[i] + regen) }
  }
}
```

So per-tick work over a roster can be a `for`, as long as N times the body fits in a
frame (see [When to use `for`](#when-to-use-for-and-what-it-costs)).

Give each entity its own chip instance when a sweep isn't enough: when it needs its own
event handlers, a lifetime tied to the thing it follows, or per-tick work too heavy to
run N times in one frame. Each instance then does its own entity's work in parallel,
and the cost no longer depends on N:

```wirescript
var minions: Map<character, entity>

on CharacterSpawned() -> (who) {
  let e = SpawnPrefab(prefab = $./minion.ws, lifetime = 0.0, limit = 64)
  minions.set(who, e)
  e.SendCustomEvent("minion.init", who)
}
```

and in `minion.ws`, the per-tick handler nests inside the init handler so it
closes over a reference that is set:

```wirescript
var owner: character
var ready: bool = false

on CustomEvent("minion.init", isObject = true) -> (who: character) {
  owner = who
  ready = true
  on ServerUptime() {
    if ready {
      // per-tick work for this one entity only
      if owner == null { ReadBrickGrid().DestroySpawnedPrefab() }
    }
  }
}
```

Four things to watch for:

- **The nested handler is not registered by the outer one.** The circuit is
  static, so its trigger is live from the moment the prefab spawns, a tick or
  more before the init event lands. It needs a guard on an init-set value.
  Nesting only scopes the reference for readability. It does not order the two
  handlers.
- **Each instance should own only its own state.** Shared state stays in the
  central chip, which stays event-driven. If an instance needs to read another
  instance's data, you are back to a roster sweep, done through events.
- **Instances must clean themselves up.** A spawned chip whose subject is gone
  keeps running. Check for that in the tick handler and call
  `DestroySpawnedPrefab()`.
- **A `fn` inside the prefab can't use the prefab's storage.** Pass values in as
  parameters and return them as results, or use a `mod`.

Keep the *bookkeeping* central and event-driven, and push only the per-tick
rendering or sampling out to the instances.

## Measured costs

The numbers below were measured in game, not estimated. They matter because
the usual intuitions about what to optimize are mostly wrong here.

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

**Map reads are free and O(1).** A 1024-entry map reads no slower than an
8-entry one. A permanent record store can grow forever without pruning, and
keying per-entity state by `character` or account id costs nothing at runtime.

**The only operation worth caching is a built string.** If the same text is
rebuilt every frame and only occasionally changes, store it in a map and read it
back. Caching anything else makes no measurable difference, and a cache with one
reader is a net loss.

### Gates, ticks and evaluations

For N items:

| Construction | Bodies (gates) | Ticks for N items |
|---|---|---|
| Unrolled calls | N | **1** |
| `fn` called N times | 1, plus N call gates | **1** |
| `for` loop | 1 | **1** (`@sliced`: several) |
| Back-edge loop (`await`) | 1 | N |
| Self-event fan-out | 1 | N |
| One send to N distinct receivers | N | **1** |
| N sends on N distinct channels | N | **1** |

With `for` and `fn` you no longer have to choose between gates and ticks: one body
handles N items in one tick. The cost moves to evaluations instead. The body runs N
times within the tick, and the frame budget limits how many that can be (see
[When to use `for`](#when-to-use-for-and-what-it-costs)).

**Firing the same event N times gives one invocation per tick.** Repeating one
channel behaves like a loop and does not parallelize the work.

**The limit is per channel, not global.** Sends on different channels in one exec
chain all land in the same tick. N distinct channels move N payloads in one tick, and an
event carries up to eight data fields, which is 8N values per tick without
serializing anything.

That matters because the obvious way around "an event cannot carry an array" is
to concatenate values into one string and split them apart at the receiver. That
packing is expensive and usually unnecessary: six rows of three fields measured
**95 gates** packed-and-unpacked against **27** over three parallel channels
carrying six fields each.

But **one send reaching many receivers costs one tick total.** Each extra send
adds cost. Each extra receiver adds none. So broadcast one packed payload and let each
receiver filter, rather than tailoring a message per receiver.

Practical rule: **use a `for` or `fn` instead of unrolling.** Either gives one body
and one tick. Unroll only a few cheap steps, where entering a plane would cost more than
the body. Spread work across ticks (`@sliced`, a back-edge loop, a self-event) only when
nothing is waiting on it.

### Where the gates go

- **A `mod` inlines per call site**, so its cost is roughly `body x call sites`.
  That is an upper bound. The compiler shares loop-invariant subexpressions
  across the copies, so hoisting a repeated read out of an unrolled mod by hand
  usually saves very little.
- **A `fn` costs its body once**, plus one Function Call gate per call site, a
  Function Input per parameter slot, a Function Output per result and a handle
  var per outer `var`, array or map its body touches.
- **A `for` costs its body once**, plus the loop chip and its entry, and about
  five always-live gates when its bound is a runtime value.
- **A `mod`'s local `var` becomes a storage gate at every call site.** Three
  locals in a helper called from a 30-wide unroll is ninety gates. Making the
  helper a `fn` collapses that to three. Hoisting them to module scope does too,
  but only do it when every call site is alone in its statement, because module
  scratch is shared and two calls in one expression will race.
- **A `mod`'s return path depends on its shape.** A mod whose body is a single
  expression returns through a wire and costs about one gate per call site. A mod
  with an early `return` returns through a storage gate, which is opaque to the
  constant folder, and costs several times more per site. Rewriting an
  early-return helper as one expression is usually the cheapest change available
  to it.
- **Unused storage is never removed.** Guarding a feature behind a false constant
  removes its logic but not its `var` declarations. Nothing prunes unused state,
  and the compiler never warns about it.

### Feature flags fold completely

A body guarded by a compile-time-false constant is **deleted, not skipped**:

```wirescript
const FEATURE = false
on tick { if FEATURE { expensiveThing() } }   // compiles to nothing
```

Measured: a 1056-gate body behind `const FEATURE = false` compiled to 3 gates.
Shipping two builds of a chip (one with a feature, one without) is therefore
close to free, and cuts both gate count and artifact size for anyone who does
not need it.

### Size is usually the real constraint

There is plenty of runtime headroom, but artifact size is limited.

- **Every `.brz` has a ~25KB floor**, whatever it contains. A 10-gate helper
  chip and a 500-gate chip cost about the same. Splitting work across many small
  chips pays that floor every time.
- **A `SpawnPrefab` reference embeds the whole prefab** into the spawning chip.
  Shrinking the prefab shrinks every chip that spawns it, and each spawner
  carries its own copy.
- **On-screen text is capped at roughly 32 elements per player**, shared across
  every circuit that player has loaded, not counted per chip. Past the cap, one
  element silently never renders, and which one it is changes when you change
  unrelated things. Budget by the number of elements, not the number of draw calls.

### Before optimising, check you have a problem

Compute what your circuit does per second and compare it to the numbers
above. A full 30-player HUD system doing ~8,000 string builds per second spends
under 1% of wall clock on the only operation that costs anything.

Gate count still matters for **paste size, world load and the text-element
budget**. It mostly does not matter for per-tick CPU. Optimize for whichever of
these is your real limit. The exception is a `for` loop or a frequently called `fn`: its
body runs many times per tick, so the body's size does affect per-tick CPU.

## Profiling: find the hot spots

`--dump-ir` prints the compiled program to **stdout** as `.wsir` text, one gate per
line with its `loc(line:col ...)` source anchor (diagnostics stay on stderr). The
format is described in `crates/wirescript-ir/FORMAT.md`, and `wirescript assemble`
builds the same save back from it. Measure before you refactor, and target the dominant
gate kind or source line rather than a guess.

```bash
# the compiled program as text
cargo run -p wirescript-cli -- compile foo.ws --dump-ir > foo.wsir

# total gate count
grep -cE '^\s*%\S+ = ' foo.wsir

# which gate kinds dominate
grep -oE '^\s*%\S+ = [A-Za-z_]+' foo.wsir | awk '{print $3}' | sort | uniq -c | sort -rn | head

# which source lines emit the most gates (an imported file's lines carry its name)
grep -oE 'loc\(("[^"]*":)?[0-9]+' foo.wsir | sort | uniq -c | sort -rn | head
```

A dominant `Expr_MathModulo` / `Expr_MathDivide` count usually means a decode called
too often. A large `Exec_ArrayVar_Get` count means reads that could be cached or
aggregated. A single source line with a big share is your first refactor target.

A `fn` or `for` shows up as a `chip` line whose body is listed once inside its braces.
Each call is a `FunctionCall` gate whose `Definition` names the `fn`, so counting those
per name tells you how many sites reach each `fn`.

## Common mistakes

- **An expression-`if` is a `Select` gate, so both arms evaluate.** Guard
  possibly-out-of-bounds array reads with a statement-`if`, never a ternary.
- **Delete dead arithmetic.** If a value's range makes an op a no-op, drop it. For example,
  `(x / 16) % 4`, where `x / 16` is already proven `<= 2`, is the same as `x / 16`, and a
  mask that can never clear a set bit does nothing. Every op you remove is a gate you don't
  build.
- **Don't recompute inside one expression.** Bind a repeated (possibly expensive) call to a
  `let` once and reuse it. Each call is its own gate subtree, so `f(x) + f(x)` builds `f`
  twice. With a `fn`, it builds two call gates and runs the body twice.
- **A `mod`-local `static var` is per-copy, not shared.** Each inlined instance gets its
  own. Hoist shared state to a root `var`.
- **A `fn`-local `var` is shared by every call.** It resets when a call starts, so one
  call doesn't see another's value, but it is one gate, not one per site.
- **Hover a `mod`, `chip`, `on`, or `if` for its gate count.** The estimate covers the
  whole construct (following its calls), and a `mod` reports that it is inlined per call
  site. This shows what a refactor costs without compiling.
- **Don't optimize prematurely.** Gate count only matters once something is instantiated
  many times. A leaf helper called twice is fine as a `mod`.

## Checklist

When a build is unexpectedly huge, walk this list:

0. Profile with `--dump-ir` first, then refactor the dominant gate kind / source line instead of guessing.
1. How many call sites reach the biggest `mod`? Its cost is its body times that count. Can it be a `fn`?
2. Are repeated steps that differ only by an index written as one `for`?
3. Can many producers be funneled through one queued dispatch site?
4. Can near-identical entry points collapse into one parameterized `mod` or `fn`?
5. Can a hot shared routine be deferred behind a dirty flag to one call per tick?
6. Is any per-slot boolean state an array that should be a bitmask?
7. Is anything being re-derived inside a callee that could be passed in?
8. Can an unrolled or looped max/sum/count/search be a one-gate `.max()` / `.sum()` / `.find()`?
9. Would a derived parallel array turn a per-element decode-and-add into a `slice` + `sum`?
10. Is display/derived logic running on the exec chain when it could be a pure output binding?
11. Is a per-tick roster sweep a `for`, or one chip per entity when each needs its own handlers?

## See also

- [Chips](chips.md): `mod` vs `chip` semantics, `ref`/`*` params, nested chips
- [`for` Loops](statements.md#for-loops) and [`fn`](statements.md#fn-function): syntax, bindings and limits
- [Execution Context](exec-context.md): pure vs exec, and why array reads need exec
- [Builtin Functions](builtins.md): `BitCount` and the other cheap primitives
