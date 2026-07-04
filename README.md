## Wirescript

Compilers that build Brickadia wire graphs from code:

- **Wirescript** - a language that compiles to wire graph saves. Try it in the
  browser at [wirescript.brickadia.dev](https://wirescript.brickadia.dev/).
- (Old, outdated) **Bearilog** - a module builder CLI for generating logic bricks.

This project requires [rust](https://www.rust-lang.org/) v1.88 or later.

## Wirescript

The [playground](https://wirescript.brickadia.dev/) writes, formats, and
compiles Wirescript to `.brz` entirely in the browser, with searchable docs
(Ctrl-K) and a downloadable SDK (Node.js CLI tools, examples, and a VS Code
extension).

Every `var` becomes a Variable gate, every operator becomes an expression
gate, and every `on` handler becomes an exec chain:

```wirescript
var count: int = 0

in trigger: exec

on trigger {
  count = count + 1
}

out total = count.Value
out doubled = count.Value * 2
```

### Features

- Pure + exec contexts: signal-flow expressions and sequential `on` handlers
- `var` / `static var` / `let` / `buffer` (one-tick delay) bindings
- Records, tuples, destructuring, and spread
- `mod` macros with ref params and `chip` microchip grouping
- Imports across files: named, namespace (`import * as u`), and transitive
- `await`, `Sleep` / `SleepTicks`, local exec signals, and `emit`
- Receiver method syntax: `entity.SetLocation(...)`, `a.Dot(b)`
- Typed arrays with 20+ built-in methods
- Vector and quaternion math with component-wise arithmetic and scalar
  broadcast
- `$asset` syntax and near-full coverage of Brickadia's logic gates
- LSP + VS Code extension: diagnostics, hover, completions, go-to-definition,
  formatting, rename, organize imports, and inlay hints

The language reference lives in [docs/wirescript/](docs/wirescript/). The
compiler is `crates/wirescript`, the LSP server is `crates/lsp`, the browser
playground is `crates/wasm`, and the editor extension is `editors/vscode`.

## Bearilog CLI

### Output to BRDB file

This will build a module and output it to a BRDB file.

```sh
cargo run ./examples/cpu reg64_16 -o example.brdb
```

Render as a big blob of bricks

```sh
cargo run ./examples/cpu reg64_16 -o example.brdb --layout grid --inline
cargo run ./examples/7seg bitwise7seg -o 7segbit.brdb --layout grid --iobelow
```

### Display a module

This will build a module and print out its structure

```sh
cargo run ./examples/cpu reg64_16
```

### Display a module as a graphviz graph

Output for a browser:

```sh
cargo run ./examples/cpu reg64_16 -g
```

Render from CLI:

```sh
cargo run ./examples/cpu reg64_16 -g | dot -q -Tsvg > output.svg
```
