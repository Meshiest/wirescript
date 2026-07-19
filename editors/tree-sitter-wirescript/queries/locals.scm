; Scope and binding information for Wirescript.
;
; Wirescript scoping is lexical: a chip/mod body, a handler body and a block
; expression each introduce a scope, and `chip`/`mod` parameters plus the
; `var`/`let`/`buffer`/`array`/`in` declarations inside them are the bindings.
; Note that chips also resolve names from enclosing scopes, so these scopes are
; nested rather than isolated.

; ------------------------------------------------------------------- scopes

(source_file) @local.scope
(block) @local.scope
(block_expression) @local.scope
(chip_declaration) @local.scope
(mod_declaration) @local.scope
(function_declaration) @local.scope
(handler) @local.scope
(event_source) @local.scope

; ----------------------------------------------------------------- bindings

(var_declaration name: (identifier) @local.definition.var)
(buffer_declaration name: (identifier) @local.definition.var)
(array_declaration name: (identifier) @local.definition.var)
(let_declaration name: (identifier) @local.definition.var)
(chip_let_binding name: (identifier) @local.definition.var)

(in_declaration name: (identifier) @local.definition.parameter)
(out_declaration name: (identifier) @local.definition.var)
(named_output name: (identifier) @local.definition.parameter)

(parameter pattern: (identifier) @local.definition.parameter)
(field_pattern name: (identifier) @local.definition.parameter)
(rest_pattern name: (identifier) @local.definition.parameter)
(tuple_pattern (identifier) @local.definition.parameter)

(chip_declaration name: (identifier) @local.definition.function)
(mod_declaration name: (identifier) @local.definition.function)
(function_declaration name: (identifier) @local.definition.function)

(type_alias_declaration name: (identifier) @local.definition.type)

(import_specifier name: (identifier) @local.definition.import)
(import_specifier alias: (identifier) @local.definition.import)
(namespace_import alias: (identifier) @local.definition.namespace)

; --------------------------------------------------------------- references

(identifier) @local.reference
