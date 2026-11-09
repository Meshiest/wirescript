// Seats report who sits in them and read that player's keys through an
// InputReader, driven by seat_input.script.
@seat(label = "Driver") in p1
@top @seat in p2

on p1.occupied {
  BroadcastChatMessage("p1 taken")
}
on !p1.occupied {
  BroadcastChatMessage("p1 empty")
}
on p2.occupied {
  BroadcastChatMessage("p2 taken")
}

let keys = p1.character.InputReader()
on keys.Forward > 0.5 {
  BroadcastChatMessage("p1 forward")
}
on keys.PressedE {
  BroadcastChatMessage("p1 pressed E")
}

let other = p2.character.InputReader()
on other.Right < -0.5 {
  BroadcastChatMessage("p2 left")
}
