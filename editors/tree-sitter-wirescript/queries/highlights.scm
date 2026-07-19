; Wirescript highlight queries.
;
; Capture names follow the nvim-treesitter / Helix convention.
;
; PRECEDENCE: when several patterns capture the same node, the one appearing
; LAST in this file wins (verified against `tree-sitter highlight`, and the
; same convention nvim-treesitter uses). So this file is ordered
; general -> specific: the `(identifier) @variable` catch-all is near the top
; and the narrow, context-sensitive rules come last.

; ------------------------------------------------------------ catch-all first

(identifier) @variable

; ---------------------------------------------------------------- punctuation

[
  "(" ")"
  "[" "]"
  "{" "}"
] @punctuation.bracket

[
  ","
  ":"
  ";"
  "."
] @punctuation.delimiter

; ------------------------------------------------------------------ operators

[
  "+" "-" "*" "/" "%" "**" ".."
  "==" "!=" "<" "<=" ">" ">="
  "&&" "||" "^^" "!"
  "&" "|" "^" "~" "<<" ">>"
  "="
  "+=" "-=" "*=" "/=" "%="
  "&=" "|=" "^=" "<<=" ">>="
  "->"
  "..."
] @operator

; ------------------------------------------------------------------- keywords

[
  "var"
  "static"
  "buffer"
  "array"
  "let"
  "type"
  "in"
  "out"
  "chip"
  "mod"
  "fn"
  "open"
] @keyword

[
  "import"
  "from"
  "as"
] @keyword.import

[
  "if"
  "else"
  "then"
] @keyword.conditional

"return" @keyword.return

[
  "on"
  "emit"
  "await"
] @keyword.coroutine

"ref" @keyword.modifier

; ------------------------------------------------------------------- literals

(integer) @number
(float) @number.float
(boolean) @boolean

(string) @string
(string_fragment) @string
(escape_sequence) @string.escape

(interpolation "${" @punctuation.special)
(interpolation "}" @punctuation.special)

(asset_reference) @string.special
(prefab_reference) @string.special.path

; ---------------------------------------------------------------------- types

(type_identifier) @type

((type_identifier) @type.builtin
  (#any-of? @type.builtin
    "int" "float" "bool" "string" "exec"
    "vector" "rotator" "quat" "color"
    "entity" "character" "controller" "brick" "prefab"
    "any" "never"))

(type_alias_declaration name: (identifier) @type.definition)

; ------------------------------------------------------------ declared names

(var_declaration name: (identifier) @variable)
(buffer_declaration name: (identifier) @variable)
(array_declaration name: (identifier) @variable)
(let_declaration name: (identifier) @variable)
(chip_let_binding name: (identifier) @variable)
(emit_statement name: (identifier) @variable)

(in_declaration name: (identifier) @variable.parameter)
(out_declaration name: (identifier) @variable.parameter)
(named_output name: (identifier) @variable.parameter)

(parameter pattern: (identifier) @variable.parameter)
(field_pattern name: (identifier) @variable.parameter)
(rest_pattern name: (identifier) @variable.parameter)
(tuple_pattern (identifier) @variable.parameter)

(import_specifier name: (identifier) @variable)
(import_specifier alias: (identifier) @variable)
(namespace_import alias: (identifier) @module)

; ------------------------------------------------------------------- members

(field_expression field: (identifier) @variable.member)
(record_field name: (identifier) @variable.member)
(shorthand_field name: (identifier) @variable.member)
(record_type_field name: (identifier) @variable.member)

; --------------------------------------------------------- functions & calls

(chip_declaration name: (identifier) @function)
(mod_declaration name: (identifier) @function)
(function_declaration name: (identifier) @function)

(call_expression
  function: (identifier) @function.call)

; Receiver-style calls: `entity.SetLocation(...)`, `arr.push(x)`.
; Must come after the plain `field_expression` rule so the method name wins.
(call_expression
  function: (field_expression
    field: (identifier) @function.method.call))

(named_argument name: (identifier) @variable.parameter)

; ------------------------------------------------------------------ triggers

; Event names (`RoundStart`, `CharacterDied`, …) read as constants.
(handler trigger: (identifier) @constant)
(handler
  trigger: (call_expression
    function: (identifier) @constant))
(event_source trigger: (identifier) @constant)
(trigger_field object: (identifier) @constant)
(trigger_field field: (identifier) @variable.member)

; --------------------------------------------------------------- annotations

(annotation (annotation_name) @attribute)
(annotation (string) @string)

; ------------------------------------------------------------------ comments

(line_comment) @comment
(block_comment) @comment
(doc_comment) @comment.documentation
