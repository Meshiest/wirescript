use super::*;
use crate::ir::Type;
use crate::typecheck::ParamKind;

#[test]
fn every_map_method_has_typed_params() {
    for m in MAP_METHODS {
        let sig = m.signature(&Type::String, &Type::Int); // must not panic
        assert!(
            sig.params.iter().all(|p| matches!(p.kind, ParamKind::Wire)),
            "map method {} has a non-wire param",
            m.name
        );
    }
}

/// Structural coverage guarantee: every method `map_methods()` lists
/// round-trips through `map_method(name)` and has a constructible
/// `signature` — `MapMethod::signature`'s match panics on a name with no
/// arm, so a method added to `MAP_METHODS` without a matching `signature`
/// arm fails this test instead of shipping with unchecked call arguments.
#[test]
fn every_map_method_has_a_signature() {
    for m in map_methods() {
        let found =
            map_method(m.name).unwrap_or_else(|| panic!("no method enum for {}", m.name));
        assert_eq!(found.name, m.name);
        let _ = m.signature(&Type::String, &Type::Int);
    }
}

#[test]
fn map_method_param_types_track_kv() {
    let set = map_method("set").unwrap().signature(&Type::String, &Type::Entity);
    assert_eq!(set.params.len(), 2);
    assert_eq!(set.params[0].ty, Type::String); // key: K
    assert_eq!(set.params[1].ty, Type::Entity); // value: V

    let get = map_method("get").unwrap().signature(&Type::String, &Type::Entity);
    assert_eq!(get.params.len(), 1);
    assert_eq!(get.params[0].ty, Type::String); // key: K

    let cp = map_method("copyFrom").unwrap().signature(&Type::String, &Type::Entity);
    assert_eq!(cp.params[0].ty, Type::Map(Box::new(Type::String), Box::new(Type::Entity)));
}

#[test]
fn map_keys_values_fill_dest_array() {
    // `keys`/`values` take a destination-array argument they fill in place
    // (`lower::access::lower_map_method` reads it as the array-ref dest); their
    // own return is `Any`, so the `out: K[]`/`V[]` array is a param, not the result.
    let keys = map_method("keys").unwrap().signature(&Type::String, &Type::Int);
    assert_eq!(keys.params.len(), 1);
    assert_eq!(keys.params[0].name, "out");
    assert_eq!(keys.params[0].ty, Type::Array(Box::new(Type::String))); // out: K[]

    let values = map_method("values").unwrap().signature(&Type::String, &Type::Int);
    assert_eq!(values.params.len(), 1);
    assert_eq!(values.params[0].name, "out");
    assert_eq!(values.params[0].ty, Type::Array(Box::new(Type::Int))); // out: V[]
}

#[test]
fn map_method_param_names_and_arity() {
    let has = map_method("has").unwrap().signature(&Type::Int, &Type::Bool);
    assert_eq!(has.params.len(), 1);
    assert_eq!(has.params[0].name, "key");

    let remove = map_method("remove").unwrap().signature(&Type::Int, &Type::Bool);
    assert_eq!(remove.params.len(), 1);
    assert_eq!(remove.params[0].name, "key");

    let set = map_method("set").unwrap().signature(&Type::Int, &Type::Bool);
    assert_eq!(set.params[0].name, "key");
    assert_eq!(set.params[1].name, "value");

    let copy_from = map_method("copyFrom").unwrap().signature(&Type::Int, &Type::Bool);
    assert_eq!(copy_from.params.len(), 1);
    assert_eq!(copy_from.params[0].name, "source");

    // clear/length take no positional args.
    for name in ["clear", "length"] {
        let sig = map_method(name).unwrap().signature(&Type::Int, &Type::Bool);
        assert!(sig.params.is_empty(), "{name} should have no params");
    }
}

#[test]
fn map_method_signature_metadata() {
    for m in MAP_METHODS {
        let sig = m.signature(&Type::String, &Type::Int);
        assert_eq!(sig.name, m.name);
        assert!(sig.config_gate.is_none());
    }
}
