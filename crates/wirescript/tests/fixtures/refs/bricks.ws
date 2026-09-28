// Bricks as values. Nothing in the language makes a brick, so this one needs
// the editor: after pasting, point `anchorA` and `anchorB` at two different
// bricks, then type /refbricks in chat. It can be run again after changing them.

var pass: int = 0
var total: int = 0

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

var anchorA: brick
var anchorB: brick
var copyA: brick
var bricks: brick[]

on ChatCommand("refbricks") {
  pass = 0
  total = 0
  bricks.clear()

  copyA = anchorA
  assert(1, 0 + (copyA == anchorA), "a brick copies var to var")
  assert(0, 0 + (anchorA == anchorB), "two different bricks differ (set both in the editor)")
  bricks.push(anchorA)
  bricks.push(anchorB)
  assert(2, bricks.length(), "a brick array holds two bricks")
  assert(1, 0 + (bricks[1] == anchorB), "element 1 is anchorB")

  BroadcastChatMessage("bricks: ${pass}/${total}")
}
