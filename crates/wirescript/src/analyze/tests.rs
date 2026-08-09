    use super::*;
    use crate::ir::{Module, Node, NodeKind, PortRef, Wire};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn gate(class: &'static str) -> Node {
        Node {
            id: NodeId::fresh(),
            kind: NodeKind::Gate,
            gate_class: class,
            properties: Arc::new(HashMap::default()),
            ports: Arc::new(crate::GateIO::default()),
            source_range: SourceRange::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        }
    }

    fn wire_between(src: NodeId, dst: NodeId) -> Wire {
        Wire {
            source: PortRef {
                node_id: src,
                port: crate::ir::port_registry::WirePort::from_name("Output"),
            },
            target: PortRef {
                node_id: dst,
                port: crate::ir::port_registry::WirePort::from_name("Input"),
            },
        }
    }

    #[test]
    fn no_cycles_no_diags() {
        let mut m = Module::new("clean");
        let a = gate("X");
        let b = gate("Y");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(wire_between(a_id, b_id));
        let r = analyze_cycles(&m);
        assert!(r.strongly_connected.is_empty());
        assert!(r.diagnostics.is_empty());
    }

    #[test]
    fn cycle_without_barrier_diags() {
        let mut m = Module::new("loop");
        let a = gate("X");
        let b = gate("Y");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(wire_between(a_id, b_id));
        m.add_wire(wire_between(b_id, a_id));
        let r = analyze_cycles(&m);
        assert_eq!(r.strongly_connected.len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].code, "WS005");
    }

    #[test]
    fn cycle_diag_lists_loop_members() {
        // A → B → A: the WS005 message must name both gates and show the path so
        // the loop is actually diagnosable.
        let mut m = Module::new("loop");
        let a = gate("Alpha");
        let b = gate("Beta");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(wire_between(a_id, b_id));
        m.add_wire(wire_between(b_id, a_id));
        let r = analyze_cycles(&m);
        assert_eq!(r.diagnostics.len(), 1);
        let msg = &r.diagnostics[0].message;
        assert!(msg.contains("Alpha"), "should name the gate: {msg}");
        assert!(msg.contains("Beta"), "should name the gate: {msg}");
        assert!(msg.contains("->"), "should show the loop path: {msg}");
        assert!(msg.contains("2 gate(s) in the loop"), "should count the loop: {msg}");
    }

    fn layout_wire(src: NodeId, dst: NodeId) -> Wire {
        Wire {
            source: PortRef {
                node_id: src,
                port: crate::ir::port_registry::WirePort::Layout,
            },
            target: PortRef {
                node_id: dst,
                port: crate::ir::port_registry::WirePort::Layout,
            },
        }
    }

    #[test]
    fn layout_wires_are_not_signal_cycles() {
        // A loop made only of `_Layout` placement wires is cosmetic, not signal flow,
        // and must NOT raise WS005 (regression: chip-inline layout edges did).
        let mut m = Module::new("layout-loop");
        let a = gate("Alpha");
        let b = gate("Beta");
        let a_id = a.id;
        let b_id = b.id;
        m.add_node(a);
        m.add_node(b);
        m.add_wire(layout_wire(a_id, b_id));
        m.add_wire(layout_wire(b_id, a_id));
        let r = analyze_cycles(&m);
        assert!(
            r.diagnostics.is_empty(),
            "layout wires must not trip the cycle check: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn buffer_breaks_cycle() {
        let mut m = Module::new("buffered");
        let a = gate("X");
        let buf = gate("BrickComponentType_WireGraphPseudo_BufferTicks");
        let a_id = a.id;
        let buf_id = buf.id;
        m.add_node(a);
        m.add_node(buf);
        m.add_wire(wire_between(a_id, buf_id));
        m.add_wire(wire_between(buf_id, a_id));
        let r = analyze_cycles(&m);
        assert_eq!(r.strongly_connected.len(), 1);
        assert!(r.diagnostics.is_empty(), "barrier should suppress the diag");
    }
