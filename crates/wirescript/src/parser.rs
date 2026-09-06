//! Hand-written recursive-descent + Pratt parser for wirescript.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Pos, Severity, SourceRange};
use crate::lexer::{InterpPart as LexInterpPart, Token, TokenKind, TokenValue, lex};

use crate::collections::HashMap;

mod types;
mod desugar;
use desugar::*;
mod anns;
mod expr;
mod handler;
use handler::*;
mod stmt;
mod decl;
mod pattern;

/// Doc comments keyed by the file and the start offset of the declaration
/// they precede.
///
/// The file is part of the key because imports merge several parses into one
/// map, so an offset alone collides across files and the last writer wins. A
/// chip's doc comment is baked into the world as its header text.
pub type DocComments = HashMap<(std::sync::Arc<str>, usize), String>;

/// The key [`DocComments`] stores a declaration under.
pub fn doc_key(range: &SourceRange) -> (std::sync::Arc<str>, usize) {
    (range.file.clone(), range.start.offset)
}

pub struct ParseResult {
    pub ast: Script,
    pub diagnostics: Vec<Diagnostic>,
    pub doc_comments: DocComments,
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

/// Test-only entry point for the pattern parser in isolation, without
/// wrapping it in a full `match` (which is the only surface syntax that
/// reaches `parse_pattern` normally).
#[cfg(test)]
pub fn parse_pattern_str(s: &str) -> Pattern {
    let lexed = lex(s, "t.ws");
    let mut p = Parser::new(lexed.tokens, "t.ws", lexed.diagnostics);
    p.parse_pattern()
}

// ---------- parser state ----------

struct Parser<'a> {
    tokens: Vec<Token>,
    file: &'a str,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    doc_comments: DocComments,
    /// Counter for generating unique synthetic binding names (`_on_expr_N`).
    expr_trigger_counter: usize,
    /// Synthetic `let` bindings queued by `parse_handler` for expression
    /// triggers.  The surrounding `parse_block` / `parse_script` loops drain
    /// this before inserting the handler itself.
    pending_stmts: Vec<Stmt>,
    /// When true, a trailing `{ ... }` after a path expression is NOT parsed as
    /// braced enum-variant construction (`Enum.Variant { f: v }`) - the `{` is
    /// left for the surrounding block header instead. Set only while parsing a
    /// header/condition expression (an `if` condition; extend to a `while`/`for`
    /// header or `match` scrutinee if those are added), so `if f.bar { }` reads
    /// as an `if` with a body, not a construction. Reset to false inside any
    /// bracketed sub-expression (`( )`, `[ ]`, call args, a record/map/array
    /// body), where a trailing `{` is unambiguous again. The Go-style
    /// composite-literal disambiguation.
    no_brace_construct: bool,
    /// Current nesting level, in expression operands, blocks and type levels.
    /// See [`MAX_NEST_DEPTH`].
    depth: usize,
    /// Set once the depth limit has been reported, so the one diagnostic is
    /// not repeated for every level still on the stack.
    depth_exceeded: bool,
}

/// The deepest nesting the parser will build.
///
/// Every later pass walks this tree recursively, typecheck, the analysis
/// walkers, and `Drop` on the boxed nodes themselves, so the depth the parser
/// accepts is the depth all of them have to survive. A stack overflow is not a
/// panic: it aborts the process, `catch_unwind` cannot see it, and an editor
/// running analysis on a 2 MiB worker thread dies without reporting anything.
/// Measured on that stack, the cliff is around 800 nested parens, 2000 `else
/// if` arms and 3000 `+` operands; this leaves headroom under the tightest of
/// those while sitting far above anything real code writes: across every `.ws`
/// file in the example and project corpus the deepest is 76.
pub(crate) const MAX_NEST_DEPTH: usize = 400;

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
            no_brace_construct: false,
            depth: 0,
            depth_exceeded: false,
        }
    }

    /// Take one nesting level. `false` means the caller must not descend: the
    /// program is past [`MAX_NEST_DEPTH`], which has been reported, and the
    /// token stream has been wound to EOF so every enclosing loop unwinds
    /// instead of spinning on input it can no longer consume.
    fn enter_nesting(&mut self) -> bool {
        self.depth += 1;
        if self.depth <= MAX_NEST_DEPTH {
            return true;
        }
        if !self.depth_exceeded {
            let t = self.peek().clone();
            self.error(
                format!(
                    "expression or block nests more than {MAX_NEST_DEPTH} levels deep; split it into named parts"
                ),
                t.start,
                t.end,
            );
            self.depth_exceeded = true;
            self.pos = self.tokens.len().saturating_sub(1);
        }
        false
    }

    fn leave_nesting(&mut self, levels: usize) {
        self.depth = self.depth.saturating_sub(levels);
    }

    fn collect_doc_comment(&mut self) -> Option<String> {
        let mut lines = Vec::new();
        while self.peek().kind == TokenKind::DocComment {
            lines.push(self.peek().text.clone());
            self.advance();
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

    /// Consume a balanced `{ ... }` block starting at the current `{`, returning
    /// the closing `}`'s end position (or the last token's end if unterminated).
    /// Used to recover a malformed braced-construction body so it is not left
    /// for the top-level declaration fallback to silently re-parse as a separate
    /// block. Assumes the current token is `{`.
    fn consume_balanced_braces(&mut self) -> Pos {
        let mut end = self.advance().end; // the opening `{`
        let mut depth = 1i32;
        while depth > 0 && self.peek().kind != TokenKind::Eof {
            let tok = self.advance();
            end = tok.end;
            match tok.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth -= 1,
                _ => {}
            }
        }
        end
    }

    fn make_range(&self, start: Pos, end: Pos) -> SourceRange {
        SourceRange::new(self.file, start, end)
    }

    fn error(&mut self, message: impl Into<String>, start: Pos, end: Pos) {
        // Past the depth limit the token stream has been wound to EOF, so every
        // construct still on the stack is about to report its own missing
        // `)`/`}`. The one message that explains the file is already recorded;
        // the cascade behind it only buries it.
        if self.depth_exceeded {
            return;
        }
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: "WSP001".to_string(),
            message: message.into(),
            range: self.make_range(start, end),
        });
    }

    fn warn(&mut self, message: impl Into<String>, start: Pos, end: Pos) {
        if self.depth_exceeded {
            return;
        }
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
                    self.doc_comments.insert(doc_key(d.range()), doc);
                }
                decls.push(d);
            } else {
                // `parse_top_decl` only returns `None` after reporting, so
                // recovery here stays silent and the backstop below covers it.
                self.synchronize();
            }
            self.eat_newlines();
            // Neither arm guarantees progress. Recovery paths deliberately
            // leave a closing token in place (see `parse_primary`'s error
            // arm), so `parse_top_decl` can return a decl built entirely from
            // lookahead, and `synchronize` stops without consuming when it is
            // already sitting on a resync token. Either way the loop would
            // append one decl per iteration until memory ran out. `parse_block`
            // has the same backstop, but it can break out and let
            // `expect(RBrace)` report; the top level has no closing token, so
            // it reports here and skips the token to keep parsing the rest of
            // the file.
            if self.pos == before {
                let t = self.peek().clone();
                self.error(
                    format!("unexpected token '{}' at top level", t.text),
                    t.start,
                    t.end,
                );
                self.advance();
            }
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

}

#[cfg(test)]
mod tests;
