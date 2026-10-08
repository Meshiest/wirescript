//! Canonical array-method table — the single source of truth for the methods
//! callable on an array value (`arr.push(x)`, `arr.find(x)`, ...).
//!
//! Each entry pairs a surface name with the gate it lowers to plus curated
//! display strings (completion signature + hover docs, which the auto-generated
//! gate inventory doesn't carry). Everything *derivable* — the return type and
//! the gate's output shape — is read from the gate catalog via that gate, so it
//! can't drift from the game's actual ports. The per-method input wiring is
//! the entry's [`Lowering`], which `lower::access` builds the gate from.

use crate::ir::gate_class as gc;
use crate::ir::Type;
use crate::ir::port_registry::WirePort;
use crate::typecheck::ContainerKind;

use super::RawPortType;
use super::methods::{arg, generic, opt, val, ArgKind, ArgTy, Lowering};

/// One array method: surface name, the gate it lowers to, curated display
/// strings, and how it lowers. Its typed parameters and its return type are
/// derived: the parameters from the lowering's arguments, the return type from
/// the gate's output ports.
pub struct ArrayMethod {
    pub name: &'static str,
    /// Gate class this method lowers to (source of the derived return type).
    pub gate: &'static str,
    /// Parameter signature shown in completion, e.g. `"(value)"`.
    pub signature: &'static str,
    /// One-line hover documentation.
    pub doc: &'static str,
    /// Which argument feeds which gate input and which output the call
    /// returns. Typecheck reads the same arguments as the method's parameters
    /// ([`ArrayMethod::params`]), so a name, type or optionality has one
    /// spelling. `array_return_type` is the source of truth for the return
    /// type; `CallSignature` doesn't carry one.
    pub lowering: Lowering,
    /// Does this method CHANGE the receiver's contents?
    ///
    /// Stored per-entry as DATA so a method added to `ARRAY_METHODS` must
    /// state its own answer rather than inherit a default that could be
    /// silently wrong. Deriving it from the gate class would be a second,
    /// drift-prone spelling of the same fact.
    ///
    /// Read by the `const` container path in `lower::access` — a `const` array
    /// is a compile-time value AND a runtime container, and the two can only
    /// stay one source of truth while nothing mutates it. "Read-only" is meant
    /// with respect to the RECEIVER; no array method writes anywhere else.
    pub mutates: bool,
}

impl ArrayMethod {
    /// The method's typed parameters on an array of `elem`.
    pub fn params(&self, elem: &Type) -> Vec<crate::typecheck::Param> {
        self.lowering.params(&ContainerKind::Array { elem: elem.clone() })
    }

    /// Build the [`CallSignature`](crate::typecheck::CallSignature) for a call
    /// on an array whose element type is `elem`.
    pub fn signature(&self, elem: &Type) -> crate::typecheck::CallSignature {
        crate::typecheck::CallSignature {
            name: self.name.to_string(),
            params: self.params(elem),
            config_gate: None,
        }
    }
}

use ArgTy::{Bool, Elem, Entity, Int, String as Str, Zone};
use WirePort as P;

/// Every method callable on an array, in a stable display order.
pub static ARRAY_METHODS: &[ArrayMethod] = &[
    ArrayMethod { name: "push", mutates: true, gate: gc::ARRAY_PUSH, signature: "(value)", doc: "Append an element to the end of the array", lowering: generic(&[val("value", P::Value, Elem)], P::ExecOut) },
    ArrayMethod { name: "pop", mutates: true, gate: gc::ARRAY_POP, signature: "()", doc: "Remove and return the last element", lowering: generic(&[], P::Value) },
    ArrayMethod { name: "length", mutates: false, gate: gc::ARRAY_GET_LENGTH, signature: "() -> int", doc: "Return the number of elements", lowering: generic(&[], P::Length) },
    ArrayMethod { name: "remove", mutates: true, gate: gc::ARRAY_REMOVE_AT_INDEX, signature: "(index)", doc: "Remove the element at the given index", lowering: generic(&[val("index", P::Index, Int)], P::ExecOut) },
    ArrayMethod { name: "insert", mutates: true, gate: gc::ARRAY_INSERT, signature: "(index, value)", doc: "Insert an element at the given index", lowering: generic(&[val("index", P::Index, Int), val("value", P::Value, Elem)], P::ExecOut) },
    ArrayMethod { name: "clear", mutates: true, gate: gc::ARRAY_CLEAR, signature: "()", doc: "Remove all elements from the array", lowering: generic(&[], P::ExecOut) },
    ArrayMethod { name: "get", mutates: false, gate: gc::ARRAY_GET, signature: "(index)", doc: "Read the element at index; gives its Value (default) and OutOfBounds", lowering: generic(&[val("index", P::Index, Int)], P::Value) },
    ArrayMethod { name: "find", mutates: false, gate: gc::ARRAY_FIND, signature: "(value)", doc: "Find the first matching element; gives its Index (default, -1 if absent), Found, and Value", lowering: generic(&[val("value", P::Value, Elem)], P::Index) },
    ArrayMethod { name: "sort", mutates: true, gate: gc::ARRAY_SORT, signature: "(descending?)", doc: "Sort the array in place", lowering: generic(&[opt("descending", P::BDescending, Bool)], P::ExecOut) },
    ArrayMethod { name: "reverse", mutates: true, gate: gc::ARRAY_REVERSE, signature: "()", doc: "Reverse the element order in place", lowering: generic(&[], P::ExecOut) },
    ArrayMethod { name: "shuffle", mutates: true, gate: gc::ARRAY_SHUFFLE, signature: "()", doc: "Randomly reorder all elements", lowering: generic(&[], P::ExecOut) },
    ArrayMethod { name: "swap", mutates: true, gate: gc::ARRAY_SWAP, signature: "(a, b)", doc: "Swap the elements at indices a and b", lowering: generic(&[val("a", P::IndexA, Int), val("b", P::IndexB, Int)], P::ExecOut) },
    ArrayMethod { name: "fill", mutates: true, gate: gc::ARRAY_FILL, signature: "(value)", doc: "Set every element to value", lowering: generic(&[val("value", P::Value, Elem)], P::ExecOut) },
    ArrayMethod { name: "resize", mutates: true, gate: gc::ARRAY_RESIZE, signature: "(size, value)", doc: "Grow/shrink to size, filling new slots with value", lowering: generic(&[val("size", P::Size, Int), val("value", P::Value, Elem)], P::ExecOut) },
    ArrayMethod { name: "sum", mutates: false, gate: gc::ARRAY_SUM, signature: "()", doc: "Sum of all elements", lowering: generic(&[], P::Value) },
    ArrayMethod { name: "min", mutates: false, gate: gc::ARRAY_MIN, signature: "()", doc: "Smallest element", lowering: generic(&[], P::Value) },
    ArrayMethod { name: "max", mutates: false, gate: gc::ARRAY_MAX, signature: "()", doc: "Largest element", lowering: generic(&[], P::Value) },
    // The gate's `Value` is declared a float whatever the element type: the
    // mean of an int array is fractional.
    ArrayMethod { name: "average", mutates: false, gate: gc::ARRAY_AVERAGE, signature: "()", doc: "Mean of all elements", lowering: Lowering::Generic { args: &[], result: P::Value, port_types: &[(P::Value, Type::Float)] } },
    ArrayMethod { name: "append", mutates: true, gate: gc::ARRAY_APPEND, signature: "(source)", doc: "Append all elements of another array", lowering: generic(&[arg("source", P::SourceRef, ArgKind::ArrayRef(Elem))], P::ExecOut) },
    ArrayMethod { name: "copyFrom", mutates: true, gate: gc::ARRAY_COPY_FROM, signature: "(source)", doc: "Replace contents with a copy of another array", lowering: generic(&[arg("source", P::SourceRef, ArgKind::ArrayRef(Elem))], P::ExecOut) },
    ArrayMethod { name: "slice", mutates: true, gate: gc::ARRAY_SLICE, signature: "(source, start, count)", doc: "Copy source[start..start+count] into this array", lowering: generic(&[arg("source", P::SourceRef, ArgKind::ArrayRef(Elem)), val("start", P::Start, Int), val("count", P::Count, Int)], P::ExecOut) },
    ArrayMethod { name: "fillFromPlayers", mutates: true, gate: gc::GAMEMODE_FILL_FROM_PLAYERS, signature: "()", doc: "Fill this array with all current players", lowering: generic(&[], P::ExecOut) },
    ArrayMethod { name: "fillFromTeam", mutates: true, gate: gc::GAMEMODE_FILL_FROM_TEAM, signature: "(team)", doc: "Fill this array with the members of a team", lowering: generic(&[val("team", P::Team, Entity)], P::ExecOut) },
    // `fillFromZone*` take a `zone` rerouter reference ONLY (`in z: zone` ->
    // `Type::Zone`). The `Zone` slot never accepts a plain entity, only a
    // zone; the gate input is declared `entity`, the object wire a `ZoneRef`
    // connection carries. `TagFilter` is declared `any`.
    ArrayMethod { name: "fillFromZoneEntities", mutates: true, gate: gc::ZONE_GET_ENTITIES, signature: "(zone, tagFilter?)", doc: "Fill this array with the entities inside a zone", lowering: Lowering::Generic { args: &[val("zone", P::Zone, Zone), opt("tagFilter", P::TagFilter, Str)], result: P::ExecOut, port_types: &[(P::TagFilter, Type::Any)] } },
    ArrayMethod { name: "fillFromZonePlayers", mutates: true, gate: gc::ZONE_GET_PLAYERS, signature: "(zone, tagFilter?)", doc: "Fill this array with the players inside a zone", lowering: Lowering::Generic { args: &[val("zone", P::Zone, Zone), opt("tagFilter", P::TagFilter, Str)], result: P::ExecOut, port_types: &[(P::TagFilter, Type::Any)] } },
    // `sortMultiple` is a true variadic (this array + up to 7 parallel arrays,
    // then an optional `descending?`) that a fixed argument list can't
    // express; its empty params opt it out of `check_args`'s arity/type
    // checking rather than rejecting valid calls against a fake fixed arity.
    ArrayMethod { name: "sortMultiple", mutates: true, gate: gc::ARRAY_SORT_MULTIPLE, signature: "(other, ..., descending?)", doc: "Sort this array and up to 7 parallel arrays together, by this array's order", lowering: Lowering::Custom },
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
    field_name_ref(port).to_string()
}

/// Allocation-free view of [`field_name`] — the hot port-resolution paths
/// only ever compare it against a field string.
pub fn field_name_ref(port: &str) -> &str {
    if let Some(rest) = port.strip_prefix('b') {
        if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return rest;
        }
    }
    port
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
        RawPortType::Brick => Type::Brick,
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
    // A record ELEMENT read (`pop`/`get`) yields the element record directly, so
    // `p = pts.pop()` / `pts[i].field` type-check like a plain value read - there
    // is no `IsEmpty`/`OutOfBounds` flag to pair with a record. The record-array
    // lowering fans these out per field. Mirrors the record-valued map `get`.
    if matches!(method, "pop" | "get") && matches!(elem, Type::Record(_)) {
        return Some(Type::call_result(vec![("Value".into(), elem.clone())]));
    }
    let m = array_method(method)?;
    let gate = super::default_catalog().find_by_class(m.gate)?;
    // An output that shares a name with an input is the gate's pass-through of
    // that input (e.g. `find`'s `Value` is both the search arg and the found
    // element) — it isn't a distinct result, and exposing it would collide with
    // the input wire, so drop it.
    let input_names: std::collections::HashSet<&str> =
        gate.component.inputs.iter().map(|p| p.name.as_str()).collect();
    let mut fields: Vec<(String, Type)> = gate
        .component
        .outputs
        .iter()
        .filter(|p| p.ty != RawPortType::Exec && !input_names.contains(p.name.as_str()))
        .map(|p| (field_name(&p.name), port_type(&p.ty, elem)))
        .collect();
    // The output a bare use reads (lowering's default: `find`'s `Index`, the
    // element `Value` of the others) is the record's first field, the one an
    // operand or a scalar slot unwraps to; the gate lists `IsEmpty` first.
    let default = if method == "find" { "Index" } else { "Value" };
    if let Some(i) = fields.iter().position(|(n, _)| n == default) {
        let f = fields.remove(i);
        fields.insert(0, f);
    }
    // No distinct output = a void mutation (`push`, `clear`, `fill`, ...):
    // `Never` (not `Any`), so using its "result" as a value is a type
    // mismatch instead of being silently accepted.
    Some(Type::call_result(fields))
}

#[cfg(test)]
mod tests;
