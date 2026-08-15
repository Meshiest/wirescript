//! Blocks and statements.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_block(&mut self) -> Block {
        let start = self.expect(TokenKind::LBrace, None).start;
        self.eat_newlines();
        let mut stmts: Vec<Stmt> = Vec::new();
        while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
            let doc = self.collect_doc_comment();
            let stmt_start = self.peek().start;
            if let Some(s) = self.parse_stmt() {
                if let Some(doc) = doc {
                    // Key by the statement's own range start — this is the
                    // offset lowering looks doc comments up by (e.g. a chip
                    // decl's range starts at the `chip` keyword). When the
                    // statement begins with annotations (`@label(...)`,
                    // `@closed`, `@left`/etc.), `stmt_start` is the `@` token
                    // instead, which no lookup ever queries — so also key
                    // under `stmt_start` (harmless when it's the same offset)
                    // to keep this the safest, most permissive insertion.
                    let decl_start = s.range().start.offset;
                    if decl_start != stmt_start.offset {
                        self.doc_comments.insert(decl_start, doc.clone());
                    }
                    self.doc_comments.insert(stmt_start.offset, doc);
                }
                // Drain any synthetic let bindings queued by parse_handler
                // (expression triggers).  They must appear *before* the handler.
                let pending: Vec<Stmt> = self.pending_stmts.drain(..).collect();
                stmts.extend(pending);
                stmts.push(s);
            } else {
                self.synchronize();
            }
            self.eat_newlines();
        }
        let end = self.expect(TokenKind::RBrace, None).end;
        Block {
            stmts,
            range: self.make_range(start, end),
        }
    }

    pub(super) fn parse_stmt(&mut self) -> Option<Stmt> {
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
                    if let TopDecl::In(i) = self.parse_in_decl(side, label, label_expr, invisible)
                    {
                        return Some(Stmt::In(i));
                    }
                    return None;
                }
                return Some(Stmt::OutBinding(
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
                let (open, closed) = if next_is_open_chip {
                    if let Some(r) = &anns.closed {
                        self.error(
                            "@closed cannot be combined with 'open chip'".to_string(),
                            r.start,
                            r.end,
                        );
                    }
                    self.advance(); // consume "open"
                    (true, false)
                } else {
                    (false, anns.closed.is_some())
                };
                match self.parse_chip_decl(open, label, label_expr, closed, no_fold) {
                    TopDecl::AnonChip(ac) => {
                        if let Some(r) = &anns.nofold {
                            self.warn(
                                "@nofold has no effect on an anonymous chip",
                                r.start,
                                r.end,
                            );
                        }
                        return Some(Stmt::AnonChip(ac));
                    }
                    TopDecl::Chip(c) => return Some(Stmt::ChipDecl(c)),
                    _ => return None,
                }
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
                    if let TopDecl::Var(v) = self.parse_var_decl(true, no_fold, label, label_expr)
                    {
                        return Some(Stmt::Var(v));
                    }
                    return None;
                }
                if kw("var") {
                    if let TopDecl::Var(v) =
                        self.parse_var_decl(false, no_fold, label, label_expr)
                    {
                        return Some(Stmt::Var(v));
                    }
                    return None;
                }
                if kw("let") {
                    let mut decl = self.parse_let_decl();
                    match &mut decl {
                        TopDecl::Let(l) => l.no_fold = true,
                        TopDecl::Event(e) => e.no_fold = true,
                        TopDecl::Await(a) => a.no_fold = true,
                        _ => {}
                    }
                    return match decl {
                        TopDecl::Let(v) => Some(Stmt::Let(v)),
                        TopDecl::Await(a) => Some(Stmt::Await(a)),
                        TopDecl::Event(e) => {
                            self.error(
                                "captured events (`let x = on Event { … }`) are only allowed at the top level"
                                    .to_string(),
                                e.range.start,
                                e.range.end,
                            );
                            None
                        }
                        _ => None,
                    };
                }
                return Some(Stmt::Handler(self.parse_handler(true)));
            }
            if kw("mod") {
                self.error(
                    "annotations are not allowed on 'mod' declarations".to_string(),
                    t.start,
                    t2.end,
                );
                if let TopDecl::Chip(c) = self.parse_mod_decl() {
                    return Some(Stmt::ChipDecl(c));
                }
                return None;
            }
            self.error(
                "an annotation must be followed by an 'in', 'out', or chip declaration \
                 (a bare @nofold is also allowed before 'var', 'static var', 'let', or 'on')"
                    .to_string(),
                t.start,
                t2.end,
            );
            if matches!(t2.kind, TokenKind::Eof | TokenKind::RBrace) {
                return None;
            }
            return self.parse_stmt(); // annotations consumed → guaranteed progress
        }
        if t.kind == TokenKind::Kw {
            match t.text.as_str() {
                "var" => {
                    if let TopDecl::Var(v) = self.parse_var_decl(false, false, None, None) {
                        return Some(Stmt::Var(v));
                    }
                }
                "static" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "var" {
                        self.advance();
                        if let TopDecl::Var(v) = self.parse_var_decl(true, false, None, None) {
                            return Some(Stmt::Var(v));
                        }
                    }
                }
                "buffer" => {
                    // `buffer(...) emit` / `buffer emit` is the emit modifier;
                    // `buffer name = ...` is the value declaration.
                    let next = self.peek_at(1);
                    if next.kind == TokenKind::LParen
                        || (next.kind == TokenKind::Kw && next.text == "emit")
                    {
                        return Some(self.parse_buffered_emit());
                    }
                    if let TopDecl::Buffer(v) = self.parse_buffer_decl() {
                        return Some(Stmt::Buffer(v));
                    }
                }
                "out" => {
                    return Some(Stmt::OutBinding(
                        self.parse_out_binding(None, None, None, false, false),
                    ));
                }
                "let" => {
                    let decl = self.parse_let_decl();
                    match decl {
                        TopDecl::Let(v) => return Some(Stmt::Let(v)),
                        TopDecl::Await(a) => return Some(Stmt::Await(a)),
                        // A captured event parsed here has already consumed its
                        // whole body; without an explicit `return None` control
                        // fell through to the expression-statement fallback,
                        // which re-parsed the trailing tokens into two unrelated
                        // errors that never named the real problem.
                        TopDecl::Event(e) => {
                            self.error(
                                "captured events (`let x = on Event { … }`) are only allowed at the top level"
                                    .to_string(),
                                e.range.start,
                                e.range.end,
                            );
                            return None;
                        }
                        _ => {}
                    }
                }
                "array" => {
                    if let TopDecl::Array(a) = self.parse_array_decl() {
                        return Some(Stmt::Array(a));
                    }
                }
                "map" => {
                    if let TopDecl::Map(m) = self.parse_map_decl() {
                        return Some(Stmt::Map(m));
                    }
                }
                "in" => {
                    if let TopDecl::In(i) = self.parse_in_decl(None, None, None, false) {
                        return Some(Stmt::In(i));
                    }
                }
                "on" => return Some(Stmt::Handler(self.parse_handler(false))),
                "emit" => return Some(self.parse_emit()),
                "await" => return Some(self.parse_await_stmt()),
                "return" => {
                    let tok = self.advance();
                    let value = if !matches!(
                        self.peek().kind,
                        TokenKind::Newline | TokenKind::Semi | TokenKind::RBrace | TokenKind::Eof
                    ) {
                        Some(self.parse_expr())
                    } else {
                        None
                    };
                    let end = self.peek().start;
                    self.eat_stmt_end();
                    return Some(Stmt::Return {
                        value,
                        range: self.make_range(tok.start, end),
                    });
                }
                "if" => return Some(self.parse_if_stmt()),
                "chip" => match self.parse_chip_decl(false, None, None, false, false) {
                    TopDecl::AnonChip(ac) => return Some(Stmt::AnonChip(ac)),
                    TopDecl::Chip(c) => return Some(Stmt::ChipDecl(c)),
                    _ => {}
                },
                "open" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "chip" {
                        self.advance();
                        if let TopDecl::AnonChip(ac) =
                            self.parse_chip_decl(true, None, None, false, false)
                        {
                            return Some(Stmt::AnonChip(ac));
                        }
                    }
                }
                "mod" => {
                    if let TopDecl::Chip(c) = self.parse_mod_decl() {
                        return Some(Stmt::ChipDecl(c));
                    }
                }
                _ => {}
            }
        }
        // assignment or expression statement.
        let start = self.peek().start;
        let lhs = self.parse_expr();
        if self.match_tok(TokenKind::Op, Some("=")).is_some() {
            let rhs = self.parse_expr();
            let end = rhs.range().end;
            self.eat_stmt_end();
            return Some(Stmt::Assign(Assign {
                target: lhs,
                value: rhs,
                range: self.make_range(start, end),
            }));
        }
        // Compound assignment: += -= *= /= %= &= |= ^= <<= >>=
        let compound_ops = &["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="];
        for &cop in compound_ops {
            if self.match_tok(TokenKind::Op, Some(cop)).is_some() {
                let base_op = cop.trim_end_matches('=');
                let rhs = self.parse_expr();
                let end = rhs.range().end;
                let range = self.make_range(start, end);
                let value = Expr::BinOp {
                    op: base_op.into(),
                    left: Box::new(lhs.clone()),
                    right: Box::new(rhs),
                    range: range.clone(),
                };
                self.eat_stmt_end();
                return Some(Stmt::Assign(Assign {
                    target: lhs,
                    value,
                    range,
                }));
            }
        }
        let end = lhs.range().end;
        self.eat_stmt_end();
        if let Some(a) = gate_builtin_assign(&lhs) {
            return Some(Stmt::Assign(a));
        }
        Some(Stmt::ExprStmt(ExprStmt {
            range: self.make_range(start, end),
            expr: lhs,
        }))
    }

    fn parse_emit(&mut self) -> Stmt {
        let start = self.expect(TokenKind::Kw, Some("emit")).start;
        let name_tok = self.expect(TokenKind::Ident, None);
        let value = if self.check(TokenKind::Op, Some("=")) {
            self.advance();
            Some(self.parse_expr())
        } else {
            None
        };
        let end = value.as_ref().map_or(name_tok.end, |v| v.range().end);
        self.eat_stmt_end();
        Stmt::Emit(Emit {
            name: name_tok.text,
            value,
            buffer: None,
            range: self.make_range(start, end),
        })
    }

    /// `buffer(delay[, hold]) emit name [= value]` — a buffered emit. The
    /// spec's Buffer gate delays the emit's exec (the tick-crossing barrier
    /// that legalises loop back-edges).
    fn parse_buffered_emit(&mut self) -> Stmt {
        let spec = self.parse_buffer_spec();
        let stmt = self.parse_emit();
        match stmt {
            Stmt::Emit(mut e) => {
                e.range = self.make_range(spec.range.start, e.range.end);
                e.buffer = Some(spec);
                Stmt::Emit(e)
            }
            other => other,
        }
    }

    /// `buffer(delay[, hold])` with an optional `s` unit after each duration
    /// (`buffer(0.5s)`, `buffer(myVar s)` — seconds; unadorned = ticks), or
    /// bare `buffer` (before `emit`) — one tick.
    fn parse_buffer_spec(&mut self) -> BufferSpec {
        let buffer_tok = self.expect(TokenKind::Kw, Some("buffer"));
        let start = buffer_tok.start;
        if !self.check(TokenKind::LParen, None) {
            return BufferSpec {
                delay: None,
                hold: None,
                seconds: false,
                range: self.make_range(start, buffer_tok.end),
            };
        }
        self.advance(); // consume `(`
        let delay = self.parse_expr();
        let mut seconds = self.eat_seconds_unit();
        let hold = if self.match_tok(TokenKind::Comma, None).is_some() {
            let h = self.parse_expr();
            seconds |= self.eat_seconds_unit();
            Some(h)
        } else {
            None
        };
        let end = self.expect(TokenKind::RParen, None).end;
        BufferSpec {
            delay: Some(delay),
            hold,
            seconds,
            range: self.make_range(start, end),
        }
    }

    /// Consume a trailing `s` seconds-unit marker after a duration expression.
    fn eat_seconds_unit(&mut self) -> bool {
        if self.peek().kind == TokenKind::Ident && self.peek().text == "s" {
            self.advance();
            return true;
        }
        false
    }

    fn parse_await_stmt(&mut self) -> Stmt {
        let start = self.expect(TokenKind::Kw, Some("await")).start;
        let s = self.parse_await_inner(start, None);
        self.eat_stmt_end();
        s
    }

    pub(super) fn parse_await_inner(&mut self, start: Pos, binding: Option<String>) -> Stmt {
        let first_expr = self.parse_expr();
        let (value_expr, exec_expr) = if self.check(TokenKind::Kw, Some("on")) {
            self.advance();
            let exec = self.parse_expr();
            (Some(first_expr), exec)
        } else {
            (None, first_expr)
        };
        let end = exec_expr.range().end;
        Stmt::Await(AwaitStmt {
            binding,
            destructure: None,
            value_expr,
            exec_expr,
            no_fold: false,
            range: self.make_range(start, end),
        })
    }

    pub(super) fn parse_if_stmt(&mut self) -> Stmt {
        let start = self.expect(TokenKind::Kw, Some("if")).start;
        let cond = self.parse_expr();
        let then_block = self.parse_block();
        self.eat_newlines();
        let else_block = if self.match_tok(TokenKind::Kw, Some("else")).is_some() {
            self.eat_newlines();
            if self.check(TokenKind::Kw, Some("if")) {
                let inner = self.parse_if_stmt();
                let r = match &inner {
                    Stmt::If(i) => i.range.clone(),
                    _ => unreachable!(),
                };
                Some(Block {
                    stmts: vec![inner],
                    range: r,
                })
            } else {
                Some(self.parse_block())
            }
        } else {
            None
        };
        let end = else_block
            .as_ref()
            .map(|b| b.range.end)
            .unwrap_or(then_block.range.end);
        Stmt::If(If {
            cond,
            then_block,
            else_block,
            range: self.make_range(start, end),
        })
    }
}
