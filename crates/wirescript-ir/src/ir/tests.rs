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
        // Every registered port name maps back to a variant whose `as_str` is
        // exactly that name — the enum and the name table can't drift apart.
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
