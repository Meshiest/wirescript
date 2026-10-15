# Parts

Ready-made circuits for builders. Each part is one chip: paste it in-game,
wire it to the components listed, and tick off the checklist to see that it
works. Settings are the `const`s at the top of the code. Change them with
**Open in playground** and compile again.

You do not need the lessons to use a part. Each one names the lessons it
builds on, for when you want to change how it works.

## From the lessons

Several lessons end with a part you can use as it is:

| Part | Uses | Lesson |
|---|---|---|
| Light switch | Switch, Point Light | [First Circuit](../first-circuit.md) |
| Two-way hall light | 2 Switches, Point Light | [Gates and Wires](../gates-and-wires.md) |
| Toggle door | Button, Brick Property Changer | [Pure and Exec](../pure-and-exec.md) |
| Press-count sign | Button, Text Decal | [Remembering Values](../state.md) |
| Code door | 4 Switches, Brick Property Changer | [Code Door](../logic/code-door.md) |
| Score card | Buttons, Switch, Text Decal | [Score Card](../templates/score-card.md) |
| Dice sign | Button, Text Decal | [Dice](../random/dice.md) |
| Flickering firelight | Switch, Point Light | [Random Colors](../random/random-colors.md) |
| Traffic light | Switch, Point Light | [Enums](../enums/unit-enums.md) |
| Seat dimmer | Seat, Point Light, Text Decal | [Seat Controls](../input/seat-controls.md) |

## Catalog

<!-- parts-catalog -->
### Building Parts

| Part | Uses | Builds on |
|---|---|---|
| [Toggle Light](toggle-light.md) | Button, Point Light | [Reacting to the Game](../reacting-to-the-game.md) |
| [Combination Lock](combination-lock.md) | Buttons, Brick Property Changer, Point Light | [Mods](../abstraction/mods.md), [Lists](../values/lists.md) |
| [Countdown Sign](countdown-sign.md) | Button, Text Decal, One Shot Audio, Point Light | [Exec Pins](../events/exec-pins.md) |
| [Color-Cycling Light](color-cycling-light.md) | Switch, Point Light | [Lists](../values/lists.md) |
| [Doorbell](doorbell.md) | Button, One Shot Audio | [Exec Pins](../events/exec-pins.md) |
| [Zone Chime](zone-chime.md) | Attached Zone | [Reacting to the Game](../reacting-to-the-game.md) |
| [Drawbridge](drawbridge.md) | Servo, Button, Point Light | [Reacting to the Game](../reacting-to-the-game.md), [Choosing a Value](../logic/choosing-a-value.md) |
| [Proximity Door](proximity-door.md) | Attached Zone, Brick Property Changer | [Waiting](../events/waiting.md) |
| [Clock Sign](clock-sign.md) | Text Decal | [Reacting to the Game](../reacting-to-the-game.md), [Number Options](../templates/number-options.md) |

### Player Parts

| Part | Uses | Builds on |
|---|---|---|
| [Pressure Pad Door](pressure-pad-door.md) | Touch, Brick Property Changer | [Reacting to the Game](../reacting-to-the-game.md), [Waiting](../events/waiting.md) |
| [Welcome Zone](welcome-zone.md) | Attached Zone, Text Decal | [Maps](../values/maps.md) |
| [Teleporter Pad](teleporter-pad.md) | Attached Zone, Teleport Destination | [Maps](../values/maps.md) |
| [Shooting Gallery](shooting-gallery.md) | Target, Text Decal, Button | [Looping Over a Map](../loops/looping-over-a-map.md), [Rich Text](../strings/rich-text.md), [Per-Player State](../references/per-player-state.md) |
| [Healing Zone](healing-zone.md) (needs admin to load) | Attached Zone | [Looping Over a List](../loops/looping-over-a-list.md) |
| [Launch Pad](launch-pad.md) | Attached Zone | [Reacting to the Game](../reacting-to-the-game.md), [Vectors and Colors](../values/vectors-and-colors.md) |
| [Item Dispenser](item-dispenser.md) | Button, Text Decal | [Maps](../values/maps.md) |
| [Paint Gun](paint-gun.md) | Switch, Button, Point Light | [Remote Control](../input/remote-control.md), [Forking and Merging](../events/forking-and-merging.md), [Game Enums](../enums/game-enums.md), [Weighted Loot](../random/weighted-loot.md) |
| [Hold to Use](hold-to-use.md) | Button, Brick Property Changer | [Waiting](../events/waiting.md) |
| [Moving Platform](moving-platform.md) | Rigid Slider, Touch, Button | [Waiting](../events/waiting.md), [Choosing a Value](../logic/choosing-a-value.md) |
| [Keycard Door](keycard-door.md) | Button, Attached Zone, Brick Property Changer | [Maps](../values/maps.md), [Waiting](../events/waiting.md) |

### Game Systems

| Part | Uses | Builds on |
|---|---|---|
| [Checkpoints](checkpoints.md) | Attached Zone | [Maps](../values/maps.md), [Mods](../abstraction/mods.md), [Per-Player State](../references/per-player-state.md) |
| [Race Timer](race-timer.md) | Attached Zone, Text Decal | [Number Options](../templates/number-options.md), [Mods](../abstraction/mods.md), [Per-Player State](../references/per-player-state.md) |
| [Scoreboard HUD](scoreboard-hud.md) | Attached Zone, Target, Button | [Rich Text](../strings/rich-text.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md), [A HUD for Everyone](../screen/a-hud-for-everyone.md) |
| [Round Manager](round-manager.md) | Button, Text Decal, Point Light | [Payloads](../enums/payloads.md), [Exec Pins](../events/exec-pins.md), [Number Options](../templates/number-options.md) |
| [King of the Hill](king-of-the-hill.md) | Attached Zone, Button, Text Decal, One Shot Audio | [Ready Check](../input/ready-check.md), [Exec Pins](../events/exec-pins.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md) |
| [Shop](shop.md) | Button, Target | [Mods](../abstraction/mods.md), [Maps](../values/maps.md), [Per-Player State](../references/per-player-state.md) |
| [Voting](voting.md) | Button, Text Decal | [Looping Over a Map](../loops/looping-over-a-map.md), [Mods](../abstraction/mods.md), [Exec Pins](../events/exec-pins.md), [Per-Player State](../references/per-player-state.md) |

### Capstones

| Part | Uses | Builds on |
|---|---|---|
| [Obstacle Course](obstacle-course.md) | Attached Zone, Text Decal, Button | [Custom Events](../events/custom-events.md), [Mods](../abstraction/mods.md), [Per-Player State](../references/per-player-state.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Number Options](../templates/number-options.md), [Safe Text](../strings/safe-text.md) |
| [Whack-a-Light](arcade-game.md) | Point Light, Button, Text Decal | [Payloads](../enums/payloads.md), [Mods](../abstraction/mods.md), [Dice](../random/dice.md), [Safe Text](../strings/safe-text.md), [Waiting](../events/waiting.md) |
| [Card Table](card-game.md) | Attached Zone, Button, Text Decal | [Shuffling](../random/shuffling.md), [Ready Check](../input/ready-check.md), [Index and Value](../loops/index-and-value.md), [Mods](../abstraction/mods.md), [Safe Text](../strings/safe-text.md), [Keeping It Current](../screen/keeping-it-current.md) |
| [Team Match](team-game.md) | Attached Zone, Button, Text Decal, Point Light | [Payloads](../enums/payloads.md), [Ready Check](../input/ready-check.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Per-Player State](../references/per-player-state.md), [Rich Text](../strings/rich-text.md), [Number Options](../templates/number-options.md), [Keeping It Current](../screen/keeping-it-current.md) |
<!-- /parts-catalog -->
