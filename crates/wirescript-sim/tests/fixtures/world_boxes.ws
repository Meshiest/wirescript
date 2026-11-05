// Boxes a player answers, driven by world_boxes.script.
on ChatCommand("ask", "Asks you things") -> (controller, arguments) {
  let ready = controller.ShowYesNoBox("Ready?", title = "Start")
  on ready.YesClicked { BroadcastChatMessage("${ready.ClickedBy.GetUserName()} is ready") }
  on ready.Declined { BroadcastChatMessage("not ready") }
  let name = controller.ShowTextInputBox("Your name?", title = "Name")
  on name.OkClicked { BroadcastChatMessage("named ${name.Text}") }
  let ok = controller.ShowOkCancelBox("Continue?", title = "Next")
  on ok.OkClicked { BroadcastChatMessage("continuing") }
  on ok.Declined { BroadcastChatMessage("cancelled") }
  BroadcastChatMessage("asked")
}
