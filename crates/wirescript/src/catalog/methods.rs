//! The lowering half of a container method's table entry
//! ([`super::arrays::ArrayMethod`], [`super::maps::MapMethod`]): which source
//! argument feeds which gate input, and which output the call returns.
//!
//! One description serves both sides. Typecheck reads the arguments as the
//! method's parameters ([`MethodArg::param`]); lowering binds the call's
//! arguments to them and builds the gate. The gate's outputs are not listed:
//! they come from the gate catalog ([`gate_outputs`]).

use crate::ir::Type;
use crate::ir::port_registry::WirePort;
use crate::typecheck::ContainerKind;

/// The type of a value argument, written against the receiver's own types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgTy {
    /// The array's element type.
    Elem,
    /// The map's key type.
    Key,
    /// The map's value type.
    MapValue,
    Int,
    Bool,
    String,
    Entity,
    /// A `zone` rerouter reference; on the wire it is the entity object it
    /// carries.
    Zone,
}

impl ArgTy {
    /// The source-level type of this argument on a receiver of `kind`.
    pub fn ty(self, kind: &ContainerKind) -> Type {
        match (self, kind) {
            (ArgTy::Elem, ContainerKind::Array { elem }) => elem.clone(),
            (ArgTy::Key, ContainerKind::Map { key, .. }) => key.clone(),
            (ArgTy::MapValue, ContainerKind::Map { value, .. }) => value.clone(),
            (ArgTy::Elem | ArgTy::Key | ArgTy::MapValue, _) => Type::Any,
            (ArgTy::Int, _) => Type::Int,
            (ArgTy::Bool, _) => Type::Bool,
            (ArgTy::String, _) => Type::String,
            (ArgTy::Entity, _) => Type::Entity,
            (ArgTy::Zone, _) => Type::Zone,
        }
    }

    /// The type the gate input this argument feeds is declared with. An array
    /// gate's element slot is the catalog's generic `any`. A map's `Key` and
    /// `Value` carry the map's concrete types, because a port's declared type
    /// sets the default the game bakes into the field, and an `any` key bakes a
    /// float the game rejects on a string-keyed map.
    pub fn port_type(self, kind: &ContainerKind) -> Type {
        match self {
            ArgTy::Elem => Type::Any,
            ArgTy::Zone => Type::Entity,
            other => other.ty(kind),
        }
    }
}

/// How an argument is lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    /// A value, lowered as an expression.
    Value(ArgTy),
    /// Another array of the given element type, wired by its ref.
    ArrayRef(ArgTy),
    /// Another map of the receiver's key and value types, wired by its ref.
    MapRef,
    /// An array of the given element type the method fills, wired by its ref.
    /// Being written, it cannot be a `const` container.
    DestArray(ArgTy),
}

/// One argument of a container method: its source name (matched positionally
/// or by name), the gate input it feeds, and how it is lowered.
#[derive(Clone, Copy, Debug)]
pub struct MethodArg {
    pub name: &'static str,
    pub port: WirePort,
    pub kind: ArgKind,
    pub optional: bool,
}

impl MethodArg {
    /// The typecheck parameter for this argument on a receiver of `kind`.
    pub fn param(&self, kind: &ContainerKind) -> crate::typecheck::Param {
        let ty = match self.kind {
            ArgKind::Value(t) => t.ty(kind),
            ArgKind::ArrayRef(t) | ArgKind::DestArray(t) => Type::Array(Box::new(t.ty(kind))),
            ArgKind::MapRef => kind.as_type(),
        };
        crate::typecheck::Param {
            name: self.name.to_string(),
            ty,
            optional: self.optional,
            kind: crate::typecheck::ParamKind::Wire,
        }
    }

    /// The type the gate input is declared with on a receiver of `kind`.
    pub fn port_type(&self, kind: &ContainerKind) -> Type {
        match self.kind {
            ArgKind::Value(t) => t.port_type(kind),
            ArgKind::ArrayRef(_) | ArgKind::DestArray(_) => Type::Array(Box::new(Type::Any)),
            ArgKind::MapRef => Type::Ref(Box::new(Type::Any)),
        }
    }
}

/// A required value argument.
pub const fn val(name: &'static str, port: WirePort, ty: ArgTy) -> MethodArg {
    MethodArg { name, port, kind: ArgKind::Value(ty), optional: false }
}

/// An optional value argument; the gate input stays unwired when it is omitted.
pub const fn opt(name: &'static str, port: WirePort, ty: ArgTy) -> MethodArg {
    MethodArg { name, port, kind: ArgKind::Value(ty), optional: true }
}

/// A required argument of the given kind.
pub const fn arg(name: &'static str, port: WirePort, kind: ArgKind) -> MethodArg {
    MethodArg { name, port, kind, optional: false }
}

/// How a container method lowers.
pub enum Lowering {
    /// One gate: the receiver's ref and exec, plus one input per argument.
    /// `result` is the port the call's value is read from. `port_types`
    /// declares a gate port (input or output) with a type other than the one
    /// the argument or the catalog gives it.
    Generic {
        args: &'static [MethodArg],
        result: WirePort,
        port_types: &'static [(WirePort, Type)],
    },
    /// Lowered by its own function in `lower::access`, for a call whose shape
    /// an argument list cannot describe. Its typecheck parameters are empty.
    Custom,
}

impl Lowering {
    /// The arguments, empty for a `Custom` method.
    pub fn args(&self) -> &'static [MethodArg] {
        match self {
            Lowering::Generic { args, .. } => args,
            Lowering::Custom => &[],
        }
    }

    /// The typecheck parameters on a receiver of `kind`.
    pub fn params(&self, kind: &ContainerKind) -> Vec<crate::typecheck::Param> {
        self.args().iter().map(|a| a.param(kind)).collect()
    }
}

/// A `Generic` lowering with no port type overrides.
pub const fn generic(args: &'static [MethodArg], result: WirePort) -> Lowering {
    Lowering::Generic { args, result, port_types: &[] }
}

/// The value outputs of `gate` on a receiver of `kind`, as lowering declares
/// them: the catalog's outputs minus the exec-out and minus any output that
/// shares its name with an input (the gate's pass-through of that input, e.g.
/// `find`'s `Value`). A generic `any` output carries the container's element
/// (an array's element, a map's value). `result` comes first, the output emit
/// reads a gate's value type from; the rest keep the catalog's order.
pub fn gate_outputs(
    gate: &str,
    kind: &ContainerKind,
    result: WirePort,
    port_types: &[(WirePort, Type)],
) -> Vec<(WirePort, Type)> {
    let spec = super::default_catalog()
        .find_by_class(gate)
        .unwrap_or_else(|| panic!("container method gate {gate} is not in the gate catalog"));
    let elem = match kind {
        ContainerKind::Array { elem } => elem,
        ContainerKind::Map { value, .. } => value,
    };
    let inputs: crate::collections::HashSet<&str> =
        spec.component.inputs.iter().map(|p| p.name.as_str()).collect();
    let mut outs: Vec<(WirePort, Type)> = spec
        .component
        .outputs
        .iter()
        .filter(|p| p.ty != super::RawPortType::Exec && !inputs.contains(p.name.as_str()))
        .map(|p| {
            let port = WirePort::from_name(&p.name);
            let ty = port_types
                .iter()
                .find(|(q, _)| *q == port)
                .map_or_else(|| super::arrays::port_type(&p.ty, elem), |(_, t)| t.clone());
            (port, ty)
        })
        .collect();
    if let Some(i) = outs.iter().position(|(p, _)| *p == result) {
        let r = outs.remove(i);
        outs.insert(0, r);
    }
    outs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::storage::{access, Access};

    /// Every `(name, gate, mutates, lowering, receiver port)` in both tables.
    fn entries() -> Vec<(&'static str, &'static str, bool, &'static Lowering, WirePort)> {
        let arrays = super::super::arrays::ARRAY_METHODS
            .iter()
            .map(|m| (m.name, m.gate, m.mutates, &m.lowering, WirePort::ArrayVarRef));
        let maps = super::super::maps::MAP_METHODS
            .iter()
            .map(|m| (m.name, m.gate, m.mutates, &m.lowering, WirePort::MapVarRef));
        arrays.chain(maps).collect()
    }

    /// Each argument feeds a real input of its gate, each `port_types`
    /// override names a real port, and the result is a real output.
    #[test]
    fn lowering_ports_exist_on_the_gate() {
        for (name, gate, _, lowering, _) in entries() {
            let spec = crate::catalog::default_catalog()
                .find_by_class(gate)
                .unwrap_or_else(|| panic!("`{name}`: gate {gate} is not in the catalog"));
            let has = |ports: &[crate::catalog::Port], p: WirePort| ports.iter().any(|q| q.name == p.as_str());
            let Lowering::Generic { args, result, port_types } = lowering else { continue };
            for a in *args {
                assert!(has(&spec.component.inputs, a.port), "`{name}`: {} is not an input of {gate}", a.port);
            }
            for (p, _) in *port_types {
                assert!(
                    has(&spec.component.inputs, *p) || has(&spec.component.outputs, *p),
                    "`{name}`: override {p} is not a port of {gate}"
                );
            }
            assert!(has(&spec.component.outputs, *result), "`{name}`: result {result} is not an output of {gate}");
        }
    }

    /// `mutates` says what the storage classification says about the gate's
    /// receiver port.
    #[test]
    fn mutates_agrees_with_storage_access() {
        for (name, gate, mutates, _, receiver) in entries() {
            let expected = if mutates { Access::Write } else { Access::Read };
            assert_eq!(access(gate, receiver), Some(expected), "`{name}` ({gate})");
        }
    }
}
