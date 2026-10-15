/// Simple counter that increments each time `go` fires
var count: int = 0

in go: exec
on go {
  count += 1
}

out total: int = count
