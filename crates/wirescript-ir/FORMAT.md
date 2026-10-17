# The `.wsir` text format

`.wsir` is the text form of an `ir::Program`: a graph of Brickadia logic gates
plus the settings that decide how it is laid out and emitted. The syntax
follows LLVM IR and MLIR's generic-op form: a gate is an instruction, and its
operands are the wires into its input ports.

`wirescript compile x.ws --emit-ir x.wsir` writes one and
`wirescript assemble x.wsir -o x.brz` builds the save from it. The parser and
writer in `src/wsir/` follow this document exactly.

```llvm
; wsir 1
source_filename = "and.ws"

module @and {
  %a = in bool
  %b = in bool
  %g = Expr_LogicalAND(bInputA: %a, bInputB: %b)
  out %y bool = %g
}
```

## Contents

- [Lexical structure](#lexical-structure)
- [File layout](#file-layout)
- [Values and names](#values-and-names)
- [Gates](#gates)
- [Inputs, outputs and chips](#inputs-outputs-and-chips)
- [Attributes](#attributes)
- [Port signatures](#port-signatures)
- [Metadata](#metadata)
- [Literals](#literals)
- [Types](#types)
- [Locations](#locations)
- [Defaults the parser fills in](#defaults-the-parser-fills-in)
- [Canonical output](#canonical-output)
- [Errors](#errors)
- [Grammar](#grammar)

## Lexical structure

- Whitespace (space, tab, CR, LF) separates tokens and carries no other
  meaning. An instruction may span lines.
- `;` starts a comment that runs to the end of the line.
- Line and column numbers in errors count bytes.
- An identifier is `[A-Za-z_][A-Za-z0-9_]*`.
- A value is `%` followed by `[A-Za-z0-9_]+`: `%count`, `%17`.
- A module name is `@` followed by an identifier or a string: `@race`,
  `@"my chip"`.
- A metadata keyword is `!` followed by an identifier (`!scope`); a metadata
  number is `!` followed by digits (`!3`).
- A string is `"..."` with the escapes `\"`, `\\`, `\n`, `\r`, `\t` and
  `\u{HEX}`. Any other character, including a raw line break, is itself.
- An integer is `-?[0-9]+`. A float has a `.` or an exponent (`5.0`,
  `-0.5`, `1e30`, `2.5e-7`), or is `inf`, `-inf`, `nan` or `-nan`.
- A color is `#` followed by eight hex digits, `#rrggbbaa`.
- Punctuation: `= ( ) { } [ ] , : . * -> => < > +`.

## File layout

```
; wsir 1                     ; optional header comment
source_filename = "chip.ws"  ; directives, any order
layout = dag
!source(...)                 ; code-layout source map, optional
!comment(...)
module @chip { ... }         ; exactly one root module
```

Directives are `name = value` lines before the first `module`:

| Directive | Value | Program field |
|---|---|---|
| `source_filename` | string | informational; the entry file |
| `layout` | `dag`, `code` or `cube` | `settings.layout` |
| `invisible` | `true` / `false` | `settings.invisible` |
| `no_gate_labels` | `true` / `false` | `settings.no_gate_labels` |
| `no_fold` | `true` / `false` | `settings.no_fold`: `.ws` source prefabs compile without folding |
| `doc` | string | `settings.module_doc` |
| `label` | string | root `root_label_override` |
| `label_from` | `%value.Port` | root `root_dynamic_label` |
| `color` | color literal (`#rrggbbaa`) | root `root_style.color`, baked |
| `color_from` | `%value.Port` | root `root_style.color`, wired |
| `material` | integer (`EBrickMaterial` value) | root `root_style.material`, baked |
| `material_from` | `%value.Port` | root `root_style.material`, wired |

All directives are optional; each defaults to the plain case (`dag`, `false`,
none).

The code-layout source map (`settings.source_map`) is written as top-level
metadata, only when the program has one:

```llvm
!source("race.ws", indent: [0, 2, 2, 4, 2, 0])
!comment("race.ws":12:3, "reset the lap", own_line)
!comment("race.ws":40:7, "row 3", in_array)
```

`!source` gives the file and its per-line indent table (0-based columns, one
per line). Each `!comment` is one `//` comment: its 1-based line and column,
its text, and the flags `own_line` and `in_array` when they hold.

## Values and names

Every node is defined once, as `%name = ...` (or `out %name ...`). Names are
unique across the whole program, not per module: a chip body may name a value
defined in its parent, and a parent may name a value inside a chip, because
wires cross module boundaries by node.

A use may come before its definition. The graph has cycles through buffers
and back-edges, so the parser resolves names in a second pass.

**Node ids.** Every node has a numeric id, and some layout decisions follow
it. The id is:

1. the name itself when the name is all digits (`%17` is node 17);
2. otherwise the number given by `!id N`;
3. otherwise one more than the id of the node defined just before it in the
   file (the first node is 1).

So a hand-written file needs no ids at all.

## Gates

```llvm
%g = <Class>(<InputPort>: <operand>, ...) { <attrs> } : <signature> <metadata> loc(...)
```

Everything after the class is optional, and the parentheses may be dropped
when there are no operands: `%n = Var { Value = 0 }`.

- **Class.** The component class, with or without its `BrickComponentType_`
  prefix. The short form is accepted only when it names exactly one class;
  the full name is always accepted. A class outside the inventory (such as
  the pseudo-class `_Literal`) is written in full.
- **Operands.** `Port: %node.OutPort` is a wire from `%node`'s `OutPort` into
  this gate's `Port`. `Port: %node` means `%node`'s only output port, and is
  an error when it has several. `Port: <literal>` is no wire: the literal
  becomes the property that bakes that unwired input (`Value: 5`).
- A port name that is not an identifier is quoted, here and in signatures
  and references: `"Position.X": %v.X`, `%d."Position.X"`.
- An input takes at most one wire. Two wires into one input cannot be
  written, which is intended: wire fan-in fails to load in game. Fan-out is
  using a value twice.
- **Kind.** A node's kind is inferred: a gate whose inventory entry is an
  event component is an `Event`, any other is a `Gate`. `!kind` overrides it.

## Inputs, outputs and chips

```llvm
%a = in bool                    ; a module input; its PortLabel defaults to "a"
%p = in exec(%tick)             ; a chip boundary pin fed by %tick
%f = in FunctionInput int       ; another input class
out %y bool = %g                ; a module output and its one wire
out %z int                      ; an output with nothing wired in yet
```

- `in [Class] [type] [(operand)]` defines an input node. The class defaults to
  `Internal_MicrochipInput`. The operand, if any, is the wire into its
  `RER_Input`; write `in int(Port: %x)` for another port.
- `out %name [Class] [type] [= operand]` defines an output node. The class
  defaults to `Internal_MicrochipOutput`, and the operand is the wire into
  its `RER_Input`.
- Unless the node has a `PortLabel` attribute or the `nolabel` flag, an input
  or output gets `PortLabel = "<its name>"`.
- The type sets the pin's port types (see [Defaults](#defaults-the-parser-fills-in)).
  An omitted type is taken from the source of the operand, or `any` when
  there is none. `{ }` right after `in` or `out %name` is the empty record
  type, not empty attributes.

A chip is a node that holds a child module:

```llvm
%c = chip { closed } !layout_after(%tick, %n) {
  %p0 = in exec(%tick)
  %inc = Exec_Var_Increment(Exec: %p0, VarRef: %p1)
} loc("chip.ws":3:9)
```

`chip [Class] [@name] [(operands)] [attrs] [signature] [metadata] { body } [loc]`.
The class defaults to `Component_Internal_Microchip`, and the child module's
name to `_anon_n<id>`. Operands are the wires into the chip's own ports, such
as a loop shell's `Count`. The body is the child module, written like the
root's. It always has braces, even when empty. Empty attributes are written
as no braces at all, so a lone `{ }` after `chip` is always the body.

## Attributes

`{ ... }` after the operands is the attribute dictionary: the gate's other
properties. Each entry is `key = <literal>`, or a bare key for a flag. A key
is an identifier, or a string for a property name that is not one.

These keys are annotations, spelled short:

| Written | Property |
|---|---|
| `closed` | `_closed = true` (a chip compiled collapsed) |
| `label = "..."` | `_label` (a declaration's source name, shown on its gate) |
| `side = left` | `_reroute_side = "left"` (also `right`, `top`, `bottom`) |
| `invisible` | `_reroute_invisible = true` |
| `doc = "..."` | `_doc` (a chip's doc header) |
| `handle_target = %x` | `_handle_target` = `%x`'s node id |
| `nolabel` | not a property: suppresses an input/output's default `PortLabel` |

Any other key is the property of that name, written as is: `PortLabel = "go"`,
`_bindname = "init"`, `_variant = 1`.

A bare word value (`side = left`) is a string. A word that starts a literal
(`true`, `nan`, `null`, `vec`, ...) is read as that literal, so the writer
quotes such a value: `side = "true"`.

## Port signatures

```llvm
%s = Exec_Var_Set(Exec: %go, VarRef: %n.VarRef, Value: 5)
       : (Exec: exec, VarRef: *int, Value: int) -> (ExecOut: exec)
```

A node's ports are a list of named, typed inputs and outputs. When the
signature is omitted, the parser uses the node's default ports (see below).
The writer writes a signature exactly when the node's ports differ from that
default. Port order is significant.

## Metadata

Metadata uses LLVM's `!`. It is all optional: a hand-written program needs
none of it. The writer emits it to make a lowered program round-trip exactly.

On a node, after the signature:

| Metadata | Meaning |
|---|---|
| `!id 17` | this node's id (see [Values and names](#values-and-names)) |
| `!scope !3` | the lexical scope the node was lowered under (default `!0`) |
| `!chain 7` | the node's exec-chain id (layout row hint) |
| `!chip %c` | the anonymous chip the node is assigned to (`chip_id`) |
| `!label(%x.Port)` | the node's floating label is driven at runtime by `%x.Port` |
| `!color(%x.Port)` | the node's brick color is driven at runtime by `%x.Port` (`dynamic_colors`) |
| `!material(%x.Port)` | the node's brick material is driven at runtime by `%x.Port` (`dynamic_materials`) |
| `!layout_after(%a, %b)` | layout-only edges from `%a` and `%b` into this node; not wires |
| `!kind event` | the node's kind: `gate`, `event`, `chip`, `input`, `output` or `coerce` |
| `!note "fn call"` | the lowering note on the node |

Inside a module body, after its nodes:

| Metadata | Meaning |
|---|---|
| `!3 = !scope(IfThen, parent: !2, loc(...))` | scope 3 of this module |
| `!inputs(%a, %b)` | the module's input list, in order |
| `!outputs(%y)` | the module's output list, in order |
| `!wire_order(4, 0, 1)` | the wires this module stores, in order (see below) |

A scope kind is `ModuleRoot`, `ChipBody "name"`, `HandlerBody "trigger"`,
`IfGroup`, `IfCond`, `IfThen`, `IfElse` or `Block`. `parent` and the location
are optional. A node or module reference that names no node in the file is
written as its bare id.

**Wire order.** The parser numbers the program's wires in file order: node by
node, each node's operand wires left to right, then its `!layout_after` edges.
Without `!wire_order`, a wire is stored in the innermost module that contains
both of its ends, and each module keeps its wires in file order. A module with
`!wire_order(...)` stores exactly the listed wires in the listed order;
every other wire goes to its default module.

## Literals

Each `ir::Literal` variant has one spelling:

| Literal | Spelling |
|---|---|
| `Bool` | `true`, `false` |
| `Int` | `5`, `-3` |
| `Float` | `5.0`, `-0.5`, `1e30`, `inf`, `-inf`, `nan`, `-nan` |
| `String` | `"text"` |
| `Vector` | `vec(1.0, 2.0, 3.0)` |
| `Rotator` | `rot(pitch, yaw, roll)` |
| `Quat` | `quat(x, y, z, w)` |
| `Color` | `#ff8000ff` |
| `LinearColor` | `lrgba(1.0, 0.5, 0.0, 1.0)` |
| `Object` | `null` |
| `Array` | `[1, 2, 3]` |
| `Map` | `map[1 => "a", 2 => "b"]` |
| `Record` | `rec{x: 1, y: 2}` |
| `Asset` | `asset("Type", "Name")` |
| `PrefabRef` | `prefab("path/relative/to/this/file.brz")` |
| `NestedPrefab` | `nested_prefab("source text")` |
| `PrefabBytes` | `prefab_bytes("<base64 of the .brz>")` |

A float is written in Rust's shortest round-trip form, so text to IR to text is
exact. A NaN keeps its sign (`-nan`) but not its payload. Inside `vec`, `rot`, `quat` and `lrgba` an integer is accepted for a
float component.

`--emit-ir` compiles every nested prefab to its `.brz` bytes (`prefab_bytes`),
so `assemble` needs no front end for them. A `prefab("...")` path is relative to
the `.wsir` file.

## Types

Types spell as in Wirescript, with compound forms that parse unambiguously:

| Type | Spelling |
|---|---|
| scalars | `bool int float string vector rotator quat color entity character controller brick zone teleport prefab exec any opaque never` |
| `Ref(T)` | `*T` |
| `Array(T)` | `T[]`, or `array<T>`; an array of refs is `array<*T>` |
| `Map(K, V)` | `map<K, V>` |
| `Union` | `union<A, B>` |
| `Tuple` | `tuple<A, B>` |
| `Record` | `{x: int, y: int}` |
| `Outputs` | `outputs{min: int, max: int}` |
| `Param` | `param<"T">` |
| `Enum` | `enum<"Option", int>` |
| `Component` | `component<"Component_PointLight">` |

## Locations

`loc("file":line:col)` is a node's source position, as in MLIR. The full form
also gives byte offsets and an end:

```llvm
loc("chip.ws":7:9+120 to 7:22+133)
```

`+N` is the byte offset of that position. Each part after the first
`line:col` is optional; a missing offset is 0 and a missing end equals the
start. Without a file, `loc(7:9)` is in the `source_filename` file, which the
writer uses for every location in the entry file. A node with no location
has an empty source range.

## Defaults the parser fills in

| Field | Default |
|---|---|
| ports, inventory class | the inventory entry's ports, each typed by its kind (`bool`, `int`, ...; references and wildcards are `any`); then each `any` input that an operand wires takes the type of that operand's output port |
| ports, `in`/`out` of type `T` | `(RER_Input: T) -> (RER_Output: T)` |
| ports, `_Literal` | `() -> (Output: <type of its Value>)` |
| ports, anything else | none |
| kind | see [Gates](#gates) |
| scope | `!0` |
| module scopes | `!0 = !scope(ModuleRoot)` |
| `inputs` / `outputs` | the module's input / output nodes in id order |
| wire order | see [Metadata](#metadata) |

## Canonical output

The writer is deterministic, so text to IR to text is a fixed point and two
compiles of one program diff cleanly:

- directives in the order of the table above, then `!source` and `!comment`;
- within a module, nodes in ascending id order; a chip's body sits where the
  chip is defined;
- operands in port order, attributes sorted by key, then signature, metadata
  and location;
- module metadata last: scopes by number, then `!inputs`, `!outputs` and
  `!wire_order`.

A node read from text keeps its name (`Module::value_names`). Otherwise a
name is taken from the node's source name where it has one (an input's or
output's `PortLabel`, a declaration's `_label` or `_bindname`), made unique
with an `_<id>` suffix, and failing that is the node's id. Names go to nodes
in id order, so a declaration gets its plain name before a boundary pin
copied from it.

## Errors

Every parse error gives `file:line:col`, the token found, and what was
expected. Resolution errors say what is wrong with what was named:

- an unknown port lists the valid ports of that node;
- an operand naming a node with several outputs, written without `.Port`,
  lists those outputs;
- a name used but never defined, or defined twice, names the name.

## Grammar

```
file       := directive* source_meta* module
directive  := IDENT '=' (STRING | IDENT | BOOL | ref | literal)
source_meta:= '!source' '(' STRING ',' 'indent' ':' '[' (INT (',' INT)*)? ']' ')'
            | '!comment' '(' STRING ':' INT ':' INT ',' STRING (',' IDENT)* ')'
module     := 'module' MODNAME body
body       := '{' item* modmeta* '}'
item       := VALUE '=' rhs | 'out' VALUE outrhs
rhs        := 'in' CLASS? type? ('(' (PORT ':')? operand ')')? tail
            | 'chip' CLASS? MODNAME? ('(' (opnd (',' opnd)*)? ')')? attrs? sig? meta* body loc?
            | CLASS ('(' (opnd (',' opnd)*)? ')')? tail
outrhs     := CLASS? type? ('=' operand)? tail
tail       := attrs? sig? meta* loc?
opnd       := PORT ':' operand
operand    := ref | literal
ref        := VALUE ('.' PORT)?
attrs      := '{' (attr (',' attr)*)? '}'
attr       := KEY ('=' (literal | IDENT | ref))?
sig        := ':' '(' ports? ')' '->' '(' ports? ')'
ports      := PORT ':' type (',' PORT ':' type)*
meta       := '!id' INT | '!scope' MDNUM | '!chain' INT | '!chip' (VALUE | INT)
            | ('!label' | '!color' | '!material') '(' ref ')' | '!layout_after' '(' VALUE (',' VALUE)* ')'
            | '!kind' IDENT | '!note' STRING
modmeta    := MDNUM '=' '!scope' '(' SCOPEKIND STRING? (',' 'parent' ':' MDNUM)? (',' loc)? ')'
            | ('!inputs' | '!outputs') '(' ((VALUE | INT) (',' (VALUE | INT))*)? ')'
            | '!wire_order' '(' (INT (',' INT)*)? ')'
loc        := 'loc' '(' (STRING ':')? INT ':' INT ('+' INT)? ('to' INT ':' INT ('+' INT)?)? ')'
```
