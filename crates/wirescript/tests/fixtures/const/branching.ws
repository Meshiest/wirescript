// Differential: const branching vs runtime branching. Each pair of mods below
// has a BYTE-IDENTICAL body; the only difference is which evaluator runs it.
//
// HOW THE CONST HALF IS FORCED. The const call is passed as the `cv` argument
// of `checkInt`/`checkStr`, whose `cv` parameter is declared `const`. That is a
// const-REQUIRED position, so the call must be answered by the compile-time
// interpreter and its result is baked as a literal — the const half contributes
// ZERO gates. Two shapes that look equivalent are NOT:
//
//   - `cGrade(0) == rGrade(0)` passed as a plain `bool` — the const mod lowers
//     as an ordinary INLINED mod (ret_val / ret_set / ret_get, and a live
//     Select for an if-expression). Gate versus gate: the check cannot fail.
//   - `const G0 = cGrade(0)` then comparing `G0` — the binding compiles, but
//     the result is NOT a compile-time constant (using `G0` where one is
//     required reports WS046 `'G0' is a runtime value`), so this also lowers
//     as an inlined mod.
//
// So every check below is a compile-time answer versus a live gate result.
//
// Values sit at the bucket EDGES (the exact boundary and one either side),
// because that is where a `<` vs `<=` off-by-one in const evaluation hides; a
// mid-bucket value sails straight past one.

let start = ReadBrickGrid()

// `cv` is `const`, which is what forces its argument through the compile-time
// interpreter; `rv` is an ordinary wire carrying the runtime mod's result.
mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

mod checkStr(pass: *int, total: *int, cv: const string, rv: string, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

// Sequential early-return `if`s — two independent statements, no `else`.
const mod cGrade(n: int) -> int {
  if n < 10 { return 0 }
  if n < 100 { return 1 }
  return 2
}

mod rGrade(n: int) -> int {
  if n < 10 { return 0 }
  if n < 100 { return 1 }
  return 2
}

// A genuine `else if` chain: ONE `if` statement carrying two `else` arms,
// which lowers differently from cGrade's two separate statements.
const mod cBand(n: int) -> string {
  if n < 0 { return "neg" } else if n == 0 { return "zero" } else { return "pos" }
}

mod rBand(n: int) -> string {
  if n < 0 { return "neg" } else if n == 0 { return "zero" } else { return "pos" }
}

// Nested `if`: the inner condition is reachable only through the outer one,
// so a const evaluator that mishandles branch nesting disagrees here while
// still getting every flat case above right.
const mod cQuad(x: int, y: int) -> int {
  if x >= 0 {
    if y >= 0 { return 1 }
    return 4
  }
  if y >= 0 { return 2 }
  return 3
}

mod rQuad(x: int, y: int) -> int {
  if x >= 0 {
    if y >= 0 { return 1 }
    return 4
  }
  if y >= 0 { return 2 }
  return 3
}

// `if`-`then`-`else` as an EXPRESSION (a Select gate at runtime), not a
// statement.
const mod cPick(flag: bool) -> string { return if flag then "on" else "off" }
mod rPick(flag: bool) -> string { return if flag then "on" else "off" }

on start {
  var pass: int = 0
  var total: int = 0

  // Both bucket edges of the early-return chain, and one value either side.
  checkInt(pass, total, cGrade(0), rGrade(0), "grade 0")
  checkInt(pass, total, cGrade(9), rGrade(9), "grade 9 (under first edge)")
  checkInt(pass, total, cGrade(10), rGrade(10), "grade 10 (first edge)")
  checkInt(pass, total, cGrade(11), rGrade(11), "grade 11 (over first edge)")
  checkInt(pass, total, cGrade(99), rGrade(99), "grade 99 (under second edge)")
  checkInt(pass, total, cGrade(100), rGrade(100), "grade 100 (second edge)")
  checkInt(pass, total, cGrade(101), rGrade(101), "grade 101 (over second edge)")

  // The else-if chain, on and around its own zero boundary.
  checkStr(pass, total, cBand(-1), rBand(-1), "band -1 (under edge)")
  checkStr(pass, total, cBand(0), rBand(0), "band 0 (edge)")
  checkStr(pass, total, cBand(1), rBand(1), "band 1 (over edge)")

  // Nested if: every quadrant, plus the origin where both axes sit on the edge.
  checkInt(pass, total, cQuad(1, 1), rQuad(1, 1), "quad x+ y+")
  checkInt(pass, total, cQuad(1, -1), rQuad(1, -1), "quad x+ y-")
  checkInt(pass, total, cQuad(-1, 1), rQuad(-1, 1), "quad x- y+")
  checkInt(pass, total, cQuad(-1, -1), rQuad(-1, -1), "quad x- y-")
  checkInt(pass, total, cQuad(0, 0), rQuad(0, 0), "quad origin (both edges)")

  // The if-EXPRESSION pair. This is the one that was previously gate-vs-gate:
  // cPick's Select survived into the graph instead of being interpreted.
  checkStr(pass, total, cPick(true), rPick(true), "pick true")
  checkStr(pass, total, cPick(false), rPick(false), "pick false")

  BroadcastChatMessage("const branching: " .. pass .. "/" .. total)
}
