// Guards sortMultiple's parallel arrays: a record array argument expands to one
// array per field. Wired into the `descending` pin instead, the keys sort and
// every record column stays put, with no diagnostic.

var pass: int = 0
var total: int = 0

mod assert<T: int | float | string>(want: T, got: T, label: string) {
  total = total + 1
  let ok = want == got
  pass = pass + ok
  if !ok {
    BroadcastChatMessage("FAIL: ${label} want ${want} got ${got}")
  }
}

type Row = { name: string, pts: int, bonus: float }

var keys: int[]
var order: int[]
var rows: Row[]
var uids: string[]

var origKeys: int[]
var origNames: string[]
var origPts: int[]
var origBonus: float[]
var origUids: string[]

var fires: int = 0

mod add(key: int, name: string, pts: int, bonus: float, uid: string) {
  order.push(keys.length())
  keys.push(key)
  rows.push({ name: name, pts: pts, bonus: bonus })
  uids.push(uid)
  origKeys.push(key)
  origNames.push(name)
  origPts.push(pts)
  origBonus.push(bonus)
  origUids.push(uid)
}

mod checkRow(k: int, label: string) {
  let i = order[k]
  assert(origKeys[i], keys[k], "${label} key ${k}")
  assert(origUids[i], uids[k], "${label} uid ${k}")
  assert(origNames[i], rows[k].name, "${label} row name ${k}")
  assert(origPts[i], rows[k].pts, "${label} row pts ${k}")
  assert(origBonus[i], rows[k].bonus, "${label} row bonus ${k}")
}

on ReadBrickGrid() {
  fires += 1
  if fires > 1 { return }

  add(20, "twenty", 201, 2.5, "u20")
  add(50, "fifty", 502, 5.5, "u50")
  add(10, "ten", 103, 1.5, "u10")
  add(40, "forty", 404, 4.5, "u40")

  keys.sortMultiple(order, rows, uids, true)
  assert(50, keys[0], "descending: largest key first")
  assert(10, keys[3], "descending: smallest key last")
  assert("fifty", rows[0].name, "descending: record row follows its key")
  assert(502, rows[0].pts, "descending: record int column follows its key")
  assert(5.5, rows[0].bonus, "descending: record float column follows its key")
  assert("u50", uids[0], "descending: uid follows its key")
  assert(1, order[0], "descending: order follows its key")
  checkRow(0, "descending")
  checkRow(1, "descending")
  checkRow(2, "descending")
  checkRow(3, "descending")

  keys.sortMultiple(order, rows, uids, false)
  assert(10, keys[0], "ascending: smallest key first")
  assert(50, keys[3], "ascending: largest key last")
  assert("ten", rows[0].name, "ascending: record row follows its key")
  assert(103, rows[0].pts, "ascending: record int column follows its key")
  assert(1.5, rows[0].bonus, "ascending: record float column follows its key")
  assert("u10", uids[0], "ascending: uid follows its key")
  assert(2, order[0], "ascending: order follows its key")
  checkRow(0, "ascending")
  checkRow(1, "ascending")
  checkRow(2, "ascending")
  checkRow(3, "ascending")

  BroadcastChatMessage("race sort: ${pass}/${total}")
}
