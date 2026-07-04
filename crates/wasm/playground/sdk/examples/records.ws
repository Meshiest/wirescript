// Records - bundling related state into named types
//
// Records are compile-time bundles that dissolve into individual wires.
// No runtime overhead - `state.counter` resolves to the underlying var gate.

// Declare a record type
type Point = { x: int, y: int }
type State = { counter: *int, step: int }

// Record literal with named fields
let origin: Point = { x: 0, y: 0 }

// Shorthand - captures var by name
var score: int = 0
let game: State = { counter: score, step: 1 }

// Spread - copy fields, override some
let offset: Point = { ...origin, y: 10 }

// Destructuring
let { x, y } = offset

// Mod with destructured parameter
mod add_point({ x, y }: Point) -> int {
  return x + y
}

// Mod with record ref field - mutates through the record
mod bump({ counter, step }: State) {
  counter = counter + step
}

// Tuples
let pair = (42, true)
let answer = pair.0

in player: character
on player {
  // Use records to pass state cleanly
  bump(game)
  bump(game)
  bump(game)

  let sum = add_point(offset)
  let p: Point = { x: sum, y: score }
  let { x: px, y: py } = p

  player.DisplayText(
    "score=${score} sum=${sum} px=${px} py=${py} answer=${answer}",
    positionX = 0.0,
    positionY = 0.0,
    fontSize = 20,
    lifetime = 10.0,
    textId = 0
  )
}
