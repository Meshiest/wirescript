# Changelog

## 0.1.9

- Wirescript 2.4.1: `character.GetScale()` and `character.SetScale(scale)` read
  and set a player's size, and Sweep's removed `bodyPartsOnly` setting is now
  an error.

## 0.1.8

- A Run test lens above each `@test` runs it in the gate simulator and shows whether it passed; what it printed goes to the Wirescript output channel.
- `.wsir` files get their own icon, and Compile and Copy (`Ctrl+Shift+B`)
  assembles them into a `.brz` the same way it compiles a `.ws`.

## 0.1.7

- Wirescript 2.3.1: compiler fixes (enum discriminants in hex, binary and octal,
  float division in annotated `let`s, data from an event used as a value, array
  and map vars initialized from a call, spreads into builtins).
- Fixed: hovering a `static var` read now says whether it reads in exec or pure
  context, and completion picks the right declaration of a shadowed name after
  non-ASCII text on the same line.

## 0.1.6

- Wirescript 2.3.0: `continue` in loops, highlighted as a keyword, with
  hovers on `break` and `continue`.

## 0.1.5

- Fixed: hovering a name a pattern binds (`where` in `if let "tp ${where}"`,
  `x` in `Some(x)`) showed nothing, and go to definition on one went nowhere.
  Both now find it, as do `let`s inside `if let`, `let ... else` and `match`
  blocks.

## 0.1.4

- Wirescript 2.2.0: `match` on strings, numbers and bools, with literal,
  range and string patterns (`"say ${rest}"`), `|` alternatives and `if`
  guards.
- Fixed: the minimap no longer labels every line that opens a `{` block.

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
