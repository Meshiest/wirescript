//! Source positions and the diagnostic value type.
//!
//! The diagnostic code table and the policy for which diagnostics a compile
//! reports live with the front end; this module holds only the values the
//! backend needs to point at source and report an emit-time finding.

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

/// A source file's path as a person reads it: an imported file's range
/// carries its canonical path, which on Windows starts with the verbatim
/// `\\?\` prefix (`\\?\UNC\` for a share).
pub fn display_path(file: &str) -> std::borrow::Cow<'_, str> {
    if let Some(share) = file.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}").into()
    } else {
        file.strip_prefix(r"\\?\").unwrap_or(file).into()
    }
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

/// `[CODE] message (file:line:col)`.
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} ({})", self.code, self.message, self.location())
    }
}

impl Diagnostic {
    /// Where the diagnostic points: `file:line:col`, the file as
    /// [`display_path`] shows it.
    pub fn location(&self) -> String {
        self.location_or("")
    }

    /// [`Diagnostic::location`], naming `file` when the range names none.
    pub fn location_or(&self, file: &str) -> String {
        let named = if self.range.file.is_empty() {
            file.into()
        } else {
            display_path(&self.range.file)
        };
        format!("{named}:{}:{}", self.range.start.line, self.range.start.col)
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>, range: SourceRange) -> Self {
        Self {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            range,
        }
    }
    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        range: SourceRange,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.into(),
            message: message.into(),
            range,
        }
    }
}
