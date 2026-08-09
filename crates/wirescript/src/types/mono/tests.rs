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

    #[test]
    fn collect_derives_params_through_compound_shapes() {
        use crate::types::classes::variant_mask;

        // `Map<K, V>` param vs `Map<string, int>` arg -> K=string, V=int.
        let mut cm = Vec::new();
        collect(
            &Type::Map(
                Box::new(Type::Param("K".into())),
                Box::new(Type::Param("V".into())),
            ),
            &Type::Map(Box::new(Type::String), Box::new(Type::Int)),
            &mut cm,
        );
        let sm = solve(
            &cm,
            &[("K".into(), variant_mask()), ("V".into(), variant_mask())],
        )
        .unwrap();
        assert_eq!(sm.get("K"), Some(&Type::String));
        assert_eq!(sm.get("V"), Some(&Type::Int));

        // Tuple param vs concrete tuple, collected positionally.
        let mut ct = Vec::new();
        collect(
            &Type::Tuple(vec![Type::Param("A".into()), Type::Param("B".into())]),
            &Type::Tuple(vec![Type::Int, Type::Float]),
            &mut ct,
        );
        let st = solve(
            &ct,
            &[("A".into(), variant_mask()), ("B".into(), variant_mask())],
        )
        .unwrap();
        assert_eq!(st.get("A"), Some(&Type::Int));
        assert_eq!(st.get("B"), Some(&Type::Float));

        // Record param matches by field NAME (order-independent), and a
        // param-only field with no counterpart in the arg contributes nothing.
        let mut cr = Vec::new();
        collect(
            &Type::Record(vec![
                ("x".into(), Type::Param("T".into())),
                ("missing".into(), Type::Param("U".into())),
            ]),
            &Type::Record(vec![("y".into(), Type::Bool), ("x".into(), Type::Vector)]),
            &mut cr,
        );
        assert_eq!(cr, vec![Constraint::Eq("T".into(), Type::Vector)]);
    }

    #[test]
    fn substitute_rewrites_compound_shapes() {
        let mut s = Subst::new();
        s.insert("K".into(), Type::String);
        s.insert("V".into(), Type::Int);
        s.insert("A".into(), Type::Int);
        s.insert("B".into(), Type::Float);
        s.insert("T".into(), Type::Vector);

        // Map<K, V> -> Map<string, int>.
        assert_eq!(
            substitute(
                &Type::Map(
                    Box::new(Type::Param("K".into())),
                    Box::new(Type::Param("V".into())),
                ),
                &s,
            ),
            Type::Map(Box::new(Type::String), Box::new(Type::Int)),
        );
        // Tuple(A, B) -> Tuple(int, float).
        assert_eq!(
            substitute(
                &Type::Tuple(vec![Type::Param("A".into()), Type::Param("B".into())]),
                &s,
            ),
            Type::Tuple(vec![Type::Int, Type::Float]),
        );
        // Record { x: T } -> Record { x: vector }, field names preserved.
        assert_eq!(
            substitute(
                &Type::Record(vec![("x".into(), Type::Param("T".into()))]),
                &s,
            ),
            Type::Record(vec![("x".into(), Type::Vector)]),
        );
        // Union(T | float | Unbound) -> Union(vector | float | Unbound): each
        // option is rewritten, and a param missing from the subst is left as-is.
        assert_eq!(
            substitute(
                &Type::Union(vec![
                    Type::Param("T".into()),
                    Type::Float,
                    Type::Param("Unbound".into()),
                ]),
                &s,
            ),
            Type::Union(vec![
                Type::Vector,
                Type::Float,
                Type::Param("Unbound".into()),
            ]),
        );
    }
