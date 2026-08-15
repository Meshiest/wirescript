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
