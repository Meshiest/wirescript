// Guards record columns: a record whose fields hold arrays and maps of records
// is stored one column per field. A dropped, swapped or unmoved column still
// compiles cleanly and only shows up as a wrong value here.

var pass: int = 0
var total: int = 0

mod assert<T: int | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

type P = { x: int, y: int }
type Pts = P[]
type Arr<T> = T[]
type H = { pts: P[], m: Map<string, P>, xs: int[], ys: int[], n: int }
type Q = { p: P, k: int }

var h: H
var h2: H
var al: Pts
var ga: Arr<P>
var qs: Q[]
var fires: int = 0

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  h.pts.push({ x: 1, y: 10 })
  h.pts.push({ x: 2, y: 20 })
  h.pts.push({ x: 3, y: 30 })
  assert(3, h.pts.length(), "push onto a record-array field")
  assert(20, h.pts[1].y, "element field read")
  assert(3, h.pts.x[2], "column read")
  h.pts.x[0] = 5
  assert(5, h.pts[0].x, "column write")
  assert(10, h.pts[0].y, "column write leaves the sibling column")
  h.pts[1] = { x: 7, y: 70 }
  assert(70, h.pts.y[1], "whole element write (y)")
  assert(7, h.pts.x[1], "whole element write (x)")
  let last = h.pts.pop()
  assert(30, last.y, "pop returns the last element (y)")
  assert(3, last.x, "pop returns the last element (x)")
  assert(2, h.pts.length(), "pop shrinks every column")
  h.pts.insert(0, { x: 9, y: 90 })
  assert(90, h.pts[0].y, "insert at the front (y)")
  assert(9, h.pts[0].x, "insert at the front (x)")
  assert(5, h.pts[1].x, "insert shifts the rest (x)")
  assert(10, h.pts[1].y, "insert shifts the rest (y)")
  h.pts.remove(0)
  assert(5, h.pts[0].x, "remove at the front (x)")
  assert(10, h.pts[0].y, "remove at the front (y)")

  h.pts.y.sort(true)
  assert(70, h.pts[0].y, "sort by a column moves whole rows (y)")
  assert(7, h.pts[0].x, "sort by a column moves whole rows (x)")
  assert(5, h.pts[1].x, "sort by a column moves whole rows (x, second row)")

  h.xs.push(3)
  h.xs.push(1)
  h.ys.push(100)
  h.ys.push(200)
  h.xs.sort()
  assert(1, h.xs[0], "sorting one array field")
  assert(100, h.ys[0], "sorting one array field leaves its sibling alone")

  h.m["a"] = { x: 4, y: 40 }
  assert(40, h.m["a"].y, "map field of records")
  h.m.x["a"] = 6
  assert(6, h.m["a"].x, "map column write")
  assert(40, h.m["a"].y, "map column write leaves the sibling column")

  h2 = { ...h, pts: [{ x: 11, y: 12 }], xs: [8, 9], n: 3 }
  assert(1, h2.pts.length(), "whole literal assign rebuilds a record-array field")
  assert(11, h2.pts[0].x, "whole literal assign element (x)")
  assert(12, h2.pts[0].y, "whole literal assign element (y)")
  assert(9, h2.xs[1], "whole literal assign rebuilds an int-array field")
  assert(200, h2.ys[1], "spread copies an array field it does not name")
  assert(3, h2.n, "whole literal assign scalar field")
  h2 = { ...h2, n: 4 }
  assert(4, h2.n, "spread update sets the named field")
  assert(1, h2.pts.length(), "spread update keeps the other fields")
  assert(11, h2.pts[0].x, "spread update keeps a record column (x)")
  assert(12, h2.pts[0].y, "spread update keeps a record column (y)")
  assert(9, h2.xs[1], "spread update keeps an array field")

  al.push({ x: 1, y: 2 })
  assert(2, al[0].y, "aliased record array")
  ga.push({ x: 3, y: 4 })
  assert(4, ga[0].y, "generic-aliased record array")

  qs.push({ p: { x: 2, y: 0 }, k: 20 })
  qs.push({ p: { x: 1, y: 0 }, k: 10 })
  qs.p.x.sort()
  assert(10, qs[0].k, "sorting a nested column moves the whole row")
  assert(1, qs[0].p.x, "sorting a nested column sorts the key")

  BroadcastChatMessage("record columns: ${pass}/${total}")
}
