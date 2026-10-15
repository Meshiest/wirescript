# One Chip per Player

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Nested Prefabs](nested-prefabs.md), [Looping Over a Map](../loops/looping-over-a-map.md), [Seat Controls](../input/seat-controls.md)

**Next steps:** [Performance](../mastery/performance.md), [Multiplayer Architecture](../mastery/multiplayer.md)

</div>
<!-- /learn-links -->

Some work belongs to each player, all the time: following their keys, a
readout that stays with them. One chip cannot grow new gates for every
player who turns up, but it can spawn a small chip for each one that serves
only that player. This lesson gives every player in a room a live
clicker of their own.

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
zone "Zone Ref" -> chip room
```

## Code

Every player who walks into the room gets a pad: a spawned chip that
watches their Q key. Each press scores them a point.

```wirescript name=p7-pads
@left in room: zone
var pads: Map<string, entity>
var taps: Map<string, int>

on ZoneEntered(zone = room) -> (character) {
  let id = character.GetUserId()
  let mine = pads.get(id)
  if mine.Found {
    mine.Value.SendCustomEvent("pad.init", character)
    return
  }
  let pad = SpawnPrefab(
    prefab = $```
      @label("Pad")

      var owner: character
      var ready: bool = false
      var id: string = ""
      let keys = InputReader(owner.Value)

      on CustomEvent("pad.init", isObject = true) -> (who: character) {
        owner = who
        ready = true
        on Clock(interval = 1.0) {
          if ready {
            if owner == null {
              id = ReadBrickGrid().GetCustomProperty("owner")
              SendCustomEvent("pad.gone", id)
              ReadBrickGrid().DestroySpawnedPrefab()
            }
          }
        }
      }

      on CustomEvent("pad.stop", isObject = true) {
        ReadBrickGrid().DestroySpawnedPrefab()
      }

      on keys.PressedQ {
        SendCustomEvent("pad.q", owner)
      }
    ```,
    offset = Vec(0.0, 0.0, 300.0),
    limit = 64
  )
  pad.SetCustomProperty("owner", id)
  pads.set(id, pad)
  pad.SendCustomEvent("pad.init", character)
}

on ZoneLeft(zone = room) -> (character) {
  let id = character.GetUserId()
  let mine = pads.get(id)
  if mine.Found {
    mine.Value.SendCustomEvent("pad.stop")
    pads.remove(id)
  }
}

on CustomEvent("pad.gone") -> (id: string) {
  pads.remove(id)
}

on CustomEvent("pad.q") -> (who: character) {
  let id = who.GetUserId()
  let before = taps.get(id)
  let count = if before.Found then before.Value + 1 else 1
  taps.set(id, count)
  who.ShowStatusMessage("Taps: ${count}")
}
```

The central chip spawns the pads, tracks which pad is whose, and counts
the taps.

- `pads` maps each user id to that player's pad, the entity `SpawnPrefab`
  returned. A player who walks in gets a pad. The chip tags the pad with
  their user id (`SetCustomProperty("owner", id)`) and sends it
  `"pad.init"` with their character, targeted at that pad.
- A player who already has a pad gets no second one. Their pad is sent the
  character again instead. `return` ends a handler early, and nothing
  after it runs.
- A player who walks out is sent `"pad.stop"` and forgotten with
  `pads.remove(id)`, which deletes that key from the map. A pad whose
  player left the server or died reports `"pad.gone"` with the id, and
  that entry is removed the same way.
- `taps` counts each player's presses, and `ShowStatusMessage` shows the
  count to that player only.

Each pad works for one player.

- `owner` stores the player from `"pad.init"`, and `ready` records that it
  has arrived.
- `InputReader(owner.Value)` follows that player's keys, live, as in
  [Seat Controls](../input/seat-controls.md).
- `on keys.PressedQ` runs once per press and sends `"pad.q"` back, with no
  target, so it reaches the central chip.
- `"pad.stop"` makes the pad remove itself.
- The Clock checks once a second that the stored character is still
  there. It turns into `null` when the player leaves the server, and also
  some time after the player dies. The pad then reads its
  user id back from its own tag (`ReadBrickGrid().GetCustomProperty`),
  reports `"pad.gone"`, and removes itself. The Clock is written inside
  the init handler to keep it next to the `owner` it checks, but it runs
  from the moment the pad appears, so it checks `ready` first.

Ten players are in the room. How many Seat Control Splitters are there,
and how many of them are in the central chip?

## Run

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   covering the room, and wire its **Zone Ref** to `room`.
2. Walk in. A pad appears above the chip. Press Q: the status line says
   `Taps: 1`, then counts up with each press.
3. Walk out: the pad vanishes, and Q does nothing. Walk back in: a new pad
   appears, and the count carries on from where it was.
4. With a second player: each of you gets a pad and a count of your own,
   and presses at the same moment both count.

## Under the Hood

The central chip has a **Player Entered Zone Event** and a **Player Left
Zone Event**, each followed by a **Get User Id**, a **Get Map Element** and
a **Branch**. The enter chain goes on to a **Spawn Prefab**, a **Set Map
Element**, and a **Send Personal Custom Event** with the new pad on its
**Target** input. A **Personal Custom Event** for `"pad.q"` starts the
counting chain.

The enter chain also has a **Set Entity Custom Property** that tags the
new pad. A third **Personal Custom Event**, for `"pad.gone"`, removes a map
entry.

Open a pad. A **Var** holds the owner, and its value is wired into a
**Seat Control Splitter**'s **Character**. **Pressed Q** fires a **Get
Variable** and a **Send Personal Custom Event**. Two object-scoped
**Personal Custom Event** receivers handle `"pad.init"` and `"pad.stop"`.
The second is a **Read Brick Grid** into a **Destroy Spawned Prefab**. The
**Clock** chain reads `ready` and `owner`, and for a player who has gone
runs a **Get Entity Custom Property**, a send and a second **Destroy
Spawned Prefab**.

Spawned chips are needed because a Seat Control Splitter reads one
character, wired in. A pasted chip has a fixed set of gates, and gates
cannot be added while the game runs, so one chip cannot hold a splitter for
every player who might come. Without spawned chips, it could only loop over
the room on a timer and read each player with `GetInputs`. That runs on
every pulse whether anyone presses or not, and misses a tap between two
pulses. A pad does nothing until its player presses a key, and then sends
exactly one event.

Each shape has its use.

- **A `for` sweep** in one chip ([Looping Over a
  Map](../loops/looping-over-a-map.md)) is the simpler choice when the work
  happens now and then and needs every player together: comparing them,
  ranking them, totalling them. Its cost grows with the number of players.
- **One chip per player** suits work that belongs to one player: live
  controls, a display that follows them, work every tick. Each pad costs
  the same however many players there are, and is gone when its player is.

Keep the shared records, such as `taps`, in the central chip, and send out
only the per-player part.

## Modify

1. Add a second key: the pad also sends `"pad.e"` on `keys.PressedE`, and
   the central chip takes a point away for it.
2. Send the count back to the pad: after `taps.set`, look up the player's
   pad in `pads` and send it a targeted `"pad.score"` with the count. In the
   pad, show it with `owner.ShowStatusMessage` instead of the central chip
   doing it.
3. Show the best score in the room on a sign. In which chip does that
   belong, and why?

## Make

Make a buzzer room for a quiz. Every player in the room gets a pad. When the
host presses a Button, the first Q press from any pad wins: a sign shows the
winner's name, escaped as in [Safe Text](../strings/safe-text.md), and
further presses are ignored until the host presses the button again.

```wiring
zone "Zone Ref" -> chip room
button Held -> chip open
chip sign -> decal Text
```

## Good to Know

- A copy's handlers start when it appears, before `"pad.init"` lands. Until
  then `owner` is empty, and an empty character gives the splitter no
  player to read.
- A respawn makes a new character, and the pad still holds the old one.
  A death inside the room also leaves with an empty **Character**, so
  `"pad.stop"` is not sent. The old character turns into `null` a little
  after the death, and the pad then reports `"pad.gone"` and removes
  itself. If the player walks back in before that, their pad is sent the
  new character instead, found again in the central map by user id.
  Walking back in after it has gone spawns a new pad.
- Each pad has its own send gate. A send gate fired many times in one tick
  delivers once, but many different gates on one channel all deliver, so
  presses from different players in the same tick all count.
- Over a long session a zone event can be missed. A missed leave leaves a
  pad that still counts its player's taps outside the room. A missed
  entry leaves a player with no pad until they walk in again. Where that
  matters, a slow Clock can fill an array with the room's players
  (`fillFromZonePlayers`) and stop the pads of anyone not in it.
- `limit = 64` is a ceiling on pads. Raise it if more players may come.
- `SpawnPrefab` is the one thing that makes more gates while the game runs.
  A central chip has to be built for the largest crowd you expect. Pads
  cost only for the players who are there.

See [per-entity fan-out](../../best-practices.md#10-per-entity-work-a-for-sweep-or-one-chip-each),
[InputReader](../../builtins.md#inputreader) and
[Custom Events](../../builtins.md#custom-events) in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
