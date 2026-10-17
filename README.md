# Wirescript

Try it in the browser at [wirescript.brickadia.dev](https://wirescript.brickadia.dev/),
and read the language reference at
[wirescript.brickadia.dev/docs](https://wirescript.brickadia.dev/docs/).

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
- `fn` functions compiled once into a function plane and run by a Function Call
  gate at each call site; `mod` and `fn` parameters take default values
- `GetMicrochipBrick()`: the program's own microchip brick, as a `brick`
- Imports across files: named, namespace (`import * as u`), and transitive
- `await`, `Sleep` / `SleepTicks`, local exec signals, and `emit`
- Receiver method syntax: `entity.SetLocation(...)`, `a.Dot(b)`
- Typed arrays with 20+ built-in methods
- Vector and quaternion math with component-wise arithmetic and scalar
  broadcast
- `$asset` syntax and near-full coverage of Brickadia's logic gates
- LSP + VS Code extension: diagnostics, hover, completions, go-to-definition,
  formatting, rename, organize imports, fill-record-fields, and inlay hints

The language reference is published at
[wirescript.brickadia.dev/docs](https://wirescript.brickadia.dev/docs/) and its
source lives in [docs/src/](docs/src/) (`just mdbook` renders it locally with
[mdBook](https://rust-lang.github.io/mdBook/), `just mdbook serve` previews it
with live reload). The compiler is
`crates/wirescript`, the LSP server is `crates/lsp`, the browser playground is
`crates/wasm`, and the editor extension is `editors/vscode`.

## LSP + VS Code Extension

The extension in [editors/vscode/](editors/vscode/) runs the LSP server (native,
or compiled to WASI; see below) for diagnostics, hover, completions, go-to-definition, rename, organize
imports, and inlay hints, plus a Prettier-based formatter and a
**Compile and Copy Path** command (`Ctrl+Shift+B`) that compiles the current
file to a `.brz` and copies its path for Brickadia's load dialog.

A **Fill record fields** code action (the lightbulb / `Ctrl+.`) is offered
when the cursor is inside a record literal whose expected type is a record —
`let x: Card = { … }`. It inserts every *missing* field (so partial literals
complete too) with a type-appropriate default, recursing into nested records:
`{ foo: "", bar: { baz: 0 } }`.

Building it requires [Node.js](https://nodejs.org/):

```sh
just lsp        # build the native LSP server (target/release/wirescript-lsp)
just lsp-wasi   # build the WASI LSP server (target/wasm32-wasip1-threads/release/wirescript-lsp.wasm)
just vscode     # npm install + build the extension (editors/vscode/dist)
just vscode-package  # package editors/vscode/brickadia-wirescript.vsix, with the WASI server inside
just vscode-try      # package, then open it in a separate VS Code profile
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

The `wirescript.server` setting picks the server:

- `native` runs `wirescript.lspPath`, else `target/release/wirescript-lsp`
  **relative to the repo root** (the extension resolves its own real path).
  Build it with `just lsp`.
- `wasi` runs `wirescript.wasmPath`, else
  `target/wasm32-wasip1-threads/release/wirescript-lsp.wasm`, else a bundled
  `server/wirescript-lsp.wasm`, on the WebAssembly Execution Engine extension
  (`ms-vscode.wasm-wasi-core`). Build it with `just lsp-wasi`.
- `auto` (the default) is native when one is configured or built, WASI
  otherwise.

After changing compiler or LSP code, rerun `just lsp` (or `just lsp-wasi`):
the extension watches the server it runs and restarts it automatically (on
Windows the native server runs from a temp copy so cargo can rebuild it).
**Wirescript: Restart Language Server** restarts it by hand.

## Formatting

`wirescript fmt <files or directories>` formats `.ws` files in place
(`just fmt <paths>`); `--check` (`just fmt-check <paths>`) lists the files
that are not formatted and fails if there are any. The same formatter runs in
the playground and in the LSP server.

It re-indents by bracket depth, normalizes comma spacing, joins `}` and
`else {`, and splits lines wider than the print width (100 columns): an
import list across lines, a binary expression before its loosest operators. It
only moves whitespace and line breaks. `// fmt-ignore` on its own line keeps
the next line as written; at the end of a line, that line. Block comments and
nested-prefab blocks are kept as written.

Settings come from `.editorconfig`:

```ini
[*.ws]
indent_style = space      # or tab
indent_size = 2
max_line_length = 100     # print width; "off" never splits
end_of_line = lf          # or crlf; unset keeps the file's own
insert_final_newline = true
```

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
