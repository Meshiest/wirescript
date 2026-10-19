# Forking and Merging

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Waiting](waiting.md)

**Next steps:** [World Control](../admin/world-control.md)

**Parts that use this:** [Paint Gun](../parts/paint-gun.md)

</div>
<!-- /learn-links -->

An exec chain does not have to be a single line. It can split one way or the
other, split into two paths that both run, and join back together. This
lesson builds a shop counter with an open sign and a bell, then takes its
pulses apart and puts them back together.

<!-- toc -->
## Contents

- [Code](#code)
- [Run](#run)
- [Under the Hood](#under-the-hood)
- [Merging with Union](#merging-with-union)
- [Forking with `.exec`](#forking-with-exec)
- [Naming a handler](#naming-a-handler)
- [Modify](#modify)
- [Make](#make)
- [Good to Know](#good-to-know)
<!-- /toc -->

```wiring
switch Enabled -> chip staffed
button Held -> chip bell
chip sign -> decal Text
chip chime -> speaker Play
```

## Code

```wirescript name=e4-shop
@left in staffed: bool
@left in bell: exec
var shown: string = "Closed"
@right out sign = shown.Value
@right out chime: exec

on staffed {
  shown = "Open"
}

on !staffed {
  shown = "Closed"
}

let door = Branch(staffed, bell)

on door.A {
  emit chime
}

on door.B {
  shown = "Closed. Back soon"
}
```

`on !staffed` runs when `staffed` turns off, the opposite edge to
`on staffed`.

`Branch(staffed, bell)` is a gate that sends each `bell` pulse one of two
ways: out of `.A` while `staffed` is on, out of `.B` while it is off.
`on door.A` and `on door.B` are handlers on those two outputs.

The switch is off and someone rings. What does the sign say, and does the
chime play? What happens when the switch is then flipped on and off again?

## Run

1. Compile the chip and paste it in-game. Place a **Switch** behind the
   counter, a **Button** in front of it, a brick with a **Text Decal** and a
   **One Shot Audio** brick with a sound picked in its settings.
2. Wire the Switch's **Enabled** to `staffed` and the Button's **Held** to
   `bell`. Wire `sign` to the Text Decal's **Text** and `chime` to the One
   Shot Audio's **Play**.
3. Flip the switch on and the sign says "Open". Ring, and the chime plays.
4. Flip the switch off and ring: no chime, and the sign asks customers to
   come back.

## Under the Hood

Open the chip. `on staffed` is a Set Variable wired straight from `staffed`.
`on !staffed` puts a **NOT** gate in front of its Set Variable, so the chain
starts when the NOT turns on, which is when `staffed` turns off.

There is one **Branch** gate. `staffed` goes into its **Cond** input and
`bell` into its **Exec**. **Exec Out A** is wired straight to the `chime`
pin, and **Exec Out B** to the Set Variable for the "Back soon" sign.

An `if` inside a handler compiles to this same Branch gate. `Branch(...)` is
the gate itself, for when there is no handler to put an `if` in.

`door.A` reads a field of a result, and `on` can trigger on a field like
that, the same as on a pin.

## Merging with Union

Two exec wires cannot go into one input. To run one handler from two
pulses, merge them with a **Union** gate:

```wirescript name=e4-two-doors
@left in front: exec
@left in back: exec
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

let rang = Union(front, back)

on rang {
  rings += 1
}
```

`rang` is a named exec, a Union of the two buttons. It fires when either
does. You can also write the call straight into the trigger:
`on Union(front, back) { ... }`.

`||` is not a merge. It makes one value, true while either side is true, so
`on front || back` misses a press of one button while the other is held.
With an object on one side, it is true for as long as the object exists,
and the compiler warns:

```wirescript name=e4-or expect=WS116
@left in front: bool
@left in rider: character
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"

on front || rider {
  rings += 1
}
```

```text
WARN [WS116] `||` here makes one value, it does not wait for either side: an object is true for as long as it exists, so this handler fires once, when the object appears, and never again. To fire on either, write `on Union(a, b)`
```

## Forking with `.exec`

One exec output wired into two inputs runs both. That is how one pulse
starts two things at once. A call normally joins the chain, so the next
line waits until it has finished.

```wirescript name=e4-flash
@left in bell: exec
var lit: bool = false
var rings: int = 0
@right out light = lit.Value
@right out sign = "Rings: ${rings.Value}"

chip Flash() {
  lit = true
  await Sleep(_, delay = 1.0)
  lit = false
}

on bell {
  let _ = Flash().exec
  rings += 1
}
```

`chip Flash() { ... }` names a block of steps that runs wherever `Flash()` is
called, on a microchip brick of its own.
[Chips](../abstraction/chips.md) covers chips in full. `Flash()` lights the
lamp for a second. `.exec` on the call names the pulse
it sends when it has finished, and `let _ =` lets that pulse go unused. The
call then runs beside the chain instead of in it, so the count goes up at
once. Write plain `Flash()` instead and the sign changes a second late,
because `rings += 1` waits for the flash.

To run something after the flash, keep the result and trigger on its
`.exec`:

```wirescript nocompile
let f = Flash(exec = bell)

on f.exec {
  flashes += 1
}
```

`exec = bell` starts the flash from `bell` with no handler around it, as
`Random(..., exec = ...)` did in [Waiting](waiting.md). `on f.exec` runs
when it has finished.

## Naming a handler

`let ... = on` gives a handler a name, and the name is a trigger that fires
after the handler's body has run:

```wirescript name=e4-let-on
@left in bell: exec
var rings: int = 0
@right out sign = "Rings: ${rings.Value}"
@right out chime: exec

let rang = on bell {
  rings += 1
}

on rang {
  emit chime
}
```

Without a body, `let ... = on` names the trigger itself. An `on` that uses
the name runs as if the trigger, with its settings, were written there:

```wirescript name=e4-alias
@left in room: zone
var visits: int = 0
@right out sign = "Visits: ${visits.Value}"

let entered = on ZoneEntered(zone = room)

on entered -> (character) {
  visits += 1
  character.ShowChatMessage("Welcome!")
}
```

## Modify

1. A handler can declare an output of its own. Replace `shown` and `sign`
   in the shop with these two handlers:

   ```wirescript name=e4-handler-out
   @left in staffed: bool

   on staffed {
     @right out sign: string = "Open"
   }

   on !staffed {
     out sign = "Closed"
   }
   ```

   Open the chip. The two writes share a hidden **Var** that drives the
   pin. It starts empty, so the sign is blank until the switch first moves.
2. Give the shop a back door: merge a second bell Button into the first
   with `Union`, and branch on `staffed` after the merge.
3. Make the chime flash a light too, without delaying the sign: fork a
   `Flash()` from `on door.A`.

## Make

Make a stage light. A Switch turns the show on and off. While the show is
on, a Button flashes a spotlight for one second and plays a sound at the
same moment, and a sign counts the flashes after each one ends. While the
show is off, pressing the Button shows "No show tonight" on the sign.

```wiring
switch Enabled -> chip show
button Held -> chip flash
chip spot -> light Enabled
chip sound -> speaker Play
chip sign -> decal Text
```

## Good to Know

- `on x` fires when `x` turns on and `on !x` when it turns off. Neither
  fires again while the switch stays where it is.
- A Branch decides by its **Cond** at the moment the pulse arrives. Changing
  the condition afterwards does not reroute a pulse already sent.
- Exec wires may split but not join. One output into many inputs is fine,
  but many outputs into one input is not. The compiler merges them with a Union
  when your code asks for it.

See [Forking the Exec Chain](../../exec-context.md#forking-the-exec-chain),
[Exec-flow gates](../../builtins.md#exec-flow-gates-union--branch),
[Negated Triggers](../../statements.md#negated-triggers),
[Union Triggers](../../statements.md#union-triggers),
[Handler-declared outputs](../../statements.md#handler-declared-outputs) and
[`let on`](../../statements.md#let-on-event-declaration) in the reference.

Next: pick a lesson from the tree on the [Learn](../README.md) page.
