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
    cargo build --release -p wirescript -p wirescript-lsp -p bearilog-cli

# Build LSP server (release)
lsp:
    cargo build --release -p wirescript-lsp

# Build check CLI (release)
check-bin:
    cargo build --release -p wirescript --bin wirescript-check

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
    cargo run --release -p bearilog-cli -- compile {{file}}

# Compile a .ws file to .brdb (SQLite, for BR.World.LoadAdditive)
compile-brdb file:
    cargo run --release -p bearilog-cli -- compile {{file}} -o {{without_extension(file)}}.brdb

# Dump the lowered IR for a .ws file
ir file:
    cargo run --release -p bearilog-cli -- compile {{file}} --dump-ir

# Rebuild VS Code extension (compile TS + formatter)
[windows]
vscode:
    Set-Location editors/vscode; npm install; npm run build

# Rebuild VS Code extension (compile TS + formatter)
[unix]
vscode:
    cd editors/vscode && npm install && npm run build

# Copy wirescript docs into playground for serving
[windows]
playground-docs:
    Copy-Item -Path docs/wirescript/*.md -Destination crates/wasm/playground/docs/ -Force

# Copy wirescript docs into playground for serving
[unix]
playground-docs:
    cp -f docs/wirescript/*.md crates/wasm/playground/docs/

# Build everything (lib + lsp + cli + wasm + vscode)
all: release wasm vscode
