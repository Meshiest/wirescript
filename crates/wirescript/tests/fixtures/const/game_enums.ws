// The built-in game enum EasingFunction: its variant-path discriminant is the
// schema integer, checked folded (const) and read back off a stored value.
let start = ReadBrickGrid()

mod check(pass: *int, total: *int, ok: bool, label: string) {
  total = total + 1
  pass = pass + ok
  if !ok { BroadcastChatMessage("FAIL: " .. label) }
}

const BOUNCE = EasingFunction.Bounce.Discriminant

on start {
  var pass: int = 0
  var total: int = 0
  static var e: EasingFunction = EasingFunction.Bounce
  check(pass, total, e.Discriminant == BOUNCE, "runtime disc == const disc")
  check(pass, total, BOUNCE == EasingFunction.Bounce.Discriminant, "const disc stable")
  BroadcastChatMessage("game enums: " .. pass .. "/" .. total)
}
