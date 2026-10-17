# Reacting to the Game

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remembering Values](state.md)

**Next steps:** [Is It Working?](is-it-working.md), [Lists](values/lists.md), [Taking Text Apart](strings/taking-text-apart.md), [Templates Chosen at Run Time](templates/runtime-templates.md), [Exec Pins](events/exec-pins.md), [Mods](abstraction/mods.md), [Records](data/records.md), [Counting Loops](loops/counting-loops.md), [Dice](random/dice.md), [Seat Controls](input/seat-controls.md), [Object References](references/object-references.md), [Spawning a Prefab](prefabs/spawning.md), [Distance and Direction](space/distance-and-direction.md), [Easing](motion/easing.md), [Messages to One Player](screen/messages-to-one-player.md)

**Parts that use this:** [Toggle Light](parts/toggle-light.md), [Zone Chime](parts/zone-chime.md), [Drawbridge](parts/drawbridge.md), [Clock Sign](parts/clock-sign.md), [Pressure Pad Door](parts/pressure-pad-door.md), [Launch Pad](parts/launch-pad.md)

</div>
<!-- /learn-links -->

Handlers so far ran when a pin turned on. The game itself also starts
handlers: a clock that ticks, a player walking into a zone. This lesson also
adds `if`, constants and the arithmetic shortcuts.

<!-- toc -->
## Contents

- [A clock](#a-clock)
- [Someone walks in](#someone-walks-in)
- [Deciding with `if`](#deciding-with-if)
- [Shortcuts](#shortcuts)
- [Good to Know](#good-to-know)
- [Make](#make)
<!-- /toc -->

## A clock

```wirescript name=blinker
const BLINK_SECONDS = 0.5

var lit: bool = false
@right out light = lit.Value

on Clock(interval = BLINK_SECONDS) {
  lit = !lit
}
```

`on Clock(interval = ...)` runs its handler every `interval` seconds, forever.
`const BLINK_SECONDS = 0.5` names a fixed value. Change it in one place to
change the speed.

Wire `light` to a **Point Light**'s **Enabled**. It is on for half a second,
then off for half a second.

## Someone walks in

```wirescript name=hello
import { escape } from "sanitize"

@left in room: zone

on ZoneEntered(zone = room) -> (character) {
  character.ShowChatMessage("Hello, ${escape(character.GetDisplayName())}!")
}
```

`@left in room: zone` is a pin for a zone. Give a brick an **Attached Zone**
and wire its **Zone Ref** output to `room`. `on ZoneEntered(zone = room)` runs
when a player walks into it, and `-> (character)` names the player who did.
`ShowChatMessage` sends a message only that player sees.

A display name is chosen by the player and can contain `<`, which text
displays read as the start of formatting. `import { escape } from "sanitize"`
brings in one helper from the lessons' starter library, `sanitize.ws`, which
makes a name safe to show. The Compile button fetches the library with the
example; to build it in the playground or VS Code, save
[the starter library](tools.md#the-starter-library) beside your file. Use it
whenever a circuit shows text a player chose.

## Deciding with `if`

```wirescript name=wrap-count
@left in pressed: exec
var count: int = 0
@right out text = "Count: ${count.Value}"

on pressed {
  if count >= 9 {
    count = 0
  } else {
    count += 1
  }
}
```

`if` runs one block or the other. This counter goes up to 9, then back to 0.
`>=` compares, as do `==`, `<`, `>`, `<=` and `!=`.

## Shortcuts

`count += 1` is `count = count + 1`. The same works for `-=`, `*=` and `/=`.

## Good to Know

- `Clock` starts when the chip loads and keeps going. Give it
  `enabled = someBool` to turn it on and off. Its first pulse comes one
  `interval` after it starts, not at once.
- `ZoneEntered` and `ZoneLeft` are right for reacting to one player, like
  this greeting. Over a long game a few can be missed, so do not keep a
  list or a count of who is inside from them. To know who is in a zone,
  ask the zone itself with `fillFromZonePlayers` ([Ready
  Check](input/ready-check.md)), on a Clock when it must stay current.
- Some events and actions only an admin may use: a chip containing them is
  blocked when someone without admin rights pastes it. Chat commands, the list
  of every player, broadcasts to everyone, and the world's weather and time of
  day are among them. Lessons that need them are marked "needs admin to load"
  in the tree.

## Make

Make a light that blinks only while a switch is on (`on Clock(interval = 0.5,
enabled = ...)`), and a door that opens for three seconds whenever someone
walks into a zone in front of it.

```wiring
switch Enabled -> chip blinking
chip light -> light Enabled
zone "Zone Ref" -> chip room
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

Next: [Is It Working?](is-it-working.md).
