/**
 * @file Wirescript grammar for tree-sitter
 * @license MIT
 *
 * Derived from the authoritative Rust implementation in
 * `crates/wirescript/src/lexer.rs` and `crates/wirescript/src/parser.rs`.
 *
 * Notes on fidelity (see README.md "Deviations" for the full list):
 *  - The reference parser makes newlines lexer tokens but then skips them
 *    almost everywhere (binary-operator continuation, list separators,
 *    statement ends). Statements there are self-delimiting, so this grammar
 *    treats newlines as ordinary whitespace.
 *  - `match` is a reserved keyword in the lexer but has NO production in the
 *    reference parser, so it has none here either.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// Infix binding powers, copied verbatim from `infix_prec()` in parser.rs.
const PREC = {
  or: 2, // || ^^
  and: 3, // &&
  bitor: 4, // |
  bitxor: 5, // ^
  bitand: 6, // &
  equality: 7, // == !=
  comparison: 8, // < <= > >=
  shift: 9, // << >>
  additive: 10, // + - ..
  multiplicative: 11, // * / %
  power: 12, // **  (right associative)
  unary: 15, // - ! ~ * & ref
  postfix: 16, // call, index, field, tuple pick
};

module.exports = grammar({
  name: 'wirescript',

  word: ($) => $.identifier,

  extras: ($) => [/\s/, $.doc_comment, $.line_comment, $.block_comment],

  supertypes: ($) => [$._expression, $._statement, $._type],

  rules: {
    source_file: ($) => repeat($._top_level_item),

    // `import`, `type` and `fn` are accepted only at the top level by
    // `parse_top_decl`; `parse_stmt` does not handle them.
    _top_level_item: ($) =>
      seq(
        choice(
          $.import_declaration,
          $.type_alias_declaration,
          $.function_declaration,
          $._statement,
        ),
        // `eat_stmt_end` treats `;` and newline as interchangeable, optional
        // statement terminators.
        optional(';'),
      ),

    _block_statement: ($) => seq($._statement, optional(';')),

    _statement: ($) =>
      choice(
        $.var_declaration,
        $.buffer_declaration,
        $.array_declaration,
        $.in_declaration,
        $.out_declaration,
        $.let_declaration,
        $.chip_declaration,
        $.mod_declaration,
        $.handler,
        $.emit_statement,
        $.await_expression,
        $.return_statement,
        $.if_statement,
        $.assignment_statement,
        $.expression_statement,
      ),

    // ---------------------------------------------------------------- comments

    // `///` must out-rank `//`; both match the same span otherwise.
    doc_comment: (_$) => token(prec(1, seq('///', /[^\n]*/))),
    line_comment: (_$) => token(seq('//', /[^\n]*/)),
    // NOTE: the reference lexer nests block comments; a regex cannot.
    block_comment: (_$) => token(seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/')),

    // ------------------------------------------------------------- annotations

    // The lexer emits `@word` as a single token, so `@` and the word must be
    // adjacent. `@label` additionally takes a parenthesised string.
    annotation: ($) =>
      seq(
        field('name', $.annotation_name),
        optional(seq('(', optional(field('argument', $.string)), ')')),
      ),

    annotation_name: (_$) => token(seq('@', /[a-zA-Z_][a-zA-Z0-9_]*/)),

    _annotations: ($) => repeat1($.annotation),

    // ------------------------------------------------------------------ import

    import_declaration: ($) =>
      seq(
        'import',
        choice(
          field('source', $.string),
          seq(
            choice($.named_imports, $.namespace_import),
            'from',
            field('source', $.string),
          ),
        ),
      ),

    named_imports: ($) => seq('{', commaSepTrailing($.import_specifier), '}'),

    import_specifier: ($) =>
      seq(
        field('name', $.identifier),
        optional(seq('as', field('alias', $.identifier))),
      ),

    namespace_import: ($) => seq('*', 'as', field('alias', $.identifier)),

    // -------------------------------------------------------------- type alias

    type_alias_declaration: ($) =>
      seq('type', field('name', $.identifier), '=', field('value', $._type)),

    // ---------------------------------------------------------------- bindings

    var_declaration: ($) =>
      seq(
        optional($._annotations),
        optional('static'),
        'var',
        field('name', $.identifier),
        optional(seq(':', field('type', $._type))),
        optional(seq('=', field('value', $._expression))),
      ),

    buffer_declaration: ($) =>
      seq(
        'buffer',
        field('name', $.identifier),
        optional(seq(':', field('type', $._type))),
        '=',
        field('value', $._expression),
      ),

    array_declaration: ($) =>
      seq(
        'array',
        field('name', $.identifier),
        ':',
        field('type', $._type),
        optional(seq('=', field('value', $.array_literal))),
      ),

    in_declaration: ($) =>
      seq(
        optional($._annotations),
        'in',
        field('name', $.identifier),
        ':',
        field('type', $._type),
      ),

    out_declaration: ($) =>
      seq(
        optional($._annotations),
        'out',
        field('name', $.identifier),
        optional(seq(':', field('type', $._type))),
        optional(seq('=', field('value', $._expression))),
      ),

    // `let x = 1`, `let sig: exec`, `let {a, b: c} = r`, `let (a, b) = t`,
    // `let e = on Trigger { … }`, `let v = await x on done`
    let_declaration: ($) =>
      seq(
        optional($._annotations),
        'let',
        field('name', choice($.identifier, $.record_pattern, $.tuple_pattern)),
        optional(seq(':', field('type', $._type))),
        optional(
          seq(
            '=',
            field(
              'value',
              choice($._expression, $.event_source, $.await_expression),
            ),
          ),
        ),
      ),

    // `on Trigger` / `on Trigger { captured body }` as a `let` initialiser.
    // Unlike a handler, `parse_let_decl` calls `parse_trigger` directly, so the
    // trigger here is the restricted trigger grammar — never a call.
    event_source: ($) =>
      prec.right(
        seq('on', field('trigger', $._trigger), optional(field('body', $.block))),
      ),

    // Mirrors `parse_trigger` / `parse_trigger_atom`:
    //   atom := '(' trigger ')' | '!' atom | ident ('.' ident)?
    //   trigger := atom ('|' atom)*
    _trigger: ($) => choice($._trigger_atom, $.trigger_union),

    _trigger_atom: ($) =>
      choice(
        $.identifier,
        $.trigger_field,
        $.trigger_not,
        $.trigger_group,
      ),

    trigger_union: ($) =>
      prec.left(seq($._trigger_atom, repeat1(seq('|', $._trigger_atom)))),

    trigger_field: ($) =>
      seq(field('object', $.identifier), '.', field('field', $.identifier)),

    trigger_not: ($) => seq('!', $._trigger_atom),

    trigger_group: ($) => seq('(', $._trigger, ')'),

    // --------------------------------------------------------------- patterns

    record_pattern: ($) =>
      seq('{', commaSepTrailing(choice($.field_pattern, $.rest_pattern)), '}'),

    field_pattern: ($) =>
      seq(
        field('name', $.identifier),
        optional(seq(':', field('alias', $.identifier))),
      ),

    rest_pattern: ($) => seq('...', field('name', $.identifier)),

    tuple_pattern: ($) =>
      seq('(', commaSepTrailing(choice($.identifier, $.rest_pattern)), ')'),

    // -------------------------------------------------------- chips, mods, fns

    // One rule covers every `chip` form so the shared `chip` prefix never
    // forces an early reduce. A named chip is the variant carrying a `name`
    // field; the others are anonymous.
    chip_declaration: ($) =>
      seq(
        optional($._annotations),
        optional('open'),
        'chip',
        choice(
          seq(
            field('name', $.identifier),
            field('parameters', $.parameter_list),
            optional(seq('->', field('output', $._chip_output))),
            field('body', $.block),
          ),
          field('body', $.block),
          // `chip let a = 1, b = 2`
          seq('let', commaSep1($.chip_let_binding)),
          // `chip on Trigger { … }`
          field('body', $.handler),
        ),
      ),

    chip_let_binding: ($) =>
      seq(
        field('name', $.identifier),
        optional(seq(':', field('type', $._type))),
        '=',
        field('value', $._expression),
      ),

    mod_declaration: ($) =>
      seq(
        'mod',
        field('name', $.identifier),
        field('parameters', $.parameter_list),
        optional(seq('->', field('output', $._chip_output))),
        field('body', $.block),
      ),

    // `fn name(params) -> T = expr` — expression bodied.
    function_declaration: ($) =>
      seq(
        'fn',
        field('name', $.identifier),
        field('parameters', $.parameter_list),
        optional(seq('->', field('return_type', $._type))),
        '=',
        field('body', $._expression),
      ),

    parameter_list: ($) => seq('(', commaSepTrailing($.parameter), ')'),

    parameter: ($) =>
      seq(
        field(
          'pattern',
          choice($.identifier, $.record_pattern, $.tuple_pattern),
        ),
        ':',
        field('type', $._type),
      ),

    // `-> (a: int, b: bool)` or `-> int`
    _chip_output: ($) => choice($.output_list, $._type),

    // `parse_chip_outputs` always reads a `(` as an output list, never as a
    // tuple type, so `-> ()` resolves to an (empty) output list.
    output_list: ($) => prec(1, seq('(', commaSepTrailing($.named_output), ')')),

    named_output: ($) =>
      seq(field('name', $.identifier), ':', field('type', $._type)),

    // --------------------------------------------------------------- handlers

    handler: ($) =>
      seq(
        optional($._annotations),
        'on',
        field('trigger', $._handler_trigger),
        field('body', $.block),
      ),

    // `parse_handler` accepts either a simple trigger or, via
    // `looks_like_expr_trigger`, an arbitrary expression. A simple trigger is
    // itself a valid expression (idents, `.field`, `!`, `|`, parens, and the
    // arg list as a call), so one expression rule covers both.
    _handler_trigger: ($) =>
      choice($._expression_no_brace, $._if_expression_no_brace),

    // ------------------------------------------------------------- statements

    block: ($) => seq('{', repeat($._block_statement), '}'),

    emit_statement: ($) =>
      seq(
        optional(field('buffer', $.buffer_modifier)),
        'emit',
        field('name', $.identifier),
        optional(seq('=', field('value', $._expression))),
      ),

    // `buffer`, `buffer(3)`, `buffer(0.5s, 2)`
    buffer_modifier: ($) =>
      seq(
        'buffer',
        optional(
          seq(
            '(',
            field('delay', $.duration),
            optional(seq(',', field('hold', $.duration))),
            ')',
          ),
        ),
      ),

    duration: ($) =>
      seq($._expression, optional(field('unit', alias('s', $.time_unit)))),

    // `await sig` / `await value on sig`
    // Greedy on `on`: `await v on sig` binds the trailing `on` to the await
    // rather than starting a new handler, matching `parse_await_inner`.
    await_expression: ($) =>
      prec.right(
        seq(
          'await',
          field('value', $._expression),
          optional(seq('on', field('signal', $._expression))),
        ),
      ),

    // Greedy: `return (x)` returns `(x)`. The reference parser stops a bare
    // `return` at a newline; this grammar has no newline token, so a value on
    // the *following* line is absorbed. See README "Deviations".
    return_statement: ($) =>
      prec.right(seq('return', optional(field('value', $._expression)))),

    if_statement: ($) =>
      prec.right(
        seq(
          'if',
          field('condition', $._expression_no_brace),
          field('consequence', $.block),
          optional(
            seq('else', field('alternative', choice($.if_statement, $.block))),
          ),
        ),
      ),

    assignment_statement: ($) =>
      seq(
        field('left', $._expression_no_brace),
        field(
          'operator',
          choice(
            '=',
            '+=',
            '-=',
            '*=',
            '/=',
            '%=',
            '&=',
            '|=',
            '^=',
            '<<=',
            '>>=',
          ),
        ),
        field('right', $._expression),
      ),

    // Lower precedence than the postfix operators so that a `(`, `[` or `.`
    // starting the next line keeps extending the expression, which is what the
    // reference parser's `parse_postfix` loop does.
    expression_statement: ($) => prec(-1, $._expression_no_brace),

    // ------------------------------------------------------------------ types

    _type: ($) => choice($._type_postfix, $.union_type),

    union_type: ($) =>
      prec.left(seq($._type_postfix, repeat1(seq('|', $._type_postfix)))),

    _type_postfix: ($) => choice($._type_primary, $.array_type),

    // Greedy, like the `while match_tok(LBracket)` loop in `parse_type_postfix`.
    array_type: ($) =>
      prec.left(1, seq(field('element', $._type_postfix), '[', ']')),

    _type_primary: ($) =>
      choice(
        alias($.identifier, $.type_identifier),
        $.ref_type,
        $.tuple_type,
        $.record_type,
      ),

    // Both spellings mean the same thing: `ref T` and `*T`.
    ref_type: ($) =>
      prec.right(seq(choice('ref', '*'), field('inner', $._type_postfix))),

    tuple_type: ($) => seq('(', commaSepTrailing($._type), ')'),

    record_type: ($) => seq('{', commaSepTrailing($.record_type_field), '}'),

    record_type_field: ($) =>
      seq(field('name', $.identifier), ':', field('type', $._type)),

    // ------------------------------------------------------------ expressions

    _expression: ($) =>
      choice(
        $._expression_no_brace,
        $.record_literal,
        $.block_expression,
        $.if_expression,
      ),

    // Everything that cannot begin with `{` and cannot be an `if … then … else`.
    // Used wherever a `{` immediately following the expression opens a block
    // (`if`, `on`, statement position).
    _expression_no_brace: ($) =>
      choice(
        $.identifier,
        $.integer,
        $.float,
        $.string,
        $.boolean,
        $.array_literal,
        $.asset_reference,
        $.prefab_reference,
        $.parenthesized_expression,
        $.tuple_expression,
        $.unary_expression,
        $.reference_expression,
        $.dereference_expression,
        $.binary_expression,
        $.call_expression,
        $.field_expression,
        $.tuple_index_expression,
        $.index_expression,
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    tuple_expression: ($) =>
      seq(
        '(',
        $._expression,
        ',',
        optional(
          seq($._expression, repeat(seq(',', $._expression)), optional(',')),
        ),
        ')',
      ),

    unary_expression: ($) =>
      prec.right(
        PREC.unary,
        seq(field('operator', choice('-', '!', '~')), field('argument', $._expression)),
      ),

    // `&x` and `ref x` both take a reference.
    reference_expression: ($) =>
      prec.right(
        PREC.unary,
        seq(field('operator', choice('&', 'ref')), field('argument', $._expression)),
      ),

    dereference_expression: ($) =>
      prec.right(PREC.unary, seq('*', field('argument', $._expression))),

    binary_expression: ($) => {
      const table = [
        [PREC.or, choice('||', '^^')],
        [PREC.and, '&&'],
        [PREC.bitor, '|'],
        [PREC.bitxor, '^'],
        [PREC.bitand, '&'],
        [PREC.equality, choice('==', '!=')],
        [PREC.comparison, choice('<', '<=', '>', '>=')],
        [PREC.shift, choice('<<', '>>')],
        [PREC.additive, choice('+', '-', '..')],
        [PREC.multiplicative, choice('*', '/', '%')],
      ];
      return choice(
        ...table.map(([precedence, operator]) =>
          prec.left(
            /** @type {number} */ (precedence),
            seq(
              field('left', $._expression),
              field('operator', operator),
              field('right', $._expression),
            ),
          ),
        ),
        // `**` is the only right-associative operator.
        prec.right(
          PREC.power,
          seq(
            field('left', $._expression),
            field('operator', '**'),
            field('right', $._expression),
          ),
        ),
      );
    },

    call_expression: ($) =>
      prec(
        PREC.postfix,
        seq(
          field('function', $._expression),
          field('arguments', $.argument_list),
        ),
      ),

    argument_list: ($) =>
      seq(
        '(',
        commaSepTrailing(
          choice($.named_argument, $.spread_argument, $._expression),
        ),
        ')',
      ),

    named_argument: ($) =>
      seq(field('name', $.identifier), '=', field('value', $._expression)),

    spread_argument: ($) => seq('...', $._expression),

    field_expression: ($) =>
      prec(
        PREC.postfix,
        seq(field('object', $._expression), '.', field('field', $.identifier)),
      ),

    // `pair.0` — `.` followed by an integer.
    tuple_index_expression: ($) =>
      prec(
        PREC.postfix,
        seq(field('object', $._expression), '.', field('index', $.integer)),
      ),

    index_expression: ($) =>
      prec(
        PREC.postfix,
        seq(
          field('object', $._expression),
          '[',
          field('index', $._expression),
          ']',
        ),
      ),

    if_expression: ($) =>
      prec.right(
        seq(
          'if',
          field('condition', $._expression_no_brace),
          'then',
          field('consequence', $._expression),
          'else',
          field('alternative', $._expression),
        ),
      ),

    // Same node, but the `else` branch cannot be (or end in) a `{…}`, so a
    // following `{` reliably opens a block. Needed for `on if c then a else b {`
    // — `looks_like_expr_trigger` sends a `if`-led trigger through `parse_expr`.
    // The alternative recurses into this variant so `else if … then … else …`
    // chains still work.
    _if_expression_no_brace: ($) =>
      alias(
        prec.right(
          seq(
            'if',
            field('condition', $._expression_no_brace),
            'then',
            field('consequence', $._expression),
            'else',
            field(
              'alternative',
              choice($._expression_no_brace, $._if_expression_no_brace),
            ),
          ),
        ),
        $.if_expression,
      ),

    // `{ stmt* value }`
    block_expression: ($) =>
      seq('{', repeat($._block_statement), field('value', $._expression), '}'),

    // `{ a: 1, b, ...rest }`. `{}` cannot be a `block_expression` (which needs
    // a trailing value), and `{ a }` is disambiguated by `shorthand_field`.
    record_literal: ($) =>
      seq(
        '{',
        commaSepTrailing(
          choice($.record_field, $.shorthand_field, $.spread_field),
        ),
        '}',
      ),

    record_field: ($) =>
      seq(field('name', $.identifier), ':', field('value', $._expression)),

    // `{ x }` is a shorthand record field, not a block expression whose value
    // is `x` — this mirrors `looks_like_record_lit` in the reference parser.
    // Only reachable with `,`/`}` lookahead, so `{ x + 1 }` stays a block.
    shorthand_field: ($) => prec(1, field('name', $.identifier)),

    spread_field: ($) => seq('...', field('value', $._expression)),

    array_literal: ($) =>
      seq('[', commaSepTrailing(choice($.spread_element, $._expression)), ']'),

    spread_element: ($) => seq('...', $._expression),

    // ---------------------------------------------------------------- literals

    identifier: (_$) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    boolean: (_$) => choice('true', 'false'),

    integer: (_$) =>
      token(
        choice(
          /0[xX][0-9a-fA-F_]+/,
          /0[bB][01_]+/,
          /0[oO][0-7_]+/,
          /[0-9][0-9_]*/,
        ),
      ),

    float: (_$) =>
      token(
        choice(
          seq(
            /[0-9][0-9_]*/,
            '.',
            /[0-9][0-9_]*/,
            optional(seq(/[eE]/, optional(/[+-]/), /[0-9]+/)),
          ),
          seq(/[0-9][0-9_]*/, /[eE]/, optional(/[+-]/), /[0-9]+/),
        ),
      ),

    // `$AssetType/AssetName`
    asset_reference: (_$) =>
      token(seq('$', /[a-zA-Z_][a-zA-Z0-9_\-.]*\/[a-zA-Z0-9_\-./]*/)),

    // `$./rel/file.brz` or `$/abs/file.brz`
    prefab_reference: (_$) => token(seq('$', /[./][a-zA-Z0-9_\-./]*/)),

    string: ($) =>
      choice(
        seq(
          '"',
          repeat(
            choice(
              alias($._double_quoted_fragment, $.string_fragment),
              alias(token.immediate('$'), $.string_fragment),
              $.escape_sequence,
              $.interpolation,
            ),
          ),
          '"',
        ),
        seq(
          "'",
          repeat(
            choice(
              alias($._single_quoted_fragment, $.string_fragment),
              alias(token.immediate('$'), $.string_fragment),
              $.escape_sequence,
              $.interpolation,
            ),
          ),
          "'",
        ),
      ),

    _double_quoted_fragment: (_$) => token.immediate(prec(1, /[^"\\$\n]+/)),
    _single_quoted_fragment: (_$) => token.immediate(prec(1, /[^'\\$\n]+/)),

    escape_sequence: (_$) => token.immediate(seq('\\', /./)),

    // `${ expr }` — the reference lexer captures the raw slice and re-parses it.
    interpolation: ($) => seq(token.immediate('${'), $._expression, '}'),
  },
});

function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)));
}

function commaSepTrailing(rule) {
  return optional(seq(commaSep1(rule), optional(',')));
}
