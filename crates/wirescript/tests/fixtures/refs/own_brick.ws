// A brick as a value with no editor setup: the program's own microchip brick,
// copied, compared, and kept in an array and as a map key.

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

var mine: brick
var copy: brick
var held: brick[]
var seen: Map<brick, int>
var fires: int = 0

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  mine = GetMicrochipBrick()
  copy = mine
  assert(1, 0 + (copy == GetMicrochipBrick()), "a brick copies var to var")

  held.push(mine)
  held.push(copy)
  assert(2, held.length(), "a brick array holds two bricks")
  assert(1, 0 + (held[1] == mine), "an element equals the brick it holds")

  seen.set(mine, 7)
  assert(7, seen[copy], "a copy finds the brick's map entry")
  assert(1, seen.length(), "the brick and its copy are one key")

  BroadcastChatMessage("own brick: ${pass}/${total}")
}
