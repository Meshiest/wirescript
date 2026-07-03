//! Source positions + diagnostic reporting.
//!
//! table itself (WSP001, WS001, WS002, ...) is elsewhere; this module
//! just holds the shared value types.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub offset: usize,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
