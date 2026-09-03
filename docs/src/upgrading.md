# Upgrading

Wirescript's syntax and gate catalog evolve as the game updates. This page
collects the **breaking changes** and how to migrate existing `.ws` code. The
complete, per-version log — every change, not only breaking ones — lives in the
repository's [`CHANGELOG.md`](../../CHANGELOG.md).

## 1.1.0

### Event handlers bind outputs with `->`

An event's data outputs are captured in a trailing `-> (…)` (tuple, positional —
the cleanest form) or `-> { … }` (record, by field name), **not** inside the
event call. The event call's parens now hold **config/inputs only** (a
custom-event channel, `isObject`, `zone`, `interval`, a `ChatCommand` name/help).
Binding outputs inside the call is an error.

```wirescript ignore
// before
on CharacterDied(character, killer) { … }
on CustomEvent("dmg", amount: int) { … }
```
```wirescript ignore
// after
on CharacterDied() -> (character, killer) { … }
on CustomEvent("dmg") -> (amount: int) { … }
```

Config stays in the parens; only data moves to `->`:

```wirescript ignore
on ChatCommand("greet", "Greets you") -> (controller, args) { … }
on ZoneEntered(zone = z) -> (character) { … }
```

### Event triggers are calls — `on RoundStart()`

An event trigger is written with `()`, uniform with the `on <call> -> (…)`
model. The no-parens form — a handler head (`on RoundStart { }`) or a
captured-event alias (`let x = on RoundStart`) — is now an error. The `()` makes
it clear the event is a gate/call.

```wirescript ignore
on RoundStart { … }        // before   ->   on RoundStart() { … }        // after
let x = on RoundStart      // before   ->   let x = on RoundStart()      // after
```

### Custom-event data types move to the capture (and are inferred)

Write a slot's type in the tuple capture, or omit it and let the compiler infer
it from the matching in-unit `SendCustomEvent`. When no sender supplies a type,
the slot defaults to `float` and warns **WS042** (replacing the old `WS029`
"annotate this param" lint).

```wirescript ignore
on CustomEvent("dmg") -> (amount: int, source) { … }
//                              ^annotated  ^inferred from the sender
on CharacterSpawned() -> (ch) { SendCustomEvent("dmg", 5, ch) }
```

### `emit` is for exec signals, not data

Set a data output with **`out name = value`** (in a handler or a chip/mod body)
or **`return value`** (a single-output `mod`). Reserve `emit` for firing an exec
signal (`emit done`) or ferrying a payload with a local signal you `await`
(`emit loop = value` … `await loop`).

### General-call triggers

`on <call> -> (…)` works for any exec-producing call — a `mod`/`chip` call, or a
gate driven by an `exec =` input (`on Foo(exec = go) -> (out)`). `on`
auto-extracts the call's exec output; a general call with no exec output (or an
`exec =` with nowhere to attach) is **`WS043`**.

### New whole-grid events

`on WholeGridInteracted() -> (character, held)` fires when the grid is interacted
with; `on WholeGridTargeted() -> (character, damage, weapon, weaponName)` fires
when it is hit.

---

For the full, per-version history — features, fixes, and migration notes — see
[`CHANGELOG.md`](../../CHANGELOG.md) at the repository root.
