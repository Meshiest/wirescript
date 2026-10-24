use super::*;

#[test]
fn coerce_rules() {
    use CoerceRule::*;
    use Type::*;
    let r = |t: Type| Ref(Box::new(t));
    let arr = |t: Type| Array(Box::new(t));
    let map = |k: Type, v: Type| Map(Box::new(k), Box::new(v));
    let rec = |fields: &[(&str, Type)]| {
        Record(
            fields
                .iter()
                .map(|(n, t)| ((*n).into(), t.clone()))
                .collect(),
        )
    };
    let found = rec(&[("Index", Int), ("Found", Bool)]);
    // GetVelocity's `{Vector, Rotation}`.
    let vel = rec(&[("Vector", Vector), ("Rotation", Rotator)]);
    let mut rows: Vec<(&str, Type, Type, CoerceRule)> = vec![
        ("same types", Int, Int, Same),
        ("any as a source", Any, Int, Same),
        ("any as a target", Int, Any, Same),
        ("opaque as a source", Opaque, Int, Same),
        ("opaque as a target", Int, Opaque, Same),
        ("opaque to string", Opaque, String, Same),
        // A Sweep's HitEntity (or any entity wire) can be a player: wires carry
        // plain object refs, so the downcast is implicit in-game.
        ("entity to character", Entity, Character, Coerce),
        ("entity to controller", Entity, Controller, Coerce),
        ("character to controller", Character, Controller, Coerce),
        ("controller to character", Controller, Character, Coerce),
        ("controller to entity", Controller, Entity, Coerce),
        ("int to float", Int, Float, Coerce),
        ("float to int", Float, Int, Coerce),
        ("bool to int", Bool, Int, Coerce),
        ("rotator to quat", Rotator, Quat, Coerce),
        ("quat to rotator", Quat, Rotator, Coerce),
        ("bool pulses exec", Bool, Exec, Coerce),
        ("int pulses exec", Int, Exec, Coerce),
        ("vector pulses exec", Vector, Exec, Coerce),
        ("character pulses exec", Character, Exec, Coerce),
        ("controller pulses exec", Controller, Exec, Coerce),
        // Exactly `s != ""`: lowering inserts a `CompareNotEqual(s, "")` gate at
        // every string-source -> bool-destination wire.
        ("string to bool", String, Bool, Coerce),
        ("int to string", Int, String, ViaString),
        ("vector to string", Vector, String, ViaString),
        ("numeric to vector", Int, Vector, Mismatch),
        // `String -> Bool` must not chain into the numeric family: `coerce` is
        // single-step, so these stay Mismatch forever.
        ("string to int", String, Int, Mismatch),
        ("string to float", String, Float, Mismatch),
        ("ref invariance", r(Int), r(Float), Mismatch),
        ("same ref", r(Int), r(Int), Same),
        // Unwraps to a matching field; no field matches -> still a mismatch.
        (
            "record with no matching field",
            found.clone(),
            Vector,
            Mismatch,
        ),
        // A record target is not unwrapped.
        (
            "record into a record",
            found.clone(),
            Record(vec![]),
            Mismatch,
        ),
        // Lowering always wires outputs[0], so a `rotator` target must NOT
        // unwrap via the later Rotation field: that produced a Vector-into-
        // Rotation miscompile. Field 0 still drives a valid unwrap.
        ("record unwrap past field 0", vel.clone(), Rotator, Mismatch),
        ("record unwrap via field 0", vel, Vector, Same),
        // An index-keyed record is a tuple literal; `(1, "abc")` is not an int.
        (
            "tuple literal to a scalar",
            rec(&[("0", Int), ("1", String)]),
            Int,
            Mismatch,
        ),
        // A concrete array flows into an `any[]` sink.
        ("int[] into any[]", arr(Int), arr(Any), Same),
        // An `any[]` does NOT narrow into a concrete `int[]` (the backing variant
        // differs and would miscompile).
        ("any[] into int[]", arr(Any), arr(Int), Mismatch),
        ("int[] into float[]", arr(Int), arr(Float), Mismatch),
        (
            "Map<int, int> into Map<any, any>",
            map(Int, Int),
            map(Any, Any),
            Same,
        ),
        (
            "Map<int, int> into Map<float, int>",
            map(Int, Int),
            map(Float, Int),
            Mismatch,
        ),
    ];
    // Bool renders "true"/"false" through the format-text path, distinct from
    // the string->bool truthiness rule.
    for t in [Bool, Float, Entity, Character, Controller, Rotator, Color] {
        rows.push(("primitive casts to string", t, String, ViaString));
    }
    let mut failures = Vec::new();
    for (label, from, to, want) in &rows {
        let got = coerce(from, to);
        if got != *want {
            failures.push(format!(
                "{label}: coerce({from:?}, {to:?}) = {got:?}, want {want:?}"
            ));
        }
    }
    // Unwraps to whichever field matches the target.
    for to in [Int, Bool] {
        if coerce(&found, &to) == Mismatch {
            failures.push(format!("record unwraps to its {to:?} field"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len() + 2);
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
    assert_eq!(
        widening_join(&Type::Character, &Type::Entity),
        Some(Type::Entity)
    );
    assert_eq!(
        widening_join(&Type::Controller, &Type::Entity),
        Some(Type::Entity)
    );
    assert_eq!(
        widening_join(&Type::Character, &Type::Controller),
        Some(Type::Entity)
    );
    // rotator/quat -> canonical rotator
    assert_eq!(
        widening_join(&Type::Rotator, &Type::Quat),
        Some(Type::Rotator)
    );
    assert_eq!(
        widening_join(&Type::Quat, &Type::Rotator),
        Some(Type::Rotator)
    );
    // no common widening
    assert_eq!(widening_join(&Type::Int, &Type::Vector), None);
    assert_eq!(widening_join(&Type::String, &Type::Int), None);
    // compound types only join with themselves (structural mismatch -> None)
    let arr_int = Type::Array(Box::new(Type::Int));
    let arr_float = Type::Array(Box::new(Type::Float));
    assert_eq!(
        widening_join(&arr_int, &arr_int.clone()),
        Some(arr_int.clone())
    );
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
    assert_eq!(
        widening_join_all(vec![Type::Int, Type::Float, Type::Vector]),
        None
    );
}

#[test]
fn enum_type_is_nominal_and_joins_with_itself_only() {
    use crate::ir::Type;
    let opt_int = Type::Enum {
        name: "Option".into(),
        args: vec![Type::Int],
    };
    let opt_int2 = Type::Enum {
        name: "Option".into(),
        args: vec![Type::Int],
    };
    let opt_flt = Type::Enum {
        name: "Option".into(),
        args: vec![Type::Float],
    };
    let res_int = Type::Enum {
        name: "Result".into(),
        args: vec![Type::Int],
    };
    assert!(type_eq(&opt_int, &opt_int2));
    assert!(!type_eq(&opt_int, &opt_flt));
    assert!(!type_eq(&opt_int, &res_int));
    assert_eq!(widening_join(&opt_int, &opt_int2), Some(opt_int.clone()));
    assert_eq!(widening_join(&opt_int, &opt_flt), None);
    assert_eq!(format!("{opt_int}"), "Option<int>");
}
