//! Canonical map-method table — the methods callable on a `map` value
//! (`m.get(k)`, `m.set(k, v)`, `m.has(k)`, ...). Parallels [`super::arrays`].
//!
//! Each entry pairs a surface name with the `MapVar` gate it lowers to plus
//! curated display strings and its [`Lowering`], which `lower::access` builds
//! the gate from.

use crate::ir::gate_class as gc;
use crate::ir::Type;
use crate::ir::port_registry::WirePort;
use crate::typecheck::ContainerKind;

use super::methods::{arg, generic, opt, val, ArgKind, ArgTy, Lowering};

/// One map method: surface name, the gate it lowers to, curated display, and
/// how it lowers. Its typed parameters are derived from the lowering's
/// arguments.
pub struct MapMethod {
    pub name: &'static str,
    pub gate: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
    /// Which argument feeds which gate input and which output the call
    /// returns. Typecheck reads the same arguments as the method's parameters
    /// ([`MapMethod::params`]). `map_return_type` is the source of truth for
    /// the return type; `CallSignature` doesn't carry one.
    pub lowering: Lowering,
    /// Does this method CHANGE the receiver's contents? Same per-entry-data
    /// rule and the same reader as [`super::arrays::ArrayMethod::mutates`].
    /// `keys`/`values` are read-only here because what they write is a
    /// caller-supplied destination ARRAY, not the map they are called on.
    pub mutates: bool,
}

impl MapMethod {
    /// The method's typed parameters on a `Map<key, value>`.
    pub fn params(&self, key: &Type, value: &Type) -> Vec<crate::typecheck::Param> {
        self.lowering.params(&ContainerKind::Map { key: key.clone(), value: value.clone() })
    }

    /// Build the [`CallSignature`](crate::typecheck::CallSignature) for a call
    /// on a `Map<key, value>`.
    pub fn signature(&self, key: &Type, value: &Type) -> crate::typecheck::CallSignature {
        crate::typecheck::CallSignature {
            name: self.name.to_string(),
            params: self.params(key, value),
            config_gate: None,
        }
    }
}

use ArgTy::{Bool, Key, MapValue};
use WirePort as P;

/// Every method callable on a map, in a stable display order.
pub static MAP_METHODS: &[MapMethod] = &[
    MapMethod { name: "get", mutates: false, gate: gc::MAP_GET, signature: "(key)", doc: "Read the value at key; gives its Value (default) and Found", lowering: generic(&[val("key", P::Key, Key)], P::Value) },
    MapMethod { name: "set", mutates: true, gate: gc::MAP_SET, signature: "(key, value)", doc: "Insert or overwrite the value at key", lowering: generic(&[val("key", P::Key, Key), val("value", P::Value, MapValue)], P::ExecOut) },
    MapMethod { name: "has", mutates: false, gate: gc::MAP_HAS, signature: "(key)", doc: "Whether the map contains key", lowering: generic(&[val("key", P::Key, Key)], P::BFound) },
    MapMethod { name: "remove", mutates: true, gate: gc::MAP_REMOVE, signature: "(key)", doc: "Remove key; gives whether it was present", lowering: generic(&[val("key", P::Key, Key)], P::BFound) },
    MapMethod { name: "clear", mutates: true, gate: gc::MAP_CLEAR, signature: "()", doc: "Remove all entries", lowering: generic(&[], P::ExecOut) },
    MapMethod { name: "copyFrom", mutates: true, gate: gc::MAP_COPY_FROM, signature: "(source)", doc: "Replace contents with a copy of another map", lowering: generic(&[arg("source", P::SourceRef, ArgKind::MapRef)], P::ExecOut) },
    MapMethod { name: "length", mutates: false, gate: gc::MAP_GET_LENGTH, signature: "() -> int", doc: "Number of entries", lowering: generic(&[], P::Length) },
    // `keys`/`values` fill a caller-supplied destination array in place; the
    // `destArray: K[]`/`V[]` array is a PARAMETER, and the call itself has no
    // value (`Never`, `map_return_type`).
    MapMethod { name: "keys", mutates: false, gate: gc::MAP_GET_KEYS, signature: "(destArray, includeDead?)", doc: "Fill an array with the map's keys; a brick key whose brick is gone is skipped unless includeDead", lowering: generic(&[arg("destArray", P::ArrayVarRef, ArgKind::DestArray(Key)), opt("includeDead", P::BIncludeDead, Bool)], P::ExecOut) },
    MapMethod { name: "values", mutates: false, gate: gc::MAP_GET_VALUES, signature: "(destArray)", doc: "Fill an array with the map's values", lowering: generic(&[arg("destArray", P::ArrayVarRef, ArgKind::DestArray(MapValue))], P::ExecOut) },
];

pub fn map_methods() -> &'static [MapMethod] {
    MAP_METHODS
}

pub fn map_method(name: &str) -> Option<&'static MapMethod> {
    MAP_METHODS.iter().find(|m| m.name == name)
}

static MAP_METHOD_NAMES: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| MAP_METHODS.iter().map(|m| m.name).collect());

pub fn is_map_method(name: &str) -> bool {
    MAP_METHOD_NAMES.contains(name)
}

/// The return type of `m.<method>()` for a `Map<key, value>`. `get` yields a
/// record that auto-unwraps to `Value`; `has`/`remove` a bool; `length` an int;
/// the rest are void statements (`never`).
pub fn map_return_type(method: &str, _key: &Type, value: &Type) -> Option<Type> {
    Some(match method {
        // A record-valued map returns the value RECORD directly, so
        // `m.get(k).field` and `p = m.get(k)` type-check like the `m[k]`
        // subscript form (which the record-map lowering also fans out per
        // field). There is no `Found` flag to expose alongside a record - the
        // `{Value, Found}` pair is kept only for a scalar value.
        "get" if matches!(value, Type::Record(_)) => Type::call_result(vec![("Value".into(), value.clone())]),
        "get" => Type::call_result(vec![("Value".into(), value.clone()), ("Found".into(), Type::Bool)]),
        "has" | "remove" => Type::Bool,
        "length" => Type::Int,
        // All void statements: `set`/`clear`/`copyFrom` mutate the map;
        // `keys`/`values` fill the destination array passed as their argument
        // (`m.keys(destArray)`). None produce a value, so `Never` (not `Any`)
        // makes consuming a "result" a type error.
        "set" | "clear" | "copyFrom" | "keys" | "values" => Type::Never,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
