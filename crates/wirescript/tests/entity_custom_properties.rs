//! Gates from the custom-properties game build: an entity's key/value store,
//! Get Value Type, and the advanced inventory gates' wireable
//! `bOverrideColors`.

use wirescript::compile::{CompileInput, FoldMode, compile};
use wirescript::ir::gate_class as gc;
use wirescript::ir::{Literal, Module, Node};

mod common;

fn lowered(src: &str) -> Module {
    let (_, tc, lowered) = common::run_stages(src, "props.ws", FoldMode::Auto);
    assert!(tc.diagnostics.is_empty(), "{:?}", tc.diagnostics);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    lowered.module
}

fn node<'m>(m: &'m Module, class: &str) -> &'m Node {
    let found: Vec<&Node> = m.nodes.values().filter(|n| n.gate_class == class).collect();
    assert_eq!(found.len(), 1, "one {class}");
    found[0]
}

/// The `k`th declared `in` port.
fn input(m: &Module, k: usize) -> &Node {
    &m.nodes[&m.inputs[k]]
}

/// Whether `to.port` is wired from `from`, at any port.
fn fed(m: &Module, from: &Node, to: &Node, port: &str) -> bool {
    m.wires.iter().any(|w| w.source.node_id == from.id && w.target.node_id == to.id && w.target.port.as_str() == port)
}

/// Whether anything reads `from.port`.
fn read(m: &Module, from: &Node, port: &str) -> bool {
    m.wires.iter().any(|w| w.source.node_id == from.id && w.source.port.as_str() == port)
}

fn prop(n: &Node, name: &str) -> Option<Literal> {
    n.properties.get(&wirescript::intern::intern(name)).cloned()
}

const CUSTOM: &str = "
in go: exec
in e: entity
in other: entity
in key: string
in amount: int
var n: int = 0
var gone: bool = false
on go {
  e.SetCustomProperty(key, amount)
  let g = e.GetCustomProperty(key)
  if g.Found { n = g.Value }
  gone = e.RemoveCustomProperty(key)
  other.CopyCustomProperties(e)
  n = GetValueType(amount)
}
";

/// Each call is its gate, with the entity, key and value on their pins and
/// the results read from theirs.
#[test]
fn custom_property_calls_lower_to_their_gates() {
    let m = lowered(CUSTOM);
    let (e, other, key, amount) = (input(&m, 1), input(&m, 2), input(&m, 3), input(&m, 4));

    let set = node(&m, gc::ENTITY_SET_CUSTOM_PROPERTY);
    assert!(fed(&m, e, set, "Entity") && fed(&m, key, set, "Key") && fed(&m, amount, set, "Value"));

    let get = node(&m, gc::ENTITY_GET_CUSTOM_PROPERTY);
    assert!(fed(&m, e, get, "Entity") && fed(&m, key, get, "Key"));
    assert!(read(&m, get, "bFound") && read(&m, get, "Value"), "`.Found` and `.Value` read the gate");

    let remove = node(&m, gc::ENTITY_REMOVE_CUSTOM_PROPERTY);
    assert!(fed(&m, e, remove, "Entity") && fed(&m, key, remove, "Key"));
    assert!(read(&m, remove, "bFound"));

    let copy = node(&m, gc::ENTITY_COPY_CUSTOM_PROPERTIES);
    assert!(fed(&m, other, copy, "Entity") && fed(&m, e, copy, "Source"), "the receiver gets the source's properties");

    let kind = node(&m, gc::GET_VALUE_TYPE);
    assert!(fed(&m, amount, kind, "Input") && read(&m, kind, "Type"));
}

/// A constant key and value are baked into the gate, and the save loads.
#[test]
fn a_constant_key_and_value_are_baked() {
    let src = "in go: exec\nin e: entity\nvar n: int = 0\non go {\n  e.SetCustomProperty(\"hp\", 5)\n  n = e.GetCustomProperty(\"hp\")\n}\n";
    let m = lowered(src);
    let set = node(&m, gc::ENTITY_SET_CUSTOM_PROPERTY);
    assert_eq!(prop(set, "Key"), Some(Literal::String("hp".into())));
    assert_eq!(prop(set, "Value"), Some(Literal::Int(5)));
    assert_eq!(prop(node(&m, gc::ENTITY_GET_CUSTOM_PROPERTY), "Key"), Some(Literal::String("hp".into())));

    let brz = compile(CompileInput { source: src, file: "props.ws", module_name: None, fold_mode: FoldMode::Auto })
        .unwrap_or_else(|e| panic!("compile: {e:?}"))
        .brz;
    let comps = common::world_components(&brz);
    let shown: Vec<String> = comps
        .iter()
        .map(|c| format!("{c}").split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|c| c.contains("CustomProperty"))
        .collect();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown.iter().all(|c| c.contains("Key: \"hp\"")), "{shown:?}");
}

/// `bOverrideColors` is a wire input now, so a runtime bool wires into it,
/// while `meshColors` stays constant-only data on the same call.
#[test]
fn override_colors_takes_a_runtime_bool() {
    let src = "
in go: exec
in c: character
in weapon: entity
in tint: bool
on go {
  c.AddInventoryItemAdv(weapon, overrideColors = tint, meshColors = [ColorSRGB(255, 0, 0, 255)])
  c.SetInventoryItemAdv(weapon, 2, overrideColors = !tint)
}
";
    let m = lowered(src);
    let tint = input(&m, 3);
    let add = node(&m, gc::CHARACTER_ADD_INVENTORY_ITEM_ADV);
    assert!(fed(&m, tint, add, "bOverrideColors"));
    assert!(matches!(prop(add, "MeshColors"), Some(Literal::Array(c)) if c.len() == 1));
    let set = node(&m, gc::CHARACTER_SET_INVENTORY_ITEM_ADV);
    assert!(m.wires.iter().any(|w| w.target.node_id == set.id && w.target.port.as_str() == "bOverrideColors"));
}
