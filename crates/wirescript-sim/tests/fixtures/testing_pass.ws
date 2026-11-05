// Every `testing.*` form, each written to pass: a test build checks them and
// a passing test prints nothing.
type Point = { x: int, y: float, tag: { name: string } }

fn half(n: float) -> float {
  return n / 2.0
}

@test fn asserts_hold() {
  testing.assert(half(3.0) == 1.5)
  testing.assert(half(4.0) > 1.0, "half of 4 is above 1")
  testing.assertEq(half(5.0), 2.5)
  testing.assertEq(half(1.0), 0.5, "half of 1")
}

@test mod expect_scalars() {
  var xs: int[]
  xs.push(3)
  xs.push(7)
  testing.expect(xs.length()).toBe(2)
  testing.expect(xs[0]).not.toBe(7)
  testing.expect(xs[1]).toEqual(7)
  testing.expect(xs[1]).toBeGreaterThan(xs[0])
  testing.expect(xs[0]).not.toBeGreaterThan(xs[1])
  testing.expect(xs[0]).toBeLessThan(4)
  testing.expect(xs[1]).not.toBeLessThan(7)
  testing.expect(half(1.0) + 0.001).toBeCloseTo(0.5)
  testing.expect(half(1.0) + 0.001).toBeCloseTo(0.5, digits = 2)
  testing.expect(half(1.0) + 0.001).not.toBeCloseTo(0.5, digits = 4)
}

@test mod expect_records() {
  let p: Point = { x: 1, y: 2.5, tag: { name: "a" } }
  let same: Point = { x: 1, y: 2.5, tag: { name: "a" } }
  testing.expect(p).toEqual(same)
  testing.expect(p).not.toEqual({ x: 1, y: 2.5, tag: { name: "b" } })
}

@test mod expect_contents() {
  var names: string[]
  names.push("ada")
  names.push("bob")
  testing.expect("wirescript").toContain("script")
  testing.expect("wirescript").not.toContain("Script")
  testing.expect(names).toContain("bob")
  testing.expect(names).not.toContain("cy")
}
