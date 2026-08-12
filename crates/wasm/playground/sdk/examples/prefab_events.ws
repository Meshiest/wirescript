/// Storage of prefabs by player id
var prefabs: Map<string, entity>

on CharacterSpawned() -> (p) {
  // Spawn a prefab when the player spawns
  let e = SpawnPrefab(lifetime = 0, limit = 50, offset = Vec(0, 0, 10), $```
    // Label the microchip by the player's name
    @label(playerName)

    var playerName =""

    on CustomEvent("init", isObject = true) -> (p: character) {
      let name = p.GetDisplayName()
      let id = p.GetUserId()
      playerName = name
      BroadcastChatMessage("Spawned ${name} ${p} - ${id}")

      on ServerUptime() {
        let d = p.GetDamage()
        let id = p.GetUserId()
        // Every tick, check the player's damage or if the player leaves
        if d.Damage >= d.DamageLimit || id == "" {

          // Destroy the prefab on player death
          BroadcastChatMessage("Despawned ${name}")
          ReadBrickGrid().DestroySpawnedPrefab()
        }
      }
    }
    ```)

  // Send the event to the entity and freeze it
  e.SendCustomEvent("init", p)
  e.SetFrozen(true)

  // Store the prefab in the map
  prefabs.set(p.GetUserId(), e)
}
