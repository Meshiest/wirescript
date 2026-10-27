//! Callable gate builtins: function-call forms of the var/array/map gates
//! (`GetMapElement(m, k)`, `SetVariable(v, x)`, and so on), named after the
//! in-game gates. The parser desugars each to its method or assignment form
//! (`parser::desugar_gate_call` / `parser::gate_builtin_assign`). The desugar
//! tables and editor completions read the names from here.

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

/// A usage hint for a statement-form gate builtin that failed to desugar (used
/// as a value, or with the wrong number of arguments). It describes the builtin
/// rather than guessing the mistake. `None` for the expression-form builtins
/// and for non-builtins.
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
