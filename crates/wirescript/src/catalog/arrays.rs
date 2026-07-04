//! Canonical array-method table — the single source of truth for the methods
//! callable on an array value (`arr.push(x)`, `arr.find(x)`, ...).
//!
//! Each entry pairs a surface name with the gate it lowers to plus curated
//! display strings (completion signature + hover docs, which the auto-generated
//! gate inventory doesn't carry). Everything *derivable* — the return type and
//! the gate's output shape — is read from the gate catalog via that gate, so it
//! can't drift from the game's actual ports. The per-method input wiring lives
//! in [`crate::lower::access::lower_array_method`]; every name here must be
//! handled there (enforced by a test in that module).

use crate::ir::gate_class as gc;
use crate::ir::Type;

use super::RawPortType;

/// One array method: surface name, the gate it lowers to, and curated display
/// strings. Its return type is derived from the gate's output ports.
pub struct ArrayMethod {
    pub name: &'static str,
    /// Gate class this method lowers to (source of the derived return type).
    pub gate: &'static str,
    /// Parameter signature shown in completion, e.g. `"(value)"`.
    pub signature: &'static str,
    /// One-line hover documentation.
    pub doc: &'static str,
}

/// Every method callable on an array, in a stable display order.
pub static ARRAY_METHODS: &[ArrayMethod] = &[
    ArrayMethod { name: "push", gate: gc::ARRAY_PUSH, signature: "(value)", doc: "Append an element to the end of the array" },
    ArrayMethod { name: "pop", gate: gc::ARRAY_POP, signature: "()", doc: "Remove and return the last element" },
    ArrayMethod { name: "length", gate: gc::ARRAY_GET_LENGTH, signature: "() -> int", doc: "Return the number of elements" },
    ArrayMethod { name: "remove", gate: gc::ARRAY_REMOVE_AT_INDEX, signature: "(index)", doc: "Remove the element at the given index" },
    ArrayMethod { name: "insert", gate: gc::ARRAY_INSERT, signature: "(index, value)", doc: "Insert an element at the given index" },
    ArrayMethod { name: "clear", gate: gc::ARRAY_CLEAR, signature: "()", doc: "Remove all elements from the array" },
    ArrayMethod { name: "find", gate: gc::ARRAY_FIND, signature: "(value)", doc: "Find the first matching element; gives its Index (default, -1 if absent), Found, and Value" },
    ArrayMethod { name: "sort", gate: gc::ARRAY_SORT, signature: "(descending?)", doc: "Sort the array in place" },
    ArrayMethod { name: "reverse", gate: gc::ARRAY_REVERSE, signature: "()", doc: "Reverse the element order in place" },
    ArrayMethod { name: "shuffle", gate: gc::ARRAY_SHUFFLE, signature: "()", doc: "Randomly reorder all elements" },
    ArrayMethod { name: "swap", gate: gc::ARRAY_SWAP, signature: "(a, b)", doc: "Swap the elements at indices a and b" },
    ArrayMethod { name: "fill", gate: gc::ARRAY_FILL, signature: "(value)", doc: "Set every element to value" },
    ArrayMethod { name: "resize", gate: gc::ARRAY_RESIZE, signature: "(size, value)", doc: "Grow/shrink to size, filling new slots with value" },
    ArrayMethod { name: "sum", gate: gc::ARRAY_SUM, signature: "()", doc: "Sum of all elements" },
    ArrayMethod { name: "min", gate: gc::ARRAY_MIN, signature: "()", doc: "Smallest element" },
    ArrayMethod { name: "max", gate: gc::ARRAY_MAX, signature: "()", doc: "Largest element" },
    ArrayMethod { name: "average", gate: gc::ARRAY_AVERAGE, signature: "()", doc: "Mean of all elements" },
    ArrayMethod { name: "append", gate: gc::ARRAY_APPEND, signature: "(source)", doc: "Append all elements of another array" },
    ArrayMethod { name: "copyFrom", gate: gc::ARRAY_COPY_FROM, signature: "(source)", doc: "Replace contents with a copy of another array" },
    ArrayMethod { name: "slice", gate: gc::ARRAY_SLICE, signature: "(source, start, count)", doc: "Copy source[start..start+count] into this array" },
    ArrayMethod { name: "fillFromPlayers", gate: gc::GAMEMODE_FILL_FROM_PLAYERS, signature: "()", doc: "Fill this array with all current players" },
    ArrayMethod { name: "fillFromTeam", gate: gc::GAMEMODE_FILL_FROM_TEAM, signature: "(team)", doc: "Fill this array with the members of a team" },
];

/// All array methods.
pub fn array_methods() -> &'static [ArrayMethod] {
    ARRAY_METHODS
}

/// Look up an array method by name.
pub fn array_method(name: &str) -> Option<&'static ArrayMethod> {
    ARRAY_METHODS.iter().find(|m| m.name == name)
}

/// Set of array method names, for O(1) membership tests.
static ARRAY_METHOD_NAMES: std::sync::LazyLock<std::collections::HashSet<&'static str>> =
    std::sync::LazyLock::new(|| ARRAY_METHODS.iter().map(|m| m.name).collect());

/// Is `name` a method callable on an array value?
pub fn is_array_method(name: &str) -> bool {
    ARRAY_METHOD_NAMES.contains(name)
}

/// A gate output port name as a Wirescript record field: drop a leading `b`
/// boolean prefix (`bFound` -> `Found`, `bIsEmpty` -> `IsEmpty`). The lowering
/// uses this to resolve a field back to its port without a hand-written map.
pub fn field_name(port: &str) -> String {
    if let Some(rest) = port.strip_prefix('b') {
        if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return rest.to_string();
        }
    }
    port.to_string()
}

/// Map a gate port type to a Wirescript type. The generic `any` value port
/// carries the array's element type.
pub fn port_type(ty: &RawPortType, elem: &Type) -> Type {
    match ty {
        RawPortType::Bool => Type::Bool,
        RawPortType::Int => Type::Int,
        RawPortType::Float => Type::Float,
        RawPortType::String => Type::String,
        RawPortType::Vector => Type::Vector,
        RawPortType::Rotator => Type::Rotator,
        RawPortType::Color => Type::Color,
        RawPortType::Entity => Type::Entity,
        RawPortType::Character => Type::Character,
        RawPortType::Controller => Type::Controller,
        RawPortType::Any => elem.clone(),
        // VarRef / ArrayVarRef / Exec never appear as a value output.
        _ => Type::Any,
    }
}

/// The return type of `arr.<method>()` for an array of `elem`, derived from the
/// method's gate output ports (excluding the exec-out): no value outputs is a
/// statement (`Any`); one output is that scalar; several form a record (which
/// auto-unwraps to whichever field matches the use — e.g. `find` to its `int`
/// `Index`). Returns `None` for unknown methods.
pub fn array_return_type(method: &str, elem: &Type) -> Option<Type> {
    let m = array_method(method)?;
    let gate = super::default_catalog().find_by_class(m.gate)?;
    // An output that shares a name with an input is the gate's pass-through of
    // that input (e.g. `find`'s `Value` is both the search arg and the found
    // element) — it isn't a distinct result, and exposing it would collide with
    // the input wire, so drop it.
    let input_names: std::collections::HashSet<&str> =
        gate.component.inputs.iter().map(|p| p.name.as_str()).collect();
    let fields: Vec<(String, Type)> = gate
        .component
        .outputs
        .iter()
        .filter(|p| p.ty != RawPortType::Exec && !input_names.contains(p.name.as_str()))
        .map(|p| (field_name(&p.name), port_type(&p.ty, elem)))
        .collect();
    Some(match fields.len() {
        0 => Type::Any,
        1 => fields.into_iter().next().unwrap().1,
        _ => Type::Record(fields),
    })
}
