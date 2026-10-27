//! Builtin calls and receivers: the gate each lowers to, its baked data and
//! the ports it wires.

use super::*;

/// The `Asset` literal on `class`'s property `prop`, as `(type, name)`.
fn asset_prop<'a>(r: &'a LowerResult, class: &str, prop: &str) -> (&'a str, &'a str) {
    let node = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == class)
        .unwrap_or_else(|| panic!("expected a {class} gate"));
    match node.properties.get(&crate::intern::intern(prop)) {
        Some(crate::ir::Literal::Asset {
            asset_type,
            asset_name,
        }) => (asset_type, asset_name),
        other => panic!("expected an asset {prop} on {class}, got {other:?}"),
    }
}

#[test]
fn vector_arithmetic_lowers_to_math_gates() {
    // vec ⊕ vec lowers to the shared PrimMathVariant math gates.
    let r = compile_ok(
        "let a = Vec(1.0, 2.0, 3.0)\nlet b = Vec(4.0, 5.0, 6.0)\nout s = a + b\nout d = a - b",
    );
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "vec + vec should lower to MathAdd"
    );
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathSubtract"),
        "vec - vec should lower to MathSubtract"
    );
}

#[test]
fn constant_vec_inlines_into_var_set() {
    // `v = Vec(...)` in a handler folds to component data on the Var_Set gate, with
    // no MakeVector gate.
    let r = compile_ok("var v: vector\nin t: exec\non t { v = Vec(1.0, 2.0, 3.0) }");
    assert!(
        !has_gate_deep(&r, gc::MAKE_VECTOR),
        "constant Vec assigned to a var should inline, not spawn MakeVector"
    );
}

#[test]
fn constant_vec_inlines_into_math() {
    // Math gates take prim-math variant data, which carries Vector members.
    let r = compile_ok("in v: vector\nout o = v + Vec(0.0, 0.0, 1.0)");
    assert!(has_gate(&r, "BrickComponentType_Expr_MathAdd"));
    assert!(
        !has_gate_deep(&r, gc::MAKE_VECTOR),
        "constant Vec math operand should inline, not spawn MakeVector"
    );
}

#[test]
fn constant_vec_materializes_for_split_vector() {
    // `.x` lowers to SplitVector, whose Input is a plain struct field: the constant
    // stays a wired MakeVector rather than silently zeroing out.
    let r = compile_ok("let p = Vec(1.0, 2.0, 3.0)\nout x = p.x");
    assert!(has_gate(&r, gc::SPLIT_VECTOR));
    assert!(
        has_gate(&r, gc::MAKE_VECTOR),
        "component access on a folded constant needs a materialized MakeVector"
    );
}

#[test]
fn timer_call_lowers_with_exec_controls_and_outputs() {
    // Timer is a function-call instance: optional exec controls wire in, and
    // its Time/Expired outputs are usable as a value / an event.
    let r = compile_ok(
        "in trigger: exec\nlet done: exec\nlet t = Timer(10.0, restart = trigger)\n\
         out elapsed = t.Time\non t.Expired { emit done }",
    );
    assert!(
        has_gate(&r, gc::STOPWATCH),
        "Timer() should lower to a Pseudo_Timer gate"
    );
}

#[test]
fn get_aim_is_one_gate_with_both_ports() {
    // `c.GetAim().Origin` / `.Direction` resolve to a single GetAim gate,
    // with each field accessor wired from its own output port.
    let r = compile_ok(
        "var c: character\nin fire: exec\non fire {\n  let aim = c.GetAim()\n  c.SetLocation(aim.Origin)\n  c.SetVelocity(linear = aim.Direction)\n}",
    );
    let aim_id = find_gate(&r, gc::CHARACTER_GET_AIM);
    let source_ports: crate::collections::HashSet<&str> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == aim_id)
        .map(|w| w.source.port.as_str())
        .collect();
    assert!(
        source_ports.contains("Origin"),
        "Origin port not wired: {source_ports:?}"
    );
    assert!(
        source_ports.contains("Direction"),
        "Direction port not wired: {source_ports:?}"
    );
}

#[test]
fn give_weapon_lowers_to_set_inventory_entry_with_asset() {
    // `char.GiveWeapon($BRItemBase/Weapon_Pistol, 0)` lowers to the
    // SetInventoryEntry gate, carrying the weapon as an asset property.
    let r = compile_ok("in p: character\non p {\n  p.GiveWeapon($BRItemBase/Weapon_Pistol, 0)\n}");
    assert_eq!(
        asset_prop(&r, gc::CHARACTER_SET_INVENTORY_ENTRY, "ItemTypeIfItem"),
        ("BRItemBase", "Weapon_Pistol")
    );
}

#[test]
fn messaging_builtins_lower_to_their_gates() {
    // Per-controller chat/message-box plus global chat/status broadcasts.
    let r = compile_ok(
        "in p: character\non p {\n  let c = p.ControllerOf()\n  c.ShowChatMessage(\"psst\")\n  c.ShowMessageBox(\"body\", title = \"note\")\n  BroadcastChatMessage(\"hello everyone\")\n  BroadcastStatusMessage(\"round over\", flash = true)\n}",
    );
    for class in [
        gc::PLAYERSTATE_SHOW_CHAT,
        gc::PLAYERSTATE_SHOW_MESSAGE_BOX,
        gc::GAMEMODE_BROADCAST_CHAT,
        gc::GAMEMODE_BROADCAST_STATUS,
    ] {
        assert!(has_gate(&r, class), "expected a {class} gate");
    }
}

#[test]
fn play_audio_builtins_carry_audio_asset() {
    // The `$BrickOneShotAudioDescriptor/...` ref inlines into the gate's
    // AudioDescriptor data field (registered as an external asset at emit).
    let r = compile_ok(
        "in p: character\non p {\n  p.PlayAudioAt($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press, volume = 0.5)\n  PlayGlobalAudio($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press)\n  p.PlayClientAudio($BrickOneShotAudioDescriptor/BOSA_Buttons_Button_1_Press, volume = 0.5, pitch = 1.2)\n}",
    );
    let want = ("BrickOneShotAudioDescriptor", "BOSA_Buttons_Button_1_Press");
    assert_eq!(asset_prop(&r, gc::PLAY_AUDIO_AT, "AudioDescriptor"), want);
    assert!(
        has_gate(&r, gc::PLAY_GLOBAL_AUDIO),
        "expected a PlayGlobalAudio gate"
    );
    // PlayClientAudio: same asset inlining, plus a wired Player input.
    assert_eq!(
        asset_prop(&r, gc::PLAY_CLIENT_AUDIO, "AudioDescriptor"),
        want
    );
    let client_id = find_gate(&r, gc::PLAY_CLIENT_AUDIO);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == client_id && w.target.port.as_str() == "Player"),
        "PlayClientAudio's Player input should be wired"
    );
}

#[test]
fn entity_tags_lower_to_tag_gates() {
    let r = compile_ok(
        "in p: character\non p {\n  p.SetTag(\"slot3\")\n  let t = p.GetTag()\n  p.DisplayText(\"tag ${t}\")\n}",
    );
    assert!(has_gate(&r, gc::ENTITY_SET_TAG));
    assert!(has_gate(&r, gc::ENTITY_GET_TAG));
}

#[test]
fn find_player_is_exec_and_change_detector() {
    // `FindPlayer` is an exec gate (Exec/ExecOut) that emits a character, so it
    // runs inside an exec handler. `Change` targets the Exec change detector
    // (which carries the OnChanged output); the plain-bool gate is `Changed`.
    let r = compile_ok(
        "in name: string\n\
         in go: exec\n\
         static var p: character\n\
         on go { p = FindPlayer(name) }\n\
         out changed = Change(name)",
    );
    assert!(has_gate(&r, gc::FIND_PLAYER));
    assert!(has_gate(&r, gc::CHANGE_DETECTOR_EXEC));
}

#[test]
fn quat_make_split_dot_lower_to_their_gates() {
    let r = compile_ok(
        "let q = Quat(0.0, 0.0, 0.0, 1.0)\nlet s = q.SplitQuat()\nout w = s.W\nout d = q.QuatDot(q)",
    );
    assert!(has_gate(&r, gc::MAKE_QUATERNION));
    assert!(has_gate(&r, gc::SPLIT_QUATERNION));
    assert!(has_gate(&r, gc::QUAT_DOT_PRODUCT));
}

#[test]
fn inventory_family_carries_asset_properties() {
    let r = compile_ok(
        "in p: character\non p {\n  p.AddInventoryItem($BRItemBase/Weapon_Pistol)\n  p.SetInventoryItem($BRItemBase/Weapon_Bow, slot = 2)\n  p.AddInventoryItemAdv($BRItemBase/Weapon_Pistol, damage = 2.0, itemName = \"Big Iron\")\n}",
    );
    assert_eq!(
        asset_prop(&r, gc::CHARACTER_ADD_INVENTORY_ITEM, "Item").1,
        "Weapon_Pistol"
    );
    assert!(has_gate(&r, gc::CHARACTER_SET_INVENTORY_ITEM));
    assert!(has_gate(&r, gc::CHARACTER_ADD_INVENTORY_ITEM_ADV));
}

#[test]
fn adv_inventory_projectile_scale_reaches_its_port() {
    let r = compile_ok(
        "in p: character\nin s: float\non p {\n  p.AddInventoryItemAdv($BRItemBase/Weapon_Pistol, projectileScale = 2.0)\n  p.SetInventoryItemAdv($BRItemBase/Weapon_Bow, 1, projectileScale = 2.0)\n  p.AddInventoryItemAdv($BRItemBase/Weapon_Bow, projectileScale = s)\n}",
    );
    let port = crate::intern::intern("ProjectileScaleMultiplier");
    let scale_of = |id: &crate::ir::NodeId, n: &crate::ir::Node| {
        let wired = r.module.wires.iter().any(|w| {
            w.target.node_id == *id && w.target.port == WirePort::ProjectileScaleMultiplier
        });
        (n.properties.get(&port).cloned(), wired)
    };
    for class in [
        gc::CHARACTER_ADD_INVENTORY_ITEM_ADV,
        gc::CHARACTER_SET_INVENTORY_ITEM_ADV,
    ] {
        assert!(
            r.module
                .nodes
                .iter()
                .filter(|(_, n)| n.gate_class == class)
                .any(|(id, n)| scale_of(id, n) == (Some(crate::ir::Literal::Float(2.0)), false)),
            "{class} should carry ProjectileScaleMultiplier = 2.0"
        );
    }
    assert!(
        r.module
            .nodes
            .iter()
            .filter(|(_, n)| n.gate_class == gc::CHARACTER_ADD_INVENTORY_ITEM_ADV)
            .any(|(id, n)| scale_of(id, n).1),
        "a runtime projectileScale should wire into ProjectileScaleMultiplier"
    );
}

#[test]
fn has_role_lowers_with_config_role_name() {
    // `ctrl.HasRole("Admin")` lowers to the HasRole gate (RoleName is a config
    // string) and returns a bool.
    let r = compile_ok(
        "in p: character\non p {\n  let c = p.ControllerOf()\n  let a = c.HasRole(\"Admin\")\n  if a { p.DisplayText(\"hi\") }\n}",
    );
    assert!(
        has_gate(&r, gc::PLAYERSTATE_HAS_ROLE),
        "expected a HasRole gate"
    );
}

#[test]
fn inline_multi_output_call_reads_its_own_port() {
    // A swizzle-named field on an INLINE call reads that call's own output port,
    // as a let-bound result does. Claimed by the Split* synthesis instead, `.Yaw`/`.y`
    // would split the call's PRIMARY output a second time and read the wrong
    // component.
    let src = "in d: vector\n\
               out yaw = d.ToRotation().ToEuler().Yaw\n\
               out y = d.SplitVec().y";
    let r = compile_ok(src);
    for (class, port, expr) in [
        (
            gc::SPLIT_ROTATION,
            WirePort::Yaw,
            "d.ToRotation().ToEuler().Yaw",
        ),
        (gc::SPLIT_VECTOR, WirePort::Y, "d.SplitVec().y"),
    ] {
        assert_eq!(
            gate_count(&r, class),
            1,
            "`{expr}` must read the split it already emitted, not emit a second one"
        );
        let id = find_gate(&r, class);
        let out_ports: Vec<WirePort> = r
            .module
            .wires
            .iter()
            .filter(|w| w.source.node_id == id)
            .map(|w| w.source.port)
            .collect();
        assert!(
            out_ports.contains(&port),
            "`{expr}` must read the {port:?} port; source ports: {out_ports:?}"
        );
    }
}

#[test]
fn rotation_quat_color_receivers_lower_to_their_gates() {
    // The rotation/quaternion + sRGB/hex color receivers and
    // constructors lower to their gates. `quat` is a distinct type from the
    // euler `rotator`.
    let src = "in p: character\non p {\n  \
        let dir = Vec(1.0, 0.0, 0.0)\n  \
        let q = dir.ToRotation()\n  let v = q.ToDirection()\n  \
        let spun = dir.Rotate(q)\n  let inv = q.Invert()\n  \
        let q2 = dir.RotationTo(Vec(0.0, 1.0, 0.0))\n  \
        let ang = q.AngleTo(q2)\n  let mid = q.Slerp(q2, 0.5)\n  \
        let aa = q.ToAxisAngle()\n  let fa = dir.RotationByAngle(1.57)\n  \
        let rot = Rotation(0.0, 90.0, 0.0)\n  let e = rot.ToEuler()\n  \
        let c = ColorSRGB(255, 128, 0, 255)\n  let h = c.ToHex()\n  \
        let c2 = ColorHex(\"#ff8800\")\n  let s = c.ToSRGB()\n  \
        let bl = c.ColorBlend(c2, 0.5)\n  let bm = c.Blend(c2, 0.5)\n  \
        p.DisplayText(\"${ang} ${h}\")\n}";
    let r = compile_ok(src);
    for class in [
        gc::DIRECTION_TO_ROTATION,
        gc::ROTATION_TO_DIRECTION,
        gc::ROTATE_VECTOR,
        gc::INVERT_ROTATION,
        gc::QUAT_BETWEEN,
        gc::QUAT_ANGLE_BETWEEN,
        gc::QUAT_SLERP,
        gc::QUAT_TO_AXIS_ANGLE,
        gc::QUAT_FROM_AXIS_ANGLE,
        gc::MAKE_ROTATION,
        gc::SPLIT_ROTATION,
        gc::MAKE_COLOR_SRGB,
        gc::MAKE_COLOR_HEX,
        gc::SPLIT_COLOR_SRGB,
        gc::COLOR_TO_HEX,
        // `ColorBlend` is the colour-space blend gate; `Blend` is the math
        // blend gate (an alias for `lerp`), which takes colours as one of its
        // variants. Both are reachable and lower to different gates.
        gc::COLOR_BLEND,
        gc::MATH_BLEND,
    ] {
        assert!(has_gate(&r, class), "missing gate {class}");
    }
}

#[test]
fn rotator_constant_bakes_into_a_quat_port() {
    // `RotateVector.Rotation` is a `Quat` field (X/Y/Z/W) while `Rotation(p,y,r)`
    // folds to a `Literal::Rotator` (Pitch/Yaw/Roll). The two are interchangeable
    // on a WIRE (`types::coerce`, which is why the call type-checks), so the
    // constant inlines into the gate's data like any other and emit converts it
    // to the field's representation. It must NOT grow a carrier gate; a
    // rotation known at compile time costs no gate at all.
    let r = compile_ok("in v: vector\nout o = v.Rotate(Rotation(0.0, -90.0, 0.0))");
    assert_eq!(
        count_class(&r.module, gc::MAKE_ROTATION),
        0,
        "a compile-time rotation into a quaternion port should bake, not wire"
    );
    let rotate = find_gate(&r, gc::ROTATE_VECTOR);
    assert_eq!(
        r.module.nodes[&rotate]
            .properties
            .get(&crate::intern::intern("Rotation")),
        Some(&crate::ir::Literal::Rotator {
            pitch: 0.0,
            yaw: -90.0,
            roll: 0.0
        }),
        "the rotation belongs in RotateVector's own data"
    );
    assert!(
        !r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == rotate && w.target.port == WirePort::Rotation),
        "a baked rotation leaves the Rotation pin unwired"
    );
}

#[test]
fn rotator_and_quat_components_split_like_a_vector_does() {
    // A rotation's Pitch/Yaw/Roll and a quaternion's X/Y/Z/W are readable through
    // their Split gates, exactly as a vector's X/Y/Z are: `.pitch` lowers to a real
    // split, and `.x` on a quat is accepted.
    let r = compile_ok(
        "in v: vector\nin r: rotator\nin q: quat\n\
         out a = v.x\nout b = r.pitch\nout c = r.roll\nout d = q.x\nout e = q.w",
    );
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.message.contains("not yet supported")),
        "no component should fall to a placeholder: {:?}",
        r.diagnostics
    );
    // One gate per split value, shared across that value's components.
    assert_eq!(gate_count(&r, gc::SPLIT_VECTOR), 1);
    assert_eq!(
        gate_count(&r, gc::SPLIT_ROTATION),
        1,
        "r.pitch and r.roll should share one SplitRotation"
    );
    assert_eq!(
        gate_count(&r, gc::SPLIT_QUATERNION),
        1,
        "q.x and q.w should share one SplitQuaternion"
    );
    // `.x` is the one component name a vector and a quaternion share, so the
    // gate has to be chosen by the object's type, not by the field name.
    let quat_split = find_gate(&r, gc::SPLIT_QUATERNION);
    for port in ["X", "W"] {
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.source.node_id == quat_split && w.source.port.as_str() == port),
            "the quaternion split should drive its {port} output"
        );
    }
}

#[test]
fn field_access_vector_component_on_local_creates_split() {
    // `let a = ...; a.x` goes through the local-binding path, which splits out the
    // component rather than returning the whole vector port.
    let r = compile_ok(
        "in p: character\non p {\n  let a = Vec(1.0, 2.0, 3.0) + Vec(4.0, 5.0, 6.0)\n  let cx = a.x\n  p.DisplayText(\"${cx}\")\n}",
    );
    assert_eq!(
        gate_count(&r, gc::SPLIT_VECTOR),
        1,
        "expected a SplitVector gate for `.x` on a local"
    );
}

#[test]
fn splitvec_record_fields_reuse_single_split() {
    // `let p = v.SplitVec(); p.x / p.y / p.z` reads the X/Y/Z ports of the one
    // split, not a re-split of p's first (scalar) field. Exactly one SplitVector.
    let r = compile_ok(
        "in pl: character\non pl {\n  let v = Vec(11.0, 22.0, 33.0)\n  let p = v.SplitVec()\n  let s = p.x + p.y + p.z\n  pl.DisplayText(\"${s}\")\n}",
    );
    assert_eq!(
        count_class(&r.module, gc::SPLIT_VECTOR),
        1,
        "p.x/p.y/p.z should reuse the single SplitVector"
    );
}

#[test]
fn detector_builtins_map_to_split_gate_classes() {
    // `Change` (value pulse-through, OnChanged) lives on the Exec gate;
    // `Changed` (bool pulse) is the plain gate. `EdgeExec` fires exec pulses
    // for `on`/`await`, and `Edge`'s rising/falling record fields resolve via
    // the port aliases.
    let src = "in v: float\nin b: bool\nin x: int\nin t: exec\n\
        let c = Change(x)\nlet cd = Changed(x)\nlet e = Edge(b)\nlet ee = EdgeExec(v)\n\
        out cv = c\nout cdv = cd\nout rising = e.Rising\nout falling = e.Falling\n\
        var n: int = 0\non ee.Rising { n = n + 1 }\non ee.Falling { n = n - 1 }";
    let r = compile_ok(src);
    for class in [
        gc::CHANGE_DETECTOR_EXEC,
        gc::CHANGE_DETECTOR,
        gc::EDGE_DETECTOR,
        gc::EDGE_DETECTOR_EXEC,
    ] {
        assert!(has_gate(&r, class), "missing gate {class}");
    }
}

/// `match v.getType()` is one Get Value Type gate whose output each arm compares
/// with its member's number (`Int` = 1, `String` = 7), bound or inline.
#[test]
fn match_on_value_type_compares_the_gate_output() {
    for scrutinee in ["v.getType()", "GetValueType(v)", "t"] {
        let r = compile_ok(&format!(
            "in v: any\nin go: exec\nvar n: int = 0\non go {{\n  let t: Type = v.getType()\n  \
             match {scrutinee} {{\n    Int => {{ n = 1 }}\n    String => {{ n = 2 }}\n    _ => {{ n = 3 }}\n  }}\n}}"
        ));
        let gates: Vec<_> = r
            .module
            .nodes
            .iter()
            .filter(|(_, n)| n.gate_class == gc::GET_VALUE_TYPE)
            .map(|(id, _)| *id)
            .collect();
        let mut compared: Vec<i64> = r
            .module
            .nodes
            .iter()
            .filter(|(id, n)| {
                n.gate_class == gc::COMPARE_EQUAL
                    && r.module.wires.iter().any(|w| {
                        w.target.node_id == **id
                            && gates.contains(&w.source.node_id)
                            && w.source.port == WirePort::Type
                    })
            })
            .filter_map(
                |(_, n)| match n.properties.get(&crate::intern::intern("InputB")) {
                    Some(crate::ir::Literal::Int(k)) => Some(*k),
                    _ => None,
                },
            )
            .collect();
        compared.sort_unstable();
        assert_eq!(
            compared,
            [1, 7],
            "{scrutinee}: each arm compares the gate's Type output"
        );
    }
}

/// `.getType()` is a method of every value, `GetValueType` gives the same `Type`,
/// and members are named through the enum outside a `match`.
#[test]
fn value_type_method_works_on_any_value() {
    let r = compile_ok(
        "in v: any\nin e: entity\nin go: exec\nvar t: Type = Type.Bool\non go {\n  \
         t = v.getType()\n  t = e.getType()\n  t = \"s\".getType()\n  t = GetValueType(7)\n  t = Type.Float\n}",
    );
    assert_eq!(gate_count(&r, gc::GET_VALUE_TYPE), 4);
    let bad = compile("in v: any\nin go: exec\nvar n: int = 0\non go {\n  n = GetValueType(v)\n}");
    crate::test_support::assert_code(&bad.diagnostics, "WS003");
}

/// `SetVisibleTo` wires `Entity` and `VisibleTo` from a player, a character
/// or a team entity, and leaves `VisibleTo` unwired when it is left out.
#[test]
fn set_visible_to_wires_entity_and_visible_to() {
    for (decl, arg) in [
        ("in who: controller", "who"),
        ("in who: character", "who"),
        ("in who: character", "who.GetTeam()"),
        ("in who: entity", "who"),
    ] {
        let r = compile_ok(&format!(
            "in t: exec\nin crate: entity\n{decl}\non t {{\n  crate.SetVisibleTo({arg})\n}}"
        ));
        let g = find_gate(&r, gc::ENTITY_SET_VISIBLE_TO);
        let into = |port| {
            r.module
                .wires
                .iter()
                .filter(|w| w.target.node_id == g && w.target.port == port)
                .count()
        };
        assert_eq!(into(WirePort::Entity), 1, "{decl} / {arg}: Entity");
        assert_eq!(into(WirePort::VisibleTo), 1, "{decl} / {arg}: VisibleTo");
        assert_eq!(into(WirePort::Exec), 1, "{decl} / {arg}: Exec");
    }
    let r = compile_ok("in t: exec\nin crate: entity\non t {\n  SetVisibleTo(crate)\n}");
    let g = find_gate(&r, gc::ENTITY_SET_VISIBLE_TO);
    assert!(
        !r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == g && w.target.port == WirePort::VisibleTo),
        "a left-out visibleTo must stay unwired (everyone sees it)"
    );
    let bad = compile("in t: exec\nin crate: entity\non t {\n  crate.SetVisibleTo(3)\n}");
    assert!(
        bad.diagnostics
            .iter()
            .any(|d| d.code == "WS003" && d.message.contains("controller | character | entity")),
        "an int is not a player or a team: {:?}",
        bad.diagnostics
    );
}

/// `GetInputs` is the exec-form counterpart of the seat splitter: it takes the
/// character on its `Player` port (not `Character`, since the gate also accepts
/// a persistent player) and advances the exec chain through `ExecOut`.
#[test]
fn get_inputs_chains_exec_and_wires_the_player_operand() {
    let src = "in go: exec\n\
               in pl: character\n\
               on go {\n\
                 let inp = pl.GetInputs()\n\
                 pl.ShowStatusMessage(\"${inp.Forward}\")\n\
               }";
    let r = compile_ok(src);
    let gate = find_gate(&r, gc::EXEC_GET_INPUTS);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == gate && w.target.port.as_str() == "Player"),
        "the character must wire into the gate's Player port: {:?}",
        r.module.wires
    );
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target.node_id == gate && w.target.port.as_str() == "Exec"),
        "the handler must drive the gate's Exec input"
    );
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == gate && w.source.port.as_str() == "ExecOut"),
        "the gate must advance the exec chain through ExecOut"
    );
}

/// Each surface field must reach its real port. Two different renamings are in
/// play and both are shared with the splitter: the axes drop an `Input` prefix
/// (`.Forward` -> `InputForward`) and the flags drop a `b` (`.PressedC` ->
/// `bPressedC`). A field that resolved to the gate's default port instead would
/// still compile and would read the wrong control.
#[test]
fn get_inputs_fields_resolve_to_their_game_ports() {
    let src = "in go: exec\n\
               in pl: character\n\
               on go {\n\
                 let inp = pl.GetInputs()\n\
                 pl.ShowStatusMessage(\"${inp.Forward}\")\n\
                 pl.ShowStatusMessage(\"${inp.MouseWheel}\")\n\
                 if inp.PressedC { pl.ShowStatusMessage(\"c\") }\n\
               }";
    let r = compile_ok(src);
    let gate = find_gate(&r, gc::EXEC_GET_INPUTS);
    let ports: Vec<&str> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == gate)
        .map(|w| w.source.port.as_str())
        .collect();
    for want in ["InputForward", "InputMouseWheel", "bPressedC"] {
        assert!(
            ports.contains(&want),
            "expected a wire out of {want}, got {ports:?}"
        );
    }
}

#[test]
fn a_number_into_a_string_port_goes_through_format_text() {
    // types.md "String Coercion": a number wired into a `string` destination
    // arrives as its text, through one FormatText("{0}") each.
    let r = compile_ok(
        "in n: int\nvar label: string = \"\"\nout text: string = n\nout shown = label.Value\non n { label = n * 2 }",
    );
    assert_eq!(
        count_class(&r.module, gc::STRING_FORMAT_TEXT),
        2,
        "one for the out pin, one for the var store"
    );
    // `..` takes the number as it is: its gate types the operand itself.
    let r = compile_ok("in n: int\nin s: string\nout t = s .. n");
    assert_eq!(count_class(&r.module, gc::STRING_FORMAT_TEXT), 0);
}
