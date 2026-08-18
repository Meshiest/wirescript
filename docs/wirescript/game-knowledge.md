# Game Knowledge

Brickadia behaviour that the language does not define but that programs depend
on. The language reference tells you what compiles; this page collects what the
game does with the result, plus the practical tips that follow from it.

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

