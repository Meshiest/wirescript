    use super::{receiver_methods, split_signature_params};

    #[test]
    fn string_receiver_methods_are_string_only() {
        let names: Vec<&str> = receiver_methods("string").iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"Contains"), "string should have Contains: {names:?}");
        assert!(names.contains(&"Length"), "string should have Length");
        // vector/entity methods must not appear on a string.
        assert!(!names.contains(&"Dot"), "Dot leaked onto string");
        assert!(!names.contains(&"GetAim"), "GetAim leaked onto string");
    }

    #[test]
    fn unknown_type_has_no_methods() {
        assert!(receiver_methods("{ x: int }").is_empty());
        assert!(receiver_methods("nonsense").is_empty());
    }

    #[test]
    fn quat_and_color_receiver_methods() {
        let quat: Vec<&str> = receiver_methods("quat").iter().map(|(n, _)| *n).collect();
        for m in ["ToDirection", "Invert", "AngleTo", "Slerp", "ToAxisAngle"] {
            assert!(quat.contains(&m), "quat should have {m}: {quat:?}");
        }
        let color: Vec<&str> = receiver_methods("color").iter().map(|(n, _)| *n).collect();
        for m in ["ToHex", "ToSRGB", "Blend"] {
            assert!(color.contains(&m), "color should have {m}: {color:?}");
        }
        // A vector exposes the direction→quat conversions but not quat-only ops.
        let vector: Vec<&str> = receiver_methods("vector").iter().map(|(n, _)| *n).collect();
        assert!(vector.contains(&"ToRotation"), "vector should have ToRotation");
        assert!(!vector.contains(&"Slerp"), "Slerp leaked onto vector");
    }

    #[test]
    fn split_signature_params_handles_generics_records_and_returns() {
        // Plain params + return.
        let (n, t, rest) = split_signature_params("(self: vector, o: vector) -> float").unwrap();
        assert_eq!((n, t, rest.as_str()), ("self", "vector", "o: vector"));
        // Leading generic prefix is skipped; single param leaves an empty rest.
        let (n, t, rest) = split_signature_params("<T>(self: T) -> T").unwrap();
        assert_eq!((n, t, rest.as_str()), ("self", "T", ""));
        // Nested commas/colons inside a record or generic type stay with param 0.
        let (n, t, rest) =
            split_signature_params("(self: { x: int, y: int }, k: Map<string, int>)").unwrap();
        assert_eq!(n, "self");
        assert_eq!(t, "{ x: int, y: int }");
        assert_eq!(rest, "k: Map<string, int>");
        // No params → None.
        assert!(split_signature_params("() -> int").is_none());
    }

    #[test]
    fn user_receiver_methods_match_by_type() {
        use super::user_receiver_methods;
        use crate::analysis::symbols::collect_symbols_for_file;
        use crate::resolve::{resolve, FsLoader};
        let src = "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
                   mod inc(self: int) -> int { return self + 1 }\n\
                   mod plain(a: vector) -> float { return 0.0 }\n";
        let resolved = resolve(src, "test", &FsLoader);
        let tc = crate::typecheck::typecheck(&resolved.ast, "test", &crate::typecheck::CeSlotMap::default());
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"));

        let on_vec: Vec<String> = user_receiver_methods("vector", &symbols)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(on_vec.contains(&"dist".to_string()), "vector should offer dist: {on_vec:?}");
        assert!(!on_vec.contains(&"inc".to_string()), "int-receiver inc must not appear on vector");
        assert!(!on_vec.contains(&"plain".to_string()), "a non-self mod must never appear");

        let on_int: Vec<String> = user_receiver_methods("int", &symbols)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(on_int.contains(&"inc".to_string()), "int should offer inc: {on_int:?}");
        assert!(!on_int.contains(&"dist".to_string()), "vector-receiver dist must not appear on int");
    }

    #[test]
    fn collection_kind_resolves_direct_and_aliased() {
        use super::{collection_kind, CollectionKind};
        let sd = |name: &str, ty: &str| crate::analysis::SymbolDef {
            name: name.into(),
            kind: "type",
            range: Default::default(),
            ty: Some(ty.to_string()),
            exec: false,
            is_const: false,
        };
        let syms = vec![
            sd("Scores", "Map<string, int>"),
            sd("Ids", "int[]"),
            sd("A", "B"),                 // alias chain A -> B -> Map
            sd("B", "Map<int, int>"),
            sd("Grid", "Map<string, T>"), // GENERIC alias: body keeps the param T
            sd("Cyc", "Cyc"),             // self-referential alias must not loop
        ];
        let map = Some(CollectionKind::Map);
        let arr = Some(CollectionKind::Array);
        // Direct annotations, both spellings.
        assert_eq!(collection_kind("Map<string, int>", &syms), map);
        assert_eq!(collection_kind("int[]", &syms), arr);
        assert_eq!(collection_kind("Array<int>", &syms), arr);
        // Single-hop aliases.
        assert_eq!(collection_kind("Scores", &syms), map);
        assert_eq!(collection_kind("Ids", &syms), arr);
        // Multi-hop alias chain.
        assert_eq!(collection_kind("A", &syms), map);
        // Generic-alias INSTANCE resolves by base name.
        assert_eq!(collection_kind("Grid<int>", &syms), map);
        // Non-collection and cycle both yield None (no hang).
        assert_eq!(collection_kind("int", &syms), None);
        assert_eq!(collection_kind("Cyc", &syms), None);
    }
