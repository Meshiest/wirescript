//! Source positions + diagnostic reporting.
//!
//! table itself (WSP001, WS001, WS002, ...) is elsewhere; this module
//! just holds the shared value types.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pos {
    pub offset: usize,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SourceRange {
    pub file: Arc<str>,
    pub start: Pos,
    pub end: Pos,
}

impl SourceRange {
    pub fn new(file: impl Into<Arc<str>>, start: Pos, end: Pos) -> Self {
        Self {
            file: file.into(),
            start,
            end,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub range: SourceRange,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>, range: SourceRange) -> Self {
        Self {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            range,
        }
    }
    pub fn warning(code: impl Into<String>, message: impl Into<String>, range: SourceRange) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.into(),
            message: message.into(),
            range,
        }
    }
}

/// `ws-ignore` directives collected from a program's `//` comments.
///
/// A comment token of the form `ws-ignore-line:WS014` drops that code's
/// diagnostics from the comment's own line; written on its own line it covers
/// the line below as well, so a long statement can carry the note above it
/// instead of past its right edge. `ws-ignore-file:WS014` drops the code from
/// the whole file no matter where it sits, by convention at the top.
///
/// Both forms take a comma-separated list (`:WS014,WSP001`), suppress
/// everything on their line or in their file when written bare
/// (`// ws-ignore-file`), and may be surrounded by prose saying why:
/// the directive is recognised as any whitespace-delimited token of the
/// comment, not only the first, so appending one to a line that already ends
/// in a comment works.
///
/// **Errors are never suppressed.** A directive naming one is inert: the
/// program still does not compile, and hiding the reason would only move the
/// failure somewhere less legible.
#[derive(Clone, Debug, Default)]
pub struct Suppressions {
    per_file: crate::collections::HashMap<String, FileSuppressions>,
}

#[derive(Clone, Debug, Default)]
struct FileSuppressions {
    /// `// ws-ignore-file` with no codes: everything in this file.
    file_all: bool,
    file_codes: crate::collections::HashSet<String>,
    /// Lines carrying a bare `// ws-ignore-line`.
    line_all: crate::collections::HashSet<u32>,
    line_codes: crate::collections::HashMap<u32, crate::collections::HashSet<String>>,
}

/// What one directive covers.
enum IgnoreScope {
    Line,
    File,
}

/// Read one whitespace-delimited comment token as a directive. `None` codes
/// mean the bare form (everything); a `:` with nothing usable after it is a
/// typo rather than a request to ignore everything, so it parses as no
/// directive at all.
fn parse_ignore_token(token: &str) -> Option<(IgnoreScope, Option<Vec<String>>)> {
    let (keyword, codes) = match token.split_once(':') {
        Some((k, c)) => (k, Some(c)),
        None => (token, None),
    };
    let scope = match keyword {
        "ws-ignore-line" => IgnoreScope::Line,
        "ws-ignore-file" => IgnoreScope::File,
        _ => return None,
    };
    match codes {
        None => Some((scope, None)),
        Some(list) => {
            let codes: Vec<String> = list
                .split(',')
                .map(|c| c.trim().to_ascii_uppercase())
                .filter(|c| !c.is_empty())
                .collect();
            if codes.is_empty() {
                None
            } else {
                Some((scope, Some(codes)))
            }
        }
    }
}

impl Suppressions {
    /// Record one `//` comment. `line` is 1-based like [`Pos`], `text` is the
    /// comment body with its `//` already stripped, and `own_line` says whether
    /// the comment is the only thing on its line.
    pub fn add_comment(&mut self, file: &str, line: u32, own_line: bool, text: &str) {
        for token in text.split_whitespace() {
            let Some((scope, codes)) = parse_ignore_token(token) else {
                continue;
            };
            let entry = self.per_file.entry(file.to_string()).or_default();
            match scope {
                IgnoreScope::File => match codes {
                    Some(codes) => entry.file_codes.extend(codes),
                    None => entry.file_all = true,
                },
                IgnoreScope::Line => {
                    // A trailing directive covers the line it sits on. On its
                    // own line it covers the next one too, since that line is
                    // what an author writing the comment above a statement means.
                    let covered = if own_line {
                        vec![line, line + 1]
                    } else {
                        vec![line]
                    };
                    for l in covered {
                        match &codes {
                            Some(codes) => {
                                entry.line_codes.entry(l).or_default().extend(codes.iter().cloned())
                            }
                            None => {
                                entry.line_all.insert(l);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.per_file.is_empty()
    }

    /// True when a directive covers this diagnostic. Errors never are.
    pub fn suppresses(&self, d: &Diagnostic) -> bool {
        if matches!(d.severity, Severity::Error) {
            return false;
        }
        let Some(f) = self.per_file.get(&*d.range.file) else {
            return false;
        };
        let code = d.code.to_ascii_uppercase();
        if f.file_all || f.file_codes.contains(&code) {
            return true;
        }
        let line = d.range.start.line;
        f.line_all.contains(&line)
            || f.line_codes
                .get(&line)
                .is_some_and(|codes| codes.contains(&code))
    }

    /// Drop every suppressed diagnostic. Idempotent, so a pipeline that filters
    /// at more than one stage stays correct.
    pub fn apply(&self, diagnostics: &mut Vec<Diagnostic>) {
        if self.is_empty() {
            return;
        }
        diagnostics.retain(|d| !self.suppresses(d));
    }
}
