// Asset references: `$Type/Name` points at a game asset: a weapon/item
// (`$BRItemBase/...`), a one-shot sound (`$BrickOneShotAudioDescriptor/...`),
// an entity type, a brick, a font, and so on. In the editor, type `$` to
// complete the asset types, then `$Type/` for that type's names.
//
// Two ways to use one:
//   - as a VALUE: an asset is an `entity`, so you can compare it or store it.
//     It becomes a "reference" gate that outputs the asset as a wire.
//   - as an ARGUMENT: pass it to a gate that takes an asset (PlayAudioAt, the
//     inventory builtins, ...), where it inlines into the gate's data.
//
// Demo: when a player is meleed with the Pickaxe, ding a sound at them and
// slip a Bandage into their inventory. Also track the last hit and expose
// whether it was a pickaxe as an output.

static var lastWeapon: entity

on CharacterDamaged(victim, damage, attacker, weapon, weaponName) {
  lastWeapon = weapon

  // `weapon` is the attacker's weapon (an entity); compare it to an item asset.
  if weapon == $BRItemBase/Weapon_Pickaxe {
    victim.PlayAudioAt($BrickOneShotAudioDescriptor/OBA_Beep_1, volume = 1.0)
    victim.AddInventoryItem($BRItemBase/Weapon_Bandage)
  }
}

// An asset compared as a value produces a bool. Expose it as an output.
out meleedByPickaxe = lastWeapon == $BRItemBase/Weapon_Pickaxe
