//! How a gate uses the storage its ref ports name: the one table both the
//! source-order analysis (`lower::ordered`) and the constant-container checks
//! read.

use crate::ir::gate_class as gc;
use crate::ir::port_registry::WirePort;

/// What a gate does with the storage one of its ports names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// The kind of storage a port names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Var,
    Array,
    Map,
}

/// The kind of storage a ref port names, or `None` for a port that names none.
/// A copy's `SourceRef` names an array or a map by the gate it sits on.
pub fn port_kind(class: &str, port: WirePort) -> Option<Kind> {
    Some(match port {
        WirePort::VarRef => Kind::Var,
        WirePort::ArrayVarRef
        | WirePort::ArrayVarRef2
        | WirePort::ArrayVarRef3
        | WirePort::ArrayVarRef4
        | WirePort::ArrayVarRef5
        | WirePort::ArrayVarRef6
        | WirePort::ArrayVarRef7
        | WirePort::ArrayVarRef8 => Kind::Array,
        WirePort::MapVarRef => Kind::Map,
        WirePort::SourceRef if class == gc::MAP_COPY_FROM => Kind::Map,
        WirePort::SourceRef => Kind::Array,
        _ => return None,
    })
}

/// Gates that only read every storage they name.
pub const READERS: &[&str] = &[
    gc::VAR_GET,
    gc::ARRAY_GET,
    gc::ARRAY_GET_LENGTH,
    gc::ARRAY_FIND,
    gc::ARRAY_SUM,
    gc::ARRAY_MIN,
    gc::ARRAY_MAX,
    gc::ARRAY_AVERAGE,
    gc::ARRAY_JOIN,
    gc::MAP_GET,
    gc::MAP_HAS,
    gc::MAP_GET_LENGTH,
    gc::MICROCHIP_FOR_EACH_ARRAY,
    gc::MICROCHIP_FOR_EACH_MAP,
];

/// Gates that write storage they name. `SourceRef` is only read, and a map's
/// keys or values gate reads its map and writes the array it fills.
pub const WRITERS: &[&str] = &[
    gc::VAR_SET,
    gc::VAR_INCREMENT,
    gc::ARRAY_APPEND,
    gc::ARRAY_CLEAR,
    gc::ARRAY_COPY_FROM,
    gc::ARRAY_FILL,
    gc::ARRAY_INSERT,
    gc::ARRAY_POP,
    gc::ARRAY_PUSH,
    gc::ARRAY_REMOVE_AT_INDEX,
    gc::ARRAY_RESIZE,
    gc::ARRAY_REVERSE,
    gc::ARRAY_SET_AT_INDEX,
    gc::ARRAY_SHUFFLE,
    gc::ARRAY_SLICE,
    gc::ARRAY_SORT,
    gc::ARRAY_SORT_MULTIPLE,
    gc::ARRAY_SWAP,
    gc::GAMEMODE_FILL_FROM_PLAYERS,
    gc::GAMEMODE_FILL_FROM_TEAM,
    gc::MAP_CLEAR,
    gc::MAP_COPY_FROM,
    gc::MAP_GET_KEYS,
    gc::MAP_GET_VALUES,
    gc::MAP_REMOVE,
    gc::MAP_SET,
    gc::ZONE_GET_ENTITIES,
    gc::ZONE_GET_PLAYERS,
];

/// How a gate of `class` uses the storage its `port` names, or `None` when the
/// port names no storage. A gate neither list names writes what it names.
pub fn access(class: &str, port: WirePort) -> Option<Access> {
    port_kind(class, port)?;
    let read = port == WirePort::SourceRef
        || READERS.contains(&class)
        || (matches!(class, gc::MAP_GET_KEYS | gc::MAP_GET_VALUES) && port == WirePort::MapVarRef);
    Some(if read { Access::Read } else { Access::Write })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every gate in the inventory with a storage port is listed as a reader
    /// or a writer, so a new one is classified on purpose.
    #[test]
    fn every_storage_gate_is_classified() {
        let mut missing = Vec::new();
        for g in super::super::default_catalog().entries() {
            let class = g.component.class.as_str();
            let names = g.component.inputs.iter().any(|p| {
                WirePort::all_names().contains(&p.name.as_str())
                    && port_kind(class, WirePort::from_name(&p.name)).is_some()
            });
            if names && !READERS.contains(&class) && !WRITERS.contains(&class) {
                missing.push(class.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "storage gates not classified: {missing:?}"
        );
    }

    #[test]
    fn readers_and_writers() {
        assert_eq!(
            access(gc::MICROCHIP_FOR_EACH_ARRAY, WirePort::ArrayVarRef),
            Some(Access::Read)
        );
        assert_eq!(
            access(gc::ARRAY_APPEND, WirePort::SourceRef),
            Some(Access::Read)
        );
        assert_eq!(
            access(gc::ARRAY_APPEND, WirePort::ArrayVarRef),
            Some(Access::Write)
        );
        assert_eq!(
            access(gc::MAP_GET_KEYS, WirePort::MapVarRef),
            Some(Access::Read)
        );
        assert_eq!(
            access(gc::MAP_GET_KEYS, WirePort::ArrayVarRef),
            Some(Access::Write)
        );
        assert_eq!(
            access("BrickComponentType_Unknown", WirePort::ArrayVarRef),
            Some(Access::Write)
        );
        assert_eq!(access(gc::VAR_SET, WirePort::Value), None);
    }
}
