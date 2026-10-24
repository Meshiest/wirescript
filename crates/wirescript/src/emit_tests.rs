//! Emit tests that need the builtin call table, which lives in the front end.

use crate::emit::data_struct_for_gate;

#[test]
fn literal_params_with_schema_fields_are_mapped() {
    // Guard for the missing-data-mapping bug class (MakeVector,
    // EdgeDetector, ShowStatusMessage, Sleep, ...): a literal arg to a
    // builtin call is inlined into the gate's data properties at lowering,
    // but build_gate_component only writes fields listed in
    // data_struct_for_gate — an unlisted field silently drops the value.
    // For every call param that can carry a literal, if the gate's schema
    // data struct has a matching field, the mapping must list it.
    //
    // Not covered: gates whose data struct name isn't derivable from the
    // class name (checked via their mapping entry when present), and
    // gates with no schema struct at all (wire-only inputs — literals
    // there are a separate lowering concern).
    use crate::ir::Type;
    let schema = brdb::schemas::bricks_components_schema_max();
    let mut findings: Vec<String> = Vec::new();
    for (_, spec) in crate::catalog::calls::calls().iter() {
        for p in spec.params.iter() {
            if !matches!(
                p.ty,
                Type::String | Type::Int | Type::Float | Type::Bool | Type::Any
            ) {
                continue;
            }
            let field = p.port.as_str();
            let entry = data_struct_for_gate(spec.gate_class);
            let (struct_name, listed) = match entry {
                Some((s, f, _)) => (s.to_string(), Some(f)),
                None => {
                    // Resolve the gate's data struct via the game-extracted
                    // pair table — many gates share structs (PrimMath,
                    // Float_Float, …) whose names aren't derivable from the
                    // class name. Not in the table -> no data component.
                    match brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
                        .iter()
                        .find(|(t, _)| *t == spec.gate_class)
                    {
                        Some((_, s)) => (s.to_string(), None),
                        None => continue,
                    }
                }
            };
            let covered = listed.is_some_and(|f| f.contains(&field));
            if covered {
                continue;
            }
            // Allowlist. SetInventoryEntry builds its data in a dedicated
            // emit branch; the Teleport gates' Destination/Source are
            // composite TeleportDestination structs, deliberately
            // unmapped (wire-only — a scalar literal can't fill them).
            if matches!(
                spec.gate_class,
                "BrickComponentType_Exec_Character_SetInventoryEntry"
                    | "BrickComponentType_Exec_Entity_Teleport"
                    | "BrickComponentType_Exec_Entity_RelativeTeleport"
            ) {
                continue;
            }
            let has_field = schema
                .get_struct(&struct_name)
                .zip(schema.intern.get(field))
                .is_some_and(|(s, id)| s.get(&id).is_some());
            if has_field {
                findings.push(format!(
                    "{}({}) class={} field {}",
                    spec.name, p.name, spec.gate_class, field,
                ));
            }
        }
    }
    findings.sort();
    assert!(
        findings.is_empty(),
        "literal args to these params are silently dropped at emit — \
         add the field to the gate's data_struct_for_gate entry:\n{}",
        findings.join("\n")
    );
}

/// Lowering bakes a scalar literal with `const_eval::scalar_in_type`, emit
/// converts array and map elements with `wire_value_as`, and a constant in a
/// number slot goes through `const_eval::number_in_type`. For every scalar
/// literal kind in every scalar slot, emitting the raw literal gives what
/// emitting the baked one gives, and `number_in_type` changes a literal only
/// to what `scalar_in_type` makes of it.
#[test]
fn the_scalar_coercions_agree() {
    use crate::const_eval::{number_in_type, scalar_in_type};
    use crate::emit::wire_value_as;
    use crate::ir::{Literal, Type};
    let lits = [
        Literal::Int(0),
        Literal::Int(3),
        Literal::Int(-2),
        Literal::Float(0.0),
        Literal::Float(2.5),
        Literal::Float(-0.5),
        Literal::Bool(true),
        Literal::Bool(false),
        Literal::String(String::new()),
        Literal::String("x".into()),
    ];
    let slots = [
        (Type::Int, "i64"),
        (Type::Float, "f64"),
        (Type::Bool, "Bool"),
        (Type::String, "Str"),
    ];
    for lit in &lits {
        for (ty, wire) in &slots {
            let baked = scalar_in_type(lit.clone(), ty);
            assert_eq!(
                wire_value_as(wire, lit),
                wire_value_as(wire, &baked),
                "{lit:?} in a {ty:?} slot: emit and lowering disagree"
            );
            let numbered = number_in_type(lit.clone(), ty);
            if numbered != *lit {
                assert_eq!(
                    numbered, baked,
                    "{lit:?} in a {ty:?} slot: number_in_type and scalar_in_type disagree"
                );
            }
        }
    }
}
