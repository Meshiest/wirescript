//! Unified call-argument checking.
//!
//! `CallSignature`/`Param` describe a callable shape (builtin call, receiver
//! method, event handler config, …) independent of the source it was built
//! from, and `check_args` type-checks a call's `CallArg`s against it — arity,
//! per-param type coercion, and the config-only param dance (enum member
//! names, composite constant folds, scalar constant-only fields, and
//! data-driven settings-menu attributes keyed off the gate's inventory
//! `config` list). It reads from `CallSignature` (rather than a specific
//! `catalog::calls::CallSpec`) so every call form can route through one
//! checker. All call forms now go through it: builtins and receivers via
//! `sig_of_callspec`, user mod/chip + self-receiver calls via
//! `sig_of_fnchip` (both adapters live in `typecheck.rs`).

use crate::ast::{CallArg, Expr};
use crate::diagnostic::SourceRange;
use crate::ir::Type;
use crate::types::coerce::{CoerceRule, coerce};
use crate::types::mono::unwrap_ref;

use super::{
    TypeCheckCtx, validate_composite_config_arg, validate_data_driven_config,
    validate_enum_config_arg, validate_scalar_config_arg,
};

/// What a `Param` binds to, and how `check_args` should validate an argument
/// against it. Each config variant carries the gate PORT name as its
/// `&'static str` payload (the key for the schema/shape lookup); `Param::name`
/// stays the SURFACE name (the key for named-arg matching + diagnostics), so
/// the two never have to share one field.
#[derive(Clone, Debug)]
pub enum ParamKind {
    /// An ordinary wire-input param: infer the argument's type and `coerce`
    /// it against `Param::ty`.
    Wire,
    /// A data-only (non-wire) param backed by a schema enum field. The
    /// `&'static str` is the enum type name (`crate::catalog::enum_*`
    /// lookups).
    ConfigEnum(&'static str),
    /// A composite constant-only config param (`meshColors: Color[]`,
    /// `ammoOverride`'s nested struct) that must fold to a constant of the
    /// expected shape. The `&'static str` is the gate PORT name the composite
    /// shape is keyed on (`"MeshColors"` / `"WeaponAmmoOverride"`).
    ConfigComposite(&'static str),
    /// A plain scalar/asset constant-only config param — must fold to a
    /// literal. The `&'static str` is the gate PORT name (carried for
    /// symmetry / future per-port rules; the constant check itself needs only
    /// the value).
    ConfigScalar(&'static str),
}

/// One parameter of a `CallSignature`.
#[derive(Clone, Debug)]
pub struct Param {
    /// SURFACE parameter name — always the source-level name. Used both for
    /// named-arg matching (`meshColors = …`) and in diagnostics
    /// (`argument '{name}': …`). Gate PORT names for config schema lookups
    /// live on the `ParamKind` variant payload, never here.
    pub name: String,
    pub ty: Type,
    /// When true, callers may omit the argument.
    pub optional: bool,
    pub kind: ParamKind,
}

/// A callable shape: name, params (positional-order), and — for
/// builtins/receivers backed by a gate — the gate class data-driven config
/// (`config` array from the inventory) validates against. The call's RESULT
/// type is derived separately by each caller (e.g. `output_record_type`,
/// `array_return_type`, `map_return_type`) — not carried on this struct.
pub struct CallSignature {
    pub name: String,
    pub params: Vec<Param>,
    /// Gate class for data-driven settings config (`<FieldName> = value`
    /// named args not covered by a declared `Param`). `None` for call forms
    /// with no backing gate (e.g. plain math).
    pub config_gate: Option<&'static str>,
}

/// Type-check `args` against `sig`.
///
/// `pos_base` is the index into `sig.params` that the FIRST positional arg
/// maps to (0 normally). A receiver whose object is pre-inserted into `args`
/// as its own leading `CallArg` still passes `pos_base = 0` when that object
/// also occupies `sig.params[0]` — `pos_base` only shifts when a positional
/// arg maps to a later param than its index in `args` would otherwise
/// suggest. Arity messages report counts relative to `pos_base` (i.e. against
/// the params actually available to positional args), so `pos_base = 0`
/// reproduces the original per-call arg-check wording exactly.
///
/// `check_arity` gates the WS011 positional-count check. User mod/chip calls
/// already run their own arity check (WS022, in `type_user_symbol_call`)
/// before reaching here, so they pass `false` to avoid double-reporting a
/// mismatched call under two different codes; every other caller (builtins,
/// receivers) passes `true` and keeps WS011 exactly as before.
#[allow(clippy::too_many_arguments)]
pub fn check_args(
    ctx: &mut TypeCheckCtx,
    sig: &CallSignature,
    args: &[CallArg],
    pos_base: usize,
    check_arity: bool,
    range: &SourceRange,
) {
    let positional: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            _ => None,
        })
        .collect();

    if check_arity {
        let avail = sig.params.len().saturating_sub(pos_base);
        let required_count = sig.params[pos_base..]
            .iter()
            .filter(|p| !p.optional)
            .count();
        if positional.len() > avail {
            ctx.emit(
                "WS011",
                format!(
                    "'{}' expects at most {} positional arg{}, got {}",
                    sig.name,
                    avail,
                    if avail == 1 { "" } else { "s" },
                    positional.len(),
                ),
                range.clone(),
            );
        } else if positional.len() < required_count {
            ctx.emit(
                "WS011",
                format!(
                    "'{}' requires {} arg{}, got {}",
                    sig.name,
                    required_count,
                    if required_count == 1 { "" } else { "s" },
                    positional.len(),
                ),
                range.clone(),
            );
        }
    }

    for (i, arg_expr) in positional.iter().enumerate() {
        let idx = pos_base + i;
        if idx >= sig.params.len() {
            break;
        }
        check_one_arg(ctx, sig, &sig.params[idx], arg_expr);
    }

    // Named args: config params validate against the schema (enum member,
    // composite shape, or constant scalar); a named arg bound to a real
    // wire-input param is type-checked against the param type with the same
    // coerce the positional path applies. A named arg that matches no
    // declared param falls back to the gate's data-driven settings-menu
    // config (a raw struct-field name, e.g. `bOnlyHitPlayerBodyParts = true`).
    // A named arg that matches NEITHER is unknown: nothing lowers it (both
    // typecheck and emit silently drop it), so it does nothing — the classic
    // `p.DisplayText(t, positionX = 0.0)` typo that quietly no-ops. Flag it as
    // WS041, except for two never-a-param names that are handled elsewhere:
    //   - `exec`   the universal exec-override for a pure-context call (never a
    //              declared param; intercepted before param binding).
    //   - any name on a variadic call that opted out of arity checking
    //     (`check_arity == false`, e.g. `arr.sortMultiple(other, descending =
    //     true)`), whose fixed `Param` list can't enumerate its legal names.
    for a in args {
        let CallArg::Named {
            name,
            value,
            name_range,
        } = a
        else {
            continue;
        };
        if let Some(param) = sig.params.iter().find(|p| &p.name == name) {
            check_one_arg(ctx, sig, param, value);
        } else if let Some(gate_class) = sig.config_gate
            && let Some(cfg) = crate::catalog::scalar_config_field(gate_class, name)
        {
            validate_data_driven_config(ctx, gate_class, cfg, value);
        } else if check_arity && name != "exec" {
            ctx.emit(
                "WS041",
                format!("'{}' has no parameter '{}'", sig.name, name),
                name_range.clone(),
            );
        }
    }
}

/// Validate one argument expression against the `Param` it binds to,
/// dispatching on `ParamKind`. Shared by `check_args`'s positional and
/// named-arg loops.
fn check_one_arg(ctx: &mut TypeCheckCtx, sig: &CallSignature, param: &Param, arg_expr: &Expr) {
    match param.kind {
        ParamKind::ConfigEnum(enum_type) => {
            validate_enum_config_arg(ctx, enum_type, arg_expr);
        }
        ParamKind::ConfigComposite(port) => {
            validate_composite_config_arg(
                ctx,
                sig.config_gate.unwrap_or(""),
                &param.name,
                port,
                arg_expr,
            );
        }
        ParamKind::ConfigScalar(port) => {
            validate_scalar_config_arg(
                ctx,
                sig.config_gate.unwrap_or(""),
                &param.name,
                port,
                arg_expr,
            );
        }
        ParamKind::Wire => {
            // A param whose type still carries a `Type::Param` is an
            // uninferable generic — left to the caller's own WS033
            // inference diagnostics, not this concrete coercion check (see
            // `crate::typecheck::type_has_param`). Builtin/receiver params
            // never contain `Type::Param`, so this is a no-op for them.
            if crate::typecheck::type_has_param(&param.ty) {
                return;
            }
            // The Call-arm preamble already inferred every wire arg into
            // ctx.type_of_expr (via infer::infer) in THIS pass, immediately before
            // check_args runs — read that back instead of re-inferring, so an arg
            // with its own error (e.g. an undefined ident) reports it once, not
            // twice. Fall back to inferring for the rare arg the preamble didn't
            // visit (e.g. a builtin receiver's object, prepended after the preamble).
            // Safe w.r.t. "don't memoize type_of_expr across re-visits": the preamble
            // re-runs and overwrites every pass, so within this pass the entry is
            // this-pass-fresh; we only skip the redundant SECOND inference.
            let key = {
                let r = arg_expr.range();
                (r.file.clone(), r.start.offset, r.end.offset)
            };
            let raw = match ctx.type_of_expr.get(&key).cloned() {
                Some(t) => t,
                None => crate::typecheck::infer::infer(ctx, arg_expr),
            };
            let arg_ty = unwrap_ref(&raw);
            // Unwrap a leading `Ref` off the param type too: a `*T` param
            // resolves to `Ref(T)`, but (like `arg_ty` above) the argument
            // itself infers to its already-auto-derefed inner type, so the
            // ref annotation must not participate in the coercion. Builtin/
            // receiver params are never `Ref`-typed, so this is a no-op for
            // them (mirrors `type_user_symbol_call`'s former inline check).
            let param_ty = unwrap_ref(&param.ty);
            if coerce(&arg_ty, &param_ty) == CoerceRule::Mismatch {
                ctx.emit(
                    "WS003",
                    format!(
                        "argument '{}': expected {}, got {}",
                        param.name,
                        crate::analysis::types::type_str(&param_ty),
                        crate::analysis::types::type_str(&arg_ty),
                    ),
                    arg_expr.range().clone(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
