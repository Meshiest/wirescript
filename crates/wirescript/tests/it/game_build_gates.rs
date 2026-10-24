//! Gates from the game build that made several gates procedural bricks and
//! added player dialogs and brick properties. Each test compiles a real
//! program and checks what the game would load: the brick a gate sits on, the
//! wire a branch leaves from, the setting baked into a gate's data.

use wirescript::compile::{CompileInput, FoldMode, compile, compile_to_world};
use wirescript::emit::EmitOptions;
use wirescript::ir::{Module, Node};

use crate::common;

fn input(src: &str) -> CompileInput<'_> {
    common::input(src, "game_build_gates.ws")
}

fn world(src: &str) -> brdb::World {
    compile_to_world(input(src), EmitOptions::default())
        .unwrap_or_else(|e| panic!("should compile: {e:?}"))
        .world
}

/// The bricks carrying a `class` component: `(asset, half-size)`, with the
/// half-size `None` for a basic brick. Sorted, so a test reads as a set.
fn bricks_of(world: &brdb::World, class: &str) -> Vec<(String, Option<(u16, u16, u16)>)> {
    let mut out: Vec<_> = common::all_bricks(world)
        .into_iter()
        .filter(|b| {
            b.components
                .iter()
                .any(|c| c.component_type_ref() == Some(class))
        })
        .map(|b| match &b.asset {
            brdb::BrickType::Procedural { asset, size } => {
                (asset.to_string(), Some((size.x, size.y, size.z)))
            }
            brdb::BrickType::Basic(asset) => (asset.to_string(), None),
        })
        .collect();
    out.sort();
    out
}

fn lowered(src: &str) -> Module {
    let (_, tc, lowered) = common::run_stages(src, "game_build_gates.ws", FoldMode::Auto);
    assert!(tc.diagnostics.is_empty(), "{:?}", tc.diagnostics);
    lowered.module
}

fn node_of<'m>(m: &'m Module, class: &str) -> &'m Node {
    m.nodes
        .values()
        .find(|n| n.gate_class == class)
        .unwrap_or_else(|| panic!("no {class}"))
}

/// The node a wire from `from`.`port` lands on.
fn wired_from<'m>(m: &'m Module, from: &Node, port: &str) -> &'m Node {
    let w = m
        .wires
        .iter()
        .find(|w| w.source.node_id == from.id && w.source.port.as_str() == port)
        .unwrap_or_else(|| panic!("nothing wired from {port}"));
    &m.nodes[&w.target.node_id]
}

/// A custom event's send and receive gates are procedural bricks, one size per
/// count of data values: one value fits the smallest, eight need the largest.
#[test]
fn custom_event_gates_are_sized_by_their_data_count() {
    let w = world(
        "in go: exec\n\
         var a: int = 0\n\
         var r1: int = 0\n\
         var r8: int = 0\n\
         on go {\n\
           a = a + 1\n\
           SendCustomEvent(\"one\", a)\n\
           SendCustomEvent(\"eight\", a, a, a, a, a, a, a, a)\n\
         }\n\
         on CustomEvent(\"one\") -> (x: int) { r1 = x }\n\
         on CustomEvent(\"eight\") -> (x1: int, x2: int, x3: int, x4: int, x5: int, x6: int, x7: int, x8: int) {\n\
           r8 = x1 + x2 + x3 + x4 + x5 + x6 + x7 + x8\n\
         }\n",
    );
    let sized = |asset: &str| {
        vec![
            (asset.to_string(), Some((5, 5, 2))),
            (asset.to_string(), Some((10, 5, 2))),
        ]
    };
    assert_eq!(
        bricks_of(&w, "BrickComponentType_Exec_SendCustomEvent"),
        sized("PB_Gate_Exec_SendCustomEvent")
    );
    assert_eq!(
        bricks_of(&w, "BrickComponentType_Event_Custom"),
        sized("PB_Gate_Event_Custom")
    );
}

/// The sweep result's `HitBrick` is a `brick` value that a brick-property
/// setter takes, and `Color` there names the `BrickProperty` member while
/// `Color(...)` still builds a color.
#[test]
fn a_sweep_hit_brick_is_recolored() {
    let src = "in go: exec\n\
               on go {\n\
                 let r = SweepSimple(1000.0, detectBricks = true)\n\
                 on r.Hit { r.HitBrick.SetBrickProperty(Color, Color(1.0, 0.0, 0.0)) }\n\
               }\n";
    let m = lowered(src);
    let sweep = node_of(&m, "BrickComponentType_Exec_SweepSimple");
    let set = node_of(&m, "BrickComponentType_Exec_SetBrickProperty");
    assert_eq!(
        wired_from(&m, sweep, "Hit").id,
        set.id,
        "the setter runs on a hit"
    );
    assert_eq!(
        wired_from(&m, sweep, "HitBrick").id,
        set.id,
        "the hit brick is the one set"
    );

    use brdb::schema::BrdbValue;
    let brz = compile(input(src)).expect("should compile").brz;
    let comps = common::world_components(&brz);
    let data = comps
        .iter()
        .find(|c| c.get("Property").is_some() && c.get("Value").is_some())
        .expect("the setter's data");
    match data.get("Property") {
        Some(BrdbValue::Enum(e)) => assert_eq!(e.value, 1, "Property = Color"),
        other => panic!("Property should be an EBRBrickProperty, got {other:?}"),
    }
    let value = format!("{:?}", data.get("Value"));
    assert!(
        value.contains("LinearColor"),
        "a color value: {}",
        &value[..value.len().min(400)]
    );
}

/// A Yes/No dialog answers on `YesClicked`, or on `Declined` for every other
/// outcome, each wired to the code that handles that answer.
#[test]
fn a_yes_no_question_branches_on_the_answer() {
    let m = lowered(
        "var answer: int = 0\n\
         on ControllerJoined() -> (c) {\n\
           let r = c.ShowYesNoBox(\"Ready to play?\", title = \"Start\")\n\
           on r.YesClicked { answer = 1 }\n\
           on r.Declined { answer = 2 }\n\
         }\n",
    );
    let dialog = node_of(&m, "BrickComponentType_Exec_PlayerState_ShowYesNoBox");
    let set_value = |n: &Node| {
        n.properties
            .get(&wirescript::intern::intern("Value"))
            .cloned()
    };
    for (port, want) in [("YesClicked", 1), ("Declined", 2)] {
        let target = wired_from(&m, dialog, port);
        assert_eq!(
            target.gate_class, "BrickComponentType_Exec_Var_Set",
            "{port}"
        );
        assert_eq!(
            set_value(target),
            Some(wirescript::ir::Literal::Int(want)),
            "{port}"
        );
    }
}

/// Each dialog's answering player and typed answer are values a program can
/// read, and the message box's new `OkClicked` reaches code too.
#[test]
fn dialogs_expose_the_player_who_answered() {
    let src = "var who: string = \"\"\n\
               var text: string = \"\"\n\
               on ControllerJoined() -> (c) {\n\
                 let t = c.ShowTextInputBox(\"Your name?\")\n\
                 on t.OkClicked { who = t.ClickedBy.GetUserName()\n text = t.Text }\n\
                 let m = c.ShowMessageBox(\"Welcome\")\n\
                 on m.OkClicked { who = m.ClickedBy.GetUserName() }\n\
               }\n";
    lowered(src);
    assert_eq!(
        bricks_of(
            &world(src),
            "BrickComponentType_Exec_PlayerState_ShowTextInputBox"
        ),
        vec![(
            "B_1x1_Gate_Exec_PlayerState_ShowTextInputBox".to_string(),
            None
        )]
    );
}

/// The property member is baked into the gate, not left at the schema
/// default (which happens to be Color), in both the bare and qualified forms.
#[test]
fn the_brick_property_member_is_baked() {
    use brdb::schema::BrdbValue;
    for spelling in ["PlayerCollision", "BrickProperty.PlayerCollision"] {
        let src = format!(
            "in go: exec
             on go {{
               let r = SweepSimple(1000.0, detectBricks = true)
               on r.Hit {{ r.HitBrick.SetBrickProperty({spelling}, false) }}
             }}
"
        );
        let brz = compile(input(&src)).expect("should compile").brz;
        let comps = common::world_components(&brz);
        let data = comps
            .iter()
            .find(|c| c.get("Property").is_some() && c.get("Value").is_some())
            .expect("the setter's data");
        match data.get("Property") {
            Some(BrdbValue::Enum(e)) => assert_eq!(e.value, 5, "{spelling}"),
            other => panic!("Property should be an EBRBrickProperty, got {other:?}"),
        }
    }
}

/// A map can be keyed by bricks, stored as the game's brick handles, and
/// `keys` can list the keys whose brick is gone.
#[test]
fn a_map_keyed_by_bricks_lists_dead_keys_on_request() {
    use brdb::schema::BrdbValue;
    let src = "in go: exec\n\
               var hits: Map<brick, int>\n\
               var seen: brick[]\n\
               on go {\n\
                 let r = SweepSimple(1000.0, detectBricks = true)\n\
                 on r.Hit {\n\
                   hits.set(r.HitBrick, hits.get(r.HitBrick) + 1)\n\
                   hits.keys(seen, includeDead = true)\n\
                 }\n\
               }\n";
    let brz = compile(input(src)).expect("should compile").brz;
    let comps = common::world_components(&brz);
    assert!(
        comps
            .iter()
            .any(|c| matches!(c.get("bIncludeDead"), Some(BrdbValue::Bool(true)))),
        "keys(..., includeDead = true) bakes the flag"
    );
    let map = comps
        .iter()
        .filter_map(|c| c.get("Value"))
        .map(|v| format!("{v:?}"))
        .find(|v| v.contains("DynamicMap") || v.contains("Map"))
        .expect("the map's storage");
    assert!(
        map.contains("brick_handle"),
        "keyed by brick handles: {}",
        &map[..map.len().min(300)]
    );
}

/// A brick-property read is typed by the property it names, both in the
/// program (a `bool` read can't fill a `color`) and on the gate's `Value`
/// port, which is what emit and downstream wiring see.
#[test]
fn a_brick_property_read_has_the_property_type() {
    use wirescript::ir::Type;
    let read = |property: &str| {
        format!(
            "in go: exec\n\
             var keep: any = 0\n\
             on go {{\n\
               let r = SweepSimple(1000.0, detectBricks = true)\n\
               on r.Hit {{ let v = r.HitBrick.GetBrickProperty({property})\n keep = v }}\n\
             }}\n"
        )
    };
    for (property, want) in [
        ("Visible", Type::Bool),
        ("BrickProperty.Color", Type::Color),
        ("PlayerCollision", Type::Bool),
        ("Material", Type::Any),
    ] {
        let (_, _, lowered) =
            common::run_stages(&read(property), "game_build_gates.ws", FoldMode::Auto);
        let get = node_of(&lowered.module, "BrickComponentType_Exec_GetBrickProperty");
        let value = get
            .ports
            .outputs
            .iter()
            .find(|p| wirescript::intern::resolve(p.name) == "Value");
        assert_eq!(value.map(|p| p.ty.clone()), Some(want), "{property}");
    }
    assert!(
        common::diag_codes(
            "in go: exec\nvar col: color = Color(0.0, 0.0, 0.0)\non go {\n\
             let r = SweepSimple(1000.0, detectBricks = true)\n\
             on r.Hit { col = r.HitBrick.GetBrickProperty(Visible) }\n}\n"
        )
        .contains(&"WS003".to_string()),
        "a Visible read is a bool, not a color"
    );
}

/// Get Bricks In Zone left the game, so `fillFromZoneBricks` and its
/// gate-named form no longer compile, and the gate is gone from the inventory.
#[test]
fn get_bricks_in_zone_is_gone() {
    assert!(
        wirescript::catalog::default_catalog()
            .find_by_class("BrickComponentType_Exec_Zone_GetBricks")
            .is_none()
    );
    assert!(!common::compiles(
        "in z: zone\nvar bricks: brick[]\nin go: exec\non go { bricks.fillFromZoneBricks(z) }\n"
    ));
    assert!(!common::compiles(
        "in z: zone\nvar bricks: brick[]\nin go: exec\non go { GetBricksInZone(bricks, z) }\n"
    ));
}

/// `entity.SetVisible(visible)` runs Set Entity Visible with the entity and
/// the flag wired in, on the gate's own brick.
#[test]
fn set_visible_wires_the_entity_and_the_flag() {
    let src = "in e: entity\nin show: bool\nin go: exec\non go { e.SetVisible(show) }\n";
    let m = lowered(src);
    let gate = node_of(&m, "BrickComponentType_Exec_Entity_SetVisible");
    let into = |port: &str| {
        m.wires
            .iter()
            .find(|w| w.target.node_id == gate.id && w.target.port.as_str() == port)
            .map(|w| m.nodes[&w.source.node_id].gate_class)
    };
    assert_eq!(
        into("Entity"),
        Some("BrickComponentType_Internal_MicrochipInput")
    );
    assert_eq!(
        into("bVisible"),
        Some("BrickComponentType_Internal_MicrochipInput")
    );
    assert!(into("Exec").is_some(), "on the handler's exec chain");
    assert_eq!(
        bricks_of(&world(src), "BrickComponentType_Exec_Entity_SetVisible"),
        vec![("B_1x1_Gate_Exec_Entity_SetVisible".to_string(), None)]
    );
    assert!(common::compiles(
        "in e: entity\nin go: exec\non go { SetVisible(e, false) }\n"
    ));
    assert!(
        common::diag_codes(
            "in e: entity\nin go: exec\non go { e.SetVisible(Vec(0.0, 0.0, 1.0)) }\n"
        )
        .contains(&"WS003".to_string()),
        "the flag is a bool"
    );
}

/// Every player dialog fires `Declined` for any outcome but the confirming
/// button, including a dialog the game could not show, as an exec field the
/// program can handle like a button.
#[test]
fn every_dialog_reports_declined() {
    for (call, class) in [
        (
            "ShowMessageBox(\"hi\")",
            "BrickComponentType_Exec_PlayerState_ShowMessageBox",
        ),
        (
            "ShowOkCancelBox(\"ok?\")",
            "BrickComponentType_Exec_PlayerState_ShowOkCancelBox",
        ),
        (
            "ShowYesNoBox(\"yes?\")",
            "BrickComponentType_Exec_PlayerState_ShowYesNoBox",
        ),
        (
            "ShowTextInputBox(\"name?\")",
            "BrickComponentType_Exec_PlayerState_ShowTextInputBox",
        ),
    ] {
        let m = lowered(&format!(
            "var failed: int = 0\n\
             on ControllerJoined() -> (c) {{\n\
               let r = c.{call}\n\
               on r.Declined {{ failed = 1 }}\n\
             }}\n"
        ));
        let dialog = node_of(&m, class);
        let target = wired_from(&m, dialog, "Declined");
        assert_eq!(
            target.gate_class, "BrickComponentType_Exec_Var_Set",
            "{call}"
        );
    }
}

/// The dialogs' per-button cancel/no outputs and `FailedToOpen` merged into
/// `Declined`, and Player Left lost its player reference: a program reading
/// any of them is an error, not a silent no-op.
#[test]
fn removed_dialog_and_player_left_outputs_are_errors() {
    for body in [
        "on ControllerJoined() -> (c) {\n  let r = c.ShowMessageBox(\"hi\")\n  on r.FailedToOpen { n = 1 }\n}\n",
        "on ControllerJoined() -> (c) {\n  let r = c.ShowOkCancelBox(\"ok?\")\n  on r.CancelClicked { n = 1 }\n}\n",
        "on ControllerJoined() -> (c) {\n  let r = c.ShowYesNoBox(\"yes?\")\n  on r.NoClicked { n = 1 }\n}\n",
        "on ControllerJoined() -> (c) {\n  let r = c.ShowTextInputBox(\"name?\")\n  on r.CancelClicked { n = 1 }\n}\n",
        "on ControllerJoined() -> (c) {\n  let r = c.ShowYesNoBox(\"yes?\")\n  on !r.NoClicked { n = 1 }\n}\n",
        "on ControllerJoined() -> (c) {\n  let r = c.ShowYesNoBox(\"yes?\")\n  on r.YesClicked | r.NoClicked { n = 1 }\n}\n",
        "on ControllerLeft() -> { controller: controller } {\n  n = 1\n}\n",
        // Captures bind by slot: an old capture of the player would now bind
        // the user id, and every name after it the slot after that.
        "on ControllerLeft() -> (controller) {\n  n = 1\n}\n",
        "on ControllerLeft() -> (c, userId) {\n  n = 1\n}\n",
        "on ControllerLeft(ctrl, uid) {\n  n = 1\n}\n",
        "on ControllerLeft() -> (who: controller, uid: string) {\n  n = 1\n}\n",
    ] {
        let src = format!("var n: int = 0\n{body}");
        assert!(compile(input(&src)).is_err(), "should not compile:\n{src}");
    }
    for body in [
        "on ControllerLeft() -> (userId) {\n  s = userId\n}\n",
        "on ControllerLeft() -> (uid, name) {\n  s = uid .. name\n}\n",
        "on ControllerLeft() -> { userName } {\n  s = userName\n}\n",
    ] {
        lowered(&format!("var s: string = \"\"\n{body}"));
    }
    for p in ["CancelClicked", "NoClicked", "FailedToOpen"] {
        assert!(
            !wirescript::ir::port_registry::WirePort::all_names().contains(&p),
            "{p}"
        );
    }
}

/// Item Spawn's `PickupMeshColors` input left the game.
#[test]
fn item_spawn_has_no_pickup_mesh_colors() {
    let spawn = wirescript::catalog::default_catalog()
        .find_by_class("Component_ItemSpawn")
        .expect("Item Spawn in the inventory");
    assert!(
        spawn
            .component
            .inputs
            .iter()
            .all(|p| p.name != "PickupMeshColors")
    );
    assert!(
        !wirescript::ir::port_registry::WirePort::all_names().contains(&"PickupMeshColors"),
        "no port of that name is left to wire"
    );
}

/// A `$ComponentType/<name>` outside the Component Type dropdown is WS110:
/// a logic gate, Input Splitter (spawnable, but not offered), or a typo.
#[test]
fn a_component_type_outside_the_dropdown_is_ws110() {
    let codes = |name: &str| common::diag_codes(&format!("let t = $Component/{name}\n"));
    assert!(codes("PointLight").is_empty());
    assert!(codes("PointLight/Brightness").is_empty());
    for bad in [
        // A logic gate, by its display name and by its class.
        "SetVariable",
        "BrickComponentType_Exec_Var_Set",
        // Spawnable, but not offered.
        "InputSplitter",
        "PointLite",
        // The class of an offered component: told the spelling to use.
        "Component_PointLight",
        // A property the light does not have.
        "PointLight/Volume",
    ] {
        assert_eq!(codes(bad), vec!["WS110".to_string()], "{bad}");
    }
}
