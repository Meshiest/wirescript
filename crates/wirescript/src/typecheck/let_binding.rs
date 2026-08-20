//! Declaring and checking one `let`: its annotation, its bindings, and the
//! single-output alias it may carry.

use super::*;

/// The element types of `t` viewed as a tuple, POSITIONALLY.
///
/// A tuple literal desugars to a record keyed by element index, so
/// `Record([("0", T0), ("1", T1)])` describes the same shape as
/// `Tuple([T0, T1])`. A multi-output `mod`'s result types as a NAME-keyed
/// record instead (`output_record_type` in `typecheck::call`, keyed by the
/// signature's own output names) — but its fields are in the signature's
/// DECLARATION order, which is exactly what a positional pattern means, so a
/// tuple pattern reads it the same way. Field NAMES are ignored here on
/// purpose: `(a, b)` says "the first two, in order", and nothing else.
fn as_tuple_fields(t: &Type) -> Option<Vec<Type>> {
    match t {
        Type::Tuple(fields) => Some(fields.clone()),
        Type::Record(fields) => Some(fields.iter().map(|(_, ft)| ft.clone()).collect()),
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

/// The `const` post-binding assertion: after a `const` declaration's
/// initializer has evaluated AND been recorded, every name it introduced must
/// READ BACK as a compile-time constant at this point. If one does not, say so
/// HERE, at the binding — reusing WS046.
///
/// This is deliberately NOT the "initializer failed to evaluate" check (the
/// `Err(err) if l.is_const` arms in `typecheck::decl`/`typecheck::stmt` already
/// do that, and run before this). It is the OPPOSITE failure: an initializer
/// that evaluates perfectly well, whose value then lands nowhere any later
/// statement can see, because no environment in scope accepted it.
///
/// That failure used to be invisible at the declaration and surfaced later, at
/// some USE, blamed on a DIFFERENT binding — `const a = 1` recorded nowhere is
/// silent, and only the `const b = a + 1` that reads it reports "'a' is a
/// runtime value". Worse, a `const` whose only use is a constant-only slot
/// (a custom-event channel name, gate config) reported nothing at all and
/// baked an empty value; two silent miscompiles in this feature's history had
/// exactly that shape.
///
/// The oracle is plain `const_lookup` — "is a value for this name readable
/// here at all" — NOT `const_lookup_declared_only`. The stronger question
/// ("is it marked const-DECLARED in this frame") looks like the better fit and
/// is not: `typecheck::decl`'s `TopDecl::Let` arm records a value into
/// `scoped_consts` without ever writing the matching `scoped_const_declared`
/// mark, which is harmless at the top level (no frame is open there, so the
/// insert is a no-op and the marks come from `build_const_declared_names`) but
/// NOT inside the pushed scope `TopDecl::Namespace` checks a namespaced
/// module's members in. Under the strict oracle every `import * as ns` of a
/// module containing a top-level `const` reported here — measured: it made
/// `compile` fail on programs `check` passed, the exact check-vs-compile split
/// this feature is supposed to close.
///
/// The weaker oracle still catches every failure this assertion exists for,
/// because those record the name NOWHERE at all rather than recording it
/// without a mark. That asymmetry between the two recording sites is real and
/// pre-existing; it errs in the safe direction (typecheck over-checks a block
/// lowering elides) and is left alone here rather than fixed blind.
///
/// Ordering requirement: call this only AFTER the success arm has done its
/// recording, and only on the success path (an initializer that failed to
/// evaluate has already been reported, and would otherwise be reported twice).
/// At the TOP level the recording that matters is not the local one — it is
/// `build_const_env`'s whole-program fixpoint, which has already converged
/// before any decl is checked, so a name still missing from it here is settled
/// and not merely unresolved yet.
pub(super) fn check_const_recorded(ctx: &mut TypeCheckCtx, l: &LetDecl) {
    if !l.is_const {
        return;
    }
    let env = ctx.const_lookup();
    let missing: Vec<String> = crate::const_eval::bound_names(&l.binding)
        .into_iter()
        .filter(|n| !env.contains_key(n))
        .collect();
    for name in missing {
        ctx.emit(
            "WS046",
            format!(
                "not a compile-time constant: '{name}' is declared `const` and its value \
                 evaluates, but it is not recorded as a constant in this scope — reading it \
                 would give a runtime value, and passing it to a constant-only slot would \
                 bake an empty one"
            ),
            l.range.clone(),
        );
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
        LetBinding::Tuple { names, rest, range } => {
            // `rest` takes everything past `names`, so the source only has to
            // be AT LEAST as wide as the named positions when one is present.
            if let Some(fields) = as_tuple_fields(t)
                && (fields.len() == names.len()
                    || (rest.is_some() && fields.len() >= names.len()))
            {
                if let Some(rest_name) = rest {
                    let tail: Vec<(String, Type)> = fields[names.len()..]
                        .iter()
                        .enumerate()
                        .map(|(i, ft)| (i.to_string(), ft.clone()))
                        .collect();
                    ctx.scope.declare(
                        rest_name,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: rest_name.clone(),
                            ty: Type::Record(tail),
                            decl_range: range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
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
            let got = match as_tuple_fields(t) {
                Some(fields) => format!("{} value(s)", fields.len()),
                None => format!("{t:?}, which has no positional fields"),
            };
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: this pattern binds {} position(s){}, but the value has {got}",
                    names.len(),
                    if rest.is_some() { " plus a rest" } else { "" },
                ),
                range.clone(),
            );
            for n in names.iter().chain(rest.iter()) {
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
            // Field names consumed by a `Named` SO FAR, grown as the pattern
            // is walked. Incremental (prefix) rather than whole-pattern
            // precisely so this stays a true mirror of
            // `const_eval::bind_destructured`, which builds its own `consumed`
            // the same way: a `Named` appearing AFTER a `Rest` is not excluded
            // from that rest by either side. The parser currently forces
            // `...rest` last (`parser/decl.rs`), so the two forms agree on
            // every pattern that can be written today — but agreeing only by
            // accident of the grammar is exactly the latent divergence that
            // bites when the grammar relaxes.
            let mut consumed: crate::collections::HashSet<&str> =
                crate::collections::HashSet::default();
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
                        consumed.insert(name.as_str());
                        (bind_name.clone(), field_ty)
                    }
                    crate::ast::RecordDestructField::Rest { name, .. } => {
                        // `...rest` collects every field no `Named` consumed,
                        // so its type is the record of exactly those fields —
                        // the type-level mirror of `bind_destructured`'s
                        // value-level rest rule, down to preserving the SOURCE
                        // record's field order. Typing it `Any` (as this did
                        // before) made `rest.y` type as `Any` too, so any use
                        // of it — `rest.y == 222` — failed with WS004 "no
                        // overload on Any, Int": `...rest` bound a value that
                        // could not actually be read. Only a known
                        // `Type::Record` source yields a precise type;
                        // anything else stays `Any` exactly as before.
                        let rest_ty = if let Type::Record(rec_fields) = t {
                            Type::Record(
                                rec_fields
                                    .iter()
                                    .filter(|(k, _)| !consumed.contains(k.as_str()))
                                    .cloned()
                                    .collect(),
                            )
                        } else {
                            Type::Any
                        };
                        (name.clone(), rest_ty)
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

/// A binding whose initializer has no value (`Type::Never` — a void container
/// mutation like `a.push(x)` / `m.set(k, v)` used as a value) has nothing to
/// bind. Report it and return `true`. Nothing but a void mutation produces
/// `Never` today, so this can't fire spuriously. Shared by `let`/`var`/`out`.
pub(super) fn reject_never_value(
    ctx: &mut TypeCheckCtx,
    inferred: &Type,
    range: &SourceRange,
    name: &str,
) -> bool {
    if !matches!(inferred, Type::Never) {
        return false;
    }
    ctx.emit(
        "WS003",
        format!(
            "`{name}` is bound to an expression that produces no value (a void mutation \
             such as `a.push(x)` or `m.set(k, v)`); use it as a statement, not a value"
        ),
        range.clone(),
    );
    true
}

/// `reject_never_value` for a `let` binding: returning `true` lets the caller
/// skip the ordinary annotation check, which would only add a redundant WS016.
pub(super) fn reject_never_binding(
    ctx: &mut TypeCheckCtx,
    l: &crate::ast::LetDecl,
    inferred: &Type,
) -> bool {
    let name = match &l.binding {
        crate::ast::LetBinding::Ident { name, .. } => name.as_str(),
        _ => "<binding>",
    };
    reject_never_value(ctx, inferred, l.value.range(), name)
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
