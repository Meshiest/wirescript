//! Byte scanner for `.wsir`. One pass, no backtracking; identifiers and
//! strings without escapes borrow from the source.

use std::borrow::Cow;

use super::defaults::{is_ident_char, is_ident_start};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Tok<'a> {
    Ident(&'a str),
    /// `%name`, without the `%`.
    Value(&'a str),
    /// `@name` or `@"name"`, without the `@`.
    ModName(Cow<'a, str>),
    /// `!name`, without the `!`.
    MetaKw(&'a str),
    /// `!N`.
    MetaNum(u32),
    Str(Cow<'a, str>),
    Int(i64),
    Float(f64),
    Color([u8; 4]),
    Punct(u8),
    Arrow,
    FatArrow,
    Eof,
}

impl Tok<'_> {
    /// How the token reads in an error message.
    pub(super) fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Value(s) => format!("`%{s}`"),
            Tok::ModName(s) => format!("`@{s}`"),
            Tok::MetaKw(s) => format!("`!{s}`"),
            Tok::MetaNum(n) => format!("`!{n}`"),
            Tok::Str(s) => format!("string {s:?}"),
            Tok::Int(n) => format!("`{n}`"),
            Tok::Float(f) => format!("`{f:?}`"),
            Tok::Color(c) => format!("`#{:02x}{:02x}{:02x}{:02x}`", c[0], c[1], c[2], c[3]),
            Tok::Punct(c) => format!("`{}`", *c as char),
            Tok::Arrow => "`->`".into(),
            Tok::FatArrow => "`=>`".into(),
            Tok::Eof => "end of file".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Token<'a> {
    pub tok: Tok<'a>,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug)]
pub(super) struct LexError {
    pub line: u32,
    pub col: u32,
    pub found: String,
    pub message: String,
}

pub(super) fn lex(src: &str) -> Result<Vec<Token<'_>>, LexError> {
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        i: 0,
        line: 1,
        line_start: 0,
    };
    // A rough guess from typical density keeps reallocation to a couple of times.
    let mut out = Vec::with_capacity(src.len() / 6 + 16);
    loop {
        let t = lx.next()?;
        let eof = t.tok == Tok::Eof;
        out.push(t);
        if eof {
            return Ok(out);
        }
    }
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    line: u32,
    line_start: usize,
}

impl<'a> Lexer<'a> {
    /// 1-based byte column. Bytes, not characters: counting characters costs
    /// a scan from the line start per token, and metadata lines can be long.
    fn col(&self, at: usize) -> u32 {
        at.saturating_sub(self.line_start) as u32 + 1
    }

    fn err(&self, at: usize, found: impl Into<String>, message: impl Into<String>) -> LexError {
        LexError {
            line: self.line,
            col: self.col(at),
            found: found.into(),
            message: message.into(),
        }
    }

    fn skip_trivia(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'\n' => {
                    self.i += 1;
                    self.line += 1;
                    self.line_start = self.i;
                }
                b' ' | b'\t' | b'\r' => self.i += 1,
                b';' => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn ident_end(&self, mut j: usize) -> usize {
        while j < self.b.len() && is_ident_char(self.b[j]) {
            j += 1;
        }
        j
    }

    fn next(&mut self) -> Result<Token<'a>, LexError> {
        self.skip_trivia();
        let start = self.i;
        let line = self.line;
        let col = self.col(start);
        let mk = |tok| Ok(Token { tok, line, col });
        if self.i >= self.b.len() {
            return mk(Tok::Eof);
        }
        let c = self.b[self.i];
        match c {
            b'%' => {
                let end = self.ident_end(self.i + 1);
                if end == self.i + 1 {
                    return Err(self.err(start, "%", "expected a value name after `%`"));
                }
                self.i = end;
                mk(Tok::Value(&self.src[start + 1..end]))
            }
            b'@' => {
                if self.b.get(self.i + 1) == Some(&b'"') {
                    self.i += 1;
                    let s = self.string()?;
                    mk(Tok::ModName(s))
                } else {
                    let end = self.ident_end(self.i + 1);
                    if end == self.i + 1 {
                        return Err(self.err(start, "@", "expected a module name after `@`"));
                    }
                    self.i = end;
                    mk(Tok::ModName(Cow::Borrowed(&self.src[start + 1..end])))
                }
            }
            b'!' => {
                let j = self.i + 1;
                if self.b.get(j).is_some_and(u8::is_ascii_digit) {
                    let mut end = j;
                    while end < self.b.len() && self.b[end].is_ascii_digit() {
                        end += 1;
                    }
                    let n = self.src[j..end].parse::<u32>().map_err(|_| {
                        self.err(start, &self.src[start..end], "metadata number out of range")
                    })?;
                    self.i = end;
                    mk(Tok::MetaNum(n))
                } else {
                    let end = self.ident_end(j);
                    if end == j {
                        return Err(self.err(
                            start,
                            "!",
                            "expected a metadata name or number after `!`",
                        ));
                    }
                    self.i = end;
                    mk(Tok::MetaKw(&self.src[j..end]))
                }
            }
            b'"' => {
                let s = self.string()?;
                mk(Tok::Str(s))
            }
            b'#' => {
                let j = self.i + 1;
                let hex = self
                    .b
                    .get(j..j + 8)
                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit));
                let Some(hex) = hex else {
                    return Err(self.err(start, "#", "expected a color `#rrggbbaa`"));
                };
                let byte = |k: usize| {
                    u8::from_str_radix(std::str::from_utf8(&hex[k..k + 2]).unwrap(), 16).unwrap()
                };
                self.i = j + 8;
                mk(Tok::Color([byte(0), byte(2), byte(4), byte(6)]))
            }
            b'-' if self.b.get(self.i + 1) == Some(&b'>') => {
                self.i += 2;
                mk(Tok::Arrow)
            }
            b'=' if self.b.get(self.i + 1) == Some(&b'>') => {
                self.i += 2;
                mk(Tok::FatArrow)
            }
            b'-' | b'0'..=b'9' => self.number(start, line, col),
            _ if is_ident_start(c) => {
                let end = self.ident_end(self.i);
                self.i = end;
                mk(Tok::Ident(&self.src[start..end]))
            }
            b'=' | b'(' | b')' | b'{' | b'}' | b'[' | b']' | b',' | b':' | b'.' | b'*' | b'<'
            | b'>' | b'+' => {
                self.i += 1;
                mk(Tok::Punct(c))
            }
            _ => {
                let ch = self.src[start..].chars().next().unwrap_or('?');
                Err(self.err(start, ch.to_string(), "unexpected character"))
            }
        }
    }

    fn number(&mut self, start: usize, line: u32, col: u32) -> Result<Token<'a>, LexError> {
        let mut j = self.i;
        if self.b[j] == b'-' {
            j += 1;
            for (word, value) in [("inf", f64::NEG_INFINITY), ("nan", -f64::NAN)] {
                if self.src[j..].starts_with(word)
                    && !self.b.get(j + 3).is_some_and(|c| is_ident_char(*c))
                {
                    self.i = j + 3;
                    return Ok(Token {
                        tok: Tok::Float(value),
                        line,
                        col,
                    });
                }
            }
            if !self.b.get(j).is_some_and(u8::is_ascii_digit) {
                return Err(self.err(start, "-", "expected a number or `->`"));
            }
        }
        let mut float = false;
        while j < self.b.len() {
            match self.b[j] {
                b'0'..=b'9' => j += 1,
                b'.' if self.b.get(j + 1).is_some_and(u8::is_ascii_digit) => {
                    float = true;
                    j += 1;
                }
                b'e' | b'E' => {
                    float = true;
                    j += 1;
                    if matches!(self.b.get(j), Some(b'+' | b'-')) {
                        j += 1;
                    }
                }
                _ => break,
            }
        }
        let text = &self.src[start..j];
        self.i = j;
        let tok = if float {
            Tok::Float(
                text.parse()
                    .map_err(|_| self.err(start, text, "malformed float"))?,
            )
        } else {
            Tok::Int(
                text.parse()
                    .map_err(|_| self.err(start, text, "integer out of range for a 64-bit int"))?,
            )
        };
        Ok(Token { tok, line, col })
    }

    /// A string starting at the `"` under the cursor.
    fn string(&mut self) -> Result<Cow<'a, str>, LexError> {
        let open = self.i;
        let body = self.i + 1;
        let mut j = body;
        // Fast path: no escapes, borrow the slice.
        while j < self.b.len() && self.b[j] != b'"' && self.b[j] != b'\\' {
            if self.b[j] == b'\n' {
                self.line += 1;
                self.line_start = j + 1;
            }
            j += 1;
        }
        if j >= self.b.len() {
            return Err(self.err(open, "\"", "unterminated string"));
        }
        if self.b[j] == b'"' {
            self.i = j + 1;
            return Ok(Cow::Borrowed(&self.src[body..j]));
        }
        let mut s = String::from(&self.src[body..j]);
        while j < self.b.len() {
            match self.b[j] {
                b'"' => {
                    self.i = j + 1;
                    return Ok(Cow::Owned(s));
                }
                b'\\' => {
                    let esc = self.b.get(j + 1).copied();
                    match esc {
                        Some(b'"') => s.push('"'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'n') => s.push('\n'),
                        Some(b'r') => s.push('\r'),
                        Some(b't') => s.push('\t'),
                        Some(b'u') => {
                            let rest = &self.src[j + 2..];
                            let close = rest.find('}').filter(|_| rest.starts_with('{'));
                            let ch = close
                                .and_then(|c| u32::from_str_radix(&rest[1..c], 16).ok())
                                .and_then(char::from_u32);
                            match (ch, close) {
                                (Some(ch), Some(c)) => {
                                    s.push(ch);
                                    j += 2 + c + 1;
                                    continue;
                                }
                                _ => {
                                    return Err(self.err(
                                        j,
                                        "\\u",
                                        "expected `\\u{HEX}` with a valid code point",
                                    ));
                                }
                            }
                        }
                        _ => {
                            let found = esc
                                .map(|c| format!("\\{}", c as char))
                                .unwrap_or_else(|| "\\".into());
                            return Err(self.err(
                                j,
                                found,
                                "unknown escape; use \\\" \\\\ \\n \\r \\t or \\u{HEX}",
                            ));
                        }
                    }
                    j += 2;
                }
                _ => {
                    let ch = self.src[j..].chars().next().unwrap();
                    if ch == '\n' {
                        self.line += 1;
                        self.line_start = j + 1;
                    }
                    s.push(ch);
                    j += ch.len_utf8();
                }
            }
        }
        Err(self.err(open, "\"", "unterminated string"))
    }
}
