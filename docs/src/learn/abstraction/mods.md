# Mods

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](../reacting-to-the-game.md)

**Next steps:** [Chips](chips.md), [Tuples](../data/tuples.md), [Option and Result](../enums/option-and-result.md), [Ref Parameters](../references/ref-parameters.md), [Every Player](../admin/every-player.md), [Testing](../mastery/testing.md)

**Parts that use this:** [Combination Lock](../parts/combination-lock.md), [Checkpoints](../parts/checkpoints.md), [Race Timer](../parts/race-timer.md), [Shop](../parts/shop.md), [Voting](../parts/voting.md), [Obstacle Course](../parts/obstacle-course.md), [Whack-a-Light](../parts/arcade-game.md), [Card Table](../parts/card-game.md)

</div>
<!-- /learn-links -->

A `mod` gives a piece of circuit a name, so you can write it once and use it
in many places. Every place that uses it gets its own copy of the gates.

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
zone1 Occupied -> chip busy1
zone2 Occupied -> chip busy2
chip sign1 -> decal1 Text
chip sign2 -> decal2 Text
```

## Code

```wirescript name=g1-room-signs
@left in busy1: bool
@left in busy2: bool

mod status(name: string, busy: bool) -> string {
  return if busy then "${name}: busy" else "${name}: free"
}

@right out sign1 = status("Workshop", busy1)
@right out sign2 = status("Storage", busy2)
```

- `mod status(name: string, busy: bool)` declares a mod with two inputs, like
  the pins of a small chip.
- `-> string` says it gives back one value, a piece of text.
- `return` names that value. Here it is one `if ... then ... else`
  expression: `if busy then A else B` is `A` while `busy` is true and `B`
  otherwise. Choices can chain: `if a then X else if b then Y else Z`.
- `status("Workshop", busy1)` is a call. It runs the mod's body with `name`
  set to `"Workshop"` and `busy` set to `busy1`.

The mod is written once and called twice. How many gates does the chip hold?
Which of them belongs to `status`?

## Run

1. Compile the chip and paste it in-game. Build two small rooms. Give a brick
   in each an **Attached Zone** that fills the room, and place a **Text
   Decal** by each door.
2. Wire the first zone's **Occupied** to `busy1` and the second's to `busy2`.
3. Wire `sign1` and `sign2` to the two Text Decals' **Text**.
4. Walk into the workshop and its sign says `Workshop: busy`. Walk out and it
   says `Workshop: free`. The storage sign does not change.

## Under the Hood

Open the chip. There are two **Select** gates and nothing named `status`.

Each call became its own Select. Click one. Its two text inputs are typed
in, `Workshop: free` and `Workshop: busy`. The other Select holds the storage
texts. The name was a constant, so the compiler worked out both texts ahead
of time and typed them into that call's copy.

The compiler pastes a mod's body in at each call and fills in the inputs.
One call is one Select, and ten calls would be ten. In a code editor, hover over `status` to see "inlined per
call", the compiler's reminder of this.

A mod can also choose its value step by step, with `if` statements and an
early `return`:

```wirescript name=g1-early-return expect=WS007
@left in busy1: bool

mod status(name: string, busy: bool) -> string {
  if busy { return "${name}: busy" }
  return "${name}: free"
}

@right out sign1 = status("Workshop", busy1)
```

```text
Error: [WS007] `status` returns from inside a branch, so choosing between its returns needs an exec context: call it from a handler or a `mod` body, or pass `exec = ...`
```

`if` statements run one step after another, so they need a pulse to run on.
An `out` pin has none. Write the choice as one expression, as in the first
version, or call the mod inside an `on` handler, where there is a pulse.

## Modify

1. Add a third room, an office. Add `@left in busy3: bool` and
   `@right out sign3 = status("Office", busy3)`. Open the chip again and there
   is a third Select.

2. Give `status` two outputs by naming them. A light by each door shows
   whether the room is free:

   ```wirescript name=g1-two-outputs
   @left in busy1: bool
   @left in busy2: bool

   mod status(name: string, busy: bool) -> (text: string, lit: bool) {
     out text = if busy then "${name}: busy" else "${name}: free"
     out lit = !busy
   }

   let workshop = status("Workshop", busy1)
   let storage = status("Storage", busy2)
   @right out sign1 = workshop.text
   @right out sign2 = storage.text
   @right out free1 = workshop.lit
   @right out free2 = storage.lit
   ```

   With named outputs, a call gives back a record, and `workshop.text`
   and `workshop.lit` read its two values. Wire `free1` and `free2` to two
   **Point Light**s' **Enabled**.

3. Count how many times each room has been used. A mod can change a
   variable it is given, if the input is marked with `*`:

   ```wirescript name=g1-visits
   @left in busy1: bool
   @left in busy2: bool
   var visits1: int = 0
   var visits2: int = 0

   mod status(name: string, busy: bool) -> string {
     return if busy then "${name}: busy" else "${name}: free"
   }

   mod bump(count: *int, by: int = 1) {
     count += by
   }

   on busy1 { bump(visits1) }
   on busy2 { bump(visits2) }
   @right out sign1 = "${status("Workshop", busy1)}<br>${visits1.Value} visits"
   @right out sign2 = "${status("Storage", busy2)}<br>${visits2.Value} visits"
   ```

   `count: *int` means "a variable holding an int", not a copy of its value,
   so `bump(visits1)` adds to `visits1` itself. [Ref Parameters](../references/ref-parameters.md)
   covers `*` in full. `by: int = 1` is a default, so a call that leaves `by` out uses 1.
   `<br>` in the sign's text starts a new line.
   Change the storage room to count each visit twice, with `bump(visits2, 2)`.

   `on busy1` fires when the room goes from empty to occupied, so a second
   person walking in while the first is still there is not counted.

## Make

Make a sign for a parking area with three bays, each with its own zone. Write
a mod that takes a bay's number and whether it is taken, and returns one line
of text such as `Bay 2: taken`. Show all three lines on one sign, and add a
fourth line that says `Full` when every bay is taken.

```wiring
zone1 Occupied -> chip bay1
zone2 Occupied -> chip bay2
zone3 Occupied -> chip bay3
chip sign -> decal Text
```

## Good to Know

- Every call is a full copy. A mod of 50 gates called from ten places is 500
  gates, and nothing warns you. [Counting the Cost](cost.md) shows how to
  check.
- A `static var`, a variable declared inside a mod that keeps its value
  between runs, is copied too, so each call has its own. Two calls never
  share a count.
- A zone's **Occupied** stays on while anyone is inside, so the sign shows
  `busy` until the last person leaves.

See also: [`mod`: Inline Chips](../../chips.md#mod-inline-chips),
[Default and Named Arguments](../../chips.md#default-and-named-arguments) and
[`return`](../../statements.md#return).

Next: [Chips](chips.md).
