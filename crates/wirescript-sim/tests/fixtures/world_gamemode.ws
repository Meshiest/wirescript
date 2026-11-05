// Teams, leaderboards, rounds, wins and status messages, driven by
// world_gamemode.script.
on ControllerJoined() -> (controller, userId, userName) {
  let team = controller.CharacterOf().GetTeam()
  BroadcastChatMessage("${userName} joined on '${team.GetTeamName()}'")
}

on ControllerJoinedTeam() -> (controller, team, userId, userName) {
  BroadcastChatMessage("${userName} joined team ${team.GetTeamName()}")
}

on ControllerLeftTeam() -> (controller, team, userId, userName) {
  BroadcastChatMessage("${userName} left team ${team.GetTeamName()}")
}

on ChatCommand("teams", "Moves you to Blue") -> (controller, arguments) {
  let red = GetTeamByName("Red")
  let green = GetTeamByName("Green")
  BroadcastChatMessage("lookup '${red.GetTeamName()}' '${green.GetTeamName()}'")
  controller.SetTeam(GetTeamByName("Blue"), pin = true)
  BroadcastChatMessage("now on ${controller.CharacterOf().GetTeam().GetTeamName()}")
  controller.SetTeamPinned(false)
}

on ChatCommand("score", "Keeps score") -> (controller, arguments) {
  controller.SetLeaderboard("kills", 3)
  controller.IncLeaderboard("kills", 2)
  let blue = GetTeamByName("Blue")
  blue.SetTeamLeaderboardValue("flags", 1)
  blue.IncrementTeamLeaderboardValue("flags", 4)
  let kills = controller.GetLeaderboard("kills")
  let deaths = controller.GetLeaderboard("deaths")
  let flags = blue.GetTeamLeaderboardValue("flags")
  BroadcastChatMessage("kills ${kills} deaths ${deaths} flags ${flags}")
}

on RoundStart() -> (roundNumber) {
  BroadcastChatMessage("round ${roundNumber} current ${GetCurrentRound()}")
}

on RoundEnd() -> (roundNumber) {
  BroadcastChatMessage("round ${roundNumber} over")
}

on ChatCommand("win", "Wins") -> (controller, arguments) {
  controller.PlayerWins()
  controller.PlayerWins(teamWinsInstead = true)
  GetTeamByName("Red").TeamWins()
  BroadcastStatusMessage("Round over", flash = true)
}
