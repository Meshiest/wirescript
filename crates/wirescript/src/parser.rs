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

// ---------- parser state ----------

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

}

#[cfg(test)]
mod tests;
