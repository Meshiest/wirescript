// A chat command and players joining, driven by chat_command.script.
var joined: int = 0

on ControllerJoined() -> (controller, userId, userName) {
  joined += 1
  BroadcastChatMessage("welcome ${userName}, ${joined} here")
}

on ChatCommand("double", "Doubles a number") -> (controller, arguments) {
  let n = ParseInt(arguments)
  BroadcastChatMessage("${arguments} doubled is ${n * 2}")
}
