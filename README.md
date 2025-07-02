## Bearilog

This project requires [rust](https://www.rust-lang.org/) v1.88 or later.

## Usage

### Output to BRDB file

This will build a module and output it to a BRDB file.

```sh
cargo run ./examples/cpu reg64 -o example.brdb
```

### Display a module

This will build a module and print out its structure

```sh
cargo run ./examples/cpu reg64
```

### Display a module as a graphviz graph

Output for a browser:

```sh
cargo run ./examples/cpu reg64 -g
```

Render from CLI:

```sh
cargo run ./examples/cpu reg64 -g | dot -q -Tsvg > output.svg
```