// Entity velocities, teleports, freezing, visibility, tags, teams, custom
// properties and despawning, driven by world_entity.script. Nothing moves on
// its own: a velocity is a stored value.
in pad: teleport

on ChatCommand("spin", "Sets velocities") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.SetLinearVelocity(Vec(3.0, 4.0, 0.0))
  BroadcastChatMessage("speed ${c.GetSpeed()} linear ${c.GetLinearVelocity()}")
  c.SetAngularVelocity(Vec(0.0, 0.0, 90.0))
  BroadcastChatMessage("angular ${c.GetAngularVelocity()}")
  let at = c.GetVelocityAtPoint(c.GetLocation() + Vec(100.0, 0.0, 0.0))
  BroadcastChatMessage("at point ${at}")
  c.AddVelocity(linear = Vec(1.0, 0.0, 0.0), angular = Vec(10.0, 0.0, 0.0))
  let v = c.GetVelocity()
  BroadcastChatMessage("velocity ${v.Vector} ${v.Rotation}")
  c.SetVelocity(linear = Vec(0.0, 0.0, -1.0))
  BroadcastChatMessage("set ${c.GetLinearVelocity()} ${c.GetAngularVelocity()}")
}

on ChatCommand("move", "Moves you") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.AddLocationRotation(Vec(1.0, 1.0, 1.0), Rotation(10.0, 20.0, 30.0))
  BroadcastChatMessage("added ${c.GetLocation()} ${c.GetRotation()}")
  c.Teleport(pad)
  BroadcastChatMessage("teleported ${c.GetLocation()}")
  c.RelativeTeleport(pad, pad)
  BroadcastChatMessage("relative ${c.GetLocation()}")
}

on ChatCommand("mark", "Marks you") -> (controller, arguments) {
  let c = controller.CharacterOf()
  let before = c.IsFrozen()
  c.SetFrozen(true)
  BroadcastChatMessage("frozen ${before} ${c.IsFrozen()}")
  c.SetTag("runner")
  BroadcastChatMessage("tag '${c.GetTag()}' '${controller.GetTag()}'")
  c.SetVisible(false)
  c.SetVisibleTo(controller)
  c.SetGravityDirection(Rotation(0.0, 0.0, 180.0))
  let team = c.GetEntityTeam()
  c.SetEntityTeam(GetTeamByName("Red"))
  BroadcastChatMessage("team '${team.GetTeamName()}' '${c.GetEntityTeam().GetTeamName()}'")
}

on ChatCommand("props", "Keeps properties") -> (controller, arguments) {
  let c = controller.CharacterOf()
  c.SetCustomProperty("coins", 7)
  c.SetCustomProperty("name", "Ann")
  let coins = c.GetCustomProperty("coins")
  let missing = c.GetCustomProperty("gems")
  BroadcastChatMessage("coins ${coins.Value} ${coins.Found} gems ${missing.Found}")
  controller.CopyCustomProperties(c)
  let copied = controller.GetCustomProperty("name")
  BroadcastChatMessage("copied ${copied.Value} ${copied.Found}")
  let removed = c.RemoveCustomProperty("coins")
  let again = c.RemoveCustomProperty("coins")
  BroadcastChatMessage("removed ${removed} ${again} ${c.GetCustomProperty("coins").Found}")
  c.DestroySpawned()
  c.DestroySpawnedPrefab()
}
