//! Declaration checks for parameter default values and for `fn`, whose one
//! definition plane fixes what its signature can hold.

use super::ctx::TypeCheckCtx;
use crate::ast::ChipDecl;
use crate::ir::Type;

/// The most values a Function Call passes in or hands back (`Input0..7`,
/// `Output0..7`).
const MAX_SLOTS: usize = 8;

pub(super) fn check_signature(ctx: &mut TypeCheckCtx, c: &ChipDecl) {
    check_defaults(ctx, c);
    if c.is_fn {
        check_fn(ctx, c);
    }
}

/// A default value is a constant of its parameter's type, on a trailing
/// parameter of a `mod` or `fn`.
fn check_defaults(ctx: &mut TypeCheckCtx, c: &ChipDecl) {
    let mut seen_default = false;
    for p in &c.inputs {
        let Some(d) = &p.default else {
            if seen_default {
                ctx.emit(
                    "WS114",
                    format!(
                        "`{}` has no default value but follows a parameter that has one: only \
                         trailing parameters may have defaults, since an argument is matched by position",
                        p.name
                    ),
                    p.range.clone(),
                );
            }
            continue;
        };
        seen_default = true;
        if !c.inline {
            ctx.emit(
                "WS114",
                format!(
                    "`{}` is a chip parameter with a default value; only `mod` and `fn` parameters \
                     may have one",
                    p.name
                ),
                d.range().clone(),
            );
            continue;
        }
        let want = super::resolve::resolve_type_expr(ctx, &p.typ);
        let got = super::infer::infer(ctx, d);
        super::infer::coerce_value_or_emit(ctx, d, &got, &want);
        let lookup = |n: &str| ctx.resolve_mod(n);
        let cx = ctx.const_ctx(Some(&lookup));
        let mut budget = crate::const_eval::Budget::default();
        if crate::const_eval::eval_expr(d, &cx, &mut budget).is_err() {
            ctx.emit(
                "WS046",
                format!(
                    "the default value of `{}` must be a compile-time constant",
                    p.name
                ),
                d.range().clone(),
            );
        }
    }
}

/// A `fn` is one plane: no type parameters or per-call constants, values in
/// and out through at most eight slots each, a record parameter one slot per
/// field.
fn check_fn(ctx: &mut TypeCheckCtx, c: &ChipDecl) {
    let unsupported =
        |ctx: &mut TypeCheckCtx, what: &str, range: crate::diagnostic::SourceRange| {
            ctx.emit(
                "WS114",
                format!(
                    "`fn {}` {what}. A `fn` compiles once to one plane; use a `mod` for this",
                    c.name
                ),
                range,
            );
        };
    if !c.type_params.is_empty() {
        unsupported(
            ctx,
            "has type parameters, which would need a plane per type",
            c.range.clone(),
        );
        return;
    }
    if c.rest.is_some() {
        unsupported(
            ctx,
            "takes `...rest`, which would need a plane per argument count",
            c.range.clone(),
        );
    }
    let mut slots = 0;
    for p in &c.inputs {
        if p.is_const {
            unsupported(
                ctx,
                &format!(
                    "has a `const` parameter `{}`, which would need a plane per value",
                    p.name
                ),
                p.range.clone(),
            );
        }
        let ty = super::resolve::resolve_type_expr(ctx, &p.typ);
        match crate::types::mono::unwrap_ref(&ty) {
            Type::Exec => unsupported(
                ctx,
                &format!("takes `{}` as an exec; call it instead", p.name),
                p.range.clone(),
            ),
            Type::Record(fields) => {
                if fields.iter().any(|(_, t)| !one_slot(t)) {
                    unsupported(
                        ctx,
                        &format!(
                            "takes `{}`, a record holding a record, tuple or enum; pass its fields",
                            p.name
                        ),
                        p.range.clone(),
                    );
                }
                slots += fields.len();
            }
            t if !one_slot(&t) => {
                unsupported(
                    ctx,
                    &format!("takes `{}`, a tuple or enum; pass its parts", p.name),
                    p.range.clone(),
                );
                slots += 1;
            }
            _ => slots += 1,
        }
    }
    for o in &c.outputs {
        let ty = super::resolve::resolve_type_expr(ctx, &o.typ);
        if !one_slot(&ty) || ty == Type::Exec {
            unsupported(
                ctx,
                "returns a record, tuple, enum or exec; return its values instead",
                o.range.clone(),
            );
        }
    }
    for (what, n) in [("inputs", slots), ("results", c.outputs.len())] {
        if n > MAX_SLOTS {
            ctx.emit(
                "WS115",
                format!(
                    "`fn {}` has {n} {what}, and a Function Call carries at most {MAX_SLOTS} \
                     (a record parameter takes one per field)",
                    c.name
                ),
                c.range.clone(),
            );
        }
    }
}

/// Whether a value of `t` fits one call slot.
fn one_slot(t: &Type) -> bool {
    !matches!(
        crate::types::mono::unwrap_ref(t),
        Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }
    )
}
