set windows-shell := ["pwsh", "-NoProfile", "-Command"]

# List available recipes
default:
    just --list

# Run all wirescript tests
test:
    cargo test -p wirescript --lib

# Run a specific test by name
test-one name:
    cargo test -p wirescript --lib -- {{name}}

# Build wirescript lib + check binary (debug)
build:
    cargo build -p wirescript

# Build everything release
release:
    cargo build --release -p wirescript -p wirescript-lsp -p wirescript-cli

# Build LSP server (release)
lsp:
    cargo build --release -p wirescript-lsp

# Build check CLI (release)
check-bin:
    cargo build --release -p wirescript --bin wirescript-check

# Regenerate the Contents list at the top of each docs/src page
doc-toc:
    node crates/wirescript/scripts/gen_doc_toc.mjs

# Type-check every ```wirescript example in docs/src (CI gate)
doc-check: check-bin
    node crates/wirescript/scripts/check_docs.mjs
    node crates/wirescript/scripts/gen_doc_toc.mjs --check
    node crates/wirescript/scripts/gen_book_summary.mjs --check
    node crates/wirescript/scripts/gen_hljs_lang.mjs --check

# Regenerate the book's table of contents from the playground's page list
doc-summary:
    node crates/wirescript/scripts/gen_book_summary.mjs

# Regenerate the book's highlight.js grammar from the playground's monarch.js
doc-hljs:
    node crates/wirescript/scripts/gen_hljs_lang.mjs

# the version .github/workflows/deploy-playground.yml builds the published book with
MDBOOK_VERSION := "0.5.2"

# Build the docs book into docs/book (`just mdbook serve` previews with live reload)
[windows]
mdbook *ARGS='build': doc-summary doc-hljs
    @if (-not (Get-Command mdbook -ErrorAction SilentlyContinue)) { \
      Write-Host "mdbook not installed. cargo install mdbook --version {{ MDBOOK_VERSION }}"; \
      Write-Host "or grab a binary from https://github.com/rust-lang/mdBook/releases"; \
      exit 1 }
    Copy-Item -Path CHANGELOG.md -Destination docs/src/ -Force
    mdbook {{ ARGS }} docs
    Remove-Item docs/src/CHANGELOG.md -Force -ErrorAction SilentlyContinue

# Build the docs book into docs/book (`just mdbook serve` previews with live reload)
[unix]
mdbook *ARGS='build': doc-summary doc-hljs
    @command -v mdbook >/dev/null || { \
      echo "mdbook not installed. cargo install mdbook --version {{ MDBOOK_VERSION }}" >&2; \
      echo "or grab a binary from https://github.com/rust-lang/mdBook/releases" >&2; \
      exit 1; }
    cp -f CHANGELOG.md docs/src/CHANGELOG.md
    mdbook {{ ARGS }} docs
    rm -f docs/src/CHANGELOG.md

# Build WASM module (for playground/SDK)
wasm:
    wasm-pack build crates/wasm --target nodejs --release --out-dir playground/sdk/pkg

# Check a .ws file for errors
check file:
    cargo run --release --bin wirescript-check -- {{file}}

# Check all .ws files in a directory
[windows]
check-dir dir:
    Get-ChildItem -Path {{dir}} -Filter *.ws | ForEach-Object { cargo run --release --bin wirescript-check -- $_.FullName }

# Check all .ws files in a directory
[unix]
check-dir dir:
    for f in {{dir}}/*.ws; do cargo run --release --bin wirescript-check -- "$f"; done

# Compile a .ws file to .brz
compile file:
    cargo run --release -p wirescript-cli -- compile {{file}}

# Compile a .ws file to .brdb (SQLite, for BR.World.LoadAdditive)
compile-brdb file:
    cargo run --release -p wirescript-cli -- compile {{file}} -o {{without_extension(file)}}.brdb

# Dump the lowered IR for a .ws file
ir file:
    cargo run --release -p wirescript-cli -- compile {{file}} --dump-ir

# Rebuild VS Code extension (compile TS + formatter)
[windows]
vscode:
    Set-Location editors/vscode; npm install; npm run build

# Rebuild VS Code extension (compile TS + formatter)
[unix]
vscode:
    cd editors/vscode && npm install && npm run build

# Run the VS Code extension's unit tests
[windows]
vscode-test:
    Set-Location editors/vscode; npm test

# Run the VS Code extension's unit tests
[unix]
vscode-test:
    cd editors/vscode && npm test

# Regenerate the tree-sitter parser from grammar.js and run its corpus
[windows]
treesitter:
    Set-Location editors/tree-sitter-wirescript; npm install; npx tree-sitter generate; npx tree-sitter test

# Regenerate the tree-sitter parser from grammar.js and run its corpus
[unix]
treesitter:
    cd editors/tree-sitter-wirescript && npm install && npx tree-sitter generate && npx tree-sitter test

# Copy wirescript docs (+ the CHANGELOG) into playground for serving
[windows]
playground-docs:
    Copy-Item -Path docs/src/*.md -Destination crates/wasm/playground/docs/ -Force
    Copy-Item -Path CHANGELOG.md -Destination crates/wasm/playground/docs/ -Force

# Copy wirescript docs (+ the CHANGELOG) into playground for serving
[unix]
playground-docs:
    cp -f docs/src/*.md crates/wasm/playground/docs/
    cp -f CHANGELOG.md crates/wasm/playground/docs/

# Build everything (lib + lsp + cli + wasm + vscode)
all: release wasm vscode
