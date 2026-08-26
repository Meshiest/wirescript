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
        let name_tok = self.expect(TokenKind::Ident, None);
        if name_tok.text == "_" {
            return Pattern::Wildcard(self.make_range(name_tok.start, name_tok.end));
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
                variant: name_tok.text,
                sub: VariantPattern::Positional(elems),
                range: self.make_range(name_tok.start, end),
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
                variant: name_tok.text,
                sub: VariantPattern::Named { fields, ignore_rest },
                range: self.make_range(name_tok.start, end),
            };
        }
        Pattern::Binding {
            name: name_tok.text,
            range: self.make_range(name_tok.start, name_tok.end),
        }
    }
}
