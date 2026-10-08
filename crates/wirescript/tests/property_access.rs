//! `Environment.<env>.<property>`: reads and writes of an environment setting
//! through the Get/Set Environment Property gates, typed from the property map.

use wirescript::compile::{CompileInput, FoldMode, compile};
use wirescript::ir::{Literal, Module, Node, Type};

mod common;

fn lowered(src: &str) -> Module {
    let (_, tc, lowered) = common::run_stages(src, "property_access.ws", FoldMode::Auto);
    assert!(tc.diagnostics.is_empty(), "{:?}", tc.diagnostics);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    lowered.module
}

fn nodes<'m>(m: &'m Module, class: &str) -> Vec<&'m Node> {
    let mut v: Vec<&Node> = m.nodes.values().filter(|n| n.gate_class == class).collect();
    v.sort_by_key(|n| n.id.0);
    v
}

fn prop(n: &Node, name: &str) -> Option<Literal> {
    n.properties.get(&wirescript::intern::intern(name)).cloned()
}

fn handler(body: &str) -> String {
    format!("in go: exec\nvar f: float = 0.0\nvar c: color = Color(0.0, 0.0, 0.0)\nvar b: bool = false\nvar i: int = 0\non go {{\n{body}\n}}\n")
}

const GET: &str = "BrickComponentType_Exec_GetEnvironmentProperty";
const SET: &str = "BrickComponentType_Exec_SetEnvironmentProperty";

/// A read is typed by its property (float, color, bool; an enum reads as an
/// int), and its gate carries the environment and property as settings.
#[test]
fn environment_reads_are_typed_by_the_property() {
    let m = lowered(&handler(
        "  f = Environment.Sky.TimeOfDay\n  c = Environment.Sky.SkyColor\n  b = Environment.Ground.IsVisible\n  i = Environment.Ambience.ReverbEffect",
    ));
    let gets = nodes(&m, GET);
    assert_eq!(gets.len(), 4);
    let typed: Vec<(Option<Literal>, Option<Literal>, Type)> = gets
        .iter()
        .map(|n| {
            let value = n.ports.outputs.iter().find(|p| wirescript::intern::resolve(p.name) == "Value").unwrap();
            (prop(n, "Environment"), prop(n, "PropertyName"), value.ty.clone())
        })
        .collect();
    let s = |v: &str| Some(Literal::String(v.into()));
    assert_eq!(
        typed,
        vec![
            (s("Sky"), s("TimeOfDay"), Type::Float),
            (s("Sky"), s("SkyColor"), Type::Color),
            // The picker's "Ground" is stored as `GroundPlate`.
            (s("GroundPlate"), s("IsVisible"), Type::Bool),
            // The enum itself; a Convert gate below makes it an int.
            (s("Ambience"), s("ReverbEffect"), Type::Any),
        ]
    );
    // An enum wire reads as 0 in arithmetic, so the read is converted to its
    // number, which the game gives for `as int`.
    let reverb = gets[3];
    let convert = m
        .wires
        .iter()
        .find(|w| w.source.node_id == reverb.id && w.source.port.as_str() == "Value")
        .map(|w| &m.nodes[&w.target.node_id])
        .expect("the reverb read is consumed");
    assert_eq!(convert.gate_class, "BrickComponentType_Expr_Convert");
    let out = convert.ports.outputs.iter().find(|p| wirescript::intern::resolve(p.name) == "Output").map(|p| p.ty.clone());
    assert_eq!(out, Some(Type::Int));
}

/// A write wires its value into the Set gate; a constant member of an enum
/// property's enum is written as its number.
#[test]
fn environment_writes_wire_their_value() {
    let m = lowered(&handler(
        "  Environment.Sky.TimeOfDay = f + 1.0\n  Environment.Sky.SkyColor = Color(1.0, 0.5, 0.0)\n  Environment.Ambience.ReverbEffect = ReverbEffect.LongHallway",
    ));
    let sets = nodes(&m, SET);
    assert_eq!(sets.len(), 3);
    let fed = |n: &Node| m.wires.iter().any(|w| w.target.node_id == n.id && w.target.port.as_str() == "Value");
    assert!(fed(sets[0]), "the computed time is wired in");
    let reverb = sets[2];
    let into_value = m.wires.iter().find(|w| w.target.node_id == reverb.id && w.target.port.as_str() == "Value");
    let baked = prop(reverb, "Value");
    let from_literal = into_value.and_then(|w| m.nodes.get(&w.source.node_id)).and_then(|s| prop(s, "Value"));
    assert!(
        baked == Some(Literal::Int(2)) || from_literal == Some(Literal::Int(2)),
        "LongHallway is 2: baked {baked:?}, wired {from_literal:?}"
    );
}

/// The settings reach the save as the gate's own data.
#[test]
fn environment_settings_are_baked_into_the_gate() {
    let brz = compile(CompileInput {
        source: &handler("  Environment.Water.WaterHeight = 120.0"),
        file: "property_access.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile")
    .brz;
    let comps = common::world_components(&brz);
    let data = comps.iter().find(|c| c.get("Environment").is_some()).expect("the Set gate's data");
    assert!(format!("{:?}", data.get("Environment")).contains("Water"));
    assert!(format!("{:?}", data.get("PropertyName")).contains("WaterHeight"));
}

#[test]
fn environment_mistakes_are_reported() {
    let codes = |body: &str| common::diag_codes(&handler(body));
    // No such environment, or property.
    assert_eq!(codes("  f = Environment.Moon.TimeOfDay"), vec!["WS110"]);
    assert_eq!(codes("  f = Environment.Sky.Brightness"), vec!["WS110"]);
    // A path that stops at the environment.
    assert!(codes("  f = Environment.Sky").contains(&"WS110".to_string()));
    // A value of the wrong type.
    assert_eq!(codes("  Environment.Sky.TimeOfDay = Color(1.0, 0.0, 0.0)"), vec!["WS003"]);
    // A property whose type a program cannot carry.
    assert_eq!(codes("  f = Environment.Universe.NebulaTexture"), vec!["WS003"]);
    // Outside an exec context.
    assert_eq!(common::diag_codes("out t = Environment.Sky.TimeOfDay\n"), vec!["WS007"]);
    // A program's own `Environment` wins.
    assert!(common::diag_codes("type E = { Sky: { T: float } }\nlet Environment: E = { Sky: { T: 1.0 } }\nout t = Environment.Sky.T\n").is_empty());
}

fn on_hit(body: &str) -> String {
    format!(
        "in go: exec\nvar f: float = 0.0\non go {{\n  let r = SweepSimple(1000.0, detectBricks = true)\n  on r.Hit {{\n{body}\n  }}\n}}\n"
    )
}

/// `brick.GetComponent($Component/PointLight)` gives a `Component<PointLight>`
/// whose properties read and write with their own types, each through a
/// property gate fed the reference.
#[test]
fn component_properties_read_and_write_typed() {
    let m = lowered(&on_hit(
        "    let light = r.HitBrick.AddComponent($Component/PointLight)\n    f = light.Brightness\n    light.Brightness = f * 2.0\n    light.Color = Color(1.0, 0.5, 0.0)\n    r.HitBrick.RemoveComponent($Component/PointLight)",
    ));
    let add = nodes(&m, "BrickComponentType_Exec_AddComponent");
    assert_eq!(add.len(), 1);
    let fed_by_add = |n: &Node| {
        m.wires.iter().any(|w| w.source.node_id == add[0].id && w.target.node_id == n.id && w.target.port.as_str() == "Component")
    };
    let get = nodes(&m, "BrickComponentType_Exec_GetComponentProperty");
    let set = nodes(&m, "BrickComponentType_Exec_SetComponentProperty");
    let remove = nodes(&m, "BrickComponentType_Exec_RemoveComponent");
    assert_eq!((get.len(), set.len(), remove.len()), (1, 2, 1));
    assert!(get.iter().chain(&set).all(|n| fed_by_add(n)), "every property gate reads the added component");
    // Remove Component takes the brick, with the type baked in.
    let brick_of = |n: &Node| {
        m.wires.iter().find(|w| w.target.node_id == n.id && w.target.port.as_str() == "Brick").map(|w| w.source)
    };
    assert!(brick_of(remove[0]).is_some() && brick_of(remove[0]) == brick_of(add[0]), "it reads the hit brick");
    assert!(!m.wires.iter().any(|w| w.target.node_id == remove[0].id && w.target.port.as_str() == "Component"));
    let value_ty = get[0].ports.outputs.iter().find(|p| wirescript::intern::resolve(p.name) == "Value").map(|p| p.ty.clone());
    assert_eq!(value_ty, Some(Type::Float));
    let light = Some(Literal::Asset { asset_type: "BrickComponentTypeBase".into(), asset_name: "Component_PointLight".into() });
    assert_eq!(prop(get[0], "ComponentType"), light);
    assert_eq!(prop(get[0], "PropertyName"), Some(Literal::String("Brightness".into())));
    assert_eq!(prop(add[0], "ComponentType"), light);
    assert_eq!(prop(remove[0], "ComponentType"), light);
}

/// The component type and property reach the save the way the game stores
/// them: the asset `BrickComponentTypeBase/Component_PointLight` and a string.
#[test]
fn component_settings_are_baked_as_the_game_stores_them() {
    let brz = compile(CompileInput {
        source: &on_hit("    let light = r.HitBrick.GetComponent($Component/PointLight)\n    light.Radius = 400.0"),
        file: "property_access.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile")
    .brz;
    let comps = common::world_components(&brz);
    let set = comps.iter().find(|c| c.get("PropertyName").is_some()).expect("the Set gate's data");
    assert!(format!("{:?}", set.get("PropertyName")).contains("Radius"));
    assert!(set.get("ComponentType").is_some());
}

/// A reference passes to a `mod` taking `Component<PointLight>`.
#[test]
fn a_component_reference_passes_to_an_annotated_mod() {
    lowered(&format!(
        "mod brighten(l: Component<PointLight>) {{ l.Brightness = 5.0 }}\n{}",
        on_hit("    brighten(r.HitBrick.GetComponent($Component/PointLight))")
    ));
}

#[test]
fn component_mistakes_are_reported() {
    let codes = |body: &str| common::diag_codes(&on_hit(body));
    let light = "    let light = r.HitBrick.GetComponent($Component/PointLight)\n";
    // No such property, a value of the wrong type, an unsupported type.
    assert_eq!(codes(&format!("{light}    light.Volume = 1.0")), vec!["WS110"]);
    assert_eq!(codes(&format!("{light}    light.Brightness = \"x\"")), vec!["WS003"]);
    assert_eq!(
        codes("    let a = r.HitBrick.GetComponent($Component/Audio)\n    f = a.AudioDescriptor"),
        vec!["WS003"]
    );
    // The type must be a `$Component/...`.
    assert_eq!(codes("    let c = r.HitBrick.GetComponent(5)"), vec!["WS110"]);
    // An unknown type in an annotation, and a reference as a map key.
    assert_eq!(common::diag_codes("var c: Component<Lamp>\n"), vec!["WS110"]);
    assert_eq!(common::diag_codes("var m: Map<Component<PointLight>, int>\n"), vec!["WS039"]);
}

/// A component reference is stored like a var ref: a `var` holds a
/// `BRSavedComponentRef`, and an array or map holds `component_handle`s. A
/// stored reference reads back and drives the property gates.
#[test]
fn a_component_reference_is_stored_as_a_component_handle() {
    let src = on_hit(
        "    light = r.HitBrick.AddComponent($Component/PointLight)\n    lights.push(light)\n    byName.set(\"a\", light)\n    lights[0].Brightness = 4.0\n    f = light.Brightness",
    )
    .replace(
        "on go {",
        "var light: Component<PointLight>\nvar lights: Component<PointLight>[]\nvar byName: Map<string, Component<PointLight>>\non go {",
    );
    let m = lowered(&src);
    assert_eq!(nodes(&m, "BrickComponentType_Exec_SetComponentProperty").len(), 1);
    assert_eq!(nodes(&m, "BrickComponentType_Exec_GetComponentProperty").len(), 1);

    let brz = compile(CompileInput { source: &src, file: "property_access.ws", module_name: None, fold_mode: FoldMode::Auto })
        .expect("should compile")
        .brz;
    let comps = common::world_components(&brz);
    let holds = |name: &str, form: &str| {
        comps.iter().filter(|c| c.get_name() == name).any(|c| format!("{c}").contains(form))
    };
    assert!(holds("BrickComponentData_Var", "BRSavedComponentRef"));
    assert!(holds("BrickComponentData_ArrayVar", "component_handle["));
    assert!(holds("BrickComponentData_MapVar", "{str: component_handle}"));
}

/// Get/Add Component take their type as a `Component`, which is what a
/// `$Component/...` is, and a bare `Component` annotation takes any component.
#[test]
fn component_types_are_typed_not_any() {
    let spec = wirescript::catalog::calls::find_call("GetComponent").unwrap();
    let param = spec.params.iter().find(|p| p.name == "componentType").unwrap();
    assert_eq!(param.ty, Type::Component(String::new()));
    assert_eq!(param.ty.to_string(), "Component");
    assert_eq!(Type::Component("Component_PointLight".into()).to_string(), "Component<PointLight>");
    lowered(&format!(
        "var held: Component[]\nmod keep(c: Component) {{ held.push(c) }}\n{}",
        on_hit("    keep(r.HitBrick.GetComponent($Component/SpotLight))")
    ));
}

/// Remove Component saves as the shared brick-component struct, with the
/// component type set and nothing wired into a `Component` pin.
#[test]
fn remove_component_saves_its_type() {
    let brz = compile(CompileInput {
        source: &on_hit("    r.HitBrick.RemoveComponent($Component/PointLight)"),
        file: "property_access.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile")
    .brz;
    let comps = common::world_components(&brz);
    let shown: Vec<String> = comps
        .iter()
        .map(|c| format!("{} {c}", c.get_name()).split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|c| c.contains("BrickComponentBase"))
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains("Component_PointLight"), "{shown:?}");
}

/// The game removes a component by its brick and type, and no gate gives a
/// reference's brick, so `c.Remove()` names the brick form instead.
#[test]
fn a_component_reference_has_no_remove() {
    let (_, tc, _) = common::run_stages(
        &on_hit("    let light = r.HitBrick.AddComponent($Component/PointLight)\n    light.Remove()"),
        "property_access.ws",
        FoldMode::Auto,
    );
    let d: Vec<_> = tc.diagnostics.iter().map(|d| (d.code.as_str(), d.message.as_str())).collect();
    assert!(
        matches!(d.as_slice(), [("WS110", m)] if m.contains("brick.RemoveComponent($Component/PointLight)")),
        "{d:?}"
    );
}

/// A compound assignment reads the property, combines, and writes it back:
/// the read runs before the write on the exec chain.
#[test]
fn compound_assignment_reads_then_writes_a_property() {
    let m = lowered(&format!(
        "{}",
        on_hit("    Environment.Sky.TimeOfDay += 1.0\n    let light = r.HitBrick.GetComponent($Component/PointLight)\n    light.Brightness *= 2.0")
    ));
    for (get, set, math) in [
        (GET, SET, "BrickComponentType_Expr_MathAdd"),
        (
            "BrickComponentType_Exec_GetComponentProperty",
            "BrickComponentType_Exec_SetComponentProperty",
            "BrickComponentType_Expr_MathMultiply",
        ),
    ] {
        let (g, s, op) = (nodes(&m, get)[0], nodes(&m, set)[0], nodes(&m, math)[0]);
        let wired = |from: &Node, out: &str, to: &Node, inp: &str| {
            m.wires.iter().any(|w| {
                w.source.node_id == from.id && w.source.port.as_str() == out && w.target.node_id == to.id && w.target.port.as_str() == inp
            })
        };
        assert!(wired(g, "Value", op, "InputA"), "{get} feeds {math}");
        assert!(wired(op, "Output", s, "Value"), "{math} feeds {set}");
        assert!(wired(g, "ExecOut", s, "Exec"), "{get} runs right before {set}");
    }
}
