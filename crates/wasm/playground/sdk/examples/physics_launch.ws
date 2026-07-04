// Launch entity upward on trigger
in trigger: bool
in target: entity

on trigger {
  target.AddVelocity(Vec(0.0, 0.0, 1000.0))
}
