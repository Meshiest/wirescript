//! Splitting an evaluated constant against a `let`/`const` binding form
//! (`const { x, y } = p`) into the `(name, value)` pairs it introduces.
//!
//! The split never re-evaluates anything. Every site that binds a destructured
//! constant (typecheck, lowering, the `const mod` interpreter) calls this one
//! module, so they agree on what each binding form means.

use crate::ast::{LetBinding, RecordDestructField};
use crate::collections::HashSet;
use crate::ir::Literal;

use super::error::{ConstError, ConstReason};

/// Split `value` into the `(bound_name, value)` pairs `binding` introduces, in
/// source order:
/// - `Ident`: the whole value binds to the name.
/// - `RecordDestruct`: `Named { name, alias }` binds the field `name` to
///   `alias` or `name`; `Rest` binds a record of every field not named, in the
///   source record's order.
/// - `Tuple`: positional. Each name binds the field at its index, names
///   ignored, so a tuple literal (keyed `"0"`, `"1"`) and a multi-output
///   `const mod` result split alike; `rest` is the remainder re-keyed from 0.
///   A width mismatch is [`ConstReason::TupleArityMismatch`].
///
/// A missing field is [`ConstReason::RecordFieldNotFound`] on that field's
/// range; a non-record `value` is unsupported. The names must match
/// [`bound_names`].
pub(crate) fn bind_destructured(
    binding: &LetBinding,
    value: Literal,
) -> Result<Vec<(String, Literal)>, ConstError> {
    match binding {
        LetBinding::Ident { name, .. } => Ok(vec![(name.clone(), value)]),
        // `typecheck::let_binding::bind_let` applies the same positional rule
        // to the types, so the arity accepted here is the one that typechecks.
        LetBinding::Tuple { names, rest, range } => {
            let Literal::Record(fields) = value else {
                return Err(not_a_record(range));
            };
            if fields.len() < names.len() || (rest.is_none() && fields.len() != names.len()) {
                return Err(ConstError::new(
                    ConstReason::TupleArityMismatch {
                        expected: names.len(),
                        got: fields.len(),
                    },
                    range,
                ));
            }
            let mut out: Vec<(String, Literal)> = names
                .iter()
                .cloned()
                .zip(fields.iter().map(|(_, v)| v.clone()))
                .collect();
            if let Some(rest_name) = rest {
                // `rest.0` is the first leftover, as a tuple literal keys its
                // fields and `lower::decl::install_tuple_destruct` builds it.
                let tail: Vec<(String, Literal)> = fields[names.len()..]
                    .iter()
                    .enumerate()
                    .map(|(i, (_, v))| (i.to_string(), v.clone()))
                    .collect();
                out.push((rest_name.clone(), Literal::Record(tail)));
            }
            Ok(out)
        }
        LetBinding::RecordDestruct {
            fields: pattern,
            range,
        } => {
            let Literal::Record(record_fields) = value else {
                return Err(not_a_record(range));
            };
            // The fields this pattern names, which a `Rest` excludes.
            let mut consumed: HashSet<String> = HashSet::default();
            let mut out = Vec::with_capacity(pattern.len());
            for field in pattern {
                match field {
                    RecordDestructField::Named { name, alias, range } => {
                        let field_value = record_fields
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, v)| v.clone())
                            .ok_or_else(|| {
                                ConstError::new(
                                    ConstReason::RecordFieldNotFound(name.clone()),
                                    range,
                                )
                            })?;
                        consumed.insert(name.clone());
                        let bound_name = alias.clone().unwrap_or_else(|| name.clone());
                        out.push((bound_name, field_value));
                    }
                    RecordDestructField::Rest { name, .. } => {
                        // Filtering `record_fields` keeps the source order.
                        let remaining: Vec<(String, Literal)> = record_fields
                            .iter()
                            .filter(|(n, _)| !consumed.contains(n))
                            .cloned()
                            .collect();
                        out.push((name.clone(), Literal::Record(remaining)));
                    }
                }
            }
            Ok(out)
        }
    }
}

/// The names `binding` introduces, in binding order, read from the pattern
/// alone (no value, so it cannot fail). Must equal the names
/// [`bind_destructured`] returns whenever that succeeds; the test
/// `bound_names_agrees_with_bind_destructured` checks it.
pub(crate) fn bound_names(binding: &LetBinding) -> Vec<String> {
    match binding {
        LetBinding::Ident { name, .. } => vec![name.clone()],
        LetBinding::Tuple { names, rest, .. } => names.iter().chain(rest.iter()).cloned().collect(),
        LetBinding::RecordDestruct { fields, .. } => fields
            .iter()
            .map(|f| match f {
                RecordDestructField::Named { name, alias, .. } => {
                    alias.clone().unwrap_or_else(|| name.clone())
                }
                RecordDestructField::Rest { name, .. } => name.clone(),
            })
            .collect(),
    }
}

fn not_a_record(range: &crate::diagnostic::SourceRange) -> ConstError {
    ConstError::unsupported("destructuring a constant that is not a record", range)
}
