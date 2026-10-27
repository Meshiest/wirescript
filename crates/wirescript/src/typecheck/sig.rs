//! Call-argument checking.
//!
//! `CallSignature` and `Param` describe a callable shape (a builtin call, a
//! receiver method, handler config) apart from where it came from, and
//! `check_args` checks a call's `CallArg`s against it: arity, per-parameter
//! coercion, and the config-only parameters (enum member names, composite
//! constant folds, constant-only scalars, and the data-driven settings-menu
//! attributes from the gate's inventory `config`). Every call form goes through
//! it: builtins and receivers via `sig_of_callspec`, user mod, chip and
//! self-receiver calls via `sig_of_fnchip` (both in `call.rs`).

use crate::analysis::types::type_str;
use crate::ast::{CallArg, CallArgs, Expr, RecordLitField};
use crate::const_scope::Admit;
use crate::diagnostic::{Severity, SourceRange};
use crate::ir::Type;
use crate::types::coerce::{CoerceRule, coerce};
use crate::types::mono::unwrap_ref;
pub(crate) use crate::types::signature::{CallSignature, Param, ParamKind};

use super::{
    SymbolKind, TypeCheckCtx, infer, is_enum_constant_arg, type_has_param,
    validate_composite_config_arg, validate_data_driven_config, validate_enum_config_arg,
    validate_scalar_config_arg,
};

/// One positional argument slot: a real argument expression, or a single element
/// spliced in from a `...tuple` spread (which carries only its type).
enum PosSlot<'a> {
    Arg(&'a Expr),
    Elem(Type, SourceRange),
}

/// Check `args` against `sig`. Positional arguments fill `sig.params` from the
/// first; a receiver call passes its object as the leading `CallArg`.
/// `check_arity` gates the WS011 count check: user mod and chip calls already
/// report arity as WS022 (`type_user_symbol_call`) and pass `false`.
/// `check_named` gates WS041 for unknown named arguments separately: user calls
/// pass `true`; only a variadic whose fixed `Param` list cannot name its legal
/// arguments (`arr.sortMultiple(other, descending = true)`) passes `false` for both.
pub(crate) fn check_args(
    ctx: &mut TypeCheckCtx<'_>,
    sig: &CallSignature,
    args: &[CallArg],
    check_arity: bool,
    check_named: bool,
    range: &SourceRange,
) {
    // Positional slots, with a `...tuple` spread expanded to one slot per element
    // (an element slot carries only its type, with no expression to check). A
    // spread of a non-tuple, or of an unresolved `any`, is reported or ignored here.
    let mut slots: Vec<PosSlot<'_>> = Vec::new();
    for a in args {
        match a {
            CallArg::Positional(e) => slots.push(PosSlot::Arg(e)),
            // A tuple/record LITERAL spread: check each field VALUE as a real
            // argument (matches how lowering expands it).
            CallArg::Spread(Expr::RecordLit { fields, .. }) => {
                for f in fields {
                    if let RecordLitField::Named { value, .. } = f {
                        slots.push(PosSlot::Arg(value));
                    }
                }
            }
            CallArg::Spread(t) => {
                match unwrap_ref(&ctx.type_or_infer(t)) {
                    Type::Tuple(elems) => {
                        for el in elems {
                            slots.push(PosSlot::Elem(el, t.range().clone()));
                        }
                    }
                    // A multi-output result splats in declaration order.
                    Type::Record(fields) | Type::Outputs(fields) => {
                        for (_, ft) in fields {
                            slots.push(PosSlot::Elem(ft, t.range().clone()));
                        }
                    }
                    // Unresolved: do not cascade an error.
                    Type::Any => {}
                    other => ctx.emit(
                        "WS003",
                        format!("spread `...` expects a tuple, got {}", type_str(&other)),
                        t.range().clone(),
                    ),
                }
            }
            CallArg::Named { .. } => {}
        }
    }

    if check_arity {
        let avail = sig.params.len();
        // A required parameter supplied by name (`Sweep(o, d, distance = 100.0)`) is
        // satisfied and is not demanded positionally too.
        let named: Vec<&str> = args
            .iter()
            .filter_map(|a| match a {
                CallArg::Named { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let required_count = sig
            .params
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

    for (slot, param) in slots.iter().zip(&sig.params) {
        match slot {
            PosSlot::Arg(arg_expr) => check_one_arg(ctx, sig, param, arg_expr),
            PosSlot::Elem(ty, r) => {
                // A spread element has only a type; coerce it against the wire
                // param directly (config params can't be spread-filled).
                if matches!(
                    param.kind,
                    ParamKind::Wire | ParamKind::WireEnum(_) | ParamKind::Const
                ) && !type_has_param(&param.ty)
                    && coerce(&unwrap_ref(ty), &unwrap_ref(&param.ty)) == CoerceRule::Mismatch
                {
                    ctx.emit(
                        "WS003",
                        format!(
                            "spread element for '{}': expected {}, got {}",
                            param.name,
                            type_str(&unwrap_ref(&param.ty)),
                            type_str(&unwrap_ref(ty)),
                        ),
                        r.clone(),
                    );
                }
            }
        }
    }

    // Named arguments: a config parameter validates against the schema (an enum
    // member, a composite shape, a constant scalar); one bound to a wire-input
    // parameter is coerced as a positional one is. A name matching no declared
    // parameter falls back to the gate's settings-menu config (a raw struct field,
    // `bOnlyHitPlayerBodyParts = true`). A name matching neither lowers to nothing
    // (`p.DisplayText(t, positionX = 0.0)`), so it is WS041, except for:
    //   - `exec`, the exec override of a pure-context call, handled before binding;
    //   - any name on a variadic call with `check_arity == false`
    //     (`arr.sortMultiple(other, descending = true)`), whose fixed list cannot
    //     name its legal arguments.
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
            // Point at a parameter that differs only by case: matching is case-sensitive,
            // so a casing slip otherwise reads as "no such parameter".
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

    if sig.config_gate == Some(crate::ir::gate_class::SET_BRICK_PROPERTY) {
        check_brick_property_value(ctx, sig, args);
    }
}

const BRICK_PROPERTY: &str = "EBRBrickProperty";

/// The `BrickProperty` member a constant `property` argument names: bare
/// (`Color`), qualified (`BrickProperty.Color`), quoted, or its integer.
pub(super) fn brick_property_member(property: &Expr) -> Option<i64> {
    const ENUM: &str = BRICK_PROPERTY;
    match property {
        Expr::Ident { name, .. } => crate::catalog::enum_member_value(ENUM, name),
        Expr::StringLit { value, .. } => crate::catalog::enum_member_value(ENUM, value),
        Expr::IntLit { value, .. } => Some(*value),
        Expr::FieldAccess { obj, field, .. } => match obj.as_ref() {
            Expr::Ident { name, .. }
                if crate::catalog::game_enum_schema_type(name) == Some(ENUM) =>
            {
                crate::catalog::builtin_game_enums()
                    .into_iter()
                    .find(|e| e.schema_type == ENUM)
                    .and_then(|e| e.variants.into_iter().find(|v| &v.clean_name == field))
                    .map(|v| v.disc)
            }
            _ => None,
        },
        _ => None,
    }
}

/// The value a brick property reads and writes: `bool` for Visible and the
/// collision flags, `color` for Color. `None` for Material and
/// `MaterialOpacity`, whose wire type is not known.
pub(super) fn brick_property_type(member: i64) -> Option<Type> {
    match member {
        0 | 4..=11 => Some(Type::Bool),
        1 => Some(Type::Color),
        _ => None,
    }
}

/// `SetBrickProperty`'s value takes the type of the setting it writes (see
/// [`brick_property_type`]). The setting is a constant, so the mismatch is
/// caught here rather than written to a brick that silently ignores it.
fn check_brick_property_value(ctx: &mut TypeCheckCtx<'_>, sig: &CallSignature, args: &[CallArg]) {
    const ENUM: &str = BRICK_PROPERTY;
    if args.has_spread() {
        return;
    }
    let arg_for = |name: &str| -> Option<&Expr> {
        args.named(name).or_else(|| {
            let idx = sig.params.iter().position(|p| p.name == name)?;
            args.positionals().nth(idx)
        })
    };
    let (Some(property), Some(value)) = (arg_for("property"), arg_for("value")) else {
        return;
    };
    let Some(member) = brick_property_member(property) else {
        return;
    };
    let Some(want) = brick_property_type(member) else {
        return;
    };
    let got = unwrap_ref(&ctx.type_or_infer(value));
    if coerce(&got, &want) == CoerceRule::Mismatch {
        let setting = crate::catalog::enum_member_names(ENUM)
            .into_iter()
            .find(|n| crate::catalog::enum_member_value(ENUM, n) == Some(member))
            .unwrap_or_default();
        ctx.emit(
            "WS003",
            format!(
                "brick property {setting} takes a {}, got {}",
                type_str(&want),
                type_str(&got),
            ),
            value.range().clone(),
        );
    }
}

/// Validate one argument expression against the `Param` it binds to,
/// dispatching on `ParamKind`. Shared by `check_args`'s positional and
/// named-arg loops.
fn check_one_arg(ctx: &mut TypeCheckCtx<'_>, sig: &CallSignature, param: &Param, arg_expr: &Expr) {
    match param.kind {
        ParamKind::ConfigEnum(enum_type) => {
            validate_enum_config_arg(
                ctx,
                enum_type,
                &format!("`{}` setting `{}`", sig.name, param.name),
                arg_expr,
            );
        }
        ParamKind::WireEnum(enum_type) => {
            if is_enum_constant_arg(ctx, enum_type, arg_expr) {
                validate_enum_config_arg(
                    ctx,
                    enum_type,
                    &format!("`{}` setting `{}`", sig.name, param.name),
                    arg_expr,
                );
            } else if !wire_arg_is_game_enum(ctx, enum_type, arg_expr) {
                check_wire_arg(ctx, arg_expr, param);
            }
        }
        ParamKind::ConfigComposite(port) => {
            validate_composite_config_arg(ctx, &param.name, port, arg_expr);
        }
        ParamKind::ConfigScalar(_) => validate_scalar_config_arg(ctx, &param.name, arg_expr),
        ParamKind::Wire => check_wire_arg(ctx, arg_expr, param),
        ParamKind::Const => {
            // A const value still has a type, so the ordinary wire check
            // applies (this also covers `g("not an int")` against a
            // `const int` param with the usual WS003, rather than that being
            // silently subsumed by the constant check below).
            check_wire_arg(ctx, arg_expr, param);
            if let Err(err) = ctx.const_eval(arg_expr, Admit::All)
                && !err.reported_elsewhere()
            {
                ctx.emit(err.code(), err.message(), err.range.clone());
            }
        }
    }
}

/// A record literal passed where the parameter has ref fields (the shape `*T`
/// on a record distributes to): each ref field's value needs storage or a ref
/// behind it, the test a scalar `*T` argument faces (a var names storage, a ref
/// value such as `&a`, `hs[0]` or a ref-holding var names its target). Only
/// literals are checked; a named record's fields came from elsewhere.
fn check_record_lit_ref_fields(ctx: &mut TypeCheckCtx<'_>, param: &Param, arg_expr: &Expr) {
    let (Type::Record(pfields), Expr::RecordLit { fields, .. }) = (&param.ty, arg_expr) else {
        return;
    };
    for f in fields {
        let (name, value, frange) = match f {
            RecordLitField::Named { name, value, range } => (name.as_str(), Some(value), range),
            RecordLitField::Shorthand { name, range } => (name.as_str(), None, range),
            RecordLitField::Spread { .. } => continue,
        };
        if !pfields
            .iter()
            .any(|(pn, pt)| pn == name && matches!(pt, Type::Ref(_)))
        {
            continue;
        }
        // Shorthand `{ counter }` names the ident directly.
        let ident = match value {
            None => Some(name),
            Some(Expr::Ident { name, .. }) => Some(name.as_str()),
            _ => None,
        };
        let ref_value = match value {
            Some(Expr::RefOf { .. }) => true,
            Some(v) => matches!(ctx.type_or_infer(v), Type::Ref(_)),
            None => false,
        };
        let writable = ref_value
            || match ident {
                Some(n) => ctx.scope.lookup(n).is_none_or(|sym| {
                    matches!(
                        sym.kind,
                        SymbolKind::Var | SymbolKind::Array | SymbolKind::Map
                    ) || (sym.kind == SymbolKind::Param && matches!(&sym.ty, Type::Ref(_)))
                }),
                None => matches!(value, Some(Expr::FieldAccess { .. })),
            };
        if !writable {
            ctx.emit(
                "WS008",
                format!(
                    "field `{name}` of ref parameter '{}' needs a variable behind \
                     it: a literal, an `in` port, or an expression has no storage \
                     to write back to",
                    param.name
                ),
                frange.clone(),
            );
        }
    }
}

/// A wire-enum argument that is a value of the built-in game enum backing the
/// field (`justify = DisplayTextJustification.Center`, or a variable of that
/// enum). Reads the type the Call-arm preamble recorded, like
/// [`check_wire_arg`].
fn wire_arg_is_game_enum(ctx: &mut TypeCheckCtx<'_>, enum_type: &str, arg_expr: &Expr) -> bool {
    let ty = ctx.type_or_infer(arg_expr);
    matches!(
        unwrap_ref(&ty),
        Type::Enum { name, .. } if crate::catalog::game_enum_schema_type(&name) == Some(enum_type)
    )
}

/// Whether a field chain passed to a `*T` parameter names storage it can alias:
/// a field of a record `var` (each field is a gate), a namespaced var, a ref
/// field of a record parameter, or a field of a record `let` bundling vars
/// (`projection_is_place`). A chain that arrived on a wire (a literal's `n: 1`,
/// a map read, a call result, `pts[i].x`) has nothing to write back to; a ref
/// value is bound by the ref-value check before this.
pub(super) fn field_names_storage(ctx: &TypeCheckCtx<'_>, arg_expr: &Expr) -> bool {
    let (root, segs) = infer::flatten_access(arg_expr);
    let Expr::Ident { name, range } = root else {
        return false;
    };
    let Some(sym) = ctx.scope.lookup(name) else {
        return true;
    };
    match sym.kind {
        SymbolKind::Var | SymbolKind::Array | SymbolKind::Map => true,
        SymbolKind::LetBinding => infer::projection_is_place(ctx, arg_expr),
        SymbolKind::Namespace => segs.first().is_some_and(|member| {
            ctx.ns_members(name, &range.file)
                .and_then(|m| m.get(member.as_str()))
                .is_some_and(|info| match info.kind {
                    SymbolKind::Var | SymbolKind::Array | SymbolKind::Map => true,
                    SymbolKind::LetBinding => infer::projection_is_place(ctx, arg_expr),
                    _ => false,
                })
        }),
        SymbolKind::Param => {
            let mut t = sym.ty.clone();
            for seg in &segs {
                t = match &t {
                    Type::Record(fields) => match fields.iter().find(|(k, _)| k == seg) {
                        Some((_, ft)) => ft.clone(),
                        None => return false,
                    },
                    Type::Tuple(fields) => {
                        match seg.parse::<usize>().ok().and_then(|i| fields.get(i)) {
                            Some(ft) => ft.clone(),
                            None => return false,
                        }
                    }
                    Type::Ref(_) if seg == "Value" || seg == "prev" => t.clone(),
                    _ => return false,
                };
            }
            matches!(t, Type::Ref(_))
        }
        _ => false,
    }
}

/// The ordinary wire-argument check shared by `ParamKind::Wire` and
/// `ParamKind::Const`: infer `arg_expr`'s type and `coerce` it against
/// `param.ty` (WS003 on mismatch).
fn check_wire_arg(ctx: &mut TypeCheckCtx<'_>, arg_expr: &Expr, param: &Param) {
    // A parameter type still carrying a `Type::Param` is an uninferable generic,
    // left to the caller's WS033 (see `crate::typecheck::type_has_param`).
    // Builtin and receiver parameters never contain one.
    if type_has_param(&param.ty) {
        return;
    }
    // A `*T` or `ref T` parameter captures a whole variable by reference, so its
    // argument must denote one. A literal, expression, call, `arr[i]` element or
    // scalar `let` has no ref pin, and the inline binder would drop the call, so it
    // is rejected; `&x` is checked separately (WS008), and a record field chain
    // must name storage (`field_names_storage`). An array element is excluded: the
    // game has only a whole-array ref pin.
    check_record_lit_ref_fields(ctx, param, arg_expr);
    // A ref value (a `*T` held in a var or container, event data, `&x` as a
    // value) is never read through implicitly: it binds a `*T` parameter of its
    // exact type and otherwise only an `any` (`Type::Any`/`Type::Opaque`, a
    // pass-through: custom-event data ports, `Opaque(...)`/`Change(...)`
    // rerouters, message ports, as `"${h}"` interpolation is). `&x` into a `*T`
    // parameter is the static alias form, checked below.
    if !matches!(arg_expr, Expr::NullLit { .. }) {
        let raw = ctx.type_or_infer(arg_expr);
        // A `*Record` parameter distributes its ref-ness onto each field
        // (`distribute_ref`), so it never shows up as a top-level `Type::Ref`
        // the way a scalar/container `*T` does - `&r` aliasing one is still
        // the static form, just recorded per field instead of at the top.
        let is_ref_shaped_param = matches!(param.ty, Type::Ref(_))
            || matches!(&param.ty, Type::Record(fields) if fields.iter().any(|(_, t)| matches!(t, Type::Ref(_))));
        let static_alias = matches!(arg_expr, Expr::RefOf { .. }) && is_ref_shaped_param;
        if matches!(raw, Type::Ref(_)) && !static_alias {
            match &param.ty {
                Type::Any | Type::Opaque => {}
                want if coerce(&raw, want) == CoerceRule::Same => {}
                want => {
                    let hint = if matches!(want, Type::Ref(_)) {
                        ""
                    } else {
                        "; read through the ref with `*`"
                    };
                    ctx.emit(
                        "WS003",
                        format!(
                            "argument '{}': expected {want}, got {raw}{hint}",
                            param.name
                        ),
                        arg_expr.range().clone(),
                    );
                }
            }
            return;
        }
    }
    if matches!(&param.ty, Type::Ref(_)) && !matches!(arg_expr, Expr::RefOf { .. }) {
        let ok = match arg_expr {
            Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
                Some(s) => {
                    matches!(
                        s.kind,
                        SymbolKind::Var | SymbolKind::Array | SymbolKind::Map
                    ) || (s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)))
                        || (s.kind == SymbolKind::In && matches!(unwrap_ref(&s.ty), Type::Array(_)))
                        || (s.kind == SymbolKind::LetBinding
                            && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)))
                }
                // Undefined ident: reported as WS002 elsewhere, do not pile on.
                None => true,
            },
            Expr::FieldAccess { .. } | Expr::TuplePick { .. } => field_names_storage(ctx, arg_expr),
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
        infer::check(ctx, arg_expr, &param.ty);
        return;
    }
    // `{}` for a map parameter is an empty map, which the callee gets as a
    // map of its own, the way a constant array literal argument is passed.
    if let Expr::RecordLit { fields, range } = arg_expr
        && fields.is_empty()
        && let map @ Type::Map(..) = unwrap_ref(&param.ty)
    {
        ctx.record_expr_type(range, &map);
        return;
    }
    // The call preamble already inferred every wire argument into
    // `ctx.type_of_expr` in this pass, so it is read back rather than inferred
    // again, and an argument's own error is reported once. An argument the
    // preamble did not visit (a builtin receiver's object) is inferred here. The
    // preamble re-runs every pass, so the entry is this pass's.
    let raw = ctx.type_or_infer(arg_expr);
    let arg_ty = unwrap_ref(&raw);
    // Unwrap a leading `Ref` off the param type too: a `*T` param
    // resolves to `Ref(T)`, but (like `arg_ty` above) the argument
    // itself infers to its already-auto-derefed inner type, so the
    // ref annotation must not participate in the coercion. Builtin/
    // receiver params are never `Ref`-typed, so this is a no-op for
    // them.
    let param_ty = unwrap_ref(&param.ty);
    // An argument that produces no value (a void mutation, a `mod`/`chip` with
    // no output) has no wire to pass, whatever the param type - including an
    // `any` param, which `coerce` would wave through. Same WS072 sink every
    // other value position uses.
    if matches!(arg_ty, Type::Never) && !matches!(param_ty, Type::Never) {
        crate::typecheck::let_binding::emit_no_value(ctx, None, arg_expr.range());
        return;
    }
    // A `*T` parameter aliases the caller's STORAGE, so it is invariant. Every
    // other argument rides a wire, and a wire can carry a conversion; a ref
    // carries none - the callee writes its own type straight into the gate the
    // caller declared, so a `*string` bound to an `int` var puts a string in
    // int storage. Checked against the variable's DECLARED type rather than the
    // inferred argument type, which has already auto-dereferenced.
    if let Type::Ref(want) = &param.ty
        && is_concrete_storage_type(want)
        && let Some(have) = storage_inner_type(ctx, arg_expr)
        && have != **want
    {
        ctx.emit(
            "WS003",
            format!(
                "argument '{}': `*{}` aliases the caller's storage, so it needs a \
                 `{}` variable; `{}` storage cannot be written through it",
                param.name,
                type_str(want),
                type_str(want),
                type_str(&have),
            ),
            arg_expr.range().clone(),
        );
        return;
    }
    if infer::record_into_one_value(ctx, arg_expr, &arg_ty, &param_ty) {
        if matches!(arg_ty, Type::Record(_)) {
            let r = arg_expr.range();
            ctx.resolutions.rejected_record_args.insert(r.key());
        }
        return;
    }
    // A generic enum value built in the argument (`Opt.Some(7)` for an
    // `Opt<float>` parameter) is built at the parameter's instantiation, as an
    // annotated `let` or `var` builds it. Kept only when that checks cleanly.
    if let (Type::Enum { name: pn, args: pa }, Type::Enum { name: an, args: aa }) =
        (&param_ty, &arg_ty)
        && pn == an
        && !pa.is_empty()
        && pa != aa
    {
        let before = ctx.diagnostics.len();
        infer::check(ctx, arg_expr, &param_ty);
        let (old, new) = ctx.diagnostics.split_at(before);
        if !new.iter().any(|d| d.severity == Severity::Error) {
            let repeats: Vec<bool> = new
                .iter()
                .map(|d| {
                    old.iter()
                        .any(|o| o.code == d.code && o.message == d.message && o.range == d.range)
                })
                .collect();
            let mut i = 0;
            ctx.diagnostics.retain(|_| {
                i += 1;
                i <= before || !repeats[i - before - 1]
            });
            return;
        }
        ctx.diagnostics.truncate(before);
    }
    infer::literal_in_slot(ctx, arg_expr, &arg_ty, &param_ty);
    if coerce(&arg_ty, &param_ty) == CoerceRule::Mismatch
        && infer::literal_fits(ctx, arg_expr, &param_ty)
    {
        infer::coerce_value_or_emit(ctx, arg_expr, &arg_ty, &param_ty);
        return;
    }
    if coerce(&arg_ty, &param_ty) == CoerceRule::Mismatch && !ctx.is_error_typed(arg_expr) {
        ctx.emit(
            "WS003",
            format!(
                "argument '{}': expected {}, got {}",
                param.name,
                type_str(&param_ty),
                type_str(&arg_ty),
            ),
            arg_expr.range().clone(),
        );
    }
}

/// Whether a `*T` parameter's `T` is concrete enough to hold a caller's storage
/// to. A generic parameter, `any`, or an `Opaque` probe is satisfied by any
/// variable, so those keep the ordinary coercion path.
fn is_concrete_storage_type(t: &Type) -> bool {
    !matches!(t, Type::Param(_) | Type::Any | Type::Opaque)
}

/// The DECLARED inner type of the storage an argument names, for an argument
/// that is a variable (bare, or written `&x` / `ref x`). `None` for anything
/// that is a value on a wire, which coerces normally.
fn storage_inner_type(ctx: &TypeCheckCtx<'_>, e: &Expr) -> Option<Type> {
    let target = match e {
        Expr::RefOf { operand, .. } => operand.as_ref(),
        other => other,
    };
    let Expr::Ident { name, .. } = target else {
        return None;
    };
    let sym = ctx.scope.lookup(name)?;
    if sym.kind != SymbolKind::Var {
        return None;
    }
    match &sym.ty {
        Type::Ref(inner) => Some(inner.as_ref().clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
