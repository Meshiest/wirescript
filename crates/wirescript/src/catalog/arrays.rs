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

/// One array method: surface name, the gate it lowers to, curated display
/// strings, and its typed-parameter builder. Its return type is derived from
/// the gate's output ports.
pub struct ArrayMethod {
    pub name: &'static str,
    /// Gate class this method lowers to (source of the derived return type).
    pub gate: &'static str,
    /// Parameter signature shown in completion, e.g. `"(value)"`.
    pub signature: &'static str,
    /// One-line hover documentation.
    pub doc: &'static str,
    /// The method's structured, typed params as a function of the array's
    /// element type `T` — the machine-readable counterpart to the
    /// human-readable `signature` string. Stored per-entry as DATA (not a
    /// name-match) so the compiler requires every method to supply one: adding
    /// a method to `ARRAY_METHODS` without a param builder is a compile error,
    /// not a silent no-param gap or a runtime panic. Param NAMES/OPTIONALITY
    /// mirror `signature` 1:1; every param is `ParamKind::Wire` (array methods
    /// have no config-only params). `array_return_type` is the source of truth
    /// for the return type — `CallSignature` doesn't carry one.
    pub params: fn(&Type) -> Vec<crate::typecheck::Param>,
    /// Does this method CHANGE the receiver's contents?
    ///
    /// Stored per-entry as DATA for the same reason `params` is: a method added
    /// to `ARRAY_METHODS` must state its own answer rather than inherit a
    /// default that could be silently wrong. Deriving it from the gate class
    /// would be a second, drift-prone spelling of the same fact.
    ///
    /// Read by the `const` container path in `lower::access` — a `const` array
    /// is a compile-time value AND a runtime container, and the two can only
    /// stay one source of truth while nothing mutates it. "Read-only" is meant
    /// with respect to the RECEIVER; no array method writes anywhere else.
    pub mutates: bool,
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

impl ArrayMethod {
    /// Build the [`CallSignature`](crate::typecheck::CallSignature) for a call
    /// on an array whose element type is `elem`.
    pub fn signature(&self, elem: &Type) -> crate::typecheck::CallSignature {
        crate::typecheck::CallSignature {
            name: self.name.to_string(),
            params: (self.params)(elem),
            config_gate: None,
        }
    }
}

/// Every method callable on an array, in a stable display order.
pub static ARRAY_METHODS: &[ArrayMethod] = &[
    ArrayMethod { name: "push", mutates: true, gate: gc::ARRAY_PUSH, signature: "(value)", doc: "Append an element to the end of the array", params: |t| vec![p("value", t.clone(), false)] },
    ArrayMethod { name: "pop", mutates: true, gate: gc::ARRAY_POP, signature: "()", doc: "Remove and return the last element", params: |_| vec![] },
    ArrayMethod { name: "length", mutates: false, gate: gc::ARRAY_GET_LENGTH, signature: "() -> int", doc: "Return the number of elements", params: |_| vec![] },
    ArrayMethod { name: "remove", mutates: true, gate: gc::ARRAY_REMOVE_AT_INDEX, signature: "(index)", doc: "Remove the element at the given index", params: |_| vec![p("index", Type::Int, false)] },
    ArrayMethod { name: "insert", mutates: true, gate: gc::ARRAY_INSERT, signature: "(index, value)", doc: "Insert an element at the given index", params: |t| vec![p("index", Type::Int, false), p("value", t.clone(), false)] },
    ArrayMethod { name: "clear", mutates: true, gate: gc::ARRAY_CLEAR, signature: "()", doc: "Remove all elements from the array", params: |_| vec![] },
    ArrayMethod { name: "get", mutates: false, gate: gc::ARRAY_GET, signature: "(index)", doc: "Read the element at index; gives its Value (default) and OutOfBounds", params: |_| vec![p("index", Type::Int, false)] },
    ArrayMethod { name: "find", mutates: false, gate: gc::ARRAY_FIND, signature: "(value)", doc: "Find the first matching element; gives its Index (default, -1 if absent), Found, and Value", params: |t| vec![p("value", t.clone(), false)] },
    ArrayMethod { name: "sort", mutates: true, gate: gc::ARRAY_SORT, signature: "(descending?)", doc: "Sort the array in place", params: |_| vec![p("descending", Type::Bool, true)] },
    ArrayMethod { name: "reverse", mutates: true, gate: gc::ARRAY_REVERSE, signature: "()", doc: "Reverse the element order in place", params: |_| vec![] },
    ArrayMethod { name: "shuffle", mutates: true, gate: gc::ARRAY_SHUFFLE, signature: "()", doc: "Randomly reorder all elements", params: |_| vec![] },
    ArrayMethod { name: "swap", mutates: true, gate: gc::ARRAY_SWAP, signature: "(a, b)", doc: "Swap the elements at indices a and b", params: |_| vec![p("a", Type::Int, false), p("b", Type::Int, false)] },
    ArrayMethod { name: "fill", mutates: true, gate: gc::ARRAY_FILL, signature: "(value)", doc: "Set every element to value", params: |t| vec![p("value", t.clone(), false)] },
    ArrayMethod { name: "resize", mutates: true, gate: gc::ARRAY_RESIZE, signature: "(size, value)", doc: "Grow/shrink to size, filling new slots with value", params: |t| vec![p("size", Type::Int, false), p("value", t.clone(), false)] },
    ArrayMethod { name: "sum", mutates: false, gate: gc::ARRAY_SUM, signature: "()", doc: "Sum of all elements", params: |_| vec![] },
    ArrayMethod { name: "min", mutates: false, gate: gc::ARRAY_MIN, signature: "()", doc: "Smallest element", params: |_| vec![] },
    ArrayMethod { name: "max", mutates: false, gate: gc::ARRAY_MAX, signature: "()", doc: "Largest element", params: |_| vec![] },
    ArrayMethod { name: "average", mutates: false, gate: gc::ARRAY_AVERAGE, signature: "()", doc: "Mean of all elements", params: |_| vec![] },
    ArrayMethod { name: "append", mutates: true, gate: gc::ARRAY_APPEND, signature: "(source)", doc: "Append all elements of another array", params: |t| vec![p("source", Type::Array(Box::new(t.clone())), false)] },
    ArrayMethod { name: "copyFrom", mutates: true, gate: gc::ARRAY_COPY_FROM, signature: "(source)", doc: "Replace contents with a copy of another array", params: |t| vec![p("source", Type::Array(Box::new(t.clone())), false)] },
    ArrayMethod { name: "slice", mutates: true, gate: gc::ARRAY_SLICE, signature: "(source, start, count)", doc: "Copy source[start..start+count] into this array", params: |t| vec![p("source", Type::Array(Box::new(t.clone())), false), p("start", Type::Int, false), p("count", Type::Int, false)] },
    ArrayMethod { name: "fillFromPlayers", mutates: true, gate: gc::GAMEMODE_FILL_FROM_PLAYERS, signature: "()", doc: "Fill this array with all current players", params: |_| vec![] },
    ArrayMethod { name: "fillFromTeam", mutates: true, gate: gc::GAMEMODE_FILL_FROM_TEAM, signature: "(team)", doc: "Fill this array with the members of a team", params: |_| vec![p("team", Type::Entity, false)] },
    // `fillFromZone*` take a `zone` rerouter reference ONLY (`in z: zone` →
    // `Type::Zone`). The `Zone` slot never accepts a plain entity — only a
    // zone. (The `Type::Entity` seen at the wire layer in `lower::access` is
    // the internal object-wire type a `ZoneRef` connection carries, not a
    // source-level entity input.)
    ArrayMethod { name: "fillFromZoneEntities", mutates: true, gate: gc::ZONE_GET_ENTITIES, signature: "(zone, tagFilter?)", doc: "Fill this array with the entities inside a zone", params: |_| vec![p("zone", Type::Zone, false), p("tagFilter", Type::String, true)] },
    ArrayMethod { name: "fillFromZonePlayers", mutates: true, gate: gc::ZONE_GET_PLAYERS, signature: "(zone, tagFilter?)", doc: "Fill this array with the players inside a zone", params: |_| vec![p("zone", Type::Zone, false), p("tagFilter", Type::String, true)] },
    // `sortMultiple` is a true variadic (this array + up to 7 parallel arrays,
    // then an optional `descending?`) that `Param`'s fixed arity can't express;
    // empty params opts it out of `check_args`'s arity/type checking rather
    // than rejecting valid calls against a fake fixed arity.
    ArrayMethod { name: "sortMultiple", mutates: true, gate: gc::ARRAY_SORT_MULTIPLE, signature: "(other, ..., descending?)", doc: "Sort this array and up to 7 parallel arrays together, by this array's order", params: |_| vec![] },
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
        // No distinct output = a void mutation (`push`, `clear`, `fill`, …).
        // `Never` (not `Any`) so using its "result" as a value is a type
        // mismatch instead of being silently accepted.
        0 => Type::Never,
        1 => fields.into_iter().next().unwrap().1,
        _ => Type::Record(fields),
    })
}

#[cfg(test)]
mod tests;
