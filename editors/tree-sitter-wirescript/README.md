# tree-sitter-wirescript

A [tree-sitter](https://tree-sitter.github.io/) grammar for **Wirescript**, the
language that compiles to Brickadia wire graphs.

The grammar is derived from the authoritative Rust implementation in this repo —
`crates/wirescript/src/lexer.rs` and `crates/wirescript/src/parser.rs` — not from
the TextMate grammar or the prose docs. Where the docs and the parser disagree,
the parser wins (see [Notes on the reference parser](#notes-on-the-reference-parser)).

## Layout

```
grammar.js              the grammar
queries/highlights.scm  syntax highlighting
queries/locals.scm      scopes, definitions and references
queries/indents.scm     indentation
test/corpus/*.txt       tree-sitter test corpus
```

## Building

**`src/` is generated and gitignored — you must generate it before anything
works.** A fresh checkout has no parser; `tree-sitter test`, `tree-sitter parse`,
and every editor integration load `src/parser.c`, never `grammar.js` directly.
So an edit to `grammar.js` changes nothing until you regenerate.

From the repo root:

```bash
just treesitter             # npm install + tree-sitter generate + tree-sitter test
```

Or by hand, from this directory:

```bash
npm install                 # installs tree-sitter-cli
npx tree-sitter generate    # writes src/parser.c (~700KB)
npx tree-sitter test        # runs test/corpus
```

Parse a file, or a whole tree of files, to check for `ERROR` nodes:

```bash
npx tree-sitter parse path/to/file.ws
npx tree-sitter parse -q --stat $(find ../../../wirescript/projects -name '*.ws')
```

## Installing

### Neovim (nvim-treesitter)

```lua
require('nvim-treesitter.parsers').get_parser_configs().wirescript = {
  install_info = {
    url = '/path/to/bearilog/editors/tree-sitter-wirescript',
    files = { 'src/parser.c' },
    branch = 'master',
  },
  filetype = 'wirescript',
}

vim.filetype.add({ extension = { ws = 'wirescript' } })
```

Then `:TSInstall wirescript`, and copy `queries/` to
`~/.config/nvim/queries/wirescript/`.

### Helix

Add to `languages.toml`:

```toml
[[language]]
name = "wirescript"
scope = "source.wirescript"
file-types = ["ws"]
comment-token = "//"
indent = { tab-width = 2, unit = "  " }

[[grammar]]
name = "wirescript"
source = { path = "/path/to/bearilog/editors/tree-sitter-wirescript" }
```

Then `hx --grammar build` and copy `queries/` to
`~/.config/helix/runtime/queries/wirescript/`.

## Verification status

- `tree-sitter test` — **75/75 corpus tests pass**.
- `tree-sitter parse` over every `.ws` file in `wirescript/projects/` and
  `bearilog/examples/` — **162/162 parse with zero `ERROR` nodes**.
- Operator precedence, associativity and the postfix/prefix ordering were
  checked against `infix_prec()` by inspecting parse trees, not just by
  confirming the absence of errors.

## Notes on the reference parser

Things worth knowing, all verified against `parser.rs`:

- **`match` is reserved but unimplemented.** It is in the lexer's `KEYWORDS`
  list and `Expr::MatchExpr` exists in the AST, but no production ever
  constructs it. `expressions.md` documents it aspirationally. This grammar has
  no `match` rule, so `match` lexes as a plain identifier here.
- **`fn`, `import` and `type` are top-level only.** `parse_stmt` does not handle
  them; `parse_top_decl` does. The grammar enforces this.
- **`namespace` is not source syntax** — the resolver synthesises it from
  `import * as ns`.
- **`let x = on Trigger` uses a restricted trigger grammar.** `parse_let_decl`
  calls `parse_trigger` directly, so the trigger can only be
  `ident`, `ident.field`, `!atom`, `(…)` and `|` unions — never a call. A
  handler's trigger, by contrast, falls back to a full expression via
  `looks_like_expr_trigger`, which is why `on if c then a else b { … }` and
  `on ChatCommand("x", Description = "y") { … }` both parse.
- **`out name(args)` is two statements.** `parse_out_binding` reads only the
  name, so `out aw(wa)` is an `out` declaration followed by a separate
  expression statement. This grammar reproduces that shape. Note that the
  compiler is in the process of adding a *diagnostic* for this form (it still
  parses the same way, it just also reports an error), so the tree shape here
  stays correct either way — the grammar deliberately does not reject it.

## Deviations from the reference parser

These are deliberate, and each one is a place where this grammar is *more
permissive* than `parser.rs` — it will never reject something the compiler
accepts, but it may accept a few things the compiler rejects.

1. **Newlines are whitespace.** The reference lexer emits newline tokens, but
   `parse_binary` skips them for operator continuation, list parsers skip them,
   and `eat_stmt_end` treats them as optional. Statements there are
   self-delimiting — `var x = 1 var y = 2` on one line parses fine in both. The
   one place newlines genuinely matter is a bare `return`: the reference stops
   at a newline, whereas this grammar is greedy, so a value on the *following*
   line is absorbed into the `return`. `return` followed by `}` — by far the
   common case — is unaffected.
2. **Nested block comments.** The reference lexer nests `/* /* … */ */`; a
   regex-based token cannot. Non-nested block comments are handled correctly.
   Fixing this would require an external scanner.
3. **Negative literals are not folded.** The reference folds `-42` into a single
   negative `IntLit` at parse time. Here it is a `unary_expression` over an
   `integer`, which is the honest syntactic shape.
4. **`match` lexes as an identifier** (see above), rather than as a reserved
   word that triggers an error.
5. **Annotation placement is not validated.** The reference rejects e.g.
   `@closed` on an `in` declaration or any annotation on `mod`. Those are
   semantic checks; this grammar parses them and leaves the diagnosis to the
   compiler.
6. **`-> (int, bool)` parses as a tuple type.** The reference always reads a `(`
   after `->` as an output list and then errors. Only the empty `-> ()` case is
   pinned to the output-list reading here.
7. **Duration units.** `buffer(1 s)` accepts the `s` marker as a distinct token;
   the reference checks for an identifier with the text `s`. Equivalent in
   practice.

## Query precedence

`queries/highlights.scm` is ordered **general first, specific last**: when
several patterns capture the same node, the last one wins. This was verified
empirically with `tree-sitter highlight`, and matches nvim-treesitter's
behaviour. If you add patterns, put narrow context-sensitive ones at the
bottom — a rule added below `(identifier) @variable` will override it, and one
added above it will not.
