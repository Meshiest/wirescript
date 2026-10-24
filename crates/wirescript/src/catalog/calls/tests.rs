use super::*;

/// One expected property of a call's spec.
#[derive(Debug)]
enum Shape {
    Exec(bool),
    ParamCount(usize),
    Param0(&'static str, Type),
    Output0Port(WirePort),
    Output0Type(Type),
}

#[test]
fn call_shapes() {
    use Shape::*;
    let rows: &[(&str, &[Shape])] = &[
        (
            "DisplayText",
            &[Exec(true), Param0("target", Type::Controller)],
        ),
        ("sin", &[Exec(false), Output0Port(WirePort::Output)]),
        ("Vec", &[ParamCount(3)]),
        // The inventory dump types both leaderboard `Value` outputs as `int`,
        // so arithmetic on the result has an operator overload.
        ("GetLeaderboard", &[Output0Type(Type::Int)]),
        ("GetTeamLeaderboardValue", &[Output0Type(Type::Int)]),
    ];
    let mut failures = Vec::new();
    for (name, shapes) in rows {
        let Some(c) = find_call(name) else {
            failures.push(format!("{name}: no such call"));
            continue;
        };
        for s in *shapes {
            let ok = match s {
                Exec(e) => c.exec == *e,
                ParamCount(n) => c.params.len() == *n,
                Param0(n, t) => c.params.first().is_some_and(|p| p.name == *n && p.ty == *t),
                Output0Port(p) => c.outputs.first().is_some_and(|o| o.port == *p),
                Output0Type(t) => c.outputs.first().is_some_and(|o| o.ty == *t),
            };
            if !ok {
                failures.push(format!(
                    "{name}: want {s:?}; exec {}, params {:?}, outputs {:?}",
                    c.exec, c.params, c.outputs
                ));
            }
        }
    }
    if find_call("doesNotExist").is_some() {
        failures.push("doesNotExist: an unknown name found a call".into());
    }
    crate::test_support::report_failures(&failures, rows.len() + 1);
}

/// `visibleTo` takes a player or a team, and may be left out.
#[test]
fn set_visible_to_takes_a_player_or_team() {
    let c = find_call("SetVisibleTo").unwrap();
    assert!(c.exec);
    assert_eq!(c.gate_class, "BrickComponentType_Exec_Entity_SetVisibleTo");
    assert_eq!(c.receiver, Some(Type::Entity));
    assert_eq!(
        (c.params[0].name, c.params[0].port, c.params[0].optional),
        ("entity", WirePort::Entity, false)
    );
    let v = &c.params[1];
    assert_eq!(
        (v.name, v.port, v.optional),
        ("visibleTo", WirePort::VisibleTo, true)
    );
    assert_eq!(
        v.ty,
        Type::Union(vec![Type::Controller, Type::Character, Type::Entity])
    );
}

/// Params that name a settable field the gate does not expose as a wire
/// input. These only ever work as constants — lowering writes them into the
/// component's data and drops the wire; a computed value has nowhere to go.
///
/// This list pins what the current inventory reports. It should shrink, not
/// grow: a new entry means either a mistyped port or a gate whose ports
/// changed in a game update.
const DATA_ONLY_PARAMS: &[&str] = &[
    "AddComponent.componentType -> ComponentType",
    "AddInventoryItemAdv.ammoOverride -> WeaponAmmoOverride",
    "AddInventoryItemAdv.meshColors -> MeshColors",
    "Blend.clampAlpha -> bClampAlpha",
    "ColorBlend.blendSpace -> BlendSpace",
    "ColorBlend.clampAlpha -> bClampAlpha",
    "ConvertColor.fromSpace -> FromSpace",
    "ConvertColor.toSpace -> ToSpace",
    "DisplayText.font -> Font",
    "DisplayText.typeface -> Typeface",
    "Easing.direction -> Direction",
    "Easing.function -> Function",
    "GetAim.localAim -> bLocalAim",
    "GetBrickProperty.property -> Property",
    "GetComponent.componentType -> ComponentType",
    "GiveWeapon.weapon -> ItemTypeIfItem",
    "HasPermission.permission -> PermissionName",
    "HasRole.role -> RoleName",
    "Remap.direction -> Direction",
    "Remap.function -> Function",
    "RemoveComponent.componentType -> ComponentType",
    "SendCustomEvent.eventName -> EventName",
    "SendGlobalCustomEvent.eventName -> EventName",
    "SetBrickProperty.property -> Property",
    "SetInventoryItemAdv.ammoOverride -> WeaponAmmoOverride",
    "SetInventoryItemAdv.meshColors -> MeshColors",
    "SetTempPermission.permission -> PermissionTagStr",
    "ShowHint.text -> HintText",
    "ShowHint.title -> HintTitle",
    "Slerp.clampAlpha -> bClampAlpha",
    "Slerp.shortestPath -> bShortestPath",
    "SpawnPrefab.prefab -> Prefab",
    "Sweep.bodyPartsOnly -> bOnlyHitPlayerBodyParts",
    "SweepSimple.bodyPartsOnly -> bOnlyHitPlayerBodyParts",
    "SweepSimple.detectBricks -> bDetectBricks",
    "SweepSimple.detectMap -> bDetectMap",
    "SweepSimple.detectPhysics -> bDetectPhysics",
    "SweepSimple.detectPlayers1 -> bDetectPlayers1",
    "SweepSimple.detectPlayers2 -> bDetectPlayers2",
    "SweepSimple.detectPlayers3 -> bDetectPlayers3",
    "SweepSimple.detectPlayers4 -> bDetectPlayers4",
    "SweepSimple.direction -> Direction",
    "SweepSimple.spreadTowardCenter -> bSpreadBiasedTowardsCenter",
];

/// Every catalog param must name a real wire input on its gate, or be a
/// known data-only field. A param pointing at a field the gate has no input
/// port for wires to a slot the game does not have.
#[test]
fn every_call_param_targets_a_real_wire_input() {
    let mut found: Vec<String> = Vec::new();
    for (name, spec) in calls().iter() {
        // Pseudo/internal gates are absent from the inventory dump.
        if crate::catalog::default_catalog()
            .find_by_class(spec.gate_class)
            .is_none()
        {
            continue;
        }
        for p in &spec.params {
            if !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str()) {
                found.push(format!("{name}.{} -> {}", p.name, p.port.as_str()));
            }
        }
    }
    found.sort();
    let expected: Vec<String> = DATA_ONLY_PARAMS.iter().map(|s| s.to_string()).collect();
    let new: Vec<&String> = found.iter().filter(|f| !expected.contains(f)).collect();
    assert!(new.is_empty(), "params bound to non-wire ports: {new:#?}");
    let gone: Vec<&String> = expected.iter().filter(|e| !found.contains(e)).collect();
    assert!(
        gone.is_empty(),
        "these are wireable now — drop them from DATA_ONLY_PARAMS: {gone:#?}"
    );
}

/// Companion to `every_call_param_targets_a_real_wire_input`, for the other
/// wire directions the param test doesn't cover: every CallSpec OUTPUT port,
/// and every EVENT exec-out / data / wired-input port, must name a real port
/// (right direction) on its gate per the inventory. A stale name here wires
/// to a slot the game doesn't have — a silent load failure.
#[test]
fn every_call_output_and_event_port_is_real() {
    let cat = crate::catalog::default_catalog();
    let ports = |names: &[crate::catalog::Port]| -> std::collections::HashSet<String> {
        let mut s = std::collections::HashSet::new();
        for p in names {
            s.insert(p.name.clone());
            if let Some(c) = &p.composite {
                s.extend(c.sub_ports.iter().cloned());
            }
        }
        s
    };
    let mut bad: Vec<String> = Vec::new();
    for (name, spec) in calls().iter() {
        let Some(g) = cat.find_by_class(spec.gate_class) else {
            continue;
        };
        let outs = ports(&g.component.outputs);
        for o in &spec.outputs {
            let port = o.port.as_str();
            if port != "ExecOut" && !outs.contains(port) {
                bad.push(format!("call {name} output -> {port}"));
            }
        }
    }
    for (name, e) in crate::catalog::events::events().iter() {
        let Some(g) = cat.find_by_class(e.gate_class) else {
            continue;
        };
        let ins = ports(&g.component.inputs);
        let outs = ports(&g.component.outputs);
        if e.exec_out != "ExecOut" && !outs.contains(e.exec_out) {
            bad.push(format!("event {name} exec_out -> {}", e.exec_out));
        }
        for d in &e.data {
            if !outs.contains(d.port) {
                bad.push(format!("event {name} data -> {}", d.port));
            }
        }
        for (_, port, _) in &e.input_named {
            if *port != "Exec" && !ins.contains(*port) {
                bad.push(format!("event {name} input -> {port}"));
            }
        }
    }
    bad.sort();
    assert!(
        bad.is_empty(),
        "ports that don't exist on their gate: {bad:#?}"
    );
}

/// Every data-only config param on a gate that has a component data struct
/// must name a real field on that struct in the live brdb schema, and any
/// enum-typed config param must resolve to a real schema enum with members.
/// Guards against game-update drift (a renamed/removed config field, or a
/// field that changed type). Gates absent from the component->struct pair
/// table (pseudo/internal, or fields nested inside an emit-built sub-struct
/// like GiveWeapon's) have no top-level struct here and are skipped.
#[test]
fn config_params_exist_in_schema() {
    // Config fields the emitter writes into a nested sub-struct rather than
    // a top-level field of the gate's data struct (so they legitimately do
    // not appear on that struct directly).
    const NESTED_FIELDS: &[(&str, &str)] = &[("GiveWeapon", "ItemTypeIfItem")];
    let schema = brdb::schemas::bricks_components_schema_max();
    let struct_of: std::collections::HashMap<&str, &str> =
        brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
            .iter()
            .copied()
            .collect();
    for (name, spec) in calls().iter() {
        let Some(strct) = struct_of.get(spec.gate_class) else {
            continue;
        };
        let Some(s) = schema.get_struct(strct) else {
            continue;
        };
        let fields: std::collections::HashSet<&str> = s
            .keys()
            .filter_map(|k| schema.intern.lookup_ref(*k))
            .collect();
        for p in &spec.params {
            let port = p.port.as_str();
            if crate::catalog::is_wire_input(spec.gate_class, port) {
                continue;
            }
            if NESTED_FIELDS.contains(&(name, port)) {
                continue;
            }
            assert!(
                fields.contains(port),
                "{name}.{}: config field '{port}' is not on {strct}",
                p.name
            );
            if let Some(et) = crate::catalog::config_field_enum_type(spec.gate_class, port) {
                assert!(
                    !crate::catalog::enum_member_names(et).is_empty(),
                    "{name}.{}: enum {et} has no members",
                    p.name
                );
            }
        }
    }
}

#[test]
fn display_text_targets_player_state_port() {
    // DisplayText takes its player on the entity-typed `PlayerState` port,
    // and `Text` is a real wire input.
    let g = gc::PLAYERSTATE_DISPLAY_TEXT;
    assert!(crate::catalog::is_wire_input(g, "PlayerState"));
    assert!(crate::catalog::is_wire_input(g, "Text"));
    assert!(!crate::catalog::is_wire_input(g, "PositionX"));
    assert!(!crate::catalog::is_wire_input(g, "Typeface"));
    // Layout ports are Vector2D composites whose float X/Y sub-ports are
    // individually wireable (resolved via composite.sub_ports).
    assert!(crate::catalog::is_wire_input(g, "Position.X"));
    assert!(crate::catalog::is_wire_input(g, "Anchor.Y"));
    assert!(!crate::catalog::is_wire_input(g, "Position.Z"));
    assert!(!crate::catalog::is_wire_input(g, "Bogus.X"));
}
