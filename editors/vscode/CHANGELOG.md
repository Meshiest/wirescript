# Changelog

## 0.1.2

- Wirescript 2.0.4: `on Loaded()` and `entity.SetVisibleTo(visibleTo?)`.

## 0.1.1

- Wirescript 2.0.3: `arr.join(separator?)`, and `BrickChanged` / `BrickRemoved`
  bind `brick` and `character`.

## 0.1.0

First release.

- Errors and warnings as you type, completion, hover, go to definition, find
  references, rename, inlay hints and quick fixes.
- Formatting and organize imports.
- Compile and copy (`Ctrl+Shift+B`): compiles the open file to a `.brz` and
  puts it on the clipboard, ready to paste into Brickadia.
- The language docs inside the editor, with links to them from hovers and
  diagnostics.
- The language server runs as WebAssembly, so the extension works the same on
  every platform.
