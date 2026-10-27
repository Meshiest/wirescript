#[test]
fn type_param_displays_as_its_name() {
    assert_eq!(crate::ir::Type::Param("T".into()).to_string(), "T");
    assert_eq!(
        crate::ir::Type::Array(Box::new(crate::ir::Type::Param("T".into()))).to_string(),
        "T[]"
    );
}

#[test]
fn wire_port_from_name_round_trips_every_known_name() {
    use crate::ir::port_registry::WirePort;
    // Every registered port name maps back to a variant whose `as_str` is exactly that name,
    // so the enum and the name table cannot drift apart.
    for &name in WirePort::all_names() {
        assert_eq!(
            WirePort::from_name(name).as_str(),
            name,
            "from_name/as_str must round-trip for {name}"
        );
    }
    // The synthetic layout edge round-trips through its reserved name too,
    // even though it isn't part of `all_names()`.
    assert_eq!(WirePort::from_name("_Layout"), WirePort::Layout);
    assert_eq!(WirePort::Layout.as_str(), "_Layout");
}

#[test]
#[should_panic(expected = "unknown wire port")]
fn wire_port_from_name_unknown_panics() {
    // An unrecognized port name has no fallback variant: `from_name` rejects
    // it loudly rather than silently mapping to the wrong port.
    let _ = crate::ir::port_registry::WirePort::from_name("NotARealWirePort");
}

#[test]
fn walk_mut_visits_modules_in_walk_order() {
    use crate::ir::{Module, NodeId};
    let mut root = Module::new("root");
    for id in [7, 3, 5] {
        let mut chip = Module::new("chip");
        chip.chips.insert(NodeId(id + 100), Module::new("inner"));
        root.chips.insert(NodeId(id), chip);
    }
    let mut seen = Vec::new();
    root.walk(&mut |path, _| seen.push(path.to_vec()));
    let mut seen_mut = Vec::new();
    root.walk_mut(&mut |path, _| seen_mut.push(path.to_vec()));
    assert_eq!(seen.len(), 7);
    assert_eq!(seen, seen_mut);
    assert_eq!(seen[1], [NodeId(3)]);
    assert_eq!(seen[2], [NodeId(3), NodeId(103)]);
}
