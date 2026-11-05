// The fix for score_race.ws: one handler writes the score and then reports
// it, so the order of the two steps is the order they are written in.
buffer tick: int = tick + 1
var score: int = 0

on tick {
  if tick <= 3 {
    score = tick * 10
  }
  if tick == 3 {
    BroadcastChatMessage("score ${score}")
  }
}
