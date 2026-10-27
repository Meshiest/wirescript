//! The methods callable on an array value (`arr.push(x)`, `arr.find(x)`, ...).
//!
//! Each entry pairs a surface name with its gate, curated display strings, and
//! the [`Lowering`] that `lower::access` builds the gate from. The return type
//! and output shape are read from the gate catalog, so they match the game's ports.

use crate::ir::Type;
use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;
use crate::types::container::ContainerKind;

use super::methods::{ArgKind, ArgTy, Lowering, arg, generic, opt, val};

/// One array method: surface name, the gate it lowers to, curated display
/// strings, and how it lowers. Its typed parameters and its return type are
/// derived: the parameters from the lowering's arguments, the return type from
/// the gate's output ports.
pub struct ArrayMethod {
    pub name: &'static str,
    /// Gate class this method lowers to (source of the derived return type).
    pub gate: &'static str,
    /// One-line hover documentation.
    pub doc: &'static str,
    /// Which argument feeds which gate input and which output the call
    /// returns. Typecheck reads the same arguments as the method's parameters
    /// ([`ArrayMethod::params`]); the return type comes from `array_return_type`.
    pub lowering: Lowering,
    /// Whether the method changes the receiver's contents (no array method
    /// writes anywhere else). The `const` container path in `lower::access`
    /// reads it: a `const` array's compile-time value is only valid while
    /// nothing mutates the runtime container.
    pub mutates: bool,
}

impl ArrayMethod {
    /// The parameter list completion and hover show, e.g. `(index, value)`.
    pub fn shown_signature(&self) -> String {
        super::methods::shown_signature(self.name, &self.lowering)
    }

    /// The method's typed parameters on an array of `elem`.
    pub fn params(&self, elem: &Type) -> Vec<crate::types::signature::Param> {
        self.lowering
            .params(&ContainerKind::Array { elem: elem.clone() })
    }

    /// Build the [`CallSignature`](crate::types::signature::CallSignature) for a call
    /// on an array whose element type is `elem`.
    pub fn signature(&self, elem: &Type) -> crate::types::signature::CallSignature {
        crate::types::signature::CallSignature {
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
    ArrayMethod {
        name: "push",
        mutates: true,
        gate: gc::ARRAY_PUSH,
        doc: "Append an element to the end of the array",
        lowering: generic(&[val("value", P::Value, Elem)], P::ExecOut),
    },
    ArrayMethod {
        name: "pop",
        mutates: true,
        gate: gc::ARRAY_POP,
        doc: "Remove and return the last element",
        lowering: generic(&[], P::Value),
    },
    ArrayMethod {
        name: "length",
        mutates: false,
        gate: gc::ARRAY_GET_LENGTH,
        doc: "Return the number of elements",
        lowering: generic(&[], P::Length),
    },
    ArrayMethod {
        name: "remove",
        mutates: true,
        gate: gc::ARRAY_REMOVE_AT_INDEX,
        doc: "Remove the element at the given index",
        lowering: generic(&[val("index", P::Index, Int)], P::ExecOut),
    },
    ArrayMethod {
        name: "insert",
        mutates: true,
        gate: gc::ARRAY_INSERT,
        doc: "Insert an element at the given index",
        lowering: generic(
            &[val("index", P::Index, Int), val("value", P::Value, Elem)],
            P::ExecOut,
        ),
    },
    ArrayMethod {
        name: "clear",
        mutates: true,
        gate: gc::ARRAY_CLEAR,
        doc: "Remove all elements from the array",
        lowering: generic(&[], P::ExecOut),
    },
    ArrayMethod {
        name: "get",
        mutates: false,
        gate: gc::ARRAY_GET,
        doc: "Read the element at index; gives its Value (default) and OutOfBounds",
        lowering: generic(&[val("index", P::Index, Int)], P::Value),
    },
    ArrayMethod {
        name: "find",
        mutates: false,
        gate: gc::ARRAY_FIND,
        doc: "Find the first matching element; gives its Index (default, -1 if absent), Found, and Value",
        lowering: generic(&[val("value", P::Value, Elem)], P::Index),
    },
    ArrayMethod {
        name: "sort",
        mutates: true,
        gate: gc::ARRAY_SORT,
        doc: "Sort the array in place",
        lowering: generic(&[opt("descending", P::BDescending, Bool)], P::ExecOut),
    },
    ArrayMethod {
        name: "reverse",
        mutates: true,
        gate: gc::ARRAY_REVERSE,
        doc: "Reverse the element order in place",
        lowering: generic(&[], P::ExecOut),
    },
    ArrayMethod {
        name: "shuffle",
        mutates: true,
        gate: gc::ARRAY_SHUFFLE,
        doc: "Randomly reorder all elements",
        lowering: generic(&[], P::ExecOut),
    },
    ArrayMethod {
        name: "swap",
        mutates: true,
        gate: gc::ARRAY_SWAP,
        doc: "Swap the elements at indices a and b",
        lowering: generic(
            &[val("a", P::IndexA, Int), val("b", P::IndexB, Int)],
            P::ExecOut,
        ),
    },
    ArrayMethod {
        name: "fill",
        mutates: true,
        gate: gc::ARRAY_FILL,
        doc: "Set every element to value",
        lowering: generic(&[val("value", P::Value, Elem)], P::ExecOut),
    },
    ArrayMethod {
        name: "resize",
        mutates: true,
        gate: gc::ARRAY_RESIZE,
        doc: "Grow/shrink to size, filling new slots with value",
        lowering: generic(
            &[val("size", P::Size, Int), val("value", P::Value, Elem)],
            P::ExecOut,
        ),
    },
    ArrayMethod {
        name: "sum",
        mutates: false,
        gate: gc::ARRAY_SUM,
        doc: "Sum of all elements",
        lowering: generic(&[], P::Value),
    },
    ArrayMethod {
        name: "min",
        mutates: false,
        gate: gc::ARRAY_MIN,
        doc: "Smallest element",
        lowering: generic(&[], P::Value),
    },
    ArrayMethod {
        name: "max",
        mutates: false,
        gate: gc::ARRAY_MAX,
        doc: "Largest element",
        lowering: generic(&[], P::Value),
    },
    // The gate's `Value` is declared a float whatever the element type: the
    // mean of an int array is fractional.
    ArrayMethod {
        name: "average",
        mutates: false,
        gate: gc::ARRAY_AVERAGE,
        doc: "Mean of all elements",
        lowering: Lowering::Generic {
            args: &[],
            result: P::Value,
            port_types: &[(P::Value, Type::Float)],
        },
    },
    // The gate declares `Separator` and `Output` `any`, but saves both as
    // strings: every element is converted to text.
    ArrayMethod {
        name: "join",
        mutates: false,
        gate: gc::ARRAY_JOIN,
        doc: "Join every element into one string, with separator between them",
        lowering: Lowering::Generic {
            args: &[opt("separator", P::Separator, Str)],
            result: P::Output,
            port_types: &[(P::Separator, Type::String), (P::Output, Type::String)],
        },
    },
    ArrayMethod {
        name: "append",
        mutates: true,
        gate: gc::ARRAY_APPEND,
        doc: "Append all elements of another array",
        lowering: generic(
            &[arg("source", P::SourceRef, ArgKind::ArrayRef(Elem))],
            P::ExecOut,
        ),
    },
    ArrayMethod {
        name: "copyFrom",
        mutates: true,
        gate: gc::ARRAY_COPY_FROM,
        doc: "Replace contents with a copy of another array",
        lowering: generic(
            &[arg("source", P::SourceRef, ArgKind::ArrayRef(Elem))],
            P::ExecOut,
        ),
    },
    ArrayMethod {
        name: "slice",
        mutates: true,
        gate: gc::ARRAY_SLICE,
        doc: "Copy source[start..start+count] into this array",
        lowering: generic(
            &[
                arg("source", P::SourceRef, ArgKind::ArrayRef(Elem)),
                val("start", P::Start, Int),
                val("count", P::Count, Int),
            ],
            P::ExecOut,
        ),
    },
    ArrayMethod {
        name: "fillFromPlayers",
        mutates: true,
        gate: gc::GAMEMODE_FILL_FROM_PLAYERS,
        doc: "Fill this array with all current players",
        lowering: generic(&[], P::ExecOut),
    },
    ArrayMethod {
        name: "fillFromTeam",
        mutates: true,
        gate: gc::GAMEMODE_FILL_FROM_TEAM,
        doc: "Fill this array with the members of a team",
        lowering: generic(&[val("team", P::Team, Entity)], P::ExecOut),
    },
    // `fillFromZone*` take only a `zone` rerouter reference, never a plain
    // entity, although the gate declares the input `entity` (the object wire a
    // `ZoneRef` connection carries). `TagFilter` is declared `any`.
    ArrayMethod {
        name: "fillFromZoneEntities",
        mutates: true,
        gate: gc::ZONE_GET_ENTITIES,
        doc: "Fill this array with the entities inside a zone",
        lowering: Lowering::Generic {
            args: &[
                val("zone", P::Zone, Zone),
                opt("tagFilter", P::TagFilter, Str),
            ],
            result: P::ExecOut,
            port_types: &[(P::TagFilter, Type::Any)],
        },
    },
    ArrayMethod {
        name: "fillFromZonePlayers",
        mutates: true,
        gate: gc::ZONE_GET_PLAYERS,
        doc: "Fill this array with the players inside a zone",
        lowering: Lowering::Generic {
            args: &[
                val("zone", P::Zone, Zone),
                opt("tagFilter", P::TagFilter, Str),
            ],
            result: P::ExecOut,
            port_types: &[(P::TagFilter, Type::Any)],
        },
    },
    // `sortMultiple` is variadic (up to 7 parallel arrays, then an optional
    // `descending?`); its empty params opt it out of `check_args`'s arity and
    // type checks.
    ArrayMethod {
        name: "sortMultiple",
        mutates: true,
        gate: gc::ARRAY_SORT_MULTIPLE,
        doc: "Sort this array and up to 7 parallel arrays together, by this array's order",
        lowering: Lowering::Custom,
    },
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
static ARRAY_METHOD_NAMES: std::sync::LazyLock<crate::collections::HashSet<&'static str>> =
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

/// Allocation-free [`field_name`], for the port-resolution paths that only
/// compare it against a field string.
pub fn field_name_ref(port: &str) -> &str {
    if let Some(rest) = port.strip_prefix('b')
        && rest.chars().next().is_some_and(|c| c.is_ascii_uppercase())
    {
        return rest;
    }
    port
}

/// The return type of `arr.<method>()` for an array of `elem`
/// ([`super::methods::method_return_type`]): `find` gives its `int` `Index`
/// first, the others their element `Value`. Returns `None` for unknown
/// methods.
pub fn array_return_type(method: &str, elem: &Type) -> Option<Type> {
    // A record element read (`pop`/`get`) yields the record itself, with no
    // `IsEmpty`/`OutOfBounds` flag; lowering fans it out per field, like the
    // record-valued map `get`.
    if matches!(method, "pop" | "get") && matches!(elem, Type::Record(_)) {
        return Some(Type::call_result(vec![("Value".into(), elem.clone())]));
    }
    let m = array_method(method)?;
    Some(super::methods::method_return_type(
        m.gate,
        &m.lowering,
        &ContainerKind::Array { elem: elem.clone() },
    ))
}

#[cfg(test)]
mod tests;
