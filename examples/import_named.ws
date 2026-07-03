// Test: selective import with alias
import { clamp, swap as exchange } from "utils"

var a: int = 10
var b: int = 20

on RoundStart {
  exchange(a, b)
  clamp(a, 0, 100)
  clamp(b, 0, 100)
}

out va = a
out vb = b
