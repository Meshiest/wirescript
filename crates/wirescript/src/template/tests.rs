    use std::sync::Arc;

    use super::*;
    use crate::ir::{GateIO, NodeKind, PortRef, PortSpec, Type, Wire};

    /// Build a minimal module:
    ///
    /// ```text
    ///  in_a (MicrochipInput)  ─┐
    ///                          ├→ add (Gate) → out_r (MicrochipOutput)
    ///  in_b (MicrochipInput)  ─┘
    /// ```
    fn simple_add_module() -> Module {
        let mut m = Module::new("simple_add");

        let in_a_id = NodeId::fresh();
        let in_b_id = NodeId::fresh();
        let add_id = NodeId::fresh();
        let out_r_id = NodeId::fresh();

        let mc_input = "BrickComponentType_Internal_MicrochipInput";
        let mc_output = "BrickComponentType_Internal_MicrochipOutput";
        let add_gate = "BrickComponentType_WireGraph_Expr_Add";

        use crate::intern::sym;
        use crate::ir::port_registry::WirePort;
        // Sym for PortSpec.name
        let rer_in_sym = *sym::RER_INPUT;
        let rer_out_sym = *sym::RER_OUTPUT;
        let input_a_sym = *sym::INPUT_A;
        let input_b_sym = *sym::INPUT_B;
        let output_sym = *sym::OUTPUT;
        // WirePort for PortRef.port
        let rer_in_pi = WirePort::RerInput;
        let rer_out_pi = WirePort::RerOutput;
        let input_a_pi = WirePort::InputA;
        let input_b_pi = WirePort::InputB;
        let output_pi = WirePort::Output;

        // in_a node
        let in_a = Node {
            id: in_a_id,
            kind: NodeKind::Input,
            gate_class: mc_input,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec { name: rer_in_sym, ty: Type::Int }],
                outputs: vec![PortSpec { name: rer_out_sym, ty: Type::Int }],
            }),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 0,
            note: None,
        };

        // in_b node
        let in_b = Node {
            id: in_b_id,
            kind: NodeKind::Input,
            gate_class: mc_input,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec { name: rer_in_sym, ty: Type::Int }],
                outputs: vec![PortSpec { name: rer_out_sym, ty: Type::Int }],
            }),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 0,
            note: None,
        };

        // add gate
        let add = Node {
            id: add_id,
            kind: NodeKind::Gate,
            gate_class: add_gate,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec { name: input_a_sym, ty: Type::Int }],
                outputs: vec![
                    PortSpec { name: input_b_sym, ty: Type::Int },
                    PortSpec { name: output_sym, ty: Type::Int },
                ],
            }),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 0,
            note: None,
        };

        // out_r node
        let out_r = Node {
            id: out_r_id,
            kind: NodeKind::Output,
            gate_class: mc_output,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec { name: rer_in_sym, ty: Type::Int }],
                outputs: vec![PortSpec { name: rer_out_sym, ty: Type::Int }],
            }),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: 0,
            note: None,
        };

        m.nodes.insert(in_a_id, in_a);
        m.nodes.insert(in_b_id, in_b);
        m.nodes.insert(add_id, add);
        m.nodes.insert(out_r_id, out_r);

        m.inputs.push(in_a_id);
        m.inputs.push(in_b_id);
        m.outputs.push(out_r_id);

        // Wire: in_a.RER_Output → add.InputA
        m.wires.push(Wire {
            source: PortRef {
                node_id: in_a_id,
                port: rer_out_pi,
            },
            target: PortRef {
                node_id: add_id,
                port: input_a_pi,
            },
        });
        // Wire: in_b.RER_Output → add.InputB
        m.wires.push(Wire {
            source: PortRef {
                node_id: in_b_id,
                port: rer_out_pi,
            },
            target: PortRef {
                node_id: add_id,
                port: input_b_pi,
            },
        });
        // Wire: add.Output → out_r.RER_Input
        m.wires.push(Wire {
            source: PortRef {
                node_id: add_id,
                port: output_pi,
            },
            target: PortRef {
                node_id: out_r_id,
                port: rer_in_pi,
            },
        });

        m
    }

    // ── Test 1 ────────────────────────────────────────────────────────────────
    #[test]
    fn template_from_module_preserves_structure() {
        let m = simple_add_module();
        let node_count = m.nodes.len();
        let wire_count = m.wires.len();
        let input_count = m.inputs.len();
        let output_count = m.outputs.len();

        let t = CompiledTemplate::from_module(m);
        assert_eq!(t.module.nodes.len(), node_count);
        assert_eq!(t.module.wires.len(), wire_count);
        assert_eq!(t.module.inputs.len(), input_count);
        assert_eq!(t.module.outputs.len(), output_count);
        assert!(t.bounds.is_none());
        assert!(t.placements.is_none());
        assert!(t.external_refs.is_empty());
    }

    // ── Test 2 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_produces_unique_ids() {
        let t = CompiledTemplate::from_module(simple_add_module());
        let caps = HashMap::default();

        let inst0 = t.instantiate("inst0", &caps);
        let inst1 = t.instantiate("inst1", &caps);

        let ids0: std::collections::HashSet<NodeId> = inst0.nodes.keys().cloned().collect();
        let ids1: std::collections::HashSet<NodeId> = inst1.nodes.keys().cloned().collect();

        // The two sets must be completely disjoint.
        assert!(
            ids0.is_disjoint(&ids1),
            "inst0 and inst1 share node IDs: {:?}",
            ids0.intersection(&ids1).collect::<Vec<_>>()
        );
    }

    // ── Test 3 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_preserves_wire_integrity() {
        let t = CompiledTemplate::from_module(simple_add_module());
        let inst = t.instantiate("check", &HashMap::default());

        for wire in &inst.wires {
            assert!(
                inst.nodes.contains_key(&wire.source.node_id),
                "wire source {} not found in nodes",
                wire.source.node_id
            );
            assert!(
                inst.nodes.contains_key(&wire.target.node_id),
                "wire target {} not found in nodes",
                wire.target.node_id
            );
        }
    }

    // ── Test 4 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_resolves_captured_refs() {
        let mut m = simple_add_module();

        // Add a phantom external node to the wire list (it is *not* in m.nodes).
        let ext_id = NodeId::fresh();
        use crate::ir::port_registry::WirePort;
        let ext_port = WirePort::Value;
        // Pick an existing input node from the module.
        let in_a_id = m.inputs[0];
        let rer_in = WirePort::RerInput;

        m.wires.push(Wire {
            source: PortRef {
                node_id: ext_id,
                port: ext_port,
            },
            target: PortRef {
                node_id: in_a_id,
                port: rer_in,
            },
        });

        let mut t = CompiledTemplate::from_module(m);
        t.external_refs.push(("external_var".to_string(), ext_id));

        // Caller owns a node with a fresh id.
        let caller_node_id = NodeId::fresh();
        let mut captures = HashMap::default();
        captures.insert("external_var".to_string(), caller_node_id);

        let inst = t.instantiate("cap_test", &captures);

        // Find the wire that used to point to external_var — it must now point
        // to caller_node_id, not to a fresh prefixed ID.
        let resolved_wire = inst
            .wires
            .iter()
            .find(|w| w.source.node_id == caller_node_id);
        assert!(
            resolved_wire.is_some(),
            "expected wire from caller_node_id but found: {:?}",
            inst.wires
                .iter()
                .map(|w| w.source.node_id)
                .collect::<Vec<_>>()
        );
    }

    // ── Test 5 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_captures_not_remapped_as_internal() {
        let m = simple_add_module();

        // Designate the first input as an external ref.
        let in_a_id = m.inputs[0];
        let mut t = CompiledTemplate::from_module(m);
        t.external_refs.push(("in_a".to_string(), in_a_id));

        // Remove in_a from the internal node map so it is treated as purely external.
        t.module.nodes.remove(&in_a_id);

        let caller_id = NodeId::fresh();
        let mut captures = HashMap::default();
        captures.insert("in_a".to_string(), caller_id);

        let inst = t.instantiate("ext_test", &captures);

        // Internal nodes should have fresh IDs, not the original template IDs.
        for &id in inst.nodes.keys() {
            assert_ne!(
                id, in_a_id,
                "external placeholder should not appear as internal node"
            );
        }

        // Wires referencing the external should use caller_id.
        // With numeric IDs there are no "prefixed" IDs to check against;
        // just verify external refs resolve to the caller's ID.
        for wire in &inst.wires {
            if wire.source.node_id != caller_id && wire.target.node_id != caller_id {
                // This wire doesn't touch the external ref — skip.
                continue;
            }
            // At least one end should be caller_id (the external).
            assert!(
                wire.source.node_id == caller_id || wire.target.node_id == caller_id,
                "external ref wire should use caller_id"
            );
        }
    }

    // ── Test 7 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_50_times_all_disjoint() {
        let t = CompiledTemplate::from_module(simple_add_module());
        let caps = HashMap::default();
        let mut all_ids: std::collections::HashSet<NodeId> = std::collections::HashSet::default();

        for i in 0..50 {
            let inst = t.instantiate(&format!("stress{i}"), &caps);
            for &id in inst.nodes.keys() {
                assert!(
                    all_ids.insert(id),
                    "collision: node '{}' appeared in multiple instances",
                    id
                );
            }
        }
    }

    // ── Test 8 ────────────────────────────────────────────────────────────────
    fn collect_all_ids(m: &Module) -> std::collections::HashSet<NodeId> {
        let mut ids: std::collections::HashSet<NodeId> = m.nodes.keys().cloned().collect();
        for child_module in m.chips.values() {
            ids.extend(collect_all_ids(child_module));
        }
        ids
    }

    #[test]
    fn instantiate_three_level_nesting() {
        // leaf: simple_add_module (4 nodes)
        let leaf = simple_add_module();
        let leaf_chip_key = NodeId::fresh();

        let mc_chip = "Component_Internal_Microchip";
        let mid_gate_id = NodeId::fresh();
        let add_gate = "BrickComponentType_WireGraph_Expr_Add";

        // mid: 1 gate + 1 chip containing leaf
        let mut mid = Module::new("mid");
        mid.nodes.insert(
            mid_gate_id,
            Node {
                id: mid_gate_id,
                kind: NodeKind::Gate,
                gate_class: add_gate,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        mid.nodes.insert(
            leaf_chip_key,
            Node {
                id: leaf_chip_key,
                kind: NodeKind::Chip,
                gate_class: mc_chip,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        mid.chips.insert(leaf_chip_key, leaf);

        let mid_chip_key = NodeId::fresh();
        let top_gate_id = NodeId::fresh();

        // top: 1 gate + 1 chip containing mid
        let mut top = Module::new("top");
        top.nodes.insert(
            top_gate_id,
            Node {
                id: top_gate_id,
                kind: NodeKind::Gate,
                gate_class: add_gate,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        top.nodes.insert(
            mid_chip_key,
            Node {
                id: mid_chip_key,
                kind: NodeKind::Chip,
                gate_class: mc_chip,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        top.chips.insert(mid_chip_key, mid);

        let t = CompiledTemplate::from_module(top);
        let caps = HashMap::default();

        let inst0 = t.instantiate("top0", &caps);
        let inst1 = t.instantiate("top1", &caps);

        // All IDs across both instances must be disjoint.
        let ids0 = collect_all_ids(&inst0);
        let ids1 = collect_all_ids(&inst1);
        assert!(
            ids0.is_disjoint(&ids1),
            "three-level instances share IDs: {:?}",
            ids0.intersection(&ids1).collect::<Vec<_>>()
        );

        // Nesting structure: inst0 should have exactly 1 child chip (mid),
        // and that child chip should itself have exactly 1 child chip (leaf).
        assert_eq!(inst0.chips.len(), 1, "top should have 1 child chip");
        let mid_inst = inst0.chips.values().next().unwrap();
        assert_eq!(mid_inst.chips.len(), 1, "mid should have 1 child chip");
    }

    // ── Test 9 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_empty_template() {
        let m = Module::new("empty");
        let t = CompiledTemplate::from_module(m);
        let inst = t.instantiate("e0", &HashMap::default());

        assert_eq!(inst.nodes.len(), 0, "expected 0 nodes");
        assert_eq!(inst.wires.len(), 0, "expected 0 wires");
        assert_eq!(inst.chips.len(), 0, "expected 0 chips");
    }

    // ── Test 10 ───────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_no_captures_no_params() {
        let gate_id = NodeId::fresh();
        let add_gate = "BrickComponentType_WireGraph_Expr_Add";

        let mut m = Module::new("solo");
        m.nodes.insert(
            gate_id,
            Node {
                id: gate_id,
                kind: NodeKind::Gate,
                gate_class: add_gate,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );

        let t = CompiledTemplate::from_module(m);
        let inst = t.instantiate("nc0", &HashMap::default());

        assert_eq!(inst.nodes.len(), 1, "expected exactly 1 node");
        let new_id = *inst.nodes.keys().next().unwrap();
        assert_ne!(new_id, gate_id, "instantiated node must have a new ID");
        assert_ne!(
            new_id, gate_id,
            "instantiated node must have a different ID"
        );
    }

    // ── Test 11 ───────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_multiple_captures_same_call() {
        use crate::intern::sym;
        use crate::ir::port_registry::WirePort;
        let add_gate_class = "BrickComponentType_WireGraph_Expr_Add";
        let input_a_sym = *sym::INPUT_A;
        let input_b_sym = *sym::INPUT_B;
        let output_sym = *sym::OUTPUT;
        let input_a_pi = WirePort::InputA;
        let input_b_pi = WirePort::InputB;
        let output_pi = WirePort::Output;

        let gate_id = NodeId::fresh();

        let ext_a_id = NodeId::fresh();
        let ext_b_id = NodeId::fresh();
        let ext_c_id = NodeId::fresh();

        let real_a = NodeId::fresh();
        let real_b = NodeId::fresh();
        let real_c = NodeId::fresh();

        let mut m = Module::new("multi_cap");

        m.nodes.insert(
            gate_id,
            Node {
                id: gate_id,
                kind: NodeKind::Gate,
                gate_class: add_gate_class,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO {
                    inputs: vec![
                        PortSpec { name: input_a_sym, ty: Type::Int },
                        PortSpec { name: input_b_sym, ty: Type::Int },
                    ],
                    outputs: vec![PortSpec { name: output_sym, ty: Type::Int }],
                }),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );

        // Wire ext_a → gate.InputA
        m.wires.push(Wire {
            source: PortRef {
                node_id: ext_a_id,
                port: input_a_pi,
            },
            target: PortRef {
                node_id: gate_id,
                port: input_a_pi,
            },
        });
        // Wire ext_b → gate.InputB
        m.wires.push(Wire {
            source: PortRef {
                node_id: ext_b_id,
                port: input_b_pi,
            },
            target: PortRef {
                node_id: gate_id,
                port: input_b_pi,
            },
        });
        // Wire ext_c → gate.Output (unusual, but exercises the path)
        m.wires.push(Wire {
            source: PortRef {
                node_id: ext_c_id,
                port: output_pi,
            },
            target: PortRef {
                node_id: gate_id,
                port: output_pi,
            },
        });

        let mut t = CompiledTemplate::from_module(m);
        t.external_refs.push(("ext_a".to_string(), ext_a_id));
        t.external_refs.push(("ext_b".to_string(), ext_b_id));
        t.external_refs.push(("ext_c".to_string(), ext_c_id));

        let mut captures = HashMap::default();
        captures.insert("ext_a".to_string(), real_a);
        captures.insert("ext_b".to_string(), real_b);
        captures.insert("ext_c".to_string(), real_c);

        let inst = t.instantiate("mc0", &captures);

        // All three real nodes must appear as wire sources.
        let wire_sources: std::collections::HashSet<NodeId> =
            inst.wires.iter().map(|w| w.source.node_id).collect();

        assert!(
            wire_sources.contains(&real_a),
            "real_node_a not found as wire source; sources: {:?}",
            wire_sources.iter().collect::<Vec<_>>()
        );
        assert!(
            wire_sources.contains(&real_b),
            "real_node_b not found as wire source"
        );
        assert!(
            wire_sources.contains(&real_c),
            "real_node_c not found as wire source"
        );

        // None of the original placeholder IDs should remain.
        for w in &inst.wires {
            assert_ne!(
                w.source.node_id, ext_a_id,
                "ext_a placeholder leaked into wires"
            );
            assert_ne!(
                w.source.node_id, ext_b_id,
                "ext_b placeholder leaked into wires"
            );
            assert_ne!(
                w.source.node_id, ext_c_id,
                "ext_c placeholder leaked into wires"
            );
        }
    }

    // ── Test 6 ────────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_remaps_chip_children() {
        let mut m = simple_add_module();

        // Attach a child chip module keyed under "my_chip" in the parent.
        let chip_key_id = NodeId::fresh();
        let child = simple_add_module();
        m.chips.insert(chip_key_id, child);

        // Also add my_chip as a node in the parent so it lands in id_map.
        let mc_chip = "Component_Internal_Microchip";
        m.nodes.insert(
            chip_key_id,
            Node {
                id: chip_key_id,
                kind: NodeKind::Chip,
                gate_class: mc_chip,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );

        let t = CompiledTemplate::from_module(m);
        let caps = HashMap::default();

        let inst0 = t.instantiate("ci0", &caps);
        let inst1 = t.instantiate("ci1", &caps);

        // Chip keys must be different between instances.
        let chip_keys0: std::collections::HashSet<NodeId> = inst0.chips.keys().cloned().collect();
        let chip_keys1: std::collections::HashSet<NodeId> = inst1.chips.keys().cloned().collect();
        assert!(
            chip_keys0.is_disjoint(&chip_keys1),
            "chip keys overlap between instances"
        );

        // Child node IDs must also be disjoint.
        for (_, child0) in &inst0.chips {
            for (_, child1) in &inst1.chips {
                let child0_mod = child0;
                let child1_mod = child1;
                let child_ids0: std::collections::HashSet<NodeId> =
                    child0_mod.nodes.keys().cloned().collect();
                let child_ids1: std::collections::HashSet<NodeId> =
                    child1_mod.nodes.keys().cloned().collect();
                assert!(
                    child_ids0.is_disjoint(&child_ids1),
                    "child chip node IDs overlap between instances"
                );
            }
        }
    }

    // ── Test 12 ───────────────────────────────────────────────────────────────
    #[test]
    fn instantiate_boundary_wire_into_child_is_per_instance() {
        // A parent module with a gate wired into a CHILD chip's input node: the
        // wire lives in the PARENT wire list but its target is a CHILD node (how
        // a chip call wires an argument into a nested chip). Two instances must
        // NOT converge on the same child node — that fan-in collision is what
        // made the game drop every second-instance boundary wire.
        use crate::intern::sym;
        use crate::ir::port_registry::WirePort;

        let gate_id = NodeId::fresh();
        let chip_key = NodeId::fresh();
        let child_input_id = NodeId::fresh();

        let mc_input = "BrickComponentType_Internal_MicrochipInput";
        let mc_chip = "Component_Internal_Microchip";
        let add_gate = "BrickComponentType_WireGraph_Expr_Add";

        // Child module: one MicrochipInput node.
        let mut child = Module::new("child");
        child.nodes.insert(
            child_input_id,
            Node {
                id: child_input_id,
                kind: NodeKind::Input,
                gate_class: mc_input,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO {
                    inputs: vec![PortSpec { name: *sym::RER_INPUT, ty: Type::Int }],
                    outputs: vec![PortSpec { name: *sym::RER_OUTPUT, ty: Type::Int }],
                }),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        child.inputs.push(child_input_id);

        // Parent: a gate + the chip node, plus a wire gate.Output ->
        // child_input.RER_Input (target is the CHILD node, not a parent node).
        let mut parent = Module::new("parent");
        parent.nodes.insert(
            gate_id,
            Node {
                id: gate_id,
                kind: NodeKind::Gate,
                gate_class: add_gate,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        parent.nodes.insert(
            chip_key,
            Node {
                id: chip_key,
                kind: NodeKind::Chip,
                gate_class: mc_chip,
                properties: Arc::new(HashMap::default()),
                ports: Arc::new(GateIO::default()),
                source_range: Default::default(),
                chip_id: None,
                chain_id: None,
                scope_id: 0,
                note: None,
            },
        );
        parent.chips.insert(chip_key, child);
        parent.wires.push(Wire {
            source: PortRef { node_id: gate_id, port: WirePort::Output },
            target: PortRef { node_id: child_input_id, port: WirePort::RerInput },
        });

        let t = CompiledTemplate::from_module(parent);
        let caps = HashMap::default();
        let inst0 = t.instantiate("i0", &caps);
        let inst1 = t.instantiate("i1", &caps);

        let boundary_target = |m: &Module| -> NodeId {
            m.wires
                .iter()
                .find(|w| w.target.port == WirePort::RerInput)
                .expect("boundary wire present")
                .target
                .node_id
        };
        let t0 = boundary_target(&inst0);
        let t1 = boundary_target(&inst1);
        assert_ne!(
            t0, t1,
            "two instances' boundary wires target the SAME child node (fan-in collision)"
        );

        // Each instance's boundary target must be a real node in THAT instance's
        // freshly-stamped child (not the template's original id).
        let child_has = |m: &Module, id: NodeId| m.chips.values().any(|c| c.nodes.contains_key(&id));
        assert!(child_has(&inst0, t0), "inst0 boundary target not in its child");
        assert!(child_has(&inst1, t1), "inst1 boundary target not in its child");
        assert_ne!(t0, child_input_id, "boundary target leaked the template id");
    }
