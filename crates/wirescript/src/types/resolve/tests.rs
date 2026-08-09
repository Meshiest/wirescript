    use super::*;
    use crate::diagnostic::{Pos, SourceRange};

    fn range() -> SourceRange {
        SourceRange::new("test.ws", Pos::default(), Pos::default())
    }

    fn name_te(name: &str) -> TypeExpr {
        TypeExpr::Name {
            name: name.to_string(),
            range: range(),
        }
    }

    fn array_te(inner: &str) -> TypeExpr {
        TypeExpr::Array {
            inner: Box::new(name_te(inner)),
            range: range(),
        }
    }

    #[test]
    fn resolver_handles_primitives_params_and_refs() {
        let params = vec!["T".to_string()];
        let aliases: HashMap<String, Type> = HashMap::default();
        let generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        assert_eq!(resolve_type(&name_te("zone"), &cx, &mut d), Type::Zone);
        assert_eq!(
            resolve_type(&name_te("teleport"), &cx, &mut d),
            Type::Teleport
        );
        assert_eq!(
            resolve_type(&name_te("T"), &cx, &mut d),
            Type::Param("T".into())
        );
        assert_eq!(
            resolve_type(&array_te("T"), &cx, &mut d),
            Type::Array(Box::new(Type::Param("T".into())))
        );
        assert!(d.is_empty(), "known names emit no diagnostics");
    }

    #[test]
    fn resolver_reports_unknown_names_and_generics() {
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        assert_eq!(resolve_type(&name_te("Bogus"), &cx, &mut d), Type::Any);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].code, "WS002");
        assert!(d[0].message.contains("unknown type 'Bogus'"));
    }

    #[test]
    fn resolver_resolves_type_aliases() {
        let params: Vec<String> = Vec::new();
        let mut aliases: HashMap<String, Type> = HashMap::default();
        aliases.insert("Point".to_string(), Type::Vector);
        let generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        assert_eq!(resolve_type(&name_te("Point"), &cx, &mut d), Type::Vector);
        assert!(d.is_empty());
    }

    fn generic_te(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Generic {
            name: name.to_string(),
            args,
            range: range(),
        }
    }

    fn record_te(fields: &[(&str, TypeExpr)]) -> TypeExpr {
        TypeExpr::Record {
            fields: fields
                .iter()
                .map(|(n, t)| crate::ast::RecordTypeField {
                    name: n.to_string(),
                    typ: t.clone(),
                    range: range(),
                })
                .collect(),
            range: range(),
        }
    }

    #[test]
    fn resolver_instantiates_generic_alias() {
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let mut generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        // type Pair<T> = { a: T, b: T }
        generic_aliases.insert(
            "Pair".to_string(),
            GenericAlias {
                params: vec!["T".to_string()],
                body: record_te(&[("a", name_te("T")), ("b", name_te("T"))]),
            },
        );
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        // Pair<int> -> { a: int, b: int }
        let resolved = resolve_type(&generic_te("Pair", vec![name_te("int")]), &cx, &mut d);
        assert_eq!(
            resolved,
            Type::Record(vec![("a".into(), Type::Int), ("b".into(), Type::Int)])
        );
        assert!(d.is_empty(), "clean instantiation emits no diagnostics: {d:?}");

        // Bare `Pair` (no args) errors — not fully applied.
        let mut d2 = Vec::new();
        assert_eq!(resolve_type(&name_te("Pair"), &cx, &mut d2), Type::Any);
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].code, "WS002");

        // Wrong arity errors.
        let mut d3 = Vec::new();
        let bad = generic_te("Pair", vec![name_te("int"), name_te("float")]);
        assert_eq!(resolve_type(&bad, &cx, &mut d3), Type::Any);
        assert_eq!(d3.len(), 1);
        assert_eq!(d3[0].code, "WS002");
    }

    #[test]
    fn resolver_rejects_recursive_generic_alias() {
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let mut generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        // type L<T> = { head: T, tail: L<T> }
        generic_aliases.insert(
            "L".to_string(),
            GenericAlias {
                params: vec!["T".to_string()],
                body: record_te(&[
                    ("head", name_te("T")),
                    ("tail", generic_te("L", vec![name_te("T")])),
                ]),
            },
        );
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        // Terminates (cut off by the in-progress cycle guard) rather than
        // hanging, and flags the self-reference. The returned `Type` isn't the
        // contract here, only that resolution terminates and reports an error.
        let _resolved = resolve_type(&generic_te("L", vec![name_te("int")]), &cx, &mut d);
        assert!(
            d.iter().any(|diag| diag.code == "WS002"),
            "recursive alias should be flagged, not hang: {d:?}"
        );
    }

    #[test]
    fn resolver_rejects_doubly_recursive_generic_alias() {
        // A body that references its own instantiation TWICE would re-expand
        // each occurrence, so a depth-only guard blows up as 2^depth before the
        // cap trips. The in-progress cycle guard must cut this at the first
        // re-entry, keeping it linear and terminating promptly.
        let params: Vec<String> = Vec::new();
        let aliases: HashMap<String, Type> = HashMap::default();
        let mut generic_aliases: HashMap<String, GenericAlias> = HashMap::default();
        // type Tree<T> = { l: Tree<T>, r: Tree<T> }
        generic_aliases.insert(
            "Tree".to_string(),
            GenericAlias {
                params: vec!["T".to_string()],
                body: record_te(&[
                    ("l", generic_te("Tree", vec![name_te("T")])),
                    ("r", generic_te("Tree", vec![name_te("T")])),
                ]),
            },
        );
        let cx = ResolveCtx {
            params: &params,
            type_aliases: &aliases,
            generic_aliases: &generic_aliases,
        };
        let mut d = Vec::new();
        let _resolved = resolve_type(&generic_te("Tree", vec![name_te("int")]), &cx, &mut d);
        assert!(
            d.iter().any(|diag| diag.code == "WS002"),
            "doubly-recursive alias should be flagged, not hang: {d:?}"
        );
    }
