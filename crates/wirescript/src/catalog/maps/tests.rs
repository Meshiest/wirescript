use super::*;
use crate::ir::Type;
use crate::typecheck::ParamKind;

/// Every method `map_methods()` lists round-trips through `map_method(name)`
/// and has a constructible `signature` named after it, with no config gate and
/// only wire params; `signature` panics on a name with no arm, so a method
/// without one fails here.
#[test]
fn every_map_method_has_a_wire_typed_signature() {
    assert!(std::ptr::eq(map_methods(), MAP_METHODS));
    for m in map_methods() {
        let found = map_method(m.name).unwrap_or_else(|| panic!("no method enum for {}", m.name));
        assert_eq!(found.name, m.name);
        let sig = m.signature(&Type::String, &Type::Int);
        assert_eq!(sig.name, m.name);
        assert!(sig.config_gate.is_none());
        assert!(
            sig.params.iter().all(|p| matches!(p.kind, ParamKind::Wire)),
            "map method {} has a non-wire param",
            m.name
        );
    }
}

#[test]
fn map_method_param_types_track_kv() {
    let set = map_method("set")
        .unwrap()
        .signature(&Type::String, &Type::Entity);
    assert_eq!(set.params.len(), 2);
    assert_eq!(set.params[0].ty, Type::String); // key: K
    assert_eq!(set.params[1].ty, Type::Entity); // value: V

    let get = map_method("get")
        .unwrap()
        .signature(&Type::String, &Type::Entity);
    assert_eq!(get.params.len(), 1);
    assert_eq!(get.params[0].ty, Type::String); // key: K

    let cp = map_method("copyFrom")
        .unwrap()
        .signature(&Type::String, &Type::Entity);
    assert_eq!(
        cp.params[0].ty,
        Type::Map(Box::new(Type::String), Box::new(Type::Entity))
    );
}

#[test]
fn map_keys_values_fill_dest_array() {
    // `keys`/`values` take a destination-array argument they fill in place;
    // their own return is `Never`, so the `destArray: K[]`/`V[]` array is a
    // param, not the result.
    let keys = map_method("keys")
        .unwrap()
        .signature(&Type::String, &Type::Int);
    assert_eq!(keys.params.len(), 2);
    assert_eq!(keys.params[0].name, "destArray");
    assert_eq!(keys.params[0].ty, Type::Array(Box::new(Type::String))); // destArray: K[]
    // `includeDead`: also list keys whose brick or component is gone.
    assert_eq!(
        (keys.params[1].name.as_str(), keys.params[1].optional),
        ("includeDead", true)
    );

    let values = map_method("values")
        .unwrap()
        .signature(&Type::String, &Type::Int);
    assert_eq!(values.params.len(), 1);
    assert_eq!(values.params[0].name, "destArray");
    assert_eq!(values.params[0].ty, Type::Array(Box::new(Type::Int))); // destArray: V[]
}

#[test]
fn map_method_param_names_and_arity() {
    let has = map_method("has")
        .unwrap()
        .signature(&Type::Int, &Type::Bool);
    assert_eq!(has.params.len(), 1);
    assert_eq!(has.params[0].name, "key");

    let remove = map_method("remove")
        .unwrap()
        .signature(&Type::Int, &Type::Bool);
    assert_eq!(remove.params.len(), 1);
    assert_eq!(remove.params[0].name, "key");

    let set = map_method("set")
        .unwrap()
        .signature(&Type::Int, &Type::Bool);
    assert_eq!(set.params[0].name, "key");
    assert_eq!(set.params[1].name, "value");

    let copy_from = map_method("copyFrom")
        .unwrap()
        .signature(&Type::Int, &Type::Bool);
    assert_eq!(copy_from.params.len(), 1);
    assert_eq!(copy_from.params[0].name, "source");

    // clear/length take no positional args.
    for name in ["clear", "length"] {
        let sig = map_method(name).unwrap().signature(&Type::Int, &Type::Bool);
        assert!(sig.params.is_empty(), "{name} should have no params");
    }
}

/// Every map method's return type is what its gate outputs: the catalog's
/// value outputs minus the exec-out and the inputs' pass-throughs, the
/// lowering's result first, a generic output carrying the map's value type.
#[test]
fn map_return_types_follow_the_inventory() {
    let (key, value) = (Type::String, Type::Int);
    for m in map_methods() {
        let gate = crate::catalog::default_catalog()
            .find_by_class(m.gate)
            .expect("gate in the inventory");
        let inputs: Vec<&str> = gate
            .component
            .inputs
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        let mut fields: Vec<(String, Type)> = gate
            .component
            .outputs
            .iter()
            .filter(|p| {
                p.ty != crate::catalog::RawPortType::Exec && !inputs.contains(&p.name.as_str())
            })
            .map(|p| {
                (
                    crate::catalog::arrays::field_name(&p.name),
                    p.ty.to_type(&value, &Type::Any),
                )
            })
            .collect();
        if let crate::catalog::methods::Lowering::Generic { result, .. } = &m.lowering
            && let Some(i) = fields
                .iter()
                .position(|(n, _)| *n == crate::catalog::arrays::field_name(result.as_str()))
        {
            let f = fields.remove(i);
            fields.insert(0, f);
        }
        assert_eq!(
            map_return_type(m.name, &key, &value),
            Some(Type::call_result(fields)),
            "map method `{}`",
            m.name
        );
    }
}
