/// Shared utility mods for wirescript examples.

mod inc(v: *int) {
  v = v + 1
}

mod dec(v: *int) {
  v = v - 1
}

mod clamp(v: *int, lo: int, hi: int) {
  if v < lo { v = lo }
  if v > hi { v = hi }
}

mod swap(a: *int, b: *int) {
  let tmp = a
  a = b
  b = tmp
}

fn double(x: int) -> int = x * 2
