// Player, team and zone array fills, driven by world_arrays.script.
in hall: zone
var everyone: controller[] = []
var reds: controller[] = []
var inside: character[] = []
var runners: character[] = []

on ChatCommand("count", "Counts players") -> (controller, arguments) {
  everyone.fillFromPlayers()
  reds.fillFromTeam(GetTeamByName("Red"))
  inside.fillFromZonePlayers(hall)
  runners.fillFromZoneEntities(hall, "runner")
  BroadcastChatMessage("players ${everyone.length()} red ${reds.length()} zone ${inside.length()} runners ${runners.length()}")
  for p in everyone {
    BroadcastChatMessage("player ${p.GetUserName()}")
  }
}

on ChatCommand("run", "Tags you a runner") -> (controller, arguments) {
  controller.CharacterOf().SetTag("runner")
}
