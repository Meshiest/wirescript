// Damage, ammo, inventory, aim, hints and temporary permissions, driven by
// world_character.script.
on ChatCommand("hurt", "Damages you") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.SetDamage(30.0)
  c.IncDamage(15.5)
  let d = c.GetDamage()
  BroadcastChatMessage("damage ${d.Damage} of ${d.DamageLimit}")
  c.IncDamage(500.0)
  let capped = c.GetDamage().Damage
  c.SetDamage(-5.0)
  let healed = c.GetDamage().Damage
  BroadcastChatMessage("capped ${capped} healed ${healed}")
}

on ChatCommand("ammo", "Counts ammo") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.SetAmmo($BRItemBase/Weapon_Pistol, 10)
  c.GrantAmmo($BRItemBase/Weapon_Pistol, 5)
  c.GrantAmmo($BRItemBase/Weapon_Bow, 2)
  c.SetWeaponChamberAmmo($BRItemBase/Weapon_Pistol, 0, 6)
  c.IncWeaponChamberAmmo($BRItemBase/Weapon_Pistol, 0, -2)
  let pistol = c.GetAmmo($BRItemBase/Weapon_Pistol)
  let bow = c.GetAmmo($BRItemBase/Weapon_Bow)
  let chamber = c.GetWeaponChamberAmmo($BRItemBase/Weapon_Pistol, 0)
  let other = c.GetWeaponChamberAmmo($BRItemBase/Weapon_Pistol, 1)
  BroadcastChatMessage("ammo ${pistol} ${bow} chamber ${chamber} ${other}")
}

on ChatCommand("inv", "Fills your inventory") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.GiveWeapon($BRItemBase/Weapon_Pistol, 0)
  c.SetInventoryItem($BRItemBase/Weapon_Bow, 1)
  c.AddInventoryItem($BRItemBase/Weapon_Bow)
  c.AddInventoryItemAdv($BRItemBase/Weapon_Bazooka, damage = 2.0)
  c.SetInventoryItemAdv($BRItemBase/Weapon_Bazooka, slot = 5)
  let pistol = c.GetInventoryEntry(0)
  let bow = c.GetInventoryEntry(1)
  let bow2 = c.GetInventoryEntry(2)
  let bazooka = c.GetInventoryEntry(3)
  let bazooka2 = c.GetInventoryEntry(5)
  let none = c.GetInventoryEntry(4)
  let none2 = c.GetInventoryEntry(9)
  BroadcastChatMessage("same ${bow.Item == bow2.Item} ${bazooka.Item == bazooka2.Item} ${none.Item == none2.Item}")
  BroadcastChatMessage("differ ${pistol.Item != bow.Item} ${bow.Item != none.Item}")
  BroadcastChatMessage("slot ${c.GetCurrentInventorySlot()}")
}

on ChatCommand("aim", "Says where you aim") -> (controller, arguments) {
  let aim = controller.CharacterOf().GetAim()
  BroadcastChatMessage("aim from ${aim.Origin} toward ${aim.Direction}")
}

on ChatCommand("hint", "Shows a hint") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.ShowHint("Welcome", "Press E to interact")
  c.SetTempPermission("Fly", true)
  c.SetTempPermission("Fly", false)
}
