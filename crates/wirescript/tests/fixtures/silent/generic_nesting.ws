// Guards generic nesting: a chip or mod declared inside a generic mod is typed
// per instantiation of every enclosing generic. Typing it once (as int) turns
// the string instantiation's value into 0 or "" with no diagnostic.

var pass: int = 0
var total: int = 0

mod assert<T: int | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

mod outer<T: int | string>(v: T) -> (r: T) {
  chip inner(x: T) -> (y: T) {
    out y = x
  }
  return inner(v)
}

mod twoLevel<T: int | string>(v: T, w: int) -> (r: T) {
  mod mid<U: int | bool>(t: T, u: U) -> (r2: T) {
    chip deep(x: T, z: U) -> (y: T) {
      out y = x
    }
    return deep(t, u)
  }
  return mid(v, w)
}

var shadowB: int = 0

mod shadow<T: int | string>(v: T) -> (r: T) {
  mod helper(x: T) -> (r2: T) {
    return x
  }
  mod B<T: int | bool>(u: T) -> (r3: T) {
    return u
  }
  let got = helper(v)
  shadowB = B(9)
  return got
}

var ints: int[]
var strs: string[]
var fires: int = 0

on Loaded() {
  fires += 1
  if fires > 1 { return }

  ints.push(5)
  ints.push(7)
  ints.push(3)
  strs.push("hi")
  strs.push("deep")
  strs.push("s")

  assert(5, outer(ints[0]), "chip in a generic mod, int")
  assert("hi", outer(strs[0]), "chip in a generic mod, string")
  assert(7, twoLevel(ints[1], 1), "chip two generics deep, int")
  assert("deep", twoLevel(strs[1], 1), "chip two generics deep, string")
  assert(3, shadow(ints[2]), "mod under a shadowing generic, int")
  assert("s", shadow(strs[2]), "mod under a shadowing generic, string")
  assert(9, shadowB, "the shadowing generic binds its own T")

  BroadcastChatMessage("generic nesting: ${pass}/${total}")
}
