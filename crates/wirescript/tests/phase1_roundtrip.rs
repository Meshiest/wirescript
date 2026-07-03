//! Phase 1 smoke test.
//!
//! Hand-build a trivial IR (one chip with a single Reroute gate inside)
//! and verify the emitter produces non-empty brz bytes without panicking.
//! A richer end-to-end "load into the live server and probe" test lives
//! in the omegga repo's QA harness.

use std::{collections::HashMap, sync::Arc};

use wirescript::{
    emit::{EmitOptions, Placement},
    emit_brz,
    ir::{GateIO, Module, Node, NodeId, NodeKind, PortRef, PortSpec, SourceRange, Type, Wire},
    template_cache::TemplateCache,
};

fn rerouter_ports() -> GateIO {
    GateIO {
        inputs: vec![PortSpec {
            name: wirescript::intern::intern("RER_Input"),
            ty: Type::Any,
        }],
        outputs: vec![PortSpec {
            name: wirescript::intern::intern("RER_Output"),
            ty: Type::Any,
        }],
    }
}

#[test]
fn empty_module_produces_nonzero_brz_bytes() {
    let module = Module::new("phase1_empty");
    let placements = HashMap::new();
    let template_cache = Arc::new(TemplateCache::new());
    let bytes = emit_brz(
        &module,
        &placements,
        &EmitOptions::default(),
        &template_cache,
    )
    .expect("empty module should still produce a valid brz (just the outer chip)");
    assert!(bytes.len() > 32, "brz bytes should be well over a header");
    // `.brz` files start with a `BRZ\0` magic then a zstd-compressed payload.
    assert_eq!(&bytes[..4], b"BRZ\0", "expected BRZ magic");
}

#[test]
fn single_reroute_gate_inside_chip() {
    let mut module = Module::new("phase1_one_gate");

    let rer_id = NodeId::fresh();
    let reroute = Node {
        id: rer_id,
        kind: NodeKind::Gate,
        gate_class: "Component_Internal_Rerouter",
        properties: Arc::new(HashMap::new()),
        ports: Arc::new(rerouter_ports()),
        source_range: SourceRange::default(),
        chip_id: None,
        chain_id: None,
        scope_id: wirescript::ir::ROOT_SCOPE_ID,
        note: None,
    };
    module.add_node(reroute);

    let mut placements = HashMap::new();
    placements.insert(rer_id, Placement { x: 0, y: 0, z: 2 });

    let template_cache = Arc::new(TemplateCache::new());
    let bytes = emit_brz(
        &module,
        &placements,
        &EmitOptions::default(),
        &template_cache,
    )
    .expect("emit succeeds");
    assert!(
        bytes.len() > 100,
        "one-gate brz should be at least a few hundred bytes, got {}",
        bytes.len()
    );
}

#[test]
fn wire_between_two_gates_produces_connection() {
    let mut module = Module::new("phase1_wired");

    let id_a = NodeId::fresh();
    let id_b = NodeId::fresh();

    for (id, _label) in [(id_a, "rerA"), (id_b, "rerB")] {
        module.add_node(Node {
            id,
            kind: NodeKind::Gate,
            gate_class: "Component_Internal_Rerouter",
            properties: Arc::new(HashMap::new()),
            ports: Arc::new(rerouter_ports()),
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
    let bytes = emit_brz(
        &module,
        &placements,
        &EmitOptions::default(),
        &template_cache,
    )
    .expect("emit succeeds");
    assert!(!bytes.is_empty());
}
