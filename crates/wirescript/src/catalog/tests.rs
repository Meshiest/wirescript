    use super::*;

    #[test]
    fn default_catalog_loads() {
        let cat = default_catalog();
        assert!(cat.len() > 50, "catalog should have real entries");
    }

    #[test]
    fn lookup_by_class_roundtrips() {
        let cat = default_catalog();
        let g = cat
            .find_by_class("Component_Internal_Rerouter")
            .expect("rerouter exists in catalog");
        assert_eq!(g.brick_asset, "B_1x1_Reroute_Node");
    }

    #[test]
    fn config_field_enum_type_reads_schema() {
        // Enum-typed data fields resolve to their schema enum; non-enum and
        // unknown fields resolve to None. Enum member values/names come from the
        // schema (verified against exact values in the gate-config lowering tests).
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathEasing", "Function")
                .is_some(),
            "MathEasing.Function is an enum config field"
        );
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Exec_SweepSimple", "Direction")
                .is_some(),
            "SweepSimple.Direction is an enum config field"
        );
        // A bool data field is not an enum.
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathBlend", "bClampAlpha")
                .is_none()
        );
        // A field the struct does not have.
        assert!(
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathBlend", "Nonexistent")
                .is_none()
        );
        // Enum member lookups are schema-backed and consistent both ways.
        if let Some(et) =
            config_field_enum_type("BrickComponentType_WireGraph_Expr_MathEasing", "Function")
        {
            assert!(!enum_member_names(et).is_empty());
            if let Some(v) = enum_member_value(et, "Bounce") {
                assert!(enum_has_value(et, v));
            }
        }
        assert!(!enum_has_value("EBREasingFunction", 9999));
    }

    #[test]
    fn config_enum_for_named_arg_resolves_calls_and_events() {
        // Call path: SweepSimple's `direction` config → EBrickDirection.
        assert_eq!(
            config_enum_for_named_arg("SweepSimple", "direction"),
            Some("EBrickDirection")
        );
        // Event path: Clock's config is all bool/float — no enum.
        assert_eq!(config_enum_for_named_arg("Clock", "enabled"), None);
        // A wired input (not config) never resolves as a config enum.
        assert_eq!(config_enum_for_named_arg("Clock", "interval"), None);
        // Unknown callee / arg.
        assert_eq!(config_enum_for_named_arg("Nonexistent", "x"), None);
        assert_eq!(config_enum_for_named_arg("SweepSimple", "notAParam"), None);
    }

    #[test]
    fn enum_members_exclude_sentinel() {
        // EBRColorSpace: Linear=0, Srgb=1, Oklab=2, Hsv=3, _MAX=4 (sentinel dropped).
        let names = enum_member_names("EBRColorSpace");
        assert_eq!(names, vec!["Linear", "Srgb", "Oklab", "Hsv"]);
        assert_eq!(enum_member_value("EBRColorSpace", "Srgb"), Some(1));
        assert_eq!(enum_member_value("EBRColorSpace", "EBRColorSpace::Oklab"), Some(2));
        assert_eq!(enum_member_value("EBRColorSpace", "Nope"), None);
        assert!(enum_has_value("EBRColorSpace", 3));
        assert!(!enum_has_value("EBRColorSpace", 99));
        // The `_MAX` sentinel is not a selectable member — by name or by value.
        assert_eq!(enum_member_value("EBRColorSpace", "EBRColorSpace_MAX"), None);
        assert!(!enum_has_value("EBRColorSpace", 4)); // 4 == the sentinel value
        // EBrickDirection spells its sentinel bare `MAX` — also dropped.
        let dirs = enum_member_names("EBrickDirection");
        assert!(!dirs.iter().any(|d| d == "MAX"));
        assert_eq!(dirs.first().map(String::as_str), Some("X_Positive"));
        assert_eq!(enum_member_value("EBrickDirection", "MAX"), None);
    }

    #[test]
    fn every_gate_builtin_desugars_to_a_real_method_or_assignment() {
        use crate::catalog::{arrays, gate_builtins as gb, maps};
        // The set builtins carry no expression method (`method_for` returns
        // None) — they desugar to a statement assignment / variable access.
        let statement_forms = [
            gb::GET_VARIABLE,
            gb::SET_VARIABLE,
            gb::INCREMENT_VARIABLE,
            gb::SET_ARRAY_ELEMENT,
        ];
        let cat = default_catalog();
        for &name in gb::ALL {
            match gb::method_for(name) {
                Some(method) => {
                    // The desugared method must be a known array and/or map
                    // method (some names — get/length/clear/remove/copyFrom —
                    // are both), and each gate it lowers to must be a real
                    // catalog gate, never the `_Unsupported` placeholder.
                    let mut gates = Vec::new();
                    if let Some(m) = arrays::array_method(method) {
                        gates.push(m.gate);
                    }
                    if let Some(m) = maps::map_method(method) {
                        gates.push(m.gate);
                    }
                    assert!(
                        !gates.is_empty(),
                        "{name} desugars to unknown method {method:?}"
                    );
                    for gate in gates {
                        assert_ne!(
                            gate,
                            crate::ir::gate_class::UNSUPPORTED,
                            "{name} ({method}) lowers to _Unsupported"
                        );
                        assert!(
                            cat.find_by_class(gate).is_some(),
                            "{name} ({method}) gate {gate} is not a real catalog gate"
                        );
                    }
                }
                None => assert!(
                    statement_forms.contains(&name),
                    "{name} has no method_for mapping but isn't a known statement form"
                ),
            }
        }
    }
