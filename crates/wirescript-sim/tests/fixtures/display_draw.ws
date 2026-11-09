// A display draws what the program writes, one tick after the write: one
// `█` per pixel, a color tag where the color changes, and a space for a
// pixel under half alpha. The `.expected` file holds each picture.
@display(width = 4, height = 3)
var screen

on Loaded() {
  screen[0] = Color(1.0, 0.0, 0.0)
  screen[1] = Color(1.0, 0.0, 0.0)
  screen[5] = Color(0.0, 1.0, 0.0)
  screen[6] = Color(0.0, 1.0, 0.0)
  screen[11] = Color(0.0, 0.0, 1.0, 0.25)
  await SleepTicks(_, delay = 5)
  screen.fill(Color(0.0, 0.0, 1.0))
  screen[10] = Color(0.0, 0.0, 0.0, 0.0)
}

@test mod draws_a_tick_after_the_write() {
  screen[2] = Color(1.0, 1.0, 0.0)
  await SleepTicks(_, delay = 1)
  testing.assertEq(__display_screen_row0, "<color=\"FF0000\">██<color=\"FFFF00\">█ ")
  screen[3] = Color(1.0, 1.0, 0.0)
  testing.assertEq(__display_screen_row0, "<color=\"FF0000\">██<color=\"FFFF00\">█ ", "not redrawn yet")
  await SleepTicks(_, delay = 1)
  testing.assertEq(__display_screen_row0, "<color=\"FF0000\">██<color=\"FFFF00\">██")
}
