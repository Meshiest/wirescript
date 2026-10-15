// Inline mod with ref params
var a: int = 5
var b: int = 10

/// Swap the values inside of two variables
mod swap(x: *int, y: *int) {
  let tmp = x
  x = y
  y = tmp
}

in go: exec
on go {
  swap(a, b)
}

out va: int = a
out vb: int = b
