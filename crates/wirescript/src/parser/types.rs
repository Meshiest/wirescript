//! Type expressions.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_type(&mut self) -> TypeExpr {
        let mut first = self.parse_type_postfix();
        // `A | B | C`
        if self.check(TokenKind::Op, Some("|")) {
            let mut options = vec![first];
            while self.match_tok(TokenKind::Op, Some("|")).is_some() {
                options.push(self.parse_type_postfix());
            }
            let start = options[0].range().start;
            let end = options.last().unwrap().range().end;
            first = TypeExpr::Union {
                options,
                range: self.make_range(start, end),
            };
        }
        first
    }

    fn parse_type_postfix(&mut self) -> TypeExpr {
        let mut t = self.parse_type_primary();
        // `T[]` (possibly repeated for multi-dim, though unusual).
        while self.match_tok(TokenKind::LBracket, None).is_some() {
            self.expect(TokenKind::RBracket, None);
            let end = self.peek().start;
            let start = t.range().start;
            t = TypeExpr::Array {
                inner: Box::new(t),
                range: self.make_range(start, end),
            };
        }
        t
    }

    fn parse_type_primary(&mut self) -> TypeExpr {
        let t = self.peek().clone();
        if (t.kind == TokenKind::Kw && t.text == "ref")
            || (t.kind == TokenKind::Op && t.text == "*")
        {
            let start = self.advance().start;
            let inner = self.parse_type_postfix();
            let end = inner.range().end;
            return TypeExpr::Ref {
                inner: Box::new(inner),
                range: self.make_range(start, end),
            };
        }
        if t.kind == TokenKind::LParen {
            let start = self.advance().start;
            let mut fields: Vec<TypeExpr> = Vec::new();
            while !self.check(TokenKind::RParen, None) {
                fields.push(self.parse_type());
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
            }
            let end = self.expect(TokenKind::RParen, None).end;
            return TypeExpr::Tuple {
                fields,
                range: self.make_range(start, end),
            };
        }
        // Record type: `{ field: Type, ... }`
        if t.kind == TokenKind::LBrace {
            let start = self.advance().start;
            let mut fields: Vec<RecordTypeField> = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                // A `///` doc comment may precede a field; keyed by the field's
                // offset so hover can show it (same channel as decl/stmt docs).
                let doc = self.collect_doc_comment();
                if self.check(TokenKind::RBrace, None) || self.peek().kind == TokenKind::Eof {
                    break; // trailing doc with no following field
                }
                let fstart = self.peek().start;
                let fname = self.expect(TokenKind::Ident, None).text;
                self.expect(TokenKind::Colon, None);
                let ftyp = self.parse_type();
                let fend = self.peek().start;
                if let Some(doc) = doc {
                    self.doc_comments.insert(fstart.offset, doc);
                }
                fields.push(RecordTypeField {
                    name: fname,
                    typ: ftyp,
                    range: self.make_range(fstart, fend),
                });
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    self.eat_newlines();
                    break;
                }
                self.eat_newlines();
            }
            let end = self.expect(TokenKind::RBrace, None).end;
            return TypeExpr::Record {
                fields,
                range: self.make_range(start, end),
            };
        }
        // Plain identifier type name (int, bool, controller, chipTypeName, …),
        // or one qualified by a namespace alias (`T.Point` after
        // `import * as T`). The qualified form is kept as a dotted name and
        // resolved against that namespace's type aliases.
        let name_tok = self.expect(TokenKind::Ident, None);
        let mut name = name_tok.text;
        let mut end = name_tok.end;
        while self.check(TokenKind::Dot, None)
            && self.peek_at(1).kind == TokenKind::Ident
        {
            self.advance();
            let member = self.expect(TokenKind::Ident, None);
            name = format!("{name}.{}", member.text);
            end = member.end;
        }
        // Generic application `Name<Arg, ...>` (e.g. `Map<string, int>`,
        // `Array<int>`, `Ref<Point>`). `Array`/`Ref` desugar straight to the
        // existing postfix forms so downstream code sees no new shape.
        if self.check(TokenKind::Op, Some("<")) {
            self.advance();
            let mut args: Vec<TypeExpr> = Vec::new();
            while !self.check(TokenKind::Op, Some(">")) && self.peek().kind != TokenKind::Eof {
                args.push(self.parse_type());
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
            }
            let close = self.expect(TokenKind::Op, Some(">"));
            end = close.end;
            let range = self.make_range(name_tok.start, end);
            if name == "Array" && args.len() == 1 {
                return TypeExpr::Array {
                    inner: Box::new(args.into_iter().next().unwrap()),
                    range,
                };
            }
            if name == "Ref" && args.len() == 1 {
                return TypeExpr::Ref {
                    inner: Box::new(args.into_iter().next().unwrap()),
                    range,
                };
            }
            return TypeExpr::Generic { name, args, range };
        }
        TypeExpr::Name {
            name,
            range: self.make_range(name_tok.start, end),
        }
    }
}
