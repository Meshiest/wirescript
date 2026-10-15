# World Control

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Admin Gates](admin-gates.md), [Forking and Merging](../events/forking-and-merging.md)

**Next steps:** [Teams](teams.md)

</div>
<!-- /learn-links -->

A circuit can change the world itself: the time of day, the clouds, the
rain. These are the settings in the game's environment dialog, and every
player sees the change at once. This lesson turns night on with a switch and
rolls a storm in slowly.

Needs admin to load: writing a setting uses **Set Environment Property**.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Use it: a storm](#use-it-a-storm)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
switch Enabled -> chip night
```

## Code

```wirescript name=a1-daynight admin
const DAY = 12.0
const NIGHT = 0.0

@left in night: bool

on night {
  Environment.Sky.TimeOfDay = NIGHT
}

on !night {
  Environment.Sky.TimeOfDay = DAY
}
```

`Environment.Sky.TimeOfDay` is one setting: `Sky` is the section of the
environment dialog, `TimeOfDay` the setting in it. Assigning to it inside a
handler writes the world's time of day. `on !night` runs when the switch
turns off, as in [Forking and Merging](../events/forking-and-merging.md).

Flip the switch on. It goes dark. Now wait a few minutes. Does it stay dark?

## Run

1. Compile the chip and paste it in-game. Place a **Switch**.
2. Wire the Switch's **Enabled** to `night`.
3. Flip it on: the sky turns to midnight. Flip it off: noon.
4. Leave it on. If the world's day cycle is running, the clock carries on
   from midnight and the sun rises again. The circuit set the time once and
   does not hold it there.

## Under the Hood

Open the chip. There are two **Set Environment Property** gates, one per
handler. Each has the environment (`Sky`) and the setting (`TimeOfDay`) in
its own settings, and a **Value** input: `0` in one, `12` in the other. The
`on !night` gate has a **NOT** in front of its **Exec**.

Reading a setting is a step too, a **Get Environment Property** gate, so it
only works in a handler. Reading is not admin. This clock sign reads the time
once a second, and anyone can paste it:

```wirescript name=a1-clock-sign
var hour: float = 0.0
@right out text = "Hour: ${hour.Value}"

on Clock(interval = 1.0) {
  hour = Environment.Sky.TimeOfDay
}
```

Read it straight into the pin instead, and the compiler refuses:

```wirescript name=a1-pure-read expect=WS007
@right out hour: float = Environment.Sky.TimeOfDay
```

```text
Error: [WS007] environment read 'Environment.Sky.TimeOfDay' outside an exec context
```

Every setting and its type is listed in
[Environment Settings](../../environment.md). A write that does not fit the
setting's type is an error, and a name that does not exist is `WS110`.

## Use it: a storm

A setting can be written again and again. A Clock that nudges three weather
settings a little each quarter second brings a storm in over five seconds,
and takes it away again when the switch goes off:

```wirescript name=a1-storm admin
const STEPS = 20

@left in storm: bool
var level: int = 0
let moving = (storm && level.Value < STEPS) || (!storm && level.Value > 0)

on Clock(interval = 0.25, enabled = moving) {
  if storm {
    level += 1
  } else {
    level -= 1
  }
  let t = (level as float) / (STEPS as float)
  Environment.Sky.CloudCoverage = t
  Environment.Sky.Rain = t
  Environment.Sky.Thunder = t
}
```

`level` counts from 0 to `STEPS`, and `t` turns it into a fraction from 0.0
to 1.0: `as float` turns a whole number into a decimal one, and `/` divides.
`moving` is true while the storm still has somewhere to go: rolling in
(`storm` on and `level` short of `STEPS`) or rolling out (`storm` off and
`level` above 0). `||` is "or". It drives the Clock's `enabled`, so the
Clock runs only while the storm is moving and stops once it arrives.

## Modify

1. Hold the night. Write `Environment.Sky.AnimateTimeOfDay = false` before
   the time in `on night`, and `= true` after it in `on !night`.
2. Add fog to the storm: `Environment.Sky.CloudyFogDensity = t`.
3. Make a sunset button. Each press reads `Environment.Sky.TimeOfDay`, adds
   one hour, and writes it back.

## Make

Make a weather board with three buttons, Clear, Rain and Snow, that each
roll their weather in over a few seconds and roll the others out. Show the
current weather on a sign.

```wiring
button1 Held -> chip clear
button2 Held -> chip rain
button3 Held -> chip snow
chip sign -> decal Text
```

## Good to Know

- The environment is one for the whole world. Every player sees a change, and
  two circuits writing the same setting overwrite each other: the last write
  wins.
- The game converts and limits a written value as the environment dialog
  would, and leaves a setting the dialog has locked alone.
- A write happens once. The day cycle, or another circuit, can change the
  setting again straight after.

See [Environment Settings](../../environment.md) in the reference.

Next: [Teams](teams.md).
