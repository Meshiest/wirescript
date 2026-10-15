# Truth Tables

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Gates and Wires](../gates-and-wires.md)

**Next steps:** [Choosing a Value](choosing-a-value.md)

</div>
<!-- /learn-links -->

A truth table lists every way a circuit's inputs can be set, and what its
output is for each one. It is the plan for a logic circuit: fill in the table
first, then write the expression that matches it.

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
switch1 Enabled -> chip a
switch2 Enabled -> chip b
switch3 Enabled -> chip c
chip lit -> light Enabled
```

## Code

Here are the four logic operators from [Gates and Wires](../gates-and-wires.md)
as one table. `1` means on (true) and `0` means off (false).

| `a` | `b` | `a && b` | `a \|\| b` | `a ^^ b` | `!a` |
|---|---|---|---|---|---|
| 0 | 0 | 0 | 0 | 0 | 1 |
| 0 | 1 | 0 | 1 | 1 | 1 |
| 1 | 0 | 0 | 1 | 1 | 0 |
| 1 | 1 | 1 | 1 | 0 | 0 |

- `&&` (AND) is on only in the last row, where both inputs are on.
- `||` (OR) is off only in the first row, where both are off.
- `^^` (XOR, "exclusive or") is on when the inputs differ.
- `!` (NOT) has one input and gives the opposite.

Two inputs make four rows. Each new input doubles the rows, so three inputs
make eight. Here is a vote with three switches: the light turns on when at
least two of them are on.

| `a` | `b` | `c` | `lit` |
|---|---|---|---|
| 0 | 0 | 0 | 0 |
| 0 | 0 | 1 | 0 |
| 0 | 1 | 0 | 0 |
| 0 | 1 | 1 | 1 |
| 1 | 0 | 0 | 0 |
| 1 | 0 | 1 | 1 |
| 1 | 1 | 0 | 1 |
| 1 | 1 | 1 | 1 |

To turn the table into code, read it another way: the light is on when `a`
and `b` are both on, or `a` and `c` are, or `b` and `c` are. Each pair is an
`&&`, and the pairs join with `||`:

```wirescript name=l1-majority
@left in a: bool
@left in b: bool
@left in c: bool
@right out lit = (a && b) || (a && c) || (b && c)
```

The parentheses make the grouping easy to read. `&&` binds tighter than `||`
already, so the circuit is the same without them.

Check the code against the table: in the row `1 1 1`, every pair is on. Is the
light on? In how many of the eight rows is it on?

## Run

1. Compile the chip and paste it in-game. Place three **Switch**es and a
   **Point Light**.
2. Wire each Switch's **Enabled** to `a`, `b` and `c`.
3. Wire `lit` to the Point Light's **Enabled**.
4. Walk through the table: try all eight positions. The light is on in four
   of them, the four rows with two or three switches on.

## Under the Hood

Open the chip. There are five gates: three ANDs, one for each pair, and two
ORs that join them. Each `&&` and `||` in the source is one gate.

Two expressions that give the same truth table are the same circuit, as far
as anyone flipping the switches can tell. De Morgan's laws are the two most
useful pairs:

- `!(a && b)` is the same as `!a || !b`. "Not both" means "at least one is
  off".
- `!(a || b)` is the same as `!a && !b`. "Neither" means "both are off".

This chip computes both sides of the first law. Fill in the truth table for
`x` and `y`, then flip the switches to check that two lights wired to them
always agree:

```wirescript name=l1-demorgan
@left in a: bool
@left in b: bool
@right out x = !(a && b)
@right out y = !a || !b
```

Open it. `x` is a single **NAND** gate ("not and"): the compiler joins a `!`
written around an `&&` into one gate. `y` is three gates, two NOTs and an OR.
The same happens for `!(a || b)`, which becomes one **NOR** gate. When you
have a choice, write the form with the `!` outside.

## Modify

1. Count the votes instead. A `bool` counts as `1` when on and `0` when off
   wherever a number is expected, so you can add switches:

   ```wirescript name=l1-count-votes
   @left in a: bool
   @left in b: bool
   @left in c: bool
   @right out lit = a + b + c >= 2
   ```

   Its truth table is the same as the first chip's. Open it: two Add gates
   and a **Greater/Equal** gate.

2. Change the rule to "every switch is on", then to "exactly one switch is
   on". Write the truth table first, then the expression.

3. Chain XOR: `@right out lit = a ^^ b ^^ c`. Fill in the eight rows. The
   light is on when an odd number of switches are on: one or three. This is a
   parity check, and it gives you a light that any of three switches can
   change, like the hall light with a third switch.

## Make

Make a vote for four switches where the light turns on when at least three
are on. Write out the sixteen-row table, then write it two ways: with `&&` and
`||` only, and by counting. Check both against the table in-game.

```wiring
switch1 Enabled -> chip a
switch2 Enabled -> chip b
switch3 Enabled -> chip c
switch4 Enabled -> chip d
chip logic -> light1 Enabled
chip counted -> light2 Enabled
```

## Good to Know

- `a && b` always computes both sides, even when `a` alone already
  decides: wires do not skip anything. In a handler, an action called on
  either side still runs, so to do it only when `a` is on, put an `if`
  inside an `if`.
- An input pin with no wire reads as off. A three-switch chip with only two
  switches wired treats the third as `0`.
- A Switch's **Enabled** stays on until it is flipped back, so a truth table
  is easy to walk through with switches. Button **Held** is on only while it
  is held.

Next: [Choosing a Value](choosing-a-value.md).
