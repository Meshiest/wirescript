//! Callable gate builtins — function-call forms of the wire-graph var/array/map
//! gates (`GetMapElement(m, k)`, `SetVariable(v, x)`, …), named after the
//! in-game gates. The parser desugars each to its method/assignment equivalent
//! (`parser::desugar_gate_call` / `parser::gate_builtin_assign`), so behaviour is
//! identical to the `m.get(k)` / `v = x` forms. This module is the single source
//! of truth for the names — the desugar tables and editor completions read it.

// ── Variables ──
pub const GET_VARIABLE: &str = "GetVariable";
pub const SET_VARIABLE: &str = "SetVariable";
pub const INCREMENT_VARIABLE: &str = "IncrementVariable";

// ── Arrays ──
pub const GET_ARRAY_ELEMENT: &str = "GetArrayElement";
pub const SET_ARRAY_ELEMENT: &str = "SetArrayElement";
pub const PUSH_TO_ARRAY: &str = "PushToArray";
pub const POP_FROM_ARRAY: &str = "PopFromArray";
pub const INSERT_ARRAY_ELEMENT: &str = "InsertArrayElement";
pub const REMOVE_ARRAY_ELEMENT: &str = "RemoveArrayElement";
pub const GET_ARRAY_LENGTH: &str = "GetArrayLength";
pub const FIND_ARRAY_ELEMENT: &str = "FindArrayElement";
pub const CLEAR_ARRAY: &str = "ClearArray";
pub const FILL_ARRAY: &str = "FillArray";
pub const RESIZE_ARRAY: &str = "ResizeArray";
pub const REVERSE_ARRAY: &str = "ReverseArray";
pub const SHUFFLE_ARRAY: &str = "ShuffleArray";
pub const SORT_ARRAY: &str = "SortArray";
pub const SWAP_ARRAY_ELEMENTS: &str = "SwapArrayElements";
pub const SLICE_ARRAY: &str = "SliceArray";
pub const APPEND_ARRAY: &str = "AppendArray";
pub const COPY_ARRAY: &str = "CopyArray";
pub const SUM_ARRAY: &str = "SumArray";
pub const AVERAGE_ARRAY: &str = "AverageArray";
pub const ARRAY_MAXIMUM: &str = "ArrayMaximum";
pub const ARRAY_MINIMUM: &str = "ArrayMinimum";

// ── Array fills (Gamemode / Zone gates that replace an array's contents) ──
pub const FILL_ARRAY_FROM_PLAYERS: &str = "FillArrayFromPlayers";
pub const FILL_ARRAY_FROM_TEAM_MEMBERS: &str = "FillArrayFromTeamMembers";
pub const GET_ENTITIES_IN_ZONE: &str = "GetEntitiesInZone";
pub const GET_PLAYERS_IN_ZONE: &str = "GetPlayersInZone";

// ── Maps ──
pub const GET_MAP_ELEMENT: &str = "GetMapElement";
pub const SET_MAP_ELEMENT: &str = "SetMapElement";
pub const HAS_MAP_ELEMENT: &str = "HasMapElement";
pub const REMOVE_MAP_ELEMENT: &str = "RemoveMapElement";
pub const CLEAR_MAP: &str = "ClearMap";
pub const COPY_MAP: &str = "CopyMap";
pub const GET_MAP_LENGTH: &str = "GetMapLength";
pub const GET_MAP_KEYS: &str = "GetMapKeys";
pub const GET_MAP_VALUES: &str = "GetMapValues";

/// The container METHOD an expression-form gate builtin desugars to —
/// `GetMapElement(m, k)` → `m.get(k)`, `PushToArray(a, v)` → `a.push(v)`. Returns
/// `None` for the statement-form set builtins (`SetVariable`/`SetArrayElement`/
/// `IncrementVariable`, handled as assignments) and for non-builtins.
pub fn method_for(name: &str) -> Option<&'static str> {
    Some(match name {
        // Arrays
        GET_ARRAY_ELEMENT => "get",
        PUSH_TO_ARRAY => "push",
        POP_FROM_ARRAY => "pop",
        INSERT_ARRAY_ELEMENT => "insert",
        REMOVE_ARRAY_ELEMENT => "remove",
        GET_ARRAY_LENGTH => "length",
        FIND_ARRAY_ELEMENT => "find",
        CLEAR_ARRAY => "clear",
        FILL_ARRAY => "fill",
        RESIZE_ARRAY => "resize",
        REVERSE_ARRAY => "reverse",
        SHUFFLE_ARRAY => "shuffle",
        SORT_ARRAY => "sort",
        SWAP_ARRAY_ELEMENTS => "swap",
        SLICE_ARRAY => "slice",
        APPEND_ARRAY => "append",
        COPY_ARRAY => "copyFrom",
        SUM_ARRAY => "sum",
        AVERAGE_ARRAY => "average",
        ARRAY_MAXIMUM => "max",
        ARRAY_MINIMUM => "min",
        // Array fills
        FILL_ARRAY_FROM_PLAYERS => "fillFromPlayers",
        FILL_ARRAY_FROM_TEAM_MEMBERS => "fillFromTeam",
        GET_ENTITIES_IN_ZONE => "fillFromZoneEntities",
        GET_PLAYERS_IN_ZONE => "fillFromZonePlayers",
        // Maps
        GET_MAP_ELEMENT => "get",
        SET_MAP_ELEMENT => "set",
        HAS_MAP_ELEMENT => "has",
        REMOVE_MAP_ELEMENT => "remove",
        CLEAR_MAP => "clear",
        COPY_MAP => "copyFrom",
        GET_MAP_LENGTH => "length",
        GET_MAP_KEYS => "keys",
        GET_MAP_VALUES => "values",
        _ => return None,
    })
}

/// Every callable gate-builtin name, for editor completion.
pub const ALL: &[&str] = &[
    GET_VARIABLE,
    SET_VARIABLE,
    INCREMENT_VARIABLE,
    GET_ARRAY_ELEMENT,
    SET_ARRAY_ELEMENT,
    PUSH_TO_ARRAY,
    POP_FROM_ARRAY,
    INSERT_ARRAY_ELEMENT,
    REMOVE_ARRAY_ELEMENT,
    GET_ARRAY_LENGTH,
    FIND_ARRAY_ELEMENT,
    CLEAR_ARRAY,
    FILL_ARRAY,
    RESIZE_ARRAY,
    REVERSE_ARRAY,
    SHUFFLE_ARRAY,
    SORT_ARRAY,
    SWAP_ARRAY_ELEMENTS,
    SLICE_ARRAY,
    APPEND_ARRAY,
    COPY_ARRAY,
    SUM_ARRAY,
    AVERAGE_ARRAY,
    ARRAY_MAXIMUM,
    ARRAY_MINIMUM,
    FILL_ARRAY_FROM_PLAYERS,
    FILL_ARRAY_FROM_TEAM_MEMBERS,
    GET_ENTITIES_IN_ZONE,
    GET_PLAYERS_IN_ZONE,
    GET_MAP_ELEMENT,
    SET_MAP_ELEMENT,
    HAS_MAP_ELEMENT,
    REMOVE_MAP_ELEMENT,
    CLEAR_MAP,
    COPY_MAP,
    GET_MAP_LENGTH,
    GET_MAP_KEYS,
    GET_MAP_VALUES,
];
