//! The callable-function gates' settings reach the save the way the game
//! writes them: a Function Call's eight `InputSources`, and a result pin's
//! `PortOrder` and `DefaultValue`. Built from IR by hand.

use brdb::schema::BrdbValue;
use wirescript::intern::intern;
use wirescript::ir::build::{AddNodeOpts, ModuleBuilder, port_ref};
use wirescript::ir::gate_class as gc;
use wirescript::ir::{Literal, Wire};

mod common;

fn brz_of(b: &ModuleBuilder) -> Vec<u8> {
    let lr = wirescript::layout(&b.module);
    let world = wirescript::build_world(
        &b.module,
        &lr,
        &wirescript::EmitOptions::default(),
        &std::sync::Arc::new(wirescript::template_cache::TemplateCache::new()),
    )
    .expect("emit");
    world.to_brz_vec().expect("encode")
}

#[test]
fn a_function_call_writes_eight_input_sources() {
    let mut b = ModuleBuilder::new("call");
    let src = b.add_gate(AddNodeOpts { gate_class: "BrickComponentType_Expr_MathAdd", ..Default::default() });
    let mut props = wirescript::collections::HashMap::default();
    props.insert(intern("InputSources"), Literal::Array(vec![Literal::Int(1), Literal::Int(0), Literal::Int(1)]));
    props.insert(intern("Input0"), Literal::Int(5));
    let call = b.add_gate(AddNodeOpts { gate_class: gc::FUNCTION_CALL, properties: props, ..Default::default() });
    b.module.wires.push(Wire { source: port_ref(src, "Output"), target: port_ref(call, "Input2") });

    let comps = common::world_components(&brz_of(&b));
    let call = comps.iter().find(|c| c.get_name() == "BCSComponentVertex_FunctionCall").expect("the call's data");
    let Some(BrdbValue::Array(sources)) = call.get("InputSources") else {
        panic!("no InputSources array: {call}");
    };
    let values: Vec<u64> = sources
        .iter()
        .map(|v| match v {
            BrdbValue::Enum(e) => e.value,
            other => panic!("not an enum: {other:?}"),
        })
        .collect();
    assert_eq!(values, vec![1, 0, 1, 0, 0, 0, 0, 0]);
    assert!(format!("{call}").contains("Input0: 5i64"), "{call}");
}

#[test]
fn a_result_pin_writes_its_order_and_default() {
    let mut b = ModuleBuilder::new("result");
    let mut props = wirescript::collections::HashMap::default();
    props.insert(intern("PortLabel"), Literal::String("r".into()));
    props.insert(intern("PortOrder"), Literal::Int(2));
    props.insert(intern("DefaultValue"), Literal::Int(7));
    b.add_gate(AddNodeOpts { gate_class: gc::FUNCTION_OUTPUT, properties: props, ..Default::default() });

    let comps = common::world_components(&brz_of(&b));
    let out = comps
        .iter()
        .find(|c| c.get_name() == "BrickComponentData_FunctionOutput")
        .expect("the result pin's data");
    let text = format!("{out}");
    assert!(text.contains("PortOrder: 2i32") && text.contains("DefaultValue: 7i64"), "{text}");
}
