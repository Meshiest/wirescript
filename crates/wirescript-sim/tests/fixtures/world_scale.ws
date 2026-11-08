// Player scale, driven by world_scale.script: a character starts at 1, and
// each /grow doubles the caller's scale.
on ChatCommand("grow", "Doubles your size") -> (controller, arguments) {
  let c = controller.CharacterOf()
  let before = c.GetScale()
  c.SetScale(before * 2.0)
  BroadcastChatMessage("scale ${before} to ${c.GetScale()}")
}
