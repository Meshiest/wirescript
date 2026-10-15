# Reading a Variable

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Variables](variables.md)

**Next steps:** [Buffers](buffers.md)

</div>
<!-- /learn-links -->

There are two ways to read a variable, and they can give different answers in
the same moment. This lesson shows when each one sees a new value.

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
button Held -> chip pressed
chip text -> decal Text
```

## Code

```wirescript name=st2-before-after
@left in pressed: exec
var count: int = 0
var before: int = 0
var after: int = 0
@right out text = "before ${before.Value}, after ${after.Value}, now ${count.Value}"

on pressed {
  before = count
  count += 1
  after = count
}
```

Inside the handler, the bare name `count` reads the variable at that step of
the chain. `before = count` runs before the add, and `after = count` runs
after it. Outside the handler, `count.Value` is a pure wire from the Variable
gate.

What does the sign say after the first press? After the second?

## Run

1. Compile the chip and paste it in-game. Place a **Button** and a brick with
   a **Text Decal**.
2. Wire the Button's **Held** to `pressed`, and `text` to the Text Decal's
   **Text**.
3. Press once. The sign reads "before 0, after 1, now 1". Each press moves all
   three along by one.

## Under the Hood

Open the chip. The handler's chain has two Get Variable gates for `count`: one
before the Increment Variable and one after it. Each Get Variable reads the
variable when the chain reaches it, so the second one sees the new value.

The text has no Get Variable at all. Its Format String gate is wired straight
from the Variable gates' value outputs.

The game updates every wire about 60 times a second. Each update is a tick. A
handler's steps all run inside one tick, one after another. Pure gates, and
the `.Value` wires they read, settle once per tick, so while a handler runs
they still show what the variable held when the tick began. The sign shows the
new count one tick after the press, far too soon to notice.

## Modify

1. Swap the first two lines of the handler so `count += 1` comes first. Now
   `before` and `after` always match.
2. Build some text from `count.Value` in a `let`, and copy it in the handler:

   ```wirescript name=st2-stale
   @left in pressed: exec
   var count: int = 0
   var copied: string = ""
   let label = "count is ${count.Value}"
   @right out text = copied.Value

   on pressed {
     count += 1
     copied = label
   }
   ```

   Press once: the sign says "count is 0". `label` is a pure wire, and at the
   moment the handler copies it, it has not caught up with the add yet. Each
   press shows the count from before that press.

3. Fix it by building the text inside the handler, where the bare name reads
   the new value:

   ```wirescript name=st2-fresh
   @left in pressed: exec
   var count: int = 0
   var copied: string = ""
   @right out text = copied.Value

   on pressed {
     count += 1
     copied = "count is ${count}"
   }
   ```

   When the text only shows the count, it needs no copy at all:
   `@right out text = "count is ${count.Value}"` builds it outside the
   handler, with no extra variable, and follows each press a tick later.

## Make

Make a dice sign. Each press of a button rolls a new number with
`Random(1, 6)`, which gives a whole number from 1 to 6 and, like a variable
write, runs as a step inside a handler. The sign shows this roll and the one
before it: "now 4, before 2". Think about which line has to come first.

```wiring
button Held -> chip roll
chip text -> decal Text
```

## Good to Know

- Outside a handler, `count.Value` is the value as of the start of this tick.
  You will also see a bare `count` used outside a handler: it is the same
  read. These lessons write `.Value` so the delay is visible in the code.
- Inside a handler, `count.Value` is still that same pure wire, so after a
  write it shows the old value. Read the bare name, `count`, instead.
- Text built in a `let` and used by a handler that has just written one of its
  variables shows the value from before the write. It looks like a display
  that lags one press behind.

Next: [Buffers](buffers.md).
