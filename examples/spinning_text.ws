in start: exec
in startPlayer: character
in stop: exec
var ctrl: controller
var running: bool = false
buffer tick: int = if running then tick + 1 else 0

on start {
  ctrl = ControllerOf(startPlayer)
  running = true
}

on stop {
  running = false
}

on tick {
  let angle = tick * 0.15
  let x = sin(angle) * 50.0
  let y = cos(angle) * 50.0
  DisplayText(ctrl, "hello world", positionX = x, positionY = y, lifetime = 0.2, fontSize = 20)
}
