// Test: import all declarations from utils
import "utils"

var score: int = 0
var lives: int = 3

on Loaded() {
  inc(score)
  clamp(score, 0, 999)
  dec(lives)
  clamp(lives, 0, 10)
}

out score: int = score
out lives: int = lives
out doubled = double(score)
