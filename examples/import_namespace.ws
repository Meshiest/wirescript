// Test: namespace import
import * as u from "utils"

var x: int = 0
var y: int = 50

on RoundStart {
  u.inc(x)
  u.dec(y)
  u.clamp(x, 0, 100)
  u.clamp(y, 0, 100)
}

out rx = x
out ry = y
