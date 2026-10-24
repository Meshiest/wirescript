use super::*;
use crate::ir::Type;
use crate::typecheck::ParamKind;

/// Every method `array_methods()` lists round-trips through
/// `array_method(name)` and has a constructible `signature` whose params are
/// all wires; `signature` panics on a name with no arm, so a method without
/// one fails here.
#[test]
fn every_array_method_has_a_wire_typed_signature() {
    assert!(std::ptr::eq(array_methods(), ARRAY_METHODS));
    for m in array_methods() {
        let found = array_method(m.name).unwrap_or_else(|| panic!("no method enum for {}", m.name));
        assert_eq!(found.name, m.name);
        let sig = m.signature(&Type::Int);
        assert!(
            sig.params.iter().all(|p| matches!(p.kind, ParamKind::Wire)),
            "array method {} has a non-wire param",
            m.name
        );
    }
}

#[test]
fn array_method_param_types_track_element() {
    let sig = array_method("push").unwrap().signature(&Type::Vector);
    assert_eq!(sig.params.len(), 1);
    assert_eq!(sig.params[0].ty, Type::Vector); // value: T
    let ins = array_method("insert").unwrap().signature(&Type::Int);
    assert_eq!(ins.params[0].ty, Type::Int); // index: int
    assert_eq!(ins.params[1].ty, Type::Int); // value: T (elem=int)
    let sl = array_method("slice").unwrap().signature(&Type::Int);
    assert_eq!(sl.params[0].ty, Type::Array(Box::new(Type::Int))); // source: T[]
    let so = array_method("sort").unwrap().signature(&Type::Int);
    assert!(so.params[0].optional); // descending?: bool
    // The `fillFromZone*` gates take a `zone` rerouter reference ONLY on
    // their Zone slot — never a plain entity.
    let fz = array_method("fillFromZoneEntities")
        .unwrap()
        .signature(&Type::Int);
    assert_eq!(fz.params[0].ty, Type::Zone); // zone: zone
    assert!(fz.params[1].optional); // tagFilter?: string
    assert_eq!(fz.params[1].ty, Type::String);
}

/// `join` reads the gate's `Output`, a string whatever the element type,
/// and its separator is an optional string.
#[test]
fn join_is_a_string_with_an_optional_separator() {
    for elem in [Type::Int, Type::String, Type::Entity] {
        assert_eq!(array_return_type("join", &elem), Some(Type::String));
        let sig = array_method("join").unwrap().signature(&elem);
        assert_eq!(sig.params.len(), 1);
        assert!(sig.params[0].optional);
        assert_eq!(sig.params[0].ty, Type::String);
    }
}

/// `average` reads the gate's `Value`, which the gate declares a float
/// whatever the element type: the mean of an int array is fractional.
#[test]
fn average_of_an_int_array_is_a_float() {
    let ret = array_return_type("average", &Type::Int).unwrap();
    let Type::Outputs(fields) = ret else {
        panic!("expected outputs, got {ret:?}")
    };
    assert_eq!(fields[0], ("Value".to_string(), Type::Float));
}
