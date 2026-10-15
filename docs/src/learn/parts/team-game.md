# Team Match

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Payloads](../enums/payloads.md), [Ready Check](../input/ready-check.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Per-Player State](../references/per-player-state.md), [Rich Text](../strings/rich-text.md), [Number Options](../templates/number-options.md), [Keeping It Current](../screen/keeping-it-current.md)

</div>
<!-- /learn-links -->

A two-team game. Players pick a side by standing on the red or the blue
pad, then race to score: running into the goal at the far end of the field
scores a point for your team and sends you back to your pad. A round lasts
three minutes, everyone on the field sees the score and the clock on their
screen, and a sign announces the winner.

<!-- toc -->
## Contents

- [You need](#you-need)
- [Code](#code)
- [Wire it](#wire-it)
- [Check it in game](#check-it-in-game)
- [Change it](#change-it)
- [How it works](#how-it-works)
<!-- /toc -->

## You need

- Two bricks with an **Attached Zone** each, the red and the blue pads, at
  either end of the field.
- Two more **Attached Zone**s for the goals: the red goal is where Red
  scores, at the blue end, and the blue goal at the red end.
- One large **Attached Zone** over the whole field, pads included.
- A **Button** to start a round.
- A brick with a **Text Decal** for the sign, and two **Point Light**s, one
  red and one blue, to show which team leads.

```wiring
zone1 "Zone Ref" -> chip redPad
zone2 "Zone Ref" -> chip bluePad
zone3 "Zone Ref" -> chip redGoal
zone4 "Zone Ref" -> chip blueGoal
zone5 "Zone Ref" -> chip arena
button Held -> chip start
chip sign -> decal Text
chip redLeads -> light1 Enabled
chip blueLeads -> light2 Enabled
```

## Code

```wirescript name=cp4-team-match
const ROUND = 180     // seconds a round lasts
const RESULTS = 10    // seconds the result stays up before the lobby
const TEXT_ID = 7400  // a number no other circuit on the server uses
const FONT_SIZE = 30  // size of the text on each screen
const RED = 1         // team numbers in the team map
const BLUE = 2

enum Phase { Lobby, Playing { until: int }, Over { until: int } }

@left in redPad: zone
@left in bluePad: zone
@left in redGoal: zone
@left in blueGoal: zone
@left in arena: zone
@left in start: exec

var phase: Phase = Phase.Lobby
var team: Map<string, int>
var home: Map<string, vector>
var redScore: int = 0
var blueScore: int = 0
var secondsLeft: int = 0
var result: string = ""
var players: character[]

mod pad2(n: int) -> string {
  let s = "0${n}"
  return s.Substring(s.Length() - 2, 2)
}

let score = '<color="ff4040">Red ${redScore.Value}</> - <color="4080ff">${blueScore.Value} Blue</>'
@right out sign = match phase {
  Lobby => "Pick a team on a pad, then press start<br>${result.Value}",
  Playing { .. } => "${score}<br>${pad2(secondsLeft.Value / 60)}:${pad2(secondsLeft.Value % 60)}",
  Over { .. } => "${result.Value}<br>${score}",
}
@right out redLeads = redScore > blueScore
@right out blueLeads = blueScore > redScore

let redraw: exec

mod join(who: character, side: int, label: string) {
  if !(phase is Phase.Lobby) {
    return
  }
  let id = who.GetUserId()
  team.set(id, side)
  home.set(id, who.GetLocation())
  who.ShowStatusMessage("You are on team ${label}")
  emit redraw
}

mod goal(who: character, side: int) {
  if !(phase is Phase.Playing) {
    return
  }
  let id = who.GetUserId()
  let t = team.get(id)
  if !t.Found || t.Value != side {
    return
  }
  if side == RED {
    redScore += 1
  } else {
    blueScore += 1
  }
  who.ShowStatusMessage("You scored!")
  let back = home.get(id)
  if back.Found {
    who.SetLocation(back.Value)
  }
}

on ZoneEntered(zone = redPad) -> (character) { join(character, RED, "Red") }
on ZoneEntered(zone = bluePad) -> (character) { join(character, BLUE, "Blue") }
on ZoneEntered(zone = redGoal) -> (character) { goal(character, RED) }
on ZoneEntered(zone = blueGoal) -> (character) { goal(character, BLUE) }

on ZoneEntered(zone = arena) {
  emit redraw
}

on ZoneLeft(zone = arena) -> (character) {
  if character != null {
    character.DisplayText("", textId = TEXT_ID)
  }
}

on start {
  if !(phase is Phase.Lobby) {
    return
  }
  players.fillFromZonePlayers(arena)
  var reds: int = 0
  var blues: int = 0
  for p in players {
    let t = team.get(p.GetUserId())
    if t.Found && t.Value == RED {
      reds += 1
    } else if t.Found && t.Value == BLUE {
      blues += 1
    }
  }
  if reds == 0 || blues == 0 {
    result = "Each team needs a player"
    return
  }
  redScore = 0
  blueScore = 0
  secondsLeft = ROUND
  phase = Phase.Playing { until: GetUnixTime() + ROUND }
}

on Clock(interval = 0.5, enabled = !(phase is Phase.Lobby)) {
  let now = GetUnixTime()
  match phase {
    Lobby => {}
    Playing { until } => {
      secondsLeft = until - now
      if now >= until {
        if redScore > blueScore {
          result = "Red wins!"
        } else if blueScore > redScore {
          result = "Blue wins!"
        } else {
          result = "A draw!"
        }
        phase = Phase.Over { until: now + RESULTS }
      }
    }
    Over { until } => {
      if now >= until {
        phase = Phase.Lobby
      }
    }
  }
  emit redraw
}

on redraw {
  players.fillFromZonePlayers(arena)
  for p in players {
    let t = team.get(p.GetUserId())
    let mine = if !t.Found then "Pick a team" else if t.Value == RED then "You are Red" else "You are Blue"
    let top = match phase {
      Lobby => "Waiting for the round",
      Playing { .. } => "${pad2(secondsLeft / 60)}:${pad2(secondsLeft % 60)} left",
      Over { .. } => result,
    }
    let fade = if phase is Phase.Lobby then 0.0 else 1.0
    p.DisplayText("${top}<br>${score}<br>${mine}",
      textId = TEXT_ID, anchorX = 0.5, anchorY = 0.0, positionY = 60.0,
      fontSize = FONT_SIZE, justify = Center, lifetime = fade)
  }
}
```

## Wire it

1. Compile the chip and paste it in-game.
2. Wire the **Zone Ref** outputs: the red pad to `redPad`, the blue pad to
   `bluePad`, the goal at the blue end (where Red scores) to `redGoal`, the
   goal at the red end to `blueGoal`, and the zone over the field to
   `arena`.
3. Wire the Button's **Held** to `start`.
4. Wire `sign` to the Text Decal's **Text**, `redLeads` to the red light's
   **Enabled** and `blueLeads` to the blue one's.

## Check it in game

This needs at least two players.

- [ ] The sign reads "Pick a team on a pad, then press start". On the field,
  your screen says "Waiting for the round" and "Pick a team".
- [ ] Step on the red pad: "You are on team Red", and your screen says so.
- [ ] Walk off the field: the text leaves your screen at once. Walk back
  on: it is back.
- [ ] Press start with nobody on Blue: the sign adds "Each team needs a
  player".
- [ ] A second player steps on the blue pad. Press start: the sign shows
  "Red 0 - 0 Blue" and the clock, `03:00` counting down.
- [ ] Step on the blue pad during the round: you stay on Red.
- [ ] Run into the red goal: "You scored!", you are back at the red pad,
  the score reads 1 - 0, and the red light comes on.
- [ ] The Blue player runs into the red goal: nothing happens. Only Red
  scores there.
- [ ] At `00:00` the sign and every screen on the field show the winner,
  "Red wins!", "Blue wins!" or "A draw!".
- [ ] Ten seconds later the sign is back to "Pick a team" with the last
  result under it, and the teams are as they were.

## Change it

- A shorter round: `ROUND = 60`.
- Shuffle the teams instead of letting players pick: in the lobby, fill an
  array from the arena, [shuffle](../random/shuffling.md) it, and put even
  places on Red and odd places on Blue, as in [Index and
  Value](../loops/index-and-value.md).
- Score by holding a point instead: in the Clock's `Playing` arm, fill an
  array from a middle zone, count each team's players with the `team` map,
  and give a point to the team with more. That scores twice a second.
- End the round at a target score: in `goal`, after the point, when either
  score reaches 5, set `result` to the winner and move to `Over`.

## How it works

The round is an [enum](../enums/payloads.md), as in [Round
Manager](round-manager.md): `Lobby`, then `Playing` and `Over`, each
carrying the Unix time it ends, and a Clock that moves it on. The Clock
runs only outside the lobby. Team membership is a [map](../values/maps.md) from `GetUserId()` to 1
for Red or 2 for Blue ([Per-Player State](../references/per-player-state.md)),
so a player who respawns stays on their team. The game's own team gates
need admin to load. The map does the same job here, and anyone can paste
it. `home` keeps where each player stood when they joined, and a goal sends
the scorer back there. `start` fills an array with the players on the field,
as in [Ready Check](../input/ready-check.md), counts each team among them,
and refuses to begin with an empty side. A player who has left the server
is still in the map but no longer on the field, so they do not count.
Who is on the field always comes from a fill, never from a count kept by
`ZoneEntered` and `ZoneLeft`: over a long game a few of those events can
be missed, and the fill is always right.

Each player on the field sees one text, drawn with `DisplayText` as in
[Scoreboard HUD](scoreboard-hud.md): the clock or the result, the score,
and the player's team. The `redraw` signal draws it for everyone on the
field. In the lobby it fires only when something changes: a player walks
onto the field or picks a team. During a round the clock changes every
second, so each Clock pulse fires it too. Walking off the field draws an
empty text over yours. In the lobby the text is drawn with lifetime 0,
which keeps it up until it is replaced. During a round it is redrawn twice a second with a
lifetime of one second, so it fades by itself from a player who is no
longer on the field, such as one who died there (the leave event gives no
character for them). The score is one `let` with [color
tags](../strings/rich-text.md), shared by the sign and every screen.

Back to [Parts](README.md).
