# Bits

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Choosing a Value](choosing-a-value.md)

**Next steps:** [Code Door](code-door.md)

</div>
<!-- /learn-links -->

An `int` is a whole number. Inside the computer, it is stored as 64 on/off
values called bits. Think of it as a row of 64 switches. The bitwise
operators work on all of those switches at once, so one wire can carry the
state of many switches, and one gate can test them all.

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
switch1 Enabled -> chip s0
switch2 Enabled -> chip s1
switch3 Enabled -> chip s2
switch4 Enabled -> chip s3
chip text -> decal Text
```

## Code

A number written in binary shows its bits. `0b` starts a binary number, and
each digit is one bit, with the lowest on the right. Each bit is worth twice
the one to its right: 1, 2, 4, 8, and so on.

| Binary | Bits that are on | Value |
|---|---|---|
| `0b0001` | 1 | 1 |
| `0b0100` | 4 | 4 |
| `0b1011` | 8, 2, 1 | 11 |
| `0b1111` | 8, 4, 2, 1 | 15 |

This chip packs four switch pins into one number, one bit per switch:

```wirescript name=l4-pack
@left in s0: bool
@left in s1: bool
@left in s2: bool
@left in s3: bool
let code = s0 | (s1 << 1) | (s2 << 2) | (s3 << 3)
@right out text = "code ${code}, ${BitCount(code)} on"
```

- A `bool` counts as `1` or `0` where a number is expected, so `s1` is the
  number 1 while its switch is on.
- `<<` shifts left: `x << 2` moves every bit of `x` two places up. `1 << 2`
  is `0b0100`, which is 4. So `s2 << 2` is 4 while `s2` is on, and 0 while it
  is off.
- `|` is bitwise OR: each bit of the result is on if that bit is on in either
  side. Here each switch has its own bit, so OR puts them together.
- `BitCount(x)` counts how many bits of `x` are on.

Switches `s0` and `s3` are on, and the others off. What does the sign say?

## Run

1. Compile the chip and paste it in-game. Place four **Switch**es in a row and
   a brick with a **Text Decal**.
2. Wire each Switch's **Enabled** to `s0`, `s1`, `s2` and `s3`, and `text` to
   the Text Decal's **Text**.
3. Flip the switches. With `s0` and `s3` on, the sign reads
   `code 9, 2 on`: 1 + 8 is 9. All four on is 15, the largest a four-bit
   number can be.

## Under the Hood

Open the chip. There are three **Shift Left (Bitwise)** gates, one for each
of `s1`, `s2` and `s3` (`s0` is already in place), three **OR (Bitwise)**
gates joining them, and one **Bit Count**. The switch pins wire straight into
the shift gates: the game reads an on bool as 1.

Here are all the bitwise operators, on four-bit examples. Each works bit by
bit:

| Operator | Name | Example | Result |
|---|---|---|---|
| `&` | AND | `0b1100 & 0b1010` | `0b1000` |
| `\|` | OR | `0b1100 \| 0b1010` | `0b1110` |
| `^` | XOR | `0b1100 ^ 0b1010` | `0b0110` |
| `~` | NOT | `~0b1100` | every bit flipped, all 64 of them |
| `<<` | shift left | `0b0011 << 2` | `0b1100` |
| `>>` | shift right | `0b1100 >> 2` | `0b0011` |

These are different from the logic operators `&&`, `||` and `!`, which treat
the whole value as a single on or off. One `&` does 64 ANDs, one per bit.

A mask is a number that picks out bits. `code & 0b0100` keeps the bit worth
4 and clears every other, so it is 4 when `s2` is on and 0 when it is off. Some
common uses:

```wirescript name=l4-masks
@left in code: int
@right out s2On = (code & 0b0100) != 0
@right out third = (code >> 2) & 1
@right out withS0 = code | 0b0001
@right out withoutS0 = code & ~0b0001
@right out flipped = code ^ 0b1111
```

- `s2On` tests one bit: is it on?
- `third` reads the same bit as a number, 0 or 1: shift it down to the
  bottom, then mask it.
- `withS0` turns the bit worth 1 on, and `withoutS0` turns it off. `~0b0001` is every
  bit except the lowest.
- `flipped` flips the low four bits. `~code` would flip all 64, so to flip
  only some of them, XOR with a mask, or write `~code & 0b1111`.

Mind the parentheses in `s2On`. `&` binds more loosely than `==` and `!=`,
so without them `code & 0b0100 == 4` would mean `code & (0b0100 == 4)`: the
comparison first, then `&` with its true or false, which tests the wrong
switch. That is almost never what you want, so the compiler warns about it
(`WS117`) and asks for parentheses:

```wirescript name=l4-precedence expect=WS117
@left in code: int
@right out bad = code & 0b0100 == 4
```

`(code & 0b0100) == 4` tests the bit, and `code & (0b0100 == 4)` says you
really meant the comparison's result.

The compiler also joins a `~` around `&` or `|` into one gate: `~(a & b)`
becomes a single **NAND (Bitwise)** gate, and `~(a | b)` a single **NOR
(Bitwise)**, as `!` does for the logic operators.

## Modify

1. Add a light for each switch that reads its bit back out of `code`:
   `@right out lamp3 = (code & 0b1000) != 0`. Wire it to a **Point Light**'s
   **Enabled** and check it follows `s3`.

2. Show the bits in a row. Add an output for a second Text Decal that writes
   each bit as `1` or `0`, highest first:

   ```wirescript name=l4-bit-row
   @left in s0: bool
   @left in s1: bool
   @left in s2: bool
   @left in s3: bool
   let code = s0 | (s1 << 1) | (s2 << 2) | (s3 << 3)
   @right out row = "${(code >> 3) & 1}${(code >> 2) & 1}${(code >> 1) & 1}${code & 1}"
   ```

3. Count in binary. Replace the four switches with one **Button**, the
   counter chip from [Choosing a Value](choosing-a-value.md#run) (Button
   **Held** to its `pressed`), and an `@left in presses: int` pin wired from
   the counter's `presses`. Give four **Point Light**s one bit each, from
   `(presses & 1) != 0` up to `(presses & 0b1000) != 0`. Press the button and
   watch the lights count to 15 and start over.

## Make

Make a chip that takes two sets of four switches, packs each into a number,
and shows on a sign how many switches are set the same in both sets. XOR is
on wherever two bits differ, so `BitCount(a ^ b)` counts the differences.

```wiring
switch1 Enabled -> chip a0
switch2 Enabled -> chip a1
switch3 Enabled -> chip a2
switch4 Enabled -> chip a3
switch5 Enabled -> chip b0
switch6 Enabled -> chip b1
switch7 Enabled -> chip b2
switch8 Enabled -> chip b3
chip text -> decal Text
```

## Good to Know

- An `int` is 64 bits, and comparing two ints compares all 64 of them.
- A bool wired into a number input reads as exactly 1 or 0.
- `>>` compiles to the game's **Shift Right (Arithmetic)** gate. On the
  positive numbers in this lesson it moves bits down.

Next: [Code Door](code-door.md).
