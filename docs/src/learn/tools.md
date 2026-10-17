# Tools

<!-- learn-links -->
<div class="learn-links">

**Next steps:** [First Circuit](first-circuit.md), [Wirescript for Programmers](for-programmers.md)

</div>
<!-- /learn-links -->

Write Wirescript in the playground or in VS Code, compile it to a `.brz`
save, and paste the save in-game.

<!-- toc -->
## Contents

- [Writing and compiling](#writing-and-compiling)
- [Pasting a circuit in-game](#pasting-a-circuit-in-game)
- [The starter library](#the-starter-library)
- [Code layout](#code-layout)
- [Reading an error](#reading-an-error)
<!-- /toc -->

## Writing and compiling

**The playground** runs in your browser with nothing to install. Write a
file, press **Compile** (or **Ctrl+Shift+B**), and it downloads a `.brz`
save. Errors show as red underlines as you type.

**VS Code** needs the
[Brickadia Wirescript extension](https://marketplace.visualstudio.com/items?itemName=brickadia-community.brickadia-wirescript):
install it from that page, or search for "Brickadia Wirescript" in VS Code's
Extensions view. Then open a `.ws` file. You get errors as you type,
completions, hover, go to definition, rename, formatting, and these docs
inside the editor. **Ctrl+Shift+B** compiles the open file and puts the
`.brz` on the clipboard, ready to paste into Brickadia.

**The command line** comes with the
[Wirescript SDK](../../wirescript-sdk.zip), also the **SDK** button in the
playground's toolbar. Its tools need Node.js 18 or later:
`node check.mjs door.ws` checks a file, and `node compile.mjs door.ws` writes
`door.brz`.

Every example in these lessons has a **Compile** button too, next to the copy
button. It downloads that example as a save, so you can try it without
writing anything.

**The docs editor** is the **Editor** tab at the bottom right of every docs
page. It opens the playground's editor in a drawer, with the same errors,
completions and hover, plus **Compile** and **Playground** buttons. The pencil
button on an example loads that example into it, so you can change it and
compile the result. It does not save your work: it holds one piece of text,
and loading another example replaces it. Write anything you want to keep in
the playground or VS Code.

## Pasting a circuit in-game

A compiled `.brz` is an ordinary Brickadia save holding one microchip brick.
Paste it in-game, either through Brickadia's load dialog by the file's path,
or straight from the clipboard after VS Code's **Ctrl+Shift+B**, and place it
where you want it.

The microchip is the circuit. Its pins are small bricks on its sides, each
labeled with its name. You wire them to the rest of your build with the wire
tool, like any other component.

## The starter library

Many examples begin with `import { escape } from "sanitize"`. That line
brings in helpers from `sanitize.ws`, a small library that comes with these
lessons. An example's **Compile** button and the docs editor fetch it for you.
To build those examples in the playground or VS Code, save
[sanitize.ws](lib/sanitize.ws) in the same folder as your own file. This is
the whole file:

```wirescript nocompile file=lib/sanitize.ws
// Escape markup in a display name before showing it. A `mod`, so each use is a single gate.
mod escape(text: string) -> string {
  return text.Replace("<", "&lt;")
}

// Clean free-form player input: trim, cap the length, then escape.
// Capping first keeps the cut from landing inside an `&lt;`.
fn sanitize(raw: string, maxLen: int = 24) -> string {
  return raw.Trim().Substring(0, maxLen).Replace("<", "&lt;")
}
```

`escape` makes a name safe to show, and `sanitize` also trims it and cuts it
to a length. [Safe Text](strings/safe-text.md) explains both, and
[Imports](abstraction/imports.md) explains `import`.

## Code layout

The compiler normally arranges a chip's gates to keep its wires short, with
no link to where the code sits in the file. To see each line of code as a row
of gates when you open the chip, put `@layout("code")` at the top of the file,
followed by a blank line:

```wirescript name=tools-layout
@layout("code")

@left in a: bool
@left in b: bool
let both = a && b
@right out lit = both
```

Earlier lines sit higher, and indentation shows in the gates. This helps while
you learn to read compiled chips. See
[`@layout("code")`](../statements.md#layoutcode-source-shaped-gate-layout)
for the details.

## Reading an error

When a file has a mistake, the compiler stops and says where. This pin uses a
name the language keeps for itself:

```wirescript name=reserved-pin expect=WSP001
@left in on: bool
```

Read the first line of the error:

```text
Error: [WSP001] expected Ident, got 'on' (Kw)
```

It names the problem (`on` is a keyword, `Kw`) and gives the line and column.
More errors usually follow, all caused by the first one. Fix the first and
compile again.

Some reserved words make tempting pin names: `in`, `out`, `on`, `from`,
`type`, `match`, `buffer`, `emit`, `fn`, `mod`, `const`, `static`, `await`
and `chip`. Use another word (`trigger`, `door`, `go`).

Every error code is listed in [Diagnostics](../diagnostics.md).

Next: [First Circuit](first-circuit.md).
