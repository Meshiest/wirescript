//! Callable gate builtins — function-call forms of the wire-graph var/array/map
//! gates (`GetMapElement(m, k)`, `SetVariable(v, x)`, …), named after the
//! in-game gates. The parser desugars each to its method/assignment equivalent
//! (`parser::desugar_gate_call` / `parser::gate_builtin_assign`), so behaviour is
//! identical to the `m.get(k)` / `v = x` forms. This module is the single source
//! of truth for the names — the desugar tables and editor completions read it.

/// The builtins that read or write a variable or an array element, which the
/// parser turns into a read or an assignment rather than a method call.
pub const GET_VARIABLE: &str = "GetVariable";
pub const SET_VARIABLE: &str = "SetVariable";
pub const INCREMENT_VARIABLE: &str = "IncrementVariable";
pub const SET_ARRAY_ELEMENT: &str = "SetArrayElement";

/// Each expression-form builtin and the container method it desugars to:
/// `GetMapElement(m, k)` -> `m.get(k)`, `PushToArray(a, v)` -> `a.push(v)`.
const METHODS: &[(&str, &str)] = &[
    // Arrays
    ("GetArrayElement", "get"),
    ("PushToArray", "push"),
    ("PopFromArray", "pop"),
    ("InsertArrayElement", "insert"),
    ("RemoveArrayElement", "remove"),
    ("GetArrayLength", "length"),
    ("FindArrayElement", "find"),
    ("ClearArray", "clear"),
    ("FillArray", "fill"),
    ("ResizeArray", "resize"),
    ("ReverseArray", "reverse"),
    ("ShuffleArray", "shuffle"),
    ("SortArray", "sort"),
    ("SwapArrayElements", "swap"),
    ("SliceArray", "slice"),
    ("AppendArray", "append"),
    ("CopyArray", "copyFrom"),
    ("SumArray", "sum"),
    ("AverageArray", "average"),
    ("ArrayMaximum", "max"),
    ("ArrayMinimum", "min"),
    ("JoinArray", "join"),
    // Array fills
    ("FillArrayFromPlayers", "fillFromPlayers"),
    ("FillArrayFromTeamMembers", "fillFromTeam"),
    ("GetEntitiesInZone", "fillFromZoneEntities"),
    ("GetPlayersInZone", "fillFromZonePlayers"),
    // Maps
    ("GetMapElement", "get"),
    ("SetMapElement", "set"),
    ("HasMapElement", "has"),
    ("RemoveMapElement", "remove"),
    ("ClearMap", "clear"),
    ("CopyMap", "copyFrom"),
    ("GetMapLength", "length"),
    ("GetMapKeys", "keys"),
    ("GetMapValues", "values"),
];

/// The container METHOD an expression-form gate builtin desugars to. Returns
/// `None` for the statement-form set builtins (`SetVariable`/`SetArrayElement`/
/// `IncrementVariable`, handled as assignments) and for non-builtins.
pub fn method_for(name: &str) -> Option<&'static str> {
    METHODS.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
}

/// A descriptive usage hint for a STATEMENT-form gate builtin (`parser::
/// gate_builtin_assign`), shown when one fails to desugar — used as a value, or
/// with the wrong number of arguments (e.g. still being typed). Completion
/// offers these names, so recognizing them here (rather than reporting a bare
/// "unknown identifier") keeps the error sensible. The hint only DESCRIBES the
/// builtin — it never guesses which specific mistake was made, so it reads right
/// whether the call was a value misuse or an incomplete statement. `None` for
/// the expression-form builtins (which desugar to methods and CAN be
/// expressions) and for non-builtins.
pub fn statement_usage_hint(name: &str) -> Option<&'static str> {
    Some(match name {
        SET_VARIABLE => {
            "`SetVariable(variable, value)` is an exec statement that sets a variable \
                         (shorthand for `v = x`); it has no return value"
        }
        SET_ARRAY_ELEMENT => {
            "`SetArrayElement(array, index, value)` is an exec statement that sets \
                              an array element (shorthand for `a[i] = x`); it has no return value"
        }
        INCREMENT_VARIABLE => {
            "`IncrementVariable(variable, amount)` is an exec statement that adds \
                               to a variable (shorthand for `v = v + x`); it has no return value"
        }
        _ => return None,
    })
}

/// Every callable gate-builtin name, for editor completion.
pub fn all() -> impl Iterator<Item = &'static str> {
    [
        GET_VARIABLE,
        SET_VARIABLE,
        INCREMENT_VARIABLE,
        SET_ARRAY_ELEMENT,
    ]
    .into_iter()
    .chain(METHODS.iter().map(|(n, _)| *n))
}
