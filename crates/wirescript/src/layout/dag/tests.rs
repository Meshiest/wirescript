    use super::*;
    use crate::diagnostic::{Pos, SourceRange};
    use crate::ir::port_registry::WirePort;
    use crate::ir::{GateIO, Literal, Module, Node, NodeKind, PortRef, ROOT_SCOPE_ID, ScopeId};
    use std::sync::Arc;

    fn make_node(gate_class: &'static str, offset: usize) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(GateIO::default()),
            source_range: SourceRange {
                file: "t".into(),
                start: Pos {
                    offset,
                    line: 0,
                    col: 0,
                },
                end: Pos {
                    offset: offset + 1,
                    line: 0,
                    col: 0,
                },
            },
            chip_id: None,
            chain_id: None,
            scope_id: ROOT_SCOPE_ID,
            note: None,
        }
    }

    fn buffer_node(offset: usize) -> Node {
        make_node("BrickComponentType_Internal_Variable", offset)
    }

    fn make_wire(src: NodeId, dst: NodeId) -> Wire {
        Wire {
            source: PortRef {
                node_id: src,
                port: WirePort::Output,
            },
            target: PortRef {
                node_id: dst,
                port: WirePort::Input,
            },
        }
    }

    fn leaf_region<'a>(module: &'a Module) -> super::super::region::Region<'a> {
        super::super::region::build_region_tree(module)
    }

    fn into_module(nodes: Vec<Node>, wires: Vec<Wire>) -> Module {
        let mut m = Module::default();
        for n in nodes {
            m.nodes.insert(n.id, n);
        }
        m.wires = wires;
        m
    }

    #[test]
    fn linear_chain_gets_increasing_depth() {
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let c = make_node("G", 2);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let m = into_module(
            vec![a, b, c],
            vec![make_wire(a_id, b_id), make_wire(b_id, c_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(lay.local[&a_id].dx, 0);
        assert_eq!(lay.local[&b_id].dx, 1);
        assert_eq!(lay.local[&c_id].dx, 2);
        assert!(lay.feedback_edges.is_empty());
    }

    #[test]
    fn buffer_cycle_drops_buffer_edge_and_does_not_panic() {
        // a → buf → a  (buf is a Variable so is_buffer() = true)
        let a = make_node("G", 0);
        let buf = buffer_node(1);
        let a_id = a.id;
        let buf_id = buf.id;
        let m = into_module(
            vec![a, buf],
            vec![make_wire(a_id, buf_id), make_wire(buf_id, a_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        // The a → buf (Exec, target is buffer) edge is the preferred
        // feedback edge.
        assert_eq!(lay.feedback_edges.len(), 1);
        assert_eq!(lay.feedback_edges[0].1, buf_id);
        assert!(lay.warnings.is_empty());
    }

    #[test]
    fn multi_cycle_scc_breaks_all_cycles_without_panicking() {
        // One SCC, two distinct cycles sharing node a: a → b → a and a → c → a.
        // A single edge removal leaves the second cycle — layout must iterate
        // until the graph is acyclic instead of panicking in toposort.
        let a = make_node("G", 0);
        let b = buffer_node(1);
        let c = buffer_node(2);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let m = into_module(
            vec![a, b, c],
            vec![
                make_wire(a_id, b_id),
                make_wire(b_id, a_id),
                make_wire(a_id, c_id),
                make_wire(c_id, a_id),
            ],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(
            lay.feedback_edges.len(),
            2,
            "both cycles must contribute a feedback edge"
        );
    }

    #[test]
    fn non_buffer_cycle_falls_back_and_warns() {
        // Two regular gates forming a cycle — no buffer available.
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let a_id = a.id;
        let b_id = b.id;
        let m = into_module(
            vec![a, b],
            vec![make_wire(a_id, b_id), make_wire(b_id, a_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(lay.feedback_edges.len(), 1);
        assert_eq!(lay.warnings.len(), 1);
    }

    #[test]
    fn disconnected_subgraphs_dont_overlap() {
        // Two isolated pairs: (a → b) and (c → d). b's dy and d's dy
        // must differ (they're at the same dx=1 but in different WCCs).
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let c = make_node("G", 2);
        let d = make_node("G", 3);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let d_id = d.id;
        let m = into_module(
            vec![a, b, c, d],
            vec![make_wire(a_id, b_id), make_wire(c_id, d_id)],
        );
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        // No two placements share the same (dx, dy).
        let mut seen: HashSet<(i32, i32)> = HashSet::default();
        for p in lay.local.values() {
            assert!(
                seen.insert((p.dx, p.dy)),
                "duplicate placement ({}, {})",
                p.dx,
                p.dy
            );
        }
    }

    #[test]
    fn empty_region_has_zero_bbox() {
        let m = Module::default();
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        assert_eq!(lay.bbox, (0, 0));
        assert!(lay.local.is_empty());
    }

    #[test]
    fn layout_is_deterministic() {
        let a = make_node("G", 0);
        let b = make_node("G", 1);
        let c = make_node("G", 2);
        let d = make_node("G", 3);
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        let d_id = d.id;
        let m = into_module(
            vec![a, b, c, d],
            vec![
                make_wire(a_id, b_id),
                make_wire(a_id, c_id),
                make_wire(b_id, d_id),
                make_wire(c_id, d_id),
            ],
        );
        let root = leaf_region(&m);
        let a = layout_leaf(&root, &m.wires);
        let b = layout_leaf(&root, &m.wires);
        assert_eq!(a.local, b.local);
        assert_eq!(a.bbox, b.bbox);
    }

    #[test]
    fn source_order_breaks_y_ties() {
        // Two nodes both at dx=0 (no predecessors) — the one with the
        // earlier source_range must get the smaller dy.
        let early = make_node("G", 0);
        let late = make_node("G", 100);
        let early_id = early.id;
        let late_id = late.id;
        let m = into_module(vec![early, late], vec![]);
        let root = leaf_region(&m);
        let lay = layout_leaf(&root, &m.wires);
        // Both are their own WCCs → stacked vertically.
        let early_p = lay.local[&early_id];
        let late_p = lay.local[&late_id];
        assert!(early_p.dy < late_p.dy, "early should come first on y axis");
        // Unused-import guard for Literal/ScopeId.
        let _ = (Literal::Bool(false), ROOT_SCOPE_ID as ScopeId);
    }
