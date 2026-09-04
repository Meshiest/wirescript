//! The Pratt expression core, every literal form, and the record/map/block
//! `{`-disambiguation.

use super::*;

/// Higher number = tighter binding. Mirrors the TS table.
fn infix_prec(op: &str) -> Option<u8> {
    match op {
        "||" | "^^" => Some(2),
        "&&" => Some(3),
        "|" => Some(4),
        "^" => Some(5),
        "&" => Some(6),
        // `is` (the enum variant test) reads as a comparison, so it binds like
        // one. Only the contextual check in `parse_binary` ever passes it here;
        // an operator token can never spell `is`.
        "==" | "!=" | "is" => Some(7),
        "<" | "<=" | ">" | ">=" => Some(8),
        "<<" | ">>" => Some(9),
        "+" | "-" | ".." => Some(10),
        "*" | "/" | "%" => Some(11),
        "**" => Some(12),
        _ => None,
    }
}

fn is_right_assoc(op: &str) -> bool {
    op == "**"
}

fn is_prefix_op(op: &str) -> bool {
    matches!(op, "-" | "!" | "~" | "*" | "&")
}

/// Parse an integer literal's (underscore-stripped) text as an `i64`. Returns
/// `None` when the value is out of range for a 64-bit int — the caller reports a
/// diagnostic rather than silently baking `0` (an out-of-range user constant
/// reading `0` with no error is exactly the class of bug this project fights).
fn parse_int_literal(cleaned: &str) -> Option<i64> {
    if let Some(hex) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16).ok()
    } else if let Some(bin) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        i64::from_str_radix(bin, 2).ok()
    } else if let Some(oct) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        i64::from_str_radix(oct, 8).ok()
    } else {
        cleaned.parse().ok()
    }
}

impl<'a> Parser<'a> {
    pub(super) fn parse_expr(&mut self) -> Expr {
        self.parse_binary(0)
    }

    /// Parse an expression as a header/condition: a trailing `{ ... }` after a
    /// path is treated as a block body, not braced variant construction. See
    /// `Parser::no_brace_construct`.
    pub(super) fn parse_expr_no_brace_construct(&mut self) -> Expr {
        let saved = std::mem::replace(&mut self.no_brace_construct, true);
        let e = self.parse_expr();
        self.no_brace_construct = saved;
        e
    }

    /// Parse an expression inside a bracketed delimiter (`( )`, `[ ]`, call
    /// args, a record/map/array body), where a trailing `{` is unambiguous, so
    /// braced construction is re-enabled regardless of any enclosing header.
    fn parse_expr_delimited(&mut self) -> Expr {
        let saved = std::mem::replace(&mut self.no_brace_construct, false);
        let e = self.parse_expr();
        self.no_brace_construct = saved;
        e
    }

    /// [`parse_call_arg`] wrapped like [`parse_expr_delimited`] - a call's
    /// argument list is a bracketed context.
    fn parse_call_arg_delimited(&mut self) -> CallArg {
        let saved = std::mem::replace(&mut self.no_brace_construct, false);
        let a = self.parse_call_arg();
        self.no_brace_construct = saved;
        a
    }

    /// `match <scrutinee> { <pattern> => <body>, ... }`. Shared by the
    /// primary-expression form (`parse_primary`) and the statement form
    /// (`parse_stmt`), which both produce the same `Expr::MatchExpr`. The
    /// scrutinee is a header position like `if`'s condition: it is parsed
    /// with `parse_expr_no_brace_construct` so a trailing `{` always opens
    /// the match body rather than being stolen as braced variant
    /// construction (`match obj.field { ... }` must not misparse `obj.field
    /// { ... }` as a `VariantCtor`).
    pub(super) fn parse_match_expr(&mut self) -> Expr {
        let start = self.expect(TokenKind::Kw, Some("match")).start;
        let scrutinee = self.parse_expr_no_brace_construct();
        self.eat_newlines();
        self.expect(TokenKind::LBrace, None);
        self.eat_newlines();
        let mut arms: Vec<MatchArm> = Vec::new();
        while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
            let arm_start = self.peek().start;
            let pattern = self.parse_pattern();
            self.eat_newlines();
            self.expect(TokenKind::FatArrow, None);
            self.eat_newlines();
            let is_block = self.check(TokenKind::LBrace, None);
            let body = if is_block {
                MatchBody::Block(self.parse_block())
            } else {
                MatchBody::Expr(self.parse_expr_delimited())
            };
            let body_end = match &body {
                MatchBody::Expr(e) => e.range().end,
                MatchBody::Block(b) => b.range.end,
            };
            arms.push(MatchArm {
                pattern,
                body,
                range: self.make_range(arm_start, body_end),
            });
            self.eat_newlines();
            if is_block {
                // Block-body arms may omit the separating comma, mirroring
                // statement lists inside an `if` block.
                self.match_tok(TokenKind::Comma, None);
                self.eat_newlines();
            } else if self.match_tok(TokenKind::Comma, None).is_none() {
                self.eat_newlines();
                break;
            } else {
                self.eat_newlines();
            }
        }
        let end = self.expect(TokenKind::RBrace, None).end;
        Expr::MatchExpr {
            scrutinee: Box::new(scrutinee),
            arms,
            range: self.make_range(start, end),
        }
    }

    fn parse_binary(&mut self, min_prec: u8) -> Expr {
        let mut lhs = self.parse_prefix();
        loop {
            // Skip newlines to allow line continuation after operators:
            //   let x = a +
            //     b + c
            let saved = self.pos;
            while self.peek().kind == TokenKind::Newline {
                self.advance();
            }
            let tok = self.peek().clone();
            // `value is Enum.Variant` is a word-shaped infix operator.
            // Contextual, like `enum` and `unsafe`: it only reads as the
            // operator when a name follows it, so a variable spelled `is`
            // stays usable (`is = is + 1`, `is(2)`, `is + 1`).
            let variant_test = tok.kind == TokenKind::Ident
                && tok.text == "is"
                && self.peek_at(1).kind == TokenKind::Ident;
            if tok.kind != TokenKind::Op && !variant_test {
                self.pos = saved;
                break;
            }
            let Some(prec) = infix_prec(&tok.text) else {
                self.pos = saved;
                break;
            };
            if prec < min_prec {
                self.pos = saved;
                break;
            }
            self.advance();
            // Also skip newlines after the operator
            while self.peek().kind == TokenKind::Newline {
                self.advance();
            }
            // The right side of `is` is the variant path alone: taking only a
            // postfix expression keeps `e is E.A && b` grouping as
            // `(e is E.A) && b`, and leaves a non-path right side to
            // typecheck's WS066 rather than swallowing the rest of the line.
            if variant_test {
                let path = self.parse_postfix();
                let start = lhs.range().start;
                let end = path.range().end;
                lhs = Expr::Is {
                    value: Box::new(lhs),
                    path: Box::new(path),
                    range: self.make_range(start, end),
                };
                continue;
            }
            let next_min = if is_right_assoc(&tok.text) {
                prec
            } else {
                prec + 1
            };
            let rhs = self.parse_binary(next_min);
            let start = lhs.range().start;
            let end = rhs.range().end;
            lhs = Expr::BinOp {
                op: tok.text,
                left: Box::new(lhs),
                right: Box::new(rhs),
                range: self.make_range(start, end),
            };
        }
        lhs
    }

    fn parse_prefix(&mut self) -> Expr {
        let t = self.peek().clone();
        // `unsafe <value>.<Variant>.<field>`, an unchecked payload access.
        // Contextual, like `enum`: it opens the form only when an identifier
        // follows, so `unsafe` stays usable as an ordinary name (`unsafe + 1`,
        // `unsafe(x)`, a bare read) rather than becoming a reserved word.
        if t.kind == TokenKind::Ident
            && t.text == "unsafe"
            && self.peek_at(1).kind == TokenKind::Ident
        {
            self.advance();
            let inner = self.parse_postfix();
            let end = inner.range().end;
            return Expr::Unsafe {
                inner: Box::new(inner),
                range: self.make_range(t.start, end),
            };
        }
        if t.kind == TokenKind::Op && is_prefix_op(&t.text) {
            // Fold `-<number>` into a negative literal at parse time.
            if t.text == "-" {
                let next = self.peek_at(1);
                if next.kind == TokenKind::Int {
                    self.advance();
                    let num = self.advance();
                    let cleaned: String = num.text.chars().filter(|c| *c != '_').collect();
                    // Prefer negating the parsed magnitude; fall back to parsing
                    // the whole "-<n>" so i64::MIN (whose positive magnitude
                    // overflows i64) still folds. A genuine overflow reports a
                    // diagnostic instead of baking `0`.
                    let value = match parse_int_literal(&cleaned) {
                        Some(v) => Some(-v),
                        None => format!("-{cleaned}").parse::<i64>().ok(),
                    };
                    let value = match value {
                        Some(v) => v,
                        None => {
                            self.error(
                                format!(
                                    "integer literal '-{}' is out of range for a 64-bit int",
                                    num.text
                                ),
                                t.start,
                                num.end,
                            );
                            0
                        }
                    };
                    return Expr::IntLit {
                        value,
                        text: format!("-{}", num.text),
                        range: self.make_range(t.start, num.end),
                    };
                } else if next.kind == TokenKind::Float {
                    self.advance();
                    let num = self.advance();
                    let val: f64 = num.text.parse().unwrap_or(0.0);
                    return Expr::FloatLit {
                        value: -val,
                        text: format!("-{}", num.text),
                        range: self.make_range(t.start, num.end),
                    };
                }
            }
            self.advance();
            let operand = self.parse_prefix();
            let end = operand.range().end;
            if t.text == "*" {
                return Expr::Deref {
                    operand: Box::new(operand),
                    range: self.make_range(t.start, end),
                };
            }
            if t.text == "&" {
                return Expr::RefOf {
                    operand: Box::new(operand),
                    range: self.make_range(t.start, end),
                };
            }
            return Expr::UnOp {
                op: t.text,
                operand: Box::new(operand),
                range: self.make_range(t.start, end),
            };
        }
        if t.kind == TokenKind::Kw && t.text == "ref" {
            self.advance();
            let operand = self.parse_prefix();
            let end = operand.range().end;
            return Expr::RefOf {
                operand: Box::new(operand),
                range: self.make_range(t.start, end),
            };
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut e = self.parse_primary();
        loop {
            // A method/field chain may continue on a following line:
            //   let e = SpawnPrefab(...)
            //     .SendCustomEvent("init", p)
            // Skip the intervening newline(s) ONLY when the next real token is
            // `.` (a leading-dot continuation) — an ordinary line break still
            // ends the expression. A statement can never legally begin with `.`,
            // so this only accepts input that was previously a parse error.
            if self.peek().kind == TokenKind::Newline {
                let mut i = 1;
                while self.peek_at(i).kind == TokenKind::Newline {
                    i += 1;
                }
                if self.peek_at(i).kind == TokenKind::Dot {
                    self.eat_newlines();
                }
            }
            let t = self.peek().clone();
            if t.kind == TokenKind::Dot {
                self.advance();
                // `.name` or `.<int>` for tuple pick.
                let peek_kind = self.peek().kind;
                if peek_kind == TokenKind::Int {
                    let idx_tok = self.advance();
                    let idx: usize = idx_tok.text.parse().unwrap_or(0);
                    let start = e.range().start;
                    e = Expr::TuplePick {
                        obj: Box::new(e),
                        index: idx,
                        range: self.make_range(start, idx_tok.end),
                    };
                } else {
                    let field_tok = self.expect(TokenKind::Ident, None);
                    let start = e.range().start;
                    e = Expr::FieldAccess {
                        obj: Box::new(e),
                        field: field_tok.text,
                        range: self.make_range(start, field_tok.end),
                    };
                }
                continue;
            }
            // Braced-named enum payload construction: `Enum.Variant { f: v, ... }`.
            // COMMITS whenever the path so far is a `FieldAccess` (`A.B`), the
            // next token is `{` on the same logical line, and we are NOT parsing
            // a header/condition (`no_brace_construct`). `no_brace_construct`
            // alone disambiguates `if a.b { ... }` (the body block) from
            // construction; outside a header the trailing `{` after `A.B` has no
            // other meaning, so we commit rather than leave it. Committing is the
            // point: if we only took a well-formed record body and let anything
            // else fall through, the generic top-level `ExprStmt` fallback would
            // silently re-parse the leftover `{ ... }` as a separate declaration
            // - a silent split that drops the whole body (`S.Circle { 5.0 }`,
            // `E.V { let x = 1 }`, `A.B { call() }`). A bare `Ident { ... }` is
            // still left alone (`matches!(e, FieldAccess)`), and a `{` on the
            // NEXT line never reaches here (the postfix loop only skips a leading
            // newline before a `.`), so a following block still starts its own
            // statement.
            if t.kind == TokenKind::LBrace
                && !self.no_brace_construct
                && matches!(e, Expr::FieldAccess { .. })
            {
                let start = e.range().start;
                let (fields, end) = if self.looks_like_record_lit() {
                    // A well-formed record body (`{ name: v }` / shorthand /
                    // spread / empty `{}`): parse the fields as usual.
                    match self.parse_record_lit() {
                        Expr::RecordLit { fields, range } => (fields, range.end),
                        _ => unreachable!("parse_record_lit always returns Expr::RecordLit"),
                    }
                } else {
                    // A malformed body (`{ 5.0 }`, `{ let x = 1 }`, `{ call() }`):
                    // still commit - emit a parse diagnostic and consume the
                    // balanced braces so nothing is left for the decl fallback to
                    // silently split off. Recovers to a zero-field VariantCtor
                    // (typecheck then reports the payload-shape error too).
                    let brace_start = self.peek().start;
                    let end = self.consume_balanced_braces();
                    self.error(
                        "braced construction expects named fields `{ name: value }`; a \
                         positional variant is called with parentheses, e.g. `S.Circle(5.0)`",
                        brace_start,
                        end,
                    );
                    (Vec::new(), end)
                };
                e = Expr::VariantCtor {
                    path: Box::new(e),
                    fields,
                    range: self.make_range(start, end),
                };
                continue;
            }
            if t.kind == TokenKind::LBracket {
                self.advance();
                let idx = self.parse_expr_delimited();
                let end = self.expect(TokenKind::RBracket, None).end;
                let start = e.range().start;
                e = Expr::IndexAccess {
                    obj: Box::new(e),
                    index: Box::new(idx),
                    range: self.make_range(start, end),
                };
                continue;
            }
            if t.kind == TokenKind::LParen {
                self.advance();
                let mut args: Vec<CallArg> = Vec::new();
                self.eat_newlines();
                while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                    args.push(self.parse_call_arg_delimited());
                    self.eat_newlines();
                    if self.match_tok(TokenKind::Comma, None).is_none() {
                        self.eat_newlines();
                        break;
                    }
                    self.eat_newlines();
                }
                let end = self.expect(TokenKind::RParen, None).end;
                let start = e.range().start;
                e = desugar_gate_call(Box::new(e), args, Vec::new(), self.make_range(start, end));
                continue;
            }
            // Explicit type arguments: `callee<Type, ...>(args)` for a generic
            // mod/chip. `<` is otherwise a comparison operator, so only commit
            // when the `<...>` parses as a type-argument list AND is immediately
            // followed by `(` (which a `<`/`>` comparison never is). Otherwise
            // fully backtrack — position AND any speculative diagnostics — and
            // let `<` fall through to the comparison parser.
            if t.kind == TokenKind::Op && t.text == "<" {
                let save_pos = self.pos;
                let save_diag = self.diagnostics.len();
                if let Some(type_args) = self.try_parse_type_args()
                    && self.check(TokenKind::LParen, None)
                {
                    self.advance();
                    let mut args: Vec<CallArg> = Vec::new();
                    self.eat_newlines();
                    while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                        args.push(self.parse_call_arg_delimited());
                        self.eat_newlines();
                        if self.match_tok(TokenKind::Comma, None).is_none() {
                            self.eat_newlines();
                            break;
                        }
                        self.eat_newlines();
                    }
                    let end = self.expect(TokenKind::RParen, None).end;
                    let start = e.range().start;
                    e = Expr::Call {
                        callee: Box::new(e),
                        args,
                        type_args,
                        range: self.make_range(start, end),
                    };
                    continue;
                }
                self.pos = save_pos;
                self.diagnostics.truncate(save_diag);
                break;
            }
            break;
        }
        e
    }

    /// Speculatively parse an explicit type-argument list `< Type (, Type)* >`
    /// at a call site. Returns `None` (leaving diagnostics for the caller to
    /// truncate) when the tokens after `<` don't form a type list. Does NOT
    /// consume the following `(` — the caller checks for it to disambiguate real
    /// type arguments from a `<`/`>` comparison.
    fn try_parse_type_args(&mut self) -> Option<Vec<TypeExpr>> {
        self.match_tok(TokenKind::Op, Some("<"))?;
        let mut args: Vec<TypeExpr> = Vec::new();
        loop {
            // A type argument must begin with an identifier (a primitive, type
            // param, alias, or `Generic<...>`); anything else means this `<` was
            // a comparison, not a type-argument list.
            if self.peek().kind != TokenKind::Ident {
                return None;
            }
            args.push(self.parse_type());
            if self.match_tok(TokenKind::Comma, None).is_some() {
                continue;
            }
            break;
        }
        self.match_tok(TokenKind::Op, Some(">"))?;
        if args.is_empty() {
            return None;
        }
        Some(args)
    }

    fn parse_call_arg(&mut self) -> CallArg {
        // `...expr` (spread)
        if self.check(TokenKind::Op, Some("...")) {
            self.advance();
            let value = self.parse_expr();
            return CallArg::Spread(value);
        }
        // `name = value` (kwarg) vs bare expression.
        if self.peek().kind == TokenKind::Ident
            && self.peek_at(1).kind == TokenKind::Op
            && self.peek_at(1).text == "="
        {
            let name_tok = self.advance();
            let name_range = self.make_range(name_tok.start, name_tok.end);
            let name = name_tok.text;
            self.advance();
            let value = self.parse_expr();
            CallArg::Named {
                name,
                value,
                name_range,
            }
        } else {
            CallArg::Positional(self.parse_expr())
        }
    }

    fn parse_primary(&mut self) -> Expr {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Int => {
                self.advance();
                let text = t.text.clone();
                let cleaned: String = text.chars().filter(|c| *c != '_').collect();
                let value = match parse_int_literal(&cleaned) {
                    Some(v) => v,
                    None => {
                        self.error(
                            format!("integer literal '{text}' is out of range for a 64-bit int"),
                            t.start,
                            t.end,
                        );
                        0
                    }
                };
                Expr::IntLit {
                    value,
                    text,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::Float => {
                self.advance();
                let text = t.text.clone();
                let cleaned: String = text.chars().filter(|c| *c != '_').collect();
                let value: f64 = cleaned.parse().unwrap_or(0.0);
                Expr::FloatLit {
                    value,
                    text,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::Str => {
                self.advance();
                let value = match t.value {
                    Some(TokenValue::Str(s)) => s,
                    _ => String::new(),
                };
                Expr::StringLit {
                    value,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::AssetRef => {
                self.advance();
                let path = match t.value {
                    Some(TokenValue::Str(s)) => s,
                    _ => String::new(),
                };
                let range = self.make_range(t.start, t.end);
                // A leading `.` or `/` marks a prefab FILE reference
                // (`$./rel.brz`, `$/abs.brz`); otherwise it's a `$Type/Name`
                // external asset reference split on the first `/`.
                if path.starts_with('.') || path.starts_with('/') {
                    Expr::PrefabRef { path, range }
                } else {
                    let (asset_type, asset_name) = match path.split_once('/') {
                        Some((ty, name)) => (ty.to_string(), name.to_string()),
                        None => {
                            self.error(
                                String::from(
                                    "asset reference must be `$AssetType/AssetName` or a prefab path `$./file.brz` / `$./file.ws`",
                                ),
                                t.start,
                                t.end,
                            );
                            (path.clone(), String::new())
                        }
                    };
                    Expr::AssetRef {
                        asset_type,
                        asset_name,
                        range,
                    }
                }
            }
            TokenKind::NestedPrefab => {
                self.advance();
                let source = match t.value {
                    Some(TokenValue::Str(s)) => s,
                    _ => String::new(),
                };
                let range = self.make_range(t.start, t.end);
                Expr::NestedPrefab { source, range }
            }
            TokenKind::LBracket => {
                let start = t.start;
                self.advance();
                let mut elements = Vec::new();
                self.eat_newlines();
                while !self.check(TokenKind::RBracket, None) && self.peek().kind != TokenKind::Eof {
                    // `...expr` spreads another array's elements in place.
                    if self.check(TokenKind::Op, Some("...")) {
                        self.advance();
                        elements.push(ArrayElem::Spread(self.parse_expr_delimited()));
                    } else {
                        elements.push(ArrayElem::Item(self.parse_expr_delimited()));
                    }
                    self.eat_newlines();
                    if self.match_tok(TokenKind::Comma, None).is_none() {
                        self.eat_newlines();
                        break;
                    }
                    self.eat_newlines();
                }
                let end = self.expect(TokenKind::RBracket, None).end;
                Expr::Array {
                    elements,
                    range: self.make_range(start, end),
                }
            }
            TokenKind::StrInterp => {
                self.advance();
                let parts_raw = match t.value {
                    Some(TokenValue::Interp(p)) => p,
                    _ => Vec::new(),
                };
                let parts = parts_raw
                    .into_iter()
                    .map(|p| match p {
                        LexInterpPart::Lit(s) => InterpPart::Lit(s),
                        LexInterpPart::Expr {
                            source,
                            start: expr_origin,
                            end: _,
                        } => {
                            // Parse the `${...}` body as an EXPRESSION via
                            // `parse_expr`. A module parse only yields an
                            // `ExprStmt` for a bare expression statement, so an
                            // interpolated `if ... then ... else` (a statement
                            // `if`, or unparseable) produces no expression and
                            // the slot renders empty. `parse_expr` also surfaces
                            // parse errors inside the `${...}`.
                            let lexed = crate::lexer::lex(&source, self.file);
                            let mut sub = Parser::new(lexed.tokens, self.file, lexed.diagnostics);
                            let mut expr = sub.parse_expr();
                            self.diagnostics.extend(sub.diagnostics);
                            shift_expr_offsets(&mut expr, expr_origin);
                            InterpPart::Expr(Box::new(expr))
                        }
                    })
                    .collect();
                Expr::InterpLit {
                    parts,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::Kw if t.text == "true" || t.text == "false" => {
                self.advance();
                Expr::BoolLit {
                    value: t.text == "true",
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::Kw if t.text == "null" => {
                self.advance();
                Expr::NullLit {
                    range: self.make_range(t.start, t.end),
                }
            }
            // `emit` names the current exec chain point. A dual-role keyword,
            // like `if` and `match`: statement dispatch claims `emit NAME`
            // first, so reaching `parse_primary` means an expression was
            // expected and only the bare atom can appear.
            TokenKind::Kw if t.text == "emit" => {
                self.advance();
                Expr::CurrentExec {
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::Kw if t.text == "if" => {
                self.advance();
                let cond = self.parse_expr();
                self.eat_newlines();
                self.expect(TokenKind::Kw, Some("then"));
                let then_e = self.parse_expr();
                self.eat_newlines();
                self.expect(TokenKind::Kw, Some("else"));
                let else_e = self.parse_expr();
                let end = else_e.range().end;
                Expr::IfExpr {
                    cond: Box::new(cond),
                    then_branch: Box::new(then_e),
                    else_branch: Box::new(else_e),
                    range: self.make_range(t.start, end),
                }
            }
            TokenKind::Kw if t.text == "match" => self.parse_match_expr(),
            TokenKind::Atom => {
                self.advance();
                let name = match t.value {
                    Some(TokenValue::Str(s)) => s,
                    _ => String::new(),
                };
                let value = crate::hash::atom_hash(&name);
                Expr::AtomLit {
                    name,
                    value,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::Ident => {
                self.advance();
                Expr::Ident {
                    name: t.text,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::LParen => {
                self.advance();
                let e = self.parse_expr_delimited();
                if self.check(TokenKind::Comma, None) {
                    // Tuple literal: (expr, expr, ...)
                    let mut elements = vec![e];
                    while self.match_tok(TokenKind::Comma, None).is_some() {
                        if self.check(TokenKind::RParen, None) {
                            break;
                        }
                        elements.push(self.parse_expr_delimited());
                    }
                    let end = self.expect(TokenKind::RParen, None);
                    // Desugar to a record literal with numeric field names —
                    // tuples are handled via the chip output system.
                    let fields: Vec<crate::ast::RecordLitField> = elements
                        .into_iter()
                        .enumerate()
                        .map(|(i, expr)| {
                            let range = expr.range().clone();
                            crate::ast::RecordLitField::Named {
                                name: i.to_string(),
                                value: expr,
                                range,
                            }
                        })
                        .collect();
                    Expr::RecordLit {
                        fields,
                        range: self.make_range(t.start, end.end),
                    }
                } else {
                    self.expect(TokenKind::RParen, None);
                    e
                }
            }
            TokenKind::LBrace => {
                // Check the O(1) record test first — a record (`ident:` /
                // `ident,` / `ident}` / `...spread` / `{}`) and a map
                // (top-level `=>`, or a literal/computed key with `:`) are
                // mutually exclusive at this position, so whichever we probe
                // first can short-circuit the other. Record is cheap and
                // covers the common block-expr-vs-record case without ever
                // touching the (potentially large) braced-region map scan.
                if self.looks_like_record_lit() {
                    self.parse_record_lit()
                } else if self.looks_like_map_lit() {
                    self.parse_map_lit()
                } else {
                    self.parse_block_expr()
                }
            }
            _ => {
                self.error(
                    format!("unexpected token '{}' in expression", t.text),
                    t.start,
                    t.end,
                );
                self.advance();
                Expr::Ident {
                    name: String::new(),
                    range: self.make_range(t.start, t.end),
                }
            }
        }
    }

    /// A `{ … }` is a map literal when it has a top-level `=>`, or a first key
    /// that is a string / atom / int literal followed by `:`, or a bracketed
    /// `[ … ]:` computed key.
    fn looks_like_map_lit(&self) -> bool {
        let get = |idx: usize| -> &Token {
            self.tokens.get(idx).unwrap_or_else(|| self.tokens.last().unwrap())
        };
        let mut i = self.pos + 1; // after `{`
        while get(i).kind == TokenKind::Newline {
            i += 1;
        }
        let first = get(i);
        // First-token precheck — a REJECT-list, not an allow-list. A map key
        // is `self.parse_expr()` (any expression: `-1`, `f()`, `(1)`, `true`,
        // `base + 1`, an asset ref, …), so an allow-list of "map-entry
        // openers" can never be complete and would hard-reject valid maps.
        // Instead: the call site already ruled out records (record-first
        // check), so here we only separate a MAP from a BLOCK-EXPR. In
        // expression position a block-expr can only be *led* by a statement,
        // and `parse_block_expr` dispatches exactly `let` / `var` / `static`
        // to `parse_stmt` — every other leading token it parses as an
        // expression (block-style `if x { … }` is a statement form NOT
        // reachable here; expression-position `if` is the `if…then…else`
        // expression, itself a valid map key). Those three keywords cannot
        // begin a key expression, so a `{` opening with one is unambiguously
        // a block, never a map → fast-reject O(1) without the scan. An empty
        // `{}` is a record (handled at the call site) and not a map either.
        // EVERYTHING ELSE falls through to the bounded depth-0 `=>` scan
        // below, which is the sound test: a real map has a top-level `=>` (or
        // a literal/computed `:` key, checked after), a keyword-less
        // block-expr does not. This preserves exact map/record/block
        // classification while keeping the wins (records → record-first O(1);
        // `let`/`var`/`static` blocks → O(1) reject; unbalanced braces →
        // bounded scan).
        if first.kind == TokenKind::RBrace {
            return false; // empty `{}` — not a map (defensive; call site routes it to record)
        }
        if first.kind == TokenKind::Kw
            && matches!(first.text.as_str(), "let" | "var" | "static")
        {
            return false; // block-expr statement leader — never a map key
        }
        // Scan to the matching `}` at brace depth 0; a top-level `=>` ⇒ map.
        // Bounded: an unbalanced expression-position `{` (common mid-edit in
        // the LSP) must not scan the rest of the file token-by-token every
        // time this runs — give up (not a map) past a generous token budget
        // instead of relying on `Eof` as the only terminator.
        const MAX_SCAN_TOKENS: usize = 8192;
        let mut depth = 1i32;
        let mut j = i;
        let scan_limit = self.tokens.len().min(i.saturating_add(MAX_SCAN_TOKENS));
        while j < scan_limit {
            match get(j).kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::FatArrow if depth == 1 => return true,
                TokenKind::Eof => break,
                _ => {}
            }
            j += 1;
        }
        // Colon literal-key form: first token is a str/atom/int then `:`.
        if matches!(first.kind, TokenKind::Str | TokenKind::StrInterp | TokenKind::Atom | TokenKind::Int) {
            let mut k = i + 1;
            while get(k).kind == TokenKind::Newline {
                k += 1;
            }
            if get(k).kind == TokenKind::Colon {
                return true;
            }
        }
        // Computed-key form: `[ … ] :`.
        if first.kind == TokenKind::LBracket {
            let mut d = 1i32;
            let mut k = i + 1;
            while k < self.tokens.len() {
                match get(k).kind {
                    TokenKind::LBracket => d += 1,
                    TokenKind::RBracket => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    TokenKind::Eof => return false,
                    _ => {}
                }
                k += 1;
            }
            k += 1;
            while get(k).kind == TokenKind::Newline {
                k += 1;
            }
            if get(k).kind == TokenKind::Colon {
                return true;
            }
        }
        false
    }

    /// Parse a map literal. Entries are `key => value`, or (for str/atom/int
    /// literal keys and `[expr]` computed keys) `key : value`.
    fn parse_map_lit(&mut self) -> Expr {
        let start = self.expect(TokenKind::LBrace, None).start;
        let mut entries: Vec<MapLitEntry> = Vec::new();
        self.eat_newlines();
        while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
            // `[expr]` computed key, else a normal expression key.
            let key = if self.check(TokenKind::LBracket, None) {
                self.advance();
                let k = self.parse_expr_delimited();
                self.expect(TokenKind::RBracket, None);
                k
            } else {
                self.parse_expr_delimited()
            };
            // Separator: `=>` or `:`.
            if self.match_tok(TokenKind::FatArrow, None).is_none() {
                self.expect(TokenKind::Colon, None);
            }
            let value = self.parse_expr_delimited();
            let range = self.make_range(key.range().start, value.range().end);
            entries.push(MapLitEntry { key, value, range });
            self.eat_newlines();
            if self.match_tok(TokenKind::Comma, None).is_none() {
                self.eat_newlines();
                break;
            }
            self.eat_newlines();
        }
        let end = self.expect(TokenKind::RBrace, None).end;
        Expr::MapLit {
            entries,
            range: self.make_range(start, end),
        }
    }

    /// Peek ahead after `{` to decide if this is a record literal or a block expression.
    ///
    /// Record literal when next tokens are:
    /// - `ident :` (named field)
    /// - `ident ,` or `ident }` (shorthand)
    /// - `...` (spread)
    /// - `}` (empty record)
    fn looks_like_record_lit(&self) -> bool {
        // Current token is `{`.
        let after_brace = self.pos + 1;
        let get = |idx: usize| -> &Token {
            self.tokens
                .get(idx)
                .unwrap_or_else(|| self.tokens.last().unwrap())
        };
        let mut i = after_brace;
        while i < self.tokens.len() && get(i).kind == TokenKind::Newline {
            i += 1;
        }
        let first = get(i);
        // Empty record `{}`
        if first.kind == TokenKind::RBrace {
            return true;
        }
        // Spread `{ ...expr }`
        if first.kind == TokenKind::Op && first.text == "..." {
            return true;
        }
        // `{ ident : ...` or `{ ident , ...` or `{ ident }`
        if first.kind == TokenKind::Ident {
            let mut j = i + 1;
            while j < self.tokens.len() && get(j).kind == TokenKind::Newline {
                j += 1;
            }
            let after_ident = get(j);
            if after_ident.kind == TokenKind::Colon
                || after_ident.kind == TokenKind::Comma
                || after_ident.kind == TokenKind::RBrace
            {
                return true;
            }
        }
        false
    }

    /// Parse a record literal: `{ field: expr, shorthand, ...spread }`
    fn parse_record_lit(&mut self) -> Expr {
        let start = self.expect(TokenKind::LBrace, None).start;
        let mut fields: Vec<RecordLitField> = Vec::new();
        self.eat_newlines();
        while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
            // `...expr`
            if self.check(TokenKind::Op, Some("...")) {
                let spread_start = self.advance().start;
                let value = self.parse_expr_delimited();
                let spread_end = value.range().end;
                fields.push(RecordLitField::Spread {
                    value,
                    range: self.make_range(spread_start, spread_end),
                });
            } else {
                let name_tok = self.expect(TokenKind::Ident, None);
                if self.match_tok(TokenKind::Colon, None).is_some() {
                    // Named field: `name: expr`
                    let value = self.parse_expr_delimited();
                    let field_end = value.range().end;
                    fields.push(RecordLitField::Named {
                        name: name_tok.text,
                        value,
                        range: self.make_range(name_tok.start, field_end),
                    });
                } else {
                    // Shorthand: `name`
                    fields.push(RecordLitField::Shorthand {
                        name: name_tok.text.clone(),
                        range: self.make_range(name_tok.start, name_tok.end),
                    });
                }
            }
            self.eat_newlines();
            if self.match_tok(TokenKind::Comma, None).is_none() {
                self.eat_newlines();
                break;
            }
            self.eat_newlines();
        }
        let end = self.expect(TokenKind::RBrace, None).end;
        Expr::RecordLit {
            fields,
            range: self.make_range(start, end),
        }
    }

    /// Parse `{ stmt*; expr }` — a block expression whose value is its last expression.
    fn parse_block_expr(&mut self) -> Expr {
        let start = self.expect(TokenKind::LBrace, None).start;
        let mut stmts = Vec::new();
        self.eat_newlines();

        loop {
            self.eat_newlines();
            if self.check(TokenKind::RBrace, None) || self.peek().kind == TokenKind::Eof {
                break;
            }
            // `let`/`var`/`static` parse as statements; everything else falls
            // through as an expression below. `emit` is a statement leader only
            // when a target name follows it. A bare `emit` is the current-exec
            // atom, which belongs to the expression path.
            let is_stmt_kw = self.peek().kind == TokenKind::Kw
                && (matches!(self.peek().text.as_str(), "let" | "var" | "static")
                    || (self.peek().text == "emit" && self.peek_at(1).kind == TokenKind::Ident));
            if is_stmt_kw {
                if let Some(s) = self.parse_stmt() {
                    stmts.push(s);
                }
                continue;
            }
            // Otherwise parse as an expression — could be the final value
            // or an assignment statement
            let expr = self.parse_expr();
            self.eat_newlines();
            if self.match_tok(TokenKind::Op, Some("=")).is_some() {
                let value = self.parse_expr();
                let range = self.make_range(expr.range().start, value.range().end);
                stmts.push(Stmt::Assign(Assign {
                    target: expr,
                    value,
                    range,
                }));
                self.eat_stmt_end();
                continue;
            }
            // If next is } or eof, this is the final value expression
            self.eat_newlines();
            if self.check(TokenKind::RBrace, None) || self.peek().kind == TokenKind::Eof {
                let end = self.expect(TokenKind::RBrace, None).end;
                return Expr::BlockExpr {
                    stmts,
                    value: Box::new(expr),
                    range: self.make_range(start, end),
                };
            }
            // Otherwise it's an expression statement, keep going
            if let Some(a) = gate_builtin_assign(&expr) {
                stmts.push(Stmt::Assign(a));
            } else {
                stmts.push(Stmt::ExprStmt(ExprStmt {
                    expr,
                    range: SourceRange::default(),
                }));
            }
            self.eat_stmt_end();
        }

        // Empty block or block with no final expression — use 0 as default
        let end = self.expect(TokenKind::RBrace, None).end;
        Expr::BlockExpr {
            stmts,
            value: Box::new(Expr::IntLit {
                value: 0,
                text: "0".into(),
                range: self.make_range(start, end),
            }),
            range: self.make_range(start, end),
        }
    }
}
