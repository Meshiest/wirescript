//! Constant-only gate config validation (WS028).

use super::*;

/// The English name of a constant literal that has NO scalar config form, or
/// `None` for every kind that does.
///
/// Being constant is necessary but NOT sufficient for a constant-only SCALAR
/// config slot: a record, array or map is a perfectly good compile-time value
/// with no single scalar to write into the gate's data field. Skipping this
/// check lets such a value reach emit, where it is resolved by literal kind in
/// two different wrong ways:
///
///   - an ARRAY silently defaults to a zero/empty value — a program that
///     type-checks clean and bakes config the author never wrote;
///   - a RECORD reaches `literal_to_boxed_native` / `literal_to_string`, which
///     decline it via `EmitError::UnrepresentableLiteral` — a compile error
///     instead of the more precise WS028 this validator gives.
///
/// Rejecting these kinds at every scalar-config validation site closes both at
/// once. The COMPOSITE config path (`validate_composite_config_arg`, for
/// `MeshColors` / `WeaponAmmoOverride`) legitimately takes arrays and records
/// and deliberately does NOT consult this.
pub(super) fn non_scalar_config_kind(lit: &crate::ir::Literal) -> Option<&'static str> {
    match lit {
        crate::ir::Literal::Record(_) => Some("a record"),
        crate::ir::Literal::Array(_) => Some("an array"),
        crate::ir::Literal::Map(_) => Some("a map"),
        _ => None,
    }
}

/// The schema enum type of a config (non-wire, data-only) param, if any.
/// A param is data-only when its port is not a wire input on the gate; such a
/// param backed by a schema enum field takes a bare member name or an int.
pub(super) fn call_param_config_enum(
    spec: &crate::catalog::calls::CallSpec,
    param: &crate::catalog::calls::CallParam,
) -> Option<&'static str> {
    let port = param.port.as_str();
    if crate::catalog::is_wire_input(spec.gate_class, port) {
        return None;
    }
    crate::catalog::config_field_enum_type(spec.gate_class, port)
}

/// Validate an argument bound to an enum-typed config param against the
/// schema's member list. A bare identifier is an enum member (not a variable);
/// an int is range-checked; anything else is rejected (config is constant-only).
pub(super) fn validate_enum_config_arg(ctx: &mut TypeCheckCtx, enum_type: &str, e: &Expr) {
    match e {
        Expr::Ident { name, range } => {
            if crate::catalog::enum_member_value(enum_type, name).is_none() {
                // Not a member name — fall back to the constant environment
                // before erroring, so `const EASE = "Bounce"` resolves the
                // same way a quoted member name would. Enum-member
                // interpretation stays FIRST (above): a program that already
                // relies on `name` being a member (or would be if it existed)
                // keeps that meaning unchanged.
                match crate::lower::expr_to_literal_in(e, &ctx.const_lookup()) {
                    Some(crate::ir::Literal::String(s))
                        if crate::catalog::enum_member_value(enum_type, &s).is_some() => {}
                    Some(crate::ir::Literal::Int(v))
                        if crate::catalog::enum_has_value(enum_type, v) => {}
                    _ => {
                        let members = crate::catalog::enum_member_names(enum_type).join(", ");
                        ctx.emit(
                            "WS028",
                            format!(
                                "unknown enum member '{name}' for {enum_type}; expected one of: {members}"
                            ),
                            range.clone(),
                        );
                    }
                }
            }
        }
        Expr::IntLit { value, range, .. } => {
            if !crate::catalog::enum_has_value(enum_type, *value) {
                let members = crate::catalog::enum_member_names(enum_type).join(", ");
                ctx.emit(
                    "WS028",
                    format!("{value} is not a valid {enum_type} value; expected one of: {members}"),
                    range.clone(),
                );
            }
        }
        // The quoted-name form (`function = "Bounce"`) — validate the same way.
        Expr::StringLit { value, range } => {
            if crate::catalog::enum_member_value(enum_type, value).is_none() {
                let members = crate::catalog::enum_member_names(enum_type).join(", ");
                ctx.emit(
                    "WS028",
                    format!("unknown enum member \"{value}\" for {enum_type}; expected one of: {members}"),
                    range.clone(),
                );
            }
        }
        other => {
            ctx.emit(
                "WS028",
                format!("{enum_type} config must be a constant enum member name or int"),
                other.range().clone(),
            );
        }
    }
}

/// Composite constant-only config params (`meshColors: Color[]`,
/// `ammoOverride`: the `WeaponAmmoOverride` nested struct) that fold into gate
/// data rather than wiring. `true` when this param is one and targets a
/// non-wire data field on its gate.
pub(super) fn is_composite_config_param(
    spec: &crate::catalog::calls::CallSpec,
    param: &crate::catalog::calls::CallParam,
) -> bool {
    let port = param.port.as_str();
    matches!(port, "MeshColors" | "WeaponAmmoOverride")
        && !crate::catalog::is_wire_input(spec.gate_class, port)
}

/// Validate a composite constant-only config argument. It must fold to a
/// constant of the expected shape (the same fold the lowering uses); a
/// non-constant or malformed value is rejected here rather than becoming a
/// silent broken gate.
///
/// Takes `gate_class` + `display_name` + `port_name` (rather than a
/// `&CallParam`) so the `sig::check_args` port can call it from a bare `Param`
/// (which doesn't carry a `CallParam::port`). `port_name` — the gate PORT name
/// — keys the composite-shape lookup ("MeshColors"/"WeaponAmmoOverride");
/// `display_name` — the source-level surface name — is what the WS028 message
/// shows the author (these differ, e.g. surface `meshColors` binds the
/// `MeshColors` port). `gate_class` isn't used by the composite check itself
/// (kept for signature symmetry with `validate_data_driven_config`).
pub(super) fn validate_composite_config_arg(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    display_name: &str,
    port_name: &str,
    e: &Expr,
) {
    let _ = gate_class;
    let consts = ctx.const_lookup();
    let (ok, hint) = match port_name {
        "MeshColors" => (
            crate::lower::fold_mesh_colors(e, &consts).is_some(),
            "a constant array of ColorSRGB(r, g, b, a) values",
        ),
        "WeaponAmmoOverride" => (
            crate::lower::fold_ammo_override(e, &consts).is_some(),
            "a constant record { overrideStartingAmmo: bool, resources: [{ loaded: int, reserve: int }] }",
        ),
        _ => (true, ""),
    };
    if !ok {
        ctx.emit(
            "WS028",
            format!("'{display_name}' config must be {hint}"),
            e.range().clone(),
        );
    }
}

/// A plain scalar/asset config param — a non-wire settings-menu field that is
/// neither enum-typed nor a composite (meshColors/ammoOverride). Its value
/// bakes into the gate's data and cannot be wired, so it must be a constant.
pub(super) fn is_scalar_config_param(
    spec: &crate::catalog::calls::CallSpec,
    param: &crate::catalog::calls::CallParam,
) -> bool {
    !crate::catalog::is_wire_input(spec.gate_class, param.port.as_str())
        && call_param_config_enum(spec, param).is_none()
        && !is_composite_config_param(spec, param)
}

/// Reject a non-constant value for a scalar/asset config param — it has no wire
/// pin, so a variable or computed value would otherwise lower to a broken wire
/// (a silent "Failed to connect wire" at load) with the config never applied.
/// Uses the full const evaluator (`const_eval::eval_expr`, against the
/// scoped-or-top-level constant environment PLUS a `const mod` call — e.g.
/// `SendCustomEvent`'s channel name, `evtName("died")`), so this accepts
/// everything the config lowering path does; a genuine `var` or a call to a
/// non-const mod still errors and stays WS028.
///
/// Takes `gate_class` + `display_name` + `port_name` (rather than a
/// `&CallParam`) so the `sig::check_args` port can call it from a bare `Param`.
/// `display_name` — the source-level surface name — is what the WS028 message
/// shows; `gate_class`/`port_name` are unused by the constant check itself
/// (kept for signature symmetry with `validate_composite_config_arg` /
/// `validate_data_driven_config`, and for any future per-port scalar rule).
///
/// A `BudgetExceeded`/`Refused` failure is surfaced as ITS OWN code (WS048 /
/// WS047) instead of the generic WS028 below — those only exist because a
/// `const mod` call was attempted at all (a call chain too deep, or a
/// certified evaluator declining the operands), so naming it precisely is
/// more useful than the generic "not a constant" wording. Every other reason
/// keeps the ORIGINAL WS028 wording — existing tests (e.g.
/// `var_prefab_still_rejected_as_ws028`) pin that a plain runtime `var` here
/// stays WS028, not WS046.
///
/// A successfully-evaluated value is additionally checked for scalar SHAPE via
/// [`non_scalar_config_kind`] — see its doc comment for why being constant is
/// not sufficient here.
pub(super) fn validate_scalar_config_arg(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    display_name: &str,
    port_name: &str,
    e: &Expr,
) {
    let _ = (gate_class, port_name);
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    let cx = ctx.const_ctx(Some(&lookup));
    match crate::const_eval::eval_expr(e, &cx, &mut budget) {
        Ok(lit) => {
            if let Some(kind) = non_scalar_config_kind(&lit) {
                ctx.emit(
                    "WS028",
                    format!(
                        "'{display_name}' is constant-only gate config and takes a single scalar value, not {kind}"
                    ),
                    e.range().clone(),
                );
            }
        }
        Err(err) => match err.reason {
            crate::const_eval::ConstReason::BudgetExceeded
            | crate::const_eval::ConstReason::Refused(_) => {
                ctx.emit(err.code(), err.message(), err.range.clone());
            }
            _ => {
                ctx.emit(
                    "WS028",
                    format!(
                        "'{display_name}' is constant-only gate config and cannot take a variable or computed value"
                    ),
                    e.range().clone(),
                );
            }
        },
    }
}

/// Validate a data-driven config attribute (a settings-menu config field set via
/// `<FieldName> = value`, resolved from the inventory `config` array). Enum
/// fields validate their member against the schema; other scalars must be
/// compile-time constants.
pub(super) fn validate_data_driven_config(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    cfg: &crate::catalog::ConfigProperty,
    e: &Expr,
) {
    if let Some(enum_type) = crate::catalog::config_field_enum_type(gate_class, &cfg.name) {
        validate_enum_config_arg(ctx, enum_type, e);
        return;
    }
    match crate::lower::expr_to_literal_in(e, &ctx.const_lookup()) {
        // Constant, but check its SHAPE too — see `non_scalar_config_kind`.
        Some(lit) => {
            if let Some(kind) = non_scalar_config_kind(&lit) {
                ctx.emit(
                    "WS028",
                    format!(
                        "'{}' is constant-only gate config and takes a single scalar value, not {kind}",
                        cfg.name
                    ),
                    e.range().clone(),
                );
            }
        }
        None => ctx.emit(
            "WS028",
            format!(
                "'{}' is constant-only gate config and cannot take a variable or computed value",
                cfg.name
            ),
            e.range().clone(),
        ),
    }
}
