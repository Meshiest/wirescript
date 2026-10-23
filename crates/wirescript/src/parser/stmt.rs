//! Blocks and statements.

use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_block(&mut self) -> Block {
        let start = self.expect(TokenKind::LBrace, None).start;
        if !self.enter_nesting() {
            self.leave_nesting(1);
            let end = self.peek().end;
            return Block { stmts: Vec::new(), range: self.make_range(start, end) };
        }
        self.eat_newlines();
        let mut stmts: Vec<Stmt> = Vec::new();
        while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
            let before = self.pos;
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
                    let file: std::sync::Arc<str> = self.file.into();
                    if decl_start != stmt_start.offset {
                        self.doc_comments.insert((file.clone(), decl_start), doc.clone());
                    }
                    self.doc_comments.insert((file, stmt_start.offset), doc);
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
            // A statement that consumed nothing would spin here. Recovery
            // paths deliberately leave a closing token in place (see
            // `parse_primary`'s error arm), so this is the backstop that turns
            // one into an exit rather than a hang.
            if self.pos == before {
                break;
            }
        }
        let end = self.expect(TokenKind::RBrace, None).end;
        self.leave_nesting(1);
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
            // A loop's microchip takes `@sliced` and a `@label`, nothing else.
            if kw("for") || self.at_let_for() {
                let fits = anns.side.is_none()
                    && anns.closed.is_none()
                    && anns.nofold.is_none()
                    && anns.invisible.is_none()
                    && anns.color.is_none()
                    && anns.material.is_none();
                if !fits {
                    self.error(
                        "a `for` loop takes no annotation besides @sliced and @label".to_string(),
                        t.start,
                        t2.start,
                    );
                }
                let mut s = if kw("for") { self.parse_for_stmt() } else { self.parse_let_for() };
                if let Stmt::For(f) = &mut s {
                    f.sliced = anns.sliced.is_some();
                    f.label = anns.label.map(|(l, _)| l);
                    f.label_expr = anns.label_expr.map(|(e, _)| e);
                }
                return Some(s);
            }
            if let Some(r) = &anns.sliced {
                self.error(
                    "@sliced must be followed by a `for` loop".to_string(),
                    r.start,
                    r.end,
                );
            }
            if kw("in") || kw("out") {
                if let Some(r) = &anns.closed {
                    self.error(
                        "@closed is not allowed on 'in'/'out' declarations".to_string(),
                        r.start,
                        r.end,
                    );
                }
                self.reject_style(&anns, "'in'/'out' declarations");
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
            if kw("chip") {
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
                let color = anns.color.map(|(e, _)| e);
                let material = anns.material.map(|(e, _)| e);
                let no_fold = anns.nofold.is_some();
                let mut decl =
                    self.parse_chip_decl(label, label_expr, anns.closed.is_some(), no_fold);
                set_style(&mut decl, color, material);
                match decl {
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
            // floating label, same as on a port — and so are `@color` and
            // `@material`. Any other annotation combined with these still falls
            // through to the generic "must be followed by ..." error below,
            // unchanged.
            let is_static_var = kw("static")
                && self.peek_at(1).kind == TokenKind::Kw
                && self.peek_at(1).text == "var";
            let no_side_closed_invisible =
                anns.side.is_none() && anns.closed.is_none() && anns.invisible.is_none();
            let styled = anns.color.is_some() || anns.material.is_some();
            let var_ann_ok = no_side_closed_invisible
                && (anns.nofold.is_some() || anns.label.is_some() || anns.label_expr.is_some() || styled);
            let bare_nofold = no_side_closed_invisible
                && anns.nofold.is_some()
                && anns.label.is_none()
                && anns.label_expr.is_none()
                && !styled;
            if (var_ann_ok && (kw("var") || is_static_var)) || (bare_nofold && (kw("let") || kw("on")))
            {
                let no_fold = anns.nofold.is_some();
                let label = anns.label.map(|(l, _)| l);
                let label_expr = anns.label_expr.map(|(e, _)| e);
                let color = anns.color.map(|(e, _)| e);
                let material = anns.material.map(|(e, _)| e);
                if is_static_var {
                    self.advance(); // consume "static"
                    let mut decl = self.parse_var_decl(true, no_fold, label, label_expr);
                    set_style(&mut decl, color, material);
                    if let TopDecl::Var(v) = decl {
                        return Some(Stmt::Var(v));
                    }
                    return None;
                }
                if kw("var") {
                    let mut decl = self.parse_var_decl(false, no_fold, label, label_expr);
                    set_style(&mut decl, color, material);
                    if let TopDecl::Var(v) = decl {
                        return Some(Stmt::Var(v));
                    }
                    return None;
                }
                if kw("let") {
                    let mut decl = self.parse_let_decl(false);
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
                if let TopDecl::Chip(c) = self.parse_mod_decl(false) {
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
            return self.parse_stmt(); // annotations consumed -> guaranteed progress
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
                    if self.at_let_for() {
                        return Some(self.parse_let_for());
                    }
                    let decl = self.parse_let_decl(false);
                    match decl {
                        TopDecl::Let(v) => return Some(Stmt::Let(v)),
                        TopDecl::LetElse(v) => return Some(Stmt::LetElse(v)),
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
                "const" => match self.peek_at(1).text.as_str() {
                    "mod" => {
                        self.advance(); // consume "const"
                        if let TopDecl::Chip(c) = self.parse_mod_decl(/*is_const=*/ true) {
                            return Some(Stmt::ChipDecl(c));
                        }
                    }
                    // `const chip` is rejected: a chip shares one compiled template
                    // across call sites, so "every parameter is const" is not a
                    // property of the declaration the way it is for an inlined mod.
                    // Const PARAMETERS on a chip are supported — they extend its
                    // template key.
                    "chip" => {
                        let kw = self.peek().clone();
                        self.error(
                            "`const chip` is not allowed. Use `const mod`, or mark \
                             individual parameters `const` (`chip C(name: const \
                             string, …)`)",
                            kw.start,
                            kw.end,
                        );
                        self.advance(); // consume "const"
                        match self.parse_chip_decl(None, None, false, false) {
                            TopDecl::AnonChip(ac) => return Some(Stmt::AnonChip(ac)),
                            TopDecl::Chip(c) => return Some(Stmt::ChipDecl(c)),
                            _ => {}
                        }
                    }
                    _ => {
                        let decl = self.parse_let_decl(/*is_const=*/ true);
                        match decl {
                            TopDecl::Let(v) => return Some(Stmt::Let(v)),
                            TopDecl::LetElse(v) => return Some(Stmt::LetElse(v)),
                            TopDecl::Await(a) => return Some(Stmt::Await(a)),
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
                },
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
                        let first = self.parse_expr();
                        // `return a, b, c` - parens optional on a tuple return.
                        // Collect the comma-separated list into the same
                        // index-keyed record `(a, b, c)` produces.
                        if self.check(TokenKind::Comma, None) {
                            let mut elements = vec![first];
                            while self.match_tok(TokenKind::Comma, None).is_some() {
                                if matches!(
                                    self.peek().kind,
                                    TokenKind::Newline
                                        | TokenKind::Semi
                                        | TokenKind::RBrace
                                        | TokenKind::Eof
                                ) {
                                    break;
                                }
                                elements.push(self.parse_expr());
                            }
                            let r0 = elements.first().unwrap().range().clone();
                            let rn = elements.last().unwrap().range().clone();
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
                            Some(Expr::RecordLit {
                                fields,
                                range: crate::diagnostic::SourceRange {
                                    file: r0.file,
                                    start: r0.start,
                                    end: rn.end,
                                },
                            })
                        } else {
                            Some(first)
                        }
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
                "match" => {
                    let start = t.start;
                    let expr = self.parse_match_expr();
                    let end = expr.range().end;
                    self.eat_stmt_end();
                    return Some(Stmt::ExprStmt(ExprStmt {
                        range: self.make_range(start, end),
                        expr,
                    }));
                }
                "chip" => match self.parse_chip_decl(None, None, false, false) {
                    TopDecl::AnonChip(ac) => return Some(Stmt::AnonChip(ac)),
                    TopDecl::Chip(c) => return Some(Stmt::ChipDecl(c)),
                    _ => {}
                },
                "mod" => {
                    if let TopDecl::Chip(c) = self.parse_mod_decl(false) {
                        return Some(Stmt::ChipDecl(c));
                    }
                }
                "for" => return Some(self.parse_for_stmt()),
                "break" => {
                    let kw = self.advance();
                    self.eat_stmt_end();
                    return Some(Stmt::Break {
                        range: self.make_range(kw.start, kw.end),
                    });
                }
                "continue" => {
                    let kw = self.advance();
                    self.eat_stmt_end();
                    return Some(Stmt::Continue {
                        range: self.make_range(kw.start, kw.end),
                    });
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
            binding_type: None,
            destructure: None,
            tuple_destructure: None,
            value_expr,
            exec_expr,
            no_fold: false,
            range: self.make_range(start, end),
        })
    }

    pub(super) fn parse_if_stmt(&mut self) -> Stmt {
        let start = self.expect(TokenKind::Kw, Some("if")).start;
        if self.check(TokenKind::Kw, Some("let")) {
            self.advance();
            let pattern = self.parse_pattern();
            self.expect(TokenKind::Op, Some("="));
            // Header position: a trailing `{` opens the then-block, so
            // suppress braced variant construction, same as the scrutinee of
            // `match`/plain `if` (`if let Some(x) = f.bar { }` is not
            // `f.bar { }`).
            let scrutinee = self.parse_expr_no_brace_construct();
            let then_block = self.parse_block();
            let else_block = self.parse_if_else_tail();
            let end = else_block
                .as_ref()
                .map(|b| b.range.end)
                .unwrap_or(then_block.range.end);
            return Stmt::IfLet(IfLet {
                pattern,
                scrutinee,
                then_block,
                else_block,
                range: self.make_range(start, end),
            });
        }
        // Header position: a trailing `{` opens the body block, so suppress
        // braced variant construction (`if f.bar { }` is not `f.bar { }`).
        let cond = self.parse_expr_no_brace_construct();
        let then_block = self.parse_block();
        let else_block = self.parse_if_else_tail();
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

    /// The optional `else { ... }` / `else if ...` tail shared by `if` and
    /// `if let`. An `else if`/`else if let` recurses into `parse_if_stmt` and
    /// wraps the resulting `Stmt::If`/`Stmt::IfLet` as the sole statement of
    /// a synthetic block (so the AST shape stays a plain `Option<Block>`
    /// regardless of which form the chained branch took).
    fn parse_if_else_tail(&mut self) -> Option<Block> {
        self.eat_newlines();
        if self.match_tok(TokenKind::Kw, Some("else")).is_some() {
            self.eat_newlines();
            if self.check(TokenKind::Kw, Some("if")) {
                // `else if` chains nest an `If` per arm, so the chain length is
                // a tree depth even though it reads as a flat ladder.
                if !self.enter_nesting() {
                    self.leave_nesting(1);
                    return None;
                }
                let inner = self.parse_if_stmt();
                self.leave_nesting(1);
                let r = match &inner {
                    Stmt::If(i) => i.range.clone(),
                    Stmt::IfLet(i) => i.range.clone(),
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
        }
    }

    /// `for <pat> [, <pat>]* in <source> [as <name>] { <body> }`
    fn parse_for_stmt(&mut self) -> Stmt {
        self.parse_for_with_result(None, None)
    }

    /// Whether the next tokens open `let <name> = for`.
    fn at_let_for(&self) -> bool {
        let tok = |i: usize, kind: TokenKind, text: &str| {
            let t = self.peek_at(i);
            t.kind == kind && t.text == text
        };
        tok(0, TokenKind::Kw, "let")
            && self.peek_at(1).kind == TokenKind::Ident
            && tok(2, TokenKind::Op, "=")
            && tok(3, TokenKind::Kw, "for")
    }

    /// `let run = for ...`: the loop, with `run` bound to its outcome record
    /// after it ends. `start` is the `let`, so the statement spans both.
    pub(super) fn parse_let_for(&mut self) -> Stmt {
        let start = self.expect(TokenKind::Kw, Some("let")).start;
        let name = self.expect(TokenKind::Ident, None);
        let name_range = self.make_range(name.start, name.end);
        self.expect(TokenKind::Op, Some("="));
        self.parse_for_with_result(Some((name.text, name_range)), Some(start))
    }

    fn parse_for_with_result(
        &mut self,
        mut result: Option<(String, SourceRange)>,
        let_start: Option<Pos>,
    ) -> Stmt {
        let kw = self.expect(TokenKind::Kw, Some("for"));
        let start = let_start.unwrap_or(kw.start);

        let mut bindings = vec![self.parse_for_pat()];
        while self.match_tok(TokenKind::Comma, None).is_some() {
            bindings.push(self.parse_for_pat());
        }

        self.expect(TokenKind::Kw, Some("in"));
        // A bare `as` ends the source (`Parser::stop_at_as`): the retired
        // `for .. as run` form lands below rather than reading as a conversion.
        let saved_as = std::mem::replace(&mut self.stop_at_as, true);
        let source = self.parse_for_source();
        self.stop_at_as = saved_as;

        if let Some(as_tok) = self.match_tok(TokenKind::Kw, Some("as")) {
            let name = self.expect(TokenKind::Ident, None);
            self.error(
                format!(
                    "`for ... as {0}` is now written `let {0} = for ...`; to convert the loop \
                     source, put it in parentheses: `for x in (xs as T)`",
                    name.text
                ),
                as_tok.start,
                name.end,
            );
            if result.is_none() {
                result = Some((name.text.clone(), self.make_range(name.start, name.end)));
            }
        }

        let body = self.parse_block();
        let end = body.range.end;
        Stmt::For(ForStmt {
            bindings,
            source,
            result,
            sliced: false,
            label: None,
            label_expr: None,
            body,
            range: self.make_range(start, end),
        })
    }

    /// One binding slot: `name`, `_`, or `{ a, b: alias }`.
    fn parse_for_pat(&mut self) -> ForPat {
        // `{ a, b: alias, ...rest }` - the same record-destructure shape
        // `let { .. } = ..` uses, via the shared per-field parser.
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
                    break;
                }
                fields.push(self.parse_record_destruct_field());
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_newlines();
            }
            let brace_end = self.expect(TokenKind::RBrace, None).end;
            return ForPat::Record {
                fields,
                range: self.make_range(brace_start, brace_end),
            };
        }
        let t = self.peek().clone();
        if t.kind == TokenKind::Ident && t.text == "_" {
            self.advance();
            return ForPat::Ignore(self.make_range(t.start, t.end));
        }
        let name = self.expect(TokenKind::Ident, None);
        ForPat::Name(name.text.clone(), self.make_range(name.start, name.end))
    }

    /// The `for` source position, where `..` reads as a range rather than as
    /// string concatenation. Nothing else in the grammar reinterprets `..`.
    fn parse_for_source(&mut self) -> ForSource {
        let start = self.peek().start;

        // `count(...)` - contextual, so `count` stays an ordinary identifier
        // everywhere else. Only treated as the range spelling when it is
        // immediately followed by `(`.
        if self.peek().kind == TokenKind::Ident
            && self.peek().text == "count"
            && self.peek_at(1).kind == TokenKind::LParen
        {
            return ForSource::Range(self.parse_count_range(start));
        }

        // Parse the first endpoint with `..` excluded from ITS OWN operator
        // search (`parse_range_operand`, backed by `Parser::stop_at_dotdot`):
        // `..` is tied with `+`/`-` at precedence 10 and left-associative, so
        // parsing this the ordinary way would group `0..n-1` as `(0..n) - 1`
        // instead of `0 .. (n-1)`. Excluding `..` here means `+`/`-` (and
        // everything tighter) still fully group into this one endpoint, and
        // the loop simply stops the moment it reaches a bare `..` rather than
        // treating it as a same-tier continuation.
        let left = self.parse_range_operand();

        // `..` may follow the endpoint after a line break, like any infix
        // operator. The position is saved and restored so a source with no
        // `..` never consumes the newline, or the next statement would be absorbed.
        let saved = self.pos;
        while self.peek().kind == TokenKind::Newline {
            self.advance();
        }
        if self.check(TokenKind::Op, Some("..")) {
            self.advance();
            while self.peek().kind == TokenKind::Newline {
                self.advance();
            }
            // Parse the other side the same way, so `start+1..end-1` groups
            // each side's arithmetic correctly on both ends.
            let right = self.parse_range_operand();
            let end = right.range().end;
            return ForSource::Range(RangeSource {
                extent: RangeExtent::Bounds { start: left, end: right },
                step: None,
                reversed: false,
                range: self.make_range(start, end),
            });
        }
        self.pos = saved;

        // Not a bare range: either a parenthesized range, optionally with
        // `.step(..)` / `.rev()` (`(a..b).step(2)` parsed its own `..`
        // unrestricted, inside the parens - see `parse_expr_delimited`), or a
        // plain array/map collection expression.
        let end = left.range().end;
        match self.range_from_expr(left, start, end) {
            Ok(r) => ForSource::Range(r),
            Err(expr) => ForSource::Collection {
                range: self.make_range(start, end),
                expr,
            },
        }
    }

    /// Reinterpret a parsed expression as a range, or hand back its innermost
    /// form when it is not one - the array/map case.
    ///
    /// Peeling a `.step()`/`.rev()` discards the wrapper, so nothing later can
    /// see one that was applied to an array or map. WS083 is reported here, at
    /// the only point where the wrapper still exists.
    fn range_from_expr(&mut self, expr: Expr, start: Pos, end: Pos) -> Result<RangeSource, Expr> {
        let mut step: Option<Expr> = None;
        let mut reversed = false;
        let mut cur = expr;
        // Every peeled method, outermost first: what a range-only method
        // applied to a non-range source is reported at.
        let mut peeled: Vec<(&'static str, SourceRange)> = Vec::new();

        // Peel `.step(k)` and `.rev()` off the outside, innermost last. A
        // method call parses as `Call { callee: FieldAccess, .. }` - there is
        // no separate method-call node in this AST.
        loop {
            let is_step = match &cur {
                Expr::Call { callee, args, type_args, .. } if type_args.is_empty() => {
                    match callee.as_ref() {
                        Expr::FieldAccess { field, .. }
                            if field == "step"
                                && args.len() == 1
                                && matches!(args[0], CallArg::Positional(_)) =>
                        {
                            Some(true)
                        }
                        Expr::FieldAccess { field, .. } if field == "rev" && args.is_empty() => {
                            Some(false)
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            let Some(is_step) = is_step else { break };
            let Expr::Call { callee, args, range, .. } = cur else {
                unreachable!("shape just matched above")
            };
            let Expr::FieldAccess { obj, .. } = *callee else {
                unreachable!("shape just matched above")
            };
            if is_step {
                let Some(CallArg::Positional(v)) = args.into_iter().next() else {
                    unreachable!("shape just matched above")
                };
                step = Some(v);
                peeled.push(("step", range));
            } else {
                reversed = true;
                peeled.push(("rev", range));
            }
            cur = *obj;
        }

        match cur {
            Expr::BinOp { op, left, right, .. } if op == ".." => {
                // Each peel overwrites the one below it rather than combining
                // with it, so a repeated method silently keeps the innermost.
                if let Some((method, range)) = peeled
                    .iter()
                    .enumerate()
                    .find(|(i, (m, _))| peeled[i + 1..].iter().any(|(n, _)| n == m))
                    .map(|(_, p)| p)
                {
                    self.error_code(
                        "WS083",
                        format!(
                            "a range takes one `.{method}()`. A second one replaces the \
                             first rather than combining with it; keep the one you want."
                        ),
                        range.clone(),
                    );
                }
                Ok(RangeSource {
                    extent: RangeExtent::Bounds { start: *left, end: *right },
                    step,
                    reversed,
                    range: self.make_range(start, end),
                })
            }
            // Not a range. Hand back the innermost expression; the caller
            // reports the array/map `Collection` case.
            other => {
                if let Some((method, range)) = peeled.first() {
                    self.error_code(
                        "WS083",
                        format!(
                            "`.{method}()` applies to a range, not to an array or map. \
                             Give the loop a range source (`for i in 0..n`), or drop \
                             the `.{method}()`."
                        ),
                        range.clone(),
                    );
                }
                Err(other)
            }
        }
    }

    /// `count(n)`, `count(n, from, step)`, `count(n, from = a, step = b)`.
    fn parse_count_range(&mut self, start: Pos) -> RangeSource {
        self.advance(); // `count`
        self.expect(TokenKind::LParen, None);
        let mut positional: Vec<Expr> = Vec::new();
        let mut from: Option<Expr> = None;
        let mut step: Option<Expr> = None;
        self.eat_newlines();
        loop {
            if self.check(TokenKind::RParen, None) || self.peek().kind == TokenKind::Eof {
                break;
            }
            // `name = expr` named argument. `from` is already a reserved
            // keyword (`import x from y`) rather than a plain identifier, so
            // it has to be accepted by text here in addition to `step`, which
            // stays an ordinary `Ident` since it is not reserved anywhere else.
            if (self.peek().kind == TokenKind::Ident
                || (self.peek().kind == TokenKind::Kw && self.peek().text == "from"))
                && self.peek_at(1).kind == TokenKind::Op
                && self.peek_at(1).text == "="
            {
                let name_tok = self.advance();
                self.advance(); // `=`
                let value = self.parse_expr();
                match name_tok.text.as_str() {
                    "from" => from = Some(value),
                    "step" => step = Some(value),
                    _ => self.error(
                        format!("`count` takes `from` and `step`, not `{}`", name_tok.text),
                        name_tok.start,
                        name_tok.end,
                    ),
                }
            } else {
                positional.push(self.parse_expr());
            }
            self.eat_newlines();
            if self.match_tok(TokenKind::Comma, None).is_none() {
                break;
            }
            self.eat_newlines();
        }
        let close = self.expect(TokenKind::RParen, None);
        // Positional order is (count, from, step).
        let mut it = positional.into_iter();
        let count = it.next().unwrap_or_else(|| Expr::IntLit {
            value: 0,
            text: "0".into(),
            range: self.make_range(start, close.end),
        });
        from = from.or_else(|| it.next());
        step = step.or_else(|| it.next());
        RangeSource {
            extent: RangeExtent::Count { count, from },
            step,
            reversed: false,
            range: self.make_range(start, close.end),
        }
    }
}
