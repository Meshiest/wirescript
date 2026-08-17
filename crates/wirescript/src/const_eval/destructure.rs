//! Splitting an already-evaluated compile-time constant against a
//! `let`/`const` BINDING FORM.
//!
//! `eval_expr` (and `interp::eval_call`) produce one [`Literal`] for the
//! whole right-hand side of a binding; a destructuring binding
//! (`const { x, y } = p`) then needs to split that single value into the
//! several `(name, value)` pairs the binding form introduces. That split is
//! syntax-only — it never re-evaluates anything — so it lives in its own
//! small module shared by every site that registers a destructured constant:
//! `typecheck::decl`'s top-level `const`, `typecheck::stmt`'s block-scope
//! `const`, and `const_eval::interp::exec_block`'s `Stmt::Let` arm (a
//! destructuring binding inside a `const mod` body). A single shared
//! function is what keeps those three sites from drifting on what a given
//! binding form means.

use crate::ast::{LetBinding, RecordDestructField};
use crate::collections::HashSet;
use crate::ir::Literal;

use super::error::{ConstError, ConstReason};

/// Splits an already-evaluated `value` into the `(bound_name, value)` pairs
/// `binding` introduces, in BINDING (source) order.
///
/// - `Ident { name }` — the trivial case: the whole value binds to the one
///   name.
/// - `Record { names }` — `value` must be a [`Literal::Record`]; each name
///   binds its same-named field.
/// - `RecordDestruct { fields }` — `value` must be a [`Literal::Record`];
///   `Named { name, alias }` binds `alias.unwrap_or(name)` to the field
///   called `name`, and `Rest { name }` binds a fresh [`Literal::Record`] of
///   every field no `Named` in this pattern consumed, preserving the source
///   record's own field order.
/// - `Tuple { names, rest }` — POSITIONAL: `value` must be a
///   [`Literal::Record`], and each name binds the field at its own index in
///   that record's own field order. Field NAMES are never consulted, which is
///   what makes the two record shapes that reach here behave identically: a
///   tuple LITERAL evaluates to an index-keyed record (`{"0": …, "1": …}`),
///   and a multi-output `const mod`'s result to a NAME-keyed one in its
///   signature's declaration order. `rest` takes every remaining position as
///   a fresh [`Literal::Record`] re-keyed from zero.
///   `typecheck::let_binding::bind_let`'s `Tuple` arm applies the same rule
///   to the TYPES, so what type-checks is what splits here; a width mismatch
///   is [`ConstReason::TupleArityMismatch`], which names both counts.
///
/// A name naming no matching field is [`ConstReason::RecordFieldNotFound`],
/// blamed on THAT NAME's own range (not the whole statement) — `Record`'s
/// bare `names: Vec<String>` carries no per-name range, so it falls back to
/// the binding's own range; `RecordDestruct`'s `Named` fields carry one each.
/// A `value` that isn't a record at all is
/// `ConstReason::Unsupported("destructuring a constant that is not a
/// record")`.
///
/// See [`bound_names`] for the purely SYNTACTIC list of the names this
/// introduces — the two must agree, and
/// `bound_names_agrees_with_bind_destructured` in `tests.rs` asserts it.
pub(crate) fn bind_destructured(
    binding: &LetBinding,
    value: Literal,
) -> Result<Vec<(String, Literal)>, ConstError> {
    match binding {
        LetBinding::Ident { name, .. } => Ok(vec![(name.clone(), value)]),
        // Positional split, in the source record's own field order. A tuple
        // LITERAL evaluates to an index-keyed record (`{"0": …, "1": …}`) and
        // a multi-output `const mod`'s result to a NAME-keyed one, in its
        // signature's declaration order — both are positional in exactly the
        // sense a tuple pattern means, so both split here the same way and
        // field names are never consulted. `typecheck::let_binding::bind_let`
        // applies the identical rule to the TYPES (`as_tuple_fields`), so the
        // arity this accepts is the arity that type-checks.
        LetBinding::Tuple { names, rest, range } => {
            let Literal::Record(fields) = value else {
                return Err(not_a_record(range));
            };
            if fields.len() < names.len() || (rest.is_none() && fields.len() != names.len()) {
                return Err(ConstError {
                    reason: ConstReason::TupleArityMismatch {
                        expected: names.len(),
                        got: fields.len(),
                    },
                    range: range.clone(),
                });
            }
            let mut out: Vec<(String, Literal)> = names
                .iter()
                .cloned()
                .zip(fields.iter().map(|(_, v)| v.clone()))
                .collect();
            if let Some(rest_name) = rest {
                // The remainder is re-keyed by its NEW position, so `rest.0`
                // is the first leftover — matching how a tuple literal keys
                // its own fields, and how `lower::decl::install_tuple_destruct`
                // rebuilds the tail.
                let tail: Vec<(String, Literal)> = fields[names.len()..]
                    .iter()
                    .enumerate()
                    .map(|(i, (_, v))| (i.to_string(), v.clone()))
                    .collect();
                out.push((rest_name.clone(), Literal::Record(tail)));
            }
            Ok(out)
        }
        LetBinding::Record { names, range } => {
            let Literal::Record(fields) = value else {
                return Err(not_a_record(range));
            };
            let mut out = Vec::with_capacity(names.len());
            for name in names {
                let field_value = fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| ConstError {
                        reason: ConstReason::RecordFieldNotFound(name.clone()),
                        range: range.clone(),
                    })?;
                out.push((name.clone(), field_value));
            }
            Ok(out)
        }
        LetBinding::RecordDestruct { fields: pattern, range } => {
            let Literal::Record(record_fields) = value else {
                return Err(not_a_record(range));
            };
            // Every `Named` field this pattern consumes, tracked so a
            // trailing `Rest` can exclude exactly those (and only those)
            // fields — never fields the SOURCE record happens to share a
            // name with something else, and never anything a sibling `Rest`
            // in some OTHER pattern would consume.
            let mut consumed: HashSet<String> = HashSet::default();
            let mut out = Vec::with_capacity(pattern.len());
            for field in pattern {
                match field {
                    RecordDestructField::Named { name, alias, range } => {
                        let field_value = record_fields
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, v)| v.clone())
                            .ok_or_else(|| ConstError {
                                reason: ConstReason::RecordFieldNotFound(name.clone()),
                                range: range.clone(),
                            })?;
                        consumed.insert(name.clone());
                        let bound_name = alias.clone().unwrap_or_else(|| name.clone());
                        out.push((bound_name, field_value));
                    }
                    RecordDestructField::Rest { name, .. } => {
                        // Preserve the SOURCE record's own field order —
                        // filtering `record_fields` (rather than building the
                        // rest from `consumed`, which has no order of its
                        // own) is what keeps that order intact.
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

/// The names `binding` introduces, in binding order — SYNTACTICALLY, with no
/// value to destructure and therefore no possibility of failure.
///
/// [`bind_destructured`] is the sole authority on destructuring SEMANTICS
/// (which field lands on which name, what `...rest` collects, what errors);
/// this answers only "which names does this binding form declare", which is
/// visible from the pattern alone. It exists because
/// `lower::predeclare::build_const_declared_names` needs exactly that and
/// nothing more: it records which top-level names were spelled `const`,
/// runs before any value has been evaluated, and never looks at a value at
/// all.
///
/// Kept in THIS module, beside `bind_destructured`, precisely so the two
/// cannot drift apart unnoticed — `bound_names(b)` must equal the names
/// `bind_destructured(b, v)` returns whenever the latter succeeds, which
/// `bound_names_agrees_with_bind_destructured` in `tests.rs` asserts
/// directly for every supported binding form.
///
/// `Tuple` is enumerated here for the same reason as every other form: these
/// are the names it declares, which is a purely syntactic question. Keeping
/// all four arms here means there is no second site to remember to update
/// when a binding form's SEMANTICS change — as `Tuple`'s did when
/// `bind_destructured` learned to split a record positionally.
pub(crate) fn bound_names(binding: &LetBinding) -> Vec<String> {
    match binding {
        LetBinding::Ident { name, .. } => vec![name.clone()],
        LetBinding::Tuple { names, rest, .. } => {
            names.iter().chain(rest.iter()).cloned().collect()
        }
        LetBinding::Record { names, .. } => names.clone(),
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
    ConstError {
        reason: ConstReason::Unsupported("destructuring a constant that is not a record"),
        range: range.clone(),
    }
}
