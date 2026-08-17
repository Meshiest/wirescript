// Differential: a const-mod call NESTED inside a larger expression -- an
// operator, a unary operator, or a Vec constructor argument (positional and
// named) -- vs the identical runtime shape. Before this feature, nesting a
// const-mod call inside any of these was WS046 (not evaluated); the call had
// to be bound to a name on its own line first. Now `eval_expr` descends into
// these forms directly, so the call is evaluated wherever it sits.
//
// HOW THE CONST HALF IS FORCED. Same mechanism as branching.ws/arrays.ws:
// every comparison passes the const half through `checkInt`/`checkVec`'s
// `cv` argument, whose `cv` parameter is declared `const`. That is a
// const-REQUIRED position, so the WHOLE expression -- the nested call and
// the operator/constructor around it -- must be answered by the compile-time
// interpreter; the result bakes as a literal and contributes zero gates.
// Passing a plain (non-`const`) argument instead would let the const-mod
// call fall back to an ordinary inlined call (gate versus gate -- the check
// could not fail), so every check below is a compile-time answer versus a
// live gate result.

let start = ReadBrickGrid()

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

mod checkVec(pass: *int, total: *int, cv: const vector, rv: vector, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

const mod cDouble(n: int) -> int { return n * 2 }
mod rDouble(n: int) -> int { return n * 2 }

const mod cAddF(n: float) -> float { return n + 1.0 }
mod rAddF(n: float) -> float { return n + 1.0 }

on start {
  var pass: int = 0
  var total: int = 0

  // Nested in a binary operator.
  checkInt(pass, total, cDouble(3) + 1, rDouble(3) + 1, "call in binary operator")
  checkInt(pass, total, 1 + cDouble(3), 1 + rDouble(3), "call as the LEFT operand")

  // Both operands of the same operator are calls -- proves the fix isn't
  // order-dependent.
  checkInt(pass, total, cDouble(2) * cDouble(3), rDouble(2) * rDouble(3), "calls on both sides of an operator")

  // Nested in a unary operator.
  checkInt(pass, total, -cDouble(3), -rDouble(3), "call in unary operator")

  // Nested in a Vec constructor argument, positional.
  checkVec(
    pass, total,
    Vec(cAddF(1.0), 2.0, 3.0),
    Vec(rAddF(1.0), 2.0, 3.0),
    "call in Vec ctor argument (positional)"
  )

  // Nested in a Vec constructor argument, NAMED and out of source order --
  // binding must go by parameter name, not by write order.
  checkVec(
    pass, total,
    Vec(z = cAddF(1.0), x = 2.0, y = 3.0),
    Vec(z = rAddF(1.0), x = 2.0, y = 3.0),
    "call in Vec ctor argument (named, reordered)"
  )

  BroadcastChatMessage("const compound: " .. pass .. "/" .. total)
}
