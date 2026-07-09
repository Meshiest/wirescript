// Zone events. A ZoneEntered / ZoneLeft gate has a `Zone` input that selects
// which zone it watches. Declare an `in` port, wire it to a Zone brick in-game,
// then bind it into each gate with `zone = <port>` — one wire drives both the
// Entered and Left gate for that zone.
in room: entity // wire this to a Zone brick

var inside: int = 0

on ZoneEntered(player, zone = room) {
  inside = inside + 1
  player.DisplayText("inside (${inside})",
    positionX = 0.0, positionY = -200.0,
    fontSize = 30, lifetime = 3.0, textId = 1)
}

on ZoneLeft(player, zone = room) {
  inside = inside - 1
  player.DisplayText("outside (${inside})",
    positionX = 0.0, positionY = -200.0,
    fontSize = 30, lifetime = 3.0, textId = 1)
}
