//! Lowering typed property access: `Environment.Sky.TimeOfDay` reads through a
//! Get Environment Property gate and `Environment.Sky.TimeOfDay = t` writes
//! through a Set Environment Property gate, each with the environment and
//! property baked in as the gate's settings. The path resolves through
//! `typecheck::props`, which already reported any problem with it.

use super::*;
use crate::typecheck::props::{self, PropertyError, ResolvedProperty};

/// Whether the name `Environment` means the builtin here: a binding of the
/// program's own by that name wins.
fn environment_unshadowed(ctx: &LowerCtx) -> bool {
    ctx.scope.get(props::ENVIRONMENT).is_none()
}

/// The resolved property `e` reads or writes, when it is an environment
/// property path the program does not shadow.
///
/// `None` means `e` is not an environment property path; an error, that it is
/// but names nothing (typecheck reported it, and nothing is lowered for it).
fn environment_property(ctx: &LowerCtx, e: &Expr) -> Option<Result<ResolvedProperty, PropertyError>> {
    let (env, property) = props::environment_path(e)?;
    if !environment_unshadowed(ctx) {
        return None;
    }
    Some(props::resolve_environment(env, property))
}

/// The read gate's `Value` port type, and the value a program reads off it.
/// An enum property's wire carries the enum itself: it compares equal to its
/// number but reads as 0 in arithmetic and prints as its label, so the read
/// goes through a Convert gate to `int`, which gives the number (measured in
/// game: `as-probes/06_followup.ws`).
fn read_port_type(r: &ResolvedProperty) -> Type {
    if r.enum_type.is_some() { Type::Any } else { r.ty.clone() }
}

fn read_value(ctx: &mut LowerCtx, r: &ResolvedProperty, value: PortRef, range: &SourceRange) -> PortRef {
    if r.enum_type.is_some() {
        crate::lower::convert::convert_port(ctx, value, &Type::Any, &Type::Int, range)
    } else {
        value
    }
}

fn settings(r: &ResolvedProperty) -> HashMap<crate::intern::Sym, Literal> {
    let mut out = HashMap::default();
    out.insert(intern_static("Environment"), Literal::String(r.owner.to_string()));
    out.insert(intern_static("PropertyName"), Literal::String(r.name.to_string()));
    out
}

/// A read of `Environment.<env>.<property>`, run on the exec chain. `None` when
/// `e` is not one.
pub(super) fn lower_environment_read(ctx: &mut LowerCtx, e: &Expr) -> Option<PortRef> {
    let Ok(r) = environment_property(ctx, e)? else {
        return Some(synthesise_unsupported_no_warn(ctx, e.range()));
    };
    let Some(exec) = ctx.current_exec else {
        return Some(synthesise_unsupported_range(ctx, "an environment read with no exec chain to run on", e.range()));
    };
    let node = chain_gate(
        ctx,
        AddNodeOpts {
            gate_class: gc::GET_ENVIRONMENT_PROPERTY,
            source_range: e.range().clone(),
            ports: GateIO {
                inputs: vec![PortSpec::exec(*sym::EXEC)],
                outputs: vec![PortSpec::exec(*sym::EXEC_OUT), PortSpec::new(*sym::VALUE, read_port_type(&r))],
            },
            properties: settings(&r),
            note: Some("environment read"),
            ..Default::default()
        },
        exec,
        &[],
    );
    Some(read_value(ctx, &r, node.port(WirePort::Value), e.range()))
}

/// `Environment.<env>.<property> = value`, run on the exec chain. Returns
/// whether `s` was one.
pub(super) fn lower_environment_write(ctx: &mut LowerCtx, s: &Assign) -> bool {
    let Some(r) = environment_property(ctx, &s.target) else {
        return false;
    };
    let Ok(r) = r else {
        return true;
    };
    // A constant member of the property's enum is written as its number.
    let value = match r.enum_type.and_then(|e| props::enum_member_of(e, &s.value)) {
        Some(n) => literal_node_range(ctx, s.value.range(), Type::Int, Literal::Int(n)),
        None => lower_expr(ctx, &s.value),
    };
    let Some(exec) = ctx.current_exec else {
        synthesise_unsupported_range(ctx, "an environment write with no exec chain to run on", &s.range);
        return true;
    };
    chain_gate(
        ctx,
        AddNodeOpts {
            gate_class: gc::SET_ENVIRONMENT_PROPERTY,
            source_range: s.range.clone(),
            ports: GateIO {
                inputs: vec![PortSpec::exec(*sym::EXEC), PortSpec::new(*sym::VALUE, r.ty.clone())],
                outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
            },
            properties: settings(&r),
            note: Some("environment write"),
            ..Default::default()
        },
        exec,
        &[(value, WirePort::Value)],
    );
    true
}

/// The component settings of a Get/Set Component Property gate: the component
/// type, as the external asset a save stores it as, and the property's name.
fn component_settings(r: &ResolvedProperty) -> HashMap<crate::intern::Sym, Literal> {
    let mut out = HashMap::default();
    out.insert(
        intern_static("ComponentType"),
        Literal::Asset { asset_type: "BrickComponentTypeBase".to_string(), asset_name: r.owner.to_string() },
    );
    out.insert(intern_static("PropertyName"), Literal::String(r.name.to_string()));
    out
}

/// The component class `obj` holds a reference to, if it is a component.
fn component_class(ctx: &LowerCtx, obj: &Expr) -> Option<String> {
    match crate::types::mono::unwrap_ref(&ctx.type_of(obj)) {
        Type::Component(class) => Some(class),
        _ => None,
    }
}

/// `obj.field` read off a component reference, run on the exec chain. `None`
/// when `obj` is not a component.
pub(super) fn lower_component_read(ctx: &mut LowerCtx, obj: &Expr, field: &str, e: &Expr) -> Option<PortRef> {
    let class = component_class(ctx, obj)?;
    let Ok(r) = props::resolve_component(&class, field) else {
        return Some(synthesise_unsupported_no_warn(ctx, e.range()));
    };
    let component = lower_expr(ctx, obj);
    let Some(exec) = ctx.current_exec else {
        return Some(synthesise_unsupported_range(ctx, "a component read with no exec chain to run on", e.range()));
    };
    let node = chain_gate(
        ctx,
        AddNodeOpts {
            gate_class: gc::GET_COMPONENT_PROPERTY,
            source_range: e.range().clone(),
            ports: GateIO {
                inputs: vec![
                    PortSpec::exec(*sym::EXEC),
                    PortSpec::new(WirePort::Component.sym(), Type::Component(class)),
                ],
                outputs: vec![PortSpec::exec(*sym::EXEC_OUT), PortSpec::new(*sym::VALUE, read_port_type(&r))],
            },
            properties: component_settings(&r),
            note: Some("component read"),
            ..Default::default()
        },
        exec,
        &[(component, WirePort::Component)],
    );
    Some(read_value(ctx, &r, node.port(WirePort::Value), e.range()))
}

/// `obj.field = value` on a component reference, run on the exec chain.
/// Returns whether `s` was one.
pub(super) fn lower_component_write(ctx: &mut LowerCtx, s: &Assign) -> bool {
    let Expr::FieldAccess { obj, field, .. } = &s.target else { return false };
    let Some(class) = component_class(ctx, obj) else { return false };
    let Ok(r) = props::resolve_component(&class, field) else {
        return true;
    };
    let component = lower_expr(ctx, obj);
    let value = match r.enum_type.and_then(|e| props::enum_member_of(e, &s.value)) {
        Some(n) => literal_node_range(ctx, s.value.range(), Type::Int, Literal::Int(n)),
        None => lower_expr(ctx, &s.value),
    };
    let Some(exec) = ctx.current_exec else {
        synthesise_unsupported_range(ctx, "a component write with no exec chain to run on", &s.range);
        return true;
    };
    chain_gate(
        ctx,
        AddNodeOpts {
            gate_class: gc::SET_COMPONENT_PROPERTY,
            source_range: s.range.clone(),
            ports: GateIO {
                inputs: vec![
                    PortSpec::exec(*sym::EXEC),
                    PortSpec::new(WirePort::Component.sym(), Type::Component(class)),
                    PortSpec::new(*sym::VALUE, r.ty.clone()),
                ],
                outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
            },
            properties: component_settings(&r),
            note: Some("component write"),
            ..Default::default()
        },
        exec,
        &[(component, WirePort::Component), (value, WirePort::Value)],
    );
    true
}
