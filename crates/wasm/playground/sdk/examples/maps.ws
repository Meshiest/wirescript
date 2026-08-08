// Maps - keyed variable collections (Map<K, V>)
//
// A map maps keys to values, backed by the MapVar gate. Keys must be int,
// string, or an object reference (entity/character/controller); values may be
// any wire-storable type. Methods run in exec context, like array methods.

// A constant literal initializer bakes the map pre-populated - no runtime gates.
var levelPar: Map<string, int> = { "easy" => 3, "normal" => 5, "hard" => 8 }

// Atom keys (:name is a compile-time int hash) read like an enum table.
var colorHex: Map<int, string> = { :red => "ff0000", :green => "00ff00" }

// Empty maps, built at runtime.
var scores: Map<string, int> = {}
var kills: Map<character, int> = {}      // one entry per character (object key)

in player: character
on player {
  // set / get / has / remove
  scores.set("alice", 10)
  scores.set("alice", 12)             // overwrite
  scores.set("bob", 7)

  let a = scores.get("alice")         // { Value, Found } - auto-unwraps to Value
  let hadBob = scores.has("bob")
  scores.remove("bob")                // -> bool (was present)
  let count = scores.length()

  // Object-keyed map: bump this character's kill count.
  let k = kills.get(player)
  kills.set(player, k.Value + 1)

  // Copy the keys into an array.
  var names: string[]
  scores.keys(names)

  let par = levelPar.get("hard")      // 8
  let red = colorHex.get(:red)        // "ff0000"

  player.DisplayText(
    "alice=${a.Value} hadBob=${hadBob} count=${count} par=${par.Value} red=${red.Value} kills=${kills.get(player).Value}",
    positionX = 0.0,
    positionY = 0.0,
    fontSize = 20,
    lifetime = 10.0,
    textId = 0
  )
}
