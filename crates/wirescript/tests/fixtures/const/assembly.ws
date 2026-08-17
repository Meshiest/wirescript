// Differential: a const mod ASSEMBLING an array CONDITIONALLY (`if` +
// `push`, evaluated at compile time) vs the runtime mod doing the identical
// assembly with a real `if` and a real array var. This is the payoff of the
// whole const-evaluation feature: the emitted circuit's SHAPE follows a
// configuration constant — `rooms(1)` never builds the gates `rooms(2)`
// would have needed for its second room, because there are no gates at all;
// the compile-time interpreter decided the shape before lowering ever ran.
//
// HOW THE CONST HALF IS FORCED. Same mechanism as arrays.ws/branching.ws:
// every comparison passes the const half through `checkInt`'s `cv`
// argument, whose `cv` parameter is declared `const`. That is a
// const-REQUIRED position, so `rooms(n)` — its `if`s, its `push`es, all of
// it — must be answered by the compile-time interpreter; the result bakes
// as a literal array and contributes zero gates. Passing a plain `int`
// comparison instead would let the const-mod call fall back to an ordinary
// inlined mod (gate versus gate — the check could not fail), so every check
// below is a compile-time answer versus a live gate result, exactly like
// arrays.ws and branching.ws.
//
// WHY `const t = [0]` THEN `t.clear()`, NOT JUST `const t = []`. An empty
// array literal has no element to infer a type from, so `const t = []`
// binds `t` as `any[]` — and `any[]` does not coerce into this mod's
// declared `int[]` return type (a concrete array and an `any` array are
// different backing storage; only an `any[]`-typed SINK accepts a concrete
// array, never the reverse). Seeding with one `int` element first pins `t`
// as `int[]` from the start; `.clear()` (itself one of this task's
// mutations) then empties it before the conditional pushes run, so the
// array's static type matches its declared return type while its VALUE
// still starts empty.
//
// WHY THE RUNTIME HALF TAKES `t: int[]` AS A PARAMETER INSTEAD OF RETURNING
// ONE. `var r: int[] = someModThatReturnsAnArray(n)` is a KNOWN, pre-existing
// compiler defect: lowering only accepts an array LITERAL (`[...]`) as a
// `var array[]` initializer, so a call expression there — even one that
// type-checks cleanly and returns `int[]` — is silently dropped, along with
// the entire call: none of the callee's `if`s or `push`es lower at all (a
// WSP001 warning fires, but the fixture only reads chat, so it never saw the
// warning either). That is a lowering bug, not a const-evaluation one — it
// predates this feature and is pinned as a regression test in
// `array_ref.rs` (search `array_returning_mod_call_as_var_initializer`) —
// but it means a `var r: int[] = rrooms(n)`-shaped runtime half here would
// silently exercise NO gates and this fixture would once again be unable to
// fail. So the runtime half below uses the pattern that IS known to lower
// correctly: an `int[]` parameter is already a reference (never `*int[]` —
// that type-checks but silently drops writes at runtime, a separate
// footgun), so the caller declares an empty `var`, passes it into `rrooms`,
// and `rrooms` pushes into it directly instead of returning a new array.

let start = ReadBrickGrid()

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

// The const half: assembled at COMPILE TIME. Each `if` is const-evaluated
// (tree-shaking the untaken arm entirely), and each `push`/`clear` mutates
// `t` in place inside the interpreter — so `rooms(0)` never even reaches a
// `push` call, let alone emits a gate for one.
const mod rooms(n: int) -> int[] {
  const t = [0]
  t.clear()
  if n >= 1 { t.push(10) }
  if n >= 2 { t.push(20) }
  if n >= 3 { t.push(30) }
  return t
}

// The runtime half: the same shape, built with a real `if` and a real array
// var at RUNTIME — but pushing into a `t: int[]` PARAMETER the caller
// supplies, not returning a freshly-built array (see the top-of-file
// comment on why).
mod rrooms(n: int, t: int[]) {
  if n >= 1 { t.push(10) }
  if n >= 2 { t.push(20) }
  if n >= 3 { t.push(30) }
}

on start {
  var pass: int = 0
  var total: int = 0

  // Each `r*` starts as an empty array var the caller owns; `rrooms` pushes
  // into it in place (see the top-of-file comment for why it is a parameter
  // rather than a return value). `rooms(n)` needs no such var: it is the
  // WHOLE argument to `checkInt`'s const `cv` parameter, so it — and every
  // `.length()`/`[i]` chained onto it — is answered by the compile-time
  // interpreter before lowering ever looks for storage.

  // n = 0: neither `if` is taken — an empty table both ways. Only `.length()`
  // is checked here; indexing an empty compile-time array is itself an error
  // (there is no previous value to fall back on, unlike a runtime read).
  var r0: int[]
  rrooms(0, r0)
  checkInt(pass, total, rooms(0).length(), r0.length(), "rooms(0) length")

  var r1: int[]
  rrooms(1, r1)
  checkInt(pass, total, rooms(1).length(), r1.length(), "rooms(1) length")
  checkInt(pass, total, rooms(1)[0], r1[0], "rooms(1)[0]")

  var r2: int[]
  rrooms(2, r2)
  checkInt(pass, total, rooms(2).length(), r2.length(), "rooms(2) length")
  checkInt(pass, total, rooms(2)[0], r2[0], "rooms(2)[0]")
  checkInt(pass, total, rooms(2)[1], r2[1], "rooms(2)[1]")

  var r3: int[]
  rrooms(3, r3)
  checkInt(pass, total, rooms(3).length(), r3.length(), "rooms(3) length")
  checkInt(pass, total, rooms(3)[0], r3[0], "rooms(3)[0]")
  checkInt(pass, total, rooms(3)[1], r3[1], "rooms(3)[1]")
  checkInt(pass, total, rooms(3)[2], r3[2], "rooms(3)[2]")

  BroadcastChatMessage("const assembly: " .. pass .. "/" .. total)
}
