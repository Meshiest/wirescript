// Differential: compile-time record destructuring (`const { ... } = ...`) --
// plain, aliased, and rest -- and multi-output `const mod` destructuring, vs
// the identical shapes built and destructured at runtime with `let`. Before
// this feature, a destructuring `const` was rejected outright (WS046); only
// a plain named `const` binding was supported.
//
// HOW THE CONST HALF IS FORCED. Unlike branching.ws/arrays.ws, no `cv: const`
// trick is needed here: every name a `const { ... } = ...` destructure
// introduces IS a compile-time constant by construction (a `const` binding
// that fails to evaluate is a compile error, never a silent runtime
// fallback), so passing one straight into `checkInt`'s `cv` argument is
// already the strongest position -- there is no weaker "gate vs gate" shape
// for a `const` destructure to hide behind.

let start = ReadBrickGrid()

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

type Point = { x: int, y: int, z: int }

const mod mkPointC() -> Point { return { x: 1, y: 2, z: 3 } }
mod mkPointR() -> Point { return { x: 1, y: 2, z: 3 } }

// Plain, alias, and rest, all at the TOP LEVEL.
const cpTop = mkPointC()
const { x: topX, y: topY } = cpTop
const { x: topAlias } = cpTop
const { x: topBx, ...topRest } = cpTop

// The same three forms again, this time destructured INSIDE a `const mod`
// body rather than at the top level.
const mod destructureInBody() -> (x: int, y: int, alias: int, bx: int, restY: int, restZ: int) {
  const p = mkPointC()
  const { x, y } = p
  const { x: alias } = p
  const { x: bx, ...rest } = p
  out x = x
  out y = y
  out alias = alias
  out bx = bx
  out restY = rest.y
  out restZ = rest.z
}
const bodyResult = destructureInBody()

// Multi-output `const mod`s destructure the same way a record does -- this
// is the compile-time "tuple".
const mod pairC(n: const int) -> (a: int, b: int) {
  out a = n
  out b = n + 1
}
mod pairR(n: int) -> (a: int, b: int) {
  out a = n
  out b = n + 1
}
const { a: pairCA, b: pairCB } = pairC(2)

// A multi-output `const mod` that ASSEMBLES a collection, and one whose
// `out` sits inside a compile-time `if`. Both used to fail outright when
// destructured -- the call was lowered as ordinary gates instead of being
// answered at compile time, so the collection's mutations had no array var
// to target (WS044) and the gated `out` never reached the destructure
// ("no field 'p'"). The single-output spellings of both were always fine,
// which is exactly why the multi-output ones needed their own fixture.
const mod roomsC(n: const int) -> (t: int[], k: int) {
  const acc = [0]
  acc.push(n)
  acc.push(n * 5)
  out t = acc
  out k = n
}
const { t: roomsT, k: roomsK } = roomsC(7)

const GATE_ON = 3
const mod gatedC(n: const int) -> (p: int, q: int) {
  if GATE_ON > 0 { out p = n * 10 } else { out p = 0 }
  out q = n
}
const { p: gatedP, q: gatedQ } = gatedC(3)

on start {
  var pass: int = 0
  var total: int = 0

  // NOTE: `let`, not `var`. Destructuring a `var`-held record lowers its reads
  // to `_Unsupported` placeholders that silently read 0 -- a pre-existing bug
  // (identical at HEAD), unrelated to `const`. Using `var` here made every
  // comparison below fail against 0 while the const half was correct.
  let rp = mkPointR()
  let { x: rx, y: ry } = rp
  let { x: rAlias } = rp
  let { x: rBx, ...rRest } = rp

  // Top-level const destructure vs. a runtime destructure of the identical
  // record shape.
  checkInt(pass, total, topX, rx, "top level: plain destructure x")
  checkInt(pass, total, topY, ry, "top level: plain destructure y")
  checkInt(pass, total, topAlias, rAlias, "top level: alias destructure")
  checkInt(pass, total, topBx, rBx, "top level: rest destructure - bx")
  checkInt(pass, total, topRest.y, rRest.y, "top level: rest destructure - rest.y")
  checkInt(pass, total, topRest.z, rRest.z, "top level: rest destructure - rest.z")

  // Destructuring INSIDE a const mod body, same three forms.
  checkInt(pass, total, bodyResult.x, rx, "in const mod body: plain destructure x")
  checkInt(pass, total, bodyResult.y, ry, "in const mod body: plain destructure y")
  checkInt(pass, total, bodyResult.alias, rAlias, "in const mod body: alias destructure")
  checkInt(pass, total, bodyResult.bx, rBx, "in const mod body: rest destructure - bx")
  checkInt(pass, total, bodyResult.restY, rRest.y, "in const mod body: rest destructure - rest.y")
  checkInt(pass, total, bodyResult.restZ, rRest.z, "in const mod body: rest destructure - rest.z")

  // Block-scope destructuring, plain/alias/rest, inside this handler.
  const { x: blockX, y: blockY } = cpTop
  const { x: blockAlias } = cpTop
  const { x: blockBx, ...blockRest } = cpTop
  checkInt(pass, total, blockX, rx, "block scope: plain destructure x")
  checkInt(pass, total, blockY, ry, "block scope: plain destructure y")
  checkInt(pass, total, blockAlias, rAlias, "block scope: alias destructure")
  checkInt(pass, total, blockBx, rBx, "block scope: rest destructure - bx")
  checkInt(pass, total, blockRest.y, rRest.y, "block scope: rest destructure - rest.y")

  // Multi-output const mod, destructured as a record -- the compile-time
  // "tuple". The record follows the mod's DECLARATION order (a, b), not
  // assignment order.
  let { a: pairRA, b: pairRB } = pairR(2)
  checkInt(pass, total, pairCA, pairRA, "multi-output destructure: a")
  checkInt(pass, total, pairCB, pairRB, "multi-output destructure: b")

  // The compile-time-assembled collection, rebuilt at runtime element for
  // element. Indexing `roomsT` straight inside `checkInt`'s `cv: const int`
  // argument keeps the read in a const-REQUIRED position.
  var rRooms: int[]
  rRooms.push(0)
  rRooms.push(7)
  rRooms.push(7 * 5)
  checkInt(pass, total, roomsT[0], rRooms[0], "multi-output collection: t[0]")
  checkInt(pass, total, roomsT[1], rRooms[1], "multi-output collection: t[1]")
  checkInt(pass, total, roomsT[2], rRooms[2], "multi-output collection: t[2]")
  checkInt(pass, total, roomsT.length(), rRooms.length(), "multi-output collection: t length")
  checkInt(pass, total, roomsK, 7, "multi-output collection: scalar output")

  // The `out` inside a compile-time `if`: the taken branch's value is the
  // one the destructure must see.
  checkInt(pass, total, gatedP, 3 * 10, "multi-output gated out: taken branch")
  checkInt(pass, total, gatedQ, 3, "multi-output gated out: sibling output")

  BroadcastChatMessage("const destructure: " .. pass .. "/" .. total)
}
