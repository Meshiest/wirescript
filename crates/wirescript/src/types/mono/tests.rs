    use super::*;
    use crate::ir::Type;

    #[test]
    fn collect_and_substitute_roundtrip() {
        // `pick<T>(a: T, b: T) -> T` called with ints.
        let mut cs = Vec::new();
        collect(&Type::Param("T".into()), &Type::Int, &mut cs);
        collect(&Type::Param("T".into()), &Type::Int, &mut cs);
        let s = solve(&cs, &[("T".into(), crate::types::classes::variant_mask())]).unwrap();
        assert_eq!(substitute(&Type::Param("T".into()), &s), Type::Int);
        // Compound: `*T` / `T[]`.
        assert_eq!(
            substitute(&Type::Array(Box::new(Type::Param("T".into()))), &s),
            Type::Array(Box::new(Type::Int))
        );
    }

    #[test]
    fn infer_call_subst_ref_aligns() {
        // `mod inc(v: *T)` called with an int var (infers to `int`, not `*int`).
        let s = infer_call_subst(
            &[Type::Ref(Box::new(Type::Param("T".into())))],
            &[Type::Int],
            &[("T".into(), crate::types::classes::variant_mask())],
        );
        assert_eq!(s.get("T"), Some(&Type::Int));
    }

    #[test]
    fn mask_for_param_classes_and_unbounded() {
        let aliases = HashMap::default();
        assert_eq!(
            mask_for_param(None, &aliases),
            crate::types::classes::variant_mask()
        );
        assert_eq!(
            mask_for_param(
                Some(&TypeExpr::Name {
                    name: "Scalar".into(),
                    range: crate::diagnostic::SourceRange::default(),
                }),
                &aliases
            ),
            vec![Type::Int, Type::Float]
        );
    }
