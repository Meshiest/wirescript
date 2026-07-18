# Wirescript

Try it in the browser at [wirescript.brickadia.dev](https://wirescript.brickadia.dev/).

Looking for larger programs to read or remix? See
[Meshiest/wirescript-projects](https://github.com/Meshiest/wirescript-projects/) -
a place for larger projects/examples.

## Getting Started

You need [Rust](https://www.rust-lang.org/tools/install) v1.88 or later and
[just](https://github.com/casey/just) (a command runner):

```sh
# after installing Rust via rustup:
cargo install just
# or: winget install Casey.Just / brew install just / scoop install just
```

Then, from the repo root:

```sh
just                    # list all recipes
just compile file.ws    # compile a .ws file to .brz (load in-game via the load dialog)
just check file.ws      # type-check only, with error context
just test               # run the wirescript unit tests
```

The first build compiles the whole toolchain in release mode and takes a few
minutes; subsequent runs are fast.

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

## LSP + VS Code Extension

The extension in [editors/vscode/](editors/vscode/) runs the native LSP server
for diagnostics, hover, completions, go-to-definition, rename, organize
imports, and inlay hints, plus a Prettier-based formatter and a
**Compile and Copy Path** command (`Ctrl+Shift+B`) that compiles the current
file to a `.brz` and copies its path for Brickadia's load dialog.

Building it requires [Node.js](https://nodejs.org/):

```sh
just lsp        # build the native LSP server (target/release/wirescript-lsp)
just vscode     # npm install + compile the extension (editors/vscode/out)
```

### Installing the extension

In VS Code, open the Extensions view, click the `...` menu at the top of the
panel, choose **Install from Location...**, and pick `editors/vscode`. This
loads the extension from that folder in place, so it keeps working as you
rebuild.

Alternatively, symlink it into your VS Code extensions folder and reload
VS Code:

```sh
# macOS / Linux
ln -s "$(pwd)/editors/vscode" ~/.vscode/extensions/wirescript
```

```powershell
# Windows (PowerShell)
New-Item -ItemType Junction -Path "$env:USERPROFILE\.vscode\extensions\wirescript" -Target "$PWD\editors\vscode"
```

The extension locates the LSP binary at `target/release/wirescript-lsp`
**relative to the repo root** by resolving its own real path. If you copy the
folder somewhere else instead, point the extension at the binary with
the `wirescript.lspPath` setting.

After changing compiler or LSP code, rerun `just lsp` - the extension watches
the binary and restarts the server automatically (on Windows it runs a temp
copy so cargo can rebuild while the server is running).

## WASM

`crates/wasm` compiles the checker, compiler, and formatter to WebAssembly.
It powers both the browser playground and the downloadable SDK, and building
it requires [wasm-pack](https://rustwasm.github.io/wasm-pack/):

```sh
cargo install wasm-pack
```

- `just wasm` builds the Node.js-target module into
  `crates/wasm/playground/sdk/pkg`, used by the SDK's CLI tools
  (`check.mjs`, `compile.mjs`, `format.mjs`, `hover.mjs`).
- `crates/wasm/build-playground.sh` (bash; needs Node.js and `zip`) builds the
  web-target module and assembles the full playground into `_site/` - static
  files servable from any HTTP server - along with `wirescript-sdk.zip`, the
  downloadable SDK bundle (CLI tools, examples, and a WASM-based VS Code
  extension that needs no Rust toolchain).

To test the playground locally: `cd crates/wasm/_site && python3 -m http.server 8080`
(or some other static http server)

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
