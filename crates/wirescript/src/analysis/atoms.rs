//! Atom literals (`:name`) — compile-time `int` constants (the xxHash64 of the
//! name). Hover and find-references operate on the lexer's `Atom` tokens, so a
//! `:name` inside a string or comment is correctly ignored and the result is
//! independent of whether the surrounding program parses. Atoms are GLOBAL:
//! every `:name` resolves to the same value regardless of scope, so
//! find-references is a plain name match (no scope resolution).

use crate::diagnostic::SourceRange;
use crate::lexer::{lex, TokenKind, TokenValue};

/// One `:name` atom occurrence: its name, the `int` it hashes to, and its span.
pub struct AtomRef {
    pub name: String,
    pub value: i64,
    pub range: SourceRange,
}

/// Every `:name` atom literal in `source`, in source order.
fn all_atoms(source: &str, file: &str) -> Vec<AtomRef> {
    lex(source, file)
        .tokens
        .into_iter()
        .filter(|t| t.kind == TokenKind::Atom)
        .filter_map(|t| {
            // `read_atom` stores the name (without the leading `:`) as the token value.
            let name = match &t.value {
                Some(TokenValue::Str(s)) => s.clone(),
                _ => return None,
            };
            Some(AtomRef {
                value: crate::hash::atom_hash(&name),
                name,
                range: SourceRange { file: file.into(), start: t.start, end: t.end },
            })
        })
        .collect()
}

/// The atom under the cursor (`line`/`col` are 0-based, LSP convention), if any.
pub fn atom_at(source: &str, file: &str, line: usize, col: usize) -> Option<AtomRef> {
    let (cl, cc) = ((line + 1) as u32, (col + 1) as u32);
    all_atoms(source, file).into_iter().find(|a| {
        let (s, e) = (&a.range.start, &a.range.end);
        // The token span is single-line and half-open `[start, end)`.
        (cl, cc) >= (s.line, s.col) && (cl, cc) < (e.line, e.col)
    })
}

/// Every occurrence of the atom `name` in `source` (they all share one value).
pub fn atom_references(source: &str, file: &str, name: &str) -> Vec<SourceRange> {
    all_atoms(source, file)
        .into_iter()
        .filter(|a| a.name == name)
        .map(|a| a.range)
        .collect()
}

#[cfg(test)]
mod tests;
