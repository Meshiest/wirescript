//! Diagnostic reporting: the shared messages and the policy for which
//! diagnostics a compile reports. The value types (`Pos`, `SourceRange`,
//! `Diagnostic`, `Severity`) belong to the backend crate and are re-exported
//! here.

pub use wirescript_ir::diagnostic::*;

/// The code of an internal compiler error.
pub const INTERNAL_ERROR: &str = "WS094";

/// What the wiring invariant (`lower::check_no_exec_into_value`) says of an
/// exec pin wired into a value input.
pub const EXEC_INTO_VALUE: &str = "an exec pin carries no value";

/// Whether `diags` holds an error other than an internal compiler error.
pub fn has_program_error<'d>(diags: impl IntoIterator<Item = &'d Diagnostic>) -> bool {
    diags
        .into_iter()
        .any(|d| d.severity == Severity::Error && d.code != INTERNAL_ERROR)
}

/// What lowering says of a value it can't split because it is `any`.
pub const ANY_HAS_NO_FIELDS: &str = "is `any`, one wire with no fields";

/// What type checking says of a record of the program's own given where one
/// value goes (`WS003`).
pub const RECORD_INTO_ONE_VALUE: &str = "a record is several values";

/// How the `WS003` above names a bound to narrow: `<T: members>` follows.
pub const NARROW_THE_BOUND: &str = "narrow the bound to the members this body works with: ";

/// Drop every internal compiler error from a compile's final diagnostics when
/// it reports another error: that error already rejects the program, which
/// may be why something lowered to nothing. Also drop a repeat of an error
/// that both type checking and lowering report ([`REPORTED_TWICE`]) at the
/// same place, an `any` value lowering can't split beside a `WS098`, whose
/// container holds `any`, and lowering's `WS071` for a record where type
/// checking already reported it, or for a part of it ([`RECORD_INTO_ONE_VALUE`]).
pub fn drop_internal_errors_beside_errors(diags: &mut Vec<Diagnostic>) {
    if has_program_error(diags.iter()) {
        diags.retain(|d| d.code != INTERNAL_ERROR);
    }
    // Every compile in a debug build (every test) holds the wiring invariant,
    // so a test that looks only for some other code still fails on it.
    if cfg!(debug_assertions)
        && let Some(d) = diags.iter().find(|d| d.code == INTERNAL_ERROR && d.message.contains(EXEC_INTO_VALUE))
    {
        panic!("{}", d.message);
    }
    if diags.iter().any(|d| d.code == "WS098") {
        diags.retain(|d| !(d.code == "WS003" && d.message.contains(ANY_HAS_NO_FIELDS)));
    }
    let records: Vec<SourceRange> = diags
        .iter()
        .filter(|d| d.code == "WS003" && d.message.contains(RECORD_INTO_ONE_VALUE))
        .map(|d| d.range.clone())
        .collect();
    if !records.is_empty() {
        let within = |d: &Diagnostic| {
            records.iter().any(|r| {
                r.file == d.range.file && r.start.offset <= d.range.start.offset && d.range.end.offset <= r.end.offset
            })
        };
        diags.retain(|d| !(d.code == "WS071" && within(d)));
    }
    let mut seen: crate::collections::HashSet<(std::sync::Arc<str>, usize)> = Default::default();
    diags.retain(|d| !REPORTED_TWICE.contains(&d.code.as_str()) || seen.insert((d.range.file.clone(), d.range.start.offset)));
}

/// The warning lowering gives an expression it emitted an `_Unsupported`
/// placeholder for, followed by what the expression is when that is known.
pub const PLACEHOLDER: &str = "IR lowering not yet supported for this expression, so a placeholder was emitted";

/// Make every placeholder warning an error when nothing else rejects the
/// program. The type check accepted the expression, so the placeholder is a
/// gap in the compiler, and the program would ship a gate that computes
/// nothing. Beside another error a placeholder is most likely that error's
/// consequence, and stays a warning.
pub fn promote_placeholders(diags: &mut [Diagnostic]) {
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return;
    }
    for d in diags.iter_mut() {
        if let Some(e) = placeholder_error(d) {
            *d = e;
        }
    }
}

/// What a placeholder's diagnostic names a record or tuple literal written
/// where one value goes.
pub const LITERAL_AS_ONE_VALUE: &str = "a record or tuple literal where one value goes";

/// `d` as the error it is in a program nothing else rejects, when `d` is a
/// placeholder warning (see [`promote_placeholders`]).
pub fn placeholder_error(d: &Diagnostic) -> Option<Diagnostic> {
    let rest = d.message.strip_prefix(PLACEHOLDER).filter(|_| d.code == "WSP001")?;
    let what = rest.strip_prefix(": ").unwrap_or("this expression");
    let fix = match what.strip_prefix("the method call ") {
        Some(method) => format!(
            "{method} is not a method the compiler can build for this value: call one the language reference \
             lists for its type"
        ),
        None if what == LITERAL_AS_ONE_VALUE => {
            "Write the type of the `var` holding it (`var v: T = ...`), or bind it with `let`".to_string()
        }
        None => "Rewrite it with a form the language reference documents".to_string(),
    };
    Some(Diagnostic {
        severity: Severity::Error,
        code: d.code.clone(),
        message: format!(
            "this expression is not supported by the compiler yet: {what}, which would compile to a gate that \
             computes nothing. {fix}"
        ),
        range: d.range.clone(),
    })
}

/// Errors type checking and lowering each report at one place: a call before
/// its `mod`'s declaration (`WS021`), and a handler trigger that is not an
/// event or a value (`WS001`). The first, type checking's, is kept.
const REPORTED_TWICE: &[&str] = &["WS021", "WS001"];

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
