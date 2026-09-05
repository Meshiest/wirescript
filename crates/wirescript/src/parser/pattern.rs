//! Pattern syntax shared by match / if-let / let-else (Task 9: parser only -
//! no typecheck/lowering here).

use super::*;

impl<'a> Parser<'a> {
    /// Parse a single pattern: `_` (wildcard), `Name(...)` / `Name { ... }`
    /// (a variant with a positional/named payload), or a bare identifier
    /// (`Pattern::Binding` - kept dumb here; the typechecker reclassifies a
    /// bare capitalized name into a unit-variant match once the scrutinee's
    /// enum type is known).
    pub(super) fn parse_pattern(&mut self) -> Pattern {
        if !self.enter_nesting() {
            self.leave_nesting(1);
            let t = self.peek().clone();
            return Pattern::Wildcard(self.make_range(t.start, t.end));
        }
        let p = self.parse_pattern_inner();
        self.leave_nesting(1);
        p
    }

    fn parse_pattern_inner(&mut self) -> Pattern {
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
                    // Shorthand: `w` means `w: w` - the field binds a local
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
                sub: VariantPattern::Named { fields, ignore_rest },
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
