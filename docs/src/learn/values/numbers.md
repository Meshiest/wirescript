# Numbers

<!-- learn-links -->
<div class="learn-links">

**Before this:** [Remembering Values](../state.md)

**Next steps:** [True and False](true-and-false.md), [Blending Values](../motion/blending-values.md)

</div>
<!-- /learn-links -->

Wirescript has two kinds of number. An `int` is a whole number. A `float` is a
number that can have a fraction, like `2.5`. Both are 64-bit, which means an
`int` can go up to 9,223,372,036,854,775,807, and a `float` can hold very large
and very small numbers. This lesson is about what happens when you divide them.

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
button1 Held -> chip addPizza
button2 Held -> chip addPerson
chip text -> decal Text
```

## Code

```wirescript name=v1-pizza
@left in addPizza: exec
@left in addPerson: exec
var pizzas: int = 0
var people: int = 0
let slices = pizzas.Value * 8
let each = slices / people.Value
let spare = slices % people.Value
@right out text = "${slices} slices, ${people.Value} people: ${each} each, ${spare} spare"

on addPizza {
  pizzas += 1
}

on addPerson {
  people += 1
}
```

Each pizza has 8 slices, shared between everyone. `*` multiplies. `/`
divides, and an `int` divided by an `int` gives an `int`: the remainder is
dropped, so `7 / 2` is 3. `%` gives the remainder that `/` dropped, so `7 % 2`
is 1.

With 2 pizzas and 3 people, how many slices does each person get, and how many
are spare? What does the sign say with 1 pizza and nobody there yet?

## Run

1. Compile the chip and paste it in-game. Place two **Button**s and a brick
   with a **Text Decal**.
2. Wire the Buttons' **Held** outputs to `addPizza` and `addPerson`, and
   `text` to the Text Decal's **Text**.
3. Add one pizza before anyone arrives. The sign reads "8 slices, 0 people: 0
   each, 0 spare". Dividing by zero gives 0, not an error.
4. Add 3 people and another pizza: "16 slices, 3 people: 5 each, 1 spare".

## Under the Hood

Open the chip. There is one Multiply, one Divide and one Modulo gate, all
pure, wired from the two Variable gates. Divide and Modulo both take `slices`
and `people`, so they always agree with each other.

A number written with a point, like `8.0`, is a `float`. Without one, like
`8`, it is an `int`. When one side of `+`, `-`, `*`, `/` or `%` is a `float`,
the result is a `float` and keeps its fraction: `7.0 / 2` is 3.5.

## Modify

1. Show the exact share in pizzas. `as float` turns a value into a `float`
   before the divide:

   ```wirescript name=v1-share
   @left in addPizza: exec
   @left in addPerson: exec
   var pizzas: int = 0
   var people: int = 0
   let share = pizzas.Value as float / people.Value
   @right out text = "Each person gets ${share} pizzas"

   on addPizza {
     pizzas += 1
   }

   on addPerson {
     people += 1
   }
   ```

   `as` applies before the `/`, so this is `(pizzas as float) / people`. With
   2 pizzas and 4 people, each person now gets half a pizza instead of 0.

2. Order enough pizza. Everyone wants 3 slices, and pizzas come whole. Show
   how many to buy: `(people.Value * 3 + 7) / 8`. Adding 7 before dividing by
   8 rounds up instead of down. Try it with 1, 3 and 8 people.
3. Take a square root of a number that goes negative:

   ```wirescript name=v1-sqrt
   @left in down: exec
   var level: float = 4.0
   @right out text = "sqrt(${level.Value}) = ${sqrt(level.Value)}"

   on down {
     level -= 2.0
   }
   ```

   The first press shows the root of 2. The second shows the root of 0. The
   third asks for the root of -2, which has no answer as a real number: the
   sign shows 0.

## Make

Make a stopwatch. While a **Switch** is on, a `Clock` adds one to an `int`
every tenth of a second (`on Clock(interval = 0.1, enabled = running)`). The
sign shows the time as seconds and tenths, like "12.3 s", using `/` and `%`
on the `int`. A **Button** sets it back to 0.

```wiring
switch Enabled -> chip running
button Held -> chip reset
chip text -> decal Text
```

## Good to Know

- Dividing by zero, or taking the remainder by zero, gives 0.
- A result that is not a real number, such as the square root of a negative
  number, or one too large for a `float`, is read as 0 by the gate it goes
  into.

Next: [True and False](true-and-false.md).
