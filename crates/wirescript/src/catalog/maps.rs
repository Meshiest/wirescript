//! Canonical map-method table — the methods callable on a `map` value
//! (`m.get(k)`, `m.set(k, v)`, `m.has(k)`, ...). Parallels [`super::arrays`].
//!
//! Each entry pairs a surface name with the `MapVar` gate it lowers to plus
//! curated display strings. The per-method input wiring lives in
//! [`crate::lower::access::lower_map_method`]; every name here must be handled
//! there (enforced by a test in that module).

use crate::ir::Type;
use crate::ir::gate_class as gc;

/// One map method: surface name, the gate it lowers to, curated display, and
/// its typed-parameter builder.
pub struct MapMethod {
    pub name: &'static str,
    pub gate: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
    /// The method's structured, typed params as a function of the map's key
    /// type `K` and value type `V` — the machine-readable counterpart to the
    /// human-readable `signature` string. Stored per-entry as DATA (not a
    /// name-match) so the compiler requires every method to supply one: adding
    /// a method to `MAP_METHODS` without a param builder is a compile error,
    /// not a silent no-param gap or a runtime panic. Every param is
    /// `ParamKind::Wire` (map methods have no config-only params).
    /// `map_return_type` is the source of truth for the return type —
    /// `CallSignature` doesn't carry one.
    pub params: fn(&Type, &Type) -> Vec<crate::typecheck::Param>,
}

/// Build one `ParamKind::Wire` parameter (shared by the `params` builders).
fn p(name: &str, ty: Type, optional: bool) -> crate::typecheck::Param {
    crate::typecheck::Param {
        name: name.to_string(),
        ty,
        optional,
        kind: crate::typecheck::ParamKind::Wire,
    }
}

impl MapMethod {
    /// Build the [`CallSignature`](crate::typecheck::CallSignature) for a call
    /// on a `Map<key, value>`.
    pub fn signature(&self, key: &Type, value: &Type) -> crate::typecheck::CallSignature {
        crate::typecheck::CallSignature {
            name: self.name.to_string(),
            params: (self.params)(key, value),
            config_gate: None,
        }
    }
}

/// Every method callable on a map, in a stable display order.
pub static MAP_METHODS: &[MapMethod] = &[
    MapMethod { name: "get", gate: gc::MAP_GET, signature: "(key)", doc: "Read the value at key; gives its Value (default) and Found", params: |k, _| vec![p("key", k.clone(), false)] },
    MapMethod { name: "set", gate: gc::MAP_SET, signature: "(key, value)", doc: "Insert or overwrite the value at key", params: |k, v| vec![p("key", k.clone(), false), p("value", v.clone(), false)] },
    MapMethod { name: "has", gate: gc::MAP_HAS, signature: "(key)", doc: "Whether the map contains key", params: |k, _| vec![p("key", k.clone(), false)] },
    MapMethod { name: "remove", gate: gc::MAP_REMOVE, signature: "(key)", doc: "Remove key; gives whether it was present", params: |k, _| vec![p("key", k.clone(), false)] },
    MapMethod { name: "clear", gate: gc::MAP_CLEAR, signature: "()", doc: "Remove all entries", params: |_, _| vec![] },
    MapMethod { name: "copyFrom", gate: gc::MAP_COPY_FROM, signature: "(source)", doc: "Replace contents with a copy of another map", params: |k, v| vec![p("source", Type::Map(Box::new(k.clone()), Box::new(v.clone())), false)] },
    MapMethod { name: "length", gate: gc::MAP_GET_LENGTH, signature: "() -> int", doc: "Number of entries", params: |_, _| vec![] },
    // `keys`/`values` fill a caller-supplied destination array in place (see
    // `lower::access::lower_map_method`, which reads the first arg as the
    // array-ref destination); their own return is `Any` (`map_return_type`),
    // so the `out: K[]`/`V[]` array is a PARAMETER, not the result.
    MapMethod { name: "keys", gate: gc::MAP_GET_KEYS, signature: "(destArray)", doc: "Fill an array with the map's keys", params: |k, _| vec![p("out", Type::Array(Box::new(k.clone())), false)] },
    MapMethod { name: "values", gate: gc::MAP_GET_VALUES, signature: "(destArray)", doc: "Fill an array with the map's values", params: |_, v| vec![p("out", Type::Array(Box::new(v.clone())), false)] },
];

/// All map methods.
pub fn map_methods() -> &'static [MapMethod] {
    MAP_METHODS
}

pub fn map_method(name: &str) -> Option<&'static MapMethod> {
    MAP_METHODS.iter().find(|m| m.name == name)
}

static MAP_METHOD_NAMES: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| MAP_METHODS.iter().map(|m| m.name).collect());

/// Is `name` a method callable on a map value?
pub fn is_map_method(name: &str) -> bool {
    MAP_METHOD_NAMES.contains(name)
}

/// The return type of `m.<method>()` for a `Map<key, value>`. `get` yields a
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

#[cfg(test)]
mod tests;
