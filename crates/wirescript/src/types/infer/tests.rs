    use super::*;
    use crate::ir::Type;
    fn num() -> Vec<Type> {
        crate::types::classes::class_mask("Numeric").unwrap()
    }
    fn variant() -> Vec<Type> {
        crate::types::classes::variant_mask()
    }
    #[test]
    fn solver_widens() {
        // agreement pins T
        let s = solve(
            &[Constraint::Eq("T".into(), Type::Int), Constraint::Eq("T".into(), Type::Int)],
            &[("T".into(), num())],
        )
        .unwrap();
        assert_eq!(s["T"], Type::Int);
        // int + float widens to float (not a Conflict — was under strict equality)
        let s = solve(
            &[Constraint::Eq("T".into(), Type::Int), Constraint::Eq("T".into(), Type::Float)],
            &[("T".into(), num())],
        )
        .unwrap();
        assert_eq!(s["T"], Type::Float);
        // bool + int widens to int
        let s = solve(
            &[Constraint::Eq("T".into(), Type::Bool), Constraint::Eq("T".into(), Type::Int)],
            &[("T".into(), num())],
        )
        .unwrap();
        assert_eq!(s["T"], Type::Int);
        // character + entity widens to entity
        let s = solve(
            &[
                Constraint::Eq("T".into(), Type::Character),
                Constraint::Eq("T".into(), Type::Entity),
            ],
            &[("T".into(), variant())],
        )
        .unwrap();
        assert_eq!(s["T"], Type::Entity);
        // int + vector: no common widening -> Conflict
        assert!(matches!(
            solve(
                &[Constraint::Eq("T".into(), Type::Int), Constraint::Eq("T".into(), Type::Vector)],
                &[("T".into(), num())]
            ),
            Err(InferError::Conflict { .. })
        ));
        // `any`/Opaque contributes nothing -> the int pins it
        let s = solve(
            &[Constraint::Eq("T".into(), Type::Opaque), Constraint::Eq("T".into(), Type::Int)],
            &[("T".into(), num())],
        )
        .unwrap();
        assert_eq!(s["T"], Type::Int);
        // all-any -> unpinnable
        assert!(matches!(
            solve(&[Constraint::Eq("T".into(), Type::Opaque)], &[("T".into(), num())]),
            Err(InferError::Unpinnable(_))
        ));
        // no constraint at all -> unpinnable
        assert!(matches!(
            solve(&[], &[("T".into(), num())]),
            Err(InferError::Unpinnable(_))
        ));
        // out of mask (string not Numeric)
        assert!(matches!(
            solve(&[Constraint::Eq("T".into(), Type::String)], &[("T".into(), num())]),
            Err(InferError::OutOfMask { .. })
        ));
        // two independent vars both solve
        let s = solve(
            &[Constraint::Eq("T".into(), Type::Int), Constraint::Eq("U".into(), Type::Vector)],
            &[("T".into(), num()), ("U".into(), num())],
        )
        .unwrap();
        assert_eq!(s["T"], Type::Int);
        assert_eq!(s["U"], Type::Vector);
    }
