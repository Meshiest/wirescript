# Multiplayer Architecture

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Performance](performance.md), [One Chip per Player](../prefabs/one-chip-per-player.md), [Per-Player State](../references/per-player-state.md), [Custom Events](../events/custom-events.md)

**Next steps:** [Writing Libraries](libraries.md)

</div>
<!-- /learn-links -->

A circuit for one player can keep everything in one chip. With twenty
players, where each piece of work lives decides what it costs, and the game
puts limits on how much text and how many events get through. This lesson
builds a shared scoreboard that ranks everyone in one place and draws the
result on every player's screen. You need a second player to see most of it.

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
zone "Zone Ref" -> chip join
target "Just Hit" -> chip point
target "Character That Just Hit" -> chip scorer
```

## Code

```wirescript name=ma4-board
import { escape } from "sanitize"

@left in join: zone
@left in point: exec
@left in scorer: character
var scores: Map<string, int>
var names: Map<string, string>
var huds: Map<string, entity>
var board: string = "No scores yet"

mod rank() {
  var s1: int = 0
  var s2: int = 0
  var s3: int = 0
  var n1: string = "-"
  var n2: string = "-"
  var n3: string = "-"
  for id, n in scores {
    let name = names.get(id)
    if n > s1 {
      s3 = s2
      n3 = n2
      s2 = s1
      n2 = n1
      s1 = n
      n1 = name.Value
    } else if n > s2 {
      s3 = s2
      n3 = n2
      s2 = n
      n2 = name.Value
    } else if n > s3 {
      s3 = n
      n3 = name.Value
    }
  }
  board = "1. ${n1} ${s1}<br>2. ${n2} ${s2}<br>3. ${n3} ${s3}"
}

on point {
  let id = scorer.GetUserId()
  let had = scores.get(id)
  let mine = if had.Found then had.Value + 1 else 1
  scores.set(id, mine)
  names.set(id, escape(scorer.GetDisplayName()))
  rank()
  SendCustomEvent("board.top", board)
  let hud = huds.get(id)
  if hud.Found {
    hud.Value.SendCustomEvent("board.mine", mine)
  }
}

on ZoneEntered(zone = join) -> (character) {
  let id = character.GetUserId()
  let had = scores.get(id)
  let mine = if had.Found then had.Value else 0
  let old = huds.get(id)
  if old.Found {
    old.Value.SendCustomEvent("hud.init", character, mine, board)
    return
  }
  let hud = SpawnPrefab(
    prefab = $```
      var owner: character
      var id: string = ""
      var mine: int = 0
      var top: string = ""
      var ready: bool = false

      mod show() {
        owner.DisplayText("You: ${mine}<br>${top}",
          textId = 7200, anchorX = 1.0, anchorY = 0.0,
          positionX = -40.0, positionY = 40.0, justify = Right,
          lifetime = 0.0)
      }

      on CustomEvent("hud.init", isObject = true) -> (who: character, score: int, text: string) {
        owner = who
        id = who.GetUserId()
        mine = score
        top = text
        ready = true
        show()
        on Clock(interval = 1.0) {
          if ready {
            if owner == null {
              SendCustomEvent("hud.gone", id)
              ReadBrickGrid().DestroySpawnedPrefab()
            }
          }
        }
      }

      on CustomEvent("board.mine", isObject = true) -> (score: int) {
        mine = score
        show()
      }

      on CustomEvent("board.top") -> (text: string) {
        top = text
        if ready {
          show()
        }
      }
    ```,
    offset = Vec(0.0, 0.0, 300.0),
    limit = 64
  )
  huds.set(id, hud)
  hud.SendCustomEvent("hud.init", character, mine, board)
}

on CustomEvent("hud.gone") -> (id: string) {
  huds.remove(id)
}
```

There are two kinds of chip here, one central chip and a HUD per player.

- **The central chip** stores scores and escaped names by user id, and
  `huds` records which HUD belongs to which player.
- `rank()` loops over every score, keeps the best three, and builds the
  board text. The `var`s inside it start again at every call.
- **A HUD** is a small chip spawned for each player who walks through
  `join`. It draws one text on that player's screen, and redraws it in the
  handler of each event that brings it a new value. Nothing redraws on a
  timer. It holds only what the central chip has sent it.
- `owner.DisplayText(...)` draws text on one player's screen. `anchorX = 1.0,
  anchorY = 0.0` pins it to the top right corner, and `positionX` and
  `positionY` move it in from there. `lifetime = 0.0` keeps it up until a
  later draw with the same `textId` replaces it.
- Each HUD checks once a second whether its stored character is still
  there. It turns into `null` when the player leaves the server, and also
  some time after the player dies. The HUD then reports `hud.gone` and
  removes itself, and the central chip drops it from `huds`.
  The Clock is written inside `hud.init`, next to the `owner` it checks,
  but it runs from the moment the HUD appears. `ready` stops a HUD whose
  `hud.init` has not arrived yet, and whose `owner` is empty too, from
  removing itself.
- After a point, the central chip sends twice: the finished board to every
  HUD on `board.top`, and the new score to the scorer's HUD alone on
  `board.mine`.
- Walking through the gate again sends your HUD the character you have now.
  After a respawn that is a new one. If your HUD already removed itself
  after the death, walking through spawns a new one.

Ten players have joined, and one of them scores. How many chips run the
ranking loop? How many texts does each player's screen gain?

## Run

1. Compile the chip and paste it in-game. Give a gateway an **Attached
   Zone** and wire its **Zone Ref** to `join`.
2. Place a **Target**. Wire its **Just Hit** to `point` and its **Character
   That Just Hit** to `scorer`.
3. Walk through the gateway. The top right of your screen shows `You: 0`
   and `No scores yet`.
4. Shoot the target. Your line reads 1, and the board lists you first.
5. A second player walks through and shoots twice. Both screens change at
   once: the board now has them first and you second, and each screen shows
   its own `You:` line.

## Under the Hood

### Where the work lives

The central chip is about 90 gates. About 55 of them are `rank()`: a **For
Each Map** loop, six **Var**s, and the tests and copies inside the loop.
Open a HUD (each one spawns 300 units above the central chip): about 43
gates, mostly **Var**s and the gates that set and read them, three
**Personal Custom Event** gates, the **Clock** that checks for a departed
player, and three **Display Text** gates, one for each place `show()` is
called (a mod's gates are copied into every call).

To weigh a design, multiply each part by how many copies of it run. The
central chip runs once. A HUD runs once per player. Ranking inside each HUD
would mean every copy keeping its own maps of every score (a spawned copy
has storage of its own and cannot read the central chip's), its own loop,
and its own copy of every point as it happens. Twenty players would carry
twenty rankings, over a thousand gates, all working out the same answer.

So the central chip ranks once and sends the result. The board arrives as
finished text, and the HUD only shows it. Building text is the one thing
that costs measurable time each tick, which is one more reason to build it
once. The HUD keeps only what is per-player: which screen to draw on and
that player's score. This is the split from [One Chip per
Player](../prefabs/one-chip-per-player.md). The shared records stay in the
central chip, each copy draws for its own player, and events carry
everything between them. A text is redrawn only
when its words change, and only for the players whose words changed.

### One text per player

Each HUD draws one text. The `You:` line and the three board lines are
joined by `<br>` ([Rich Text](../strings/rich-text.md)), and every redraw
uses `textId = 7200`, so it replaces the text already there. The scorer's
HUD gets `board.mine` and `board.top` in the same tick and draws twice. The
second draw reads both new values, so the screen ends up right.

The game shows only about 32 texts on one player's screen at once, and
every circuit that player sees shares that budget. Drawing the four lines
with four `DisplayText` calls would spend four of them. Past the limit, a
text silently never appears, and which one goes missing changes when you
change something unrelated. Count the texts on screen at one moment, not
the **Display Text** gates in the chip: a chip with many gates that each
draw now and then may show only a few at once.

### Events: one gate, many channels

After a point, the chain sends on two channels, `board.top` and
`board.mine`. Both arrive on the next tick, together: different channels
do not wait for each other. One send on `board.top` reaches every HUD, and
each extra listener costs nothing.

The limit is on a single send gate firing again and again in one tick. Say
you want to refresh every player's own score at once:

```wirescript name=ma4-loop-send
@left in refresh: exec
var scores: Map<string, int>
var huds: Map<string, entity>

on refresh {
  for id, hud in huds {
    let s = scores.get(id)
    hud.SendCustomEvent("board.mine", s.Value)
  }
}
```

This compiles with no warning. Open it: the loop's body holds one **Send
Personal Custom Event** gate, and the loop fires it once per HUD, all in
the same tick. The game caps one gate firing many times on one channel. It
logs `Too many events at once!` with the channel's name and drops the rest,
and which sends get through is not fixed.

Separate gates on one channel are fine. Every HUD has its own `hud.gone`
send, so if many players leave in the same tick, every report arrives. That
gives three rules:

- Everyone needs the same thing: one send to all (`board.top`).
- One player's value changed: one send to that player's chip
  (`board.mine`).
- Never put a send in a loop. Change the design so each send gate fires
  once per tick.

## Modify

1. Add a reset button. Give the central chip `@left in reset: exec`. Its
   handler clears `scores` and `names`, sets `board` back to
   `"No scores yet"`, and sends `board.top` and a new `board.reset` once
   each. Every HUD sets `mine` to 0 on `board.reset`: one send, every
   player.
2. Show the gap to the leader: `You: 3 (leader 7)`. Send the leader's score
   on its own channel, `board.lead`, right after `board.top`. Store it in
   the HUD and add it to the first line.
3. Show `Top 3!` on the HUD of anyone on the board. Do not rank in the HUD:
   ship the third-place score on a channel of its own and let each HUD
   compare its own score with it. One comparison per copy is cheap, but a
   ranking per copy is not.

## Make

Make a king of the hill for any number of players, with no admin gates.
Players join through a gateway as here. Each second, every player standing
in the hill zone scores a point. `players.fillFromZonePlayers(hill)` fills a
`character[]` with everyone standing in a zone
([Ready Check](../input/ready-check.md) uses it). Every player sees their
own score and the top three in one text. Every player's score can change
every second, and a loop of sends is capped, so find a shape where no send
gate fires more than once a tick. Check it with two players: both boards
agree, each player sees exactly one text from your circuit, and the game
log never says `Too many events at once!`.

```wiring
zone1 "Zone Ref" -> chip join
zone2 "Zone Ref" -> chip hill
```

## Good to Know

- A player's screen shows about 32 texts at once, shared by every circuit
  they see. Past that, one text silently never appears, and nothing says
  which.
- One send gate fired many times in one tick is capped, and the rest are
  dropped. Separate send gates on one channel all deliver, in no fixed
  order.
- A spawned copy has storage of its own. It cannot read the central chip's
  maps. Everything it knows came to it by event.
- A respawn makes a new character. Key everything by `GetUserId()`, and
  send the HUD the new character when the player comes back.
- A spawned copy keeps running until something removes it. Each HUD here
  removes itself within a second of its player leaving the server, and a
  little later after a death, once the old character turns into `null`.

See [per-entity fan-out](../../best-practices.md#10-per-entity-work-a-for-sweep-or-one-chip-each)
and [Measured costs](../../best-practices.md#measured-costs) in the
reference.

Next: [Writing Libraries](libraries.md).
