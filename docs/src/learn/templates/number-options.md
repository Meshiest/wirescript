# Number Options

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Fmt](fmt.md)

**Next steps:** [Word Forms](word-forms.md)

**Parts that use this:** [Clock Sign](../parts/clock-sign.md), [Race Timer](../parts/race-timer.md), [Round Manager](../parts/round-manager.md), [Obstacle Course](../parts/obstacle-course.md), [Team Match](../parts/team-game.md)

</div>
<!-- /learn-links -->

A slot can carry an option that changes how its number prints: commas between
the thousands, or a fixed number of decimals. Leading zeros have no option, so
this lesson builds them by hand, then prints real dates and times.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
chip2 "Counter"
chip "Sign"
button Held -> chip2 addRed
chip2 red -> chip presses
chip big -> decal1 Text
chip share -> decal2 Text
chip both -> decal3 Text
```

## Code

```wirescript name=tp3-coins
@left in presses: int
@right out big = Fmt("{A:,} points", presses * 125000)
@right out share = Fmt("{A:.2} each", presses / 4.0)
@right out both = Fmt("{A:,.2} coins", presses * 1234.5)
```

An option goes inside the slot, after a colon.

- `{A:,}` groups the thousands with commas: `1,234,567`.
- `{A:.2}` prints exactly two digits after the point: `12.50`. Any count up
  to 6 works.
- `{A:,.2}` does both: `1,234.50`.

`presses / 4.0` divides by a number with a point in it, so the result keeps
its fraction: 3 presses give 0.75, not 0.

After three presses, what does each sign show?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and three bricks
   with a **Text Decal**.
2. Paste the counter chip from [Interpolation](interpolation.md#run). Wire
   the Button's **Held** to its `addRed` and its `red` to `presses`. Wire
   `big`, `share` and `both` to the three Text Decals' **Text**.
3. Press the button three times. The signs read `375,000 points`,
   `0.75 each` and `3,703.50 coins`. Press once more: `share` shows `1.00`,
   two decimals even when they are zero.

## Under the Hood

Open the chip. Each Format String gate still has its option in its
**Format String** (`{A:,} points`): the gate applies the option every time the
value changes.

Write options on the lettered slots, as here. Whether they also work on the
numbered form (`{0:.2}`) has not been checked, so the lettered form is the one
to use. Options need `Fmt`: a `${}` template has nowhere to put them.

## Modify

1. **Leading zeros.** No option pads `7` to `07`, so write a helper that does.

   ```wirescript name=tp3-pad
   mod pad2(n: int) -> string {
     let s = "0${n}"
     return s.Substring(s.Length() - 2, 2)
   }

   @left in presses: int
   @right out padded = pad2(presses)
   ```

   `mod` defines a helper with a name. `n: int` is its input, `-> string`
   says it gives back text, and `return` gives that text. Each use copies the
   helper's gates into the chip ([`mod`](../../chips.md#mod-inline-chips)).
   The body puts a `0` in front, then keeps the last two characters:
   `s.Length()` counts the characters, and `s.Substring(start, length)` takes
   `length` of them from position `start`. So 7 becomes `07`, and 42 becomes
   `042` and then `42`. It only works below 100: 100 would come out as `00`.
2. **Minutes and seconds.** A lap timer: the first press starts it, and every
   press after that shows the time since the first, as `mm:ss`.

   ```wirescript name=tp3-lap
   mod pad2(n: int) -> string {
     let s = "0${n}"
     return s.Substring(s.Length() - 2, 2)
   }

   @left in pressed: exec
   var started: int = 0
   var elapsed: int = 0
   @right out lap = "${pad2(elapsed.Value / 60)}:${pad2(elapsed.Value % 60)}"

   on pressed {
     let now = GetUnixTime()
     if started == 0 {
       started = now
     }
     elapsed = now - started
   }
   ```

   `GetUnixTime()` reads the server's clock as a whole number of seconds
   (counted from 1 January 1970). `if started == 0 { ... }` runs its block
   only on the first press, while nothing is stored yet. Dividing one whole
   number by another gives a whole number, so `elapsed / 60` is the full
   minutes. `%` gives the remainder: the seconds left over. Wire a Button's
   **Held** to `pressed` and `lap` to a sign.
3. **Dates.** `FormatDate` turns a Unix time into text.

   ```wirescript name=tp3-stamp
   @left in pressed: exec
   var stamp: int = 0
   @right out when = FormatDate(stamp.Value, "%Y-%m-%d %H:%M", useUTC = true).Output

   on pressed {
     stamp = GetUnixTime()
   }
   ```

   The sign shows when the button was last pressed. In the format, `%Y` is
   the year, `%m` the month, `%d` the day, `%H` the hour and `%M` the minute.
   `useUTC = true` gives the time in UTC, and `false` gives the server's local
   time. The gate also has a **Success** output, `.Success`, which is false
   when it could not format the date.

## Make

Make a sign for a parkour course: the first press of a start button stores
the time, and a finish button shows the run's time as `mm:ss`, plus the date
and time of the run.

```wiring
button1 Held -> chip start
button2 Held -> chip finish
chip text -> decal Text
```

## Good to Know

- Store times from `GetUnixTime()`, not `ServerUptime()`. Uptime starts again
  from zero when the server restarts, but a `var` keeps its value, so a stored
  uptime can point into the future.

See [String Formatting](../../builtins.md#string-formatting-pure) and
[Date / time](../../builtins.md#date--time-pure) for the reference.

Next: [Word Forms](word-forms.md).
