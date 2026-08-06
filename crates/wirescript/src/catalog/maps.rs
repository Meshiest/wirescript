//! Canonical map-method table — the methods callable on a `map` value
//! (`m.get(k)`, `m.set(k, v)`, `m.has(k)`, ...). Parallels [`super::arrays`].
//!
//! Each entry pairs a surface name with the `MapVar` gate it lowers to plus
//! curated display strings. The per-method input wiring lives in
//! [`crate::lower::access::lower_map_method`]; every name here must be handled
//! there (enforced by a test in that module).

use crate::ir::Type;
use crate::ir::gate_class as gc;

/// One map method: surface name, the gate it lowers to, and curated display.
pub struct MapMethod {
    pub name: &'static str,
    pub gate: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
}

/// Every method callable on a map, in a stable display order.
pub static MAP_METHODS: &[MapMethod] = &[
    MapMethod { name: "get", gate: gc::MAP_GET, signature: "(key)", doc: "Read the value at key; gives its Value (default) and Found" },
    MapMethod { name: "set", gate: gc::MAP_SET, signature: "(key, value)", doc: "Insert or overwrite the value at key" },
    MapMethod { name: "has", gate: gc::MAP_HAS, signature: "(key)", doc: "Whether the map contains key" },
    MapMethod { name: "remove", gate: gc::MAP_REMOVE, signature: "(key)", doc: "Remove key; gives whether it was present" },
    MapMethod { name: "clear", gate: gc::MAP_CLEAR, signature: "()", doc: "Remove all entries" },
    MapMethod { name: "copyFrom", gate: gc::MAP_COPY_FROM, signature: "(source)", doc: "Replace contents with a copy of another map" },
    MapMethod { name: "length", gate: gc::MAP_GET_LENGTH, signature: "() -> int", doc: "Number of entries" },
    MapMethod { name: "keys", gate: gc::MAP_GET_KEYS, signature: "(destArray)", doc: "Fill an array with the map's keys" },
    MapMethod { name: "values", gate: gc::MAP_GET_VALUES, signature: "(destArray)", doc: "Fill an array with the map's values" },
];

pub fn map_method(name: &str) -> Option<&'static MapMethod> {
    MAP_METHODS.iter().find(|m| m.name == name)
}

static MAP_METHOD_NAMES: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| MAP_METHODS.iter().map(|m| m.name).collect());

/// Is `name` a method callable on a map value?
pub fn is_map_method(name: &str) -> bool {
    MAP_METHOD_NAMES.contains(name)
}

/// The return type of `m.<method>()` for a `Dict<key, value>`. `get` yields a
/// record that auto-unwraps to `Value`; `has`/`remove` a bool; `length` an int;
/// the rest are statements (`Any`).
pub fn map_return_type(method: &str, _key: &Type, value: &Type) -> Option<Type> {
    Some(match method {
        "get" => Type::Record(vec![
            ("Value".into(), value.clone()),
            ("Found".into(), Type::Bool),
        ]),
        "has" | "remove" => Type::Bool,
        "length" => Type::Int,
        "set" | "clear" | "copyFrom" | "keys" | "values" => Type::Any,
        _ => return None,
    })
}
