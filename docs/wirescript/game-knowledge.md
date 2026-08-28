# Game Knowledge

Brickadia behaviour that the language does not define but that programs depend
on. The language reference tells you what compiles; this page collects what the
game does with the result, plus the practical tips that follow from it.

<!-- toc -->
## Contents

- [Rich Text Markup](#rich-text-markup)
- [Input Action Names](#input-action-names)
- [DisplayText Screen Placement](#displaytext-screen-placement)
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
| `<color="RRGGBB">` | Text colour, hex. `#` optional, `#RGB` shorthand works |
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
which takes `<inputAxis>`; everything else takes `<inputAction>`.

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
  which are roughly pixels at 1080p -- not a fraction. A realistic offset is in
  the tens or hundreds. `positionX = 0.98` is not "98% across the screen", it is
  one pixel from the anchor.

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

### Defaults worth knowing

Unset parameters bake the gate's own defaults, which are sane -- an invisible
overlay is a placement bug far more often than a scale or color one:

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
| `TextId` | `0` | `0` uses the brick's persistent handle; a non-zero id is shared by every gate using it, which is how several gates update one piece of text |

`lifetime` interacts with how often you redraw. Text redrawn every tick wants a
short lifetime so it disappears if the drawing chip stops; text redrawn only on
a change wants `0` (infinite), or it blanks itself between updates.

The 2D layout ports are `Vector2D` composites and the call feeds them one axis
at a time, so every one of these is a `float` named `...X` / `...Y`. There is no
`vector`-typed `position` or `anchor`; passing one is a `WS041` error.
