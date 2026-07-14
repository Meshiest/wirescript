//! Hand-written recursive-descent + Pratt parser for wirescript.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Pos, Severity, SourceRange};
use crate::lexer::{InterpPart as LexInterpPart, Token, TokenKind, TokenValue, lex};

use std::collections::HashMap;

fn shift_pos(p: &mut Pos, origin: &Pos) {
    p.offset += origin.offset;
    p.line = p.line.saturating_sub(1) + origin.line;
    if p.line == origin.line {
        p.col = p.col.saturating_sub(1) + origin.col;
    }
}

fn shift_expr_offsets(expr: &mut Expr, origin: Pos) {
    {
        let r = expr.range_mut();
        shift_pos(&mut r.start, &origin);
        shift_pos(&mut r.end, &origin);
    }
    match expr {
        Expr::FieldAccess { obj, .. } => shift_expr_offsets(obj, origin),
        Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
            shift_expr_offsets(operand, origin);
        }
        Expr::IndexAccess { obj, index, .. } => {
            shift_expr_offsets(obj, origin);
            shift_expr_offsets(index, origin);
        }
        Expr::TuplePick { obj, .. } => shift_expr_offsets(obj, origin),
        Expr::UnOp { operand, .. } => shift_expr_offsets(operand, origin),
        Expr::BinOp { left, right, .. } => {
            shift_expr_offsets(left, origin);
            shift_expr_offsets(right, origin);
        }
        Expr::Call { callee, args, .. } => {
            shift_expr_offsets(callee, origin);
            for a in args {
                match a {
                    CallArg::Positional(e) => shift_expr_offsets(e, origin),
                    CallArg::Named { value, .. } => shift_expr_offsets(value, origin),
                    CallArg::Spread(e) => shift_expr_offsets(e, origin),
                }
            }
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            shift_expr_offsets(cond, origin);
            shift_expr_offsets(then_branch, origin);
            shift_expr_offsets(else_branch, origin);
        }
        Expr::MatchExpr { scrutinee, .. } => {
            shift_expr_offsets(scrutinee, origin);
        }
        Expr::Array { elements, .. } => {
            for el in elements {
                shift_expr_offsets(el.expr_mut(), origin);
            }
        }
        _ => {}
    }
}

pub struct ParseResult {
    pub ast: Script,
    pub diagnostics: Vec<Diagnostic>,
    /// Doc comments keyed by the start offset of the declaration they precede.
    pub doc_comments: HashMap<usize, String>,
}

pub fn parse(source: &str, file: &str) -> ParseResult {
    let lexed = lex(source, file);
    let mut p = Parser::new(lexed.tokens, file, lexed.diagnostics);
    let script = p.parse_script();
    ParseResult {
        ast: script,
        diagnostics: p.diagnostics,
        doc_comments: p.doc_comments,
    }
}

// ---------- operator precedence table ----------

/// Higher number = tighter binding. Mirrors the TS table.
fn infix_prec(op: &str) -> Option<u8> {
    match op {
        "||" | "^^" => Some(2),
        "&&" => Some(3),
        "|" => Some(4),
        "^" => Some(5),
        "&" => Some(6),
        "==" | "!=" => Some(7),
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

fn trigger_to_expr(t: &Trigger) -> Expr {
    match t {
        Trigger::Ident { name, range } => Expr::Ident {
            name: name.clone(),
            range: range.clone(),
        },
        Trigger::Field { obj, field, range } => Expr::FieldAccess {
            obj: Box::new(Expr::Ident {
                name: obj.clone(),
                range: range.clone(),
            }),
            field: field.clone(),
            range: range.clone(),
        },
        Trigger::Not { inner, range } => Expr::UnOp {
            op: "!".into(),
            operand: Box::new(trigger_to_expr(inner)),
            range: range.clone(),
        },
        Trigger::Union { parts, range } => {
            if let Some(first) = parts.first() {
                trigger_to_expr(first)
            } else {
                Expr::Ident {
                    name: String::new(),
                    range: range.clone(),
                }
            }
        }
    }
}

// ---------- parser state ----------

/// Annotations consumed before a declaration. Each keeps its source range
/// for error reporting at the consuming site.
#[derive(Default)]
struct ParsedAnnotations {
    side: Option<(PortSide, SourceRange)>,
    label: Option<(String, SourceRange)>,
    closed: Option<SourceRange>,
}

struct Parser<'a> {
    tokens: Vec<Token>,
    file: &'a str,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    doc_comments: HashMap<usize, String>,
    /// Counter for generating unique synthetic binding names (`_on_expr_N`).
    expr_trigger_counter: usize,
    /// Synthetic `let` bindings queued by `parse_handler` for expression
    /// triggers.  The surrounding `parse_block` / `parse_script` loops drain
    /// this before inserting the handler itself.
    pending_stmts: Vec<Stmt>,
}

impl<'a> Parser<'a> {
    fn new(tokens: Vec<Token>, file: &'a str, initial: Vec<Diagnostic>) -> Self {
        Self {
            tokens,
            file,
            pos: 0,
            diagnostics: initial,
            doc_comments: HashMap::new(),
            expr_trigger_counter: 0,
            pending_stmts: Vec::new(),
        }
    }

    fn collect_doc_comment(&mut self) -> Option<String> {
        let mut lines = Vec::new();
        while self.peek().kind == TokenKind::DocComment {
            lines.push(self.peek().text.clone());
            self.advance();
            // Skip newline after doc comment
            while self.peek().kind == TokenKind::Newline {
                self.advance();
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    // --- token helpers ---

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("at least EOF"))
    }

    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens
            .get(self.pos + offset)
            .unwrap_or_else(|| self.tokens.last().expect("at least EOF"))
    }

    #[allow(dead_code)]
    fn peek_non_nl(&self) -> &Token {
        let mut i = self.pos;
        while i < self.tokens.len() && self.tokens[i].kind == TokenKind::Newline {
            i += 1;
        }
        self.tokens
            .get(i)
            .unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn eat_newlines(&mut self) {
        while self.peek().kind == TokenKind::Newline {
            self.pos += 1;
        }
    }

    fn advance(&mut self) -> Token {
        if self.pos >= self.tokens.len() {
            if let Some(last) = self.tokens.last() {
                return last.clone();
            }
            return Token {
                kind: TokenKind::Eof,
                text: String::new(),
                start: Default::default(),
                end: Default::default(),
                value: None,
            };
        }
        let t = self.tokens[self.pos].clone();
        self.pos += 1;
        t
    }

    fn check(&self, kind: TokenKind, text: Option<&str>) -> bool {
        let t = self.peek();
        if t.kind != kind {
            return false;
        }
        text.is_none_or(|s| t.text == s)
    }

    fn match_tok(&mut self, kind: TokenKind, text: Option<&str>) -> Option<Token> {
        if self.check(kind, text) {
            Some(self.advance())
        } else {
            None
        }
    }

    fn expect(&mut self, kind: TokenKind, text: Option<&str>) -> Token {
        if self.check(kind, text) {
            return self.advance();
        }
        let t = self.peek().clone();
        let want = text
            .map(|s| format!("'{s}'"))
            .unwrap_or_else(|| format!("{:?}", kind));
        self.error(
            format!("expected {want}, got '{}' ({:?})", t.text, t.kind),
            t.start,
            t.end,
        );
        Token {
            kind,
            text: text.unwrap_or("").to_string(),
            start: t.start,
            end: t.end,
            value: None,
        }
    }

    fn eat_stmt_end(&mut self) {
        while self.check(TokenKind::Newline, None) || self.check(TokenKind::Semi, None) {
            self.advance();
        }
    }

    fn make_range(&self, start: Pos, end: Pos) -> SourceRange {
        SourceRange::new(self.file, start, end)
    }

    fn error(&mut self, message: impl Into<String>, start: Pos, end: Pos) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "WSP001".to_string(),
            message: message.into(),
            range: self.make_range(start, end),
        });
    }

    fn synchronize(&mut self) {
        while self.peek().kind != TokenKind::Eof {
            let t = self.peek();
            if matches!(
                t.kind,
                TokenKind::Newline | TokenKind::Semi | TokenKind::RBrace
            ) {
                self.advance();
                return;
            }
            if t.kind == TokenKind::Kw
                && matches!(
                    t.text.as_str(),
                    "var"
                        | "array"
                        | "buffer"
                        | "fn"
                        | "chip"
                        | "mod"
                        | "on"
                        | "in"
                        | "out"
                        | "let"
                        | "if"
                        | "static"
                )
            {
                return;
            }
            self.advance();
        }
    }

    // ---------- top level ----------

    fn parse_script(&mut self) -> Script {
        let start = self.peek().start;
        let mut decls: Vec<TopDecl> = Vec::new();
        self.eat_newlines();
        // A leading `///` block separated from the first declaration by a blank
        // line documents the module, not the first decl — so it doesn't merge
        // into it.
        let module_doc = self.collect_module_doc();
        while self.peek().kind != TokenKind::Eof {
            let doc = self.collect_doc_comment();
            let before = self.pos;
            if let Some(d) = self.parse_top_decl() {
                // Drain any synthetic let bindings queued by parse_handler
                // (expression triggers).  They must appear *before* the handler
                // itself in the declaration list.
                let pending: Vec<Stmt> = self.pending_stmts.drain(..).collect();
                for stmt in pending {
                    if let Stmt::Let(let_decl) = stmt {
                        decls.push(TopDecl::Let(let_decl));
                    }
                }
                if let Some(doc) = doc {
                    self.doc_comments.insert(d.range().start.offset, doc);
                }
                decls.push(d);
            } else if self.pos == before {
                // No progress → emit a diag and skip a token to avoid a loop.
                let t = self.peek().clone();
                self.error(
                    format!("unexpected token '{}' at top level", t.text),
                    t.start,
                    t.end,
                );
                self.synchronize();
            }
            self.eat_newlines();
        }
        let end = self.peek().start;
        Script {
            decls,
            range: self.make_range(start, end),
            module_doc,
        }
    }

    /// If the file opens with a `///` block that is separated from the first
    /// declaration by a blank line, consume it as the module doc and return it.
    /// Otherwise consume nothing (the block, if any, is left for the first
    /// declaration's `collect_doc_comment`).
    fn collect_module_doc(&mut self) -> Option<String> {
        if self.peek().kind != TokenKind::DocComment {
            return None;
        }
        let save = self.pos;
        let mut lines = Vec::new();
        loop {
            // Current token is a DocComment line.
            lines.push(self.peek().text.clone());
            self.advance();
            if self.peek().kind != TokenKind::Newline {
                // Doc line at EOF / not newline-terminated — treat as module doc.
                return Some(lines.join("\n"));
            }
            self.advance(); // the doc line's terminating newline
            match self.peek().kind {
                // Another doc line continues this block.
                TokenKind::DocComment => continue,
                // A blank line (or a discarded `//` comment, which leaves only
                // its newline) separates the block from the first decl → module doc.
                TokenKind::Newline => {
                    self.eat_newlines();
                    return Some(lines.join("\n"));
                }
                // A declaration follows immediately → the block documents it, not
                // the module. Rewind so the main loop attaches it to that decl.
                _ => {
                    self.pos = save;
                    return None;
                }
            }
        }
    }

    fn parse_top_decl(&mut self) -> Option<TopDecl> {
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
                if kw("in") {
                    return Some(self.parse_in_decl(side, label));
                }
                return Some(TopDecl::Out(self.parse_out_binding(side, label)));
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
                if next_is_open_chip {
                    if let Some(r) = &anns.closed {
                        self.error(
                            "@closed cannot be combined with 'open chip'".to_string(),
                            r.start,
                            r.end,
                        );
                    }
                    self.advance(); // consume "open"
                    return Some(self.parse_chip_decl(true, label, false));
                }
                return Some(self.parse_chip_decl(false, label, anns.closed.is_some()));
            }
            if kw("mod") {
                self.error(
                    "annotations are not allowed on 'mod' declarations".to_string(),
                    t.start,
                    t2.end,
                );
                return Some(self.parse_mod_decl());
            }
            self.error(
                "an annotation must be followed by an 'in', 'out', or chip declaration"
                    .to_string(),
                t.start,
                t2.end,
            );
            if t2.kind == TokenKind::Eof {
                return None;
            }
            return self.parse_top_decl(); // annotations consumed → guaranteed progress
        }
        if t.kind == TokenKind::Kw {
            match t.text.as_str() {
                "var" => return Some(self.parse_var_decl(false)),
                "static" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "var" {
                        self.advance(); // consume "static"
                        return Some(self.parse_var_decl(true));
                    }
                }
                "buffer" => return Some(self.parse_buffer_decl()),
                "in" => return Some(self.parse_in_decl(None, None)),
                "out" => return Some(TopDecl::Out(self.parse_out_binding(None, None))),
                "let" => return Some(self.parse_let_decl()),
                "on" => return Some(TopDecl::Handler(self.parse_handler())),
                "array" => return Some(self.parse_array_decl()),
                "chip" => return Some(self.parse_chip_decl(false, None, false)),
                "mod" => return Some(self.parse_mod_decl()),
                "open" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "chip" {
                        self.advance(); // consume "open"
                        return Some(self.parse_chip_decl(true, None, false));
                    }
                }
                "fn" => return Some(self.parse_fn_decl()),
                "import" => return Some(self.parse_import_decl()),
                "type" => return Some(self.parse_type_alias_decl()),
                "if" => {
                    let s = self.parse_if_stmt();
                    if let Stmt::If(i) = s {
                        return Some(TopDecl::If(i));
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
        Some(TopDecl::ExprStmt(ExprStmt {
            range: self.make_range(expr_start, lhs.range().end),
            expr: lhs,
        }))
    }

    // ---------- declarations ----------

    fn parse_var_decl(&mut self, is_static: bool) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("var")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        let init = if self.match_tok(TokenKind::Op, Some("=")).is_some() {
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
            range: self.make_range(start, end),
        })
    }

    fn parse_buffer_decl(&mut self) -> TopDecl {
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

    fn parse_in_decl(&mut self, side: Option<PortSide>, label: Option<String>) -> TopDecl {
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
            range: self.make_range(start, end),
        })
    }

    /// Consume a run of leading annotations (`@left`-style sides,
    /// `@label("…")`, `@closed`). Newlines after each annotation are eaten so
    /// annotations may sit on their own lines above the declaration.
    fn parse_annotations(&mut self) -> ParsedAnnotations {
        let mut anns = ParsedAnnotations::default();
        while self.check(TokenKind::Annotation, None) {
            let tok = self.advance();
            match tok.text.as_str() {
                "label" => {
                    let mut text: Option<(String, Pos)> = None;
                    if self.match_tok(TokenKind::LParen, None).is_some() {
                        if self.check(TokenKind::Str, None) {
                            let s_tok = self.advance();
                            let s = match &s_tok.value {
                                Some(TokenValue::Str(s)) => s.clone(),
                                _ => s_tok.text.clone(),
                            };
                            text = Some((s, s_tok.end));
                        }
                        self.match_tok(TokenKind::RParen, None);
                    }
                    match text {
                        Some((s, end)) if s.is_empty() => {
                            self.error(
                                "@label text must not be empty".to_string(),
                                tok.start,
                                end,
                            );
                        }
                        Some((s, end)) => {
                            if anns.label.is_some() {
                                self.error("duplicate @label".to_string(), tok.start, end);
                            } else {
                                anns.label = Some((s, self.make_range(tok.start, end)));
                            }
                        }
                        None => self.error(
                            "@label requires a string argument: @label(\"text\")".to_string(),
                            tok.start,
                            tok.end,
                        ),
                    }
                }
                "closed" => {
                    if anns.closed.is_some() {
                        self.error("duplicate @closed".to_string(), tok.start, tok.end);
                    } else {
                        anns.closed = Some(self.make_range(tok.start, tok.end));
                    }
                }
                w => match PortSide::from_word(w) {
                    Some(side) => {
                        if anns.side.is_some() {
                            self.error(
                                "duplicate side annotation".to_string(),
                                tok.start,
                                tok.end,
                            );
                        } else {
                            anns.side = Some((side, self.make_range(tok.start, tok.end)));
                        }
                    }
                    None => self.error(
                        format!(
                            "unknown annotation '@{}'; expected @left, @right, @top, @bottom, @label, or @closed",
                            w
                        ),
                        tok.start,
                        tok.end,
                    ),
                },
            }
            while self.check(TokenKind::Newline, None) {
                self.advance();
            }
        }
        anns
    }

    fn parse_out_binding(&mut self, side: Option<PortSide>, label: Option<String>) -> OutBinding {
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
                range: self.make_range(start, end),
            }
        } else {
            let end = self.peek().start;
            self.eat_stmt_end();
            OutBinding {
                name,
                value: None,
                typ,
                side,
                label,
                range: self.make_range(start, end),
            }
        }
    }

    fn parse_let_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("let")).start;

        // Record destructuring: `let { a, b: alias, ...rest } = expr`
        if self.check(TokenKind::LBrace, None) {
            let brace_start = self.advance().start; // consume `{`
            let mut fields: Vec<RecordDestructField> = Vec::new();
            self.eat_newlines();
            while !self.check(TokenKind::RBrace, None) && self.peek().kind != TokenKind::Eof {
                // `...rest`
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
                names,
                rest,
                range: self.make_range(paren_start, paren_end),
            };
            let typ = if self.match_tok(TokenKind::Colon, None).is_some() {
                Some(self.parse_type())
            } else {
                None
            };
            self.expect(TokenKind::Op, Some("="));
            let value = self.parse_expr();
            let end = value.range().end;
            self.eat_stmt_end();
            return TopDecl::Let(LetDecl {
                binding,
                typ,
                value,
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
                    range: self.make_range(start, end),
                });
            }
        }
        self.expect(TokenKind::Op, Some("="));
        // `let name = on Trigger { ... }` → EventDecl (captured event)
        if self.check(TokenKind::Kw, Some("on")) {
            self.advance();
            let trigger = self.parse_trigger();
            if self.check(TokenKind::LBrace, None) {
                let captured_body = Some(self.parse_block());
                let end = captured_body.as_ref().unwrap().range.end;
                let source = trigger_to_expr(&trigger);
                return TopDecl::Event(EventDecl {
                    name,
                    source,
                    captured_body,
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
                range: self.make_range(start, end),
            });
        }
        // `let name = await expr [on trigger]`
        if self.check(TokenKind::Kw, Some("await")) {
            let await_start = self.advance().start;
            if let Stmt::Await(mut a) = self.parse_await_inner(await_start, None) {
                a.binding = Some(name);
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
            range: self.make_range(start, end),
        })
    }

    // `type Name = { field: Type, ... }` or `type Name = (A, B)`
    fn parse_type_alias_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("type")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        self.expect(TokenKind::Op, Some("="));
        let typ = self.parse_type();
        let end = self.peek().start;
        self.eat_stmt_end();
        TopDecl::TypeAlias(TypeAliasDecl {
            name,
            typ,
            range: self.make_range(start, end),
        })
    }

    // `array name: ElementType[]`
    fn parse_array_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("array")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        self.expect(TokenKind::Colon, None);
        let full_type = self.parse_type();
        let element_type = match full_type {
            TypeExpr::Array { inner, .. } => *inner,
            other => {
                let r = match &other {
                    TypeExpr::Name { range, .. }
                    | TypeExpr::Ref { range, .. }
                    | TypeExpr::Array { range, .. }
                    | TypeExpr::Tuple { range, .. }
                    | TypeExpr::Union { range, .. }
                    | TypeExpr::Record { range, .. } => range,
                };
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

    // `chip Name(params) [-> outputs] { body }`
    fn parse_chip_decl(&mut self, open: bool, label: Option<String>, closed: bool) -> TopDecl {
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
            return TopDecl::AnonChip(AnonChipDecl {
                open,
                body: Block {
                    stmts,
                    range: self.make_range(start, end),
                },
                range: self.make_range(start, end),
                label: label.clone(),
                closed,
            });
        }
        // `chip on trigger { ... }` → `chip { on trigger { ... } }`
        if self.check(TokenKind::Kw, Some("on")) {
            let handler = self.parse_handler();
            let end = handler.range.end;
            return TopDecl::AnonChip(AnonChipDecl {
                open,
                body: Block {
                    stmts: vec![Stmt::Handler(handler)],
                    range: self.make_range(start, end),
                },
                range: self.make_range(start, end),
                label: label.clone(),
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
                closed,
            });
        }
        let name = self.expect(TokenKind::Ident, None).text;
        let inputs = self.parse_param_list();
        let outputs = if self.match_tok(TokenKind::Arrow, None).is_some() {
            self.parse_chip_outputs()
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        let end = body.range.end;
        TopDecl::Chip(ChipDecl {
            name,
            inputs,
            outputs,
            body,
            range: self.make_range(start, end),
            inline: false,
            label,
            closed,
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

    fn parse_mod_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("mod")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let inputs = self.parse_param_list();
        let outputs = if self.match_tok(TokenKind::Arrow, None).is_some() {
            self.parse_chip_outputs()
        } else {
            Vec::new()
        };
        let body = self.parse_block();
        let end = body.range.end;
        TopDecl::Chip(ChipDecl {
            name,
            inputs,
            outputs,
            body,
            range: self.make_range(start, end),
            inline: true,
            label: None,
            closed: false,
        })
    }

    fn parse_param_list(&mut self) -> Vec<Param> {
        self.expect(TokenKind::LParen, None);
        let mut params = Vec::new();
        let mut synth_counter = 0usize;
        self.eat_stmt_end();
        while !self.check(TokenKind::RParen, None) && self.peek().kind != TokenKind::Eof {
            let pstart = self.peek().start;

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
                    range: self.make_range(pstart, pend),
                });
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
                self.eat_stmt_end();
                continue;
            }

            // Normal parameter: `name: Type`
            let pname = self.expect(TokenKind::Ident, None).text;
            self.expect(TokenKind::Colon, None);
            let typ = self.parse_type();
            let pend = self.peek().start;
            params.push(Param {
                name: pname,
                typ,
                pattern: None,
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
        params
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

    // `fn name(params) [-> ReturnType] = expr`
    fn parse_fn_decl(&mut self) -> TopDecl {
        let start = self.expect(TokenKind::Kw, Some("fn")).start;
        let name = self.expect(TokenKind::Ident, None).text;
        let params = self.parse_param_list();
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

    /// Return `true` when the tokens after `on` look like an arbitrary
    /// expression rather than a simple trigger pattern.
    ///
    /// A *simple* trigger consists of:
    ///   `!* ident (. ident)?` repeated, separated by `|`
    /// If, after scanning that pattern, the next real token is `{` or `(`
    /// (body / params), the trigger is simple.  Any other token (e.g. `&&`,
    /// `||`, `+`, a literal, …) means the user wrote an expression trigger.
    fn looks_like_expr_trigger(&self) -> bool {
        let mut i = self.pos;
        let len = self.tokens.len();
        let get = |idx: usize| -> &Token {
            self.tokens
                .get(idx)
                .unwrap_or_else(|| self.tokens.last().unwrap())
        };

        // Skip one or more `|`-separated trigger atoms.  Each atom is:
        //   `!*  ident  (.ident)?`
        loop {
            // Skip leading `!` prefixes.
            while i < len && get(i).kind == TokenKind::Op && get(i).text == "!" {
                i += 1;
            }
            // Must see an ident for a simple trigger.
            if i >= len || get(i).kind != TokenKind::Ident {
                // Non-ident at atom start → expression trigger (e.g. a literal
                // or a `(` grouping used as expression, not trigger grouping).
                // Actually `(` is also valid for trigger grouping; treat as
                // expression only when it's not an ident or `!`.
                return get(i).kind != TokenKind::LParen && get(i).kind != TokenKind::Ident;
            }
            i += 1; // consume ident

            // Optional `.field`.
            if i < len && get(i).kind == TokenKind::Dot {
                i += 1;
                if i < len && get(i).kind == TokenKind::Ident {
                    i += 1;
                }
            }

            // Is the next token a `|` (trigger union)?  If so, continue loop.
            if i < len && get(i).kind == TokenKind::Op && get(i).text == "|" {
                i += 1; // consume `|`
                continue;
            }
            break;
        }

        // After the last atom the next meaningful token should be `{` or `(`.
        // Anything else (e.g. `&&`, `||`, `+`, …) means expression trigger.
        let t = get(i);
        !matches!(
            t.kind,
            TokenKind::LBrace
                | TokenKind::LParen
                | TokenKind::Newline
                | TokenKind::Semi
                | TokenKind::Eof
        )
    }

    // `event name = expr` or `event name = on Trigger { body }`
    fn parse_handler(&mut self) -> Handler {
        let start = self.expect(TokenKind::Kw, Some("on")).start;

        // For expression triggers we build a synthetic let binding that is
        // queued in `pending_stmts` AFTER the body is parsed.  This avoids
        // the body's own `parse_block` call draining the pending queue early.
        let mut pending_let: Option<LetDecl> = None;

        let trigger = if self.looks_like_expr_trigger() {
            // `on <expr> { body }` — desugar into:
            //   let _on_expr_N = <expr>
            //   on _on_expr_N { body }
            let expr = self.parse_expr();
            let expr_range = expr.range().clone();
            let n = self.expr_trigger_counter;
            self.expr_trigger_counter += 1;
            let synth_name = format!("_on_expr_{}", n);

            pending_let = Some(LetDecl {
                binding: LetBinding::Ident {
                    name: synth_name.clone(),
                    range: expr_range.clone(),
                },
                typ: None,
                value: expr,
                range: expr_range.clone(),
            });

            Trigger::Ident {
                name: synth_name,
                range: expr_range,
            }
        } else {
            self.parse_trigger()
        };

        // Trigger args: bare identifiers bind the event's data outputs;
        // string/number literals and `name = value` pairs configure the event
        // gate (e.g. `on ChatCommand("greet", Description = "Greets you")`).
        let mut params: Vec<String> = Vec::new();
        let mut config: Vec<HandlerConfigArg> = Vec::new();
        if self.match_tok(TokenKind::LParen, None).is_some() {
            while !self.check(TokenKind::RParen, None) {
                if self.check(TokenKind::Ident, None) {
                    let name = self.expect(TokenKind::Ident, None).text;
                    if self.match_tok(TokenKind::Op, Some("=")).is_some() {
                        let value = self.parse_expr();
                        config.push(HandlerConfigArg::Named { name, value });
                    } else {
                        params.push(name);
                    }
                } else {
                    let value = self.parse_expr();
                    config.push(HandlerConfigArg::Positional(value));
                }
                if self.match_tok(TokenKind::Comma, None).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RParen, None);
        }
        let body = self.parse_block();
        let end = body.range.end;

        // Queue the synthetic let AFTER parsing the body so that parse_block
        // doesn't accidentally drain it into the handler body.
        if let Some(let_decl) = pending_let {
            self.pending_stmts.push(Stmt::Let(let_decl));
        }

        Handler {
            trigger,
            params,
            config,
            body,
            range: self.make_range(start, end),
        }
    }

    fn parse_trigger(&mut self) -> Trigger {
        let mut first = self.parse_trigger_atom();
        // Optional `|`-separated union.
        let mut parts: Vec<Trigger> = Vec::new();
        while self.check(TokenKind::Op, Some("|")) {
            // Only treat `|` as a trigger-union if followed by another atom.
            let save = self.pos;
            self.advance();
            let nxt = self.parse_trigger_atom();
            if parts.is_empty() {
                parts.push(first.clone());
            }
            parts.push(nxt);
            // keep going
            let _ = save;
        }
        if parts.is_empty() {
            first
        } else {
            let start = match &parts[0] {
                Trigger::Ident { range, .. }
                | Trigger::Field { range, .. }
                | Trigger::Not { range, .. }
                | Trigger::Union { range, .. } => range.start,
            };
            let end = match parts.last().unwrap() {
                Trigger::Ident { range, .. }
                | Trigger::Field { range, .. }
                | Trigger::Not { range, .. }
                | Trigger::Union { range, .. } => range.end,
            };
            // Drop `first` from capture when empty; use `parts[0]` as the new first.
            let _ = &mut first;
            Trigger::Union {
                parts,
                range: self.make_range(start, end),
            }
        }
    }

    fn parse_trigger_atom(&mut self) -> Trigger {
        let t = self.peek().clone();
        if t.kind == TokenKind::LParen {
            self.advance();
            let inner = self.parse_trigger();
            self.expect(TokenKind::RParen, None);
            return inner;
        }
        if t.kind == TokenKind::Op && t.text == "!" {
            let start = t.start;
            self.advance();
            let inner = self.parse_trigger_atom();
            let end = match &inner {
                Trigger::Ident { range, .. }
                | Trigger::Field { range, .. }
                | Trigger::Not { range, .. }
                | Trigger::Union { range, .. } => range.end,
            };
            return Trigger::Not {
                inner: Box::new(inner),
                range: self.make_range(start, end),
            };
        }
        let name_tok = self.expect(TokenKind::Ident, None);
        if self.match_tok(TokenKind::Dot, None).is_some() {
            let field_tok = self.expect(TokenKind::Ident, None);
            return Trigger::Field {
                obj: name_tok.text,
                field: field_tok.text,
                range: self.make_range(name_tok.start, field_tok.end),
            };
        }
        Trigger::Ident {
            name: name_tok.text,
            range: self.make_range(name_tok.start, name_tok.end),
        }
    }

    // ---------- type expressions ----------

    fn parse_type(&mut self) -> TypeExpr {
        let mut first = self.parse_type_postfix();
        // `A | B | C`
        if self.check(TokenKind::Op, Some("|")) {
            let mut options = vec![first];
            while self.match_tok(TokenKind::Op, Some("|")).is_some() {
                options.push(self.parse_type_postfix());
            }
            let start = match &options[0] {
                TypeExpr::Name { range, .. }
                | TypeExpr::Ref { range, .. }
                | TypeExpr::Array { range, .. }
                | TypeExpr::Tuple { range, .. }
                | TypeExpr::Union { range, .. }
                | TypeExpr::Record { range, .. } => range.start,
            };
            let end = match options.last().unwrap() {
                TypeExpr::Name { range, .. }
                | TypeExpr::Ref { range, .. }
                | TypeExpr::Array { range, .. }
                | TypeExpr::Tuple { range, .. }
                | TypeExpr::Union { range, .. }
                | TypeExpr::Record { range, .. } => range.end,
            };
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
            let start = match &t {
                TypeExpr::Name { range, .. }
                | TypeExpr::Ref { range, .. }
                | TypeExpr::Array { range, .. }
                | TypeExpr::Tuple { range, .. }
                | TypeExpr::Union { range, .. }
                | TypeExpr::Record { range, .. } => range.start,
            };
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
            let end = match &inner {
                TypeExpr::Name { range, .. }
                | TypeExpr::Ref { range, .. }
                | TypeExpr::Array { range, .. }
                | TypeExpr::Tuple { range, .. }
                | TypeExpr::Union { range, .. }
                | TypeExpr::Record { range, .. } => range.end,
            };
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
        // Plain identifier type name (int, bool, controller, chipTypeName, …)
        let name_tok = self.expect(TokenKind::Ident, None);
        TypeExpr::Name {
            name: name_tok.text,
            range: self.make_range(name_tok.start, name_tok.end),
        }
    }

    // ---------- blocks + statements ----------

    fn parse_block(&mut self) -> Block {
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

    fn parse_stmt(&mut self) -> Option<Stmt> {
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
                if kw("in") {
                    if let TopDecl::In(i) = self.parse_in_decl(side, label) {
                        return Some(Stmt::In(i));
                    }
                    return None;
                }
                return Some(Stmt::OutBinding(self.parse_out_binding(side, label)));
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
                match self.parse_chip_decl(open, label, closed) {
                    TopDecl::AnonChip(ac) => return Some(Stmt::AnonChip(ac)),
                    TopDecl::Chip(c) => return Some(Stmt::ChipDecl(c)),
                    _ => return None,
                }
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
                "an annotation must be followed by an 'in', 'out', or chip declaration"
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
                    if let TopDecl::Var(v) = self.parse_var_decl(false) {
                        return Some(Stmt::Var(v));
                    }
                }
                "static" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "var" {
                        self.advance();
                        if let TopDecl::Var(v) = self.parse_var_decl(true) {
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
                "out" => return Some(Stmt::OutBinding(self.parse_out_binding(None, None))),
                "let" => {
                    let decl = self.parse_let_decl();
                    match decl {
                        TopDecl::Let(v) => return Some(Stmt::Let(v)),
                        TopDecl::Await(a) => return Some(Stmt::Await(a)),
                        _ => {}
                    }
                }
                "array" => {
                    if let TopDecl::Array(a) = self.parse_array_decl() {
                        return Some(Stmt::Array(a));
                    }
                }
                "in" => {
                    if let TopDecl::In(i) = self.parse_in_decl(None, None) {
                        return Some(Stmt::In(i));
                    }
                }
                "on" => return Some(Stmt::Handler(self.parse_handler())),
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
                "chip" => match self.parse_chip_decl(false, None, false) {
                    TopDecl::AnonChip(ac) => return Some(Stmt::AnonChip(ac)),
                    TopDecl::Chip(c) => return Some(Stmt::ChipDecl(c)),
                    _ => {}
                },
                "open" => {
                    if self.peek_at(1).kind == TokenKind::Kw && self.peek_at(1).text == "chip" {
                        self.advance();
                        if let TopDecl::AnonChip(ac) = self.parse_chip_decl(true, None, false) {
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

    fn parse_await_inner(&mut self, start: Pos, binding: Option<String>) -> Stmt {
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
            range: self.make_range(start, end),
        })
    }

    fn parse_if_stmt(&mut self) -> Stmt {
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

    // ---------- expressions: Pratt ----------

    fn parse_expr(&mut self) -> Expr {
        self.parse_binary(0)
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
            if tok.kind != TokenKind::Op {
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
        if t.kind == TokenKind::Op && is_prefix_op(&t.text) {
            // Fold `-<number>` into a negative literal at parse time.
            if t.text == "-" {
                let next = self.peek_at(1);
                if next.kind == TokenKind::Int {
                    self.advance(); // consume '-'
                    let num = self.advance();
                    let val: i64 = num.text.parse().unwrap_or(0);
                    return Expr::IntLit {
                        value: -val,
                        text: format!("-{}", num.text),
                        range: self.make_range(t.start, num.end),
                    };
                } else if next.kind == TokenKind::Float {
                    self.advance(); // consume '-'
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
            if t.kind == TokenKind::LBracket {
                self.advance();
                let idx = self.parse_expr();
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
                    args.push(self.parse_call_arg());
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
                    range: self.make_range(start, end),
                };
                continue;
            }
            break;
        }
        e
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
            let name = self.advance().text;
            self.advance(); // '='
            let value = self.parse_expr();
            CallArg::Named { name, value }
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
                let value = parse_int_literal(&cleaned);
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
                                    "asset reference must be `$AssetType/AssetName` or a prefab path `$./file.brz`",
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
            TokenKind::LBracket => {
                let start = t.start;
                self.advance(); // consume '['
                let mut elements = Vec::new();
                self.eat_newlines();
                while !self.check(TokenKind::RBracket, None) && self.peek().kind != TokenKind::Eof {
                    // `...expr` spreads another array's elements in place.
                    if self.check(TokenKind::Op, Some("...")) {
                        self.advance();
                        elements.push(ArrayElem::Spread(self.parse_expr()));
                    } else {
                        elements.push(ArrayElem::Item(self.parse_expr()));
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
                            let sub = parse(&source, self.file);
                            let mut expr = sub
                                .ast
                                .decls
                                .into_iter()
                                .find_map(|d| match d {
                                    TopDecl::ExprStmt(es) => Some(es.expr),
                                    _ => None,
                                })
                                .unwrap_or(Expr::StringLit {
                                    value: String::new(),
                                    range: self.make_range(t.start, t.end),
                                });
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
            TokenKind::Kw if t.text == "if" => {
                self.advance();
                let cond = self.parse_expr();
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
            TokenKind::Ident => {
                self.advance();
                Expr::Ident {
                    name: t.text,
                    range: self.make_range(t.start, t.end),
                }
            }
            TokenKind::LParen => {
                self.advance();
                let e = self.parse_expr();
                if self.check(TokenKind::Comma, None) {
                    // Tuple literal: (expr, expr, ...)
                    let mut elements = vec![e];
                    while self.match_tok(TokenKind::Comma, None).is_some() {
                        if self.check(TokenKind::RParen, None) {
                            break;
                        }
                        elements.push(self.parse_expr());
                    }
                    let end = self.expect(TokenKind::RParen, None);
                    // Desugar to a record lit or keep as-is depending on AST support.
                    // For now, use existing tuple handling: emit as a Call to a synthetic tuple constructor?
                    // Actually, tuples in Wirescript are already handled by the chip output system.
                    // Create a RecordLit with numeric field names for now:
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
                if self.looks_like_record_lit() {
                    self.parse_record_lit()
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
        // Skip newlines after `{`
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
                let value = self.parse_expr();
                let spread_end = value.range().end;
                fields.push(RecordLitField::Spread {
                    value,
                    range: self.make_range(spread_start, spread_end),
                });
            } else {
                let name_tok = self.expect(TokenKind::Ident, None);
                if self.match_tok(TokenKind::Colon, None).is_some() {
                    // Named field: `name: expr`
                    let value = self.parse_expr();
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
            // Try parsing as a statement first (let, var, assign, etc.)
            // If it looks like a statement keyword, parse it as a statement
            let is_stmt_kw = self.peek().kind == TokenKind::Kw
                && matches!(self.peek().text.as_str(), "let" | "var" | "static");
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
            // Check if there's an assignment operator
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
            stmts.push(Stmt::ExprStmt(ExprStmt {
                expr,
                range: SourceRange::default(),
            }));
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

fn parse_int_literal(cleaned: &str) -> i64 {
    if let Some(hex) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        i64::from_str_radix(hex, 16).unwrap_or(0)
    } else if let Some(bin) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        i64::from_str_radix(bin, 2).unwrap_or(0)
    } else if let Some(oct) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        i64::from_str_radix(oct, 8).unwrap_or(0)
    } else {
        cleaned.parse().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Script {
        let r = parse(src, "test");
        assert!(
            r.diagnostics.is_empty(),
            "unexpected diags: {:?}",
            r.diagnostics
        );
        r.ast
    }

    #[test]
    fn empty_source_parses() {
        let s = parse_ok("");
        assert!(s.decls.is_empty());
    }

    #[test]
    fn top_doc_block_before_blank_is_module_doc_not_merged() {
        // A `///` block at the top, separated from the first decl by a blank
        // line (or a `//` comment), is the module doc — it must NOT merge into
        // the following declaration's own doc comment.
        let src = "/// mod line 1\n/// mod line 2\n\n//\n\n/// chip doc\nchip {\n  var x: int = 0\n}";
        let r = parse(src, "test");
        let md = r.ast.module_doc.as_deref().unwrap_or("<none>");
        assert!(
            md.contains("mod line 1") && md.contains("mod line 2"),
            "module doc should hold the top block: {md:?}"
        );
        assert!(!md.contains("chip doc"), "module doc must not merge the chip doc: {md:?}");
        assert!(
            r.doc_comments.values().any(|d| d == "chip doc"),
            "chip doc must remain its own comment: {:?}",
            r.doc_comments
        );
        assert!(
            r.doc_comments.values().all(|d| !d.contains("mod line")),
            "the module doc must not attach to a declaration: {:?}",
            r.doc_comments
        );
    }

    #[test]
    fn top_doc_block_adjacent_to_decl_is_decl_doc_not_module() {
        // No blank line → the block documents the first decl (unchanged), so
        // `module_doc` is None and the first decl carries it.
        let src = "/// first decl doc\nvar x: int = 0";
        let r = parse(src, "test");
        assert!(r.ast.module_doc.is_none(), "adjacent block is not a module doc: {:?}", r.ast.module_doc);
        assert!(
            r.doc_comments.values().any(|d| d == "first decl doc"),
            "adjacent block documents the decl: {:?}",
            r.doc_comments
        );
    }

    #[test]
    fn record_type_field_doc_comments_parse_and_store() {
        let r = parse(
            "type Point = {\n  /// the x coordinate\n  x: int,\n  /// the y coordinate\n  y: int,\n}",
            "test",
        );
        assert!(
            r.diagnostics.is_empty(),
            "record field doc comments should parse: {:?}",
            r.diagnostics
        );
        let docs: Vec<&String> = r.doc_comments.values().collect();
        assert!(docs.iter().any(|d| d.contains("the x coordinate")), "x doc missing: {docs:?}");
        assert!(docs.iter().any(|d| d.contains("the y coordinate")), "y doc missing: {docs:?}");
    }

    #[test]
    fn var_int_literal() {
        let s = parse_ok("var x = 42");
        assert_eq!(s.decls.len(), 1);
        match &s.decls[0] {
            TopDecl::Var(v) => {
                assert_eq!(v.name, "x");
                assert!(v.typ.is_none());
                match &v.init {
                    Some(Expr::IntLit { value, .. }) => assert_eq!(*value, 42),
                    _ => panic!("expected IntLit init"),
                }
            }
            _ => panic!("expected Var decl"),
        }
    }

    #[test]
    fn var_typed() {
        let s = parse_ok("var x: int = 1");
        match &s.decls[0] {
            TopDecl::Var(v) => match &v.typ {
                Some(TypeExpr::Name { name, .. }) => assert_eq!(name, "int"),
                _ => panic!("expected typed VarDecl"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn in_out_decls() {
        let s = parse_ok("in trigger: exec\nout count = 3");
        assert!(matches!(s.decls[0], TopDecl::In(_)));
        assert!(matches!(s.decls[1], TopDecl::Out(_)));
    }

    #[test]
    fn binary_precedence() {
        let s = parse_ok("var x = a + b * c");
        match &s.decls[0] {
            TopDecl::Var(v) => match v.init.as_ref().unwrap() {
                Expr::BinOp { op, right, .. } => {
                    assert_eq!(op, "+");
                    match right.as_ref() {
                        Expr::BinOp { op, .. } => assert_eq!(op, "*"),
                        _ => panic!("expected right = BinOp *"),
                    }
                }
                _ => panic!("expected BinOp +"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn handler_with_param() {
        let s = parse_ok("on CharacterDied(char) { emit died }");
        match &s.decls[0] {
            TopDecl::Handler(h) => {
                assert_eq!(h.params.len(), 1);
                assert_eq!(h.params[0], "char");
                match &h.trigger {
                    Trigger::Ident { name, .. } => assert_eq!(name, "CharacterDied"),
                    _ => panic!("expected TrigIdent"),
                }
            }
            _ => panic!("expected Handler"),
        }
    }

    #[test]
    fn handler_expr_trigger_desugars_to_let_plus_handler() {
        // `on a && b { x = 1 }` should desugar into:
        //   let _on_expr_0 = a && b
        //   on _on_expr_0 { x = 1 }
        let src = "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }";
        let s = parse_ok(src);
        // Expected: In(a), In(b), Var(x), Let(_on_expr_0), Handler(_on_expr_0)
        assert_eq!(
            s.decls.len(),
            5,
            "decls: {:?}",
            s.decls.iter().map(|d| d.range()).collect::<Vec<_>>()
        );
        match &s.decls[3] {
            TopDecl::Let(l) => match &l.binding {
                LetBinding::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident binding"),
            },
            d => panic!("expected Let, got {:?}", d),
        }
        match &s.decls[4] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident trigger"),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn simple_counter_program() {
        let src = "in tick: exec\nvar n: int = 0\non tick {\n  n = n + 1\n}\nout count = n";
        let s = parse_ok(src);
        assert_eq!(s.decls.len(), 4);
    }

    #[test]
    fn call_with_kwargs() {
        let s = parse_ok("var x = vec(x = 1, y = 2, z = 3)");
        match &s.decls[0] {
            TopDecl::Var(v) => match v.init.as_ref().unwrap() {
                Expr::Call { args, .. } => {
                    assert_eq!(args.len(), 3);
                    matches!(&args[0], CallArg::Named { name, .. } if name == "x");
                }
                _ => panic!("expected Call"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn hex_literal() {
        let s = parse_ok("var x = 0xff");
        match &s.decls[0] {
            TopDecl::Var(v) => match v.init.as_ref().unwrap() {
                Expr::IntLit { value, .. } => assert_eq!(*value, 255),
                _ => panic!("expected IntLit"),
            },
            _ => panic!("expected Var"),
        }
    }

    #[test]
    fn parse_array_decl() {
        let r = crate::parser::parse("array xs: int[]", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Array(a) => {
                assert_eq!(a.name, "xs");
                assert!(matches!(&a.element_type, TypeExpr::Name { name, .. } if name == "int"));
            }
            d => panic!("expected Array, got {:?}", d),
        }
    }

    #[test]
    fn parse_chip_decl() {
        let src = "chip Counter(bump: exec, reset: exec) -> (value: int, overflow: bool) {\n  var n: int = 0\n}";
        let r = crate::parser::parse(src, "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert_eq!(c.name, "Counter");
                assert_eq!(c.inputs.len(), 2);
                assert_eq!(c.outputs.len(), 2);
                assert_eq!(c.outputs[0].name, "value");
            }
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn parse_fn_decl() {
        let src = "fn add(a: int, b: int) -> int = a + b";
        let r = crate::parser::parse(src, "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Fn(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
                assert!(f.return_type.is_some());
            }
            d => panic!("expected Fn, got {:?}", d),
        }
    }

    #[test]
    fn parse_anonymous_output_defaults_to_underscore() {
        let r = crate::parser::parse("chip Double(x: int) -> int { out _ = x * 2 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert_eq!(c.outputs.len(), 1);
                assert_eq!(c.outputs[0].name, "_");
            }
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn parse_mod_with_output() {
        let r = crate::parser::parse("mod clamp(v: int) -> (r: int) { return v }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert!(c.inline);
                assert_eq!(c.outputs.len(), 1);
                assert_eq!(c.outputs[0].name, "r");
            }
            d => panic!("expected Chip (mod), got {:?}", d),
        }
    }

    #[test]
    fn parse_mod_anonymous_output_defaults_to_underscore() {
        let r = crate::parser::parse(
            "mod abs(v: int) -> int { if v < 0 { return 0 - v } return v }",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => {
                assert!(c.inline);
                assert_eq!(c.outputs.len(), 1);
                assert_eq!(c.outputs[0].name, "_");
            }
            d => panic!("expected Chip (mod), got {:?}", d),
        }
    }

    #[test]
    fn parse_return_value() {
        let r = crate::parser::parse("mod foo() -> int { return 42 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => match &c.body.stmts[0] {
                Stmt::Return { value: Some(_), .. } => {}
                s => panic!("expected Return with value, got {:?}", s),
            },
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    #[test]
    fn parse_return_no_value() {
        let r = crate::parser::parse("mod foo(x: *int) { return }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::Chip(c) => match &c.body.stmts[0] {
                Stmt::Return { value: None, .. } => {}
                s => panic!("expected Return without value, got {:?}", s),
            },
            d => panic!("expected Chip, got {:?}", d),
        }
    }

    // event keyword was removed — event alias/captured tests removed

    #[test]
    fn side_annotation_same_line_and_line_above() {
        let r = parse("@left in a: bool\n@right\nout b = a", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(i.side, Some(PortSide::Left));
        let TopDecl::Out(o) = &r.ast.decls[1] else {
            panic!("decl 1: {:?}", r.ast.decls[1])
        };
        assert_eq!(o.side, Some(PortSide::Right));
    }

    #[test]
    fn unannotated_ports_have_no_side() {
        let r = parse("in a: bool\nout b = a", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!()
        };
        assert_eq!(i.side, None);
    }

    #[test]
    fn unknown_annotation_word_errors() {
        let r = parse("@middle in a: bool", "test");
        assert_eq!(r.diagnostics.len(), 1, "diags: {:?}", r.diagnostics);
        assert!(r.diagnostics[0].message.contains("unknown annotation '@middle'"));
        // Declaration still parses, just without a side.
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!()
        };
        assert_eq!(i.side, None);
    }

    #[test]
    fn annotation_before_non_port_decl_errors() {
        let r = parse("@left var x: int = 1", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("must be followed by an 'in', 'out', or chip declaration")),
            "diags: {:?}",
            r.diagnostics
        );
        // The var itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::Var(_)));
    }

    #[test]
    fn duplicate_annotation_errors_first_wins() {
        let r = parse("@left @right in a: bool", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("duplicate side annotation")),
            "diags: {:?}",
            r.diagnostics
        );
        let TopDecl::In(i) = &r.ast.decls[0] else {
            panic!()
        };
        assert_eq!(i.side, Some(PortSide::Left));
    }

    #[test]
    fn annotation_parses_at_statement_level() {
        // Inside a chip body it must PARSE (lowering rejects it later with WS023).
        let r = parse("chip { @left in a: bool }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else {
            panic!()
        };
        let Stmt::In(i) = &ac.body.stmts[0] else {
            panic!("stmt: {:?}", ac.body.stmts[0])
        };
        assert_eq!(i.side, Some(PortSide::Left));
    }

    #[test]
    fn label_annotation_on_anon_chip() {
        let r = parse("@label(\"Score Tracker\") chip { var a: int = 0 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(ac.label.as_deref(), Some("Score Tracker"));
        assert!(!ac.closed);
    }

    #[test]
    fn closed_annotation_on_named_chip_and_chip_forms() {
        let r = parse(
            "@closed chip Foo(x: int) { }\n\
             @closed chip on t { }\n\
             @closed chip let a = 1\n\
             @label(\"consts\") @closed chip { }",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Chip(c) = &r.ast.decls[0] else { panic!() };
        assert!(c.closed);
        for i in 1..=2 {
            let TopDecl::AnonChip(ac) = &r.ast.decls[i] else {
                panic!("decl {i}: {:?}", r.ast.decls[i])
            };
            assert!(ac.closed, "decl {i} should be closed");
        }
        let TopDecl::AnonChip(ac) = &r.ast.decls[3] else { panic!() };
        assert!(ac.closed);
        assert_eq!(ac.label.as_deref(), Some("consts"));
    }

    #[test]
    fn label_stacks_with_side_on_ports_any_order() {
        let r = parse(
            "@left @label(\"Fire!\") in t: exec\n@label(\"Total\") @right out s = t",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::In(i) = &r.ast.decls[0] else { panic!() };
        assert_eq!(i.side, Some(PortSide::Left));
        assert_eq!(i.label.as_deref(), Some("Fire!"));
        let TopDecl::Out(o) = &r.ast.decls[1] else { panic!() };
        assert_eq!(o.side, Some(PortSide::Right));
        assert_eq!(o.label.as_deref(), Some("Total"));
    }

    #[test]
    fn closed_on_port_errors() {
        let r = parse("@closed in t: exec", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@closed is not allowed on 'in'/'out'")),
            "diags: {:?}",
            r.diagnostics
        );
        // The port itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::In(_)));
    }

    #[test]
    fn label_argument_errors() {
        let r = parse("@label chip { }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@label requires a string argument")),
            "diags: {:?}",
            r.diagnostics
        );
        let r = parse("@label(\"\") chip { }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@label text must not be empty")),
            "diags: {:?}",
            r.diagnostics
        );
        let r = parse("@label(\"a\") @label(\"b\") chip { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("duplicate @label")),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn closed_open_chip_contradiction_errors() {
        let r = parse("@closed open chip { var a: int = 0 }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("@closed cannot be combined with 'open chip'")),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn annotation_on_mod_errors() {
        let r = parse("@label(\"x\") mod inc(v: int) -> int { return v + 1 }", "test");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("annotations are not allowed on 'mod'")),
            "diags: {:?}",
            r.diagnostics
        );
        // The mod itself still parses.
        assert!(matches!(&r.ast.decls[0], TopDecl::Chip(c) if c.inline));
    }

    #[test]
    fn unknown_annotation_lists_all_words() {
        let r = parse("@middle in a: bool", "test");
        assert!(
            r.diagnostics[0]
                .message
                .contains("expected @left, @right, @top, @bottom, @label, or @closed"),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn open_chip_still_parses_as_noop() {
        let r = parse("open chip { var a: int = 0 }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else { panic!() };
        assert!(ac.open);
        assert!(!ac.closed);
    }

    #[test]
    fn chip_annotations_parse_at_statement_level() {
        let r = parse("chip Outer(x: int) { @closed chip { var a: int = 0 } }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Chip(c) = &r.ast.decls[0] else { panic!() };
        let Stmt::AnonChip(ac) = &c.body.stmts[0] else {
            panic!("stmt: {:?}", c.body.stmts[0])
        };
        assert!(ac.closed);
    }
}
