// Inline mod with ref params
var a: int = 5
var b: int = 10

/// Swap the values inside of two variables
mod swap(x: *int, y: *int) {
  let tmp = x
  x = y
  y = tmp
}

on RoundStart {
  swap(a, b)
  // Or use the built-in
  Swap(true, a, b)
}

out va = a.Value
out vb = b.Value
