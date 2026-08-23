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
//! checker. All call forms go through it: builtins and receivers via
//! `sig_of_callspec`, user mod/chip + self-receiver calls via
//! `sig_of_fnchip` (both adapters live in `typecheck.rs`).

use crate::ast::{CallArg, Expr};
use crate::diagnostic::SourceRange;
use crate::ir::Type;
use crate::types::coerce::{CoerceRule, coerce};
use crate::types::mono::unwrap_ref;

use super::{
    SymbolKind, TypeCheckCtx, validate_composite_config_arg, validate_data_driven_config,
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
    /// A user `const` parameter (`name: const string`, or any parameter of a
    /// `const mod`). The argument must evaluate at compile time; it is ALSO
    /// type-checked exactly like a `Wire` param, since a const value still has
    /// a type.
    Const,
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
/// receivers) passes `true`.
///
/// `check_named` gates the WS041 unknown-named-arg check independently: user
/// mod/chip calls DO know their full param list, so they pass `true` (catching
/// `g(1, bogus = 5)`) even though `check_arity` is `false`. Only true variadics
/// whose fixed `Param` list can't enumerate their legal names (e.g.
/// `arr.sortMultiple(other, descending = true)`) pass `false` for both.
#[allow(clippy::too_many_arguments)]
/// One positional argument slot: a real argument expression, or a single element
/// spliced in from a `...tuple` spread (which carries only its type).
enum PosSlot<'a> {
    Arg(&'a Expr),
    Elem(Type, SourceRange),
}

pub fn check_args(
    ctx: &mut TypeCheckCtx,
    sig: &CallSignature,
    args: &[CallArg],
    pos_base: usize,
    check_arity: bool,
    check_named: bool,
    range: &SourceRange,
) {
    // Positional slots, with a `...tuple` spread expanded to one slot per
    // element (an ELEMENT slot carries just its type — there is no per-element
    // expression to run the full argument check on). A spread of a non-tuple, or
    // of a still-unresolved `any`, is reported/ignored here.
    let mut slots: Vec<PosSlot> = Vec::new();
    for a in args {
        match a {
            CallArg::Positional(e) => slots.push(PosSlot::Arg(e)),
            // A tuple/record LITERAL spread: check each field VALUE as a real
            // argument (matches how lowering expands it).
            CallArg::Spread(Expr::RecordLit { fields, .. }) => {
                for f in fields {
                    if let crate::ast::RecordLitField::Named { value, .. } = f {
                        slots.push(PosSlot::Arg(value));
                    }
                }
            }
            CallArg::Spread(t) => {
                let key = {
                    let r = t.range();
                    (r.file.clone(), r.start.offset, r.end.offset)
                };
                let tt = ctx
                    .type_of_expr
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| crate::typecheck::infer::infer(ctx, t));
                match unwrap_ref(&tt) {
                    Type::Tuple(elems) => {
                        for el in elems {
                            slots.push(PosSlot::Elem(el, t.range().clone()));
                        }
                    }
                    // A multi-output result splats in declaration order.
                    Type::Record(fields) => {
                        for (_, ft) in fields {
                            slots.push(PosSlot::Elem(ft, t.range().clone()));
                        }
                    }
                    // Unresolved — don't cascade a spurious error.
                    Type::Any => {}
                    other => ctx.emit(
                        "WS003",
                        format!(
                            "spread `...` expects a tuple, got {}",
                            crate::analysis::types::type_str(&other)
                        ),
                        t.range().clone(),
                    ),
                }
            }
            CallArg::Named { .. } => {}
        }
    }

    if check_arity {
        let avail = sig.params.len().saturating_sub(pos_base);
        // A required param supplied BY NAME (`Sweep(o, d, distance = 100.0)`) is
        // satisfied, so it must not also be demanded positionally — counting
        // only positionals reported "requires 3 args, got 2" for a complete call.
        let named: Vec<&str> = args
            .iter()
            .filter_map(|a| match a {
                CallArg::Named { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let required_count = sig.params[pos_base..]
            .iter()
            .filter(|p| !p.optional && !named.contains(&p.name.as_str()))
            .count();
        if slots.len() > avail {
            ctx.emit(
                "WS011",
                format!(
                    "'{}' expects at most {} positional arg{}, got {}",
                    sig.name,
                    avail,
                    if avail == 1 { "" } else { "s" },
                    slots.len(),
                ),
                range.clone(),
            );
        } else if slots.len() < required_count {
            ctx.emit(
                "WS011",
                format!(
                    "'{}' requires {} arg{}, got {}",
                    sig.name,
                    required_count,
                    if required_count == 1 { "" } else { "s" },
                    slots.len(),
                ),
                range.clone(),
            );
        }
    }

    for (i, slot) in slots.iter().enumerate() {
        let idx = pos_base + i;
        if idx >= sig.params.len() {
            break;
        }
        match slot {
            PosSlot::Arg(arg_expr) => check_one_arg(ctx, sig, &sig.params[idx], arg_expr),
            PosSlot::Elem(ty, r) => {
                // A spread element has only a type; coerce it against the wire
                // param directly (config params can't be spread-filled).
                let param = &sig.params[idx];
                if matches!(param.kind, ParamKind::Wire | ParamKind::Const)
                    && !crate::typecheck::type_has_param(&param.ty)
                    && coerce(&unwrap_ref(ty), &unwrap_ref(&param.ty)) == CoerceRule::Mismatch
                {
                    ctx.emit(
                        "WS003",
                        format!(
                            "spread element for '{}': expected {}, got {}",
                            param.name,
                            crate::analysis::types::type_str(&unwrap_ref(&param.ty)),
                            crate::analysis::types::type_str(&unwrap_ref(ty)),
                        ),
                        r.clone(),
                    );
                }
            }
        }
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
        } else if check_named && name != "exec" {
            // Point at a param that differs only by case — named-arg matching is
            // case-sensitive, so a casing slip otherwise reads as "no such
            // parameter" for a parameter that plainly exists.
            let suggestion = sig
                .params
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))
                .map(|p| format!(" (did you mean '{}'?)", p.name))
                .unwrap_or_default();
            ctx.emit(
                "WS041",
                format!("'{}' has no parameter '{}'{suggestion}", sig.name, name),
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
        ParamKind::Wire => check_wire_arg(ctx, arg_expr, param),
        ParamKind::Const => {
            // A const value still has a type, so the ordinary wire check
            // applies (this also covers `g("not an int")` against a
            // `const int` param with the usual WS003, rather than that being
            // silently subsumed by the constant check below).
            check_wire_arg(ctx, arg_expr, param);
            let lookup = |n: &str| ctx.resolve_mod(n);
            let mut budget = crate::const_eval::Budget::default();
            let cx = ctx.const_ctx(Some(&lookup));
            if let Err(err) = crate::const_eval::eval_expr(arg_expr, &cx, &mut budget) {
                ctx.emit(err.code(), err.message(), err.range.clone());
            }
        }
    }
}

/// The ordinary wire-argument check shared by `ParamKind::Wire` and
/// `ParamKind::Const`: infer `arg_expr`'s type and `coerce` it against
/// `param.ty` (WS003 on mismatch).
fn check_wire_arg(ctx: &mut TypeCheckCtx, arg_expr: &Expr, param: &Param) {
    // A param whose type still carries a `Type::Param` is an
    // uninferable generic — left to the caller's own WS033
    // inference diagnostics, not this concrete coercion check (see
    // `crate::typecheck::type_has_param`). Builtin/receiver params
    // never contain `Type::Param`, so this is a no-op for them.
    if crate::typecheck::type_has_param(&param.ty) {
        return;
    }
    // A `*T` / `ref T` parameter captures a whole variable by reference, so its
    // argument must denote one. A literal, expression, call, `arr[i]` element,
    // or scalar `let` has no ref pin, and the inline binder silently drops such
    // a call, so reject it here. `&x` is validated separately (WS008 in infer),
    // and a record-field chain is left permissive (the binder resolves it).
    // `arr[i]` is excluded even though it is otherwise ref-able: a scalar
    // ref cannot capture one array element (the game has only a whole-array ref
    // pin).
    if matches!(&param.ty, Type::Ref(_)) && !matches!(arg_expr, Expr::RefOf { .. }) {
        let ok = match arg_expr {
            Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
                Some(s) => {
                    matches!(s.kind, SymbolKind::Var | SymbolKind::Array | SymbolKind::Map)
                        || (s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)))
                        || (s.kind == SymbolKind::In && matches!(unwrap_ref(&s.ty), Type::Array(_)))
                        || (s.kind == SymbolKind::LetBinding
                            && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)))
                }
                // Undefined ident: reported as WS002 elsewhere, do not pile on.
                None => true,
            },
            Expr::FieldAccess { .. } => true,
            _ => false,
        };
        if !ok {
            ctx.emit(
                "WS008",
                format!(
                    "cannot pass a non-lvalue to ref parameter '{}': `*T`/`ref` needs a \
                     variable or ref parameter, not a literal, expression, `let`, `arr[i]`, \
                     or call",
                    param.name
                ),
                arg_expr.range().clone(),
            );
            return;
        }
    }
    // `null` adopts the param's type (resolve + record + coerce via `check`),
    // rather than the `any` a bare inference would give it.
    if matches!(arg_expr, Expr::NullLit { .. }) {
        crate::typecheck::infer::check(ctx, arg_expr, &param.ty);
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
    // them.
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

#[cfg(test)]
mod tests;
