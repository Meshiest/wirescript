// Loops - `for` over ranges, arrays and maps
//
// Each loop is its own microchip. The game sweeps its body once per
// iteration, all within one tick (or spread over several with @sliced).

in go: exec

var squares: int[]
var evenTotal: int = 0
var scores: Map<string, int>
var bestTeam: string = ""
var bestScore: int = 0
var countdown: string = ""
var firstBig: int = -1
var slowSum: int = 0

on go {
  // Range: 0, 1, .. 9 (the end is excluded)
  squares.clear()
  for i in 0..10 {
    squares.push(i * i)
  }

  // Array: one name binds the value, two bind index and value
  evenTotal = 0
  for i, sq in squares {
    if i % 2 == 0 { evenTotal += sq }
  }

  // Map: one name binds the key, two bind key and value
  scores.set("red", 12)
  scores.set("blue", 30)
  scores.set("green", 21)
  bestScore = 0
  for team, pts in scores {
    if pts > bestScore {
      bestScore = pts
      bestTeam = team
    }
  }

  // .rev() counts down, .step(k) strides
  countdown = ""
  for i in (1..6).rev() {
    countdown = "${countdown}${i} "
  }

  // break stops the loop
  firstBig = -1
  for i in (0..100).step(5) {
    if i * i > 1000 {
      firstBig = i
      break
    }
  }

  // @sliced lets the game run a long loop across ticks;
  // `let run = for` reports how it went
  slowSum = 0
  @sliced
  let run = for i in 0..1000 {
    slowSum += i
  }

  BroadcastChatMessage("evens ${evenTotal}, best ${bestTeam} (${bestScore})")
  BroadcastChatMessage("countdown ${countdown}, first big ${firstBig}")
  BroadcastChatMessage("sum ${slowSum} in ${run.iterations} iterations")
}
