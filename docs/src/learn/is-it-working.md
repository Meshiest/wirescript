# Is It Working?

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Reacting to the Game](reacting-to-the-game.md)

**Next steps:** [Admin Gates](admin/admin-gates.md)

</div>
<!-- /learn-links -->

When a circuit does nothing, the compiler usually cannot tell you why. The
wires are valid but do not do what you meant. This lesson is about
seeing inside a running circuit.

<!-- toc -->
## Contents

- [Show the values](#show-the-values)
- [Silent failures to check first](#silent-failures-to-check-first)
- [Testing alone](#testing-alone)
- [Make](#make)
<!-- /toc -->

## Show the values

Add a debug output that turns the circuit's state into text, and wire it to a
**Text Decal**. To see each time a handler runs, send a chat line
from it:

```wirescript name=debug-pin admin
const NEEDED = 3

@left in pressed: bool
var count: int = 0
@right out lit = count.Value >= NEEDED
@right out debug = "pressed: ${pressed}  count: ${count.Value}  lit: ${lit}"

on pressed {
  count += 1
  BroadcastChatMessage("button pressed, count is now ${count}")
}
```

The sign shows what the circuit sees, live, and the chat line appears each time
the handler runs. If no line appears when you press, the handler's trigger is
the problem, not its steps. Remove both once the circuit works.

`BroadcastChatMessage` sends to everyone in the world, and only an admin can
load a chip that uses it. Use it on your own server while you debug, and take
it out before you share the circuit.

## Silent failures to check first

- **A pin is not wired.** An unwired input reads as zero, false or empty. If
  a handler never runs, check its trigger pin's wire first.
- **A wire goes to the wrong port.** Components have many inputs. The light's
  **Enabled** and **Cast Shadows** are both bools.
- **Reading a list past its end** gives the last value read, not zero, so a
  wrong index can look almost right.
- **A handler that waits forever.** A chain paused at an `await` that never
  resumes runs the steps before it and never the ones after. The circuit looks
  half working.

## Testing alone

Most circuits can be tested by one player: buttons, switches, zones and
clocks all work with only you in the world. When a circuit is for several
players, test each piece alone first, then together.

## Make

Take the circuit from [Remembering Values](state.md) and add a debug pin that shows the
count and whether the last press was the button's first.

```wiring
button Held -> chip pressed
chip text -> decal1 Text
chip debug -> decal2 Text
```

Next: pick a lesson from the tree on the [Learn](README.md) page.
