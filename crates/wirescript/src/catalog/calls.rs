//! Built-in call catalog.
//!
//! Maps source-level call names (e.g. `displayText`, `sin`, `vec`) to the
//! concrete gate class they lower to, their port-wiring shape, and whether
//! the call is exec-form (chains into `currentExec`) or pure-expression
//! (returns a value via an output port).
//!
//! hand-authored, so we keep the Rust form structurally identical for
//! easy cross-checking.
//!
//! This file holds the spec types, the shared `CallSpec` constructor helpers,
//! and the lookup entry points. The specs themselves live in one module per
//! domain, each exposing a `register(&mut map)` that `build_calls` chains.

use crate::collections::HashMap;
use std::sync::OnceLock;

use crate::ir::Type;
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;

mod color;
mod entity;
mod flow;
mod gamemode;
mod inventory;
mod math;
mod messaging;
mod player;
mod spawn_events;
mod string;
mod vector;

#[derive(Clone, Debug)]
pub struct CallParam {
    /// Source-level parameter name (used for named-arg form).
    pub name: &'static str,
    /// Port name on the target gate to wire this argument into.
    pub port: WirePort,
    /// Accepted type. `Character` and `Controller` are interchangeable at a
    /// param port — they wire directly into each other in Brickadia, so no
    /// adapter gate is inserted.
    pub ty: Type,
    /// When true, callers may omit the argument; the gate's default stays.
    pub optional: bool,
}

impl CallParam {
    pub const fn req(name: &'static str, port: WirePort, ty: Type) -> Self {
        Self {
            name,
            port,
            ty,
            optional: false,
        }
    }
    pub const fn opt(name: &'static str, port: WirePort, ty: Type) -> Self {
        Self {
            name,
            port,
            ty,
            optional: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CallOutput {
    pub port: WirePort,
    pub ty: Type,
    /// Record field this output binds to when the call returns a record
    /// (e.g. `Edge`'s `rising`). Named outputs make the call result bind as
    /// a field→port record, so field access resolves through the spec
    /// instead of port-name matching. None for single-value outputs.
    pub field: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub struct CallSpec {
    pub name: &'static str,
    pub gate_class: &'static str,
    pub params: Vec<CallParam>,
    /// If true, the call is exec-form: chains into the current exec and
    /// advances it via the gate's `ExecOut`. If false, the call is pure
    /// and `output` identifies the value-producing port.
    pub exec: bool,
    pub outputs: Vec<CallOutput>,
    /// The type of the first param when this call can be used as a receiver
    /// method: `entity.SetLocation(pos)` instead of `SetLocation(entity, pos)`.
    /// None means no receiver form.
    pub receiver: Option<Type>,
}

impl CallSpec {
    /// For a receiver method whose object binds to a NAMED param instead of the
    /// first positional, that param's name. `entity.SendCustomEvent("x", …)`
    /// binds the entity to `target` (the object-event recipient's grid), not the
    /// first param (`eventName`), and leaves the positional args as the channel
    /// name + data values. `None` = the ordinary first-param receiver form
    /// (`entity.SetLocation(pos)`).
    pub fn receiver_target_param(&self) -> Option<&'static str> {
        match self.name {
            "SendCustomEvent" | "SendGlobalCustomEvent" => Some("target"),
            _ => None,
        }
    }
}

fn math_unary(name: &'static str, gate_class: &'static str) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params: vec![CallParam::req("x", WirePort::Input, Type::Float)],
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: WirePort::Output,
            ty: Type::Float,
        }],
        receiver: None,
    }
}

fn vec_expr(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    out_port: WirePort,
    out_ty: Type,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: out_port,
            ty: out_ty,
        }],
        receiver: None,
    }
}

/// A pure gate returning a record: one named output per field. The spec's
/// return type is the record derived from `fields`, and lowering binds the
/// result as a field→port record (no port-name matching).
fn vec_expr_record(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    fields: Vec<(&'static str, WirePort, Type)>,
) -> CallSpec {
    let record_ty = Type::Record(
        fields
            .iter()
            .map(|(f, _, ty)| (f.to_string(), ty.clone()))
            .collect(),
    );
    let mut outputs: Vec<CallOutput> = fields
        .into_iter()
        .map(|(f, port, ty)| CallOutput {
            field: Some(f),
            port,
            ty,
        })
        .collect();
    // The first output doubles as the call's primary value; it carries the
    // record type so a bare (non-field) use typechecks as the record.
    if let Some(first) = outputs.first_mut() {
        first.ty = record_ty;
    }
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs,
        receiver: None,
    }
}

fn vec_recv(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    out_port: WirePort,
    out_ty: Type,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: out_port,
            ty: out_ty,
        }],
        receiver: Some(Type::Vector),
    }
}

/// The value types a math-variant port accepts, matching the game's
/// `WireGraphPrimMathVariant` (`f64, i64, Vector, Rotator, Quat, LinearColor`).
/// `Blend`/`lerp`/`Tween` interpolate any of these, not just floats.
fn blend_variant() -> Type {
    Type::Union(vec![
        Type::Float,
        Type::Int,
        Type::Vector,
        Type::Rotator,
        Type::Quat,
        Type::Color,
    ])
}

/// Pure (non-exec) expression gate whose first param is the method receiver.
fn expr_recv(
    name: &'static str,
    gate_class: &'static str,
    receiver: Type,
    params: Vec<CallParam>,
    out_port: WirePort,
    out_ty: Type,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: false,
        outputs: vec![CallOutput {
            field: None,
            port: out_port,
            ty: out_ty,
        }],
        receiver: Some(receiver),
    }
}

fn entity_exec(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    outputs: Vec<CallOutput>,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: true,
        outputs,
        receiver: Some(Type::Entity),
    }
}

fn controller_exec(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    outputs: Vec<CallOutput>,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: true,
        outputs,
        receiver: Some(Type::Controller),
    }
}

fn character_exec(
    name: &'static str,
    gate_class: &'static str,
    params: Vec<CallParam>,
    outputs: Vec<CallOutput>,
) -> CallSpec {
    CallSpec {
        name,
        gate_class,
        params,
        exec: true,
        outputs,
        receiver: Some(Type::Character),
    }
}

fn build_calls() -> HashMap<&'static str, CallSpec> {
    let mut m: HashMap<&'static str, CallSpec> = HashMap::default();

    color::register(&mut m);
    entity::register(&mut m);
    flow::register(&mut m);
    gamemode::register(&mut m);
    inventory::register(&mut m);
    math::register(&mut m);
    messaging::register(&mut m);
    player::register(&mut m);
    spawn_events::register(&mut m);
    string::register(&mut m);
    vector::register(&mut m);

    m
}

pub fn calls() -> &'static HashMap<&'static str, CallSpec> {
    static INSTANCE: OnceLock<HashMap<&'static str, CallSpec>> = OnceLock::new();
    INSTANCE.get_or_init(build_calls)
}

pub fn find_call(name: &str) -> Option<&'static CallSpec> {
    calls().get(name)
}

#[cfg(test)]
mod tests;
