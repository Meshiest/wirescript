# Teams

<!-- learn-links -->
<div class="learn-links">

**Before this:** [World Control](world-control.md), [Shuffling](../random/shuffling.md)

</div>
<!-- /learn-links -->

A game mode can sort players into teams, and a circuit can do the sorting
itself. This lesson splits everyone in a lobby into two random teams at the
press of a button, then builds pads that let players pick their own team.

Needs admin to load: it uses **Get Team By Name** and **Set Player Team**.

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
zone "Zone Ref" -> chip lobby
button Held -> chip pick
```

## Code

```wirescript name=a2-split admin
@left in lobby: zone
@left in pick: exec
var players: character[]

on pick {
  players.fillFromZonePlayers(lobby)
  players.shuffle()
  let red = GetTeamByName("Red")
  let blue = GetTeamByName("Blue")
  for i, p in players {
    if i % 2 == 0 {
      p.SetTeam(red)
    } else {
      p.SetTeam(blue)
    }
  }
}
```

- `players.fillFromZonePlayers(lobby)` fills the array with every player
  standing in the zone, replacing what was there.
- `shuffle()` puts them in a random order, as in
  [Shuffling](../random/shuffling.md), and the `i % 2` split from there puts
  the even places on Red and the odd places on Blue.
- `GetTeamByName("Red")` finds the team with that name, and `p.SetTeam(red)`
  moves a player onto it.

Five players stand in the lobby. How many end up on each team? Press again:
can a player land on the team they were already on?

## Run

1. Set up the world's game mode with two teams named exactly `Red` and
   `Blue`.
2. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   for the lobby, and place a **Button**.
3. Wire the Attached Zone's **Zone Ref** to `lobby` and the Button's **Held**
   to `pick`.
4. Stand in the lobby with your friends and press the button. Each player
   moves to Red or Blue. Press again for a new draw.

## Under the Hood

Open the chip. The chain is **Get Players In Zone**, **Shuffle Array**, then
two **Get Team By Name** gates, each with its team name typed in. The loop is
a **For Each Array** microchip with a **Modulo** and a **Branch** inside,
and one **Set Player Team** on each side of the Branch.

The two teams are found once, before the loop. Their **Team** outputs reach
the Set Player Team gates through two pins on the side of the loop chip, the
same way any wire crosses into a chip.

A team is an `entity`: a reference to a thing in the world, like a player
is. **Set Player Team** takes a player in its **Player State** input. The
loop's players are characters, and a character wired into that input stands
for its player.

## Modify

1. Pin the teams. `p.SetTeam(red, pin = true)` pins the player to Red, so the
   game mode's own team sorting does not move them later.
2. Tell each player their team. Inside the loop, after the Branch, add
   `p.ShowChatMessage(...)` with "You are on Red" or "You are on Blue".
3. Let players choose. Two pads, one per team, each with its own Attached
   Zone:

   ```wirescript name=a2-pads admin
   @left in redPad: zone
   @left in bluePad: zone

   on ZoneEntered(zone = redPad) -> (character) {
     let red = GetTeamByName("Red")
     character.SetTeam(red, pin = true)
     character.ShowChatMessage("You joined Red")
   }

   on ZoneEntered(zone = bluePad) -> (character) {
     let blue = GetTeamByName("Blue")
     character.SetTeam(blue, pin = true)
     character.ShowChatMessage("You joined Blue")
   }
   ```

## Make

Make a captain draft. Two captains each stand on a pad. Every press of a
button moves the next player from the lobby onto the team whose turn it is,
alternating Red and Blue, until the lobby is empty.

```wiring
zone1 "Zone Ref" -> chip lobby
zone2 "Zone Ref" -> chip redPad
zone3 "Zone Ref" -> chip bluePad
button Held -> chip pick
```

## Good to Know

- `SetTeam` moves a player whatever the team's join rules say. A team that
  was not found is empty, and an empty team takes the player off their team.
  Check the team names in the game mode if players end up on no team.
- Pinned players stay where they are put. Use `SetTeamPinned(false)` on a
  player to let the game mode sort them again.
- With one player in the lobby you only ever see Red. Test the split with
  friends, or check the Shuffling lesson's typed-in names first.

See [GetTeamByName](../../builtins.md#getteambyname--getteamname),
[SetTeam](../../builtins.md#setteam) and
[Zone array fills](../../builtins.md#zone-array-fills-exec-array-methods) in
the reference.

Next: [Persistent Players](persistent-players.md).
