//! Hand-written recursive-descent + Pratt parser for wirescript.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Pos, Severity, SourceRange};
use crate::lexer::{InterpPart as LexInterpPart, Token, TokenKind, TokenValue, lex};

use crate::collections::HashMap;

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
        Expr::MapLit { entries, .. } => {
            for e in entries.iter_mut() {
                shift_expr_offsets(&mut e.key, origin);
                shift_expr_offsets(&mut e.value, origin);
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
    /// Line indentation and `//` comments of this file's source.
    pub source_map: SourceMap,
}

pub fn parse(source: &str, file: &str) -> ParseResult {
    let lexed = lex(source, file);
    let mut p = Parser::new(lexed.tokens, file, lexed.diagnostics);
    let script = p.parse_script();
    ParseResult {
        ast: script,
        diagnostics: p.diagnostics,
        doc_comments: p.doc_comments,
        source_map: lexed.source_map,
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
    /// `@label(expr)` — the general-expression form (anything besides a bare
    /// string literal). Const-folded at lowering; at most one of `label` /
    /// `label_expr` is ever set.
    label_expr: Option<(Expr, SourceRange)>,
    closed: Option<SourceRange>,
    nofold: Option<SourceRange>,
    invisible: Option<SourceRange>,
}

/// Result of [`Parser::collect_module_annotations`] — which module-level
/// fold annotations (if any) opened the file.
#[derive(Default)]
struct ModuleAnnotations {
    no_fold: bool,
    fold: bool,
    layout: Option<crate::ast::LayoutName>,
    flat: bool,
    invisible: bool,
}

#[derive(Copy, Clone)]
enum ModuleAnnKind {
    Fold,
    NoFold,
    Layout(crate::ast::LayoutName),
    Flat,
    Invisible,
}

/// Every annotation word the module-level run accepts. Both the run's opening
/// test and its same-line continuation test read this one list, so a new
/// module annotation cannot be recognized at the start of a run and then
/// dropped mid-run (which parses as a decl-scoped annotation and reports a
/// misleading "module-level only" error).
const MODULE_ANN_WORDS: &[&str] = &["nofold", "fold", "layout", "flat", "invisible"];

fn is_module_ann_word(word: &str) -> bool {
    MODULE_ANN_WORDS.contains(&word)
}

/// The accepted `@layout` spellings, rendered for a diagnostic. Reads off
/// [`crate::ast::LayoutName::ALL`] so a new engine cannot be added without the
/// error message learning about it.
fn layout_choices() -> String {
    crate::ast::LayoutName::ALL
        .iter()
        .map(|(name, _)| format!("@layout(\"{name}\")"))
        .collect::<Vec<_>>()
        .join(" or ")
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
            doc_comments: HashMap::default(),
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

    fn warn(&mut self, message: impl Into<String>, start: Pos, end: Pos) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
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
        let module_anns = self.collect_module_annotations();
        let module_label = self.collect_module_label();
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
            no_fold: module_anns.no_fold,
            fold: module_anns.fold,
            layout: module_anns.layout,
            flat: module_anns.flat,
            invisible: module_anns.invisible,
            module_label,
        }
    }

    /// If the file opens (after any module doc / annotations) with a
    /// `@label(<expr>)` that is separated from the first declaration by a blank
    /// line, consume it as the ROOT microchip's label (same blank-line rule as
    /// the other top-of-file annotations) and return its expression. A `@label`
    /// directly above a declaration (no blank line) is left untouched for
    /// `parse_annotations` to attach to that declaration.
    ///
    /// Uses a non-advancing look-ahead to find the matching `)` and check the
    /// blank-line separator BEFORE committing, so a decl-level `@label` never
    /// gets its expression parsed here (which would otherwise emit stray
    /// diagnostics before the roll-back).
    /// Non-advancing look-ahead: is the token at `idx` a module-level
    /// `@label(<expr>)` — i.e. `@label` `(` … `)` with balanced parens, followed
    /// by a blank line (or EOF)? Shared by [`Self::collect_module_label`] and the
    /// module-annotation run so the two passes compose (a run of `@invisible`
    /// etc. may hand off to a `@label` that follows it).
    fn module_label_follows(&self, idx: usize) -> bool {
        if self.peek_at(idx).kind != TokenKind::Annotation || self.peek_at(idx).text != "label" {
            return false;
        }
        if self.peek_at(idx + 1).kind != TokenKind::LParen {
            return false;
        }
        // Scan to the `)` matching the opening `(`, counting nested parens.
        let mut c = idx + 2;
        let mut depth = 1usize;
        loop {
            match self.peek_at(c).kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => return false, // unbalanced — leave for normal parse
                _ => {}
            }
            c += 1;
        }
        // Module-level only if a blank line (or EOF) follows the closing `)`.
        let after = self.peek_at(c + 1).kind;
        let after2 = self.peek_at(c + 2).kind;
        after == TokenKind::Eof
            || (after == TokenKind::Newline && matches!(after2, TokenKind::Newline | TokenKind::Eof))
    }

    fn collect_module_label(&mut self) -> Option<Expr> {
        if !self.module_label_follows(0) {
            return None; // not present, or decl-level — leave for parse_annotations
        }
        // Commit: advance past `@label` `(`, parse the expression, eat `)`.
        self.advance();
        self.advance();
        let expr = self.parse_expr();
        self.match_tok(TokenKind::RParen, None);
        self.eat_newlines();
        Some(expr)
    }

    /// If the file opens (after any module doc) with a run of module-level
    /// annotations ([`MODULE_ANN_WORDS`]) — one per line or several
    /// sharing a line — separated from the first declaration by a blank
    /// line, consume them and mark the whole module accordingly — same
    /// blank-line rule as module doc comments. A `@nofold`/`@fold` directly
    /// above a declaration (no blank line separating it from the rest of
    /// the file) is left alone: for `@nofold` that's the pre-existing
    /// decl-scoped mechanism; `@fold` has no decl-scoped meaning and falls
    /// through to `parse_annotations`'s "unknown annotation" error. If both
    /// `@fold` and `@nofold` are present, `@nofold` wins and a warning notes
    /// the conflict. `@layout("code")` participates in the same run under
    /// the same placement rules; a bad layout argument is reported here
    /// (once) when the run is module-level, with the malformed tokens
    /// consumed along with it.
    fn collect_module_annotations(&mut self) -> ModuleAnnotations {
        let mut spans: Vec<(ModuleAnnKind, Pos, Pos)> = Vec::new();
        // Errors found while scanning a `@layout` argument are held here and
        // only reported if the run turns out to be module-level (i.e. it is
        // consumed via `finish_module_annotations`). On the bail-out paths
        // the tokens are left for the declaration loop, which reports its
        // own diagnostic for them.
        let mut errors: Vec<(String, Pos, Pos)> = Vec::new();
        let mut cursor = 0usize;
        loop {
            let t = self.peek_at(cursor);
            if t.kind != TokenKind::Annotation || !is_module_ann_word(t.text.as_str()) {
                break;
            }
            let (t_start, mut t_end) = (t.start, t.end);
            let kind = match t.text.as_str() {
                "fold" => Some(ModuleAnnKind::Fold),
                "nofold" => Some(ModuleAnnKind::NoFold),
                "flat" => Some(ModuleAnnKind::Flat),
                "invisible" => Some(ModuleAnnKind::Invisible),
                _ => {
                    // @layout("<name>") — the `(` Str `)` tokens follow directly.
                    if self.peek_at(cursor + 1).kind == TokenKind::LParen
                        && self.peek_at(cursor + 2).kind == TokenKind::Str
                        && self.peek_at(cursor + 3).kind == TokenKind::RParen
                    {
                        let s_tok = self.peek_at(cursor + 2);
                        let name = match &s_tok.value {
                            Some(TokenValue::Str(s)) => s.clone(),
                            _ => s_tok.text.clone(),
                        };
                        t_end = self.peek_at(cursor + 3).end;
                        cursor += 3;
                        if let Some(choice) = crate::ast::LayoutName::parse(&name) {
                            Some(ModuleAnnKind::Layout(choice))
                        } else {
                            errors.push((
                                format!("unknown layout \"{name}\"; expected {}", layout_choices()),
                                t_start,
                                t_end,
                            ));
                            None // consumed but invalid — keeps later decls parsing clean
                        }
                    } else {
                        // Missing or malformed argument. Swallow any `( … )`
                        // (or an unclosed `( …` cut off by the line end) so
                        // the run keeps scanning and the argument tokens are
                        // consumed along with it.
                        if self.peek_at(cursor + 1).kind == TokenKind::LParen {
                            let mut depth = 0usize;
                            loop {
                                let k = self.peek_at(cursor + 1).kind;
                                if k == TokenKind::Newline || k == TokenKind::Eof {
                                    break;
                                }
                                cursor += 1;
                                t_end = self.peek_at(cursor).end;
                                match k {
                                    TokenKind::LParen => depth += 1,
                                    TokenKind::RParen => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        errors.push((
                            format!("@layout requires a string argument: {}", layout_choices()),
                            t_start,
                            t_end,
                        ));
                        None
                    }
                }
            };
            let after_tok = self.peek_at(cursor + 1);
            let after = after_tok.kind;
            if after == TokenKind::Eof {
                // The annotation is the very last token in the file.
                if let Some(k) = kind {
                    spans.push((k, t_start, t_end));
                }
                return self.finish_module_annotations(spans, errors, cursor + 1);
            }
            let same_line_next =
                after == TokenKind::Annotation && is_module_ann_word(after_tok.text.as_str());
            if after != TokenKind::Newline && !same_line_next {
                // Not alone on its own line, and not continued by another
                // module annotation on the same line — decl-scoped, leave
                // the whole run untouched (nothing consumed).
                return ModuleAnnotations::default();
            }
            if let Some(k) = kind {
                spans.push((k, t_start, t_end));
            }
            if same_line_next {
                // Another module annotation follows directly on the same
                // line — keep scanning (the next loop iteration checks it).
                cursor += 1;
                continue;
            }
            let after2 = self.peek_at(cursor + 2).kind;
            if after2 == TokenKind::Newline || after2 == TokenKind::Eof {
                // Blank line (or EOF) follows this annotation's line — the
                // run ends here and is module-level.
                return self.finish_module_annotations(spans, errors, cursor + 2);
            }
            // Otherwise another annotation may follow directly on the next
            // line — keep scanning (the next loop iteration checks it).
            cursor += 2;
        }
        // The run stopped at a token that isn't a module-annotation word. If it
        // is a blank-line-separated `@label(<expr>)` (its own later pass) and we
        // already collected module annotations, finish the run here — consuming
        // those annotations and leaving the `@label` for `collect_module_label`
        // — so e.g. `@invisible` directly above a module `@label` isn't lost.
        if !spans.is_empty() && self.module_label_follows(cursor) {
            return self.finish_module_annotations(spans, errors, cursor);
        }
        ModuleAnnotations::default()
    }

    /// Consume the `consumed` tokens making up a module-level annotation run,
    /// report any `errors` the scan buffered for it, eat the trailing blank
    /// line(s), and fold `spans` into a [`ModuleAnnotations`], warning if
    /// both fold annotations were present.
    fn finish_module_annotations(
        &mut self,
        spans: Vec<(ModuleAnnKind, Pos, Pos)>,
        errors: Vec<(String, Pos, Pos)>,
        consumed: usize,
    ) -> ModuleAnnotations {
        for (message, start, end) in errors {
            self.error(message, start, end);
        }
        for _ in 0..consumed {
            self.advance();
        }
        self.eat_newlines();
        let mut result = ModuleAnnotations::default();
        let (mut first_start, mut last_end) = (None, None);
        for (kind, start, end) in spans {
            if first_start.is_none() {
                first_start = Some(start);
            }
            last_end = Some(end);
            match kind {
                ModuleAnnKind::Fold => result.fold = true,
                ModuleAnnKind::NoFold => result.no_fold = true,
                ModuleAnnKind::Flat => result.flat = true,
                ModuleAnnKind::Invisible => result.invisible = true,
                ModuleAnnKind::Layout(choice) => {
                    // No engine outranks another, so the last spelling wins and
                    // the file is told its earlier one was discarded.
                    if result.layout.is_some_and(|prev| prev != choice) {
                        self.warn("module-level @layout is set twice; the last one wins", start, end);
                    }
                    result.layout = Some(choice);
                }
            }
        }
        if result.fold && result.no_fold {
            self.warn(
                "module-level @fold and @nofold conflict; @nofold wins",
                first_start.unwrap(),
                last_end.unwrap(),
            );
        }
        result
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
                    let mut decl = self.parse_let_decl();
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
                return Some(self.parse_mod_decl());
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
                "let" => return Some(self.parse_let_decl()),
                "on" => return Some(TopDecl::Handler(self.parse_handler(false))),
                "array" => return Some(self.parse_array_decl()),
                "map" => return Some(self.parse_map_decl()),
                "chip" => return Some(self.parse_chip_decl(false, None, None, false, false)),
                "mod" => return Some(self.parse_mod_decl()),
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

    fn parse_var_decl(
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

    fn parse_in_decl(
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

    /// Consume a run of leading annotations (`@left`-style sides,
    /// `@label("…")`, `@closed`). Newlines after each annotation are eaten so
    /// annotations may sit on their own lines above the declaration.
    fn parse_annotations(&mut self) -> ParsedAnnotations {
        let mut anns = ParsedAnnotations::default();
        while self.check(TokenKind::Annotation, None) {
            let tok = self.advance();
            match tok.text.as_str() {
                "label" => {
                    // Fast path (unchanged): a single string literal —
                    // `@label("text")`. Anything else inside the parens
                    // (a named constant, arithmetic, …) parses as a general
                    // expression and is const-folded to display text at
                    // lowering (typecheck rejects a non-constant one).
                    let mut text: Option<(String, Pos)> = None;
                    let mut expr: Option<(Expr, Pos)> = None;
                    if self.match_tok(TokenKind::LParen, None).is_some() {
                        if self.check(TokenKind::Str, None)
                            && self.peek_at(1).kind == TokenKind::RParen
                        {
                            let s_tok = self.advance();
                            let s = match &s_tok.value {
                                Some(TokenValue::Str(s)) => s.clone(),
                                _ => s_tok.text.clone(),
                            };
                            text = Some((s, s_tok.end));
                            self.match_tok(TokenKind::RParen, None);
                        } else if !self.check(TokenKind::RParen, None) {
                            let e = self.parse_expr();
                            let end = e.range().end;
                            expr = Some((e, end));
                            self.match_tok(TokenKind::RParen, None);
                        } else {
                            self.match_tok(TokenKind::RParen, None); // `@label()`
                        }
                    }
                    match (text, expr) {
                        (Some((s, end)), _) if s.is_empty() => {
                            self.error(
                                "@label text must not be empty".to_string(),
                                tok.start,
                                end,
                            );
                        }
                        (Some((s, end)), _) => {
                            if anns.label.is_some() || anns.label_expr.is_some() {
                                self.error("duplicate @label".to_string(), tok.start, end);
                            } else {
                                anns.label = Some((s, self.make_range(tok.start, end)));
                            }
                        }
                        (None, Some((e, end))) => {
                            if anns.label.is_some() || anns.label_expr.is_some() {
                                self.error("duplicate @label".to_string(), tok.start, end);
                            } else {
                                anns.label_expr = Some((e, self.make_range(tok.start, end)));
                            }
                        }
                        (None, None) => self.error(
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
                "invisible" => {
                    if anns.invisible.is_some() {
                        self.error("duplicate @invisible".to_string(), tok.start, tok.end);
                    } else {
                        anns.invisible = Some(self.make_range(tok.start, tok.end));
                    }
                }
                "nofold" => {
                    if anns.nofold.is_some() {
                        self.error("duplicate @nofold".to_string(), tok.start, tok.end);
                    } else {
                        anns.nofold = Some(self.make_range(tok.start, tok.end));
                    }
                }
                "layout" => {
                    if self.match_tok(TokenKind::LParen, None).is_some() {
                        // Consume through the matching `)` (stopping at the
                        // line end) so a malformed argument list doesn't leak
                        // stray tokens into the declaration parser.
                        let mut depth = 1usize;
                        while depth > 0 {
                            let k = self.peek().kind;
                            if k == TokenKind::Newline || k == TokenKind::Eof {
                                break;
                            }
                            match self.advance().kind {
                                TokenKind::LParen => depth += 1,
                                TokenKind::RParen => depth -= 1,
                                _ => {}
                            }
                        }
                    }
                    self.error(
                        "'@layout' is module-level only — put it at the very top of the file with a blank line before the first declaration (and after any module doc comment)".to_string(),
                        tok.start,
                        tok.end,
                    );
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
                    None => {
                        let msg = if w == "fold" || w == "flat" {
                            format!(
                                "'@{w}' is module-level only — put it at the very top of the file with a blank line before the first declaration (and after any module doc comment)"
                            )
                        } else {
                            format!(
                                "unknown annotation '@{}'; expected @left, @right, @top, @bottom, @label, @closed, @invisible, or @nofold",
                                w
                            )
                        };
                        self.error(msg, tok.start, tok.end);
                    },
                },
            }
            while self.check(TokenKind::Newline, None) {
                self.advance();
            }
        }
        anns
    }

    fn parse_out_binding(
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
                no_fold: false,
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
                no_fold: false,
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
            no_fold: false,
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

    // `var name: ElementType[]`
    fn parse_array_decl(&mut self) -> TopDecl {
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
    fn parse_map_decl(&mut self) -> TopDecl {
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
    fn parse_chip_decl(
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
            type_params,
            inputs,
            outputs,
            body,
            range: self.make_range(start, end),
            inline: false,
            label,
            label_expr,
            closed,
            no_fold,
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
        let type_params = self.parse_type_params();
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
            type_params,
            inputs,
            outputs,
            body,
            range: self.make_range(start, end),
            inline: true,
            label: None,
            label_expr: None,
            closed: false,
            no_fold: false,
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

        // `on Foo(...)` where `Foo` is a builtin CALL that is NOT an event is a
        // call-expression trigger — `on ServerUptime()` fires whenever the call's
        // value changes, desugared like any other expr trigger into
        // `let _on_expr_N = Foo(...)` + `on _on_expr_N`. This is distinct from the
        // event-with-args form (`on Clock(...)`, `on CustomEvent(...)`), whose name
        // resolves as an event and stays a plain trigger with config/data args.
        if get(i).kind == TokenKind::Ident
            && get(i + 1).kind == TokenKind::LParen
            && crate::catalog::events::find_event(&get(i).text).is_none()
            && crate::catalog::calls::calls().get(get(i).text.as_str()).is_some()
        {
            return true;
        }

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
    fn parse_handler(&mut self, no_fold: bool) -> Handler {
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
                no_fold: false,
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
        let mut params: Vec<HandlerParam> = Vec::new();
        let mut config: Vec<HandlerConfigArg> = Vec::new();
        if self.match_tok(TokenKind::LParen, None).is_some() {
            while !self.check(TokenKind::RParen, None) {
                if self.check(TokenKind::Ident, None) {
                    let tok = self.expect(TokenKind::Ident, None);
                    let prange = self.make_range(tok.start, tok.end);
                    let name = tok.text;
                    if self.match_tok(TokenKind::Op, Some("=")).is_some() {
                        let value = self.parse_expr();
                        config.push(HandlerConfigArg::Named { name, value });
                    } else if self.match_tok(TokenKind::Colon, None).is_some() {
                        // Typed data param: `on CustomEvent(a: int, b: float)`.
                        let typ = self.parse_type();
                        params.push(HandlerParam { name, ty: Some(typ), range: prange });
                    } else {
                        params.push(HandlerParam { name, ty: None, range: prange });
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
            no_fold,
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
            self.advance(); // consume `.`
            let member = self.expect(TokenKind::Ident, None);
            name = format!("{name}.{}", member.text);
            end = member.end;
        }
        // Generic application `Name<Arg, ...>` (e.g. `Map<string, int>`,
        // `Array<int>`, `Ref<Point>`). `Array`/`Ref` desugar straight to the
        // existing postfix forms so downstream code sees no new shape.
        if self.check(TokenKind::Op, Some("<")) {
            self.advance(); // consume `<`
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
            no_fold: false,
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
                    type_args: Vec::new(),
                    range: self.make_range(start, end),
                };
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
                self.advance(); // '['
                let k = self.parse_expr();
                self.expect(TokenKind::RBracket, None);
                k
            } else {
                self.parse_expr()
            };
            // Separator: `=>` or `:`.
            if self.match_tok(TokenKind::FatArrow, None).is_none() {
                self.expect(TokenKind::Colon, None);
            }
            let value = self.parse_expr();
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
    fn out_binding_rejects_trailing_tokens() {
        // `out aw(wa)` reads like an anonymous output of a call, but an output
        // port is always `out NAME` / `out NAME = expr`. The trailing `(wa)`
        // used to be silently dropped and re-parsed as its own declaration.
        let r = parse("let wa = (1, 2)\nout aw(wa)", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("out")),
            "trailing tokens after an output port name must be reported: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn out_binding_forms_still_parse() {
        parse_ok("out foo");
        parse_ok("out foo: int");
        parse_ok("out foo = 1 + 2");
        parse_ok("out foo: int = 3");
        parse_ok("chip {\n  out foo = 1\n}");
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
                assert_eq!(h.params[0].name, "char");
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
    fn handler_call_trigger_desugars_to_let_plus_handler() {
        // `on ServerUptime() { … }` — a builtin CALL used as a trigger — desugars
        // to `let _on_expr_0 = ServerUptime()` + `on _on_expr_0 { … }`, exactly
        // like the value-capture pattern `let t = ServerUptime(); on t`. This must
        // NOT be mistaken for the event-with-args form (`on Clock(...)`).
        let src = "on ServerUptime() { BroadcastChatMessage(\"tick\") }";
        let s = parse_ok(src);
        // Expected: Let(_on_expr_0 = ServerUptime()), Handler(_on_expr_0).
        assert_eq!(s.decls.len(), 2, "decls: {:?}", s.decls);
        match &s.decls[0] {
            TopDecl::Let(l) => match &l.binding {
                LetBinding::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident binding"),
            },
            d => panic!("expected Let, got {:?}", d),
        }
        match &s.decls[1] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "_on_expr_0"),
                _ => panic!("expected Ident trigger"),
            },
            d => panic!("expected Handler, got {:?}", d),
        }
    }

    #[test]
    fn handler_event_with_args_is_not_a_call_trigger() {
        // `on Clock(enabled = true)` stays a plain event handler — its name IS an
        // event, so the call-trigger path must not hijack it into an expr trigger.
        let src = "on Clock(enabled = true) { }";
        let s = parse_ok(src);
        assert_eq!(s.decls.len(), 1, "no synthetic let for an event: {:?}", s.decls);
        match &s.decls[0] {
            TopDecl::Handler(h) => match &h.trigger {
                Trigger::Ident { name, .. } => assert_eq!(name, "Clock"),
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
    fn array_and_map_decl_keywords_are_rejected() {
        // The `array`/`map` declaration keywords were removed in favor of
        // `var NAME: T[]` / `var NAME: Map<K,V>` (identical storage). Using them
        // is a parse error pointing at the `var` replacement.
        let ra = crate::parser::parse("array xs: int[]", "test");
        assert!(
            ra.diagnostics
                .iter()
                .any(|d| d.message.contains("`array` declarations have been removed")),
            "array decl must be rejected: {:?}",
            ra.diagnostics
        );
        let rm = crate::parser::parse("map m: Map<string, int>", "test");
        assert!(
            rm.diagnostics
                .iter()
                .any(|d| d.message.contains("`map` declarations have been removed")),
            "map decl must be rejected: {:?}",
            rm.diagnostics
        );
        // The `var` forms parse clean into `TopDecl::Var`.
        let rv = crate::parser::parse("var xs: int[]", "test");
        assert!(rv.diagnostics.is_empty(), "var array: {:?}", rv.diagnostics);
        match &rv.ast.decls[0] {
            TopDecl::Var(v) => assert_eq!(v.name, "xs"),
            d => panic!("expected Var, got {:?}", d),
        }
        let rvm = crate::parser::parse("var m: Map<string, int>", "test");
        assert!(rvm.diagnostics.is_empty(), "var map: {:?}", rvm.diagnostics);
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
    fn parses_generic_decl_headers() {
        use crate::ast::TopDecl;
        let mod_of = |s: &str| {
            crate::parser::parse(s, "t").ast.decls.into_iter()
                .find_map(|d| if let TopDecl::Chip(c) = d { Some(c) } else { None }).expect("a mod/chip")
        };
        let c = mod_of("mod pick<T>(c: bool, a: T, b: T) -> T { return a }\n");
        assert_eq!(c.type_params.len(), 1);
        assert_eq!(c.type_params[0].name, "T");
        assert!(c.type_params[0].bound.is_none());

        let c2 = mod_of("mod clamp<T: Numeric>(v: T) -> T { return v }\n");
        assert_eq!(c2.type_params.len(), 1);
        assert!(c2.type_params[0].bound.is_some(), "T: Numeric has a bound");

        let c3 = mod_of("mod two<T, U>(a: T, b: U) { }\n");
        assert_eq!(c3.type_params.len(), 2);
        assert_eq!(c3.type_params[1].name, "U");

        let ast = crate::parser::parse("type Pair<T> = { a: T, b: T }\n", "t").ast;
        let ta = ast.decls.iter().find_map(|d| if let TopDecl::TypeAlias(t) = d { Some(t) } else { None }).expect("alias");
        assert_eq!(ta.type_params.len(), 1);
        assert_eq!(ta.type_params[0].name, "T");

        // all of the above (and a non-generic mod) parse with no errors
        for s in ["mod pick<T>(a: T) -> T { return a }\n", "mod plain(a: int) -> int { return a }\n",
                  "type Grid<T> = T[]\n"] {
            assert!(crate::parser::parse(s, "t").diagnostics.iter()
                .all(|d| d.severity != crate::diagnostic::Severity::Error), "should parse cleanly: {s}");
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
    fn invisible_before_non_port_decl_errors() {
        // `@invisible` must participate in the bare-@nofold validity guard
        // exactly like `@closed`: it is a port annotation, so pairing it with
        // `@nofold` before a plain `var` is not the special bare-@nofold case
        // and must be diagnosed (not silently discarded).
        let r = parse("@invisible @nofold var x: int = 0", "test");
        assert!(
            r.diagnostics.iter().any(|d| d
                .message
                .contains("must be followed by an 'in', 'out', or chip declaration")),
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
    fn label_expr_annotation_parses_general_expressions() {
        // Anything besides a bare string literal parses as a general
        // expression, stored separately from the string form. Const-folding
        // it to display text happens at lowering (typecheck rejects a
        // non-constant one).
        let r = parse("@label(1 + 2) chip { }", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::AnonChip(ac) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert!(ac.label.is_none());
        assert!(matches!(ac.label_expr, Some(Expr::BinOp { .. })));
    }

    #[test]
    fn label_expr_and_label_string_are_mutually_exclusive() {
        let r = parse("@label(\"a\") @label(x) chip { }", "test");
        assert!(
            r.diagnostics.iter().any(|d| d.message.contains("duplicate @label")),
            "diags: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn label_annotation_is_allowed_on_var() {
        // Previously any annotation besides a bare `@nofold` before `var`
        // fell through to the generic "must be followed by ..." parse error.
        let r = parse("@label(\"HP\") var hp: int = 0", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Var(v) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(v.label.as_deref(), Some("HP"));
    }

    #[test]
    fn label_and_nofold_stack_on_var() {
        let r = parse("@label(\"HP\") @nofold var hp: int = 0", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let TopDecl::Var(v) = &r.ast.decls[0] else {
            panic!("decl 0: {:?}", r.ast.decls[0])
        };
        assert_eq!(v.label.as_deref(), Some("HP"));
        assert!(v.no_fold);
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
            r.diagnostics[0].message.contains(
                "expected @left, @right, @top, @bottom, @label, @closed, @invisible, or @nofold"
            ),
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

    #[test]
    fn module_layout_annotation_sets_flag() {
        let p = crate::parser::parse("@layout(\"code\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_layout_annotation_mixes_with_fold_run() {
        let p = crate::parser::parse("@fold\n@layout(\"code\")\n\nvar x: int = 0\n", "t");
        assert!(p.ast.fold);
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_layout_annotation_selects_the_cube() {
        let p = crate::parser::parse("@layout(\"cube\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// No engine outranks another, so the file is told which one survived
    /// rather than silently getting whichever the fold order happened to keep.
    #[test]
    fn two_layout_annotations_warn_and_the_last_wins() {
        let p =
            crate::parser::parse("@layout(\"code\")\n@layout(\"cube\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube));
        assert_eq!(
            p.diagnostics.iter().filter(|d| d.message.contains("set twice")).count(),
            1,
            "{:?}",
            p.diagnostics
        );
    }

    #[test]
    fn var_initializer_without_equals_is_an_error() {
        // `var x: type LITERAL` (missing `=`) must not silently drop the value.
        for src in ["var test: string \"hello\"\n", "var n: int 5\n"] {
            let p = crate::parser::parse(src, "t");
            assert!(
                p.diagnostics
                    .iter()
                    .any(|d| d.message.contains("missing `=`")),
                "missing `=` before an initializer must error; src {src:?} gave {:?}",
                p.diagnostics
            );
        }
    }

    #[test]
    fn var_without_initializer_is_allowed() {
        // A bare `var x: int` (declaration ends after the type) is valid.
        for src in ["var x: int\n", "var y: bool\nvar z: int = 0\n", "chip { var q: int }\n"] {
            let p = crate::parser::parse(src, "t");
            assert!(
                p.diagnostics.is_empty(),
                "an uninitialized var is valid; src {src:?} gave {:?}",
                p.diagnostics
            );
        }
    }

    #[test]
    fn module_label_blank_line_separated_labels_the_root() {
        // `@label(expr)` at the top of the file, separated from the first decl
        // by a blank line, is a MODULE-level label (root chip) — not attached
        // to the var below it.
        let p = crate::parser::parse("@label(title)\n\nvar title: string = \"hi\"\n", "t");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        assert!(
            p.ast.module_label.is_some(),
            "a blank-line-separated top @label is module-level"
        );
        match &p.ast.decls[0] {
            TopDecl::Var(v) => assert!(
                v.label.is_none() && v.label_expr.is_none(),
                "the var must NOT also carry the module @label"
            ),
            other => panic!("expected var, got {other:?}"),
        }
    }

    #[test]
    fn module_annotation_run_hands_off_to_module_label() {
        // `@invisible` directly above a blank-line-separated module `@label`
        // keeps BOTH: the run finishes (module stays invisible) and the `@label`
        // is claimed as the root label — no lost-annotation error.
        let p =
            crate::parser::parse("@invisible\n@label(title)\n\nvar title: string = \"hi\"\n", "t");
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
        assert!(
            p.ast.invisible,
            "@invisible must survive the hand-off to a following module @label"
        );
        assert!(
            p.ast.module_label.is_some(),
            "the @label is still module-level"
        );
    }

    #[test]
    fn attached_label_stays_decl_level() {
        // No blank line: `@label(expr)` attaches to the var below it (the
        // declaration-level self-label), and there is no module-level label.
        let p = crate::parser::parse("@label(title)\nvar title: string = \"hi\"\n", "t");
        assert!(
            p.ast.module_label.is_none(),
            "an attached top @label is NOT module-level"
        );
        match &p.ast.decls[0] {
            TopDecl::Var(v) => assert!(
                v.label_expr.is_some(),
                "the var carries the attached @label expression"
            ),
            other => panic!("expected var, got {other:?}"),
        }
    }

    #[test]
    fn repeating_one_layout_annotation_is_not_a_conflict() {
        let p =
            crate::parser::parse("@layout(\"cube\")\n@layout(\"cube\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube));
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// The diagnostic is generated from the accepted spellings, so a new
    /// engine cannot ship with an error message that omits it.
    #[test]
    fn unknown_layout_error_offers_every_accepted_name() {
        let p = crate::parser::parse("@layout(\"spiral\")\n\nvar x: int = 0\n", "t");
        let msg = &p.diagnostics[0].message;
        for (name, _) in crate::ast::LayoutName::ALL {
            assert!(msg.contains(name), "{name} missing from {msg:?}");
        }
    }

    #[test]
    fn unknown_layout_name_errors() {
        let p = crate::parser::parse("@layout(\"grid\")\n\nvar x: int = 0\n", "t");
        assert!(p.ast.layout.is_none());
        assert!(p.diagnostics.iter().any(|d| d.message.contains("unknown layout")));
        assert_eq!(p.diagnostics.len(), 1, "{:?}", p.diagnostics);
    }

    #[test]
    fn layout_without_argument_errors() {
        let p = crate::parser::parse("@layout\n\nvar x: int = 0\n", "t");
        assert!(p.ast.layout.is_none());
        assert!(p.diagnostics.iter().any(|d| d.message.contains("requires a string argument")));
        assert_eq!(p.diagnostics.len(), 1, "{:?}", p.diagnostics);
    }

    #[test]
    fn malformed_layout_argument_reports_one_diagnostic() {
        for src in [
            "@layout(\n\nvar x: int = 0\n",
            "@layout(5)\n\nvar x: int = 0\n",
            "@layout(\"code\"\n\nvar x: int = 0\n",
        ] {
            let p = crate::parser::parse(src, "t");
            assert!(p.ast.layout.is_none(), "{src:?}");
            assert_eq!(p.diagnostics.len(), 1, "{src:?} -> {:?}", p.diagnostics);
            assert!(
                p.diagnostics[0].message.contains("requires a string argument"),
                "{src:?} -> {:?}",
                p.diagnostics
            );
        }
    }

    #[test]
    fn decl_scoped_malformed_layout_argument_is_fully_consumed() {
        let p = crate::parser::parse("@layout(5)\nvar x: int = 0\n", "t");
        assert!(p.ast.layout.is_none());
        assert!(
            p.diagnostics.iter().any(|d| d.message.contains("module-level only")),
            "{:?}",
            p.diagnostics
        );
        assert!(
            p.diagnostics.iter().all(|d| !d.message.contains("unexpected token")),
            "argument tokens must not leak into the declaration parser: {:?}",
            p.diagnostics
        );
    }

    #[test]
    fn decl_scoped_layout_errors() {
        let p = crate::parser::parse("@layout(\"code\")\nvar x: int = 0\n", "t");
        // No blank line → decl-scoped → module-level-only error.
        assert!(p.ast.layout.is_none());
        assert!(p.diagnostics.iter().any(|d| d.message.contains("module-level only")));
    }

    #[test]
    fn module_annotations_may_share_one_line() {
        let p = crate::parser::parse("@layout(\"code\") @fold\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.ast.fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_annotations_same_line_reverse_order() {
        let p = crate::parser::parse("@fold @layout(\"code\")\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.ast.fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn module_flat_annotation_sets_flag() {
        let p = crate::parser::parse("@flat\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// `@flat` is independent of the layout choice — both spellings, both
    /// orders, and on one line or two.
    #[test]
    fn module_flat_composes_with_layout_and_fold() {
        for src in [
            "@flat\n@layout(\"cube\")\n\nvar x: int = 0\n",
            "@layout(\"cube\")\n@flat\n\nvar x: int = 0\n",
            "@flat @layout(\"cube\")\n\nvar x: int = 0\n",
            "@layout(\"cube\") @flat\n\nvar x: int = 0\n",
        ] {
            let p = crate::parser::parse(src, "t");
            assert!(p.ast.flat, "{src:?}");
            assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Cube), "{src:?}");
            assert!(p.diagnostics.is_empty(), "{src:?} -> {:?}", p.diagnostics);
        }
        let p = crate::parser::parse("@fold @flat\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat && p.ast.fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    /// The run's opening test and its same-line continuation test read one
    /// allowlist. If `@flat` were only in the first, the run would stop at it
    /// and everything after it on the line would parse as decl-scoped.
    #[test]
    fn a_module_annotation_after_flat_on_one_line_stays_module_level() {
        let p = crate::parser::parse("@flat @nofold\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat);
        assert!(p.ast.no_fold);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn repeating_flat_is_not_a_conflict() {
        let p = crate::parser::parse("@flat\n@flat\n\nvar x: int = 0\n", "t");
        assert!(p.ast.flat);
        assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    }

    #[test]
    fn decl_scoped_flat_errors() {
        // No blank line → decl-scoped → module-level-only error.
        let p = crate::parser::parse("@flat\nvar x: int = 0\n", "t");
        assert!(!p.ast.flat);
        assert!(
            p.diagnostics.iter().any(|d| {
                d.message.contains("'@flat'") && d.message.contains("module-level only")
            }),
            "{:?}",
            p.diagnostics
        );
    }

    #[test]
    fn mixed_same_and_separate_lines() {
        let p = crate::parser::parse("@fold @layout(\"code\")\n@nofold\n\nvar x: int = 0\n", "t");
        assert_eq!(p.ast.layout, Some(crate::ast::LayoutName::Code));
        assert!(p.ast.no_fold);
        // @fold + @nofold still conflict-warn, exactly once.
        assert_eq!(
            p.diagnostics.iter().filter(|d| d.message.contains("conflict")).count(),
            1
        );
    }

    #[test]
    fn same_line_decl_scoped_annotations_still_hand_off() {
        // No blank line before the declaration → decl-scoped, module flags unset.
        let p = crate::parser::parse("@nofold @left\nin x: exec\n", "t");
        assert!(!p.ast.no_fold);
    }

    #[test]
    fn brace_disambiguation_preserved_and_bounded() {
        let no_err = |s: &str| crate::parser::parse(s, "t").diagnostics.iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error);
        // record literal stays a record
        assert!(no_err("let r = { x: 1, y: 2 }\n"), "record should parse");
        // map literal stays a map
        assert!(no_err("var m: Map<int,int> = { 1 => 2 }\n"), "map should parse");
        // block-expr braces still parse
        assert!(no_err("let b = { let x = 1\n x + 1 }\n"), "block-expr should parse");
        // Non-literal / non-trivial key expressions are valid maps — the key
        // is `parse_expr()`, so any expression may open an entry. These all
        // regressed under an allow-list precheck (misparsed as block-exprs,
        // spurious WSP001 on the `=>`); the reject-list precheck must let them
        // fall through to the `=>` scan and classify as maps.
        assert!(no_err("var m: Map<int,int> = { -1 => 1 }\n"), "unary-key map should parse");
        assert!(no_err("var m: Map<int,int> = { (1) => 1 }\n"), "paren-key map should parse");
        assert!(no_err("var m: Map<int,int> = { true => 1 }\n"), "bool-key map should parse");
        assert!(no_err("var m: Map<int,int> = { 1 + 1 => 2 }\n"), "binop-key map should parse");
        // an unbalanced expression-position brace must terminate quickly, not scan to EOF
        let big = format!("let x = {{ {}", "a a a a ".repeat(5000));
        let _ = crate::parser::parse(&big, "t"); // must simply COMPLETE (no hang)
    }

    #[test]
    fn parse_invisible_port_annotation() {
        let r = crate::parser::parse("@left @invisible in go: exec", "test");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        match &r.ast.decls[0] {
            TopDecl::In(p) => {
                assert!(p.invisible, "@invisible should set InDecl.invisible");
                assert_eq!(p.side, Some(crate::ast::PortSide::Left));
            }
            other => panic!("expected In decl, got {other:?}"),
        }
    }

    /// Walks a parsed script for the first `Expr::NestedPrefab`, returning its
    /// captured inner source. Only recurses through the handful of expression
    /// and statement kinds needed to reach a call argument inside a handler
    /// body — enough for this test, not a general-purpose AST visitor.
    fn find_nested_prefab_source(script: &Script) -> Option<String> {
        fn walk_expr(e: &Expr) -> Option<String> {
            match e {
                Expr::NestedPrefab { source, .. } => Some(source.clone()),
                Expr::Call { callee, args, .. } => walk_expr(callee).or_else(|| {
                    args.iter().find_map(|a| match a {
                        CallArg::Positional(e) | CallArg::Spread(e) => walk_expr(e),
                        CallArg::Named { value, .. } => walk_expr(value),
                    })
                }),
                _ => None,
            }
        }
        fn walk_stmt(s: &Stmt) -> Option<String> {
            match s {
                Stmt::Let(l) => walk_expr(&l.value),
                Stmt::ExprStmt(e) => walk_expr(&e.expr),
                Stmt::Assign(a) => walk_expr(&a.value),
                Stmt::Handler(h) => h.body.stmts.iter().find_map(walk_stmt),
                _ => None,
            }
        }
        script.decls.iter().find_map(|d| match d {
            TopDecl::Handler(h) => h.body.stmts.iter().find_map(walk_stmt),
            _ => None,
        })
    }

    #[test]
    fn parse_nested_prefab_expr() {
        let r = crate::parser::parse(
            "in go: exec\non go { let e = SpawnPrefab($```in a: exec```) }\n",
            "test",
        );
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        assert_eq!(
            find_nested_prefab_source(&r.ast),
            Some("in a: exec".to_string()),
            "should parse a NestedPrefab carrying the inner source"
        );
    }

    #[test]
    fn method_chain_continues_across_newline() {
        // A `.method(...)` on the line after its receiver continues the chain
        // (a leading-dot continuation), rather than parsing as two statements /
        // a stray-`.` error.
        let src = "in go: exec\nin obj: entity\non go {\n  obj\n    .SendCustomEvent(\"x\", 1)\n}\n";
        let r = crate::parser::parse(src, "test");
        assert!(r.diagnostics.is_empty(), "chain should parse: {:?}", r.diagnostics);
        let handler = r
            .ast
            .decls
            .iter()
            .find_map(|d| match d {
                TopDecl::Handler(h) => Some(h),
                _ => None,
            })
            .expect("a handler");
        assert_eq!(
            handler.body.stmts.len(),
            1,
            "the chain is one statement, not two: {:?}",
            handler.body.stmts
        );
        // That one statement is a chained call whose callee is `.SendCustomEvent`.
        match &handler.body.stmts[0] {
            Stmt::ExprStmt(es) => match &es.expr {
                Expr::Call { callee, .. } => assert!(
                    matches!(callee.as_ref(), Expr::FieldAccess { field, .. } if field == "SendCustomEvent"),
                    "callee should be a chained .SendCustomEvent, got {:?}",
                    es.expr
                ),
                other => panic!("expected a Call, got {other:?}"),
            },
            other => panic!("expected an ExprStmt, got {other:?}"),
        }
    }
}
