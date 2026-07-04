//! Writes a sample `.brz` file to `/tmp/wirescript_phase1.brz` so it can be
//! loaded in-game via `BR.World.LoadAdditive` for manual validation.
//! Ignored by default — run with `cargo test --test write_sample -- --ignored`.

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use wirescript::{
    emit::{EmitOptions, Placement},
    emit_brdb, emit_brz,
    ir::{GateIO, Module, Node, NodeId, NodeKind, PortRef, PortSpec, SourceRange, Type, Wire},
    template_cache::TemplateCache,
};

#[test]
#[ignore]
fn phase1_sample() {
    let mut module = Module::new("phase1_sample");

    let ports = Arc::new(GateIO {
        inputs: vec![PortSpec {
            name: wirescript::intern::intern("RER_Input"),
            ty: Type::Any,
        }],
        outputs: vec![PortSpec {
            name: wirescript::intern::intern("RER_Output"),
            ty: Type::Any,
        }],
    });

    let id_a = NodeId::fresh();
    let id_b = NodeId::fresh();

    for (id, _label) in [(id_a, "rerA"), (id_b, "rerB")] {
        module.add_node(Node {
            id,
            kind: NodeKind::Gate,
            gate_class: "Component_Internal_Rerouter",
            properties: Arc::new(HashMap::new()),
            ports: ports.clone(),
            source_range: SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: wirescript::ir::ROOT_SCOPE_ID,
            note: None,
        });
    }
    module.add_wire(Wire {
        source: PortRef {
            node_id: id_a,
            port: wirescript::ir::port_registry::WirePort::RerOutput,
        },
        target: PortRef {
            node_id: id_b,
            port: wirescript::ir::port_registry::WirePort::RerInput,
        },
    });

    let mut placements = HashMap::new();
    placements.insert(id_a, Placement { x: 0, y: 0, z: 2 });
    placements.insert(id_b, Placement { x: 8, y: 0, z: 2 });

    let template_cache = Arc::new(TemplateCache::new());
    let bytes = emit_brz(&module, &placements, &EmitOptions::default(), &template_cache).expect("emit brz");
    fs::write("/tmp/wirescript_phase1.brz", &bytes).expect("write brz file");
    let brdb_path = "/tmp/wirescript_phase1.brdb";
    let _ = fs::remove_file(brdb_path);
    emit_brdb(&module, &placements, &EmitOptions::default(), &template_cache, brdb_path).expect("emit brdb");
    println!(
        "Wrote /tmp/wirescript_phase1.brz ({} bytes) + /tmp/wirescript_phase1.brdb.\nLoad in-game with:\n  BR.World.LoadAdditive wirescript_phase1",
        bytes.len(),
    );
}
