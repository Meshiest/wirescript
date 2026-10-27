//! Constant-only gate config validation (WS028).

use super::*;
use crate::catalog::calls::{CallParam, CallSpec};
use crate::const_eval::ConstReason;
use crate::const_scope::Admit;
use crate::ir::Literal;

/// The English name of a constant literal kind with no scalar config form, or
/// `None` for every kind that has one. Being constant is not enough for a
/// constant-only scalar slot: a record, array or map is a compile-time value
/// with no scalar to write into the gate's data field. At emit an array would
/// default to a zero or empty value, and a record would be refused as
/// `EmitError::UnrepresentableLiteral`, so every scalar-config check rejects
/// these kinds (WS028). The composite path (`validate_composite_config_arg`,
/// for `MeshColors` and `WeaponAmmoOverride`) takes arrays and records and does
/// not use this.
pub(super) fn non_scalar_config_kind(lit: &Literal) -> Option<&'static str> {
    match lit {
        Literal::Record(_) => Some("a record"),
        Literal::Array(_) => Some("an array"),
        Literal::Map(_) => Some("a map"),
        _ => None,
    }
}

/// The schema enum type of a config (non-wire, data-only) param, if any.
/// A param is data-only when its port is not a wire input on the gate; such a
/// param backed by a schema enum field takes a bare member name or an int.
pub(super) fn call_param_config_enum(spec: &CallSpec, param: &CallParam) -> Option<&'static str> {
    let port = param.port.as_str();
    if crate::catalog::is_wire_input(spec.gate_class, port) {
        return None;
    }
    crate::catalog::config_field_enum_type(spec.gate_class, port)
}

/// The schema enum type of a WIRE param whose gate data field is also a schema
/// enum (`DisplayText`'s `justify`/`easing`). A constant enum form bakes into the
/// data field exactly like enum config; any other value wires like an ordinary
/// input.
pub(super) fn call_param_wire_enum(spec: &CallSpec, param: &CallParam) -> Option<&'static str> {
    let port = param.port.as_str();
    if !crate::catalog::is_wire_input(spec.gate_class, port) {
        return None;
    }
    crate::catalog::config_field_enum_type(spec.gate_class, port)
}

/// A bare identifier naming a member of `enum_type`: an enum member name, not
/// a variable, so it is never type-inferred as one.
pub(super) fn is_bare_enum_member(enum_type: &str, e: &Expr) -> bool {
    matches!(e, Expr::Ident { name, .. } if crate::catalog::enum_member_value(enum_type, name).is_some())
}

/// Whether a wire-enum argument is written as an enum constant, so it bakes
/// into the gate's data field instead of wiring: a member name (bare or
/// quoted), an int literal, or a constant naming a member.
pub(super) fn is_enum_constant_arg(ctx: &TypeCheckCtx<'_>, enum_type: &str, e: &Expr) -> bool {
    match e {
        Expr::StringLit { .. } | Expr::IntLit { .. } => true,
        Expr::Ident { .. } => {
            is_bare_enum_member(enum_type, e)
                || matches!(
                    crate::const_eval::expr_to_literal_in(e, &ctx.const_lookup()),
                    Some(Literal::String(s))
                        if crate::catalog::enum_member_value(enum_type, &s).is_some()
                )
        }
        _ => false,
    }
}

/// Validate an argument bound to an enum-typed config param against the
/// schema's member list. A bare identifier is an enum member (not a variable);
/// an int is range-checked; anything else is rejected (config is constant-only).
/// `setting` names the argument as a diagnostic quotes it.
pub(super) fn validate_enum_config_arg(
    ctx: &mut TypeCheckCtx<'_>,
    enum_type: &str,
    setting: &str,
    e: &Expr,
) {
    match e {
        Expr::Ident { name, range } => {
            if crate::catalog::enum_member_value(enum_type, name).is_none() {
                // Not a member name: try the constant environment before erroring, so
                // `const EASE = "Bounce"` resolves as a quoted member name would. The member
                // reading comes first (above).
                match crate::const_eval::expr_to_literal_in(e, &ctx.const_lookup()) {
                    Some(Literal::String(s))
                        if crate::catalog::enum_member_value(enum_type, &s).is_some() => {}
                    Some(Literal::Int(v)) if crate::catalog::enum_has_value(enum_type, v) => {}
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
                    format!("{setting} cannot be {value}: it is not a valid {enum_type} value; expected one of: {members}"),
                    range.clone(),
                );
            }
        }
        // The quoted-name form (`function = "Bounce"`), validated the same way.
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
        // A qualified built-in enum value (`function = EasingFunction.Bounce`), or any
        // value whose type is a built-in game enum: accepted when its enum backs this
        // field's schema type, WS003 for another game enum; a non-enum falls through
        // to WS028 below. The bare member, int and quoted forms are handled above, so
        // `other` is never a bare member name here (which would misreport WS002).
        other => {
            let ty = crate::typecheck::infer::infer(ctx, other);
            if let crate::ir::Type::Enum { name, .. } = &ty {
                if crate::catalog::game_enum_schema_type(name) == Some(enum_type) {
                    return;
                }
                ctx.emit(
                    "WS003",
                    format!(
                        "expected {}, got {name}",
                        crate::catalog::clean_game_enum_type(enum_type)
                    ),
                    other.range().clone(),
                );
                return;
            }
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
pub(super) fn is_composite_config_param(spec: &CallSpec, param: &CallParam) -> bool {
    let port = param.port.as_str();
    matches!(port, "MeshColors" | "WeaponAmmoOverride")
        && !crate::catalog::is_wire_input(spec.gate_class, port)
}

/// Validate a composite constant-only config argument: it must fold to a
/// constant of the expected shape (the fold lowering uses). `port_name`, the
/// gate port, keys the shape lookup (`MeshColors`, `WeaponAmmoOverride`);
/// `display_name`, the source name, is what WS028 shows (`meshColors` binds the
/// `MeshColors` port).
pub(super) fn validate_composite_config_arg(
    ctx: &mut TypeCheckCtx<'_>,
    display_name: &str,
    port_name: &str,
    e: &Expr,
) {
    let consts = ctx.const_lookup();
    let (ok, hint) = match port_name {
        "MeshColors" => (
            crate::const_eval::fold_mesh_colors(e, &consts).is_some(),
            "a constant array of ColorSRGB(r, g, b, a) values",
        ),
        "WeaponAmmoOverride" => (
            crate::const_eval::fold_ammo_override(e, &consts).is_some(),
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

/// A plain scalar or asset config parameter: a settings-menu field that is
/// neither enum-typed nor composite. Its value bakes into the gate's data with
/// no wire, so it must be constant.
pub(super) fn is_scalar_config_param(spec: &CallSpec, param: &CallParam) -> bool {
    !crate::catalog::is_wire_input(spec.gate_class, param.port.as_str())
        && call_param_config_enum(spec, param).is_none()
        && !is_composite_config_param(spec, param)
}

/// Reject a non-constant value for a scalar or asset config parameter: with no
/// wire pin, a runtime value would lower to a broken wire and the config would
/// never apply. Uses the full evaluator (`const_eval::eval_expr`) over the
/// constant environment, `const mod` calls included (`evtName("died")` as a
/// channel name), so it accepts what config lowering does; a `var` or a call to
/// a non-const mod stays WS028. `display_name` is the source name WS028 shows.
///
/// A `BudgetExceeded` or `Refused` failure is its own code (WS048, WS047), since
/// those arise only from attempting a `const mod` call; every other reason is
/// WS028 (`var_prefab_still_rejected_as_ws028`). A value that evaluates is
/// checked for scalar shape too ([`non_scalar_config_kind`]).
pub(super) fn validate_scalar_config_arg(ctx: &mut TypeCheckCtx<'_>, display_name: &str, e: &Expr) {
    match ctx.const_eval(e, Admit::All) {
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
            ConstReason::BudgetExceeded | ConstReason::Refused(_) => {
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
    ctx: &mut TypeCheckCtx<'_>,
    gate_class: &str,
    cfg: &crate::catalog::ConfigProperty,
    e: &Expr,
) {
    if let Some(enum_type) = crate::catalog::config_field_enum_type(gate_class, &cfg.name) {
        validate_enum_config_arg(ctx, enum_type, &format!("setting `{}`", cfg.name), e);
        return;
    }
    match crate::const_eval::expr_to_literal_in(e, &ctx.const_lookup()) {
        // Constant; its shape is checked too (`non_scalar_config_kind`).
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
