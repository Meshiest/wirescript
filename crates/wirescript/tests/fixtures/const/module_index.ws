// Differential: a module-level `const` array, indexed at module scope
// (`const z = t[1]`), used both to bake an array's initial contents and as a
// live wire operand -- vs the identical value read back from a runtime
// array built the ordinary way. Before this feature, a module-level const
// index used as a wire operand had no lowering form and silently fell back
// to an unsupported placeholder -- the read compiled clean but the wire
// carried nothing, so a comparison against it read the type's default
// instead of the real value. Only the WIRE-OPERAND channel needed the fix;
// the bake channel (an initializer like `counts` below) already worked.

let start = ReadBrickGrid()

mod checkInt(pass: *int, total: *int, cv: const int, rv: int, label: string) {
  total = total + 1
  let ok = cv == rv
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

const t = [10, 20, 30]
const z = t[1]

// Bake channel: `z` feeds this array's initial contents alongside a
// distinctive control literal, so a dropped bake cannot pass by accident.
var counts: int[] = [z, 12345]

on start {
  var pass: int = 0
  var total: int = 0

  var rArr: int[]
  rArr.push(10)
  rArr.push(20)
  rArr.push(30)

  // Wire-operand channel: `z` feeds `checkInt`'s `cv` argument directly,
  // exactly like `if z == rv` would -- the value must inline as a real
  // literal operand, not an unwired placeholder.
  checkInt(pass, total, z, rArr[1], "module-level const index as a wire operand")

  // Bake channel: `z` (the compile-time answer) against a RUNTIME read of
  // `counts[0]` (what the emitted gate's initializer actually holds) -- a
  // dropped bake would read `counts[0]` as the type's default instead.
  checkInt(pass, total, z, counts[0], "module-level const index baked into an initializer")
  checkInt(pass, total, 12345, counts[1], "control literal alongside the baked index")

  BroadcastChatMessage("const module index: " .. pass .. "/" .. total)
}
