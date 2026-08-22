    use super::*;
    use crate::ir::{GateIO, Module, Node, NodeKind, PortSpec, SourceRange, Type};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn gate(_label: &str) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class: crate::ir::gate_class::REROUTER,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec { name: *crate::intern::sym::RER_INPUT, ty: Type::Any }],
                outputs: vec![PortSpec { name: *crate::intern::sym::RER_OUTPUT, ty: Type::Any }],
            }),
            source_range: SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        }
    }

    #[test]
    fn empty_module_layout_empty() {
        let m = Module::new("empty");
        let l = layout(&m);
        assert!(l.placements.is_empty());
    }

    fn cube_opts() -> LayoutOptions {
        LayoutOptions { mode: LayoutMode::Cube, ..Default::default() }
    }

    #[test]
    fn layout_annotation_maps_to_its_engine() {
        let engine = |layout| {
            layout_options_for(
                &crate::ast::Script { layout, ..Default::default() },
                None,
            )
            .mode
        };
        assert_eq!(engine(Some(crate::ast::LayoutName::Cube)), LayoutMode::Cube);
        assert_eq!(engine(Some(crate::ast::LayoutName::Code)), LayoutMode::Code);
        assert_eq!(engine(None), LayoutMode::Dag);
    }

    /// The size fallback is a ceiling on DAG cost, not the only way in: asking
    /// for the cube gets it at eight nodes, five thousand short of the
    /// threshold. Stacking in `z` is what tells the two engines apart — the
    /// DAG layout is a single plane.
    #[test]
    fn cube_mode_is_forced_well_under_the_size_threshold() {
        let mut m = Module::new("cube");
        for _ in 0..8 {
            m.add_node(gate("g"));
        }
        assert!(m.nodes.len() < GRID_LAYOUT_THRESHOLD);

        let cube = layout_with_opts(&m, &cube_opts());
        let levels: std::collections::BTreeSet<i32> =
            cube.placements.values().map(|p| p.z).collect();
        assert!(levels.len() > 1, "cube stacks in z, got levels {levels:?}");

        let dag = layout_with_opts(&m, &LayoutOptions::default());
        assert_eq!(
            dag.placements.values().map(|p| p.z).collect::<std::collections::BTreeSet<_>>().len(),
            1,
            "the DAG layout is a single plane, so the check above is a real distinction"
        );
    }

    /// The game silently DROPS overlapping bricks, so a mode a file can now
    /// ask for at any size has to be swept with real footprints, not the
    /// nominal cell.
    #[test]
    fn cube_mode_never_overlaps_two_bricks() {
        let mut m = Module::new("cube");
        for _ in 0..40 {
            m.add_node(gate("g"));
        }
        let l = layout_with_opts(&m, &cube_opts());
        let boxes: Vec<(i32, i32, i32, i32, i32)> = l
            .placements
            .iter()
            .map(|(id, p)| {
                let (hsx, hsy) = brick_half_size(&m.nodes[id]);
                (p.z, p.x, p.x + 2 * hsx, p.y, p.y + 2 * hsy)
            })
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(
                    a.0 != b.0 || a.2 <= b.1 || b.2 <= a.1 || a.4 <= b.3 || b.4 <= a.3,
                    "cube bricks overlap: {a:?} and {b:?}"
                );
            }
        }
    }

    /// Layer spacing has to clear the bricks in the layer. Most gates are 4
    /// tall and fit any sane fixed step, so this pins the case that does not:
    /// a brick taller than the nominal step must still not reach the layer
    /// above it.
    #[test]
    fn cube_layers_clear_the_tallest_brick_they_hold() {
        // 80 units tall against a 12-unit nominal step, so a fixed step puts
        // this brick through the six layers above it.
        let class = "Component_Internal_ProjectileSpawner_Cannon";
        let tall = default_catalog()
            .find_by_class(class)
            .expect("catalog must know the class this test is built on")
            .half_size
            .z;
        assert!(2 * tall > 12, "brick must exceed the nominal step to pin anything");

        let mut m = Module::new("cube");
        for _ in 0..8 {
            m.add_node(Node { gate_class: class, ..gate("g") });
        }
        let l = layout_with_opts(&m, &cube_opts());
        let mut levels: Vec<i32> = l.placements.values().map(|p| p.z).collect();
        levels.sort();
        levels.dedup();
        for pair in levels.windows(2) {
            assert!(
                pair[1] - pair[0] >= 2 * tall,
                "layers {} and {} are closer than the {}-tall bricks in them",
                pair[0],
                pair[1],
                2 * tall
            );
        }
    }

    /// A chip's interior is laid by a separate call, so the mode has to travel
    /// with the options rather than being read once at the root.
    #[test]
    fn cube_mode_reaches_a_chips_interior() {
        let mut m = Module::new("outer");
        let chip = Node { kind: NodeKind::Chip, ..gate("c") };
        let chip_id = chip.id;
        m.add_node(chip);
        let mut inner = Module::new("inner");
        for _ in 0..8 {
            inner.add_node(gate("g"));
        }
        m.chips.insert(chip_id, inner);

        let l = layout_with_opts(&m, &cube_opts());
        let interior = l.chip_layouts.get(&chip_id).expect("chip interior laid");
        let levels: std::collections::BTreeSet<i32> =
            interior.placements.values().map(|p| p.z).collect();
        assert!(levels.len() > 1, "chip interior is not a cube: levels {levels:?}");
    }

    #[test]
    fn layout_output_is_deterministic() {
        let mut m = Module::new("det");
        for id in ["a", "b", "c"] {
            m.add_node(gate(id));
        }
        let a = layout(&m);
        let b = layout(&m);
        assert_eq!(a.placements, b.placements);
    }

    #[test]
    fn every_node_gets_a_placement() {
        let mut m = Module::new("coverage");
        for id in ["a", "b", "c", "d"] {
            m.add_node(gate(id));
        }
        let l = layout(&m);
        assert_eq!(l.placements.len(), m.nodes.len());
    }

    #[test]
    fn nested_chip_gets_its_own_layout() {
        let mut parent = Module::new("parent");
        let mut chip_node = gate("my_chip");
        chip_node.kind = NodeKind::Chip;
        chip_node.gate_class = crate::ir::gate_class::MICROCHIP;
        let chip_id = chip_node.id;
        parent.add_node(chip_node);

        let mut child = Module::new_chip_body("child", "my_chip");
        let inner_node = gate("inner");
        let inner_id = inner_node.id;
        child.add_node(inner_node);
        parent.chips.insert(chip_id, child);

        let l = layout(&parent);
        assert!(
            l.placements.contains_key(&chip_id),
            "parent must place the chip node"
        );
        let child_l = l
            .chip_layouts
            .get(&chip_id)
            .expect("chip layout must exist");
        assert!(
            child_l.placements.contains_key(&inner_id),
            "child module must place its inner gate"
        );
    }

    #[test]
    fn layout_options_passed_through_to_chip_recursion() {
        // Smoke test: different options don't alter the collapsed-mode
        // output (the only mode we currently implement). AdjacentInline
        // is a no-op placeholder today.
        let mut parent = Module::new("parent");
        parent.add_node(gate("a"));
        let default_out = layout(&parent);
        let inline_out = layout_with_opts(
            &parent,
            &LayoutOptions {
                chips: ChipLayoutMode::AdjacentInline,
                ..Default::default()
            },
        );
        assert_eq!(default_out.placements, inline_out.placements);
    }
