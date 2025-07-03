## Bearilog

This project requires [rust](https://www.rust-lang.org/) v1.88 or later.

## Usage

### Output to BRDB file

This will build a module and output it to a BRDB file.

```sh
cargo run ./examples/cpu reg64_16 -o example.brdb
```

Render as a big blob of bricks

```sh
cargo run ./examples/cpu reg64_16 -o example.brdb --layout grid --inline
cargo run ./examples/7seg bitwise7seg -o 7segbit.brdb --layout grid --iobelow
```

### Display a module

This will build a module and print out its structure

```sh
cargo run ./examples/cpu reg64_16
```

### Display a module as a graphviz graph

Output for a browser:

```sh
cargo run ./examples/cpu reg64_16 -g
```

Render from CLI:

```sh
cargo run ./examples/cpu reg64_16 -g | dot -q -Tsvg > output.svg
```