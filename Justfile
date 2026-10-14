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

# Time every compile stage on generated programs at 4k/8k/16k and fail a stage
# that grows faster than linearly (--update writes the baseline)
scale-bench *ARGS:
    node crates/wirescript/scripts/scale_bench.mjs {{ARGS}}

# How the editor's per-request analysis grows with file size (2k / 4k / 8k
# lines); fails a request whose 4k -> 8k ratio is above 2.6.
editor-scale *ARGS:
    cargo run --release -p wirescript --example editor_scale -- {{ARGS}}

# Run the repository scripts' and the playground's JS tests
js-test:
    node --test "crates/wirescript/scripts/test/*.test.mjs" "crates/wasm/playground/test/*.test.mjs"

# Regenerate the Contents list at the top of each docs/src page
doc-toc:
    node crates/wirescript/scripts/gen_doc_toc.mjs

# Type-check every ```wirescript example in docs/src (CI gate)
doc-check: check-bin
    node crates/wirescript/scripts/check_docs.mjs
    node crates/wirescript/scripts/gen_doc_toc.mjs --check
    node crates/wirescript/scripts/gen_book_summary.mjs --check
    node crates/wirescript/scripts/gen_hljs_lang.mjs --check
    cargo run -q -p wirescript --example gen_environment_doc -- --check

# Regenerate the reference half of docs/src/environment.md from the game's
# property map (after a game build's property map is refreshed)
doc-environment:
    cargo run -q -p wirescript --example gen_environment_doc

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

# Fuzz the compiler: generate random programs and report anything that
# crashes, emits an `_Unsupported` placeholder, or fails to compile.
# `just fuzz 20000 7` runs 20k programs from seed 7.
fuzz count='5000' seed='1':
    cargo run --release --example fuzz_programs -p wirescript -- --count {{count}} --seed {{seed}}

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

# Print a .ws file's compiled program as .wsir text (stdout)
ir file:
    cargo run --release -p wirescript-cli -- compile {{file}} --dump-ir

# Build a .brz (or .brdb, by -o's extension) from a .wsir program
assemble file *ARGS:
    cargo run --release -p wirescript-cli -- assemble {{file}} {{ARGS}}

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

# Build the wasm crate for wasm32 without wasm-pack (a build gate; writes nothing outside target/)
wasm-check:
    cargo build -p wasm --target wasm32-unknown-unknown --release

# Compile the frozen oracle set (target/ir-oracle/src) and byte-compare every .brz
# with the baseline. Exit 0 means 0 differences. `just ir-oracle --via-ir` goes
# through `compile --emit-ir` + `assemble` instead.
ir-oracle *ARGS:
    cargo build --release -p wirescript-cli
    node crates/wirescript/scripts/ir_oracle.mjs check {{ARGS}}

# Rebuild the oracle baseline with a given compiler binary. Only before a refactor starts.
ir-oracle-baseline bin:
    node crates/wirescript/scripts/ir_oracle.mjs baseline --bin {{bin}}

# Every gate the wirescript-ir refactor keeps green
ir-gates: ir-oracle doc-check lsp wasm-check
    cargo test -p wirescript
    cargo test -p wirescript-ir
    node crates/wirescript/scripts/ir_oracle.mjs check --via-ir
    cargo run -p wirescript-ir --features test-internals --example and_gate

# Parse and write throughput of the .wsir text form, on 2raab (the largest
# program in the oracle set). Compiles it to .wsir first.
ir-bench runs='5':
    cargo build --release -p wirescript-cli
    cargo run --release -p wirescript-cli -- compile target/ir-oracle/src/projects/2raab/src/main.ws -o target/ir-bench-2raab.brz --emit-ir target/ir-bench-2raab.wsir
    cargo run --release -p wirescript-ir --example wsir_bench -- target/ir-bench-2raab.wsir {{runs}}
