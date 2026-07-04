var n: int = 0

on RoundStart {
  if (n > 0) {
    if (n > 10) {
      n = 100
    } else {
      n = 10
    }
  } else {
    n = -1
  }
}
