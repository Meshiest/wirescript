# Admin Gates

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Is It Working?](../is-it-working.md)

**Next steps:** [World Control](world-control.md), [Persistent Players](persistent-players.md), [Chat Commands](chat-commands.md)

</div>
<!-- /learn-links -->

Some gates only an admin can load. They reach every player or the whole
world: a chat line to everyone, the time of day, the list of players. This
branch teaches them. Every page in it needs admin to load.

Needs admin to load: the examples here use **Broadcast Chat Message**.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Finding the culprit](#finding-the-culprit)
- [A debug line](#a-debug-line)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip ring
chip sign -> decal Text
```

## Code

```wirescript name=a0-bell admin
@left in ring: exec
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

on ring {
  rings += 1
  BroadcastChatMessage("The bell has rung ${rings} times")
}
```

A bell that counts its rings on a sign and tells the whole server about each
one. `BroadcastChatMessage` sends one chat line that every player sees.

You paste this in a world you host. A friend pastes the same file on a server
where they are not an admin. What does each of you see when the bell rings?

## Run

1. Compile the chip and paste it in-game, in a world you host. Place a
   **Button** and a brick with a **Text Decal**.
2. Wire the Button's **Held** to `ring`, and `sign` to the Text Decal's
   **Text**.
3. Press the button. The sign counts, and a chat line appears for everyone.

A player without admin rights who pastes it sees the paste report blocked
bricks: the **Broadcast Chat Message** gate is missing, and its wires cannot
connect. Only the admin gates are missing. The rest of the chip loads.

## Under the Hood

Open the chip. The handler's chain is the button, an **Increment Variable**
for `rings`, a **Get Variable** that reads the new count, then **Broadcast
Chat Message** with a **Format String** feeding its **Message**.

For a player who could not load the broadcast gate, the chain has a gap. The
increment comes before the gap, so the sign should still count. Swap the two
lines in the handler and the gap comes first: nothing after it runs, and the
sign stays at 0.

The game marks each gate as admin or not. These Wirescript calls compile to
an admin gate:

| Wirescript | Gate |
|---|---|
| `on ChatCommand(...)` | **Chat Command Event** |
| `on CharacterSpawned`, `CharacterDied`, `CharacterDamaged`, `CharacterFiredWeapon` | **Player Spawned Event** and the others |
| `on ControllerJoined`, `ControllerLeft`, `ControllerJoinedTeam`, `ControllerLeftTeam` | **Player Joined Event** and the others |
| `BroadcastChatMessage`, `BroadcastStatusMessage` | **Broadcast Chat Message**, **Broadcast Status Message** |
| `arr.fillFromPlayers()`, `arr.fillFromTeam(team)` | **Fill Array From Players**, **Fill Array From Team Members** |
| `FindPlayer(name)` | **Find Player by Name** |
| `ControllerOf(character)` | **Get Player (Persistent)** |
| `Environment.Sky.TimeOfDay = ...` (any environment write) | **Set Environment Property** |
| `SetTeam`, `SetTeamPinned`, `GetTeam`, `GetTeamByName`, `SetEntityTeam` | **Set Player Team** and the others |
| `ForceRespawn`, `SetCanRespawn` | **Force Respawn Player**, **Set Can Respawn** |
| `SetDamage`, `IncDamage`, the ammo setters | **Set Player Damage** and the others |
| leaderboards, `PlayerWins`, `TeamWins`, `GetCurrentRound` | **Set Leaderboard Value** and the others |
| `ShowMessageBox`, `ShowYesNoBox`, `ShowOkCancelBox`, `ShowTextInputBox` | the four dialogs |
| `PlayGlobalAudio`, `SpawnExplosion`, `SpawnExplosionAt` | **Play Global Audio**, **Spawn Explosion** |
| `PrintToConsole`, `SendGlobalCustomEvent` | **Print to Console**, **Send Global Custom Event** |

Not admin, and used all through the other branches: `Clock`, zone events,
`ShowChatMessage` and `ShowStatusMessage` to one player, `GetDisplayName`,
`GetUserId`, `HasRole`, reading the environment, personal custom events,
`fillFromZonePlayers`, `GetInputs` and `GetAim`.

## Finding the culprit

When a paste reports blocked bricks, look for these gates first. Open the
chip as an admin and find them by name, or search the compiled program. The
compiler prints it with `wirescript compile bell.ws --dump-ir` (or `just ir
bell.ws`), one gate per line, by class name:

```text
  %9 = Exec_Gamemode_BroadcastChatMessage(Exec: %7.ExecOut, Message: %8) : ...
```

Search for `ChatCommand`, `Event_Gamemode_`, `Broadcast`, `FillArrayFromPlayers`,
`FindPlayer`, `GetFromEntity` and `SetEnvironmentProperty`, which cover most
cases. The full list of class names is in `docs/admin-gates.json` in the
compiler's repository, and the Learn pages are checked against it: a page
not marked admin that compiles to one of them fails the check.

## A debug line

[Is It Working?](../is-it-working.md) showed how to watch a circuit with a
debug pin. A chat line is another way to watch one. Put a
`BroadcastChatMessage` in a handler, and each run prints the values it read.

```wirescript name=a0-debug admin
@left in pressed: exec
@left in armed: bool
var count: int = 0

on pressed {
  BroadcastChatMessage("pressed: armed=${armed} count=${count}")
  if armed {
    count += 1
  }
}
```

No line when you press means the trigger is the problem, not the steps.
Remove the line before you share the chip, or it stops a non-admin from
loading it.

## Modify

1. Move the broadcast above `rings += 1`. Nothing changes for you. For a
   non-admin, it is now the first step and nothing after it runs.
2. Make the bell work for everyone: remove the broadcast and keep only the
   sign. The chip has no admin gates left.
3. Tell only the player who rang. The Button's **Character** output says who
   pressed it: wire it to a `@left in ringer: character` pin and replace the
   broadcast with `ringer.ShowChatMessage(...)`, which is not admin.

## Make

Take a circuit from another branch, add a debug line to every handler, and
use it to find which handler runs first when two fire on the same press.
Then remove them all and compile again to check the chip has no admin gate
left.

## Good to Know

- A player without admin rights cannot load an admin gate. The rest of the
  chip loads, so the circuit half works instead of failing outright.
- A chain stops at the first missing gate. Put admin-only steps last in a
  handler when the rest should still work without them.
- In a world you host, you are the admin.
- Reading the environment, a player's name or their role is not admin. Writing
  the environment, reaching every player, and the player events are.

See [Broadcast Messaging](../../builtins.md#broadcast-messaging-exec) in the
reference.

Next: [World Control](world-control.md).
