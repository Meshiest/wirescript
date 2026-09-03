// Aggregate-storage self-check: record VARIABLES, ARRAYS, and MAPS.
//
// A record decomposes into one backing gate per field, and every operation fans
// out across those fields. This program exercises the whole surface and asserts
// the runtime values, so it catches a field wired to the wrong gate, a mutation
// that skipped a field, or two fields that fell out of lockstep - none of which
// type checking can see. Paste it into a grid: silence (plus the summary line)
// is a pass; any `FAIL:` line names the check and prints want vs got.
//
// Compile-verified with `just compile`; run in game for the value checks.

type Point = { x: int, y: int }
type Mixed = { n: int, f: float, s: string }
type Line = { a: Point, b: Point }

enum Which { First, Second }
enum Shape { Empty, Circle(float), Rect(float, float) }

// Set to `Opaque(true)` in the handler, so the record/enum choices below are
// made at RUNTIME by a Select rather than folded to one arm at compile time.
var takeA: bool = false

var pass: int = 0
var total: int = 0

// Constant record-array / record-map constructors bake per-field columns.
var cpts: Point[] = [{ x: 11, y: 12 }, { x: 13, y: 14 }]
var cmap: Map<int, Point> = { 5 => { x: 50, y: 60 } }

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

// `bool` doesn't stringify, so this variant reports the label only.
mod assertBool(want: bool, got: bool, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label}")
  }
}

on ReadBrickGrid() {
  takeA = Opaque(true)

  // ---------- record VARIABLE ----------
  var p: Point = { x: 3, y: 5 }
  assert(3, p.x, "var field x init")
  assert(5, p.y, "var field y init")

  p.x = 10                                    // single-field write
  assert(10, p.x, "var field x write")
  assert(5, p.y, "var field y left alone by x write")

  p = { x: 7, y: 8 }                          // whole-record literal assign
  assert(7, p.x, "whole-record literal assign x")
  assert(8, p.y, "whole-record literal assign y")

  var q: Point = { x: 1, y: 2 }
  p = q                                       // record-to-record assign
  assert(1, p.x, "record-to-record assign x")
  assert(2, p.y, "record-to-record assign y")
  q.x = 100                                   // assign copies, does not alias
  assert(1, p.x, "record assign is a copy, not an alias")

  // typed fields: int / float / string in one record
  var mx: Mixed = { n: 42, f: 1.5, s: "hi" }
  assert(42, mx.n, "mixed int field")
  assert(1.5, mx.f, "mixed float field")
  assert("hi", mx.s, "mixed string field")
  mx.s = "bye"
  assert("bye", mx.s, "mixed string field write")
  assert(42, mx.n, "string write left int field alone")

  // nested record: each leaf is its own gate
  var ln: Line = { a: { x: 1, y: 2 }, b: { x: 3, y: 4 } }
  assert(1, ln.a.x, "nested a.x init")
  assert(4, ln.b.y, "nested b.y init")
  ln.a.x = 9
  assert(9, ln.a.x, "nested a.x write")
  assert(3, ln.b.x, "nested write left the other sub-record alone")

  // ---------- record ARRAY (parallel per-field arrays) ----------
  var pts: Point[]
  pts.push({ x: 10, y: 20 })
  pts.push({ x: 30, y: 40 })
  assert(2, pts.length(), "array length after two pushes")
  assert(10, pts[0].x, "array elem 0 field x")
  assert(20, pts[0].y, "array elem 0 field y")
  assert(30, pts[1].x, "array elem 1 field x")
  assert(40, pts[1].y, "array elem 1 field y")

  var e: Point = { x: 0, y: 0 }
  e = pts[1]                                  // whole element read into a var
  assert(30, e.x, "whole element read x")
  assert(40, e.y, "whole element read y")

  pts[0].x = 99                               // element field write
  assert(99, pts[0].x, "array elem field write")
  assert(20, pts[0].y, "array elem field write kept sibling field")

  pts[1] = { x: 5, y: 6 }                     // whole element write
  assert(5, pts[1].x, "whole element write x")
  assert(6, pts[1].y, "whole element write y")
  assert(99, pts[0].x, "whole element write left row 0 alone")

  pts.insert(0, { x: 100, y: 200 })           // insert shifts every field's array
  assert(3, pts.length(), "length after insert")
  assert(100, pts[0].x, "inserted row x")
  assert(200, pts[0].y, "inserted row y")
  assert(99, pts[1].x, "insert shifted old row 0 to 1 (x)")
  assert(20, pts[1].y, "insert kept the shifted row's fields together")

  pts.remove(0)                               // remove shifts back
  assert(2, pts.length(), "length after remove")
  assert(99, pts[0].x, "remove shifted rows back (x)")
  assert(20, pts[0].y, "remove kept the row's fields together")

  pts.fill({ x: 7, y: 8 })                    // fill every row with the same record
  assert(7, pts[0].x, "fill row 0 x")
  assert(8, pts[0].y, "fill row 0 y")
  assert(7, pts[1].x, "fill row 1 x")
  assert(8, pts[1].y, "fill row 1 y")

  // reverse mirrors deterministically, so each row's fields stay together
  var rp: Point[]
  rp.push({ x: 1, y: 11 })
  rp.push({ x: 2, y: 22 })
  rp.push({ x: 3, y: 33 })
  rp.reverse()
  assert(3, rp[0].x, "reverse row 0 x")
  assert(33, rp[0].y, "reverse kept x and y together on row 0")
  assert(2, rp[1].x, "reverse row 1 x")
  assert(1, rp[2].x, "reverse row 2 x")
  assert(11, rp[2].y, "reverse kept x and y together on row 2")

  // swap moves whole rows - both fields of each endpoint travel together
  var sp: Point[]
  sp.push({ x: 1, y: 10 })
  sp.push({ x: 2, y: 20 })
  sp.swap(0, 1)
  assert(2, sp[0].x, "swap row 0 x")
  assert(20, sp[0].y, "swap kept x and y together on row 0")
  assert(1, sp[1].x, "swap row 1 x")

  // pop removes the last row and returns it as a record
  var popped: Point = { x: 0, y: 0 }
  popped = sp.pop()
  assert(1, popped.x, "pop returned row x")
  assert(10, popped.y, "pop returned row y")
  assert(1, sp.length(), "length after pop")

  // resize grows/shrinks every field array, filling new rows with the record
  var rz: Point[]
  rz.push({ x: 5, y: 6 })
  rz.resize(3, { x: 7, y: 8 })
  assert(3, rz.length(), "length after resize grow")
  assert(5, rz[0].x, "resize kept the existing row")
  assert(7, rz[2].x, "resize filled a new row x")
  assert(8, rz[2].y, "resize filled a new row y")
  rz.resize(1, { x: 0, y: 0 })
  assert(1, rz.length(), "length after resize shrink")

  pts.clear()
  assert(0, pts.length(), "length after clear")

  // ---------- record MAP (parallel per-field maps) ----------
  var mp: Map<int, Point>
  mp.set(1, { x: 11, y: 21 })
  mp.set(2, { x: 12, y: 22 })
  assert(2, mp.length(), "map length after two sets")
  assert(11, mp.get(1).x, "map get(1).x")
  assert(21, mp.get(1).y, "map get(1).y")
  assert(22, mp.get(2).y, "map get(2).y")
  assert(11, mp[1].x, "map subscript [1].x")

  var mv: Point = { x: 0, y: 0 }
  mv = mp.get(2)                              // whole value read
  assert(12, mv.x, "map whole value read x")
  assert(22, mv.y, "map whole value read y")

  mp[1] = { x: 99, y: 88 }                    // subscript write
  assert(99, mp[1].x, "map subscript write x")
  assert(88, mp[1].y, "map subscript write y")
  assert(12, mp[2].x, "map subscript write left key 2 alone")

  assertBool(true, mp.has(1), "map has present key")
  assertBool(false, mp.has(5), "map has absent key")

  mp.remove(1)
  assertBool(false, mp.has(1), "map has after remove")
  assert(1, mp.length(), "map length after remove")

  // keys(dest) fills a scalar array with the (shared) keys of every field map
  var km: Map<int, Point>
  km.set(3, { x: 1, y: 1 })
  km.set(7, { x: 2, y: 2 })
  var kk: int[]
  km.keys(kk)
  assert(2, kk.length(), "keys filled both entries")
  assert(10, kk[0] + kk[1], "keys are the two set keys (3 + 7)")

  mp.clear()
  assert(0, mp.length(), "map length after clear")

  // ---------- constructor literals (baked per field) ----------
  assert(2, cpts.length(), "ctor array length")
  assert(11, cpts[0].x, "ctor array [0].x baked")
  assert(12, cpts[0].y, "ctor array [0].y baked")
  assert(13, cpts[1].x, "ctor array [1].x baked")
  assert(14, cpts[1].y, "ctor array [1].y baked")
  assert(1, cmap.length(), "ctor map length")
  assert(50, cmap.get(5).x, "ctor map get(5).x baked")
  assert(60, cmap[5].y, "ctor map [5].y baked")

  // ---------- struct-of-arrays access ----------
  // `soa.field` IS that field's parallel array: index it, read/aggregate it.
  var soa: Point[]
  soa.push({ x: 5, y: 100 })
  soa.push({ x: 2, y: 200 })
  soa.push({ x: 8, y: 300 })
  assert(3, soa.x.length(), "SoA x-array length matches the container")
  assert(15, soa.x.sum(), "SoA x-array sum (5+2+8)")
  // min/max/find return multi-output records ({Value,IsEmpty} / {Index,Found});
  // binding to an int var auto-unwraps to the matching field.
  var soaMax: int = 0
  var soaMin: int = 0
  var soaFind: int = 0
  soaMax = soa.x.max()
  soaMin = soa.x.min()
  soaFind = soa.x.find(2)
  assert(8, soaMax, "SoA x-array max")
  assert(2, soaMin, "SoA x-array min")
  assert(1, soaFind, "SoA x-array find returns the row index")
  assert(8, soa.x[2], "SoA x-array index")
  assert(200, soa.y[1], "SoA y-array index reads the right column")

  // sort on a field sorts the WHOLE record by that field (rows stay intact)
  var srt: Point[]
  srt.push({ x: 30, y: 1 })
  srt.push({ x: 10, y: 2 })
  srt.push({ x: 20, y: 3 })
  srt.x.sort()
  assert(10, srt[0].x, "sort by x ascending: row 0 x")
  assert(2, srt[0].y, "sort by x carried y along: row 0 y")
  assert(30, srt[2].x, "sort by x ascending: row 2 x")
  assert(1, srt[2].y, "sort by x carried y along: row 2 y")

  // ---------- record CHOSEN per leaf field ----------
  // A record has no single wire, so an `if`/`match` over records picks each
  // leaf independently. The failure this catches is a choice that silently
  // wired only the first field, or dropped the whole statement.
  var ca: Point = { x: 71, y: 72 }
  var cb: Point = { x: 81, y: 82 }
  var chosen: Point = { x: 0, y: 0 }

  chosen = if takeA then ca else cb
  assert(71, chosen.x, "if-expr record choice: then-side x")
  assert(72, chosen.y, "if-expr record choice: then-side y")
  chosen = if !takeA then ca else cb
  assert(81, chosen.x, "if-expr record choice: else-side x")
  assert(82, chosen.y, "if-expr record choice: else-side y")

  chosen = match Which.First { First => ca, Second => cb }
  assert(71, chosen.x, "match-expr record arm: first arm x")
  assert(72, chosen.y, "match-expr record arm: first arm y")
  var w: Which = Which.Second
  chosen = match w { First => ca, Second => cb }
  assert(81, chosen.x, "match-expr record arm: second arm x")
  assert(82, chosen.y, "match-expr record arm: second arm y")

  // The same choice bound to a `let` and then read by field.
  let picked = match w { First => ca, Second => cb }
  assert(81, picked.x, "match-expr record bound to a let: x")
  assert(82, picked.y, "match-expr record bound to a let: y")

  // Enum VALUE arms: the leaves are the tag plus each payload slot, so the
  // chosen variant must carry its own payload, not the other arm's.
  var shape: Shape = Shape.Empty
  shape = match w { First => Shape.Circle(2.5), Second => Shape.Rect(3.0, 4.0) }
  assert(12.0, match shape { Circle(r) => r * 2.0, Rect(rw, rh) => rw * rh, Empty => 0.0 },
         "match-expr enum arm: second arm keeps its own payload")
  w = Which.First
  shape = match w { First => Shape.Circle(2.5), Second => Shape.Rect(3.0, 4.0) }
  assert(5.0, match shape { Circle(r) => r * 2.0, Rect(rw, rh) => rw * rh, Empty => 0.0 },
         "match-expr enum arm: first arm keeps its own payload")

  BroadcastChatMessage("aggregate checks: ${pass}/${total}")
}
