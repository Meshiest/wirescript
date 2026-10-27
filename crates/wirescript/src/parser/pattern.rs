//! Pattern syntax shared by match / if-let / let-else.

use super::*;

impl Parser<'_> {
    /// Parse a single pattern: alternatives joined by `|`, each a `..` chain
    /// (a string concat or a number range) of pattern atoms. An atom is `_`, a
    /// literal (`"+"`, `3`, `-1`, `2.5`, `true`, `:red`), `Name(...)` /
    /// `Name { ... }` (a variant with a positional/named payload), or a bare
    /// identifier (`Pattern::Binding`; the typechecker reclassifies a bare
    /// capitalized name into a unit-variant match once the scrutinee's enum
    /// type is known).
    pub(super) fn parse_pattern(&mut self) -> Pattern {
        if !self.enter_nesting() {
            self.leave_nesting(1);
            let t = self.peek().clone();
            return Pattern::Wildcard(self.make_range(t.start, t.end));
        }
        let p = self.parse_or_pattern();
        self.leave_nesting(1);
        p
    }

    /// Whether the current token can only begin a literal pattern atom.
    pub(super) fn at_literal_pattern(&self) -> bool {
        let t = self.peek();
        match t.kind {
            TokenKind::Str | TokenKind::Int | TokenKind::Float | TokenKind::Atom => true,
            TokenKind::Kw => matches!(t.text.as_str(), "true" | "false"),
            TokenKind::Op => {
                t.text == "-" && matches!(self.peek_at(1).kind, TokenKind::Int | TokenKind::Float)
            }
            _ => false,
        }
    }

    fn parse_or_pattern(&mut self) -> Pattern {
        let first = self.parse_dotdot_pattern();
        // An alternative may continue on the next line, led by its `|`: no
        // arm or payload position starts with one.
        let continues =
            |p: &Self| p.peek_non_nl().kind == TokenKind::Op && p.peek_non_nl().text == "|";
        if !continues(self) {
            return first;
        }
        let start = first.range().start;
        let mut alts = vec![first];
        while continues(self) {
            self.eat_newlines();
            self.advance();
            self.eat_newlines();
            alts.push(self.parse_dotdot_pattern());
        }
        let end = alts.last().map(|p| p.range().end).unwrap_or(start);
        Pattern::Or {
            alts,
            range: self.make_range(start, end),
        }
    }

    /// One alternative: an atom, or atoms joined by `..`. Joined numbers are a
    /// range (`0..10`); anything else is a string concat (`"say " .. rest`),
    /// whose shape typecheck validates.
    fn parse_dotdot_pattern(&mut self) -> Pattern {
        let first = self.parse_pattern_atom();
        if !self.check(TokenKind::Op, Some("..")) {
            return first;
        }
        let start = first.range().start;
        let is_number = |p: &Pattern| matches!(p, Pattern::Literal { expr, .. } if matches!(**expr, Expr::IntLit { .. } | Expr::FloatLit { .. }));
        if is_number(&first) {
            self.advance();
            let hi = self.parse_pattern_atom();
            let end = hi.range().end;
            let range = self.make_range(start, end);
            return match (first, hi) {
                (Pattern::Literal { expr: lo, .. }, hi @ Pattern::Literal { .. })
                    if is_number(&hi) =>
                {
                    let Pattern::Literal { expr: hi, .. } = hi else {
                        unreachable!()
                    };
                    Pattern::Range { lo, hi, range }
                }
                (_, hi) => {
                    let r = hi.range().clone();
                    self.error(
                        "a range pattern's end must be a number literal",
                        r.start,
                        r.end,
                    );
                    Pattern::Wildcard(range)
                }
            };
        }
        let mut parts = vec![first];
        while self.match_tok(TokenKind::Op, Some("..")).is_some() {
            parts.push(self.parse_pattern_atom());
        }
        let end = parts.last().map(|p| p.range().end).unwrap_or(start);
        Pattern::Concat {
            parts,
            template: false,
            range: self.make_range(start, end),
        }
    }

    fn parse_pattern_atom(&mut self) -> Pattern {
        if self.at_literal_pattern() {
            let expr = if self.check(TokenKind::Op, Some("-")) {
                self.parse_prefix()
            } else {
                self.parse_primary()
            };
            let range = expr.range().clone();
            return Pattern::Literal {
                expr: Box::new(expr),
                range,
            };
        }
        if self.check(TokenKind::StrInterp, None) {
            return self.parse_template_pattern();
        }
        self.parse_name_pattern()
    }

    /// `"give ${item} to ${who}"`: the string pattern `"give " .. item ..
    /// " to " .. who`. Each `${name}` is a hole binding `name`, `${_}` one
    /// binding nothing. Anything else in a `${}` stays a part typecheck
    /// rejects (WS122), since a pattern computes nothing.
    fn parse_template_pattern(&mut self) -> Pattern {
        let Expr::InterpLit { parts, range } = self.parse_primary() else {
            let t = self.peek().clone();
            return Pattern::Wildcard(self.make_range(t.start, t.end));
        };
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            out.push(match part {
                InterpPart::Lit(value) if value.is_empty() => continue,
                InterpPart::Lit(value) => Pattern::Literal {
                    expr: Box::new(Expr::StringLit {
                        value,
                        range: range.clone(),
                    }),
                    range: range.clone(),
                },
                InterpPart::Expr(e) => match *e {
                    Expr::Ident { name, range } if name == "_" => Pattern::Wildcard(range),
                    Expr::Ident { name, range } => Pattern::Binding { name, range },
                    other => {
                        let range = other.range().clone();
                        Pattern::Literal {
                            expr: Box::new(other),
                            range,
                        }
                    }
                },
            });
        }
        // `"${x}"` alone is the name `x`: it matches every string.
        if let [Pattern::Binding { .. } | Pattern::Wildcard(_)] = out.as_slice() {
            return out.pop().expect("one part");
        }
        Pattern::Concat {
            parts: out,
            template: true,
            range,
        }
    }

    fn parse_name_pattern(&mut self) -> Pattern {
        let mut name_tok = self.expect(TokenKind::Ident, None);
        if name_tok.text == "_" {
            return Pattern::Wildcard(self.make_range(name_tok.start, name_tok.end));
        }
        // `Shape.Circle(r)`, the enum-qualified spelling. Construction and
        // `is` both require it, so writing it in a pattern is the natural
        // guess. The scrutinee's type still decides the enum; typecheck checks
        // the qualifier against it.
        let start = name_tok.start;
        let mut enum_path = None;
        if self.check(TokenKind::Dot, None) && self.peek_at(1).kind == TokenKind::Ident {
            self.advance();
            enum_path = Some(std::mem::take(&mut name_tok.text));
            name_tok = self.expect(TokenKind::Ident, None);
        }
        if self.check(TokenKind::LParen, None) {
            self.advance();
            self.eat_newlines();
            let mut elems: Vec<Pattern> = Vec::new();
            while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                elems.push(self.parse_pattern());
                self.eat_newlines();
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    self.eat_newlines();
                    break;
                }
                self.eat_newlines();
            }
            let end = self.expect(TokenKind::RParen, None).end;
            return Pattern::Variant {
                enum_path,
                variant: name_tok.text,
                sub: VariantPattern::Positional(elems),
                range: self.make_range(start, end),
            };
        }
        if self.check(TokenKind::LBrace, None) {
            self.advance();
            self.eat_newlines();
            let mut fields: Vec<(String, Pattern)> = Vec::new();
            let mut ignore_rest = false;
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                if self.check(TokenKind::Op, Some("..")) {
                    self.advance();
                    ignore_rest = true;
                    self.eat_newlines();
                    break;
                }
                let field_tok = self.expect(TokenKind::Ident, None);
                let field_pattern = if self.match_tok(TokenKind::Colon, None).is_some() {
                    self.parse_pattern()
                } else {
                    // Shorthand: `w` means `w: w`, so the field binds a local
                    // of the same name.
                    Pattern::Binding {
                        name: field_tok.text.clone(),
                        range: self.make_range(field_tok.start, field_tok.end),
                    }
                };
                fields.push((field_tok.text, field_pattern));
                self.eat_newlines();
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    self.eat_newlines();
                    break;
                }
                self.eat_newlines();
            }
            let end = self.expect(TokenKind::RBrace, None).end;
            return Pattern::Variant {
                enum_path,
                variant: name_tok.text,
                sub: VariantPattern::Named {
                    fields,
                    ignore_rest,
                },
                range: self.make_range(start, end),
            };
        }
        // A qualified name with no payload is a unit variant outright, there
        // is nothing for the typechecker's bare-identifier reclassification to
        // be ambiguous about.
        match enum_path {
            Some(_) => Pattern::Variant {
                enum_path,
                variant: name_tok.text,
                sub: VariantPattern::Unit,
                range: self.make_range(start, name_tok.end),
            },
            None => Pattern::Binding {
                name: name_tok.text,
                range: self.make_range(start, name_tok.end),
            },
        }
    }
}
