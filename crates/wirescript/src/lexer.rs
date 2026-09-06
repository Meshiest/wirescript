//! The lexer.
//!
//! Newlines are tokens (significant for statement termination in the
//! parser). Horizontal whitespace is skipped. Block comments are
//! discarded and may nest; `//` line comments produce no token but are
//! recorded in the [`SourceMap`] alongside every line's indentation.

use crate::collections::HashSet;
use std::sync::OnceLock;

use crate::ast::{SourceComment, SourceMap};
use crate::diagnostic::{Diagnostic, Pos, SourceRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Int,
    Float,
    Str,
    /// A string with `${...}` interpolation segments. The lexer captures
    /// the raw contents; the parser re-tokenises the embedded expressions.
    StrInterp,
    Ident,
    /// `:name` atom literal. `value = Str(name)` (without the leading `:`).
    Atom,
    Kw,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Dot,
    Question,
    Dollar,
    /// Asset reference literal `$AssetType/AssetName`. The `Str` value holds the
    /// path without the leading `$`.
    AssetRef,
    /// Inline nested-prefab block `` $```…``` ``. The `Str` value holds the
    /// verbatim inner source (between the fences), for a later compile stage
    /// to lex/parse as its own program.
    NestedPrefab,
    /// `@word` annotation (`@left` etc.). `text` holds the word without `@`;
    /// the parser validates it.
    Annotation,
    Arrow,    // `->`
    FatArrow, // `=>`
    Op,
    Newline,
    DocComment,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub start: Pos,
    pub end: Pos,
    pub value: Option<TokenValue>,
}

#[derive(Clone, Debug)]
pub enum TokenValue {
    Str(String),
    Interp(Vec<InterpPart>),
}

#[derive(Clone, Debug)]
pub enum InterpPart {
    Lit(String),
    /// Embedded expression — captured as raw source + its range; the
    /// parser re-lexes & re-parses this slice at parse time.
    Expr {
        source: String,
        start: Pos,
        end: Pos,
    },
}

// `enum` is deliberately absent: it is CONTEXTUAL, recognised by the parser
// only when it opens a declaration (`enum Name {`), so existing code may go on
// using `enum` as a variable, parameter, or field name.
pub const KEYWORDS: &[&str] = &[
    "var", "array", "map", "buffer", "chip", "fn", "on", "in", "out", "emit", "let", "if", "else",
    "then", "match", "return", "true", "false", "null", "ref", "open", "mod", "import", "from",
    "as", "static", "type", "await", "const",
];

fn keyword_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| KEYWORDS.iter().copied().collect())
}

const TWO_CHAR_OPS: &[&str] = &[
    "&&", "||", "^^", "==", "!=", "<=", ">=", "<<", ">>", "**", "..",
    "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
];
const THREE_CHAR_OPS: &[&str] = &["...", "<<=", ">>="];
const SINGLE_CHAR_OPS: &[char] = &[
    '&', '|', '^', '~', '+', '-', '*', '/', '%', '=', '<', '>', '!',
];

pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    /// Per-line indentation and the `//` comments dropped from `tokens`.
    pub source_map: SourceMap,
}

pub fn lex(source: &str, file: &str) -> LexResult {
    lex_at(source, file, Pos { offset: 0, line: 1, col: 1 })
}

/// Lex a FRAGMENT carved out of a larger file, reporting positions in the
/// containing file's coordinates.
///
/// The one fragment is a `${...}` interpolation body, which the parser lexes
/// and parses on its own. Seeding the lexer is what makes every span in that
/// sub-tree right by construction; shifting them afterwards needs a walker
/// over every expression shape, and a shape it misses reports at line 1.
pub fn lex_at(source: &str, file: &str, origin: Pos) -> LexResult {
    Lexer::new(source, file, origin).run()
}

/// The 0-based column of each line's first non-whitespace character;
/// blank and whitespace-only lines get 0. Columns count bytes the way
/// [`Pos::col`] advances, less its 1-based origin.
fn line_indents(source: &str) -> Vec<u32> {
    source
        .split('\n')
        .map(|line| {
            let indent = line
                .bytes()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count();
            if line[indent..].trim().is_empty() {
                0
            } else {
                indent as u32
            }
        })
        .collect()
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    file: String,
    pos: usize,
    line: u32,
    col: u32,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    comments: Vec<SourceComment>,
    /// Unclosed `[` seen so far. A comment inside one is inside an array
    /// literal or a data table; `emit` is the single funnel every token
    /// passes through, so counting there cannot miss one.
    bracket_depth: i32,
    /// Where this source sits in the containing file. `{0, 1, 1}` for a whole
    /// file; see [`lex_at`].
    origin: Pos,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str, file: &str, origin: Pos) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            file: file.to_string(),
            pos: 0,
            line: 1,
            col: 1,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            comments: Vec::new(),
            bracket_depth: 0,
            origin,
        }
    }

    fn run(mut self) -> LexResult {
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;

            // line comment
            if c == '/' && self.peek_char(1) == Some('/') {
                let start = self.snapshot();
                if self.peek_char(2) == Some('/') {
                    // Doc comment: `/// text`
                    self.advance(); self.advance(); self.advance(); // skip ///
                    if self.pos < self.bytes.len() && self.bytes[self.pos] == b' ' {
                        self.advance(); // skip optional leading space
                    }
                    let content_start = self.pos;
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.advance();
                    }
                    // `trim_end`, as the plain-comment path below already
                    // does: the scan stops at `\n`, so on a CRLF file the text
                    // would keep its `\r`, and a doc comment is baked into the
                    // world as a chip's header text.
                    let text = self.source[content_start..self.pos].trim_end().to_string();
                    let end = self.snapshot();
                    self.emit(TokenKind::DocComment, text, start, end, None);
                    continue;
                }
                // Plain line comment: no token, but the text is kept.
                let own_line = self.source[..start.offset]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .is_empty();
                self.advance();
                self.advance(); // skip //
                if self.pos < self.bytes.len() && self.bytes[self.pos] == b' ' {
                    self.advance(); // skip optional leading space
                }
                let content_start = self.pos;
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.advance();
                }
                self.comments.push(SourceComment {
                    line: start.line,
                    col: start.col,
                    text: self.source[content_start..self.pos].trim_end().to_string(),
                    own_line,
                    in_array: self.bracket_depth > 0,
                });
                continue;
            }
            // block comment (nestable)
            if c == '/' && self.peek_char(1) == Some('*') {
                self.read_block_comment();
                continue;
            }
            // horizontal whitespace
            if c == ' ' || c == '\t' || c == '\r' {
                self.advance();
                continue;
            }
            // significant newline
            if c == '\n' {
                let start = self.snapshot();
                self.advance();
                let end = self.snapshot();
                self.emit(TokenKind::Newline, "\n", start, end, None);
                continue;
            }
            if c == '"' {
                self.read_string();
                continue;
            }
            if c == '\'' {
                self.read_single_quote_string();
                continue;
            }
            if c.is_ascii_digit() {
                self.read_number();
                continue;
            }
            if is_ident_start(c) {
                self.read_ident();
                continue;
            }
            // Inline nested-prefab block `$```…```` — checked before the asset-
            // ref branch below since both start with `$`.
            if c == '$'
                && self.peek_char(1) == Some('`')
                && self.peek_char(2) == Some('`')
                && self.peek_char(3) == Some('`')
            {
                self.read_nested_prefab();
                continue;
            }
            // Asset reference `$AssetType/AssetName`, or prefab file reference
            // `$./file.brz` / `$/abs.brz` (only outside strings; the `${...}`
            // interpolation form is handled inside string reading).
            if c == '$'
                && self
                    .peek_char(1)
                    .is_some_and(|n| is_ident_start(n) || n == '.' || n == '/')
            {
                self.read_asset_ref();
                continue;
            }
            // `@word` annotation (port-side annotations `@left/@right/...`).
            if c == '@' && self.peek_char(1).is_some_and(is_ident_start) {
                let start = self.snapshot();
                self.advance(); // skip '@'
                let word_start = self.pos;
                while self.pos < self.bytes.len() && is_ident_cont(self.bytes[self.pos] as char) {
                    self.advance();
                }
                let text = self.source[word_start..self.pos].to_string();
                let end = self.snapshot();
                self.emit(TokenKind::Annotation, text, start, end, None);
                continue;
            }
            // `:name` atom — only in value position. A `:` that follows a
            // value-completing token (an annotation / record-field / map-key
            // separator) stays a plain Colon.
            if c == ':'
                && self.peek_char(1).is_some_and(is_ident_start)
                && !self.prev_tok_completes_value()
            {
                self.read_atom();
                continue;
            }
            if self.read_punct() {
                continue;
            }

            let start = self.snapshot();
            self.advance();
            let end = self.snapshot();
            self.diag(
                "WSP001",
                format!("unexpected character '{c}'"),
                start,
                end,
            );
        }

        // `abs` by hand: this is the one token that does not go through
        // `emit`, and leaving it fragment-local put every "expected an
        // expression, got nothing" inside a `${...}` slot on line 1 of the
        // containing file. Whole-file lexing has the identity origin, so
        // nothing else moves.
        let p = self.abs(self.snapshot());
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            text: String::new(),
            start: p,
            end: p,
            value: None,
        });
        let source_map = SourceMap {
            file: self.file.as_str().into(),
            line_indent: line_indents(self.source),
            comments: self.comments,
        };
        LexResult {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
            source_map,
        }
    }

    fn peek_char(&self, off: usize) -> Option<char> {
        self.bytes.get(self.pos + off).map(|&b| b as char)
    }

    /// The current position WITHIN this lexer's source. Callers slice
    /// `self.source` with these offsets, so they stay fragment-local; [`abs`]
    /// converts one for the outside world.
    fn snapshot(&self) -> Pos {
        Pos {
            offset: self.pos,
            line: self.line,
            col: self.col,
        }
    }

    /// A local position in the containing file's coordinates. Applied at the
    /// three places a position leaves this lexer, a token, a diagnostic, and
    /// an interpolation slot's own origin, and nowhere else, since everything
    /// in between indexes `self.source`.
    ///
    /// A fragment's first line CONTINUES the origin's line, so its columns
    /// continue too; every later line starts at column 1 like any other. With
    /// the whole-file origin `{0, 1, 1}` this is the identity.
    fn abs(&self, p: Pos) -> Pos {
        let line = p.line - 1 + self.origin.line;
        Pos {
            offset: p.offset + self.origin.offset,
            line,
            col: if line == self.origin.line {
                p.col - 1 + self.origin.col
            } else {
                p.col
            },
        }
    }

    fn advance(&mut self) {
        if self.pos < self.bytes.len() {
            if self.bytes[self.pos] == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    fn emit(
        &mut self,
        kind: TokenKind,
        text: impl Into<String>,
        start: Pos,
        end: Pos,
        value: Option<TokenValue>,
    ) {
        match kind {
            TokenKind::LBracket => self.bracket_depth += 1,
            TokenKind::RBracket => self.bracket_depth = (self.bracket_depth - 1).max(0),
            _ => {}
        }
        self.tokens.push(Token {
            kind,
            text: text.into(),
            start: self.abs(start),
            end: self.abs(end),
            value,
        });
    }

    fn diag(&mut self, code: &str, message: impl Into<String>, start: Pos, end: Pos) {
        let range = SourceRange::new(self.file.clone(), self.abs(start), self.abs(end));
        self.diagnostics.push(Diagnostic::error(code, message.into(), range));
    }

    /// Read an asset reference into a [`TokenKind::AssetRef`] token. Two forms
    /// share the token; the parser distinguishes them by the leading char:
    /// - `$AssetType/AssetName` — an embedded external asset (a single `/`
    ///   separates type from name).
    /// - `$./rel/path.brz` or `$/abs/path.brz` — a prefab file reference (path
    ///   begins with `.` or `/`). `.` and `-` are allowed so file names and
    ///   relative segments lex.
    fn read_asset_ref(&mut self) {
        let start = self.snapshot();
        self.advance(); // '$'
        let path_start = self.pos;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;
            if c.is_ascii_alphanumeric() || c == '_' || c == '/' || c == '.' || c == '-' {
                self.advance();
            } else {
                break;
            }
        }
        let path = self.source[path_start..self.pos].to_string();
        let end = self.snapshot();
        let text = self.source[start.offset..end.offset].to_string();
        self.emit(TokenKind::AssetRef, text, start, end, Some(TokenValue::Str(path)));
    }

    /// Read an inline nested-prefab block `` $```…``` `` into a
    /// [`TokenKind::NestedPrefab`] token. The inner text (between the
    /// fences) is captured verbatim for a later compile stage to lex/parse
    /// as its own program. The scan is string- and line-comment-aware so a
    /// backtick inside a string or `//` comment can't miscount, and a
    /// nested `` $``` `` block is tracked via depth so the outer fence owns
    /// the whole span.
    fn read_nested_prefab(&mut self) {
        let start = self.snapshot();
        self.advance(); // '$'
        self.advance(); // '`'
        self.advance(); // '`'
        self.advance(); // '`'
        let content_start = self.pos;
        let mut depth: i32 = 1;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;
            // String literal — skip verbatim (honoring `\` escapes) so a
            // backtick inside it can't be mistaken for a fence.
            if c == '"' || c == '\'' {
                let quote = c;
                self.advance();
                while self.pos < self.bytes.len() && self.bytes[self.pos] as char != quote {
                    if self.bytes[self.pos] == b'\\' {
                        self.advance();
                        if self.pos >= self.bytes.len() {
                            break;
                        }
                    }
                    self.advance();
                }
                if self.pos < self.bytes.len() {
                    self.advance(); // closing quote
                }
                continue;
            }
            // `//` line comment — may itself contain backticks.
            if c == '/' && self.peek_char(1) == Some('/') {
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.advance();
                }
                continue;
            }
            // Nested `$``` open.
            if c == '$'
                && self.peek_char(1) == Some('`')
                && self.peek_char(2) == Some('`')
                && self.peek_char(3) == Some('`')
            {
                depth += 1;
                self.advance();
                self.advance();
                self.advance();
                self.advance();
                continue;
            }
            // A closing fence.
            if c == '`' && self.peek_char(1) == Some('`') && self.peek_char(2) == Some('`') {
                depth -= 1;
                if depth == 0 {
                    let close_start = self.pos;
                    self.advance();
                    self.advance();
                    self.advance();
                    let inner = self.source[content_start..close_start].to_string();
                    let end = self.snapshot();
                    let text = self.source[start.offset..end.offset].to_string();
                    self.emit(
                        TokenKind::NestedPrefab,
                        text,
                        start,
                        end,
                        Some(TokenValue::Str(inner)),
                    );
                    return;
                }
                self.advance();
                self.advance();
                self.advance();
                continue;
            }
            self.advance();
        }
        self.diag(
            "WSP001",
            "unterminated $``` nested-prefab block",
            start,
            self.snapshot(),
        );
    }

    /// True when the previously emitted token completes a value, so a following
    /// `:` is an annotation / field / map-key separator rather than an atom.
    fn prev_tok_completes_value(&self) -> bool {
        match self.tokens.last() {
            Some(t) => matches!(
                t.kind,
                TokenKind::Ident
                    | TokenKind::Str
                    | TokenKind::StrInterp
                    | TokenKind::Int
                    | TokenKind::Float
                    | TokenKind::Atom
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
            ) || (t.kind == TokenKind::Kw && matches!(t.text.as_str(), "true" | "false")),
            None => false,
        }
    }

    fn read_atom(&mut self) {
        let start = self.snapshot();
        self.advance(); // ':'
        let name_start = self.pos;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.advance();
            } else {
                break;
            }
        }
        let name = self.source[name_start..self.pos].to_string();
        let end = self.snapshot();
        let text = self.source[start.offset..end.offset].to_string();
        self.emit(TokenKind::Atom, text, start, end, Some(TokenValue::Str(name)));
    }

    fn read_block_comment(&mut self) {
        let start = self.snapshot();
        self.advance(); // '/'
        self.advance(); // '*'
        let mut depth: i32 = 1;
        while self.pos < self.bytes.len() && depth > 0 {
            let c = self.bytes[self.pos] as char;
            let n = self.peek_char(1);
            if c == '/' && n == Some('*') {
                depth += 1;
                self.advance();
                self.advance();
            } else if c == '*' && n == Some('/') {
                depth -= 1;
                self.advance();
                self.advance();
            } else {
                self.advance();
            }
        }
        if depth > 0 {
            self.diag("WSP001", "unterminated block comment", start, self.snapshot());
        }
    }

    fn read_string(&mut self) {
        let start = self.snapshot();
        self.advance(); // opening "
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut literal = String::new();
        let mut has_interp = false;

        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;
            if c == '"' {
                self.advance();
                let end = self.snapshot();
                let text = self.source[start.offset..end.offset].to_string();
                if has_interp {
                    if !literal.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                    }
                    self.emit(
                        TokenKind::StrInterp,
                        text,
                        start,
                        end,
                        Some(TokenValue::Interp(parts)),
                    );
                } else {
                    self.emit(
                        TokenKind::Str,
                        text,
                        start,
                        end,
                        Some(TokenValue::Str(literal)),
                    );
                }
                return;
            }
            if c == '\\' {
                self.advance();
                if self.pos >= self.bytes.len() {
                    break;
                }
                let esc = self.bytes[self.pos] as char;
                self.push_escape(&mut literal, esc, '"');
                continue;
            }
            if c == '$' && self.peek_char(1) == Some('{') {
                has_interp = true;
                if !literal.is_empty() {
                    parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                }
                self.advance(); // $
                self.advance(); // {
                let expr_start = self.snapshot();
                let expr_start_offset = self.pos;
                let mut depth: i32 = 1;
                while self.pos < self.bytes.len() && depth > 0 {
                    let ch = self.bytes[self.pos] as char;
                    if ch == '{' {
                        depth += 1;
                    } else if ch == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if ch == '"' {
                        // skip nested string contents (with escapes)
                        self.advance();
                        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
                            if self.bytes[self.pos] == b'\\' {
                                self.advance();
                            }
                            self.advance();
                        }
                    }
                    self.advance();
                }
                let expr_end = self.snapshot();
                if depth != 0 {
                    self.diag("WSP001", "unterminated string interpolation", start, expr_end);
                    return;
                }
                parts.push(InterpPart::Expr {
                    source: self.source[expr_start_offset..self.pos].to_string(),
                    start: self.abs(expr_start),
                    end: self.abs(expr_end),
                });
                self.advance(); // consume closing '}'
                continue;
            }
            if c == '\n' {
                self.diag("WSP001", "unterminated string", start, self.snapshot());
                return;
            }
            // Literal content char. `c` is only the first byte cast to char, so
            // read the real UTF-8 char from the source — otherwise a multi-byte
            // char (e.g. `█` = E2 96 88) would be split into three Latin-1 chars
            // and re-encoded as garbage on emit. Structural chars above are all
            // ASCII, so only this branch can see a multi-byte char.
            let real = self.source[self.pos..].chars().next().unwrap_or(c);
            literal.push(real);
            for _ in 0..real.len_utf8() {
                self.advance();
            }
        }
        self.diag(
            "WSP001",
            "unterminated string at end of file",
            start,
            self.snapshot(),
        );
    }

    /// Append the escape `\<esc>` to `literal`, and consume it.
    ///
    /// One table for both quote styles, so they cannot drift: `quote` is the
    /// delimiter of the literal being read, which escapes to itself, and
    /// everything else is the table published in `docs/src/syntax.md`.
    ///
    /// An unrecognized escape is reported and its text kept verbatim, so the
    /// recovered string still contains what was written.
    fn push_escape(&mut self, literal: &mut String, esc: char, quote: char) {
        let mapped = match esc {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            '\\' => Some('\\'),
            '$' => Some('$'),
            '0' => Some('\0'),
            c if c == quote => Some(quote),
            _ => None,
        };
        match mapped {
            Some(ch) => literal.push(ch),
            None => {
                let p = self.snapshot();
                self.diag("WSP001", format!("unknown string escape '\\{esc}'"), p, p);
                literal.push('\\');
                literal.push(esc);
            }
        }
        self.advance();
    }

    fn read_single_quote_string(&mut self) {
        let start = self.snapshot();
        self.advance(); // opening '
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut literal = String::new();
        let mut has_interp = false;

        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;
            if c == '\'' {
                self.advance();
                let end = self.snapshot();
                let text = self.source[start.offset..end.offset].to_string();
                if has_interp {
                    if !literal.is_empty() {
                        parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                    }
                    self.emit(TokenKind::StrInterp, text, start, end, Some(TokenValue::Interp(parts)));
                } else {
                    self.emit(TokenKind::Str, text, start, end, Some(TokenValue::Str(literal)));
                }
                return;
            }
            if c == '\\' {
                self.advance();
                if self.pos >= self.bytes.len() {
                    break;
                }
                let esc = self.bytes[self.pos] as char;
                self.push_escape(&mut literal, esc, '\'');
                continue;
            }
            if c == '$' && self.peek_char(1) == Some('{') {
                has_interp = true;
                if !literal.is_empty() {
                    parts.push(InterpPart::Lit(std::mem::take(&mut literal)));
                }
                self.advance(); // $
                self.advance(); // {
                let expr_start = self.snapshot();
                let expr_start_offset = self.pos;
                let mut depth: i32 = 1;
                while self.pos < self.bytes.len() && depth > 0 {
                    let ch = self.bytes[self.pos] as char;
                    if ch == '{' {
                        depth += 1;
                    } else if ch == '}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if ch == '\'' || ch == '"' {
                        self.advance();
                        let quote = ch;
                        while self.pos < self.bytes.len() && self.bytes[self.pos] as char != quote {
                            if self.bytes[self.pos] == b'\\' {
                                self.advance();
                            }
                            self.advance();
                        }
                    }
                    self.advance();
                }
                let expr_end = self.snapshot();
                if depth != 0 {
                    self.diag("WSP001", "unterminated string interpolation", start, expr_end);
                    return;
                }
                parts.push(InterpPart::Expr {
                    source: self.source[expr_start_offset..self.pos].to_string(),
                    start: self.abs(expr_start),
                    end: self.abs(expr_end),
                });
                self.advance(); // closing '}'
                continue;
            }
            if c == '\n' {
                self.diag("WSP001", "unterminated string", start, self.snapshot());
                return;
            }
            // Literal content char. `c` is only the first byte cast to char, so
            // read the real UTF-8 char from the source — otherwise a multi-byte
            // char (e.g. `█` = E2 96 88) would be split into three Latin-1 chars
            // and re-encoded as garbage on emit. Structural chars above are all
            // ASCII, so only this branch can see a multi-byte char.
            let real = self.source[self.pos..].chars().next().unwrap_or(c);
            literal.push(real);
            for _ in 0..real.len_utf8() {
                self.advance();
            }
        }
        self.diag("WSP001", "unterminated string at end of file", start, self.snapshot());
    }

    fn read_number(&mut self) {
        let start = self.snapshot();
        let mut text = String::new();
        let mut is_float = false;
        let mut is_hex = false;
        let mut is_bin = false;
        let mut is_oct = false;

        if self.bytes[self.pos] == b'0'
            && matches!(self.peek_char(1), Some('x') | Some('X'))
        {
            text.push(self.bytes[self.pos] as char);
            self.advance();
            text.push(self.bytes[self.pos] as char);
            self.advance();
            is_hex = true;
            while self.pos < self.bytes.len() {
                let c = self.bytes[self.pos] as char;
                if c.is_ascii_hexdigit() || c == '_' {
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
        } else if self.bytes[self.pos] == b'0'
            && matches!(self.peek_char(1), Some('b') | Some('B'))
        {
            text.push(self.bytes[self.pos] as char);
            self.advance();
            text.push(self.bytes[self.pos] as char);
            self.advance();
            is_bin = true;
            while self.pos < self.bytes.len() {
                let c = self.bytes[self.pos] as char;
                if c == '0' || c == '1' || c == '_' {
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
        } else if self.bytes[self.pos] == b'0'
            && matches!(self.peek_char(1), Some('o') | Some('O'))
        {
            text.push(self.bytes[self.pos] as char);
            self.advance();
            text.push(self.bytes[self.pos] as char);
            self.advance();
            is_oct = true;
            while self.pos < self.bytes.len() {
                let c = self.bytes[self.pos] as char;
                if ('0'..='7').contains(&c) || c == '_' {
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
        } else {
            while self.pos < self.bytes.len() {
                let c = self.bytes[self.pos] as char;
                if c.is_ascii_digit() || c == '_' {
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
            // fractional part
            if self.bytes.get(self.pos).copied() == Some(b'.')
                && self.peek_char(1).map(|c| c.is_ascii_digit()).unwrap_or(false)
            {
                is_float = true;
                text.push('.');
                self.advance();
                while self.pos < self.bytes.len() {
                    let c = self.bytes[self.pos] as char;
                    if c.is_ascii_digit() || c == '_' {
                        text.push(c);
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            // exponent
            if matches!(self.bytes.get(self.pos).copied(), Some(b'e') | Some(b'E')) {
                is_float = true;
                text.push(self.bytes[self.pos] as char);
                self.advance();
                if matches!(self.bytes.get(self.pos).copied(), Some(b'+') | Some(b'-')) {
                    text.push(self.bytes[self.pos] as char);
                    self.advance();
                }
                // `_` is a digit separator in the exponent too. Without this
                // `2.5e1_0` lexed as `2.5e1` followed by the identifier `_0`,
                // silently changing the value by nine orders of magnitude.
                while self.pos < self.bytes.len() {
                    let c = self.bytes[self.pos] as char;
                    if c.is_ascii_digit() || c == '_' {
                        text.push(c);
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }

        let end = self.snapshot();
        let kind = if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        };
        // Note: value validation (parse to f64/i64/u64) is deferred to the parser,
        // which knows the literal's sign context and base.
        let _ = (is_hex, is_bin, is_oct);
        self.emit(kind, text, start, end, None);
    }

    fn read_ident(&mut self) {
        let start = self.snapshot();
        let mut text = String::new();
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos] as char;
            if is_ident_cont(c) {
                text.push(c);
                self.advance();
            } else {
                break;
            }
        }
        let end = self.snapshot();
        let kind = if keyword_set().contains(text.as_str()) {
            TokenKind::Kw
        } else {
            TokenKind::Ident
        };
        self.emit(kind, text, start, end, None);
    }

    fn read_punct(&mut self) -> bool {
        let start = self.snapshot();
        // `str::get` returns None when the range end isn't a char boundary, so
        // a stray multi-byte char ahead yields "" (no punct match) instead of
        // panicking on a mid-codepoint byte slice.
        let slice3 = self
            .source
            .get(self.pos..(self.pos + 3).min(self.source.len()))
            .unwrap_or("");
        let slice2 = self
            .source
            .get(self.pos..(self.pos + 2).min(self.source.len()))
            .unwrap_or("");
        let c = self.bytes[self.pos] as char;

        // `->` and `=>` first — they'd otherwise be caught by the two-char-op list as invalid.
        if slice2 == "->" {
            self.advance();
            self.advance();
            self.emit(TokenKind::Arrow, "->", start, self.snapshot(), None);
            return true;
        }
        if slice2 == "=>" {
            self.advance();
            self.advance();
            self.emit(TokenKind::FatArrow, "=>", start, self.snapshot(), None);
            return true;
        }
        if THREE_CHAR_OPS.contains(&slice3) {
            let s3 = slice3.to_string();
            self.advance();
            self.advance();
            self.advance();
            self.emit(TokenKind::Op, s3, start, self.snapshot(), None);
            return true;
        }
        if TWO_CHAR_OPS.contains(&slice2) {
            let s2 = slice2.to_string();
            self.advance();
            self.advance();
            self.emit(TokenKind::Op, s2, start, self.snapshot(), None);
            return true;
        }
        let punct = match c {
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '{' => Some(TokenKind::LBrace),
            '}' => Some(TokenKind::RBrace),
            '[' => Some(TokenKind::LBracket),
            ']' => Some(TokenKind::RBracket),
            ',' => Some(TokenKind::Comma),
            ';' => Some(TokenKind::Semi),
            ':' => Some(TokenKind::Colon),
            '.' => Some(TokenKind::Dot),
            '?' => Some(TokenKind::Question),
            '$' => Some(TokenKind::Dollar),
            _ => None,
        };
        if let Some(k) = punct {
            self.advance();
            self.emit(k, c.to_string(), start, self.snapshot(), None);
            return true;
        }
        if SINGLE_CHAR_OPS.contains(&c) {
            self.advance();
            self.emit(TokenKind::Op, c.to_string(), start, self.snapshot(), None);
            return true;
        }
        false
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_ident_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests;
