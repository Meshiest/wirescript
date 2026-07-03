var n: int = 0

on RoundStart {
  n = n + 1
  if (n > 10) {
    n = 0
  } else {
    n = n + 1
  }
  n = n * 2
}
