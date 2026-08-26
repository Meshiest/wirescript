    use super::*;

    #[test]
    fn same_types_same() {
        assert_eq!(coerce(&Type::Int, &Type::Int), CoerceRule::Same);
    }
    #[test]
    fn any_is_universal() {
        assert_eq!(coerce(&Type::Any, &Type::Int), CoerceRule::Same);
        assert_eq!(coerce(&Type::Int, &Type::Any), CoerceRule::Same);
    }
    #[test]
    fn opaque_is_universal_like_any() {
        assert_eq!(coerce(&Type::Opaque, &Type::Int), CoerceRule::Same);
        assert_eq!(coerce(&Type::Int, &Type::Opaque), CoerceRule::Same);
        assert_eq!(coerce(&Type::Opaque, &Type::String), CoerceRule::Same);
    }
    #[test]
    fn record_auto_unwraps_to_matching_field() {
        let rec = Type::Record(vec![
            ("Index".into(), Type::Int),
            ("Found".into(), Type::Bool),
        ]);
        // unwraps to whichever field matches the target
        assert_ne!(coerce(&rec, &Type::Int), CoerceRule::Mismatch);
        assert_ne!(coerce(&rec, &Type::Bool), CoerceRule::Mismatch);
        // no field matches → still a mismatch
        assert_eq!(coerce(&rec, &Type::Vector), CoerceRule::Mismatch);
        // a record target is not unwrapped
        assert_eq!(coerce(&rec, &Type::Record(vec![])), CoerceRule::Mismatch);
    }
    #[test]
    fn entity_downcasts_to_character_and_controller() {
        // A Sweep's HitEntity (or any entity wire) can be a player — wires
        // carry plain object refs, so the downcast is implicit in-game.
        assert_eq!(coerce(&Type::Entity, &Type::Character), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Entity, &Type::Controller), CoerceRule::Coerce);
    }
    #[test]
    fn numeric_coerces_both_ways() {
        assert_eq!(coerce(&Type::Int, &Type::Float), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Float, &Type::Int), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Bool, &Type::Int), CoerceRule::Coerce);
    }
    #[test]
    fn pulsing_into_exec() {
        assert_eq!(coerce(&Type::Bool, &Type::Exec), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Int, &Type::Exec), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Vector, &Type::Exec), CoerceRule::Coerce);
    }
    #[test]
    fn string_via_format() {
        assert_eq!(coerce(&Type::Int, &Type::String), CoerceRule::ViaString);
        assert_eq!(coerce(&Type::Vector, &Type::String), CoerceRule::ViaString);
    }
    #[test]
    fn everything_primitive_casts_to_string() {
        for t in [
            Type::Bool,
            Type::Float,
            Type::Entity,
            Type::Character,
            Type::Controller,
            Type::Rotator,
            Type::Color,
        ] {
            assert_eq!(
                coerce(&t, &Type::String),
                CoerceRule::ViaString,
                "{t:?} should cast to string"
            );
        }
    }
    #[test]
    fn ref_invariance() {
        let r_int = Type::Ref(Box::new(Type::Int));
        let r_float = Type::Ref(Box::new(Type::Float));
        assert_eq!(coerce(&r_int, &r_float), CoerceRule::Mismatch);
        assert_eq!(coerce(&r_int, &r_int.clone()), CoerceRule::Same);
    }
    #[test]
    fn numeric_not_to_vector() {
        assert_eq!(coerce(&Type::Int, &Type::Vector), CoerceRule::Mismatch);
    }
    #[test]
    fn character_to_controller() {
        assert_eq!(coerce(&Type::Character, &Type::Controller), CoerceRule::Coerce);
    }
    #[test]
    fn controller_to_character() {
        assert_eq!(coerce(&Type::Controller, &Type::Character), CoerceRule::Coerce);
    }
    #[test]
    fn controller_to_entity() {
        assert_eq!(coerce(&Type::Controller, &Type::Entity), CoerceRule::Coerce);
    }
    #[test]
    fn rotator_and_quat_interconvert() {
        assert_eq!(coerce(&Type::Rotator, &Type::Quat), CoerceRule::Coerce);
        assert_eq!(coerce(&Type::Quat, &Type::Rotator), CoerceRule::Coerce);
    }
    #[test]
    fn character_pulses_to_exec() {
        assert_eq!(coerce(&Type::Character, &Type::Exec), CoerceRule::Coerce);
    }
    #[test]
    fn controller_pulses_to_exec() {
        assert_eq!(coerce(&Type::Controller, &Type::Exec), CoerceRule::Coerce);
    }
    #[test]
    fn string_coerces_to_bool() {
        // The coercion means exactly `s != ""` (empty false, everything
        // else true): lowering inserts a `CompareNotEqual(s, "")` gate at
        // every string-typed-source → bool-typed-destination wire, so this
        // rule just tells typecheck the pair is legal.
        assert_eq!(coerce(&Type::String, &Type::Bool), CoerceRule::Coerce);
    }
    #[test]
    fn bool_to_string_stays_via_format() {
        // bool → string still renders "true"/"false" text through the
        // format-text path, distinct from the string→bool truthiness rule
        // above.
        assert_eq!(coerce(&Type::Bool, &Type::String), CoerceRule::ViaString);
    }
    #[test]
    fn string_never_coerces_to_numeric() {
        // PIN: `String -> Bool` must not chain transitively into the numeric
        // family (String -> Bool -> Int would let a string masquerade as a
        // number). `coerce` is single-step by construction — it never routes
        // through an intermediate type — so the direct queries must stay
        // Mismatch forever.
        assert_eq!(coerce(&Type::String, &Type::Int), CoerceRule::Mismatch);
        assert_eq!(coerce(&Type::String, &Type::Float), CoerceRule::Mismatch);
    }
    #[test]
    fn widening_join_lattice() {
        // same type -> itself
        assert_eq!(widening_join(&Type::Int, &Type::Int), Some(Type::Int));
        // any/Opaque is neutral
        assert_eq!(widening_join(&Type::Any, &Type::Int), Some(Type::Int));
        assert_eq!(widening_join(&Type::Int, &Type::Opaque), Some(Type::Int));
        // numerics: bool < int < float, widest wins
        assert_eq!(widening_join(&Type::Bool, &Type::Int), Some(Type::Int));
        assert_eq!(widening_join(&Type::Int, &Type::Float), Some(Type::Float));
        assert_eq!(widening_join(&Type::Bool, &Type::Float), Some(Type::Float));
        assert_eq!(widening_join(&Type::Float, &Type::Bool), Some(Type::Float));
        // objects: character/controller < entity
        assert_eq!(widening_join(&Type::Character, &Type::Entity), Some(Type::Entity));
        assert_eq!(widening_join(&Type::Controller, &Type::Entity), Some(Type::Entity));
        assert_eq!(widening_join(&Type::Character, &Type::Controller), Some(Type::Entity));
        // rotator/quat -> canonical rotator
        assert_eq!(widening_join(&Type::Rotator, &Type::Quat), Some(Type::Rotator));
        assert_eq!(widening_join(&Type::Quat, &Type::Rotator), Some(Type::Rotator));
        // no common widening
        assert_eq!(widening_join(&Type::Int, &Type::Vector), None);
        assert_eq!(widening_join(&Type::String, &Type::Int), None);
        // compound types only join with themselves (structural mismatch -> None)
        let arr_int = Type::Array(Box::new(Type::Int));
        let arr_float = Type::Array(Box::new(Type::Float));
        assert_eq!(widening_join(&arr_int, &arr_int.clone()), Some(arr_int.clone()));
        assert_eq!(widening_join(&arr_int, &arr_float), None);
    }
    #[test]
    fn widening_join_all_folds_left_to_right() {
        // empty -> no join of nothing
        assert_eq!(widening_join_all(vec![]), None);
        // single element -> itself, no widening_join call needed
        assert_eq!(widening_join_all(vec![Type::Int]), Some(Type::Int));
        // multiple: same lattice as the pairwise fold
        assert_eq!(
            widening_join_all(vec![Type::Bool, Type::Int, Type::Float]),
            Some(Type::Float)
        );
        assert_eq!(
            widening_join_all(vec![Type::Character, Type::Controller, Type::Entity]),
            Some(Type::Entity)
        );
        // any/Opaque stay neutral mid-sequence
        assert_eq!(
            widening_join_all(vec![Type::Any, Type::Int, Type::Opaque]),
            Some(Type::Int)
        );
        // no common widening anywhere in the sequence -> None
        assert_eq!(widening_join_all(vec![Type::Int, Type::Float, Type::Vector]), None);
    }

    #[test]
    fn record_unwrap_only_drives_from_first_field() {
        // GetVelocity's `{Vector, Rotation}`: lowering always wires outputs[0]
        // (Vector), so a `rotator` target must NOT unwrap via the later Rotation
        // field — that produced a Vector-into-Rotation miscompile (P0-13).
        let vel = Type::Record(vec![
            ("Vector".into(), Type::Vector),
            ("Rotation".into(), Type::Rotator),
        ]);
        assert_eq!(coerce(&vel, &Type::Rotator), CoerceRule::Mismatch);
        // Field 0 still drives a valid unwrap.
        assert_eq!(coerce(&vel, &Type::Vector), CoerceRule::Same);
    }

    #[test]
    fn tuple_literal_never_scalar_unwraps() {
        // An index-keyed record is a tuple literal; `(1, "abc")` is not an int.
        let tup = Type::Record(vec![("0".into(), Type::Int), ("1".into(), Type::String)]);
        assert_eq!(coerce(&tup, &Type::Int), CoerceRule::Mismatch);
    }

    #[test]
    fn enum_type_is_nominal_and_joins_with_itself_only() {
        use crate::ir::Type;
        let opt_int = Type::Enum { name: "Option".into(), args: vec![Type::Int] };
        let opt_int2 = Type::Enum { name: "Option".into(), args: vec![Type::Int] };
        let opt_flt = Type::Enum { name: "Option".into(), args: vec![Type::Float] };
        let res_int = Type::Enum { name: "Result".into(), args: vec![Type::Int] };
        assert!(type_eq(&opt_int, &opt_int2));
        assert!(!type_eq(&opt_int, &opt_flt));
        assert!(!type_eq(&opt_int, &res_int));
        assert_eq!(widening_join(&opt_int, &opt_int2), Some(opt_int.clone()));
        assert_eq!(widening_join(&opt_int, &opt_flt), None);
        assert_eq!(format!("{opt_int}"), "Option<int>");
    }

    #[test]
    fn any_container_param_accepts_concrete_but_not_reverse() {
        use Type::*;
        let ai = || Array(Box::new(Int));
        let aa = || Array(Box::new(Any));
        // A concrete array flows into an `any[]` sink (the P0-17b trap).
        assert_eq!(coerce(&ai(), &aa()), CoerceRule::Same);
        // But an `any[]` does NOT narrow into a concrete `int[]` (backing
        // variant differs — would miscompile), and `int[]` != `float[]`.
        assert_eq!(coerce(&aa(), &ai()), CoerceRule::Mismatch);
        assert_eq!(
            coerce(&ai(), &Array(Box::new(Float))),
            CoerceRule::Mismatch
        );
        // Map<any, any> accepts a concrete map; concrete->concrete mismatch stays.
        let mii = Map(Box::new(Int), Box::new(Int));
        let maa = Map(Box::new(Any), Box::new(Any));
        assert_eq!(coerce(&mii, &maa), CoerceRule::Same);
        assert_eq!(
            coerce(&mii, &Map(Box::new(Float), Box::new(Int))),
            CoerceRule::Mismatch
        );
    }
