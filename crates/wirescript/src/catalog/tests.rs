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
    fn config_referenced_enum_types_covers_the_known_config_enums() {
        let types = super::config_referenced_enum_types();
        assert!(!types.is_empty());
        // The DisplayText enums are named `EBRDisplayTextJustification` /
        // `EBRTextTypeface` in the bundled schema (verified against
        // `brdb/crates/brdb/schemas/BRSavedComponentChunkSoA_max.schema`); there
        // is no `EBRJustification` / `EBRTypeface`, so those exact names are
        // what this test asserts against.
        for want in [
            "EBREasingFunction",
            "EBrickDirection",
            "EBRColorSpace",
            "EBRDisplayTextJustification",
            "EBRTextTypeface",
        ] {
            assert!(types.contains(&want), "missing {want} in {types:?}");
        }
        // Deduped and sorted.
        let mut sorted = types.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, types);
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

    #[test]
    fn clean_names_follow_the_rules() {
        assert_eq!(super::clean_game_enum_type("EBREasingFunction"), "EasingFunction");
        assert_eq!(super::clean_game_enum_type("EBrickDirection"), "Direction");
        assert_eq!(super::clean_game_enum_type("EBRColorSpace"), "ColorSpace");
        assert_eq!(super::clean_game_enum_variant("X_Positive"), "XPositive");
        assert_eq!(super::clean_game_enum_variant("Linear"), "Linear");
    }

    #[test]
    fn builtin_game_enums_are_well_formed_and_unique() {
        let enums = super::builtin_game_enums();
        assert!(!enums.is_empty());

        // Clean type names are unique (no silent collision).
        let mut names: Vec<_> = enums.iter().map(|e| e.clean_name.clone()).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "two schema enums cleaned to the same name");

        // A cleaned name must be a usable Wirescript identifier: non-empty and
        // starting with a letter (never empty or digit-leading), so a future
        // schema enum whose cleaning degenerates fails loud here.
        fn is_identifier_start(name: &str) -> bool {
            name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        }

        for e in &enums {
            assert!(is_identifier_start(&e.clean_name), "bad clean type name {:?}", e.clean_name);
            // Each variant carries the schema's real integer as its discriminant.
            for v in &e.variants {
                assert!(is_identifier_start(&v.clean_name), "bad clean variant {:?}", v.clean_name);
                assert_eq!(super::enum_member_value(e.schema_type, &v.raw_member), Some(v.disc));
            }
            // Variant clean names are unique within the enum.
            let mut vs: Vec<_> = e.variants.iter().map(|v| v.clean_name.clone()).collect();
            let vc = vs.len();
            vs.sort();
            vs.dedup();
            assert_eq!(vs.len(), vc, "duplicate cleaned variant in {}", e.clean_name);
            assert_eq!(super::game_enum_schema_type(&e.clean_name), Some(e.schema_type));
        }

        // A concrete anchor: EasingFunction exists with a Bounce variant.
        let easing = enums.iter().find(|e| e.clean_name == "EasingFunction").expect("EasingFunction");
        assert!(easing.variants.iter().any(|v| v.clean_name == "Bounce"));
    }
