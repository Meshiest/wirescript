; Indentation queries (nvim-treesitter `indents.scm` capture vocabulary).
;
; Wirescript is brace-delimited, so indentation follows the bracket pairs.
; Newlines are not significant to the parser, which means indentation is purely
; a formatting convention — these rules reproduce the style used across
; `examples/` and `projects/`.

; Nodes whose body should indent one level.
[
  (block)
  (block_expression)
  (record_literal)
  (record_type)
  (record_pattern)
  (array_literal)
  (argument_list)
  (parameter_list)
  (output_list)
  (tuple_type)
  (tuple_pattern)
  (tuple_expression)
  (named_imports)
] @indent.begin

; The closing bracket returns to the parent's level.
[
  "}"
  ")"
  "]"
] @indent.branch @indent.end

; Comments keep whatever indentation the author gave them.
[
  (line_comment)
  (block_comment)
  (doc_comment)
] @indent.auto
