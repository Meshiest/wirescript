//! Constant-only gate config validation (WS028).

use super::*;

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
    let (ok, hint) = match port_name {
        "MeshColors" => (
            crate::lower::fold_mesh_colors(e).is_some(),
            "a constant array of ColorSRGB(r, g, b, a) values",
        ),
        "WeaponAmmoOverride" => (
            crate::lower::fold_ammo_override(e).is_some(),
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
/// Uses the same fold check (`expr_to_literal_in`, against the scoped-or-top-level
/// constant environment) the config lowering path uses — so a `let`-bound
/// constant (body-local or top-level) resolves here too, while a genuine `var`
/// or computed value still folds to `None` and stays WS028.
///
/// Takes `gate_class` + `display_name` + `port_name` (rather than a
/// `&CallParam`) so the `sig::check_args` port can call it from a bare `Param`.
/// `display_name` — the source-level surface name — is what the WS028 message
/// shows; `gate_class`/`port_name` are unused by the constant check itself
/// (kept for signature symmetry with `validate_composite_config_arg` /
/// `validate_data_driven_config`, and for any future per-port scalar rule).
pub(super) fn validate_scalar_config_arg(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    display_name: &str,
    port_name: &str,
    e: &Expr,
) {
    let _ = (gate_class, port_name);
    if crate::lower::expr_to_literal_in(e, &ctx.const_lookup()).is_none() {
        ctx.emit(
            "WS028",
            format!(
                "'{display_name}' is constant-only gate config and cannot take a variable or computed value"
            ),
            e.range().clone(),
        );
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
    } else if crate::lower::expr_to_literal_in(e, &ctx.const_lookup()).is_none() {
        ctx.emit(
            "WS028",
            format!(
                "'{}' is constant-only gate config and cannot take a variable or computed value",
                cfg.name
            ),
            e.range().clone(),
        );
    }
}
