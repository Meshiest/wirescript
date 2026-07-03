// Chips vs Mods demonstration
// chip {} = physical microchip (visual grouping in-game)
// mod name() {} = inline expansion (no microchip, like a macro)

// Variables in a chip — visually grouped in-game
chip {
  /// Team Health
  var health: int = 100
  /// Team Armor
  var armor: int = 50
}

// mod = inline function with ref params
// Expanded at each call site, no physical chip created

/// Clamp a variable between high and low
mod clamp(val: *int, lo: int, hi: int) {
  if val < lo { val = lo }
  if val > hi { val = hi }
}

// chip on = handler inside a physical microchip
chip on RoundStart {
  health = 100
  armor = 50
}

chip on CharacterDied(character) {
  health = health - 25
  armor = armor - 10
  clamp(health, 0, 100)
  clamp(armor, 0, 100)
}

out hp = health.Value
out ap = armor.Value
