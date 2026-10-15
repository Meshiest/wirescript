# Records in Storage

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Records](records.md)

**Next steps:** [Tuples](tuples.md)

</div>
<!-- /learn-links -->

A record can live in an array or a map as well as in a `var`. This lesson
keeps a record for every player who hits a target, and counts what that
costs: one storage gate per field, however many players there are.

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
target "Just Hit" -> chip hit
target "Character That Just Hit" -> chip shooter
chip text -> decal Text
```

## Code

```wirescript name=d2-target-hits
import { escape } from "sanitize"

type Stats = { name: string, hits: int }

@left in hit: exec
@left in shooter: character
var stats: Map<string, Stats>
var shown: string = "No hits yet"
@right out text = shown.Value

on hit {
  let id = shooter.GetUserId()
  let before: Stats = if stats.has(id) then stats.get(id) else { name: "", hits: 0 }
  let next: Stats = { name: shooter.GetDisplayName(), hits: before.hits + 1 }
  stats.set(id, next)
  shown = "${escape(next.name)}: ${next.hits} hits"
}
```

`Map<string, Stats>` files a whole `Stats` record under each player's user
id. `stats.has(id)` checks whether this player has a record yet. If so,
`stats.get(id)` reads it. If not, the `else` starts them at zero. `next` is
the updated record, and `stats.set(id, next)` files it.

`stats` holds two fields for each player. How many map gates will the chip
need for it: one, two, or one per player?

## Run

1. Compile the chip and paste it in-game. Place a **Target** and a brick with
   a **Text Decal**.
2. Wire the Target's **Just Hit** to `hit`, its **Character That Just Hit** to
   `shooter`, and `text` to the Text Decal's **Text**.
3. Shoot the target a few times. The sign shows your name and your count. A
   second player's hits start their own count from 1.

## Under the Hood

Open the chip. There are two **Map Var** gates, labeled `stats.name` and
`stats.hits`. A map of records is one map per field, and it stays two
however many players it holds.

Every operation fans out across the fields. `stats.get(id)` is two **Get Map
Element** gates, one per map, and `stats.set(id, next)` is two **Set Map
Element** gates. `stats.has(id)` is one **Has Map Element**, since every field
has the same keys. `before` and `next` are records too, but they are values,
not storage, and have no gates of their own.

The `stats.name` read is there although nothing uses `before.name`, because
getting a whole record reads every field.

The same rule holds for each kind of storage:

```wirescript name=d2-storage
type Point = { x: int, y: int }

@left in step: exec
var here: Point = { x: 0, y: 0 }
var trail: Point[]
var marks: Map<string, Point>
@right out text = "At ${here.x.Value}, ${here.y.Value}"

on step {
  let next: Point = { x: here.x + 1, y: here.y + 2 }
  here = next
  trail.push(next)
  marks.set("last", next)
}
```

| Declaration | Storage gates |
|---|---|
| `var here: Point` | 2 **Var** |
| `var trail: Point[]` | 2 **Array Var** |
| `var marks: Map<string, Point>` | 2 **Map Var** |
| `let next: Point` | none |

`trail.push(next)` is two **Push To Array** gates, one per column. Some array
operations make no sense across columns, and the compiler refuses them:

```wirescript name=d2-sort expect=WS050
type Point = { x: int, y: int }

@left in step: exec
var trail: Point[]

on step {
  trail.push({ x: 1, y: 2 })
  trail.sort()
}
```

```text
Error: [WS050] `.sort()` is not supported on a record array - it has no per-field meaning (sorting/shuffling would desync the fields, and the aggregate/dual-array forms are not implemented). Use `pts[i]` for element access, or store a scalar array
```

Sort by one field instead: `trail.x.sort()` sorts the rows by `x` and moves
every `y` along with its row.

## Modify

1. Read only the field you need. Replace the `before` line with
   `let hits = if stats.has(id) then stats.get(id).hits else 0`, and use
   `hits + 1` in `next`. The chip now has one Get Map Element instead of two.
2. Add a `lastHit: int` field set to `GetUnixTime()` on every hit, and show
   how many seconds passed since that player's previous hit.
3. Keep a leader. Add `var leader: Stats = { name: "nobody", hits: 0 }`, and
   after each hit, if `next.hits > leader.hits`, set `leader = next`. Show
   the leader on a second sign. Count the gates the leader adds.

## Make

Make a guest book for a zone. Keep each player's name, number of visits and
the Unix time of their first visit in one `Map<string, Visitor>`. When a
player walks in, greet them with their visit count and how long ago they
first came.

```wiring
zone "Zone Ref" -> chip room
chip text -> decal Text
```

## Good to Know

- A Target's **Just Hit** stays on for the Target's **On Time** setting.
  The handler runs when it turns on, so a second hit that lands while it is
  still on is likely to be missed. Keep **On Time** short.
- A missing key is not a zero record you can count on. Check `has()` before
  reading a record map.
- Key players by `GetUserId()`, not by the character. A player who dies comes
  back as a new character, and would start a new record.

See [Records as storage](../../types.md#records-as-storage) for every
operation a record array or map supports.

Next: [Tuples](tuples.md).
