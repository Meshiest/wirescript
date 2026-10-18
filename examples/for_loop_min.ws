// Smallest possible `for` loop, for debugging emit against the game log.
// One counted loop, one accumulator, one message. Compare with
// `examples/for_loops.ws`, which is the full acceptance suite and emits 23
// planes, so its log is unreadable while emit is still wrong.
//
// Expected in chat: `sum 45, ran 10`.
// A silent grid, or `sum 0, ran 0`, means the shell brick or the entry gate
// is inert.

var total: int = 0
var runs: int = 0

on Loaded() {
  for i in 0..10 {
    total = total + i
    runs = runs + 1
  }
  BroadcastChatMessage("sum ${total}, ran ${runs}")
}
