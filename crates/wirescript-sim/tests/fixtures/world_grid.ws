// Whole-grid interaction and targeting, driven by world_grid.script.
on WholeGridInteracted() -> (character, held) {
  BroadcastChatMessage("interacted, held ${held}")
}

on WholeGridTargeted() -> (character, damage, weapon, weaponName) {
  BroadcastChatMessage("${character.ControllerOf().GetUserName()} hit the grid for ${damage} with '${weaponName}'")
}
