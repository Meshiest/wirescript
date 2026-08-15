//! The `@`-annotation grammar: module-level annotations, module labels and
//! module docs, plus the per-declaration annotation set.

use super::*;

/// Annotations consumed before a declaration. Each keeps its source range
/// for error reporting at the consuming site.
#[derive(Default)]
pub(super) struct ParsedAnnotations {
    pub(super) side: Option<(PortSide, SourceRange)>,
    pub(super) label: Option<(String, SourceRange)>,
    /// `@label(expr)` — the general-expression form (anything besides a bare
    /// string literal). Const-folded at lowering; at most one of `label` /
    /// `label_expr` is ever set.
    pub(super) label_expr: Option<(Expr, SourceRange)>,
    pub(super) closed: Option<SourceRange>,
    pub(super) nofold: Option<SourceRange>,
    pub(super) invisible: Option<SourceRange>,
}

/// Result of [`Parser::collect_module_annotations`] — which module-level
/// fold annotations (if any) opened the file.
#[derive(Default)]
pub(super) struct ModuleAnnotations {
    pub(super) no_fold: bool,
    pub(super) fold: bool,
    pub(super) layout: Option<crate::ast::LayoutName>,
    pub(super) flat: bool,
    pub(super) invisible: bool,
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

impl<'a> Parser<'a> {
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

    pub(super) fn collect_module_label(&mut self) -> Option<Expr> {
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
    pub(super) fn collect_module_annotations(&mut self) -> ModuleAnnotations {
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
    pub(super) fn collect_module_doc(&mut self) -> Option<String> {
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

    /// Consume a run of leading annotations (`@left`-style sides,
    /// `@label("…")`, `@closed`). Newlines after each annotation are eaten so
    /// annotations may sit on their own lines above the declaration.
    pub(super) fn parse_annotations(&mut self) -> ParsedAnnotations {
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
}
