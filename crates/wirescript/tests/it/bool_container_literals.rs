//! A float literal in a `bool` array or map keeps its truth value in the save:
//! nonzero is `true`, as everywhere else a number becomes a bool.

use crate::common;

use brdb::schema::BrdbValue;
use wirescript::EmitOptions;

const SRC: &str = "in start: exec\nvar m: Map<int, bool> = { 1 => 2.5, 2 => true }\nvar a: bool[] = [2.5, true]\non start { let x: bool = m.get(1) }\n";

fn storage_value(class: &str) -> BrdbValue {
    let brz = wirescript::compile_to_world(common::input(SRC, "bools.ws"), EmitOptions::default())
        .unwrap_or_else(|e| panic!("{e}"))
        .world
        .to_brz_vec()
        .expect("encode");
    common::world_components(&brz)
        .into_iter()
        .find(|c| c.get_name() == class)
        .and_then(|c| c.get("Value").cloned())
        .unwrap_or_else(|| panic!("no {class}"))
}

#[test]
fn a_float_in_a_bool_array_is_saved_as_its_truth_value() {
    let BrdbValue::DynamicArray(a) = storage_value("BrickComponentData_ArrayVar") else {
        panic!("not an array")
    };
    assert_eq!(a.values, vec![BrdbValue::Bool(true), BrdbValue::Bool(true)]);
}

#[test]
fn a_float_in_a_bool_map_is_saved_as_its_truth_value() {
    let BrdbValue::DynamicMap(m) = storage_value("BrickComponentData_MapVar") else {
        panic!("not a map")
    };
    let value = m
        .entries
        .iter()
        .find(|(k, _)| **k == BrdbValue::I64(1))
        .map(|(_, v)| v.clone());
    assert_eq!(value, Some(BrdbValue::Bool(true)));
}
