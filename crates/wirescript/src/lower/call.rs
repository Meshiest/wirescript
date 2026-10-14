use super::*;

mod const_fold;
pub(super) use const_fold::*;
mod mono;
pub(super) use mono::*;
mod binding;
pub(crate) use binding::*;
mod wiring;
use wiring::*;
mod builtin;
pub(super) use builtin::*;
mod dispatch;
pub(super) use dispatch::*;
mod instance_body;
use instance_body::*;
pub(in crate::lower) use instance_body::wire_record_backing;
mod inline;
pub(super) use inline::*;
mod instance;
pub(super) use instance::*;

/// The call's arguments given by position, in order. `Named` and `Spread` args
/// are dropped: a spread has already been expanded by
/// [`expand_spread_args`] wherever positional binding runs, and a named arg
/// binds by name, not index.
pub(in crate::lower) fn positional_args(args: &[CallArg]) -> Vec<&Expr> {
    args.iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect()
}

/// Expand each `...tuple` spread argument into one `TuplePick` positional arg per
/// element, so the ordinary positional binding wires every element into its own
/// param/port. Arity comes from the spread expression's tuple/record type
/// (recorded by typecheck); an unresolved / non-tuple spread expands to nothing
/// (typecheck already reported it). A call with no spreads is returned as-is.
pub(in crate::lower) fn expand_spread_args<'a>(
    ctx: &LowerCtx,
    args: &'a [CallArg],
) -> std::borrow::Cow<'a, [CallArg]> {
    if !args.iter().any(|a| matches!(a, CallArg::Spread(_))) {
        return std::borrow::Cow::Borrowed(args);
    }
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        match a {
            // A tuple/record LITERAL (a tuple `(a, b)` desugars to a numeric-field
            // record literal) expands to its field value expressions directly — a
            // literal isn't a resolvable binding, so a `TuplePick` on it would not
            // lower. Each value then lowers in place.
            CallArg::Spread(t @ Expr::RecordLit { fields, .. }) => {
                let _ = t;
                for f in fields {
                    match f {
                        crate::ast::RecordLitField::Named { value, .. } => {
                            out.push(CallArg::Positional(value.clone()))
                        }
                        crate::ast::RecordLitField::Shorthand { name, range } => {
                            out.push(CallArg::Positional(Expr::Ident {
                                name: name.clone(),
                                range: range.clone(),
                            }))
                        }
                        crate::ast::RecordLitField::Spread { .. } => {}
                    }
                }
            }
            // A bound tuple (`let t = (a, b)`) or a field chain reaching one: pick
            // each element by index (resolves through its `Binding::Record`). Arity
            // comes from the live binding when the spread reaches one — a variadic
            // `...rest` is bound per call site and has no single static tuple type,
            // so the type map alone (checked once at the mod's declaration) would
            // under-count it. Fall back to the recorded type otherwise.
            CallArg::Spread(t) => {
                let ty = ctx.type_of(t);
                let inner = crate::types::mono::unwrap_ref(&ty);
                let n = match resolve_field_chain(ctx, t) {
                    Some(Binding::Record(fields)) => fields.len(),
                    _ => match &inner {
                        Type::Tuple(elems) => elems.len(),
                        Type::Record(fields) => fields.len(),
                        _ => 0,
                    },
                };
                // Pick each element by its declared KEY, not a numeric index. A
                // tuple literal binds its record by "0"/"1", but a multi-output
                // CALL result binds it by output NAME, so a numeric `TuplePick`
                // missed the call-result case and lowered to `_Unsupported`.
                // `tuple_positions` yields the ordered keys from the type (field
                // names for a record, "0".."n" for a tuple), and a `FieldAccess`
                // resolves both spellings — including an inline `...f()`, via the
                // record-returning-call field projection in `lower_field_access`.
                for key in crate::lower::decl::tuple_positions(&inner, n) {
                    out.push(CallArg::Positional(Expr::FieldAccess {
                        obj: Box::new(t.clone()),
                        field: key,
                        range: t.range().clone(),
                    }));
                }
            }
            other => out.push(other.clone()),
        }
    }
    std::borrow::Cow::Owned(out)
}

/// A `mod`/`chip` call with its named arguments moved to their parameters'
/// positions, so the positional binding below sees `scale(n, by = 3)` as
/// `scale(n, 3)`. A parameter skipped before a named one takes its default.
/// Named args that name no parameter (`exec = ...`) stay as they are, and a
/// call with a spread is left alone: its positions are only known once the
/// spread is expanded.
fn named_args_in_place<'a>(decl: &ChipDecl, args: &'a [CallArg]) -> std::borrow::Cow<'a, [CallArg]> {
    let param_of = |name: &str| decl.inputs.iter().position(|p| p.name == name);
    let moves = args.iter().any(|a| matches!(a, CallArg::Named { name, .. } if param_of(name).is_some()));
    if !moves || args.iter().any(|a| matches!(a, CallArg::Spread(_))) {
        return std::borrow::Cow::Borrowed(args);
    }
    let positional = positional_args(args);
    let mut slots: Vec<Option<&Expr>> = vec![None; decl.inputs.len()];
    for (slot, e) in slots.iter_mut().zip(&positional) {
        *slot = Some(e);
    }
    let mut kept = Vec::new();
    for a in args {
        match a {
            CallArg::Named { name, value, .. } => match param_of(name) {
                Some(i) if slots[i].is_none() => slots[i] = Some(value),
                Some(_) => {}
                None => kept.push(a.clone()),
            },
            CallArg::Positional(_) | CallArg::Spread(_) => {}
        }
    }
    let given = slots.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
    let mut out = Vec::with_capacity(args.len());
    for (slot, param) in slots[..given].iter().zip(&decl.inputs) {
        match slot.or(param.default.as_ref()) {
            Some(e) => out.push(CallArg::Positional(e.clone())),
            None => break,
        }
    }
    out.extend(positional.iter().skip(decl.inputs.len()).map(|e| CallArg::Positional((*e).clone())));
    out.extend(kept);
    std::borrow::Cow::Owned(out)
}

pub(super) fn lower_chip_call(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    type_args: &[TypeExpr],
    range: &SourceRange,
) -> PortRef {
    if chip_decl.is_fn {
        return crate::lower::function::lower_fn_call(ctx, chip_decl, args, range);
    }
    let named = !chip_decl.name.is_empty();
    if named && ctx.chip_call_stack.iter().any(|(decl, open)| *open && *decl == chip_decl.range) {
        ctx.diagnostics.push(Diagnostic::error(
            "WS020",
            format!(
                "recursive call to `{}`: chips and mods cannot call themselves \
                 (directly or mutually): every call is expanded into the wire \
                 graph at compile time. Re-trigger an exec input or use a \
                 buffer-based loop instead.",
                chip_decl.name
            ),
            range.clone(),
        ));
        return synthesise_unsupported_range(ctx, &format!("the call `{}(..)`", chip_decl.name), range);
    }
    if named {
        ctx.chip_call_stack.push((chip_decl.range.clone(), false));
    }

    let args = &*named_args_in_place(chip_decl, args);
    let result = if chip_decl.inline {
        lower_chip_call_inline(ctx, chip_decl, args, type_args, range)
    } else {
        lower_chip_call_instance(ctx, chip_decl, args, type_args, range)
    };

    if named {
        ctx.chip_call_stack.pop();
    }
    result
}
