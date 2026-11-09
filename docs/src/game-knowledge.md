# Game Knowledge

Brickadia behavior that the language does not define but that programs depend
on. The language reference tells you what compiles. This page collects what the
game does with the result, plus the practical tips that follow from it.

<!-- toc -->
## Contents

- [Rich Text Markup](#rich-text-markup)
- [Input Action Names](#input-action-names)
- [DisplayText Screen Placement](#displaytext-screen-placement)
- [References in wire values](#references-in-wire-values)
- [How a Tick Runs](#how-a-tick-runs)
- [Loop Ticks](#loop-ticks)
- [Players and Components](#players-and-components)
<!-- /toc -->

## Rich Text Markup

Any string the game renders as text accepts markup: `DisplayText`,
`BroadcastChatMessage`, `ShowStatusMessage`, chat, and the text components a
`@label` writes. Markup is ordinary string content, so it composes with
interpolation and `..` concatenation like any other text.

| Tag | Effect |
|-----|--------|
| `<b>` | Bold |
| `<i>` | Italic |
| `<br>` | Line break. It is not a container, so it takes no close |
| `<color="RRGGBB">` | Text color, hex. `#` optional, `#RGB` shorthand works |
| `<size="N">` | Point size |
| `<font="Name">` | Typeface, from the font list below |
| `<icon>Name</>` | The named game icon, inline at text size |
| `<inputAction>Name</>` | The key the reader has bound to that action |
| `<inputAxis>Name</>` | The same, for an axis |

`</>` closes the most recently opened tag, so nesting reads as it does in HTML
apart from the closing form:

```wirescript
in go: exec
let hex = "ff4040"

on go {
  BroadcastChatMessage('<b><color="${hex}">RED TEAM</></> wins')
}
```

### Colour tags need no close

`<color>` does not have to be closed. An open tag applies until the next
`<color>` replaces it, and it keeps applying across `<br>`. Emitting a tag only
when the color differs from the previous one is therefore correct.

That matters at scale. A tag is 16 characters against one for a block
character, so tagging every cell of a 60x40 grid costs about 40,000 characters
while tagging only the changes costs a few thousand. Run-length encoding by
color is what makes a full screen fit in one `DisplayText`.

```wirescript
in go: exec
var cells: string[]

on go {
  var line: string = ""
  var last: string = ""
  for i in 0..cells.length() {
    let hex = cells[i]
    if hex != last {
      line = line .. '<color="${hex}">'
      last = hex
    }
    line = line .. "#"
  }
  BroadcastChatMessage(line)
}
```

One caveat if the text is split across several displays: color carries forward
within one string only. A chunk that begins mid-run needs its own opening tag,
so reset the tracked color at every boundary you might split on.

Prefer single-quoted strings for anything containing markup. Both quote styles
interpolate, and single quotes let the attribute's own double quotes stand
without escaping.

### Input Glyphs

`<inputAction>Jump</>` renders the key each player has bound to that action, so
a prompt reads correctly for someone on a remapped keyboard or a controller
rather than hardcoding a key name. Axes take `<inputAxis>` instead.

```wirescript
in go: exec

on go {
  BroadcastChatMessage('press <inputAction>Interact</> to draw a card')
}
```

### Fonts

The 25 typefaces the game ships. Anything else falls back.

```
Aaaiight              BadComic              BlackOpsOne
Bungee                CherryBombOne         Cinzel
EagleLake             GlacialIndifference   GlacialIndifferencePlus
Gotfridus             IosevkaTerm           IosevkaTermSlab
Kurland               MonaspaceArgon        MonaspaceKrypton
MonaspaceNeon         MonaspaceRadon        MonaspaceXenon
MostWasted            NotoSans              NotoSerif
Orbitron              PirataOne             Roboto
RobotoMono
```

`IosevkaTerm`, `IosevkaTermSlab`, and the five Monaspace faces are monospaced,
which is what makes grid and table rendering line up.

### Text Decal sizing

A Text Decal draws one em of its font a fixed number of grid units tall per
unit of `LineHeight` (10 grid units are one stud), and that number depends on
the font: 1.28 for Iosevka Term and 1.24 for Monaspace Argon, measured in
game. `WidthScale` stretches each glyph and its advance alike, so a character
is `advance × scale × LineHeight × WidthScale` wide.

A `█` (U+2588) is a solid pixel when it is drawn square: `WidthScale` equal
to its height over its advance. `@display` sizes its rows this way, except
that Monaspace rows drawn exactly one pixel tall leave hairlines between
them, so it draws them 1% taller. A negative `Offset.Y` moves text down, in
grid units; at 0 the top of a `█` sits above the top of its brick face, by
about 0.217 em in Iosevka Term and 0.178 em in Monaspace, and `@display`
moves it down by that much. The `█` in ems, read from the published font
files:

| Font | Advance | Bottom | Top | Height | Square at `WidthScale` |
|---|---:|---:|---:|---:|---:|
| Iosevka Term | 0.500 | -0.285 | 0.965 | 1.250 | 2.50 |
| Monaspace (all five) | 0.620 | -0.305 | 0.950 | 1.255 | 2.02 |
| Bungee | 1.000 | -0.180 | 0.900 | 1.080 | 1.08 |

Black Ops One, Cherry Bomb One, Cinzel, Eagle Lake, Noto Sans, Noto Serif,
Orbitron, Pirata One, Roboto and Roboto Mono have no `█`. The other faces
were not measured.

## Input Action Names

Every name accepted by `<inputAction>` and `<inputAxis>`. A `*` marks an axis,
which takes `<inputAxis>`. Everything else takes `<inputAction>`.

### Special Functions

```
OpenEscapeMenu           SelfDestruct             FreeMouse
PlayerList               OpenEnvironmentDialog    OpenAvatarCustomization
OpenOptions              HideHUD                  ToggleSmoothCamera
ToggleFreezeCamera       HoldToZoomCamera         Teleport
TakeScreenshot           TakeScreenshotNoUI
```

### Movement

```
Turn *             LookUp *           TurnRate *
LookUpRate *       MoveForward *      MoveLeft *
Jump               Sprint             ToggleFlying
ToggleGhostFlying  MoveUp *           SwitchCameraMode
EmoteMenu          ToggleFlashlight   Duck
HoldToWalk         Reload             Inspect
Fire               AltFire            AltFire2
AltFire3
```

### Chat

```
OpenChatBox          ChatHistoryPageUp    ChatHistoryPageDown
ChatHistoryLineUp    ChatHistoryLineDown  ChatHistoryStart
ChatHistoryEnd
```

### Tools

```
OpenToolPieMenu              UseBrickTool                 UseHammer
Paint                        UseResizeTool                UseSelectionTool
UseApplicator                UseManipulator               Paint_PaintMaterial
Paint_FillPaint              Paint_PickColor              Hammer_CheckOwner
Selector_SplitGrid           Selector_AddSelect           Selector_ToggleSelect
Selector_SelectContraption   Selector_SwitchMode          Selector_SelectBox
Selector_DeselectBox         Applicator_SwitchMode        PasteSelectionWithOwnership
Resizer_ExtendToMax          Resizer_ShrinkToMin          Resizer_FindSize
ToolAlt                      Connector_BulkConnect        Connector_SwitchMode
Connector_ReselectLastPort   ManipulatorLaunch            Manipulator_SoftPick
Manipulator_Detach           Manipulator_HoldToRotate     Manipulator_DoNotAttach
```

### Building

```
ToggleBuilding                 BrickRotate                    OrbitMode
UndoAction                     RedoAction                     CopySelection
PasteSelection                 CutSelection                   DeleteSelection
FineSelection                  LockBrickAlignmentPlane        BrickChangePlacementMode
BrickChangeAlignmentMode       BrickSuperAlignmentMode        BrickAlignToWorld
DetachedMode                   Placer_PastePhysics            DeleteBrick
Builder_PickBrick              Builder_PickColor              Builder_PickBrickIntoQuickbar
```

### Building (Keyboard)

```
ToggleDetachedMode     BrickMoveAway *        BrickMoveLeft *
BrickMoveUp *          BrickDetachedTurn *    BrickMoveUpPlate *
BrickPlant             BrickDetachedReorient  BrickDetachedRotate
```

### Quickbar

```
Quickbar_Slot0  Quickbar_Slot1  Quickbar_Slot2
Quickbar_Slot3  Quickbar_Slot4  Quickbar_Slot5
Quickbar_Slot6  Quickbar_Slot7  Quickbar_Slot8
Quickbar_Slot9
```

### Misc

```
Rename  Find
```

### Vehicles

```
LeaveSeat        Vehicle_ZoomIn   Vehicle_ZoomOut
```


## DisplayText Screen Placement

`DisplayText` positions text with an **anchor** and a **position offset**, and
the two use different units. Getting this wrong puts the text off-screen, where
it looks identical to the text not drawing at all.

- **`anchorX` / `anchorY` are fractions of the screen.** `0` is left / top,
  `0.5` is center, `1` is right / bottom. This is the only pair that takes a
  `0..1` value.
- **`positionX` / `positionY` are an offset from that anchor in slate units**,
  which are roughly pixels at 1080p. They are not a fraction. A realistic offset
  is in the tens or hundreds. `positionX = 0.98` does not mean "98% across the
  screen". It is one pixel from the anchor.

Anchor to the corner you want, then offset *inward* with the sign that moves you
away from that edge:

```wirescript
on show {
  // top-right, inset 40 units from the right edge and 40 down from the top
  ctrl.DisplayText("LAP 3",
    anchorX = 1.0, anchorY = 0.0,
    positionX = -40.0, positionY = 40.0,
    fontSize = 28, justify = "Right")

  // bottom-right, 300 in from the right and 460 up from the bottom
  ctrl.DisplayText("STATUS",
    anchorX = 1.0, anchorY = 1.0,
    positionX = -300.0, positionY = -460.0,
    fontSize = 24)
}
```

### Default values

Unset parameters bake the gate's own defaults, which are reasonable. An invisible
overlay is far more often a placement bug than a scale or color one:

| Field | Default | Note |
|---|---|---|
| `Scale` | `(1, 1)` | not zero, so unset scale never hides text |
| `FontColor` | opaque white | not transparent |
| `Anchor` | `(0.5, 0.5)` | screen center |
| `Position` | `(0, 0)` | no offset from the anchor |
| `Pivot` | `(-1, 0.5)` | |
| `FontSize` | `16` | |
| `OutlineSize` | `2` | text has an outline unless you pass `0` |
| `Lifetime` | `5.0` | seconds; **`0` means infinite**, not "vanish now" |
| `TextId` | `0` | `0` uses the brick's persistent handle. A non-zero id is shared by every gate using it, which is how several gates update one piece of text |

`lifetime` interacts with how often you redraw. Text redrawn every tick needs a
short lifetime so it disappears if the drawing chip stops. Text redrawn only on
a change needs `0` (infinite), or it blanks itself between updates.

The 2D layout ports are `Vector2D` composites and the call feeds them one axis
at a time, so every one of these is a `float` named `...X` / `...Y`. There is no
`vector`-typed `position` or `anchor`. Passing one is a `WS041` error.

## References in wire values

Measured in game, with the probes that motivated ref values as a language
feature (see [Reference values](types.md#reference-values)):

- A component ref is stored in an ordinary var. A gate reading its `VarRef`
  from that var reads, writes and increments the target, and copying the ref
  var-to-var keeps it.
- Handle containers are kind-strict: a `component_handle` array or map value
  keeps a component ref, a `brick_handle` array keeps a brick ref, and each
  only keeps its own kind.
- A number container (an `i64`/`f64` array, or an `f64` map value) turns a
  pushed ref into the plain number `1`. The compiler's storage rules exist to
  prevent this: a ref only ever picks a handle container.
- A non-ref value used where the game expects a ref halts the exec chain
  silently, with nothing logged.
- `1 == <ref>` compares true in the native gate, which is why the language
  rejects a ref/brick compared with a non-ref (`WS084`) instead of letting the
  comparison reach the gate.
- A Select gate routes a ref through unchanged, and refs travel as
  custom-event data, both without special-casing.
- Arrays of arrays work: a `component_handle` array holding array refs, with
  an element read back out able to drive `Push`, `Get` and `GetLength` on the
  inner array.
- A brick ref baked into a var's data survives paste and resolves under both
  addressing forms. The relative form (grid 0, chunk offset, index) and the
  absolute form (saved grid id, chunk, index) of the same brick compare equal,
  and refs to two different bricks compare unequal.
- No gate consumes or produces a brick: `GetLocation` on one returns the
  origin, and nothing outputs one. A brick ref comes only from the editor or
  from save data.
- `FormatText` prints a var ref as `Var`, an array ref as `Array Var`, and a
  brick as `Brick`.

## How a Tick Runs

Measured in game with self-reporting circuits. These are the rules a program
can rely on, and the two it cannot.

**A tick is one frame.** The game runs its logic once per frame, as fast as it
renders, so `DeltaTime()` varies from tick to tick. Time-based gates
(`Sleep`, `Clock`, `Timer`) add up `DeltaTime()` until they reach their
interval, so they fire on whole ticks, never between them.

**An exec input fires when its value changes to something truthy.** A plain
value wired into an exec input (what `on <expression>` compiles to) fires once
per change to true, not on every tick it stays true:

- a bool held true for several ticks fires once, when it turns true
- an int fires on every change to a nonzero value (0 to 1, 1 to 2, 0 to -1),
  but not on a change to 0; a counter that goes up every tick fires every tick,
  which is how `on tick` works
- a string fires on a change to anything but `""`, `"0"` and `"false"`
- a constant never fires, not even when the grid loads

**Arithmetic settles within the tick.** A change works through any number of
pure gates (math, compares, string building) in the same tick, in whatever
order they were placed.

**Inside a handler, writes are visible to the handler's own reads, not to
everything else.** After `x = 7` in a handler, a later read of `x` in that
handler sees 7. A value computed from `x` outside the handler (a top-level
`let`, or a select tree in a chip) still has its old value until the handler
finishes, so a display rebuilt in the same handler that changed the value shows
the old value. Rebuild it on the next tick, or compute the text inside the
handler from the values the handler just wrote.

**After a handler finishes:**

- A handler that runs later in the same tick sees the updated values.
- Reactions to the writes (`on Change(x)`, `on x > 5`) run after the handler,
  and before the next tick's handlers.
- `on Change(x)` reacts to a change of value, not to a write: writing a var
  its own value, or writing 9 and then 0 back in one handler, fires nothing.
- A call forked into a chip (`F().exec`, `F(exec = emit)`) runs once the
  handler that forked it has finished.

**Next tick:** a custom event arrives on the tick after it is sent, and an
event handler that sends another adds a tick per hop. `buffer emit` lands on
the next tick. `SleepTicks(_, delay = N)` resumes N ticks later, or 1 for
`delay = 0`.

**One branch at a time.** Where one exec point drives several things (a fork,
an `emit` with a listener, several handlers on one event), each runs to
completion before the next starts. Nothing runs halfway and interleaves.

**A signal runs once per tick.** An exec fired from several places (two
`emit`s of one signal, two handlers that emit it) meets in an Exec Union,
which passes one pulse per tick, so the signal's handlers run once.

**Not visible in the source, so do not depend on it:**

- The order of several handlers triggered by the same change, such as two
  `on tick` handlers. It is fixed for a given save, but set by how the
  compiled gates are laid out and wired: a handler that emits a signal runs
  with that signal's listeners, for one. A handler that reads state another
  handler writes can see it either before or after that write, so give each
  piece of state one writer.
- The order custom events from different senders arrive in.

**At load,** the Load Event runs before the first tick handler, and gates
already hold the values computed from their stored inputs: a var initialized to
5 reads 5, and a value computed from it is ready too. A `buffer` starts from
its own stored output, so it reads 0 at load and its input's value one tick
later.

**Text.** Interpolation (`"${x}"`) and `..` print numbers differently:

| Value | `"${x}"` | `"" .. x` |
|---|---|---|
| `1000` | `1000` | `1000` |
| `1.0 / 3.0` | `0.333` | `0.333333` |
| `123456.789` | `123456.789` | `123457` |
| `1e20` | `100000000000000000000` | `1e+20` |
| `true` | `true` | `true` |

Interpolation rounds to 3 decimals, ties to even (`0.0625` prints `0.062`).
`..` prints 6 significant digits, switching to an exponent outside roughly
0.0001 to 1,000,000.

## Loop Ticks

A `for` loop runs its entire body, every iteration, to completion within the
tick that triggers it. An N-iteration loop costs one tick, not N. The
exception is a loop marked `@sliced`, whose chip lets the game spread
the iterations over several ticks.

The standing guidance to give each entity its own chip instance rather than
sweep a central one in a loop (see
[per-entity fan-out](best-practices.md#10-per-entity-work-a-for-sweep-or-one-chip-each))
exists because a back-edge loop, built from `emit`/`await` and a buffered
edge, costs a tick per iteration. That is no longer the only option: a bounded
sweep that must finish before the tick ends belongs in a `for` now. Keep the
back-edge form for work that should stay spread across ticks on purpose, such
as a slow reveal or a rate-limited rebuild.

**No exec leaves a loop body.** An exec wired from inside a `for` body to a
gate outside it runs nothing, and an Exec Union it reaches passes nothing else
that tick. A plain pulse that meets, in an Exec Union, a value a loop body
changed that tick is lost too. Wirescript routes such an `emit`, and every
other emit of its signal, through a counter var instead: the target fires
once per tick, one tick later when only the loop emitted.

## Players and Components

Behavior checked in game:

- **Button.** Its outputs are **Held**, **Character** and **Press Count**;
  it has no exec output, and nothing fires on release except **Held** and
  **Press Count** changing. **Press Count** is the number of players holding
  the button right now: it goes up when someone presses and down when they
  let go. It is not a running total of presses. **Held** stays on while
  anyone holds the button. **Character** is set on every press, including a
  second player pressing while the button is already held. With **Held**
  wired to an exec pin (`in held: exec`), `on held` fires when Held turns
  on, so it misses that second press. To count presses from
  one player, `on held { count += 1 }`. To catch every player's press and who
  made it, wire **Press Count** to an `int` pin `holding` and keep its last
  value in a `var last`: in `on Change(holding)`, act only
  `if holding > last`, reading **Character** there, then set
  `last = holding`. `Change` does not fire when the chip loads.
- **Switch.** Its outputs are **Enabled** and **Character** (the Toggle
  Switch has the same two). **Character** holds the last player who flipped
  the switch, either way, and keeps holding them: it is a steady character
  wire that changes only when someone else flips it. Wired into
  `InputReader` (the **Seat Control Splitter**), it gives that player's
  keys live, wherever they stand, with no seat and no Clock.
- **Get Inputs** (`GetInputs`) reads any player, wherever they are. They
  need not be seated or inside a zone.
- **Spawn Prefab**'s `offset` is relative to the Spawn Prefab gate's own
  position, not world space. For a world position, spawn the copy and then
  `SetLocation` the returned entity.
- **`SetLocation`** moves a spawned prefab, from the spawner (the entity
  `SpawnPrefab` returns) or from inside the copy (`ReadBrickGrid()`). It
  does not move a pasted chip's own grid.
- **Controllers.** A controller (player state) is the persistent player,
  not the body. A character and its controller are not equal: a character
  used as a `Map<controller, V>` key files under that body, and a respawn
  starts a new key. Custom properties (`SetCustomProperty`) can be set on a
  controller, which keeps them through respawns (but not a reconnect; on a
  character they are lost at respawn). Getting a controller from a
  character takes `ControllerOf`, an admin gate; without admin, key
  per-player state by `GetUserId()`.
- **A player who leaves.** A stored reference to a player who has left the
  server (in a `var`, an array or a map) turns into `null`, so
  `owner == null` detects it. `GetUserId()` on it reads back `""` too.
- **A player who dies.** A stored character of a player who died also
  turns into `null`, but not at once: some time after the death, with no
  fixed delay. An `owner == null` check therefore also removes a dead
  player's copy, a little later than the death.
- **Zone events.** `ZoneEntered` and `ZoneLeft` are fine for reacting to
  one player stepping in or out (a greeting, a chime, a door), but over a
  long session some are missed. A list of who is inside, or a count per
  team, kept up from them drifts and does not come back. Ask the zone
  instead: `arr.fillFromZonePlayers(zone)` (**Get Players In Zone**) gives
  everyone in it now. When that has to stay current, run the fill on a
  Clock; that is a proper use of one.
- **Clock.** The first pulse comes one `interval` after the Clock is
  enabled, not at the moment it turns on.
- **Server restarts.** `var`s, arrays and maps are saved with the bricks
  and come back. Buffers survive a restart too, so a pending
  `await Sleep(...)` or a `buffer emit` still lands after it.
  `ServerUptime()` starts again from 0, so a deadline stored from it comes
  back unreachable; `GetUnixTime()` keeps counting.
- **Tagging a spawned copy.** A spawned prefab's entity holds custom
  properties like any other. The spawner can tag a copy with its player's
  user id right after spawning it (`copy.SetCustomProperty("owner", id)`),
  and the copy reads its own tag with
  `ReadBrickGrid().GetCustomProperty("owner")`. Store the read in a typed
  `var` before sending it in an event: an annotated `let` still sends as an
  untyped value (`WS045`).
