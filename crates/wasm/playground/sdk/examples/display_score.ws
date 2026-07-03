// Display score on screen using handler
in running: bool
in player: character
var score: int = 0

on running {
  score = 0
}

on RoundStart {
  score = score + 10
}

if running {
  player.DisplayText("Score: ${score}",
    positionX = 0.0, positionY = -200.0,
    fontSize = 40, lifetime = 10.0, textId = 1, exec = running)
}
