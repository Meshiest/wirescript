//! Tests for game-exposed gate config properties: enum config validation
//! (WS028) and enum config landing as gate data.

use super::*;
use crate::ir::Literal;

/// Error diagnostic codes from typechecking `src`.
fn errors(src: &str) -> Vec<String> {
    let parsed = crate::parser::parse(src, "test");
    crate::typecheck::typecheck(&parsed.ast, "test")
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string())
        .collect()
}

#[test]
fn enum_config_accepts_bare_member_name() {
    // `function`/`direction` on Easing are data-only EBREasingFunction /
    // EBREasingDirection config fields — bare member names are valid.
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bounce, direction = InOut)\n";
    assert!(
        errors(src).is_empty(),
        "bare enum member names should typecheck: {:?}",
        errors(src)
    );
}

#[test]
fn enum_config_rejects_unknown_member() {
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = Wobble)\n";
    let e = errors(src);
    assert!(
        e.contains(&"WS028".to_string()),
        "unknown enum member should be WS028, got {e:?}"
    );
    // The bare identifier must NOT read as an unknown variable (WS002).
    assert!(
        !e.contains(&"WS002".to_string()),
        "enum member should not be treated as a variable: {e:?}"
    );
}

#[test]
fn enum_config_accepts_raw_int_in_range() {
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = 3)\n";
    assert!(
        errors(src).is_empty(),
        "raw int should be accepted: {:?}",
        errors(src)
    );
}

#[test]
fn enum_config_rejects_out_of_range_int() {
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = 99)\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "out-of-range int should be WS028: {:?}",
        errors(src)
    );
}

#[test]
fn enum_config_accepts_quoted_name() {
    // The pre-existing quoted-name form keeps working (and is validated).
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = \"Bounce\")\n";
    assert!(
        errors(src).is_empty(),
        "quoted member name: {:?}",
        errors(src)
    );
}

#[test]
fn enum_config_lowers_to_data_int() {
    // The resolved member value lands on the gate's data field, not as a wire.
    let src = "in t: float\nlet e = Easing(0.0, 1.0, t, function = Bounce)\n";
    let r = compile(src);
    assert_no_errors(&r);
    let node = find_gate(&r, crate::ir::gate_class::MATH_EASING);
    let props = &r.module.nodes[&node].properties;
    let func = props
        .get(&crate::intern::intern("Function"))
        .expect("Function data field set from `function = Bounce`");
    // EBREasingFunction::Bounce == 10.
    assert!(matches!(func, Literal::Int(10)), "got {func:?}");
}

fn prop<'a>(r: &'a LowerResult, class: &str, field: &str) -> Option<&'a Literal> {
    let node = find_gate(r, class);
    r.module.nodes[&node]
        .properties
        .get(&crate::intern::intern(field))
}

#[test]
fn sweep_simple_bool_and_enum_config_land_as_data() {
    let src = "in go: exec\non go {\n  SweepSimple(100.0, direction = Y_Positive, detectBricks = false, bodyPartsOnly = true)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::SWEEP_SIMPLE;
    // EBrickDirection::Y_Positive == 2.
    assert!(matches!(prop(&r, c, "Direction"), Some(Literal::Int(2))));
    assert!(matches!(
        prop(&r, c, "bDetectBricks"),
        Some(Literal::Bool(false))
    ));
    assert!(matches!(
        prop(&r, c, "bOnlyHitPlayerBodyParts"),
        Some(Literal::Bool(true))
    ));
}

#[test]
fn color_blend_enum_config_lands_as_data() {
    let src = "in a: color\nin b: color\nin t: float\nlet c = ColorBlend(a, b, t, blendSpace = Oklab, clampAlpha = false)\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::COLOR_BLEND;
    // EBRColorSpace::Oklab == 2.
    assert!(matches!(prop(&r, c, "BlendSpace"), Some(Literal::Int(2))));
    assert!(matches!(
        prop(&r, c, "bClampAlpha"),
        Some(Literal::Bool(false))
    ));
}

#[test]
fn display_text_font_ref_lands_as_data() {
    // `font` is a font asset ref (`$BrickFontDescriptor/…`) inlined into the
    // gate's `Font` (object) data field, reusing the asset-ref path.
    let src =
        "in go: exec\nin p: controller\non go {\n  p.DisplayText(\"hi\", font = $BrickFontDescriptor/Roboto)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT;
    assert!(
        matches!(prop(&r, c, "Font"), Some(Literal::Asset { .. })),
        "Font asset ref should inline as data"
    );
}

#[test]
fn clock_event_config_lands_as_data() {
    // Clock is an event: the body chains from its Pulse; `interval`/`enabled`
    // wire into IntervalSeconds/bEnabled; pulseOn/onTime/offTime are constant
    // settings-menu config (these bake as data).
    let src = "static var n: int = 0\non Clock(interval = 2.0, enabled = true, pulseOn = false, onTime = 0.5, offTime = 0.5) {\n  n = n + 1\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = "BrickComponentType_Clock";
    assert!(has_gate(&r, c), "Clock gate placed");
    // `enabled` is now a wire input, not baked config.
    assert!(prop(&r, c, "bEnabled").is_none());
    assert!(matches!(
        prop(&r, c, "bPulseOn"),
        Some(Literal::Bool(false))
    ));
    assert!(
        matches!(prop(&r, c, "OnTimeSeconds"), Some(Literal::Float(f)) if (*f - 0.5).abs() < 1e-9)
    );
    assert!(
        matches!(prop(&r, c, "OffTimeSeconds"), Some(Literal::Float(f)) if (*f - 0.5).abs() < 1e-9)
    );
}

#[test]
fn display_text_typeface_and_justify_enums_land() {
    let src = "in go: exec\nin p: controller\non go {\n  p.DisplayText(\"hi\", typeface = Bold, justify = Center)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT;
    // EBRTextTypeface::Bold == 1, EBRDisplayTextJustification::Center == 1.
    assert!(matches!(prop(&r, c, "Typeface"), Some(Literal::Int(1))));
    assert!(matches!(
        prop(&r, c, "Justification"),
        Some(Literal::Int(1))
    ));
}

// ---- Advanced inventory: composite (array/struct) gate config -----------

/// `weapon` is wired into `ItemType`, so the round-trip test never bakes an
/// asset index the bare max schema (empty asset table) would reject on read.
const ADV_INVENTORY_SRC: &str = concat!(
    "in go: exec\nin c: character\nin weapon: entity\n",
    "on go {\n",
    "  c.AddInventoryItemAdv(weapon, overrideColors = true,\n",
    "    meshColors = [ColorSRGB(255, 0, 0, 255), ColorSRGB(0, 255, 0, 128)],\n",
    "    ammoOverride = { overrideStartingAmmo: true, resources: [{ loaded: 30, reserve: 90 }] })\n",
    "}\n",
);

#[test]
fn adv_inventory_composite_config_lands_as_data() {
    let r = compile(ADV_INVENTORY_SRC);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::CHARACTER_ADD_INVENTORY_ITEM_ADV;
    // The plain bool config field still lands.
    assert!(matches!(
        prop(&r, c, "bOverrideColors"),
        Some(Literal::Bool(true))
    ));
    // meshColors folds to an array of sRGB Color literals (semantic RGBA u8).
    match prop(&r, c, "MeshColors") {
        Some(Literal::Array(cols)) => {
            assert_eq!(cols.len(), 2);
            assert!(matches!(
                cols[0],
                Literal::Color {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255
                }
            ));
            assert!(matches!(
                cols[1],
                Literal::Color {
                    r: 0,
                    g: 255,
                    b: 0,
                    a: 128
                }
            ));
        }
        other => panic!("MeshColors should be an Array of Color, got {other:?}"),
    }
    // ammoOverride folds to Array[Bool, Array[Array[Int, Int], ...]].
    match prop(&r, c, "WeaponAmmoOverride") {
        Some(Literal::Array(parts)) => {
            assert!(matches!(parts[0], Literal::Bool(true)));
            match &parts[1] {
                Literal::Array(res) => {
                    assert_eq!(res.len(), 1);
                    assert!(matches!(&res[0], Literal::Array(p)
                        if matches!(p.as_slice(), [Literal::Int(30), Literal::Int(90)])));
                }
                other => panic!("resources should be an Array, got {other:?}"),
            }
        }
        other => panic!("WeaponAmmoOverride should be an Array, got {other:?}"),
    }
}

#[test]
fn adv_inventory_composite_config_roundtrips_to_bytes() {
    // Distinct, asymmetric values so a mislabeled/reordered byte can't pass.
    let src = concat!(
        "in go: exec\nin c: character\nin weapon: entity\n",
        "on go {\n",
        "  c.AddInventoryItemAdv(weapon,\n",
        "    meshColors = [ColorSRGB(10, 20, 30, 40), ColorSRGB(200, 150, 100, 255)],\n",
        "    ammoOverride = { overrideStartingAmmo: true,\n",
        "      resources: [{ loaded: 7, reserve: 13 }, { loaded: 1, reserve: 2 }] })\n",
        "}\n",
    );
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::CHARACTER_ADD_INVENTORY_ITEM_ADV;
    let node = find_gate(&r, c);
    let val = crate::emit::roundtrip_adv_inventory_component(&r.module.nodes[&node]);
    use brdb::schema::BrdbValue;
    let BrdbValue::Struct(s) = &val else {
        panic!("expected a struct, got {val:?}")
    };

    // MeshColors serializes as Color[] in BGRA byte order, no gamma.
    let Some(BrdbValue::Array(colors)) = s.get("MeshColors") else {
        panic!("MeshColors not an array: {:?}", s.get("MeshColors"))
    };
    // MeshColors is a fixed-size array: the two supplied colours, then white
    // padding to exactly 8.
    assert_eq!(colors.len(), 8, "MeshColors must be padded to the fixed slot count");
    let bgra = |v: &BrdbValue| -> (u8, u8, u8, u8) {
        let BrdbValue::Struct(cs) = v else {
            panic!("color element not a struct")
        };
        let u8f = |name: &str| match cs.get(name) {
            Some(BrdbValue::U8(n)) => *n,
            other => panic!("{name} = {other:?}"),
        };
        (u8f("B"), u8f("G"), u8f("R"), u8f("A"))
    };
    // ColorSRGB(r, g, b, a) -> struct Color { B, G, R, A }.
    assert_eq!(bgra(&colors[0]), (30, 20, 10, 40));
    assert_eq!(bgra(&colors[1]), (100, 150, 200, 255));
    for c in &colors[2..] {
        assert_eq!(bgra(c), (255, 255, 255, 255), "unspecified slots are white-padded");
    }

    // WeaponAmmoOverride: bOverrideStartingAmmo + Resources[] of Loaded/Reserve.
    let Some(BrdbValue::Struct(ammo)) = s.get("WeaponAmmoOverride") else {
        panic!("WeaponAmmoOverride not a struct")
    };
    assert!(matches!(
        ammo.get("bOverrideStartingAmmo"),
        Some(BrdbValue::Bool(true))
    ));
    // Resources is a fixed-size array the game rejects at any other length: the
    // two supplied entries, then zero-padding to exactly 8.
    let Some(BrdbValue::Array(res)) = ammo.get("Resources") else {
        panic!("Resources not an array")
    };
    assert_eq!(res.len(), 8, "Resources must be padded to the fixed slot count");
    let loaded_reserve = |v: &BrdbValue| -> (i32, i32) {
        let BrdbValue::Struct(rs) = v else {
            panic!("resource not a struct")
        };
        let i32f = |name: &str| match rs.get(name) {
            Some(BrdbValue::I32(n)) => *n,
            other => panic!("{name} = {other:?}"),
        };
        (i32f("Loaded"), i32f("Reserve"))
    };
    assert_eq!(loaded_reserve(&res[0]), (7, 13));
    assert_eq!(loaded_reserve(&res[1]), (1, 2));
    for r in &res[2..] {
        assert_eq!(loaded_reserve(r), (0, 0), "unspecified slots are zero-padded");
    }
}

#[test]
fn adv_inventory_omitted_ammo_override_still_fills_fixed_resources() {
    // Regression: with NO `ammoOverride`, the WeaponAmmoOverride field must
    // still serialize a full fixed-length Resources array (the schema writer's
    // default would otherwise emit an empty one and the game load fails with
    // `FixedArraySizeInvalid`). Overriding stays off; every slot is zero.
    let src = concat!(
        "in go: exec\nin c: character\nin weapon: entity\n",
        "on go {\n",
        "  c.AddInventoryItemAdv(weapon, meshColors = [ColorSRGB(1, 2, 3, 4)])\n",
        "}\n",
    );
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::CHARACTER_ADD_INVENTORY_ITEM_ADV;
    let node = find_gate(&r, c);
    let val = crate::emit::roundtrip_adv_inventory_component(&r.module.nodes[&node]);
    use brdb::schema::BrdbValue;
    let BrdbValue::Struct(s) = &val else {
        panic!("expected a struct, got {val:?}")
    };
    let Some(BrdbValue::Struct(ammo)) = s.get("WeaponAmmoOverride") else {
        panic!("WeaponAmmoOverride missing when ammoOverride omitted")
    };
    assert!(matches!(
        ammo.get("bOverrideStartingAmmo"),
        Some(BrdbValue::Bool(false))
    ));
    let Some(BrdbValue::Array(res)) = ammo.get("Resources") else {
        panic!("Resources not an array")
    };
    assert_eq!(res.len(), 8, "omitted ammoOverride must still fill all slots");
}

#[test]
fn set_inventory_item_adv_mesh_colors_roundtrips() {
    // SetInventoryItemAdv shares the composite fields (plus a leading Slot).
    let src = concat!(
        "in go: exec\nin c: character\nin weapon: entity\n",
        "on go {\n",
        "  c.SetInventoryItemAdv(weapon, 2, meshColors = [ColorSRGB(1, 2, 3, 4)])\n",
        "}\n",
    );
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::CHARACTER_SET_INVENTORY_ITEM_ADV;
    let node = find_gate(&r, c);
    let val = crate::emit::roundtrip_adv_inventory_component(&r.module.nodes[&node]);
    use brdb::schema::BrdbValue;
    let BrdbValue::Struct(s) = &val else {
        panic!("expected a struct, got {val:?}")
    };
    let Some(BrdbValue::Array(colors)) = s.get("MeshColors") else {
        panic!("MeshColors not an array")
    };
    assert_eq!(colors.len(), 8, "MeshColors must be padded to the fixed slot count");
    let BrdbValue::Struct(cs) = &colors[0] else {
        panic!("color not a struct")
    };
    let u8f = |name: &str| match cs.get(name) {
        Some(BrdbValue::U8(n)) => *n,
        other => panic!("{name} = {other:?}"),
    };
    assert_eq!((u8f("B"), u8f("G"), u8f("R"), u8f("A")), (3, 2, 1, 4));
}

#[test]
fn mesh_colors_rejects_non_constant() {
    // A non-constant element can't fold into gate data — clean WS028, never a
    // silent broken wire into the non-input MeshColors port.
    let src = "in go: exec\nin c: character\nin weapon: entity\nin live: color\non go {\n  c.AddInventoryItemAdv(weapon, meshColors = [live])\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "non-constant meshColors should be WS028, got {:?}",
        errors(src)
    );
}

#[test]
fn ammo_override_rejects_non_constant() {
    let src = "in go: exec\nin c: character\nin weapon: entity\nin n: int\non go {\n  c.AddInventoryItemAdv(weapon, ammoOverride = { overrideStartingAmmo: true, resources: [{ loaded: n, reserve: 90 }] })\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "non-constant ammoOverride should be WS028, got {:?}",
        errors(src)
    );
}

#[test]
fn non_constant_bool_config_is_rejected() {
    // A variable passed to a non-wire (settings-menu) bool config port is a
    // WS028 typecheck error, not a silent wire into a nonexistent pin.
    let src = "in a: quat\nin b: quat\nin t: float\nin useShortest: bool\nlet q = Slerp(a, b, t, shortestPath = useShortest)\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "non-constant bool config should be WS028: {:?}",
        errors(src)
    );
    // The lowering safety net drops it — no bogus property, and (since it never
    // reaches the wire path) no wire into the nonexistent config pin. The Slerp
    // gate keeps only its three real inputs (a, b, alpha).
    let r = compile(src);
    let node = find_gate(&r, crate::ir::gate_class::QUAT_SLERP);
    assert!(prop(&r, crate::ir::gate_class::QUAT_SLERP, "bShortestPath").is_none());
    assert_eq!(
        r.module.wires.iter().filter(|w| w.target.node_id == node).count(),
        3,
        "Slerp should have exactly its 3 real input wires (a, b, alpha)"
    );
}

#[test]
fn non_constant_font_config_is_rejected() {
    // Same guard for the asset-ref config param.
    let src = "in go: exec\nin p: controller\nin f: entity\non go {\n  p.DisplayText(\"hi\", font = f)\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "non-constant font config should be WS028: {:?}",
        errors(src)
    );
}

// ---- Event-handler config validation ------------------------------------

#[test]
fn non_constant_event_config_is_rejected() {
    // `pulseOn` is constant-only Clock config; a variable would be silently
    // dropped at lowering, so it must be a WS028 typecheck error. (`enabled` is
    // now a wire input, so it accepts a variable — covered separately.)
    let src = "in flag: bool\nstatic var n: int = 0\non Clock(interval = 2.0, pulseOn = flag) {\n  n = n + 1\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "non-constant event config should be WS028: {:?}",
        errors(src)
    );
}

#[test]
fn wired_event_input_stays_dynamic() {
    // `interval` wires into the Clock's IntervalSeconds input port, so a
    // variable there is legitimate and must NOT be rejected.
    let src = "in secs: float\nstatic var n: int = 0\non Clock(interval = secs, enabled = true) {\n  n = n + 1\n}\n";
    assert!(
        errors(src).is_empty(),
        "a variable wired into an event input port should typecheck: {:?}",
        errors(src)
    );
}

// ---- Composite config: missing record fields ----------------------------

#[test]
fn ammo_override_missing_resource_field_is_rejected() {
    // A resource record missing `reserve` must be a WS028 error, not silently
    // baked as reserve = 0.
    let src = "in go: exec\nin c: character\nin weapon: entity\non go {\n  c.AddInventoryItemAdv(weapon, ammoOverride = { overrideStartingAmmo: true, resources: [{ loaded: 30 }] })\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "missing resource field should be WS028: {:?}",
        errors(src)
    );
}

#[test]
fn ammo_override_missing_top_field_is_rejected() {
    // The ammoOverride record missing `resources` is likewise an error.
    let src = "in go: exec\nin c: character\nin weapon: entity\non go {\n  c.AddInventoryItemAdv(weapon, ammoOverride = { overrideStartingAmmo: true })\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "missing ammoOverride field should be WS028: {:?}",
        errors(src)
    );
}

#[test]
fn ammo_override_explicit_empty_resources_is_ok() {
    // An explicit (present) `resources: []` is a deliberate no-op, not a
    // silent default — it stays valid.
    let src = "in go: exec\nin c: character\nin weapon: entity\non go {\n  c.AddInventoryItemAdv(weapon, ammoOverride = { overrideStartingAmmo: false, resources: [] })\n}\n";
    assert!(
        errors(src).is_empty(),
        "explicit empty resources should typecheck: {:?}",
        errors(src)
    );
}

// ---- Data-driven config attributes (raw struct field names) --------------
// Any gate config field in the inventory `config` array is settable by its raw
// name (`bOnlyHitPlayerBodyParts = true`), in addition to any hand-coded
// friendly alias. Enum members are bare names; other scalars are constants.

#[test]
fn data_driven_bool_config_lands_by_raw_name() {
    let src = "in go: exec\non go {\n  SweepSimple(500.0, bOnlyHitPlayerBodyParts = true, bDetectBricks = true)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::SWEEP_SIMPLE;
    assert!(matches!(
        prop(&r, c, "bOnlyHitPlayerBodyParts"),
        Some(Literal::Bool(true))
    ));
    assert!(matches!(prop(&r, c, "bDetectBricks"), Some(Literal::Bool(true))));
}

#[test]
fn data_driven_enum_config_lands_by_raw_name() {
    // `Direction` (raw) resolves the bare member to its int, same as the friendly
    // `direction` alias.
    let src = "in go: exec\non go {\n  SweepSimple(500.0, Direction = Y_Positive)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    // EBrickDirection::Y_Positive == 2.
    assert!(matches!(
        prop(&r, crate::ir::gate_class::SWEEP_SIMPLE, "Direction"),
        Some(Literal::Int(2))
    ));
}

#[test]
fn data_driven_int_config_lands_by_raw_name() {
    let src = "in go: exec\nin p: controller\non go {\n  p.DisplayText(\"hi\", FontSize = 40)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(matches!(
        prop(&r, crate::ir::gate_class::PLAYERSTATE_DISPLAY_TEXT, "FontSize"),
        Some(Literal::Int(40))
    ));
}

#[test]
fn friendly_alias_still_works_alongside_raw() {
    // The hand-coded `bodyPartsOnly` alias keeps setting the same field.
    let src = "in go: exec\non go {\n  SweepSimple(500.0, bodyPartsOnly = true)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(matches!(
        prop(&r, crate::ir::gate_class::SWEEP_SIMPLE, "bOnlyHitPlayerBodyParts"),
        Some(Literal::Bool(true))
    ));
}

#[test]
fn data_driven_enum_rejects_unknown_member() {
    let src = "in go: exec\non go {\n  SweepSimple(500.0, Direction = Nope)\n}\n";
    let e = errors(src);
    assert!(e.contains(&"WS028".to_string()), "unknown member -> WS028: {e:?}");
    assert!(!e.contains(&"WS002".to_string()), "bare member must not read as a variable: {e:?}");
}

#[test]
fn data_driven_non_constant_config_is_rejected() {
    let src = "in go: exec\nin live: bool\non go {\n  SweepSimple(500.0, bOnlyHitPlayerBodyParts = live)\n}\n";
    assert!(
        errors(src).contains(&"WS028".to_string()),
        "non-constant data-driven config -> WS028: {:?}",
        errors(src)
    );
}

// ---- Custom events + Clock enabled-as-wire -------------------------------

#[test]
fn send_custom_event_inlines_name_and_data() {
    let src = "in go: exec\non go {\n  SendCustomEvent(\"spawn\", 42, 99)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::PSEUDO_SEND_CUSTOM_EVENT;
    assert!(has_gate(&r, c));
    // Constant data args inline as typed data (a WireGraphVariant at emit); the
    // name bakes into the EventName str field.
    assert!(matches!(prop(&r, c, "DataIn1"), Some(Literal::Int(42))));
    assert!(matches!(prop(&r, c, "DataIn2"), Some(Literal::Int(99))));
    assert!(matches!(prop(&r, c, "EventName"), Some(Literal::String(s)) if s == "spawn"));
}

#[test]
fn custom_event_typed_params_type_the_data_ports() {
    // `on CustomEvent("name", a: int, b: character, …)` — the leading positional
    // bakes into EventName; the typed params type the DataOut ports; unused
    // slots default to float.
    let src = "static var last: int = 0\non CustomEvent(\"dmg\", amount: int, source: character, sameOwner = true) {\n  last = amount\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let c = crate::ir::gate_class::PSEUDO_CUSTOM_EVENT;
    let node = find_gate(&r, c);
    let outs = &r.module.nodes[&node].ports.outputs;
    let port_ty = |name: &str| {
        outs.iter()
            .find(|p| p.name == crate::intern::intern(name))
            .map(|p| p.ty.clone())
    };
    assert_eq!(port_ty("DataOut1"), Some(crate::ir::Type::Int));
    assert_eq!(port_ty("DataOut2"), Some(crate::ir::Type::Character));
    // Unused data slots default to float (0.0), never `any`.
    assert_eq!(port_ty("DataOut3"), Some(crate::ir::Type::Float));
    assert_eq!(port_ty("DataOut8"), Some(crate::ir::Type::Float));
    // The channel name bakes into the EventName str field.
    assert!(matches!(prop(&r, c, "EventName"), Some(Literal::String(s)) if s == "dmg"));
}

#[test]
fn custom_event_object_config_and_send_target() {
    // The receive gate's `objectEvent` bakes bIsObjectEvent (scopes the event to
    // a grid/object), and the send gate's `target` (the entity whose grid
    // receives object events) wires into the Target port.
    let src = "in go: exec\n\
               in obj: entity\n\
               static var last: int = 0\n\
               on CustomEvent(\"dmg\", amount: int, objectEvent = true) {\n  last = amount\n}\n\
               on go {\n  SendCustomEvent(\"dmg\", 7, target = obj)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let recv = crate::ir::gate_class::PSEUDO_CUSTOM_EVENT;
    let send = crate::ir::gate_class::PSEUDO_SEND_CUSTOM_EVENT;
    assert!(
        matches!(prop(&r, recv, "bIsObjectEvent"), Some(Literal::Bool(true))),
        "objectEvent = true should bake bIsObjectEvent, got {:?}",
        prop(&r, recv, "bIsObjectEvent")
    );
    // `target = obj` wires an entity into the send gate's Target input.
    let send_node = find_gate(&r, send);
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == send_node
            && w.target.port == crate::ir::port_registry::WirePort::Target),
        "target = obj should produce a wire into the send gate's Target port"
    );
}

#[test]
fn send_custom_event_target_is_entity_typed() {
    // The Target input (the grid that receives object events) is entity-typed:
    // a non-entity value is a clear WS003, not a silently-wired type violation.
    let bad = "in go: exec\non go {\n  SendCustomEvent(\"dmg\", 7, target = 5)\n}\n";
    assert!(
        errors(bad).contains(&"WS003".to_string()),
        "non-entity target should be WS003, got {:?}",
        errors(bad)
    );
    // Any object-family reference (entity/character/controller) connects cleanly.
    let ok = "on CharacterSpawned(ch) {\n  SendCustomEvent(\"dmg\", 7, target = ch)\n}\n";
    assert!(
        errors(ok).is_empty(),
        "character target should typecheck: {:?}",
        errors(ok)
    );
}

#[test]
fn spawn_explosion_at_wires_world_position() {
    // SpawnExplosionAt spawns at an absolute world position: `worldPosition`
    // (vector) wires into the WorldPosition input on the SpawnExplosionAt gate.
    let src = "in go: exec\nin pos: vector\nin who: entity\n\
               on go {\n  SpawnExplosionAt(pos, who, instigator = who)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    let gate = crate::ir::gate_class::EXEC_SPAWN_EXPLOSION_AT;
    let node = find_gate(&r, gate);
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == node
            && w.target.port == crate::ir::port_registry::WirePort::WorldPosition),
        "worldPosition should wire into the SpawnExplosionAt gate's WorldPosition port"
    );
}

#[test]
fn global_custom_event_warns_and_keeps_separate_namespace() {
    let ws030 = |src: &str| {
        crate::typecheck::typecheck(&parse(src, "test").ast, "test")
            .diagnostics
            .into_iter()
            .filter(|d| d.code == "WS030")
            .count()
    };
    // A Global send whose data type disagrees with the `on GlobalCustomEvent`
    // receiver warns (WS030), exactly like the personal pair.
    let mismatch = "in go: exec\nstatic var n: int = 0\n\
                    on GlobalCustomEvent(\"g\", amount: int) {\n  n = amount\n}\n\
                    on go {\n  SendGlobalCustomEvent(\"g\", 1.5)\n}\n";
    assert_eq!(ws030(mismatch), 1, "global send/recv mismatch must warn WS030");
    // Personal and Global are SEPARATE namespaces: the same channel name with
    // different types across the two kinds must NOT cross-warn.
    let separate = "in go: exec\nstatic var n: int = 0\nstatic var s: string = \"\"\n\
                    on CustomEvent(\"x\", a: int) {\n  n = a\n}\n\
                    on GlobalCustomEvent(\"x\", b: string) {\n  s = b\n}\n\
                    on go {\n  SendCustomEvent(\"x\", 5)\n  SendGlobalCustomEvent(\"x\", \"hi\")\n}\n";
    assert_eq!(ws030(separate), 0, "personal/global channels of the same name must not cross-warn");
}

#[test]
fn team_predicates_are_pure_bool() {
    // IsBuilderTeam / IsUnaffiliatedTeam are pure predicates over a Team entity,
    // yielding bool. Receiver form works too (`team.IsUnaffiliatedTeam()`).
    let src = "in t: entity\nlet a = IsBuilderTeam(t)\nlet b = t.IsUnaffiliatedTeam()\n\
               out x: bool = a\nout y: bool = b\n";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(has_gate(&r, crate::ir::gate_class::GAMEMODE_IS_BUILDER_TEAM), "IsBuilderTeam gate");
    assert!(
        has_gate(&r, crate::ir::gate_class::GAMEMODE_IS_UNAFFILIATED_TEAM),
        "IsUnaffiliatedTeam gate (receiver form)"
    );
    // A vector annotation rejects the bool result (bool has no vector coercion),
    // proving the return type is a real bool rather than `any`.
    let bad = crate::typecheck::typecheck(
        &parse("in t: entity\nout v: vector = IsBuilderTeam(t)\n", "test").ast,
        "test",
    );
    assert!(
        bad.diagnostics.iter().any(|d| d.code == "WS003"),
        "IsBuilderTeam returns bool, which has no vector coercion"
    );
}

#[test]
fn team_join_leave_events_expose_team_and_playerstate() {
    // ControllerJoinedTeam/LeftTeam fire on team join/leave, binding the joining
    // player's PlayerState plus the Team entity, userId, userName.
    let src = "static var n: int = 0\n\
               on ControllerJoinedTeam(controller, team, userId, userName) {\n  n = n + 1\n}\n\
               on ControllerLeftTeam(controller, team, userId, userName) {\n  n = n - 1\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Fake_Gamemode_ControllerJoinedTeamEvent"));
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Fake_Gamemode_ControllerLeftTeamEvent"));
}

#[test]
fn untyped_custom_event_param_lints_ws029() {
    // A Custom Event receiver param without a type annotation warns (WS029) —
    // the data has no wire type otherwise.
    let src = "static var n: int = 0\non CustomEvent(\"dmg\", amount) {\n  n = n + 1\n}\n";
    let parsed = crate::parser::parse(src, "test");
    let diags = crate::typecheck::typecheck(&parsed.ast, "test").diagnostics;
    assert!(
        diags
            .iter()
            .any(|d| d.code == "WS029" && d.severity == crate::diagnostic::Severity::Warning),
        "untyped custom event param should warn WS029: {:?}",
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>()
    );

    // The typed form does NOT warn, and neither do ordinary fixed-type events.
    for ok in [
        "static var n: int = 0\non CustomEvent(\"dmg\", amount: int) {\n  n = n + 1\n}\n",
        "on CharacterSpawned(character) {}\n",
    ] {
        let p = crate::parser::parse(ok, "test");
        let d = crate::typecheck::typecheck(&p.ast, "test").diagnostics;
        assert!(!d.iter().any(|x| x.code == "WS029"), "should not warn: {ok}");
    }
}

#[test]
fn send_custom_event_type_mismatch_lints_ws030() {
    // A `SendCustomEvent` data value whose wire type disagrees with the
    // `on CustomEvent` receiver's declared param type warns (WS030).
    let src = "in go: exec\nvar last: int = 0\non CustomEvent(\"dmg\", amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", 1.5)\n}\n";
    let parsed = crate::parser::parse(src, "test");
    let diags = crate::typecheck::typecheck(&parsed.ast, "test").diagnostics;
    assert!(
        diags
            .iter()
            .any(|d| d.code == "WS030" && d.severity == crate::diagnostic::Severity::Warning),
        "float sent to int custom-event param should warn WS030: {:?}",
        diags
            .iter()
            .map(|d| (d.code.to_string(), d.message.clone()))
            .collect::<Vec<_>>()
    );

    for ok in [
        // Exact type match — silent.
        "in go: exec\nvar last: int = 0\non CustomEvent(\"dmg\", amount: int) {\n  last = amount\n}\non go {\n  SendCustomEvent(\"dmg\", 7)\n}\n",
        // character vs entity: both the Object wire variant, not a real mismatch.
        "on CharacterSpawned(character) {\n  SendCustomEvent(\"hit\", character)\n}\non CustomEvent(\"hit\", who: entity) {}\n",
        // Dynamic (non-literal) channel name — can reach any receiver, never linted.
        "in go: exec\nvar chan: string = \"dmg\"\non CustomEvent(\"dmg\", amount: int) {}\non go {\n  SendCustomEvent(chan, 1.5)\n}\n",
        // No matching receiver in this unit — nothing to compare against.
        "in go: exec\non go {\n  SendCustomEvent(\"orphan\", 1.5)\n}\n",
    ] {
        let p = crate::parser::parse(ok, "test");
        let d = crate::typecheck::typecheck(&p.ast, "test").diagnostics;
        assert!(
            !d.iter().any(|x| x.code == "WS030"),
            "should not warn WS030: {ok}\n{:?}",
            d.iter()
                .map(|x| (x.code.to_string(), x.message.clone()))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn clock_enabled_accepts_variable_wire() {
    // `enabled` is a wire input now, so a variable is valid (no WS028).
    let src = "in flag: bool\nstatic var n: int = 0\non Clock(interval = 2.0, enabled = flag) {\n  n = n + 1\n}\n";
    assert!(
        errors(src).is_empty(),
        "enabled wire should accept a variable: {:?}",
        errors(src)
    );
}

// ---- zone / teleport reference types -------------------------------------

#[test]
fn zone_input_accepts_zone_and_rejects_other() {
    // A `zone`-typed value wires into `ZoneEntered`'s `zone` input cleanly.
    let ok = "in z: zone\non ZoneEntered(character, zone = z) {}\n";
    assert!(
        errors(ok).is_empty(),
        "a zone value should wire into the zone input: {:?}",
        errors(ok)
    );
    // A non-zone (entity) wired into the `zone` input is a type mismatch.
    let bad = "in z: entity\non ZoneEntered(character, zone = z) {}\n";
    assert!(
        errors(bad).contains(&"WS003".to_string()),
        "entity into a zone input should be WS003, got {:?}",
        errors(bad)
    );
}

#[test]
fn teleport_gate_requires_a_teleport_point() {
    // Teleport takes a teleport point — a `teleport` reference wires cleanly.
    let ok = "in e: entity\nin p: teleport\nin go: exec\non go {\n  e.Teleport(p)\n}\n";
    assert!(
        errors(ok).is_empty(),
        "a teleport point should teleport cleanly: {:?}",
        errors(ok)
    );
    // A raw position (vector) no longer teleports — that's SetLocation's job.
    let vec_dest = "in e: entity\nin go: exec\non go {\n  e.Teleport(Vec(0.0, 0.0, 100.0))\n}\n";
    assert!(
        errors(vec_dest).contains(&"WS003".to_string()),
        "a vector destination should be rejected, got {:?}",
        errors(vec_dest)
    );
    // Neither does an entity.
    let ent_dest = "in e: entity\nin d: entity\nin go: exec\non go {\n  e.Teleport(d)\n}\n";
    assert!(
        errors(ent_dest).contains(&"WS003".to_string()),
        "an entity destination should be rejected, got {:?}",
        errors(ent_dest)
    );
}

#[test]
fn reference_types_cannot_be_selected() {
    // A `zone`/`teleport` reference can't flow through an if-expr's Select gate.
    let zone = "in c: bool\nin a: zone\nin b: zone\nlet z = if c then a else b\n";
    assert!(
        errors(zone).contains(&"WS031".to_string()),
        "selecting between zones should be WS031, got {:?}",
        errors(zone)
    );
    let tp = "in c: bool\nin a: teleport\nin b: teleport\nlet p = if c then a else b\n";
    assert!(
        errors(tp).contains(&"WS031".to_string()),
        "selecting between teleport points should be WS031, got {:?}",
        errors(tp)
    );
}

#[test]
fn reference_types_cannot_be_stored() {
    // Like a var ref, `zone`/`teleport` can't back a variable gate.
    for src in ["var z: zone\n", "var p: teleport\n"] {
        assert!(
            errors(src).contains(&"WS025".to_string()),
            "storing a reference type should be WS025: {src} -> {:?}",
            errors(src)
        );
    }
}

#[test]
fn baked_map_literal_rejects_non_string_key_for_string_map() {
    // A baked map entry has no gate to run a string-format coercion through,
    // so a non-string constant key/value must be rejected, not silently "".
    for src in [
        "var m: Dict<string, int> = { 1 => 10 }\n",
        "var m: Dict<string, int> = { :alice => 1, :bob => 2 }\n",
        "var v: Dict<int, string> = { 1 => 5 }\n",
    ] {
        assert!(
            errors(src).contains(&"WS003".to_string()),
            "non-string baked key/value should be WS003: {src} -> {:?}",
            errors(src)
        );
    }
    // A correctly-typed string-keyed map still compiles clean.
    let ok = "var m: Dict<string, int> = { \"a\" => 1, \"b\" => 2 }\n";
    assert!(errors(ok).is_empty(), "valid string map should compile: {:?}", errors(ok));
}

#[test]
fn ws030_sees_send_inside_captured_event() {
    // A SendCustomEvent inside a captured event body (`let h = on go { … }`)
    // must be seen by the WS030 sender/receiver type check.
    let src = "on CustomEvent(\"hp\", amount: float) { let x = amount }\n\
               let h = on go { SendCustomEvent(\"hp\", 5) }\n";
    let d = crate::typecheck::typecheck(&crate::parser::parse(src, "t").ast, "t").diagnostics;
    assert!(
        d.iter().any(|x| x.code == "WS030"),
        "WS030 must see a send inside a captured event body: {:?}",
        d.iter().map(|x| x.code.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn any_annotation_warns_but_storage_and_probe_do_not() {
    let diags = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t").diagnostics;
    let has = |s: &str, code: &str, sev: crate::diagnostic::Severity|
        diags(s).iter().any(|x| x.code == code && x.severity == sev);
    use crate::diagnostic::Severity;
    // param + output annotations `any` → warn
    assert!(has("mod f(a: any) -> any { return a }\n", "WS032", Severity::Warning),
        "any param/output should warn");
    // in-port `any` → warn
    assert!(has("in x: any\n", "WS032", Severity::Warning), "any in-port should warn");
    // Opaque(...) probe VALUE is not an annotation → no warn
    assert!(!diags("in x: int\nlet y = Opaque(x)\n").iter().any(|z| z.code == "WS032"),
        "Opaque() value must not warn");
    // storage `any` is WS025 (error), and must NOT also emit the WS032 warning
    assert!(has("var v: any = 0\n", "WS025", Severity::Error), "storage any is WS025 error");
    assert!(!diags("var v: any = 0\n").iter().any(|z| z.code == "WS032"),
        "storage any must not double-fire WS032");
}

#[test]
fn any_annotation_warns_on_compound_and_stmt_out() {
    let diags = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t").diagnostics;
    let has = |s: &str, code: &str, sev: crate::diagnostic::Severity|
        diags(s).iter().any(|x| x.code == code && x.severity == sev);
    let count = |s: &str, code: &str| diags(s).iter().filter(|x| x.code == code).count();
    use crate::diagnostic::Severity;
    // `*any` (Ref-wrapped) ref param → warn (shallow matches! would miss it)
    assert!(has("mod inc(v: *any) { v = v }\n", "WS032", Severity::Warning),
        "*any ref param should warn");
    // `any[]` (Array-wrapped) param → warn
    assert!(has("mod f(a: any[]) -> int { return 0 }\n", "WS032", Severity::Warning),
        "any[] param should warn");
    // anon-chip statement-level `out` annotation `any` → warn (Stmt::OutBinding path)
    assert!(has("in go: exec\nchip { @bottom out done: any = 5 }\n", "WS032", Severity::Warning),
        "anon-chip statement-level out any should warn");
    // fires once per annotation (at the top-level range), not once per nested Opaque
    assert_eq!(count("mod inc(v: *any) { v = v }\n", "WS032"), 1, "one warn for *any param");
    assert_eq!(count("mod f(a: any[]) -> int { return 0 }\n", "WS032"), 1, "one warn for any[] param");
    assert_eq!(count("in go: exec\nchip { @bottom out done: any = 5 }\n", "WS032"), 1,
        "one warn for anon-chip out");
}

#[test]
fn generic_mod_type_params_resolve_not_ws002() {
    let errs = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
        .diagnostics.into_iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string()).collect::<Vec<_>>();
    // T is a recognized type param -> NO WS002 for T (sig + body annotation)
    let e = errs("mod pick<T>(a: T, b: T) -> T {\n  let x: T = a\n  return x\n}\n");
    assert!(!e.contains(&"WS002".to_string()), "T should resolve, not WS002: {e:?}");
    // a bound param resolves too
    let e2 = errs("mod clamp<T: Numeric>(v: T) -> T { return v }\n");
    assert!(!e2.contains(&"WS002".to_string()), "bounded T should resolve: {e2:?}");
    // an actually-unknown type still errors WS002
    let e3 = errs("mod bad<T>(a: T) -> T {\n  let y: Q = a\n  return a\n}\n");
    assert!(e3.contains(&"WS002".to_string()), "unknown Q should be WS002: {e3:?}");
    // type params do NOT leak out: a non-generic mod using `T` still errors
    let e4 = errs("mod plain(a: T) -> int { return 0 }\n");
    assert!(e4.contains(&"WS002".to_string()), "T outside a generic mod should be WS002: {e4:?}");
}

#[test]
fn generic_mod_call_inference() {
    let errs = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
        .diagnostics.into_iter().filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string()).collect::<Vec<_>>();
    // Error diagnostics as (code, message) pairs — used where the message text
    // itself is load-bearing (proving the *concrete* substituted type).
    let err_msgs = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
        .diagnostics.into_iter().filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| (d.code.to_string(), d.message)).collect::<Vec<_>>();
    let pick = "in flag: bool\nmod pick<T>(c: bool, a: T, b: T) -> T { return a }\n";
    // inference succeeds: pick(flag, 1, 2) => T=int, no error
    assert!(errs(&format!("{pick}let x = pick(flag, 1, 2)\n")).is_empty(),
        "int inference should be clean: {:?}", errs(&format!("{pick}let x = pick(flag, 1, 2)\n")));
    // result type is CONCRETELY float (not any, not a leaked `Param("T")`):
    // int + float safe-widens T to float, and assigning the result to a
    // vector-typed var errors, AND the error MESSAGE names `Float` as the
    // actual type. (A `let` annotation mismatch is only a WS016 *warning*
    // here — `var`'s initializer coercion is the hard WS003 error, so
    // that's the construct that proves the result type.) Asserting the message
    // (not just the WS003 code) is deliberate: `coerce(Param("T"), Vector)` is
    // ALSO a Mismatch → WS003, so a regression that stopped substituting and
    // leaked a raw `Param` would still emit WS003 — only the "got Float" text
    // proves substitution actually produced the concrete widened `float`.
    let vecerr = err_msgs(&format!("{pick}var v: vector = pick(flag, 1, 2.0)\n"));
    assert!(vecerr.iter().any(|(c, m)| c == "WS003" && m.contains("float")),
        "int+float widened result into vector var should WS003 whose message names Float \
         (proves result is concrete float, not any / leaked Param): {vecerr:?}");
    // widening: int + float have no strict-equality agreement but DO share a
    // safe widening (int -> float) -> T=float, clean (NOT a WS033 conflict —
    // this used to be the strict-equality conflict case pre-widening).
    assert!(errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n")).is_empty(),
        "int+float should widen to float, not conflict: {:?}",
        errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n")));
    // incompatible: int vs vector share no common widening -> WS033
    let e = errs(&format!("in vecArg: vector\n{pick}let x = pick(flag, 1, vecArg)\n"));
    assert!(e.iter().any(|c| c == "WS033"), "int/vector incompatible should be WS033: {e:?}");
    // out of mask: string is not Numeric. (Named `onlyNumeric`, not `clamp` —
    // `clamp` collides with the builtin math function of the same name, which
    // `find_call` resolves before user symbols and so never reaches this
    // user-mod inference path at all.)
    let only_numeric = "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\n";
    let e2 = errs(&format!("{only_numeric}let x = onlyNumeric(\"hi\")\n"));
    assert!(e2.iter().any(|c| c == "WS033"), "string out of Numeric mask should be WS033: {e2:?}");
    // vector inference: pick(flag, va, vb) => T=vector, clean. (`pick` already
    // declares `in flag: bool`; only `va`/`vb` are new here — redeclaring
    // `flag` would itself be a duplicate-decl error unrelated to inference.)
    let vecsrc = format!("in va: vector\nin vb: vector\n{pick}let r = pick(flag, va, vb)\n");
    assert!(errs(&vecsrc).is_empty(), "vector inference should be clean: {:?}", errs(&vecsrc));
    // Ref-param inference: a `*T` param resolves to `Ref(Param(T))`, but a var
    // arg auto-derefs to its inner type — the Ref layers must be aligned before
    // collecting or `T` is never pinned. Ref params are core idiomatic
    // Wirescript (`mod inc(v: *int)`), so this MUST infer cleanly (no WS033).
    let e3 = errs("mod passRef<T>(v: *T) { }\nvar x: int = 0\npassRef(x)\n");
    assert!(!e3.iter().any(|c| c == "WS033"),
        "ref-param generic call should infer cleanly (no WS033): {e3:?}");
    // ...and a ref param that's actually written through (the `inc` shape) works too.
    let e4 = errs("mod inc<T>(v: *T) { v = v }\nvar y: int = 0\ninc(y)\n");
    assert!(!e4.iter().any(|c| c == "WS033"),
        "ref-param write-through generic call should infer cleanly (no WS033): {e4:?}");
}

#[test]
fn generic_widening_inference() {
    let errs = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
        .diagnostics.into_iter().filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string()).collect::<Vec<_>>();
    let pick = "in flag: bool\nmod pick<T>(c: bool, a: T, b: T) -> T { return a }\n";
    // int + float widens to float -> clean (was a strict-equality error)
    assert!(errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n")).is_empty(),
        "int+float should widen to float: {:?}", errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n")));
    // int + vector don't widen -> WS033
    let e = errs(&format!("in v: vector\n{pick}let x = pick(flag, 1, v)\n"));
    assert!(e.iter().any(|c| c == "WS033"), "int+vector incompatible should be WS033: {e:?}");
    // character + entity widens to entity -> clean
    let obj = "in flag: bool\nin ch: character\nin en: entity\nmod pick2<T>(c: bool, a: T, b: T) -> T { return a }\n";
    assert!(errs(&format!("{obj}let r = pick2(flag, ch, en)\n")).is_empty(),
        "character+entity should widen to entity: {:?}", errs(&format!("{obj}let r = pick2(flag, ch, en)\n")));
}

#[test]
fn generic_body_checked_per_mask_member() {
    let errs = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
        .diagnostics.into_iter().filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string()).collect::<Vec<_>>();
    // `a + 1` is valid for every Scalar member (int + 1 -> int, float + 1 -> float),
    // so the generic body checks CLEAN once we check per concrete member.
    let ok = "mod addOne<T: Scalar>(a: T) -> T { return a + 1 }\n";
    assert!(errs(ok).is_empty(), "a + 1 valid for all Scalar members should be clean: {:?}", errs(ok));
    // `Dot()` takes two vectors — invalid for every Scalar member (int/float
    // aren't vectors) — so the body fails per-member checking and surfaces
    // an error at the DEFINITION. (Confirmed in Step 0: `a.length()` is
    // NOT a counterexample here — method-call resolution doesn't check the
    // receiver type strictly, so it silently passes on int/float too;
    // `Dot()`'s WS003 "expected Vector, got Int/Float" argument-type check
    // is a genuine per-member failure.)
    let bad = "mod wrong<T: Scalar>(a: T) -> float { return a.Dot(a) }\n";
    assert!(!errs(bad).is_empty(), "member-invalid op should error at the generic definition: {:?}", errs(bad));
    // the error surfaces ONCE (not once per member) — dedup check:
    assert!(errs(bad).len() <= 2, "member errors should be deduped, got {:?}", errs(bad));
    // a non-generic mod is unaffected
    assert!(errs("mod plain(a: int) -> int { return a + 1 }\n").is_empty());
}

#[test]
fn if_expr_and_builtins_widen() {
    let errs = |s: &str| crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
        .diagnostics.into_iter().filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| (d.code.to_string(), d.message.clone())).collect::<Vec<_>>();
    let codes = |s: &str| errs(s).into_iter().map(|(c, _)| c).collect::<Vec<_>>();

    // if-expr: int/float branches widen to float -> clean.
    assert!(codes("in c: bool\nlet x = if c then 1 else 2.0\n").is_empty(),
        "if int/float should widen to float: {:?}", errs("in c: bool\nlet x = if c then 1 else 2.0\n"));
    // Same, branches swapped: under the OLD "else-branch wins" rule this
    // resolved to `int` (the else type) and was clean; under widening it's
    // `float` either way -- still clean, but now proven by the assignment
    // check below rather than by luck of which branch is which.
    assert!(codes("in c: bool\nlet y = if c then 2.0 else 1\n").is_empty(),
        "if float/int (branches swapped) should also widen to float: {:?}",
        errs("in c: bool\nlet y = if c then 2.0 else 1\n"));
    // result is float: assigning to a vector annotation errors "got Float".
    // (`var`'s initializer coercion is the hard WS003 error -- a `let`
    // annotation mismatch is only a WS016 warning here, per the `pick`
    // tests above, so `var` is what proves the concrete result type.)
    // Proves the *swapped* branch order also now yields Float (old else-wins
    // code would have produced Int here, whose WS003 message would say
    // "Int", not "Float").
    let swapped_vec_err = errs("in c: bool\nvar v: vector = if c then 2.0 else 1\n");
    assert!(swapped_vec_err.iter().any(|(cd, m)| cd == "WS003" && m.contains("float")),
        "if-expr result (else-wins branch order) should still be Float under widening: {:?}",
        swapped_vec_err);
    let vec_err = errs("in c: bool\nvar v: vector = if c then 1 else 2.0\n");
    assert!(vec_err.iter().any(|(cd, m)| cd == "WS003" && m.contains("float")),
        "if-expr result should be Float: {:?}", vec_err);
    // incompatible branches still error (WS003, no common widening).
    assert!(codes("in c: bool\nin v: vector\nlet x = if c then 1 else v\n").iter().any(|c| c == "WS003"),
        "int/vector branches should still error");

    // Builtin half: `union_output_type` is used by the math-variant gates
    // whose declared output is `Type::Union` -- confirmed (via catalog/calls.rs)
    // to be `Blend`, `lerp`, and `Easing` (all built from `blend_variant()` ==
    // Union([Float, Int, Vector, Rotator, Quat, Color])). `Tween`'s output is a
    // *Record* wrapping a union field, not a bare Union, so it doesn't go
    // through this path. Use `lerp(a, b, t)` -- the plainest signature.
    assert!(codes("let x = lerp(1, 2.0, 0.5)\n").is_empty(),
        "lerp(int, float, float) should widen to float, clean: {:?}", errs("let x = lerp(1, 2.0, 0.5)\n"));
    // incompatible: int vs vector share no common widening -> WS033 (both
    // ARE individually valid blend-variant members, so this only fails at
    // the join step, not at argument-type checking).
    let e = errs("in vecArg: vector\nlet x = lerp(1, vecArg, 0.5)\n");
    assert!(e.iter().any(|(c, _)| c == "WS033"), "lerp(int, vector) incompatible should be WS033: {e:?}");
    // Blend, receiver form: `a.Blend(b, alpha)` -- same widening, same gate.
    assert!(codes("in a: int\nlet x = a.Blend(2.0, 0.5)\n").is_empty(),
        "a.Blend(2.0, ...) should widen to float, clean: {:?}", errs("in a: int\nlet x = a.Blend(2.0, 0.5)\n"));
}

#[test]
fn generic_chip_type_checks_clean() {
    // Generic *chips* (physical microchips) are now monomorphized per distinct
    // type instantiation at lowering time (one template per `(name, subst)`),
    // so a generic chip decl type-checks clean — the old WS034 hard error is
    // gone. (The per-type lowering + no-shared-grid proof lives in
    // `lower::tests::generics::generic_chip_monomorphizes_per_instantiation`.)
    let gchip = "chip Box<T>(v: T) -> (r: T) { out r = v }\n";
    assert!(
        errors(gchip).is_empty(),
        "generic chip must type-check clean now: {:?}",
        errors(gchip)
    );
    // a generic MOD is likewise clean -- mods inline+monomorphize per call.
    let gmod = "mod pick<T>(a: T) -> T { return a }\n";
    assert!(
        errors(gmod).is_empty(),
        "generic mod must be allowed: {:?}",
        errors(gmod)
    );
    // a non-generic chip is allowed (same syntax, no `<T>`).
    let plain = "chip Box(v: int) -> (r: int) { out r = v }\n";
    assert!(
        errors(plain).is_empty(),
        "non-generic chip must be allowed: {:?}",
        errors(plain)
    );
}

#[test]
fn generic_type_aliases_instantiate() {
    // Pair<int> resolves to { a: int, b: int } — `.a` used where an `int` is
    // expected is clean.
    let ok = "type Pair<T> = { a: T, b: T }\nin p: Pair<int>\nout r: int = p.a\n";
    assert!(errors(ok).is_empty(), "Pair<int>.a should be int, clean: {:?}", errors(ok));
    // Pair<string> resolves to { a: string, b: string } — `.a` no longer
    // coerces to `int` (string doesn't coerce to a numeric), proving `T`
    // actually substituted per instantiation rather than resolving to `any`
    // (which would coerce to anything and stay clean).
    let bad = "type Pair<T> = { a: T, b: T }\nin p: Pair<string>\nout r: int = p.a\n";
    assert!(
        !errors(bad).is_empty(),
        "Pair<string>.a should be string, mismatched against an int out port (T=string): {:?}",
        errors(bad)
    );
    // Not fully applied.
    let bare = "type Pair<T> = { a: T, b: T }\nlet p: Pair = { a: 1, b: 2 }\n";
    assert!(!errors(bare).is_empty(), "bare generic alias should error: {:?}", errors(bare));
    // Wrong arity.
    let arity = "type Pair<T> = { a: T, b: T }\nlet p: Pair<int, float> = { a: 1, b: 2 }\n";
    assert!(!errors(arity).is_empty(), "arity mismatch should error: {:?}", errors(arity));
    // Recursive alias errors (does not hang).
    let recursive = "type L<T> = { head: T, tail: L<T> }\nin x: L<int>\n";
    assert!(!errors(recursive).is_empty(), "recursive generic alias should error: {:?}", errors(recursive));
    // A DOUBLY self-referencing alias would re-expand each occurrence — a
    // depth-only guard blows up exponentially; the in-progress cycle guard must
    // cut it off so this terminates with a diagnostic rather than hanging.
    let tree = "type Tree<T> = { l: Tree<T>, r: Tree<T> }\nin x: Tree<int>\n";
    assert!(!errors(tree).is_empty(), "doubly-recursive generic alias should error, not hang: {:?}", errors(tree));
    // A non-generic alias still works.
    let plain = "type P = { x: int }\nlet q: P = { x: 1 }\n";
    assert!(errors(plain).is_empty(), "non-generic alias should still work: {:?}", errors(plain));
}

/// Regression for the review's Critical 1: a generic-alias record type used at
/// a PORT or a chip param must dissolve into real per-field wire gates at
/// LOWERING — not silently degrade to a single `any` port whose field accesses
/// emit `_Unsupported`/`SplitColor` swizzle gates with zero diagnostics.
/// typecheck alone can't catch this (it was green while the IR was broken), so
/// this asserts on the emitted IR.
#[test]
fn generic_alias_record_port_lowers_to_real_gates() {
    // Top-level record PORT: `Pair<int>` → two int sub-ports feeding a MathAdd.
    let r = compile("type Pair<T> = { a: T, b: T }\nin p: Pair<int>\nout r: int = p.a + p.b\n");
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "generic-alias port field access must not lower to _Unsupported"
    );
    assert!(
        !has_gate(&r, gc::SPLIT_COLOR),
        "generic-alias port field access must not lower to a SplitColor swizzle"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "p.a + p.b on a Pair<int> port must lower to a real MathAdd gate"
    );
    // The port must have dissolved into one sub-port per field (2), not stayed
    // a single collapsed `any` port.
    assert_eq!(
        gate_count(&r, "BrickComponentType_Internal_MicrochipInput"),
        2,
        "Pair<int> port should expand into two int sub-ports"
    );

    // Chip param form — the `resolved_record` site the review named. The
    // generic case must produce IR identical in shape to the non-generic one.
    let rg = compile(
        "type Pt<T> = { foo: T, bar: T }\nchip Sum(p: Pt<int>) -> (r: int) { out r = p.foo + p.bar }\nin a: int\nout total: int = Sum({ foo: a, bar: 2 })\n",
    );
    assert_no_errors(&rg);
    assert!(!has_gate(&rg, "_Unsupported"), "root of generic chip-param program must have no _Unsupported");
    let child_has_add = rg
        .module
        .chips
        .values()
        .any(|c| c.nodes.values().any(|n| n.gate_class == "BrickComponentType_WireGraph_Expr_MathAdd"));
    let child_has_unsupported = rg
        .module
        .chips
        .values()
        .any(|c| c.nodes.values().any(|n| n.gate_class == "_Unsupported"));
    assert!(child_has_add, "generic chip body must lower p.foo + p.bar to a MathAdd");
    assert!(!child_has_unsupported, "generic chip body must have no _Unsupported field-access gates");
}

#[test]
fn deeply_nested_generic_decls_terminate_quickly() {
    // Whole-branch review: the per-mask-member body check is a whole-PATH
    // budget, not per-decl, so nesting generic decls can't multiply the combo
    // count (`13^d` → a 17s+ typecheck / LSP hang). Depth-6 nested generic mods
    // must type-check clean and near-instantly (this test would time out
    // conspicuously in the suite if the budget regressed).
    let src = "mod l0<A>(a: A) -> A {\n\
               mod l1<B>(b: B) -> B {\n\
               mod l2<C>(c: C) -> C {\n\
               mod l3<D>(d: D) -> D {\n\
               mod l4<E>(e: E) -> E {\n\
               mod l5<F>(f: F) -> F { return f }\n return e }\n return d }\n\
               return c }\n return b }\n return a }\n\
               in go: exec\nin n: int\non go { let z = l0(n) }\n";
    let parsed = crate::parser::parse(src, "t");
    let tc = crate::typecheck::typecheck(&parsed.ast, "t");
    let errs: Vec<String> = tc
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string())
        .collect();
    assert!(errs.is_empty(), "deep nested generics must check clean: {errs:?}");
}
