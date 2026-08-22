    use super::*;
    use crate::ir::Type;
    use crate::typecheck::ParamKind;

    #[test]
    fn every_array_method_has_typed_params() {
        for m in ARRAY_METHODS {
            let sig = m.signature(&Type::Int); // must not panic
            assert!(sig.params.iter().all(|p| matches!(p.kind, ParamKind::Wire)),
                "array method {} has a non-wire param", m.name);
        }
    }

    /// Structural coverage guarantee: every method `array_methods()` lists
    /// round-trips through `array_method(name)` and has a constructible
    /// `signature` — `ArrayMethod::signature`'s match panics on a name with
    /// no arm, so a method added to `ARRAY_METHODS` without a matching
    /// `signature` arm fails this test instead of shipping with unchecked
    /// call arguments.
    #[test]
    fn every_array_method_has_a_signature() {
        for m in array_methods() {
            let found = array_method(m.name)
                .unwrap_or_else(|| panic!("no method enum for {}", m.name));
            assert_eq!(found.name, m.name);
            let _ = m.signature(&Type::Int);
        }
    }

    #[test]
    fn array_method_param_types_track_element() {
        let sig = array_method("push").unwrap().signature(&Type::Vector);
        assert_eq!(sig.params.len(), 1);
        assert_eq!(sig.params[0].ty, Type::Vector);                       // value: T
        let ins = array_method("insert").unwrap().signature(&Type::Int);
        assert_eq!(ins.params[0].ty, Type::Int);                          // index: int
        assert_eq!(ins.params[1].ty, Type::Int);                         // value: T (elem=int)
        let sl = array_method("slice").unwrap().signature(&Type::Int);
        assert_eq!(sl.params[0].ty, Type::Array(Box::new(Type::Int)));    // source: T[]
        let so = array_method("sort").unwrap().signature(&Type::Int);
        assert!(so.params[0].optional);                                   // descending?: bool
        // The `fillFromZone*` gates take a `zone` rerouter reference ONLY on
        // their Zone slot — never a plain entity.
        let fz = array_method("fillFromZoneEntities").unwrap().signature(&Type::Int);
        assert_eq!(fz.params[0].ty, Type::Zone);                          // zone: zone
        assert!(fz.params[1].optional);                                   // tagFilter?: string
        assert_eq!(fz.params[1].ty, Type::String);
    }
