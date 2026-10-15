# Clock Sign

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md), [Number Options](../templates/number-options.md)

</div>
<!-- /learn-links -->

A sign that shows the real time, updated every second. Hang it in a lobby
or over a station platform.

<!-- toc -->
## Contents

- [You need](#you-need)
- [Code](#code)
- [Wire it](#wire-it)
- [Check it in game](#check-it-in-game)
- [Change it](#change-it)
- [How it works](#how-it-works)
<!-- /toc -->

## You need

- a brick with a **Text Decal**

```wiring
chip text -> decal Text
```

## Code

```wirescript name=bp9-clock-sign
const FORMAT = "%H:%M:%S" // %H hours, %M minutes, %S seconds
const USE_UTC = true // true for UTC, false for the server's local time

var now: int = 0
@right out text = FormatDate(now.Value, FORMAT, useUTC = USE_UTC).Output

on Clock(interval = 1.0) {
  now = GetUnixTime()
}
```

## Wire it

1. Compile the chip and paste it in-game. Place the Text Decal brick.
2. Wire `text` to the Text Decal's **Text**.

The chip has no inputs: it starts as soon as it is pasted.

## Check it in game

- Within a second of pasting, the sign shows a time such as `14:05:32`.
- Watch it: the seconds go up by one each second.
- Compare it with a clock set to UTC: the hours and minutes match.

## Change it

- Drop the seconds: `const FORMAT = "%H:%M"`, and set `interval = 10.0` on
  the Clock so the sign updates less often.
- Show the date too: `const FORMAT = "%Y-%m-%d %H:%M"`, with `%Y` the year,
  `%m` the month and `%d` the day.
- Show the server's own time zone: `const USE_UTC = false`.
- Put the date on a second line: join two `FormatDate` calls with `..` and
  a `"<br>"` between them, which starts a new line on a sign
  ([Rich Text](../strings/rich-text.md)). Use `"%H:%M"` for the first and
  `"%Y-%m-%d"` for the second.

## How it works

`GetUnixTime()` reads the server's clock as a whole number of seconds,
counted from 1 January 1970. Once a second the Clock stores that number in
`now` ([Reacting to the Game](../reacting-to-the-game.md)), and
`FormatDate` turns it into text using the codes in `FORMAT`
([Number Options](../templates/number-options.md)). Its `.Output` is the
text. It also has a `.Success`, which is false when the date could not be
formatted. A Clock's first pulse comes one interval after it starts, and
until then `now` is 0, so the sign shows the start of 1970 for its first
second.
