// Array operations
array items: int[]

on RoundStart {
  items.clear()
  items.push(10)
  items.push(20)
  items.push(30)
  items[1] = 99

  /// Length of an array
  let len = items.length()
  /// First item in the array
  let first = items[0]
  let second = items[1]
}
