// Permissions, roles, trust, respawning, status messages, boxes and
// FindPlayer, driven by world_player.script.
on ControllerJoined() -> (controller, userId, userName) {
  let build = controller.HasPermission("Build")
  let admin = controller.HasRole("Admin")
  BroadcastChatMessage("${userName} build ${build} admin ${admin} trusted ${controller.IsTrusted()}")
}

on ChatCommand("respawn", "Respawns you") -> (controller, arguments) {
  let before = controller.CharacterOf()
  controller.SetCanRespawn(false)
  controller.ForceRespawn()
  BroadcastChatMessage("new body ${before != controller.CharacterOf()}")
}

on CharacterSpawned() -> (character) {
  BroadcastChatMessage("${character.ControllerOf().GetUserName()} spawned")
}

on ChatCommand("ask", "Asks you things") -> (controller, arguments) {
  controller.ShowStatusMessage("Thinking")
  controller.ShowMessageBox("Hello there", title = "Hi")
  controller.ShowOkCancelBox("Continue?", title = "Next")
  controller.ShowYesNoBox("Ready?", title = "Start")
  controller.ShowTextInputBox("Your name?", title = "Name")
}

on ChatCommand("find", "Finds a player") -> (controller, arguments) {
  let found = FindPlayer(arguments)
  BroadcastChatMessage("find '${arguments}': ${found == controller} '${found.GetUserName()}'")
}
