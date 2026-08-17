// Differential: const array construction/indexing/length vs the runtime
// equivalent. Each pair of mods below computes the SAME value; the only
// difference is which evaluator runs it.
//
// HOW THE CONST HALF IS FORCED. Every comparison is passed through
// `checkInt`'s `cv` argument, whose `cv` parameter is declared `const`. That
// is a const-REQUIRED position, so the array literal — and every `.length()`
// / `[i]` read on it — must be answered by the compile-time evaluator; the
// result is baked as a literal and contributes zero gates. A plain `bool`
// argument would let a const-mod call fall back to an ordinary inlined call
// (gate versus gate — the check could not fail), so every check below is a
// compile-time answer versus a live gate result, exactly like
// `branching.ws`.

let start = ReadBrickGrid()

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

// Each array ELEMENT is a const-mod CALL, not a bare literal or a named
// constant (both of which already baked before this feature) — so the array
// literal only bakes if `const_eval::eval_expr` actually evaluates the call
// per element.
const mod size(n: int) -> int { return n * 10 }
mod rsize(n: int) -> int { return n * 10 }

on start {
  var pass: int = 0
  var total: int = 0

  var rArr: int[]
  rArr.push(rsize(1))
  rArr.push(rsize(2))
  rArr.push(rsize(3))

  checkInt(pass, total, [size(1), size(2), size(3)].length(), rArr.length(), "array length")
  checkInt(pass, total, [size(1), size(2), size(3)][0], rArr[0], "array[0]")
  checkInt(pass, total, [size(1), size(2), size(3)][1], rArr[1], "array[1]")
  checkInt(pass, total, [size(1), size(2), size(3)][2], rArr[2], "array[2]")

  // A different set of arguments, to catch anything hard-coded to (1, 2, 3).
  var rArr2: int[]
  rArr2.push(rsize(4))
  rArr2.push(rsize(5))
  checkInt(pass, total, [size(4), size(5)].length(), rArr2.length(), "array length (2)")
  checkInt(pass, total, [size(4), size(5)][1], rArr2[1], "array[1] (2)")

  BroadcastChatMessage("const arrays: " .. pass .. "/" .. total)
}
