//! Build the AND program with the builder (no text, no front end), assemble
//! it, then read the save back with brdb and check what the game would load:
//! one AND gate, two inputs, one output, and the three wires between them.
//!
//! `cargo run -p wirescript-ir --features test-internals --example and_gate`

use brdb::schema::BrdbValue;
use wirescript_ir::emit::{EmitOptions, assemble};
use wirescript_ir::ir::{ProgramBuilder, Type};
use wirescript_ir::readback::Save;

fn main() {
    let mut b = ProgramBuilder::new("and");
    let root = b.root();
    let a = b.input(root, "a", Type::Bool);
    let c = b.input(root, "b", Type::Bool);
    let g = b.gate(root, "Expr_LogicalAND").expect("an inventory class");
    b.name(g, "g");
    let y = b.output(root, "y", Type::Bool);
    b.wire(a, "RER_Output", g, "bInputA").unwrap();
    b.wire(c, "RER_Output", g, "bInputB").unwrap();
    b.wire(g, "bOutput", y, "RER_Input").unwrap();
    let program = b.build();

    let errors = program.validate();
    assert!(errors.is_empty(), "the builder made an invalid program: {errors:?}");

    let world = assemble(&program, &EmitOptions::default()).expect("assemble").world;
    let save = Save::read(&world.to_brz_vec().expect("encode"));
    let type_names = save.component_type_names();
    let port_names = save.wire_port_names();
    let type_name = |i: usize| type_names.get(i).cloned().unwrap_or_default();

    let mut components: Vec<String> = Vec::new();
    for (_, _, soa, _) in save.component_chunks() {
        for t in &soa.component_type_counters {
            components.extend(std::iter::repeat_n(type_name(t.type_index as usize), t.num_instances as usize));
        }
    }
    let mut wires: Vec<String> = Vec::new();
    for (_, _, soa) in save.wires() {
        let ends = |field: &str| -> Vec<String> {
            let Some(BrdbValue::Array(items)) = soa.get(field) else { return Vec::new() };
            items
                .iter()
                .map(|v| {
                    let BrdbValue::Struct(s) = v else { panic!("a wire end is a struct") };
                    let num = |k: &str| match s.get(k) {
                        Some(BrdbValue::U16(n)) => *n as usize,
                        Some(BrdbValue::U32(n)) => *n as usize,
                        other => panic!("{k}: {other:?}"),
                    };
                    let port = port_names.get(num("PortIndex")).cloned().unwrap_or_default();
                    format!("{}.{port}", short(&type_name(num("ComponentTypeIndex"))))
                })
                .collect()
        };
        let sources = ends("LocalWireSources");
        let targets = ends("LocalWireTargets");
        assert_eq!(sources.len(), targets.len());
        wires.extend(sources.into_iter().zip(targets).map(|(s, t)| format!("{s} -> {t}")));
    }

    let count = |class: &str| components.iter().filter(|c| c.as_str() == class).count();
    assert_eq!(count("BrickComponentType_Expr_LogicalAND"), 1, "{components:?}");
    assert_eq!(count("BrickComponentType_Internal_MicrochipInput"), 2, "{components:?}");
    assert_eq!(count("BrickComponentType_Internal_MicrochipOutput"), 1, "{components:?}");
    wires.sort();
    assert_eq!(
        wires,
        [
            "Expr_LogicalAND.bOutput -> Internal_MicrochipOutput.RER_Input",
            "Internal_MicrochipInput.RER_Output -> Expr_LogicalAND.bInputA",
            "Internal_MicrochipInput.RER_Output -> Expr_LogicalAND.bInputB",
        ],
        "{wires:#?}"
    );
    println!("{} components, wires:", components.len());
    for w in &wires {
        println!("  {w}");
    }
    println!("ok");
}

fn short(class: &str) -> &str {
    class.strip_prefix("BrickComponentType_").unwrap_or(class)
}
