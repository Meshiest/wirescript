// Differential: a `const` array/map read at RUNTIME vs the same value read
// from the ordinary runtime equivalent.
//
// A `const` container is two things at once. It is a compile-time value —
// `xs[1]` below is answered by the evaluator and baked, contributing zero
// gates — and it is also a real container gate, built on first runtime use
// with its contents baked into the gate's initial value, so a read at a
// RUNTIME index works like any other array. This file exists to prove those
// two never disagree: every check runs the same lookup through both.
//
// HOW THE CONST HALF IS FORCED. Every comparison goes through `checkInt`'s
// `cv` parameter, declared `const` — a const-REQUIRED position, so that side
// must be answered at compile time or the program does not compile. `rv` is a
// live gate result. So each line is a compile-time answer against a wire.
//
// Before this feature the runtime side had no lowering at all: the read fell
// back to an unsupported placeholder that emitted no component, so every
// lookup silently read the type's default (0) while the compile-time side
// stayed correct. It compiled clean, which is why the check has to be a
// differential rather than a compile.

let start = ReadBrickGrid()

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

const xs = [10, 20, 30]
const m = { "a": 11, "b": 22 }

// A const container reaches a `T[]` parameter by reference, exactly like a
// `var` array does — the mod body reads the same container gate.
mod pick(ys: int[], at: int) -> int {
  return ys[at]
}

on start {
  var pass: int = 0
  var total: int = 0

  // Runtime index and key. Their values match the constant indices used on
  // the `cv` side, so the two sides must agree element for element.
  var i: int = 1
  var k: string = "b"

  // The ordinary runtime equivalent, built the long way.
  var rArr: int[]
  rArr.push(10)
  rArr.push(20)
  rArr.push(30)

  // The headline check: the SAME const table, indexed at compile time on one
  // side and at runtime on the other.
  checkInt(pass, total, xs[1], xs[i], "const table: constant index vs runtime index")

  // ... and against an ordinary var array holding the same elements, so a
  // const table that materialized with the WRONG contents can't pass by
  // agreeing with itself.
  checkInt(pass, total, xs[1], rArr[i], "const table vs an ordinary var array")
  checkInt(pass, total, xs[0], rArr[0], "const table element 0")
  checkInt(pass, total, xs[2], rArr[2], "const table element 2")

  // Read-only methods on a const container run against the materialized gate.
  checkInt(pass, total, 3, xs.length(), "const table length at runtime")
  checkInt(pass, total, 60, xs.sum(), "const table sum at runtime")

  // Passing the const container to a `T[]` parameter.
  checkInt(pass, total, xs[2], pick(xs, 2), "const table through an array parameter")
  checkInt(pass, total, xs[2], pick(rArr, 2), "control: var array through the same parameter")

  // A const map, read at a runtime key.
  checkInt(pass, total, m["b"], m[k], "const map: constant key vs runtime key")
  checkInt(pass, total, 11, m["a"], "control literal against the map's other entry")

  BroadcastChatMessage("const containers: " .. pass .. "/" .. total)
}
