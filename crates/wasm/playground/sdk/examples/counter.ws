/// Simple counter that increments each round
var count: int = 0

on RoundStart {
  count = count + 1
}

out total = count.Value
