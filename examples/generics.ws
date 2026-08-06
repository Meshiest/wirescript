// Generic mods -- verified against `just check` (see docs/wirescript/types.md,
// "Generics" section, for the full writeup of what's shipped).

// Unbounded `<T>` == `<T: Variant>` -- any wire value variant.
mod pick<T>(c: bool, a: T, b: T) -> T {
  return if c then a else b
}

// Multiple type params -- each inferred independently from its own arguments.
mod first<T, U>(a: T, b: U) -> T {
  return a
}

// `<T: Numeric>` = int, float, vector, rotator, quat, color. The body is
// checked against EVERY member of the bound, so it must typecheck for all of
// them -- `v * v` does (same-type multiply is defined for the whole family).
mod square<T: Numeric>(v: T) -> T {
  return v * v
}

// `<T: Scalar>` = int, float only -- use this instead of `Numeric` when the
// body needs an operator (like `<`/`>` against another `T`, or `+ 1`) that
// isn't defined for vector/rotator/quat/color.
mod clamp<T: Scalar>(v: T, lo: T, hi: T) -> T {
  if v < lo { return lo }
  if v > hi { return hi }
  return v
}

// Ref param (`*T` / `ref T`) -- T is inferred through the reference. The body
// does arithmetic, so it needs the `Scalar` bound for the same reason as
// `clamp` above (unbounded `<T>` would have to typecheck `v + 1` for string,
// entity, ... too, and fail).
mod inc<T: Scalar>(v: *T) {
  v = v + 1
}

// A ref-param mod whose body only assigns (no arithmetic) stays unbounded --
// valid for every Variant member.
mod swap<T>(a: *T, b: *T) {
  let tmp = a
  a = b
  b = tmp
}

// Anonymous union bound: `<T: A | B>` restricts T to exactly that set.
mod pickAxis<T: int | vector>(v: T) -> T {
  return v
}

// `self`-receiver (UFCS): a mod whose FIRST parameter is named `self` is
// callable with method syntax -- `v.lengthSq()` desugars to `lengthSq(v)`.
// Builtin receiver-methods (e.g. vector `.Dot`) still win, so a self-mod may
// not shadow one on the same receiver type.
mod lengthSq(self: vector) -> float {
  return self.Dot(self)
}

// The receiver drives generic inference too: `T` here is pinned by whatever
// value the method is called on.
mod echo<T>(self: T) -> T {
  return self
}

in go: exec
in i: int
in f: float
in vec: vector
in ch: character
in ctrl: controller

var counter: int = 0
var ratio: float = 0.0
var lo: int = 1
var hi: int = 2
var foo: Dict<string, int> = { "bar": 5 }

on go {
  let a = pick(true, i, i) // T = int
  let widened = pick(true, i, f) // T = float -- numeric widening (int -> float)
  let obj = pick(true, ch, ctrl) // T = entity -- object widening (character/controller -> entity)

  let both = first(i, f) // T = int, U = float
  let sq = square(i) // T = int
  let sqv = square(vec) // T = vector
  let c = clamp(i, 0, 10) // T = int

  inc(counter) // T = int
  inc(ratio) // T = float
  swap(lo, hi) // T = int

  let p1 = pickAxis(i) // T = int
  let p2 = pickAxis(vec) // T = vector
}
