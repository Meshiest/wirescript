//! Typed property access: `Environment.Sky.TimeOfDay` reads and writes an
//! environment setting through the Get/Set Environment Property gates, typed
//! from the property map (`catalog::properties`). Lowering reads the same
//! resolution (`resolve_environment`), so a path typecheck accepts is the one
//! lowering emits.

use super::*;
use crate::catalog::properties::{self, PropertyType};

/// The name a program reads environment settings through.
pub(crate) const ENVIRONMENT: &str = "Environment";

/// `Environment.<env>.<property>`, if `e` spells one: the environment's name
/// and the property's.
pub(crate) fn environment_path(e: &Expr) -> Option<(&str, &str)> {
    let Expr::FieldAccess { obj, field, .. } = e else {
        return None;
    };
    let Expr::FieldAccess {
        obj: base,
        field: env,
        ..
    } = obj.as_ref()
    else {
        return None;
    };
    let Expr::Ident { name, .. } = base.as_ref() else {
        return None;
    };
    (name == ENVIRONMENT).then_some((env.as_str(), field.as_str()))
}

/// A property a program reads or writes: what the gate stores to name it, and
/// the type its value has.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedProperty {
    /// The owner's stored option (`Sky`, `Component_PointLight`).
    pub owner: &'static str,
    /// The stored property name.
    pub name: &'static str,
    /// The value's type. An enum property reads and writes as `int`.
    pub ty: Type,
    /// The schema enum of an enum property, whose constant members a write
    /// also accepts.
    pub enum_type: Option<&'static str>,
}

pub(crate) enum PropertyError {
    /// No such environment, component or property (WS110).
    Unknown(String),
    /// The property exists but has a type a program cannot carry (WS003).
    Unsupported(String),
}

/// The environment a program names `label` (the game's picker label without
/// spaces: `Sky`, `Ground`, `Lighting`).
pub(crate) fn environment_by_label(label: &str) -> Option<&'static properties::Environment> {
    properties::property_map()
        .environments
        .iter()
        .find(|e| squash(&e.label) == label)
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// The environment names a program can write, in the map's order.
pub(crate) fn environment_names() -> Vec<String> {
    properties::property_map()
        .environments
        .iter()
        .map(|e| squash(&e.label))
        .collect()
}

/// Resolve `Environment.<env>.<property>`.
pub(crate) fn resolve_environment(
    env: &str,
    property: &str,
) -> Result<ResolvedProperty, PropertyError> {
    let Some(e) = environment_by_label(env) else {
        return Err(PropertyError::Unknown(format!(
            "`{ENVIRONMENT}.{env}` names no environment; there are {}",
            environment_names().join(", ")
        )));
    };
    let Some(p) = e.property(property) else {
        let names: Vec<&str> = e.properties.iter().map(|p| p.name.as_str()).collect();
        return Err(PropertyError::Unknown(format!(
            "`{ENVIRONMENT}.{env}` has no property `{property}`; it has {}",
            names.join(", ")
        )));
    };
    resolved(&e.option, p, &format!("{ENVIRONMENT}.{env}.{property}"))
}

fn resolved(
    owner: &'static str,
    p: &'static properties::Property,
    shown: &str,
) -> Result<ResolvedProperty, PropertyError> {
    match properties::property_type(p) {
        PropertyType::Value(ty) => Ok(ResolvedProperty {
            owner,
            name: &p.name,
            ty,
            enum_type: None,
        }),
        PropertyType::Enum(_) => Ok(ResolvedProperty {
            owner,
            name: &p.name,
            ty: Type::Int,
            enum_type: p.enum_type.as_deref(),
        }),
        PropertyType::Unsupported(why) => Err(PropertyError::Unsupported(format!(
            "`{shown}` is {why}, so a program can't read or write it"
        ))),
    }
}

/// Whether the name `Environment` means the builtin here: a binding of the
/// program's own by that name wins.
fn environment_unshadowed(ctx: &TypeCheckCtx) -> bool {
    ctx.scope.lookup(ENVIRONMENT).is_none()
}

fn report(ctx: &mut TypeCheckCtx, err: PropertyError, range: &SourceRange) {
    match err {
        PropertyError::Unknown(m) => ctx.emit("WS110", m, range.clone()),
        PropertyError::Unsupported(m) => ctx.emit("WS003", m, range.clone()),
    }
}

/// Type a read of `e` when it is an environment property path (`None`
/// otherwise). A read runs the Get Environment Property gate, so it needs an
/// exec context.
pub(super) fn infer_environment_read(ctx: &mut TypeCheckCtx, e: &Expr) -> Option<Type> {
    let (env, property) = environment_path(e)?;
    if !environment_unshadowed(ctx) {
        return None;
    }
    if ctx.exec_mode() != ExecMode::Exec {
        ctx.emit(
            "WS007",
            format!("environment read '{ENVIRONMENT}.{env}.{property}' outside an exec context"),
            e.range().clone(),
        );
    }
    Some(match resolve_environment(env, property) {
        Ok(r) => r.ty,
        Err(err) => {
            report(ctx, err, e.range());
            Type::Any
        }
    })
}

/// `Environment.<env>` with no property: name one.
pub(super) fn environment_without_property(ctx: &mut TypeCheckCtx, e: &Expr) -> bool {
    let Expr::FieldAccess { obj, field, range } = e else {
        return false;
    };
    if !matches!(obj.as_ref(), Expr::Ident { name, .. } if name == ENVIRONMENT)
        || !environment_unshadowed(ctx)
    {
        return false;
    }
    ctx.emit(
        "WS110",
        format!(
            "`{ENVIRONMENT}.{field}` needs a property to read: `{ENVIRONMENT}.{field}.<property>`"
        ),
        range.clone(),
    );
    true
}

/// Check `target = value` when the target is an environment property path,
/// returning whether it was one. The value must fit the property's type; an
/// enum property also takes a constant member of its enum.
pub(super) fn check_property_write(ctx: &mut TypeCheckCtx, target: &Expr, value: &Expr) -> bool {
    let Some((env, property)) = environment_path(target).filter(|_| environment_unshadowed(ctx))
    else {
        return check_component_write(ctx, target, value);
    };
    match resolve_environment(env, property) {
        Ok(r) => check_written_value(ctx, &r, value),
        Err(err) => {
            report(ctx, err, target.range());
            infer::infer(ctx, value);
        }
    }
    true
}

fn check_written_value(ctx: &mut TypeCheckCtx, r: &ResolvedProperty, value: &Expr) {
    if let Some(e) = r.enum_type
        && enum_member_of(e, value).is_some()
    {
        return;
    }
    infer::check_stored_value(ctx, value, &r.ty);
}

/// The discriminant of `value` when it is a constant member of schema enum
/// `enum_type`: bare (`Cave`) or qualified (`ReverbEffect.Cave`).
pub(crate) fn enum_member_of(enum_type: &str, value: &Expr) -> Option<i64> {
    match value {
        Expr::Ident { name, .. } => crate::catalog::enum_member_value(enum_type, name),
        Expr::FieldAccess { obj, field, .. } => match obj.as_ref() {
            Expr::Ident { name, .. }
                if crate::catalog::game_enum_schema_type(name) == Some(enum_type) =>
            {
                crate::catalog::builtin_game_enums()
                    .into_iter()
                    .find(|e| e.schema_type == enum_type)
                    .and_then(|e| e.variants.into_iter().find(|v| &v.clean_name == field))
                    .map(|v| v.disc)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Resolve property `property` of a component of type `class`.
pub(crate) fn resolve_component(
    class: &str,
    property: &str,
) -> Result<ResolvedProperty, PropertyError> {
    let name = crate::catalog::component_name(class);
    let Some(c) = properties::component(class) else {
        return Err(PropertyError::Unknown(format!(
            "`{name}` is not a component type the component gates take"
        )));
    };
    let Some(p) = c.property(property) else {
        let names: Vec<&str> = c.properties.iter().map(|p| p.name.as_str()).collect();
        return Err(PropertyError::Unknown(format!(
            "a `{name}` component has no property `{property}`; it has {}",
            names.join(", ")
        )));
    };
    resolved(&c.option, p, &format!("{name}.{property}"))
}

/// Type `obj.field` read off a component reference of type `class`. A read
/// runs the Get Component Property gate, so it needs an exec context.
pub(super) fn infer_component_read(
    ctx: &mut TypeCheckCtx,
    class: &str,
    field: &str,
    range: &SourceRange,
) -> Type {
    if ctx.exec_mode() != ExecMode::Exec {
        ctx.emit(
            "WS007",
            format!("component property read '.{field}' outside an exec context"),
            range.clone(),
        );
    }
    match resolve_component(class, field) {
        Ok(r) => r.ty,
        Err(err) => {
            report(ctx, err, range);
            Type::Any
        }
    }
}

/// Check `obj.field = value` when `obj` is a component reference, returning
/// whether it was one. The receiver is typed here; for any other receiver its
/// diagnostics are dropped so the ordinary assignment check reports them once.
pub(super) fn check_component_write(ctx: &mut TypeCheckCtx, target: &Expr, value: &Expr) -> bool {
    let Expr::FieldAccess { obj, field, range } = target else {
        return false;
    };
    let mark = ctx.diagnostics.len();
    let Type::Component(class) = crate::types::mono::unwrap_ref(&infer::infer(ctx, obj)) else {
        ctx.diagnostics.truncate(mark);
        return false;
    };
    match resolve_component(&class, field) {
        Ok(r) => check_written_value(ctx, &r, value),
        Err(err) => {
            report(ctx, err, range);
            infer::infer(ctx, value);
        }
    }
    true
}

/// The component class a `$Component/<name>` argument names, for Get/Add
/// Component. `None` for anything else.
pub(crate) fn component_type_arg(e: &Expr) -> Option<&'static str> {
    let Expr::AssetRef {
        asset_type,
        asset_name,
        ..
    } = e
    else {
        return None;
    };
    if asset_type != crate::analysis::assets::COMPONENT {
        return None;
    }
    crate::catalog::component_type(asset_name).map(|o| o.class)
}
