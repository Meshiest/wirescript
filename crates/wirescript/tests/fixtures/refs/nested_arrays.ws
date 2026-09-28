// Arrays of arrays: an outer array of inner-array refs, a var holding an array
// ref, a map of array refs, and array refs bound to array parameters.

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

var row0: int[]
var row1: int[]
var rows: int[][]
var cur: *int[]
var grids: Map<string, int[]>
var fires: int = 0

mod firstTwo(xs: int[]) -> int {
  return xs[0] + xs[1]
}

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  rows.push(&row0)
  rows.push(row1)
  rows[0].push(1)
  rows[0].push(2)
  rows[1].push(30)
  rows[1].push(40)
  assert(2, row0.length(), "pushes through rows[0] reach row0")
  assert(40, row1[1], "pushes through rows[1] reach row1")
  rows[1][0] = 31
  assert(31, row1[0], "an element write through rows[1]")
  assert(71, rows[1][0] + rows[1][1], "element reads through rows[1]")

  cur = rows[0]
  cur.push(3)
  assert(3, row0.length(), "a var holding an array ref pushes in place")
  assert(3, firstTwo(cur), "a held array ref binds an array parameter")
  assert(71, firstTwo(rows[1]), "an element binds an array parameter")
  assert(1, 0 + (rows[0] == row0), "an array ref equals its array")

  grids.set("g", row1)
  grids["g"].push(50)
  assert(3, row1.length(), "a map of array refs pushes in place")

  BroadcastChatMessage("nested arrays: ${pass}/${total}")
}
