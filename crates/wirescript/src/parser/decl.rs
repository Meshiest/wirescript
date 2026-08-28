//! Declaration parsing: one `TopDecl` variant per method.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_top_decl(&mut self) -> Option<TopDecl> {
        self.eat_newlines();
        let t = self.peek().clone();
        if t.kind == TokenKind::Annotation {
            let anns = self.parse_annotations();
            let t2 = self.peek().clone();
            let kw = |k: &str| t2.kind == TokenKind::Kw && t2.text == k;
            if kw("in") || kw("out") {
                if let Some(r) = &anns.closed {
                    self.error(
                        "@closed is not allowed on 'in'/'out' declarations".to_string(),
                        r.start,
                        r.end,
                    );
                }
                let side = anns.side.map(|(s, _)| s);
                let label = anns.label.map(|(l, _)| l);
                let label_expr = anns.label_expr.map(|(e, _)| e);
                let invisible = anns.invisible.is_some();
                let no_fold = anns.nofold.is_some();
                if kw("in") {
                    if let Some(r) = &anns.nofold {
                        self.warn(
                            "@nofold has no effect on an 'in' declaration",
                            r.start,
                            r.end,
                        );
                    }
                    return Some(self.parse_in_decl(side, label, label_expr, invisible));
                }
                return Some(TopDecl::Out(
                    self.parse_out_binding(side, label, label_expr, invisible, no_fold),
                ));
            }
            let next_is_open_chip = kw("open")
                && self.peek_at(1).kind == TokenKind::Kw
                && self.peek_at(1).text == "chip";
            if kw("chip") || next_is_open_chip {
                if let Some((_, r)) = &anns.side {
                    self.error(
                        "a side annotation must be followed by an 'in' or 'out' declaration"
                            .to_string(),
                        r.start,
                        r.end,
                    );
                }
                let label = anns.label.map(|(l, _)| l);
                let label_expr = anns.label_expr.map(|(e, _)| e);
                let no_fold = anns.nofold.is_some();
                if next_is_open_chip {
                    if let Some(r) = &anns.closed {
                        self.error(
                            "@closed cannot be combined with 'open chip'".to_string(),
                            r.start,
                            r.end,
                        );
                    }
                    self.advance(); // consume "open"
                    let decl = self.parse_chip_decl(true, label, label_expr, false, no_fold);
                    if let (TopDecl::AnonChip(_), Some(r)) = (&decl, &anns.nofold) {
                        self.warn("@nofold has no effect on an anonymous chip", r.start, r.end);
                    }
                    return Some(decl);
                }
                let decl =
                    self.parse_chip_decl(false, label, label_expr, anns.closed.is_some(), no_fold);
                if let (TopDecl::AnonChip(_), Some(r)) = (&decl, &anns.nofold) {
                    self.warn("@nofold has no effect on an anonymous chip", r.start, r.end);
                }
                return Some(decl);
            }
            // `@nofold` (alone — no side/label/closed) is also legal directly on
            // `var` / `static var` / `let` / `on`, at any nesting depth. `@label`
            // (string or constant expression) is additionally legal directly on
            // `var` / `static var` — it overrides the var's name-derived
            // floating label, same as on a port. Any other annotation combined
            // with these still falls through to the generic "must be followed
            // by ..." error below, unchanged.
            let is_static_var = kw("static")
                && self.peek_at(1).kind == TokenKind::Kw
                && self.peek_at(1).text == "var";
            let no_side_closed_invisible =
                anns.side.is_none() && anns.closed.is_none() && anns.invisible.is_none();
            let var_ann_ok = no_side_closed_invisible
                && (anns.nofold.is_some() || anns.label.is_some() || anns.label_expr.is_some());
            let bare_nofold = no_side_closed_invisible
                && anns.nofold.is_some()
                && anns.label.is_none()
                && anns.label_expr.is_none();
            if (var_ann_ok && (kw("var") || is_static_var)) || (bare_nofold && (kw("let") || kw("on")))
            {
                let no_fold = anns.nofold.is_some();
                let label = anns.label.map(|(l, _)| l);
                let label_expr = anns.label_expr.map(|(e, _)| e);
                if is_static_var {
                    self.advance(); // consume "static"
                    return Some(self.parse_var_decl(true, no_fold, label, label_expr));
                }
                if kw("var") {
                    return Some(self.parse_var_decl(false, no_fold, label, label_expr));
                }
                if kw("let") {
                    let mut decl = self.parse_let_decl(false);
                    match &mut decl {
                        TopDecl::Let(l) => l.no_fold = true,
                        TopDecl::Event(e) => e.no_fold = true,
                        TopDecl::Await(a) => a.no_fold = true,
                        _ => {}
                    }
                    return Some(decl);
                }
                return Some(TopDecl::Handler(self.parse_handler(true)));
            }
            if kw("mod") {
                self.error(
                    "annotations are not allowed on 'mod' declarations".to_string(),
                    t.start,
                    t2.end,
                );
                return Some(self.parse_mod_decl(false));
            }
            self.error(
                "an annotation must be followed by an 'in', 'out', or chip declaration \
                 (a bare @nofold is also allowed before 'var', 'static var', 'let', or 'on')"
                    .to_string(),
                t.start,
                t2.end,
            );
            if t2.kind == TokenKind::Eof {
                return None;
            }
            return self.parse_top_decl(); // annotations consumed → guaranteed progress
        }
        // `enum` is a contextual keyword, so it still lexes as an identifier and
        // stays usable as a name. Only `enum` followed by another identifier —
        // the type's name, a shape no expression can take — opens a declaration.
        if t.kind == TokenKind::Ident
            && t.text == "enum"
            && self.peek_at(1).kind == TokenKind::Ident
        {
            return Some(self.parse_enum_decl());
        }
        if t.kind == TokenKind::Kw {
            match t.text.as_str() {
                "var" => return Some(self.parse_var_decl(false, false, None, None)),
                "static" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "var" {
                        self.advance(); // consume "static"
                        return Some(self.parse_var_decl(true, false, None, None));
                    }
                }
                "buffer" => return Some(self.parse_buffer_decl()),
                "in" => return Some(self.parse_in_decl(None, None, None, false)),
                "out" => {
                    return Some(TopDecl::Out(
                        self.parse_out_binding(None, None, None, false, false),
                    ));
                }
                "let" => return Some(self.parse_let_decl(false)),
                "const" => {
                    return Some(match self.peek_at(1).text.as_str() {
                        "mod" => {
                            self.advance(); // consume "const"
                            self.parse_mod_decl(/*is_const=*/ true)
                        }
                        // `const chip` is rejected: a chip shares one compiled template
                        // across call sites, so "every parameter is const" is not a
                        // property of the declaration the way it is for an inlined mod.
                        // Const PARAMETERS on a chip are supported — they extend its
                        // template key.
                        "chip" => {
                            let kw = self.peek().clone();
                            self.error(
                                "`const chip` is not allowed — use `const mod`, or mark \
                                 individual parameters `const` (`chip C(name: const \
                                 string, …)`)",
                                kw.start,
                                kw.end,
                            );
                            self.advance(); // consume "const"
                            self.parse_chip_decl(false, None, None, false, false)
                        }
                        _ => self.parse_let_decl(/*is_const=*/ true),
                    });
                }
                "on" => return Some(TopDecl::Handler(self.parse_handler(false))),
                "array" => return Some(self.parse_array_decl()),
                "map" => return Some(self.parse_map_decl()),
                "chip" => return Some(self.parse_chip_decl(false, None, None, false, false)),
                "mod" => return Some(self.parse_mod_decl(false)),
                "open" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "chip" {
                        self.advance(); // consume "open"
                        return Some(self.parse_chip_decl(true, None, None, false, false));
                    }
                }
                "fn" => return Some(self.parse_fn_decl()),
                "import" => return Some(self.parse_import_decl()),
                "type" => return Some(self.parse_type_alias_decl()),
                "if" => {
                    let s = self.parse_if_stmt();
                    match s {
                        Stmt::If(i) => return Some(TopDecl::If(i)),
                        Stmt::IfLet(i) => return Some(TopDecl::IfLet(i)),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Fallthrough: assignment or expression-statement.
        let expr_start = self.peek().start;
        let lhs = self.parse_expr();
        if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            let rhs = self.parse_expr();
            let end = rhs.range().end;
            self.eat_stmt_end();
            return Some(TopDecl::Assign(Assign {
                target: lhs,
                value: rhs,
                range: self.make_range(expr_start, end),
            }));
        }
        self.eat_stmt_end();
        if let Some(a) = gate_builtin_assign(&lhs) {
            return Some(TopDecl::Assign(a));
        }
        Some(TopDecl::ExprStmt(ExprStmt {
            range: self.make_range(expr_start, lhs.range().end),
            expr: lhs,
        }))
    }

    pub(super) fn parse_var_decl(
        &mut self,
        is_static: bool,
        no_fold: bool,
        label: Option<String>,
        label_expr: Option<Expr>,
    ) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("var")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        let init = if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            Some(self.parse_expr())
        } else if !matches!(
            self.peek().kind,
            TokenKind::Newline | TokenKind::Semi | TokenKind::Eof | TokenKind::RBrace
        ) {
            // A `var` may legitimately have no initializer (`var x: int`), but
            // only when the declaration actually ends here. A leftover token
            // that isn't `=` is almost always a missing `=`: `var x: int 5`.
            // Report it, then recover by taking the expression as the value.
            let (s, e, txt) = {
                let t = self.peek();
                (t.start, t.end, t.text.clone())
            };
            self.error(
                format!("missing `=` before the initializer for `var {name}`; found `{txt}`"),
                s,
                e,
            );
            Some(self.parse_expr())
        } else {
            None
        };
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::Var(VarDecl {
            name,
            typ,
            init,
            is_static,
            no_fold,
            label,
            label_expr,
            range: self.make_range(start, end),
        })
    }

    pub(super) fn parse_buffer_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("buffer")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(TokenKind::Op, Some("="));
        let init = self.parse_expr();
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::Buffer(BufferDecl {
            name,
            typ,
            init,
            range: self.make_range(start, end),
        })
    }

    pub(super) fn parse_in_decl(
        &mut self,
        side: Option<PortSide>,
        label: Option<String>,
        label_expr: Option<Expr>,
        invisible: bool,
    ) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("in")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        self.expect(TokenKind::Colon, None);
        let typ = self.parse_type();
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::In(InDecl {
            name,
            typ,
            side,
            label,
            label_expr,
            invisible,
            range: self.make_range(start, end),
        })
    }

    pub(super) fn parse_out_binding(
        &mut self,
        side: Option<PortSide>,
        label: Option<String>,
        label_expr: Option<Expr>,
        invisible: bool,
        no_fold: bool,
    ) -> OutBinding {
        let start = self.expect(TokenKind::Kw, Some("out")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            let value = self.parse_expr();
            let end = value.range().end;
            self.eat_stmt_end();
            OutBinding {
                name,
                value: Some(value),
                typ,
                side,
                label: label.clone(),
                label_expr: label_expr.clone(),
                invisible,
                no_fold,
                range: self.make_range(start, end),
            }
        } else {
            let end = self.peek().start;
            // `out NAME` declares a port driven by `emit`. Anything else on the
            // line — most often `out f(x)`, which reads like an anonymous output
            // of a call — is not a value binding, and letting it fall through
            // silently re-parsed the remainder as its own declaration.
            let t = self.peek().clone();
            if !matches!(
                t.kind,
                TokenKind::Newline | TokenKind::Semi | TokenKind::Eof | TokenKind::RBrace
            ) {
                self.error(
                    format!(
                        "unexpected `{}` after output port `{name}` — write `out {name} = <expr>` \
                         to bind a value, or `out {name}` to declare a port driven by `emit`",
                        t.text
                    ),
                    t.start,
                    t.end,
                );
            }
            self.eat_stmt_end();
            OutBinding {
                name,
                value: None,
                typ,
                side,
                label,
                label_expr,
                invisible,
                no_fold,
                range: self.make_range(start, end),
            }
        }
    }

    pub(super) fn parse_let_decl(&mut self, is_const: bool) -> TopDecl {
        let start = self
            .expect(TokenKind::Kw, Some(if is_const { "const" } else { "let" }))
            .start;

        // Record destructuring: `let { a, b: alias, ...rest } = expr`
        if self.check(TokenKind::LBrace, None) {
            let brace_start = self.advance().start; // consume `{`
            let mut fields: Vec<RecordDestructField> = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                if self.check(TokenKind::Op, Some("...")) {
                    let spread_start = self.advance().start;
                    let rest_tok = self.expect(TokenKind::Ident, None);
                    fields.push(RecordDestructField::Rest {
                        name: rest_tok.text,
                        range: self.make_range(spread_start, rest_tok.end),
                    });
                    self.eat_newlines();
                    // `...rest` must be last
                    break;
                }
                let name_tok = self.expect(TokenKind::Ident, None);
                let alias = if self.match_tok(TokenKind::Colon, None).is_some() {
                    let alias_tok = self.expect(TokenKind::Ident, None);
                    Some(alias_tok.text)
                } else {
                    None
                };
                let field_end = self.peek().start;
                fields.push(RecordDestructField::Named {
                    name: name_tok.text,
                    alias,
                    range: self.make_range(name_tok.start, field_end),
                });
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_newlines();
            }
            let brace_end = self.expect(TokenKind::RBrace, None).end;
            let binding = LetBinding::RecordDestruct {
                fields,
                range: self.make_range(brace_start, brace_end),
            };
            let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
                Some(self.parse_type())
            } else {
                None
            };
            self.expect(TokenKind::Op, Some("="));
            // `let { a, b } = await sig` — destructure the awaited signal's
            // ferried payload fields into locals.
            if self.check(TokenKind::Kw, Some("await")) {
                let await_start = self.advance().start;
                if let Stmt::Await(mut a) = self.parse_await_inner(await_start, None) {
                    let pairs: Vec<(String, String)> = match &binding {
                        LetBinding::RecordDestruct { fields, .. } => fields
                            .iter()
                            .filter_map(|f| match f {
                                RecordDestructField::Named { name, alias, .. } => Some((
                                    name.clone(),
                                    alias.clone().unwrap_or_else(|| name.clone()),
                                )),
                                RecordDestructField::Rest { .. } => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    a.destructure = Some(pairs);
                    a.range = self.make_range(start, a.range.end);
                    self.eat_stmt_end();
                    return TopDecl::Await(a);
                }
            }
            let value = self.parse_expr();
            let end = value.range().end;
            self.eat_stmt_end();
            return TopDecl::Let(LetDecl {
                binding,
                typ,
                value,
                no_fold: false,
                is_const,
                range: self.make_range(start, end),
            });
        }

        // Tuple destructuring: `let (a, b, ...rest) = expr`
        if self.check(TokenKind::LParen, None) {
            let paren_start = self.advance().start; // consume `(`
            let mut names: Vec<String> = Vec::new();
            let mut rest: Option<String> = None;
            self.eat_newlines();
            while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                if self.check(TokenKind::Op, Some("...")) {
                    self.advance();
                    let rest_tok = self.expect(TokenKind::Ident, None);
                    rest = Some(rest_tok.text);
                    self.eat_newlines();
                    break;
                }
                let name_tok = self.expect(TokenKind::Ident, None);
                names.push(name_tok.text);
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_newlines();
            }
            let paren_end = self.expect(TokenKind::RParen, None).end;
            let binding = LetBinding::Tuple {
                names: names.clone(),
                rest,
                range: self.make_range(paren_start, paren_end),
            };
            let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
                Some(self.parse_type())
            } else {
                None
            };
            self.expect(TokenKind::Op, Some("="));
            // `let (a, b) = await CustomEvent("c")`: POSITIONAL capture of the
            // event's data outputs (a = DataOut1, b = DataOut2).
            if self.check(TokenKind::Kw, Some("await")) {
                let await_start = self.advance().start;
                if let Stmt::Await(mut a) = self.parse_await_inner(await_start, None) {
                    a.tuple_destructure = Some(names);
                    a.range = self.make_range(start, a.range.end);
                    self.eat_stmt_end();
                    return TopDecl::Await(a);
                }
            }
            let value = self.parse_expr();
            let end = value.range().end;
            self.eat_stmt_end();
            return TopDecl::Let(LetDecl {
                binding,
                typ,
                value,
                no_fold: false,
                is_const,
                range: self.make_range(start, end),
            });
        }

        // `let Some(x) = expr else { ... }` / `let Box { w } = expr else { ... }`:
        // a refutable variant-pattern head, detected by shape - an identifier
        // directly followed by `(` or `{`, which never occurs in the plain
        // single-binding form below (that form's name is always followed by
        // `:` or `=`). `parse_pattern` always returns `Pattern::Variant` for
        // this shape. Phase 4: single-variant refutable binds only - a bare
        // capitalized unit-variant name (`let None = ... else`) parses as
        // `Pattern::Binding`, not `Pattern::Variant`, so it still falls
        // through to the plain binding path below and is out of scope here.
        if self.peek().kind == TokenKind::Ident
            && matches!(self.peek_at(1).kind, TokenKind::LParen | TokenKind::LBrace)
        {
            let pattern = self.parse_pattern();
            self.expect(TokenKind::Op, Some("="));
            // Header position like `if`/`match`'s scrutinee: the mandatory
            // `else` keyword already disambiguates a trailing `{` from
            // braced variant construction, but suppress it too for
            // consistency with `if let`'s scrutinee.
            let scrutinee = self.parse_expr_no_brace_construct();
            self.eat_newlines();
            self.expect(TokenKind::Kw, Some("else"));
            self.eat_newlines();
            let else_block = self.parse_block();
            let end = else_block.range.end;
            return TopDecl::LetElse(LetElse {
                pattern,
                scrutinee,
                else_block,
                is_const,
                range: self.make_range(start, end),
            });
        }

        let name_tok = self.expect(TokenKind::Ident, None);
        let name = name_tok.text.clone();
        let binding = LetBinding::Ident {
            name: name_tok.text,
            range: self.make_range(name_tok.start, name_tok.end),
        };
        let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        // `let name: exec` — local exec signal, no value needed
        if let Some(TypeExpr::Name {
            name: ref type_name,
            range: ref type_range,
        }) = typ
        {
            if type_name == "exec" && !self.check(TokenKind::Op, Some("=")) {
                let end = type_range.end;
                self.eat_stmt_end();
                return TopDecl::Let(LetDecl {
                    binding,
                    typ,
                    value: Expr::IntLit {
                        value: 0,
                        text: "0".into(),
                        range: self.make_range(start, end),
                    },
                    no_fold: false,
                    is_const,
                    range: self.make_range(start, end),
                });
            }
        }
        self.expect(TokenKind::Op, Some("="));
        // `let name = on Trigger() { ... }` → EventDecl (captured event)
        if self.check(TokenKind::Kw, Some("on")) {
            self.advance();
            let trigger = self.parse_trigger();
            // Events are called here too: `let x = on RoundStart()`. Require and
            // consume the `()` on an event-name trigger, matching the handler form.
            self.require_event_call_parens(&trigger);
            if self.check(TokenKind::LParen, None) {
                // Consume the event call's balanced `( ... )` (config args; the
                // capture form does not currently thread config through).
                self.advance();
                let mut depth = 1usize;
                while depth > 0 && self.peek().kind != TokenKind::Eof {
                    match self.peek().kind {
                        TokenKind::LParen => depth += 1,
                        TokenKind::RParen => depth -= 1,
                        _ => {}
                    }
                    self.advance();
                }
            }
            if self.check(TokenKind::LBrace, None) {
                let captured_body = Some(self.parse_block());
                let end = captured_body.as_ref().unwrap().range.end;
                let source = trigger_to_expr(&trigger);
                return TopDecl::Event(EventDecl {
                    name,
                    source,
                    captured_body,
                    no_fold: false,
                    range: self.make_range(start, end),
                });
            }
            // `let name = on Trigger` (no body) → event alias
            let source = trigger_to_expr(&trigger);
            let end = source.range().end;
            self.eat_stmt_end();
            return TopDecl::Event(EventDecl {
                name,
                source,
                captured_body: None,
                no_fold: false,
                range: self.make_range(start, end),
            });
        }
        // `let name[: T] = await expr [on trigger]`: carry the `: T` annotation
        // onto the await so `let x: int = await CustomEvent("c")` can type the
        // captured event-data value.
        if self.check(TokenKind::Kw, Some("await")) {
            let await_start = self.advance().start;
            if let Stmt::Await(mut a) = self.parse_await_inner(await_start, None) {
                a.binding = Some(name);
                a.binding_type = typ;
                a.range = self.make_range(start, a.range.end);
                self.eat_stmt_end();
                return TopDecl::Await(a);
            }
        }
        let value = self.parse_expr();
        let end = value.range().end;
        self.eat_stmt_end();
        TopDecl::Let(LetDecl {
            binding,
            typ,
            value,
            no_fold: false,
            is_const,
            range: self.make_range(start, end),
        })
    }

    // `type Name = { field: Type, ... }` or `type Name = (A, B)`
    fn parse_type_alias_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("type")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let type_params = self.parse_type_params();
        self.expect(TokenKind::Op, Some("="));
        let typ = self.parse_type();
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::TypeAlias(TypeAliasDecl {
            name,
            type_params,
            typ,
            range: self.make_range(start, end),
        })
    }

    // `enum Name { Variant, Variant(T, ...), Variant { field: T, ... } = N, ... }`
    fn parse_enum_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Ident, Some("enum")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let type_params = self.parse_type_params();
        self.expect(TokenKind::LBrace, None);
        self.eat_newlines();
        let mut variants = Vec::new();
        while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
            variants.push(self.parse_enum_variant_decl());
            if self.match_tok(TokenKind::Comma, None).is_none() {
                self.eat_newlines();
                break;
            }
            self.eat_newlines();
        }
        let end = self.expect(TokenKind::RBrace, None).end;
        self.eat_stmt_end();
        TopDecl::Enum(EnumDecl {
            name,
            type_params,
            variants,
            range: self.make_range(start, end),
        })
    }

    // A single `enum` variant: an optional payload (`(T, ...)` positional or
    // `{ field: T, ... }` named), followed by an optional `= N` explicit
    // discriminant. No auto-numbering or duplicate detection happens here;
    // that is the registry's job once every variant has been parsed.
    fn parse_enum_variant_decl(&mut self) -> EnumVariantDecl {
        let name_tok = self.expect(TokenKind::Ident, None);
        let start = name_tok.start;
        let mut end = name_tok.end;
        let payload = if self.check(TokenKind::LParen, None) {
            self.advance();
            self.eat_newlines();
            let mut types = Vec::new();
            while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                types.push(self.parse_type());
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    self.eat_newlines();
                    break;
                }
                self.eat_newlines();
            }
            end = self.expect(TokenKind::RParen, None).end;
            EnumPayloadDecl::Positional(types)
        } else if self.check(TokenKind::LBrace, None) {
            self.advance();
            self.eat_newlines();
            let mut fields = Vec::new();
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                let fstart = self.peek().start;
                let fname = self.expect(TokenKind::Ident, None).text;
                self.expect(TokenKind::Colon, None);
                let ftyp = self.parse_type();
                let fend = self.peek().start;
                fields.push((fname, ftyp, self.make_range(fstart, fend)));
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    self.eat_newlines();
                    break;
                }
                self.eat_newlines();
            }
            end = self.expect(TokenKind::RBrace, None).end;
            EnumPayloadDecl::Named(fields)
        } else {
            EnumPayloadDecl::Unit
        };
        let explicit_disc = if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            let neg = self.match_tok(TokenKind::Op, Some("-")).is_some();
            let int_tok = self.expect(TokenKind::Int, None);
            let cleaned: String = int_tok.text.chars().filter(|c| *c != '_').collect();
            let value: i64 = cleaned.parse().unwrap_or(0);
            end = int_tok.end;
            Some(if neg { -value } else { value })
        } else {
            None
        };
        EnumVariantDecl {
            name: name_tok.text,
            explicit_disc,
            payload,
            range: self.make_range(start, end),
        }
    }

    // `var name: ElementType[]`
    pub(super) fn parse_array_decl(&mut self) -> TopDecl {
        let kw = self.expect(TokenKind::Kw, Some("array"));
        let start = kw.start;
        // The `array` declaration keyword has been removed — arrays are declared
        // with `var NAME: T[]` (identical storage). Reject, but keep parsing the
        // rest so a stray `array` decl doesn't derail the whole file.
        self.error(
            "`array` declarations have been removed — declare an array with `var NAME: T[]` instead",
            kw.start,
            kw.end,
        );
        let name = self.expect(TokenKind::Ident, None).text;
        self.expect(TokenKind::Colon, None);
        let full_type = self.parse_type();
        let element_type = match full_type {
            TypeExpr::Array { inner, .. } => *inner,
            other => {
                let r = other.range();
                self.error(
                    String::from("array element type must end with `[]`"),
                    r.start,
                    r.end,
                );
                other
            }
        };
        // Optional constant initializer: `= [ e, e, ... ]`.
        let mut init = Vec::new();
        if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            match self.parse_expr() {
                Expr::Array { elements, .. } => init = elements,
                other => self.error(
                    String::from("array initializer must be an array literal `[...]`"),
                    other.range().start,
                    other.range().end,
                ),
            }
        }
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::Array(ArrayDecl {
            name,
            element_type,
            init,
            range: self.make_range(start, end),
        })
    }

    // `var name: Map<K, V>`
    pub(super) fn parse_map_decl(&mut self) -> TopDecl {
        let kw = self.expect(TokenKind::Kw, Some("map"));
        let start = kw.start;
        // The `map` declaration keyword has been removed — maps are declared with
        // `var NAME: Map<K, V>` (identical storage). Reject, but keep parsing.
        self.error(
            "`map` declarations have been removed — declare a map with `var NAME: Map<K, V>` instead",
            kw.start,
            kw.end,
        );
        let name = self.expect(TokenKind::Ident, None).text;
        self.expect(TokenKind::Colon, None);
        let full_type = self.parse_type();
        let (key_type, value_type) = match full_type {
            TypeExpr::Generic { name: gname, mut args, .. }
                if gname == "Map" && args.len() == 2 =>
            {
                let value_type = args.pop().unwrap();
                let key_type = args.pop().unwrap();
                (key_type, value_type)
            }
            other => {
                let r = other.range();
                self.error(
                    String::from("map type must be `Map<KeyType, ValueType>`"),
                    r.start,
                    r.end,
                );
                // Recover with `any` key/value so parsing continues.
                let anyt = |r: SourceRange| TypeExpr::Name { name: "any".into(), range: r };
                (anyt(other.range().clone()), anyt(other.range().clone()))
            }
        };
        let init = if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            Some(self.parse_expr())
        } else {
            None
        };
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::Map(MapDecl {
            name,
            key_type,
            value_type,
            init,
            range: self.make_range(start, end),
        })
    }

    /// Optional declaration-header generic type params: `<T>` / `<T: Numeric>`
    /// / `<T, U>`. Called right after a `mod`/`chip`/`type` decl's name is
    /// consumed — in that position `<` is unambiguous (it can only start a
    /// type-param list, never the `<` comparison operator), unlike at a call
    /// site, where an explicit type-argument `<...>` needs the speculative
    /// disambiguation in [`Self::try_parse_type_args`]. Returns `Vec::new()`
    /// when there's no `<`.
    fn parse_type_params(&mut self) -> Vec<TypeParam> {
        if !self.check(TokenKind::Op, Some("<")) {
            return Vec::new();
        }
        self.advance(); // consume `<`
        let mut params = Vec::new();
        while !self.check(TokenKind::Op, Some(">")) && self.peek().kind != TokenKind::Eof {
            let name_tok = self.expect(TokenKind::Ident, None);
            let mut end = name_tok.end;
            let bound = if self.match_tok(TokenKind::Colon, None).is_some() {
                let b = self.parse_type();
                end = b.range().end;
                Some(b)
            } else {
                None
            };
            params.push(TypeParam {
                name: name_tok.text,
                bound,
                range: self.make_range(name_tok.start, end),
            });
            if self.match_tok(TokenKind::Comma, None).is_none() {
                break;
            }
        }
        self.expect(TokenKind::Op, Some(">"));
        params
    }

    // `chip Name(params) [-> outputs] { body }`
    pub(super) fn parse_chip_decl(
        &mut self,
        open: bool,
        label: Option<String>,
        label_expr: Option<Expr>,
        closed: bool,
        no_fold: bool,
    ) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("chip")).start;
        // Shorthand: `chip let a = 1, b = 2, c = 3`
        if self.check(TokenKind::Kw, Some("let")) {
            self.advance();
            let mut stmts = Vec::new();
            loop {
                let ls = self.peek().start;
                let name_tok = self.expect(TokenKind::Ident, None);
                let binding = LetBinding::Ident {
                    name: name_tok.text,
                    range: self.make_range(name_tok.start, name_tok.end),
                };
                let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
                    Some(self.parse_type())
                } else {
                    None
                };
                self.expect(TokenKind::Op, Some("="));
                let value = self.parse_expr();
                let le = value.range().end;
                stmts.push(Stmt::Let(LetDecl {
                    binding,
                    typ,
                    value,
                    no_fold: false,
                    is_const: false,
                    range: self.make_range(ls, le),
                }));
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_stmt_end();
            }
            let end = stmts
                .last()
                .map(|s| match s {
                    Stmt::Let(l) => l.range.end,
                    _ => unreachable!(),
                })
                .unwrap_or(start);
            self.eat_stmt_end();
            // A `chip let` has no name of its own, so default its display label
            // to the binding name(s) — the chip should show what it computes.
            // An explicit `@label(...)` (string OR expression) still wins —
            // the name-derived fallback only applies when neither is given.
            let derived_label = if label.is_some() || label_expr.is_some() {
                label.clone()
            } else {
                let names: Vec<&str> = stmts
                    .iter()
                    .filter_map(|s| match s {
                        Stmt::Let(LetDecl {
                            binding: LetBinding::Ident { name, .. },
                            ..
                        }) => Some(name.as_str()),
                        _ => None,
                    })
                    .collect();
                (!names.is_empty()).then(|| names.join(", "))
            };
            return TopDecl::AnonChip(AnonChipDecl {
                open,
                body: Block {
                    stmts,
                    range: self.make_range(start, end),
                },
                range: self.make_range(start, end),
                label: derived_label,
                label_expr: label_expr.clone(),
                closed,
            });
        }
        // `chip on trigger { ... }` → `chip { on trigger { ... } }`
        if self.check(TokenKind::Kw, Some("on")) {
            let handler = self.parse_handler(false);
            let end = handler.range.end;
            return TopDecl::AnonChip(AnonChipDecl {
                open,
                body: Block {
                    stmts: vec![Stmt::Handler(handler)],
                    range: self.make_range(start, end),
                },
                range: self.make_range(start, end),
                label: label.clone(),
                label_expr: label_expr.clone(),
                closed,
            });
        }
        // Anonymous chip: `chip { body }` — no name, no params.
        if self.check(TokenKind::LBrace, None) {
            let body = self.parse_block();
            let end = body.range.end;
            return TopDecl::AnonChip(AnonChipDecl {
                open,
                body,
                range: self.make_range(start, end),
                label,
                label_expr,
                closed,
            });
        }
        let name = self.expect(TokenKind::Ident, None).text;
        let type_params = self.parse_type_params();
        let (inputs, rest) = self.parse_param_list();
        let outputs = if self.match_tok(TokenKind::Arrow, None).is_some() {
            self.parse_chip_outputs()
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        let end = body.range.end;
        TopDecl::Chip(ChipDecl {
            name,
            type_params,
            inputs,
            rest,
            outputs,
            body,
            range: self.make_range(start, end),
            inline: false,
            label,
            label_expr,
            closed,
            no_fold,
            is_const: false,
        })
    }

    fn expect_import_path(&mut self) -> (String, Pos) {
        let tok = self.expect(TokenKind::Str, None);
        let path = match tok.value {
            Some(TokenValue::Str(s)) => s,
            _ => tok.text,
        };
        (path, tok.end)
    }

    fn parse_import_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("import")).start;

        // import * as ns from "path"
        if self.check(TokenKind::Op, Some("*")) {
            self.advance();
            self.expect(TokenKind::Kw, Some("as"));
            let ns_name = self.expect(TokenKind::Ident, None).text;
            self.expect(TokenKind::Kw, Some("from"));
            let (path, end) = self.expect_import_path();
            self.eat_stmt_end();
            return TopDecl::Import(ImportDecl {
                path,
                kind: ImportKind::Namespace(ns_name),
                range: self.make_range(start, end),
            });
        }

        // import { foo, bar as baz } from "path"
        if self.check(TokenKind::LBrace, None) {
            self.advance();
            let mut bindings = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                let before = self.pos;
                let name_tok = self.expect(TokenKind::Ident, None);
                let name = name_tok.text;
                let (alias, binding_range) = if self.match_tok(TokenKind::Kw, Some("as")).is_some()
                {
                    let alias_tok = self.expect(TokenKind::Ident, None);
                    let r = self.make_range(alias_tok.start, alias_tok.end);
                    (Some(alias_tok.text), r)
                } else {
                    let r = self.make_range(name_tok.start, name_tok.end);
                    (None, r)
                };
                bindings.push(ImportBinding {
                    name,
                    alias,
                    range: binding_range,
                });
                self.eat_newlines();
                if !self.check(TokenKind::RBrace, None) {
                    self.expect(TokenKind::Comma, None);
                    self.eat_newlines();
                }
                // A token that is neither a binding nor a comma (both expects
                // fail without consuming) must not stall the loop.
                if self.pos == before {
                    self.advance();
                }
            }
            self.expect(TokenKind::RBrace, None);
            self.expect(TokenKind::Kw, Some("from"));
            let (path, end) = self.expect_import_path();
            self.eat_stmt_end();
            return TopDecl::Import(ImportDecl {
                path,
                kind: ImportKind::Named(bindings),
                range: self.make_range(start, end),
            });
        }

        // import "path"
        let (path, end) = self.expect_import_path();
        self.eat_stmt_end();
        TopDecl::Import(ImportDecl {
            path,
            kind: ImportKind::All,
            range: self.make_range(start, end),
        })
    }

    pub(super) fn parse_mod_decl(&mut self, is_const: bool) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("mod")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let type_params = self.parse_type_params();
        let (mut inputs, rest) = self.parse_param_list();
        // `const mod f(...)`: every parameter is implicitly const, regardless
        // of whether the parameter itself was written with a `const` modifier.
        if is_const {
            for p in &mut inputs {
                p.is_const = true;
            }
        }
        let outputs = if self.match_tok(TokenKind::Arrow, None).is_some() {
            self.parse_chip_outputs()
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        let end = body.range.end;
        TopDecl::Chip(ChipDecl {
            name,
            type_params,
            inputs,
            rest,
            outputs,
            body,
            range: self.make_range(start, end),
            inline: true,
            label: None,
            label_expr: None,
            closed: false,
            no_fold: false,
            is_const,
        })
    }

    /// Returns the fixed parameters plus, if present, the name of a trailing
    /// `...ident` variadic parameter that captures the leftover positional args
    /// into a compile-time tuple at each call site.
    fn parse_param_list(&mut self) -> (Vec<Param>, Option<String>) {
        self.expect(TokenKind::LParen, None);
        let mut params = Vec::new();
        let mut rest_param: Option<String> = None;
        let mut synth_counter = 0usize;
        self.eat_stmt_end();
        while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
            let pstart = self.peek().start;

            // Trailing variadic capture: `...rest` as the final parameter. Must
            // be last; anything after it is a parse error at the closing paren.
            if self.check(TokenKind::Op, Some("...")) {
                self.advance();
                let rest_tok = self.expect(TokenKind::Ident, None);
                rest_param = Some(rest_tok.text);
                self.eat_newlines();
                break;
            }

            // Record destructuring pattern: `{ x, y, ...rest }: Type`
            if self.check(TokenKind::LBrace, None) {
                self.advance(); // consume `{`
                let mut fields: Vec<RecordDestructField> = Vec::new();
                let mut rest: Option<String> = None;
                self.eat_newlines();
                while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                    if self.check(TokenKind::Op, Some("...")) {
                        let spread_start = self.advance().start;
                        let rest_tok = self.expect(TokenKind::Ident, None);
                        rest = Some(rest_tok.text.clone());
                        fields.push(RecordDestructField::Rest {
                            name: rest_tok.text,
                            range: self.make_range(spread_start, rest_tok.end),
                        });
                        self.eat_newlines();
                        break;
                    }
                    let name_tok = self.expect(TokenKind::Ident, None);
                    let alias = if self.match_tok(TokenKind::Colon, None).is_some() {
                        let alias_tok = self.expect(TokenKind::Ident, None);
                        Some(alias_tok.text)
                    } else {
                        None
                    };
                    let field_end = self.peek().start;
                    fields.push(RecordDestructField::Named {
                        name: name_tok.text,
                        alias,
                        range: self.make_range(name_tok.start, field_end),
                    });
                    if self.match_tok(TokenKind::Comma, None).is_none() {
                        self.eat_newlines();
                        break;
                    }
                    self.eat_newlines();
                }
                self.expect(TokenKind::RBrace, None);
                self.expect(TokenKind::Colon, None);
                let typ = self.parse_type();
                let pend = self.peek().start;
                let synth_name = format!("_p{}", synth_counter);
                synth_counter += 1;
                params.push(Param {
                    name: synth_name,
                    typ,
                    pattern: Some(ParamPattern::Record { fields, rest }),
                    is_const: false,
                    range: self.make_range(pstart, pend),
                });
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_stmt_end();
                continue;
            }

            // Tuple destructuring pattern: `(a, b, ...rest): Type`
            if self.check(TokenKind::LParen, None) {
                self.advance(); // consume `(`
                let mut names: Vec<String> = Vec::new();
                let mut rest: Option<String> = None;
                self.eat_newlines();
                while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                    if self.check(TokenKind::Op, Some("...")) {
                        self.advance();
                        let rest_tok = self.expect(TokenKind::Ident, None);
                        rest = Some(rest_tok.text);
                        self.eat_newlines();
                        break;
                    }
                    let name_tok = self.expect(TokenKind::Ident, None);
                    names.push(name_tok.text);
                    if self.match_tok(TokenKind::Comma, None).is_none() {
                        self.eat_newlines();
                        break;
                    }
                    self.eat_newlines();
                }
                self.expect(TokenKind::RParen, None);
                self.expect(TokenKind::Colon, None);
                let typ = self.parse_type();
                let pend = self.peek().start;
                let synth_name = format!("_p{}", synth_counter);
                synth_counter += 1;
                params.push(Param {
                    name: synth_name,
                    typ,
                    pattern: Some(ParamPattern::Tuple { names, rest }),
                    is_const: false,
                    range: self.make_range(pstart, pend),
                });
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_stmt_end();
                continue;
            }

            // Normal parameter: `name: Type`
            // `name: const int` — a per-parameter modifier, NOT part of the type
            // grammar. `const` is not a type (a `const int` IS an `int`), so
            // putting it in `parse_type` would make `const int[]`, `*const int`
            // and `Map<const int, V>` parse as though they meant something.
            let pname = self.expect(TokenKind::Ident, None).text;
            self.expect(TokenKind::Colon, None);
            let is_const = self.match_tok(TokenKind::Kw, Some("const")).is_some();
            let typ = self.parse_type();
            let pend = self.peek().start;
            params.push(Param {
                name: pname,
                typ,
                pattern: None,
                is_const,
                range: self.make_range(pstart, pend),
            });
            if self.match_tok(TokenKind::Comma, None).is_none() {
                break;
            }
            self.eat_stmt_end();
        }
        self.expect(TokenKind::RParen, None);
        // Tolerate a line break between the parameter list and what follows
        // (`-> (outputs)` or the body brace on the next line).
        self.eat_newlines();
        (params, rest_param)
    }

    fn parse_chip_outputs(&mut self) -> Vec<NamedOutput> {
        if self.check(TokenKind::LParen, None) {
            // Multiple named outputs: -> (name: type, ...)
            self.advance();
            let mut outs = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
                let ostart = self.peek().start;
                let oname = self.expect(TokenKind::Ident, None).text;
                self.expect(TokenKind::Colon, None);
                let typ = self.parse_type();
                let oend = self.peek().start;
                outs.push(NamedOutput {
                    name: oname,
                    typ,
                    range: self.make_range(ostart, oend),
                });
                self.eat_newlines();
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_newlines();
            }
            self.expect(TokenKind::RParen, None);
            outs
        } else {
            // Single anonymous output: -> type
            let ostart = self.peek().start;
            let typ = self.parse_type();
            let oend = self.peek().start;
            vec![NamedOutput {
                name: "_".into(),
                typ,
                range: self.make_range(ostart, oend),
            }]
        }
    }

    // `fn name(params) [-> ReturnType] = expr` — REMOVED.
    fn parse_fn_decl(&mut self) -> TopDecl {
        let kw = self.expect(TokenKind::Kw, Some("fn"));
        let start = kw.start;
        // The `fn` declaration form has been removed — use a `mod` with a return
        // value. Reject, but keep parsing the rest so a stray `fn` doesn't derail
        // the whole file (mirrors the removed `array`/`map` decl keywords).
        self.error(
            "`fn` declarations have been removed — use `mod NAME(params) -> T { return <expr> }` instead",
            kw.start,
            kw.end,
        );
        let name = self.expect(TokenKind::Ident, None).text;
        let (params, _rest) = self.parse_param_list();
        let return_type = if self.match_tok(TokenKind::Arrow, None).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(TokenKind::Op, Some("="));
        let body = self.parse_expr();
        let end = body.range().end;
        self.eat_stmt_end();
        TopDecl::Fn(FnDecl {
            name,
            params,
            return_type,
            body,
            range: self.make_range(start, end),
        })
    }
}
