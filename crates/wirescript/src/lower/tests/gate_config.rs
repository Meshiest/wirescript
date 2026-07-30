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
