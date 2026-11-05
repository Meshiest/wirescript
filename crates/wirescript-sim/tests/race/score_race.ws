// Two handlers on one tick: one writes the score, the other reports it. The
// game does not order handlers fired by the same change, so the report reads
// the score before or after this tick's write. `wirescript test --race`
// flags it; score_single_writer.ws is the fix.
buffer tick: int = tick + 1
var score: int = 0

on tick {
  if tick <= 3 {
    score = tick * 10
  }
}

on tick {
  if tick == 3 {
    BroadcastChatMessage("score ${score}")
  }
}
