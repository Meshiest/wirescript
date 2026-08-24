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
mod inline;
pub(super) use inline::*;
mod instance;
pub(super) use instance::*;

/// Expand each `...tuple` spread argument into one `TuplePick` positional arg per
/// element, so the ordinary positional binding wires every element into its own
/// param/port. Arity comes from the spread expression's tuple/record type
/// (recorded by typecheck); an unresolved / non-tuple spread expands to nothing
/// (typecheck already reported it). A call with no spreads is returned as-is.
pub(in crate::lower) fn expand_spread_args(ctx: &LowerCtx, args: &[CallArg]) -> Vec<CallArg> {
    if !args.iter().any(|a| matches!(a, CallArg::Spread(_))) {
        return args.to_vec();
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
    out
}

pub(super) fn lower_chip_call(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    type_args: &[TypeExpr],
    range: &SourceRange,
) -> PortRef {
    let named = !chip_decl.name.is_empty();
    if named && ctx.chip_call_stack.contains(&chip_decl.range) {
        ctx.diagnostics.push(Diagnostic::error(
            "WS020",
            format!(
                "recursive call to `{}` — chips and mods cannot call themselves \
                 (directly or mutually): every call is expanded into the wire \
                 graph at compile time. Re-trigger an exec input or use a \
                 buffer-based loop instead.",
                chip_decl.name
            ),
            range.clone(),
        ));
        return synthesise_unsupported_range(ctx, range);
    }
    if named {
        ctx.chip_call_stack.push(chip_decl.range.clone());
    }

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
