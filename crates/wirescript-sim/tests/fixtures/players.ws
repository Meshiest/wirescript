// Players, their names and ids, their screens and where their characters
// are, driven by players.script.
on ControllerJoined() -> (controller, userId, userName) {
  BroadcastChatMessage("${userName} joined as ${controller.GetDisplayName()}")
  let id = controller.DisplayText("Hello ${controller.GetUserName()}")
  controller.DisplayText("Hello again", textId = id)
  controller.ShowChatMessage("your id is ${controller.GetUserId()}")
}

on ChatCommand("where", "Says where you are") -> (controller, arguments) {
  let body = controller.CharacterOf()
  BroadcastChatMessage("${body.ControllerOf().GetUserName()} is at ${body.GetLocation()}")
  body.SetLocation(Vec(1.0, 2.0, 3.0))
  BroadcastChatMessage("now at ${body.GetLocation()}")
}

on ChatCommand("keys", "Says what you hold") -> (controller, arguments) {
  let keys = controller.CharacterOf().GetInputs()
  BroadcastChatMessage("forward ${keys.Forward} E ${keys.PressedE} Q ${keys.PressedQ}")
}
