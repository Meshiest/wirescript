# Wirescript

Language support for [Wirescript](https://wirescript.brickadia.dev), a language
that compiles to Brickadia's logic gates: write the circuit as code, compile it to
a `.brz`, and paste it into Brickadia.

## Features

- **Errors and warnings as you type**, including across imported files.
- **Completion** for variables, gates, events, record fields and enum variants.
- **Hover** with types, documentation, gate defaults and gate counts.
- **Go to definition**, **find references** and **rename**, across imports.
- **Inlay hints** for inferred types and parameter names.
- **Quick fixes**: fill a record's missing fields, fill a `match`'s missing
  arms, and silence a warning on one line or in the whole file.
- **Organize imports** (`Shift+Alt+O`): sorts imports and removes unused ones.
- **Formatting** (`Shift+Alt+F`), following the project's `.editorconfig`
  (indentation, line length, line endings). A `// fmt-ignore` comment keeps the
  next line as written.
- **Compile and copy** (`Ctrl+Shift+B`): compiles the current file to a `.brz`
  and puts it on the clipboard as a file, ready to paste into Brickadia. Build
  errors show up in the Problems panel.
- **The language docs, inside the editor**: the **Wirescript Docs** view in the
  Explorer, or **Wirescript: Open Documentation** to search them. Pages open in
  the markdown preview, with Wirescript examples colored like the editor.

## Getting started

1. Open a folder and create a file ending in `.ws`.
2. Write a circuit, for example:

   ```wirescript
   in go: exec
   var count: int = 0

   on go {
     count += 1
     BroadcastChatMessage("pressed ${count} times")
   }
   ```

3. Press `Ctrl+Shift+B`, switch to Brickadia and paste.

New to Wirescript? Start with **Wirescript: Open Documentation** and pick
**Learn**, or try the [online playground](https://wirescript.brickadia.dev).

## Settings

| Setting | Default | What it does |
|---|---|---|
| `wirescript.server` | `auto` | Which language server runs: `wasi` (the WebAssembly build that ships with the extension), `native` (a native `wirescript-lsp` binary), or `auto` (native when one is configured or built in a Wirescript checkout, otherwise WebAssembly). |
| `wirescript.lspPath` | | A native `wirescript-lsp` binary to run. |
| `wirescript.wasmPath` | | A `wirescript-lsp.wasm` to run instead of the bundled one. |

Changing any of them restarts the server. **Wirescript: Restart Language
Server** restarts it by hand.

## Requirements

The WebAssembly language server runs on Microsoft's
[WebAssembly Execution Engine](https://marketplace.visualstudio.com/items?itemName=ms-vscode.wasm-wasi-core)
extension, which is installed along with this one.

## Privacy

The extension collects no telemetry.

## Credits

The icon's lettering is set in Glacial Indifference by Hanken Design Co.,
licensed under the SIL Open Font License 1.1. The `.ws` file icon is the
"microchip" icon from [Font Awesome Free](https://fontawesome.com) by Fonticons,
Inc., licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

## License

[CC0 1.0](https://github.com/Meshiest/wirescript/blob/main/LICENSE): public
domain.
