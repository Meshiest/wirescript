// Display score on screen
in reset: exec
in point: exec
in player: character
var score: int = 0

on reset {
  score = 0
}

on point {
  score += 10
  player.DisplayText("Score: ${score}",
    positionX = 0.0, positionY = -200.0,
    fontSize = 40, lifetime = 10.0, textId = 1)
}
