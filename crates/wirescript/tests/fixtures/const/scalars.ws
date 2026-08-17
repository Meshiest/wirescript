// Differential: every value is computed twice, once at compile time through a
// `const mod` and once at runtime through the identical ordinary mod. Any
// disagreement is a const-evaluation bug. Run in game: build it, then read
// chat.

let start = ReadBrickGrid()

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

const mod cShift(n: int) -> int { return 1 << n }
mod rShift(n: int) -> int { return 1 << n }

const mod cMix(a: int, b: int) -> int { return a * 3 + b - 1 }
mod rMix(a: int, b: int) -> int { return a * 3 + b - 1 }

const mod cName(kind: string) -> string { return "evt_" .. kind }
mod rName(kind: string) -> string { return "evt_" .. kind }

const mod cCompose(kind: string) -> string { return cName(kind) .. "!" }
mod rCompose(kind: string) -> string { return rName(kind) .. "!" }

on start {
  var pass: int = 0
  var total: int = 0

  check(pass, total, cShift(4) == rShift(4), "shift")
  check(pass, total, cMix(7, 2) == rMix(7, 2), "mixed arithmetic")
  check(pass, total, cName("died") == rName("died"), "string concat")
  check(pass, total, cCompose("x") == rCompose("x"), "const mod calls const mod")

  BroadcastChatMessage("const scalars: " .. pass .. "/" .. total)
}
