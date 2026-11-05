// Spawned prefab copies: each runs its own Load Event with its own storage, a
// limit of 2 destroys the oldest, and a destroyed copy stops running.
buffer tick: int = tick + 1
var third: entity

on tick {
  if tick == 1 || tick == 3 || tick == 5 {
    let copy = SpawnPrefab(prefab = $```
      buffer age: int = age + 1
      var said: bool = false
      on Loaded() {
        BroadcastChatMessage("copy up")
      }
      on age {
        if age == 6 && !said {
          said = true
          BroadcastChatMessage("copy still here")
        }
      }
    ```, lifetime = 0.0, limit = 2)
    if tick == 5 {
      third = copy
    }
  }
  if tick == 8 {
    third.DestroySpawnedPrefab()
  }
}
