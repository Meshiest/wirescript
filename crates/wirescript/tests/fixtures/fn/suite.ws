// Functions and loops, checked in game. Paste it: it runs when the grid loads,
// prints a FAIL line for each check that does not hold, and a summary per
// section. The `expect` helpers are themselves `fn`s, so every check also
// exercises a call that writes outer storage (`pass`, `total`) through a
// handle.

var pass: int = 0
var total: int = 0

fn expectInt(want: int, got: int, label: string) {
  total += 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

fn expectFloat(want: float, got: float, label: string) {
  total += 1
  let ok = abs(want - got) < 0.001
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

fn expectStr(want: string, got: string, label: string) {
  total += 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want '${want}' got '${got}'")
  }
}

// --- functions -------------------------------------------------------------

fn double(x: int) -> int {
  return x * 2
}

fn addWithDefault(x: int, y: int = 10) -> int {
  return x + y
}

fn split(a: int, b: int) -> (sum: int, diff: int) {
  return (a + b, a - b)
}

fn area(r: { w: int, h: int }) -> int {
  return r.w * r.h
}

fn greet(name: string) -> string {
  return "hi " .. name
}

fn half(x: float) -> float {
  return x / 2.0
}

fn sign(x: int) -> int {
  if x < 0 { return -1 }
  if x > 0 { return 1 }
  return 0
}

fn quadruple(x: int) -> int {
  return double(double(x))
}

fn bump(v: *int) {
  *v = *v + 1
}

fn pushTwice(xs: int[], v: int) {
  xs.push(v)
  xs.push(v)
}

fn tally(m: Map<string, int>, k: string) {
  m.set(k, m.get(k).Value + 1)
}

fn fresh() -> int {
  var c: int = 0
  c += 1
  return c
}

var calls: int = 0
var log: int[]
var seen: Map<int, int>

fn record(x: int) -> int {
  calls += 1
  log.push(x)
  seen.set(x, calls)
  return calls
}

// --- functions with loops --------------------------------------------------

fn sumTo(n: int) -> int {
  var acc: int = 0
  for i in 0..n {
    acc = acc + i
  }
  return acc
}

var squares: int[]

fn fillSquares(n: int) {
  squares.clear()
  for i in 0..n {
    squares.push(i * i)
  }
}

fn countEven(xs: int[]) -> int {
  var n: int = 0
  for v in xs {
    if v % 2 == 0 { n += 1 }
  }
  return n
}

// --- refs, containers and select -------------------------------------------

type Pair = { a: int, b: int }
type Box = { v: *int }

var ra: int = 0
var rb: int = 0
var held: *int
var refs: (*int)[]
var row0: int[]
var row1: int[]
var rows: int[][]
var planes: int[][][]
var cursor: *int[]
var rowsByName: Map<string, int[]>
var refsByName: Map<string, *int>
var bools: bool[]
var floats: float[]
var strings: string[]
var vectors: vector[]
var colors: color[]
var pairs: Pair[]
var labels: Map<int, string>
var weights: Map<string, float>
var places: Map<string, vector>

fn sumRow(xs: int[]) -> int {
  var s: int = 0
  for v in xs {
    s += v
  }
  return s
}

fn pick(c: bool, a: int, b: int) -> int {
  return if c then a else b
}

fn setThrough(p: *int, v: int) {
  *p = v
}

// --- environment, bricks and components ------------------------------------

fn setTime(t: float) {
  Environment.Sky.TimeOfDay = t
}

fn readTime() -> float {
  return Environment.Sky.TimeOfDay
}

var target: brick
var hitBricks: brick[]
var brickTags: Map<brick, int>

fn paint(b: brick, c: color) {
  b.SetBrickProperty(Color, c)
}

fn colorOf(b: brick) -> string {
  return b.GetBrickProperty(Color).ToHex()
}

fn tagTarget(v: int) {
  brickTags.set(target, v)
}

var light: Component<PointLight>
var lights: Component<PointLight>[]
var lightByName: Map<string, Component<PointLight>>

fn attach(b: brick) -> Component<PointLight> {
  return b.AddComponent($Component/PointLight)
}

fn brighten(l: Component<PointLight>, by: float) {
  l.Brightness = l.Brightness + by
}

fn radiusOf(l: Component<PointLight>) -> float {
  return l.Radius
}

fn dimStored() {
  light.Brightness = 1.0
}

// --- the run ---------------------------------------------------------------

var counter: int = 0
var bucket: int[]
var hits: Map<string, int>
var nums: int[]
var names: string[]
var scores: Map<string, int>
var fires: int = 0

on Loaded() {
  fires += 1
  if fires > 1 { return }

  // Functions.
  let p0 = pass
  let t0 = total
  expectInt(14, double(7), "a single result")
  expectInt(15, addWithDefault(5), "an omitted argument takes its default")
  expectInt(8, addWithDefault(5, 3), "a given argument overrides the default")
  let s = split(10, 3)
  expectInt(13, s.sum, "first of two results")
  expectInt(7, s.diff, "second of two results")
  expectInt(12, area({ w: 3, h: 4 }), "a record argument")
  expectStr("hi bob", greet("bob"), "a string in and out")
  expectFloat(1.25, half(2.5), "a float in and out")
  expectInt(-1, sign(-5), "an early return, first branch")
  expectInt(1, sign(5), "an early return, second branch")
  expectInt(0, sign(0), "the fall-through return")
  expectInt(12, quadruple(3), "a fn calling a fn")
  bump(&counter)
  bump(&counter)
  expectInt(2, counter, "a *int parameter writes the caller's var")
  pushTwice(bucket, 9)
  expectInt(2, bucket.length(), "an array parameter is the caller's array")
  expectInt(9, bucket[1], "the pushed value")
  tally(hits, "a")
  tally(hits, "a")
  expectInt(2, hits.get("a").Value, "a map parameter is the caller's map")
  expectInt(1, fresh(), "a local var starts fresh, first call")
  expectInt(1, fresh(), "a local var starts fresh, second call")
  expectInt(1, record(5), "outer var through a handle, first call")
  expectInt(2, record(6), "outer var through a handle, second call")
  expectInt(2, log.length(), "outer array through a handle")
  expectInt(6, log[1], "the outer array's second element")
  expectInt(2, seen.get(6).Value, "outer map through a handle")
  BroadcastChatMessage("functions: ${pass - p0}/${total - t0}")

  // Functions with loops.
  let p1 = pass
  let t1 = total
  expectInt(45, sumTo(10), "a loop inside a fn, local accumulator")
  expectInt(0, sumTo(0), "a loop inside a fn, zero iterations")
  fillSquares(5)
  expectInt(5, squares.length(), "a loop inside a fn pushes onto an outer array")
  expectInt(16, squares[4], "the last square")
  nums.clear()
  nums.push(1)
  nums.push(2)
  nums.push(4)
  nums.push(7)
  expectInt(2, countEven(nums), "a loop over an array parameter")
  var inLoop: int = 0
  for i in 0..4 {
    inLoop = inLoop + double(i)
  }
  expectInt(12, inLoop, "a fn called from a loop body")
  BroadcastChatMessage("functions with loops: ${pass - p1}/${total - t1}")

  // Loops.
  let p2 = pass
  let t2 = total
  var sum: int = 0
  for i in 0..10 {
    sum = sum + i
  }
  expectInt(45, sum, "0..10")
  sum = 0
  for i in 3..7 {
    sum = sum + i
  }
  expectInt(18, sum, "3..7 excludes 7")
  sum = 0
  let stepped = for i in (0..10).step(3) {
    sum = sum + i
  }
  expectInt(18, sum, "a step visits 0, 3, 6, 9")
  expectInt(4, stepped.iterations, "a step's count is a ceiling divide")
  expectInt(0, 0 + stepped.aborted, "a finished loop is not aborted")
  var digits: int = 0
  for i in (0..4).rev() {
    digits = digits * 10 + i
  }
  expectInt(3210, digits, ".rev() counts down")
  sum = 0
  for i in count(4, from = 10, step = 5) {
    sum = sum + i
  }
  expectInt(70, sum, "count(4, from = 10, step = 5)")
  var none: int = 0
  for i in 5..5 {
    none += 1
  }
  for i in 10..0 {
    none += 1
  }
  expectInt(0, none, "empty and backwards ranges run no iterations")
  let halves = for i in (0..6).step(0.5) {
    none = none
  }
  expectInt(12, halves.iterations, "a float step")
  sum = 0
  for v in nums {
    sum = sum + v
  }
  expectInt(14, sum, "array values")
  sum = 0
  for i, v in nums {
    sum = sum + i * v
  }
  expectInt(31, sum, "array index and value")
  names.push("a")
  names.push("b")
  names.push("c")
  var joined: string = ""
  for n in names {
    joined = joined .. n
  }
  expectStr("abc", joined, "a string array in order")
  scores.set("x", 3)
  scores.set("y", 4)
  sum = 0
  var keys: int = 0
  for k, v in scores {
    sum = sum + v
    keys += 1
  }
  expectInt(7, sum, "map values")
  expectInt(2, keys, "map entries")
  sum = 0
  for i in 0..10 {
    if i == 4 { break }
    sum = sum + i
  }
  expectInt(6, sum, "break stops the loop")
  var cells: int = 0
  for r in 0..3 {
    for c in 0..4 {
      cells += 1
    }
  }
  expectInt(12, cells, "nested loops")
  var last: int = 0
  for i in 0..3 {
    var each: int = 0
    each += 1
    last = each
  }
  expectInt(1, last, "a var declared in a loop body resets every iteration")
  BroadcastChatMessage("loops: ${pass - p2}/${total - t2}")

  // Refs, containers and select.
  let p6 = pass
  let t6 = total
  held = &ra
  *held = 5
  expectInt(5, ra, "a write through a ref held in a var")
  expectInt(6, *held + 1, "a read through a ref held in a var")
  expectInt(1, 0 + (held == &ra), "a held ref equals a fresh ref to the same var")
  expectInt(0, 0 + (held == &rb), "a ref differs from a ref to another var")
  refs.push(&ra)
  refs.push(&rb)
  *refs[1] += 2
  expectInt(2, rb, "a write through a ref in an array")
  setThrough(refs[0], 11)
  expectInt(11, ra, "a ref from an array passed to a fn")
  refsByName.set("b", &rb)
  setThrough(refsByName.get("b").Value, 21)
  expectInt(21, rb, "a ref from a map passed to a fn")
  let box: Box = { v: &rb }
  *box.v = 4
  expectInt(4, rb, "a write through a ref in a record field")
  rows.push(row0)
  rows.push(row1)
  rows[1].push(7)
  expectInt(7, row1[0], "an array of arrays writes the inner array")
  cursor = rows[0]
  cursor.push(3)
  expectInt(3, row0[0], "a var holding an array ref")
  expectInt(7, sumRow(rows[1]), "an inner array passed to a fn")
  planes.push(rows)
  planes[0][1].push(8)
  expectInt(8, row1[1], "three levels of arrays")
  var lengths: int = 0
  for row in rows {
    lengths += row.length()
  }
  expectInt(3, lengths, "a loop over an array of arrays")
  rowsByName.set("one", row1)
  rowsByName.get("one").Value.push(9)
  expectInt(3, row1.length(), "a map of arrays writes the inner array")
  expectInt(3, pick(ra > 0, 3, 4), "select inside a fn, true branch")
  expectInt(4, pick(ra < 0, 3, 4), "select inside a fn, false branch")
  expectStr("big", if ra > 10 then "big" else "small", "select of strings")
  expectFloat(2.5, if rb > 0 then 2.5 else 0.5, "select of floats")
  let chosen = if ra > rb then &ra else &rb
  *chosen = 100
  expectInt(100, ra, "select of refs writes the chosen var")
  let pr: Pair = if rb > 0 then { a: 1, b: 2 } else { a: 3, b: 4 }
  expectInt(2, pr.b, "select of records")
  bools.push(true)
  floats.push(1.5)
  strings.push("s")
  vectors.push(Vec(1.0, 2.0, 3.0))
  colors.push(Color(0.0, 0.0, 1.0))
  pairs.push({ a: 5, b: 6 })
  expectInt(1, 0 + bools[0], "a bool array")
  expectFloat(1.5, floats[0], "a float array")
  expectStr("s", strings[0], "a string array")
  expectInt(1, 0 + (vectors[0] == Vec(1.0, 2.0, 3.0)), "a vector array")
  expectStr("0000FF", colors[0].ToHex(), "a color array")
  expectInt(6, pairs[0].b, "a record array")
  labels.set(1, "one")
  weights.set("w", 0.25)
  places.set("p", Vec(4.0, 5.0, 6.0))
  expectStr("one", labels.get(1).Value, "an int to string map")
  expectFloat(0.25, weights.get("w").Value, "a string to float map")
  expectInt(1, 0 + (places.get("p").Value == Vec(4.0, 5.0, 6.0)), "a string to vector map")
  expectInt(0, 0 + labels.get(2).Found, "a missing key is not found")
  BroadcastChatMessage("refs, containers and select: ${pass - p6}/${total - t6}")

  // Environment. Every setting is put back afterwards.
  let p3 = pass
  let t3 = total
  let tod = Environment.Sky.TimeOfDay
  Environment.Sky.TimeOfDay = 7.5
  expectFloat(7.5, Environment.Sky.TimeOfDay, "a float setting written and read")
  setTime(13.25)
  expectFloat(13.25, Environment.Sky.TimeOfDay, "a setting written from a fn")
  expectFloat(13.25, readTime(), "a setting read from a fn")
  let sky = Environment.Sky.SkyColor
  Environment.Sky.SkyColor = Color(1.0, 0.0, 0.0)
  expectStr("FF0000", Environment.Sky.SkyColor.ToHex(), "a color setting")
  Environment.Sky.SkyColor = sky
  Environment.Sky.TimeOfDay = tod
  expectFloat(tod, Environment.Sky.TimeOfDay, "the time of day is put back")
  BroadcastChatMessage("environment: ${pass - p3}/${total - t3}")

  // Bricks: the suite's own microchip brick. Its color is put back.
  let p4 = pass
  let t4 = total
  target = GetMicrochipBrick()
  expectInt(1, 0 + (target == GetMicrochipBrick()), "a stored brick equals GetMicrochipBrick()")
  let original = target.GetBrickProperty(Color)
  paint(target, Color(0.0, 1.0, 0.0))
  expectStr("00FF00", colorOf(target), "a fn recolors a brick it is passed")
  target.SetBrickProperty(Color, Color(0.0, 0.0, 1.0))
  expectStr("0000FF", target.GetBrickProperty(Color).ToHex(), "a brick color written and read")
  target.SetBrickProperty(Color, original)
  expectStr(original.ToHex(), colorOf(target), "the color is put back")
  hitBricks.push(target)
  hitBricks.push(GetMicrochipBrick())
  expectInt(2, hitBricks.length(), "a brick array")
  expectInt(1, 0 + (hitBricks[1] == target), "a brick array element equals the stored brick")
  brickTags.set(target, 5)
  tagTarget(7)
  expectInt(7, brickTags.get(target).Value, "a map keyed by brick, written from a fn")
  // A sweep straight down hits a brick: the suite's own, or the one under it.
  let r = Sweep(GetOwnTransform().Location, Vec(0.0, 0.0, -1.0), 5000.0, detectBricks = true, ignoreOwningGrid = true)
  if r.Hit {
    expectInt(1, 0 + (r.HitDistance > 0.0), "a sweep hit reports a distance")
    hitBricks.push(r.HitBrick)
    expectInt(3, hitBricks.length(), "a hit brick stored in a brick array")
  }
  if r.Miss {
    BroadcastChatMessage("SEE the downward sweep hit nothing")
  }
  BroadcastChatMessage("bricks: ${pass - p4}/${total - t4}")

  // Components, last: a gate given a reference it cannot use stops the
  // chain, so a missing `components` line means one of these halted.
  let p5 = pass
  let t5 = total
  let added = attach(target)
  added.Brightness = 5.0
  expectFloat(5.0, added.Brightness, "a property of a component a fn added")
  added.Radius = 250.0
  expectFloat(250.0, radiusOf(added), "a component passed to a fn")
  brighten(added, 2.0)
  expectFloat(7.0, added.Brightness, "a fn writes a component property")
  light = added
  dimStored()
  expectFloat(1.0, target.GetComponent($Component/PointLight).Brightness, "a fn writes a stored component through a handle")
  lights.push(light)
  lights[0].Brightness = 3.0
  expectFloat(3.0, light.Brightness, "a component in an array")
  lightByName.set("a", light)
  let viaMap = lightByName.get("a").Value
  viaMap.Color = Color(1.0, 0.0, 0.0)
  expectStr("FF0000", light.Color.ToHex(), "a component color through a map")
  target.RemoveComponent($Component/PointLight)
  BroadcastChatMessage("components: ${pass - p5}/${total - t5}")

  BroadcastChatMessage("fn and loop suite: ${pass}/${total}")
}
