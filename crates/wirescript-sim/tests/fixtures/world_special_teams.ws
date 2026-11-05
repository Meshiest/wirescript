// The builder and unaffiliated teams, driven by world_special_teams.script.
on ControllerJoined() -> (controller, userId, userName) {
  let team = controller.CharacterOf().GetTeam()
  BroadcastChatMessage("${userName}: builder ${team.IsBuilderTeam()}, unaffiliated ${team.IsUnaffiliatedTeam()}")
}
