//! The certified evaluator, re-exported from `wirescript_ir::semantics` (the
//! laws, the table loader and the replay tests live there), plus the
//! interpolation grouping that needs the language's AST.
use std::fmt::Write as _;

// `pub` for the `--fold-diff` fuzz harness (via the `fold_eval` re-export).
pub use wirescript_ir::semantics::{
    FORMAT_SLOTS, Value, eval, format_text, render_for_format, sanitize, truthy,
};

/// The `FormatText` templates an interpolation `parts` renders through: each
/// literal chunk with `{`/`}` escaped, each `${expr}` a `{N}` slot numbered
/// from 0 within its group, paired with what `value` gives for that
/// expression. A group holds at most [`FORMAT_SLOTS`] substitutions; one more
/// starts the next.
pub(crate) fn interp_groups<T, E>(
    parts: &[crate::ast::InterpPart],
    mut value: impl FnMut(&crate::ast::Expr) -> Result<T, E>,
) -> Result<Vec<(String, Vec<T>)>, E> {
    use crate::ast::InterpPart;
    let mut groups: Vec<(String, Vec<T>)> = Vec::new();
    let mut fmt = String::new();
    let mut slots: Vec<T> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Lit(s) => fmt.push_str(&s.replace('{', "{{").replace('}', "}}")),
            InterpPart::Expr(expr) => {
                if slots.len() == FORMAT_SLOTS.len() {
                    groups.push((std::mem::take(&mut fmt), std::mem::take(&mut slots)));
                }
                let v = value(expr)?;
                let _ = write!(fmt, "{{{}}}", slots.len());
                slots.push(v);
            }
        }
    }
    groups.push((fmt, slots));
    Ok(groups)
}
