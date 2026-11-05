// Every `testing.*` form, each written to fail: the expected chat is the line
// each one prints, with this file's name and the statement's line.
type Point = { x: int, tag: { name: string } }

on Loaded() {
  var xs: int[]
  xs.push(3)
  let p: Point = { x: 1, tag: { name: "a" } }
  testing.log("started with", xs.length(), "element")
  testing.fail("always", 1)
  testing.fail()
  testing.assert(xs.length() == 2)
  testing.assert(false, "the message", xs[0])
  testing.assertEq(xs[0], 4)
  testing.assertEq(xs[0], 4, "first")
  testing.expect(xs[0]).toBe(5)
  testing.expect(xs[0]).not.toBe(3)
  testing.expect(p).toEqual({ x: 2, tag: { name: "b" } })
  testing.expect(p).not.toEqual(p)
  testing.expect(xs[0]).toEqual(9)
  testing.expect(xs[0]).toBeGreaterThan(3)
  testing.expect(xs[0]).not.toBeLessThan(4)
  testing.expect(1.5).toBeCloseTo(1.0)
  testing.expect(1.001).not.toBeCloseTo(1.0)
  testing.expect("abc").toContain("d")
  testing.expect("abc").not.toContain("b")
  testing.expect(xs).toContain(4)
  testing.expect(xs).not.toContain(3)
}
