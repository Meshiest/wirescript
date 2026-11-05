// A seat control splitter reads its character's held inputs, driven by
// world_splitter.script.
var rider: character

on CharacterSpawned() -> (character) {
  rider = character
}

let input = rider.InputReader()
on input.PressedE {
  BroadcastChatMessage("E pressed")
}
on input.Forward > 0.5 {
  BroadcastChatMessage("moving forward at ${input.Forward}")
}
