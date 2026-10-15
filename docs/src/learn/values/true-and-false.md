# True and False

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Numbers](numbers.md)

**Next steps:** [Vectors and Colors](vectors-and-colors.md), [Object References](../references/object-references.md)

</div>
<!-- /learn-links -->

A `bool` is the simplest value: `true` or `false`. Switches and buttons give
bools, lights take them, and every comparison makes one. This lesson also
shows what happens when a bool meets a number.

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
chip text -> decal Text
chip passed -> light Enabled
```

## Code

```wirescript name=v2-vote
@left in a: bool
@left in b: bool
@left in c: bool
let votes = a + b + c
@right out text = "${votes} of 3 say yes"
@right out passed = votes >= 2
```

Each of three judges has a switch. When a bool is used in arithmetic, `true`
counts as 1 and `false` as 0, so `a + b + c` is the number of switches that
are on.

`votes >= 2` compares two numbers and gives a bool: `true` when there are at
least two yes votes. The other comparisons work the same way: `==` (equal),
`!=` (not equal), `<`, `>`, `<=` and `>=`.

With two switches on, what does the sign say, and is the light on?

## Run

1. Compile the chip and paste it in-game. Place three **Switch**es, a brick
   with a **Text Decal**, and a **Point Light**.
2. Wire each Switch's **Enabled** to `a`, `b` and `c`. Wire `text` to the Text
   Decal's **Text** and `passed` to the Point Light's **Enabled**.
3. Flip the switches. The sign counts the yes votes, and the light is on with
   two or three of them.

## Under the Hood

Open the chip. Two Add gates sum the three pins, and a **Greater/Equal** gate
compares the total with 2. No gate turns the bools into numbers first: the
Add gate takes them as they are.

A comparison is a gate like any other, and its bool output can go anywhere a
bool can: a light, an `if`, a variable, or another operator such as `&&`.

## Modify

1. Show each vote on the sign: `"${a} ${b} ${c}: ${votes} of 3"`. A bool in
   text shows as `true` or `false`.
2. Give judge `a` two votes: `let votes = a * 2 + b + c`. Now `a` and one
   other judge are enough, but `b` and `c` alone are not.
3. Combine comparisons on a stored number. This chip counts presses and its
   light is on only for even counts above 5:

   ```wirescript name=v2-even
   @left in pressed: exec
   var count: int = 0
   let isEven = count.Value % 2 == 0
   let big = count.Value > 5
   @right out text = "${count.Value}: even ${isEven}, big ${big}"
   @right out lamp = isEven && big

   on pressed {
     count += 1
   }
   ```

   `count.Value % 2 == 0` reads as "the remainder after dividing by 2 is 0".
   `%` comes before `==`, as `*` comes before `+`.

## Make

Make a "two-key" vault door: two switches far apart must both be on, and the
door opens only if the number of presses on a third button is exactly 3. Wire
the result to a **Brick Property Changer**'s **Collision Enabled** and
**Visibility** so the door vanishes when it opens.

```wiring
switch1 Enabled -> chip key1
switch2 Enabled -> chip key2
button Held -> chip pressed
chip solid -> changer "Collision Enabled"
chip solid -> changer Visibility
```

## Good to Know

- A number used where a bool is wanted is `true` when it is not zero, so `-1`
  as a condition is true.
- But comparing a bool with a number compares it as exactly 1 or 0: `1 ==
  true` is true, and `-1 == true` is false.
- `a == b` on two bools is true when they match, both on or both off: the
  opposite of `^^`.

Next: [Vectors and Colors](vectors-and-colors.md).
