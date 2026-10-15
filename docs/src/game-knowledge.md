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
