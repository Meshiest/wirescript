# Zone Chime

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

</div>
<!-- /learn-links -->

A shop-door chime: a player who walks into an area hears a short sound.
Only the player who walked in hears it, so a busy doorway stays quiet for
everyone else.

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

- a brick with an **Attached Zone**, covering the doorway or area

```wiring
zone "Zone Ref" -> chip room
```

## Code

```wirescript name=bp6-zone-chime
const SOUND = $BrickOneShotAudioDescriptor/OBA_CashRegister_Ding // the chime
const PITCH = 1.0 // 1.0 is the sound as recorded; higher is higher

@left in room: zone

on ZoneEntered(zone = room) -> (character) {
  character.PlayClientAudio(SOUND, pitch = PITCH)
}
```

## Wire it

1. Compile the chip and paste it in-game. Give a brick an **Attached Zone**
   that covers the doorway.
2. Wire the Attached Zone's **Zone Ref** to `room`.

## Check it in game

- Walk into the zone: you hear the chime once.
- Stand inside: nothing more plays.
- Walk out and back in: it chimes again.
- Ask a friend to watch from outside while you walk in: they hear nothing.

## Change it

- Pick another sound: type `$BrickOneShotAudioDescriptor/` in the
  playground, and the editor lists the sounds.
  `OBA_UI_Checkpoint_Acquire_Tune_Cue` makes a checkpoint chime.
- Play a lower note on the way out:

  ```wirescript nocompile
  on ZoneLeft(zone = room) -> (character) {
    character.PlayClientAudio(SOUND, pitch = 0.7)
  }
  ```

- Let everyone nearby hear it: change `PlayClientAudio` to `PlayAudioAt`,
  which plays the sound at the player's position for all players in range.

## How it works

`@left in room: zone` takes the zone from the Attached Zone's **Zone Ref**,
and `on ZoneEntered(zone = room)` runs each time a player walks in, naming
that player `character` ([Reacting to the Game](../reacting-to-the-game.md)).
`$BrickOneShotAudioDescriptor/...` names one of the game's sounds.
`PlayClientAudio` plays it for that one player only, with no direction or
distance: it sounds the same wherever they stand.
