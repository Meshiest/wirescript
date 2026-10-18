// Self-checking test for `for` loops. Paste into a grid and read the summary
// line in chat; only failures print. See docs/src/testing.md for the pattern.
//
// Every check that can be computed two ways is, so a check fails when the two
// paths disagree even if the expected value written here is wrong. The counted
// sums are compared against a closed form, not against a number typed by hand.

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

on Loaded() {

  // Counted ranges

  var sum: int = 0
  var runs: int = 0
  for i in 0..10 {
    sum = sum + i
    runs = runs + 1
  }
  assert(10 * 9 / 2, sum, "0..10 sums to the closed form")
  assert(10, runs, "0..10 runs ten times")

  // The end is excluded, so the last index is one below it.
  var last: int = -1
  for i in 0..5 {
    last = i
  }
  assert(4, last, "0..5 stops at 4")

  var firstSeen: int = -1
  var boundedRuns: int = 0
  for i in 3..7 {
    if boundedRuns == 0 { firstSeen = i }
    boundedRuns = boundedRuns + 1
  }
  assert(3, firstSeen, "3..7 starts at 3")
  assert(4, boundedRuns, "3..7 runs four times")

  // A stride changes how many iterations there are, so the count is a
  // ceiling divide. 0, 3, 6 and 9 are all below 10, so this runs four times,
  // not three.
  var stepRuns: int = 0
  var stepSum: int = 0
  for i in (0..10).step(3) {
    stepRuns = stepRuns + 1
    stepSum = stepSum + i
  }
  assert(4, stepRuns, "(0..10).step(3) runs four times")
  assert(0 + 3 + 6 + 9, stepSum, "(0..10).step(3) visits 0 3 6 9")

  var revFirst: int = -1
  var revLast: int = -1
  var revRuns: int = 0
  for i in (0..5).rev() {
    if revRuns == 0 { revFirst = i }
    revLast = i
    revRuns = revRuns + 1
  }
  assert(4, revFirst, "(0..5).rev() starts at 4")
  assert(0, revLast, "(0..5).rev() ends at 0")
  assert(5, revRuns, "(0..5).rev() runs five times")

  var countSum: int = 0
  for i in count(4, from = 10, step = 5) {
    countSum = countSum + i
  }
  assert(10 + 15 + 20 + 25, countSum, "count(4, from = 10, step = 5)")

  var emptyRuns: int = 0
  for i in 5..5 {
    emptyRuns = emptyRuns + 1
  }
  assert(0, emptyRuns, "an empty range runs zero times")

  var backwardsRuns: int = 0
  for i in 10..0 {
    backwardsRuns = backwardsRuns + 1
  }
  assert(0, backwardsRuns, "a backwards range runs zero times")

  // Entry-gate bindings

  // `index` is the counter value and respects `from`/`step`.
  var indexSum: int = 0
  for { index } in count(3, from = 10, step = 5) {
    indexSum = indexSum + index
  }
  assert(10 + 15 + 20, indexSum, "index respects from and step")

  // Arrays

  var xs: int[]
  xs.push(4)
  xs.push(9)
  xs.push(12345)

  var valueSum: int = 0
  for v in xs {
    valueSum = valueSum + v
  }
  assert(4 + 9 + 12345, valueSum, "for v in xs sums the elements")

  // 12345 cannot be confused with a default or an empty read, so a dead
  // channel cannot pass this.
  var control: int = 0
  for i, v in xs {
    if i == 2 { control = v }
  }
  assert(12345, control, "control value survives an indexed loop")

  // The index and value slots must agree with an ordinary indexed read.
  var mismatches: int = 0
  for i, v in xs {
    if v != xs[i] { mismatches = mismatches + 1 }
  }
  assert(0, mismatches, "i and v agree with xs[i]")

  // Maps

  var scores: Map<string, int>
  scores.set("a", 1)
  scores.set("b", 2)
  scores.set("c", 39)

  var mapValueSum: int = 0
  var mapEntries: int = 0
  for k, v in scores {
    mapValueSum = mapValueSum + v
    mapEntries = mapEntries + 1
  }
  assert(1 + 2 + 39, mapValueSum, "for k, v in scores sums the values")
  assert(3, mapEntries, "the map loop visits every entry")

  // A single binding on a map is the key, not the value.
  var keyLengths: int = 0
  for k in scores {
    keyLengths = keyLengths + k.Length()
  }
  assert(3, keyLengths, "a single map binding binds the key")

  // State across iterations

  // A var written in one iteration must read back as the new value in the
  // next, within the same tick. Without this an accumulator silently reads
  // its pre-loop value every pass.
  var acc: int = 0
  for i in 0..5 {
    acc = acc + 1
  }
  assert(5, acc, "a var accumulates across iterations")

  // Nesting and capture

  var outerLimit: int = 3
  var inner: int = 0
  for i in 0..outerLimit {
    for j in 0..4 {
      inner = inner + 1
    }
  }
  assert(12, inner, "a nested loop runs the product of its bounds")

  // The body reads a var declared outside the loop, which crosses the plane
  // boundary as a capture.
  var factor: int = 7
  var captured: int = 0
  for i in 0..3 {
    captured = captured + factor
  }
  assert(21, captured, "the body captures an outer var")

  // Loop result

  var seen: int = 0
  let run = for i in 0..6 {
    seen = seen + 1
  }
  assert(6, seen, "the loop ran six times")
  assert(6, run.iterations, "run.iterations agrees with the body count")
  assert(0, run.aborted, "an uninterrupted loop is not aborted")

  // break

  // `break` stops the loop after the current body sweep, so a sum taken
  // before it reflects only the iterations that ran before the break.
  var breakSum: int = 0
  for i in 0..10 {
    if i == 4 { break }
    breakSum = breakSum + i
  }
  assert(0 + 1 + 2 + 3, breakSum, "break stops a counted loop early")

  // The gate alone only stops FUTURE sweeps, so a statement after `break` in
  // the same body must be cut off some other way, or it would still run once.
  var afterBreak: int = 0
  for i in 0..5 {
    if i == 2 {
      break
      afterBreak = afterBreak + 1
    }
  }
  assert(0, afterBreak, "a statement after break in the same body does not run")

  // A deliberate `break` is a normal early exit, not an abort. `run.aborted`
  // reports the runtime cutting a loop off, such as an invocation limit, so
  // breaking leaves it false. Verified in game.
  var brokeRuns: int = 0
  let run = for i in 0..10 {
    if i == 3 { break }
    brokeRuns = brokeRuns + 1
  }
  assert(3, brokeRuns, "the loop stops after the break's iteration")
  assert(0, run.aborted, "a deliberate break is not an abort")

  BroadcastChatMessage("for loops: ${pass}/${total}")
}
