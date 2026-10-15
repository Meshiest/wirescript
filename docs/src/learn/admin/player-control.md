# Player Control

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Persistent Players](persistent-players.md)

**Next steps:** [Moderation](moderation.md)

</div>
<!-- /learn-links -->

A circuit can decide when a player comes back to life, bring them back at
once, and heal or hurt them. This lesson builds an elimination game: die and
you are out until the next round, which a button starts.

Needs admin to load: it uses **Player Died Event**, **Get Player
(Persistent)**, **Set Can Respawn** and **Force Respawn Player**, and the
health chip uses **Set Player Damage** and **Inc Player Damage**.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Health](#health)
- [Temporary permissions](#temporary-permissions)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
button Held -> chip revive
chip sign -> decal Text
```

## Code

```wirescript name=a4-elimination admin
@left in revive: exec
var benched: controller[]
var shown: string = "Nobody is out"
@right out sign = shown.Value

on CharacterDied() -> (character) {
  let p = character.ControllerOf()
  p.SetCanRespawn(false)
  benched.push(p)
  shown = "Out: ${benched.length()}"
}

on revive {
  for p in benched {
    p.SetCanRespawn(true)
    p.ForceRespawn()
  }
  benched.clear()
  shown = "Nobody is out"
}
```

- `on CharacterDied() -> (character)` runs when any player in the world dies,
  with the character that died.
- `ControllerOf()` turns that character into the player, as in
  [Persistent Players](persistent-players.md). The player is what can be
  stopped from respawning, and it is still there to bring back later.
- `SetCanRespawn(false)` keeps them out. `benched` remembers who is out.
- The button lets everyone respawn again, respawns them at once with
  `ForceRespawn()`, and empties the list.

Two players die, one after the other. What does the sign say? Then the
button is pressed. Where do the two players appear?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with
   a **Text Decal**.
2. Wire the Button's **Held** to `revive` and `sign` to the Text Decal's
   **Text**.
3. Fall off the map, or ask a friend to. The player who died does not come
   back, and the sign counts them.
4. Press the button. Everyone who was out respawns, at a spawn point as
   usual, and the sign resets.

## Under the Hood

Open the chip. **Player Died Event** has no input: the game fires it. Its
**Character** goes into **Get Player (Persistent)**, and that gate's **Player
State** output feeds **Set Can Respawn**, with its **Can Respawn** input set
off, and **Push To Array**.

In the loop, the same player reference feeds **Set Can Respawn**, now on, and
**Force Respawn Player**. After the loop, **Clear Array** empties the list.

The list stores the persistent player rather than the character, because the
character that died is gone. The player stays the same, so the list still
reaches them when the button is pressed.

## Health

A player takes damage until it reaches their limit, and then dies. Reading
both numbers is not admin. This scale shows them for whoever steps on it:

```wirescript name=a4-scale
@left in scale: zone
var shown: string = "Step on the scale"
@right out sign = shown.Value

on ZoneEntered(zone = scale) -> (character) {
  let d = character.GetDamage()
  shown = "Damage: ${d.Damage} of ${d.DamageLimit}"
}
```

Changing damage is admin. `SetDamage` sets it, so `0.0` heals fully, and
`IncDamage` adds to it, so a negative amount heals a little. This chip has a
pit that hurts everyone in it each second, and a pad that heals:

```wirescript name=a4-pit admin
const HURT = 5.0

@left in pit: zone
@left in burning: bool
@left in pad: zone
var inside: character[]

on Clock(interval = 1.0, enabled = burning) {
  inside.fillFromZonePlayers(pit)
  for c in inside {
    c.IncDamage(HURT)
  }
}

on ZoneEntered(zone = pad) -> (character) {
  character.SetDamage(0.0)
  character.ShowStatusMessage("Healed")
}
```

`fillFromZonePlayers` fills the list with the characters standing in the pit
right now. Wire the pit's Attached Zone **Occupied** to `burning`, so the
Clock only runs while someone is in it. Damage belongs to a body, so these
work on a character, not on a player.

## Temporary permissions

`character.SetTempPermission(name, enable)` overrides one of the team's
gameplay permissions for one character: `true` allows it, `false` denies it.
It is not admin. The `name` is one of the game's gameplay permissions,
written as the game names it in a team's settings. Because it is set on the
character, expect a respawn to start from the team's settings again.

## Modify

1. Announce each elimination to the player who died:
   `p.ShowChatMessage("You are out until the next round")`.
2. End the round automatically: after the push, if only one player is left
   alive, revive everyone. You will need the number of players in the game,
   which [Every Player](every-player.md) covers. Until then, set it with a
   `const PLAYERS = 4`.
3. Turn the heal pad into a slow heal: a second Clock, enabled by the pad's
   **Occupied**, that runs `c.IncDamage(-2.0)` for everyone on the pad each
   second.

## Make

Make a lives system. Each player has three lives, kept in a
`Map<string, int>` under their user id. A death takes a life, and the player
is benched only when they reach zero. A button gives everyone three lives again
and revives the benched.

```wiring
button Held -> chip revive
```

## Good to Know

- A player who cannot respawn waits as a spectator. Players in builder mode
  are not moved to spectator.
- `ForceRespawn()` removes the player's current character, if they have one,
  and spawns a new one by the game mode's rules.
- `on CharacterDied` runs for every death in the world, from any cause:
  falling, weapons, damage from another circuit.
- `SetDamage` keeps damage at zero or above, so a heal cannot give a player
  more than full health.

See [Player info](../../builtins.md#player-info),
[Damage](../../builtins.md#damage) and
[SetTempPermission](../../builtins.md#settemppermission) in the reference.

Next: [Chat Commands](chat-commands.md).
