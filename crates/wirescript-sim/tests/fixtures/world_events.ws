// Spawning, damage, weapon fire and death, driven by world_events.script.
var bodies: int = 0

on CharacterSpawned() -> (character) {
  bodies += 1
  BroadcastChatMessage("${character.ControllerOf().GetUserName()} spawned, body ${bodies}")
}

on CharacterDamaged() -> (character, damage, attacker, attackerWeapon, attackerWeaponName) {
  let by = attacker.ControllerOf().GetUserName()
  BroadcastChatMessage("${character.ControllerOf().GetUserName()} took ${damage} from '${by}', now ${character.GetDamage().Damage}")
}

on CharacterFiredWeapon() -> (character, direction, start, weapon, weaponName) {
  BroadcastChatMessage("${character.ControllerOf().GetUserName()} fired '${weaponName}' from ${start} toward ${direction}")
}

on CharacterDied() -> (character, killer, killerWeapon, killerWeaponName) {
  let by = killer.ControllerOf().GetUserName()
  BroadcastChatMessage("${character.ControllerOf().GetUserName()} died to '${by}'")
}

on ChatCommand("body", "Says whether you have the body you spawned with") -> (controller, arguments) {
  BroadcastChatMessage("damage ${controller.CharacterOf().GetDamage().Damage}")
}
