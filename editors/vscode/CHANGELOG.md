# Changelog

## 0.1.3

- Wirescript 2.1.0: `any` vars, arrays and maps, `v.getType()` with the
  `Type` enum and narrowing, and builtin hovers say what each gate does.
- Fixed: the language server crashed as you typed into an empty file, and
  the built-in server stayed stopped until the window was reloaded. It now
  restarts after a crash.
- Fixed: errors from the last Compile stayed in the Problems panel after the
  language server restarted, even when the new server no longer reports them.

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
