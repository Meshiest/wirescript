//! Declaring and checking one `let`: its annotation, its bindings, and the
//! single-output alias it may carry.

use super::*;

/// The element types of `t` viewed as a tuple. A tuple literal desugars to a
/// record keyed by element index, so `Record([("0", T0), ("1", T1)])` describes
/// the same shape as `Tuple([T0, T1])` and destructures the same way.
fn as_tuple_fields(t: &Type) -> Option<Vec<Type>> {
    match t {
        Type::Tuple(fields) => Some(fields.clone()),
        Type::Record(fields) => fields
            .iter()
            .enumerate()
            .map(|(i, (key, ft))| (*key == i.to_string()).then(|| ft.clone()))
            .collect(),
        _ => None,
    }
}

/// Record `let f = Foo(…)` -> the sole output's name, when `Foo` is a user
/// chip/mod (plain or namespaced) with exactly one output. A single-output call
/// types as the bare output value, so `f.result` would otherwise be
/// indistinguishable from a typo; see `TypeCheckCtx::single_output_alias`.
/// Also propagates through a plain re-binding (`let g = f`) so an aliased
/// result keeps its projectable name.
pub(super) fn record_single_output_alias(ctx: &mut TypeCheckCtx, b: &LetBinding, value: &Expr) {
    let LetBinding::Ident { name, .. } = b else {
        return;
    };
    // `Some(entry)` = this binding IS a single-output call result;
    // `entry` names the one legal projection, or `None` if unindexed.
    let entry: Option<Option<String>> = match value {
        // `let g = f` — inherit whatever `f` may be projected by.
        Expr::Ident { name: src, .. } => ctx.single_output_alias.get(src).cloned(),
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident { name: callee_name, .. } => ctx
                .scope
                .lookup(callee_name)
                .and_then(|s| s.signature.as_ref())
                .filter(|sig| sig.outputs.len() == 1)
                .map(|sig| Some(sig.outputs[0].name.clone())),
            // `let r = ns.Foo(…)`. A multi-output member's return type is a
            // record (its fields are checked the normal way); a single-output
            // member's is the output type itself, whose NAME isn't indexed —
            // so mark it unindexed rather than risk calling a valid
            // projection a typo.
            Expr::FieldAccess { obj, field, .. } => match obj.as_ref() {
                Expr::Ident { name: ns, .. } => ctx
                    .namespaces
                    .get(ns.as_str())
                    .and_then(|m| m.get(field.as_str()))
                    .and_then(|info| info.return_type.as_ref())
                    .and_then(|te| match te {
                        TypeExpr::Record { .. } => None,
                        _ => Some(None),
                    }),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    if let Some(out) = entry {
        ctx.single_output_alias.insert(name.clone(), out);
    }
}

pub(super) fn bind_let(ctx: &mut TypeCheckCtx, b: &LetBinding, t: &Type) {
    match b {
        LetBinding::Ident { name, range } => {
            ctx.scope.declare(
                name,
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: name.clone(),
                    ty: t.clone(),
                    decl_range: range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        LetBinding::Tuple { names, range, .. } => {
            if let Some(fields) = as_tuple_fields(t)
                && fields.len() == names.len()
            {
                for (n, ft) in names.iter().zip(fields.iter()) {
                    ctx.scope.declare(
                        n,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: n.clone(),
                            ty: ft.clone(),
                            decl_range: range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                return;
            }
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: expected tuple[{}], got {:?}",
                    names.len(),
                    t
                ),
                range.clone(),
            );
            for n in names {
                ctx.scope.declare(
                    n,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: n.clone(),
                        ty: Type::Any,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        LetBinding::Record { names, range } => {
            for n in names {
                let ty = if let Type::Record(fields) = t {
                    fields
                        .iter()
                        .find(|(k, _)| k == n)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                } else {
                    Type::Any
                };
                ctx.scope.declare(
                    n,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: n.clone(),
                        ty,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        LetBinding::RecordDestruct { fields, range } => {
            for field in fields {
                let (name, ty) = match field {
                    crate::ast::RecordDestructField::Named { name, alias, .. } => {
                        let bind_name = alias.as_ref().unwrap_or(name);
                        let field_ty = if let Type::Record(rec_fields) = t {
                            rec_fields
                                .iter()
                                .find(|(k, _)| k == name)
                                .map(|(_, t)| t.clone())
                                .unwrap_or(Type::Any)
                        } else {
                            Type::Any
                        };
                        (bind_name.clone(), field_ty)
                    }
                    crate::ast::RecordDestructField::Rest { name, .. } => {
                        // Rest collects remaining fields into a new record
                        (name.clone(), Type::Any)
                    }
                };
                ctx.scope.declare(
                    &name,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: name.clone(),
                        ty,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
    }
}

pub(super) fn check_let_type_annotation(
    ctx: &mut TypeCheckCtx,
    l: &crate::ast::LetDecl,
    inferred: &Type,
) {
    if let Some(ref te) = l.typ {
        // Record literals: validate field names against the expected record type.
        // Point errors at the specific field/spread that introduced the mismatch.
        if let Expr::RecordLit { fields, .. } = &l.value {
            let expected = resolve_type_expr(ctx, te);
            warn_any_annotation(ctx, &expected, type_expr_range(te));
            let before = ctx.diagnostics.len();
            if let Type::Record(expected_fields) = &expected {
                let type_name = crate::analysis::types::type_expr_str(te);
                // Check each field/spread for extra fields
                for f in fields {
                    match f {
                        RecordLitField::Named { name, range, .. } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{}' not in type {}", name, type_name),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Shorthand { name, range } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{}' not in type {}", name, type_name),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Spread { value, range } => {
                            let spread_ty = infer::infer(ctx, value);
                            if let Type::Record(spread_fields) = &spread_ty {
                                let extras: Vec<&str> = spread_fields
                                    .iter()
                                    .filter(|(n, _)| !expected_fields.iter().any(|(en, _)| en == n))
                                    .map(|(n, _)| n.as_str())
                                    .collect();
                                if !extras.is_empty() {
                                    ctx.emit(
                                        "WS003",
                                        format!(
                                            "spread introduces fields not in {}: {}",
                                            type_name,
                                            extras.join(", ")
                                        ),
                                        range.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
                // Check for missing fields (use the whole literal range)
                if let Type::Record(inferred_fields) = inferred {
                    for (fname, _) in expected_fields {
                        if !inferred_fields.iter().any(|(n, _)| n == fname) {
                            ctx.emit(
                                "WS003",
                                format!("missing field '{}' for type {}", fname, type_name),
                                l.range.clone(),
                            );
                        }
                    }
                }
            }
            // A record literal is also how a tuple literal parses (index-keyed
            // fields `{"0": …, "1": …}`), so `expected` may be a `Type::Tuple`
            // the checks above skip entirely; and for a record target those
            // checks only cover field PRESENCE, not value types. If none fired,
            // run the full structural coercion so a wrong element/field value
            // type, a wrong tuple arity, or a record literal against a
            // non-record target is caught (WS003, like every other annotated
            // position). Guarded on the diag count so a missing/extra-field
            // error isn't duplicated by a whole-type mismatch.
            if ctx.diagnostics.len() == before {
                infer::coerce_or_emit(ctx, inferred, &expected, &l.range);
            }
            return;
        }
        let expected = resolve_type_expr(ctx, te);
        warn_any_annotation(ctx, &expected, type_expr_range(te));
        let rule = coerce(inferred, &expected);
        // `ViaString` is fine: anything primitive casts to string, so
        // `let s: string = 5` is an intentional format, not a type lie.
        if rule == CoerceRule::Mismatch {
            let name = match &l.binding {
                crate::ast::LetBinding::Ident { name, .. } => name.clone(),
                _ => "<binding>".into(),
            };
            ctx.diagnostics.push(crate::Diagnostic {
                severity: crate::diagnostic::Severity::Warning,
                code: "WS016".into(),
                message: format!(
                    "let '{}' annotated as {}, but expression has type {}",
                    name,
                    crate::analysis::types::type_expr_str(te),
                    crate::analysis::types::type_str(inferred),
                ),
                range: l.range.clone(),
            });
        }
    }
}
