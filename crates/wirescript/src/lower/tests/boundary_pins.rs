use crate::ir::{Module, NodeId, Type, gate_class as gc, port_registry::WirePort};
use crate::lower::boundary_pins::synthesize_boundary_pins;
use crate::template_cache::TemplateCache;

use super::*;

fn lowered(src: &str) -> LowerResult {
    let parsed = crate::parser::parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = crate::typecheck::typecheck(&parsed.ast, "test");
    let r = lower(LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: std::sync::Arc::new(TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
        fold_mode: FoldMode::Auto,
    });
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    r
}

/// Recursively collects every synthesized boundary pin's `PortLabel` across
/// the whole module tree.
fn collect_pin_labels(m: &crate::ir::Module, out: &mut Vec<String>) {
    for n in m.nodes.values() {
        if n.note == Some("boundary_pin") {
            if let Some(crate::ir::Literal::String(s)) =
                n.properties.get(&*crate::intern::sym::PORT_LABEL)
            {
                out.push(s.clone());
            }
        }
    }
    for c in m.chips.values() {
        collect_pin_labels(c, out);
    }
}

/// Reads a node's `PortLabel` property as a string, if present.
fn port_label(n: &crate::ir::Node) -> Option<String> {
    match n.properties.get(&*crate::intern::sym::PORT_LABEL) {
        Some(crate::ir::Literal::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Total `MicrochipInput` node count across the whole module tree
/// (declared params, plus anything this pass has already added).
fn chip_input_count(m: &Module) -> usize {
    m.inputs.len() + m.chips.values().map(chip_input_count).sum::<usize>()
}

fn total_node_count(m: &Module) -> usize {
    m.nodes.len() + m.chips.values().map(total_node_count).sum::<usize>()
}

fn total_wire_count(m: &Module) -> usize {
    m.wires.len() + m.chips.values().map(total_wire_count).sum::<usize>()
}

/// A pin synthesized by this pass, as opposed to one declared in source.
fn is_boundary_pin(n: &crate::ir::Node) -> bool {
    n.note == Some("boundary_pin")
}

/// Synthesized boundary pins across the whole module tree.
fn boundary_pin_count(m: &Module) -> usize {
    m.nodes.values().filter(|n| is_boundary_pin(n)).count()
        + m.chips.values().map(boundary_pin_count).sum::<usize>()
}

/// A port declared in source: carries a real source range (synthesized pins
/// are built with `SourceRange::default()`) and no `boundary_pin` marker.
fn is_declared_port(n: &crate::ir::Node) -> bool {
    !is_boundary_pin(n) && (n.source_range.end.offset > 0 || !n.source_range.file.is_empty())
}

/// After the pass, every wire must be legal relative to its endpoints'
/// common path prefix (the wire's LCA): each endpoint sits at most one
/// nesting level below the LCA, and every endpoint that IS one level below
/// must be a MicrochipInput/Output pin. This admits module-local wires,
/// parent<->child-pin hops, and the sibling middle wire (outpin → inpin,
/// both one level below their shared prefix on different branches), while
/// still rejecting any wire spanning >1 wall on either side and any deep
/// endpoint that isn't a pin. No (target node, target port) tuple may
/// appear twice.
fn assert_pin_adjacent(root: &Module) {
    fn owners(
        m: &Module,
        path: &mut Vec<NodeId>,
        out: &mut std::collections::HashMap<NodeId, Vec<NodeId>>,
    ) {
        for id in m.nodes.keys() {
            out.insert(*id, path.clone());
        }
        let mut kids: Vec<_> = m.chips.keys().copied().collect();
        kids.sort();
        for k in kids {
            path.push(k);
            owners(&m.chips[&k], path, out);
            path.pop();
        }
    }
    // Gate class of a node found by descending `path` from `root`, so the
    // caller can check it's the pin kind the wire DIRECTION requires — a
    // wire's deep source endpoint is only legal through an output pin, and
    // its deep target endpoint only through an input pin (a backwards
    // inpin→outpin wire at the deep end must fail).
    fn class_at(root: &Module, path: &[NodeId], id: NodeId) -> Option<&'static str> {
        let mut m = root;
        for c in path {
            m = &m.chips[c];
        }
        m.nodes.get(&id).map(|n| n.gate_class)
    }
    let mut own = std::collections::HashMap::new();
    owners(root, &mut Vec::new(), &mut own);
    let mut targets = std::collections::HashSet::new();
    fn walk(
        root: &Module,
        m: &Module,
        own: &std::collections::HashMap<NodeId, Vec<NodeId>>,
        targets: &mut std::collections::HashSet<(NodeId, crate::ir::port_registry::WirePort)>,
    ) {
        for w in &m.wires {
            if w.source.port == crate::ir::port_registry::WirePort::Layout {
                continue;
            }
            assert!(
                targets.insert((w.target.node_id, w.target.port)),
                "duplicate wire target tuple {:?}",
                w.target
            );
            let (Some(sp), Some(tp)) = (own.get(&w.source.node_id), own.get(&w.target.node_id))
            else {
                continue;
            };
            if sp == tp {
                continue;
            }
            let lca = sp.iter().zip(tp.iter()).take_while(|(a, b)| a == b).count();
            assert!(
                sp.len() <= lca + 1,
                "source end spans >1 wall from the wire's LCA: {:?}",
                w
            );
            assert!(
                tp.len() <= lca + 1,
                "target end spans >1 wall from the wire's LCA: {:?}",
                w
            );
            if sp.len() == lca + 1 {
                assert_eq!(
                    class_at(root, sp, w.source.node_id),
                    Some(gc::MICROCHIP_OUTPUT),
                    "deep source endpoint of {:?} must be a MicrochipOutput pin",
                    w
                );
            }
            if tp.len() == lca + 1 {
                assert_eq!(
                    class_at(root, tp, w.target.node_id),
                    Some(gc::MICROCHIP_INPUT),
                    "deep target endpoint of {:?} must be a MicrochipInput pin",
                    w
                );
            }
        }
        for c in m.chips.values() {
            walk(root, c, own, targets);
        }
    }
    walk(root, root, &own, &mut targets);
}

#[test]
fn global_pure_read_gets_labeled_input_pin() {
    let r = lowered("var g: int = 1\nchip C() -> (r: int) { out r = g + 2 }\nlet c = C()\n");
    let chip = r.module.chips.values().next().unwrap();
    let added: Vec<_> = chip
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .filter(|n| port_label(n).as_deref() == Some("g"))
        .collect();
    assert_eq!(added.len(), 1, "one labeled pin for the g crossing");
    assert_eq!(
        chip_input_count(&r.module),
        1,
        "C() declares no `in` params, so the synthesized g pin is the only registered input"
    );
    assert_pin_adjacent(&r.module);
}

/// Two `out` bindings in the same chip both read the same external var
/// directly (`.Value`) — the two crossings share one feeder, so `pin_for`'s
/// dedup should collapse them onto a single pin with two interior wires.
#[test]
fn two_consumers_share_one_pin() {
    let r = lowered(
        "var g: int = 1\nchip C() -> (r: int, s: int) { out r = g + 1\n  out s = g + 2 }\nlet c = C()\n",
    );
    let chip = r.module.chips.values().next().unwrap();
    let added: Vec<_> = chip
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .filter(|n| port_label(n).as_deref() == Some("g"))
        .collect();
    assert_eq!(added.len(), 1, "the two reads of g must share one pin");
    let pin_id = added[0].id;
    let interior = chip
        .wires
        .iter()
        .filter(|w| w.source.node_id == pin_id)
        .count();
    assert_eq!(interior, 2, "the shared pin fans out to both consumers");
    assert_pin_adjacent(&r.module);
}

/// A value computed inside a top-level anon chip block, consumed by a
/// top-level `out` binding after the block closes: a raw (unpinned)
/// crossing pre-pass, since anon chips carry no declared ports of their own.
#[test]
fn inside_value_consumed_outside_gets_output_pin() {
    let r = lowered("chip { let x = 5 + 5 }\nout y = x\n");
    let chip = r.module.chips.values().next().unwrap();
    let added: Vec<_> = chip
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_OUTPUT)
        .collect();
    assert_eq!(added.len(), 1, "the chip-internal value needs one new output pin");
    assert_pin_adjacent(&r.module);
}

/// Producer's declared output feeding Consumer's declared input, as an
/// argument (`Consumer(p.r)`) — ordinary declared chip I/O already wires
/// this pin to pin between two sibling modules, with the middle wire held
/// at their LCA (root). That is exactly the approved boundary shape:
/// MicrochipOutput in the source chip, MicrochipInput in the target chip,
/// direct middle wire at the LCA — so the pass must keep it, adding no
/// bridge gate and no extra pins.
#[test]
fn sibling_chips_chain_out_then_in() {
    let r = lowered(
        "chip Producer() -> (r: int) { out r = 5 }\nchip Consumer(v: int) -> (r2: int) { out r2 = v }\nlet p = Producer()\nlet c = Consumer(p.r)\n",
    );
    assert_pin_adjacent(&r.module);

    let producer = r
        .module
        .chips
        .values()
        .find(|c| c.inputs.is_empty())
        .expect("producer module");
    let consumer = r
        .module
        .chips
        .values()
        .find(|c| !c.inputs.is_empty())
        .expect("consumer module");
    assert_eq!(producer.outputs.len(), 1, "producer keeps its single declared output pin");
    assert_eq!(consumer.inputs.len(), 1, "consumer keeps its single declared input pin");
    // The middle segment is a DIRECT outpin→inpin wire held at the LCA (root).
    let producer_out = producer.outputs[0];
    let consumer_in = consumer.inputs[0];
    let direct = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == producer_out && w.target.node_id == consumer_in)
        .count();
    assert_eq!(
        direct, 1,
        "exactly one direct outpin→inpin middle wire, held in the root (LCA) module"
    );
    fn has_rerouter(m: &Module) -> bool {
        m.nodes.values().any(|n| n.gate_class == gc::REROUTER)
            || m.chips.values().any(has_rerouter)
    }
    assert!(!has_rerouter(&r.module), "no bridge rerouter is created anywhere");
}

/// A normal chip call's declared outpin→inpin wire is already the approved
/// boundary shape, so the pass — which `lower()` runs unconditionally —
/// must leave it alone on its real, first run: the middle wire still runs
/// directly between the two DECLARED pins, and no pin was synthesized
/// anywhere in the tree.
#[test]
fn declared_chip_call_wires_are_untouched() {
    let r = lowered(
        "chip Producer() -> (r: int) { out r = 5 }\nchip Consumer(v: int) -> (r2: int) { out r2 = v }\nlet p = Producer()\nlet c = Consumer(p.r)\n",
    );
    let producer = r
        .module
        .chips
        .values()
        .find(|c| c.inputs.is_empty())
        .expect("producer module");
    let consumer = r
        .module
        .chips
        .values()
        .find(|c| !c.inputs.is_empty())
        .expect("consumer module");
    assert_eq!(producer.outputs.len(), 1, "producer keeps its single declared output pin");
    assert_eq!(consumer.inputs.len(), 1, "consumer keeps its single declared input pin");

    // Both endpoints are ports DECLARED in source, not synthesized pins.
    let producer_out = producer.outputs[0];
    let consumer_in = consumer.inputs[0];
    let out_node = &producer.nodes[&producer_out];
    let in_node = &consumer.nodes[&consumer_in];
    assert!(
        is_declared_port(out_node),
        "producer's out pin must be the source-declared one, not a synthesized pin"
    );
    assert!(
        is_declared_port(in_node),
        "consumer's in pin must be the source-declared one, not a synthesized pin"
    );
    assert_eq!(out_node.gate_class, gc::MICROCHIP_OUTPUT);
    assert_eq!(in_node.gate_class, gc::MICROCHIP_INPUT);

    // The middle segment is a DIRECT outpin→inpin wire held at the LCA (root).
    let direct = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == producer_out && w.target.node_id == consumer_in)
        .count();
    assert_eq!(
        direct, 1,
        "exactly one direct declared-outpin→declared-inpin middle wire, held in the root (LCA) module"
    );

    // Declared wiring needs no help, so the pass synthesized nothing.
    assert_eq!(
        boundary_pin_count(&r.module),
        0,
        "an all-declared chip call must not produce any synthesized boundary pin"
    );
    fn has_rerouter(m: &Module) -> bool {
        m.nodes.values().any(|n| n.gate_class == gc::REROUTER)
            || m.chips.values().any(has_rerouter)
    }
    assert!(!has_rerouter(&r.module), "no bridge rerouter is created anywhere");
    assert_pin_adjacent(&r.module);
}

/// `var g` read from inside a doubly-nested named chip call
/// (`Outer -> Inner`, both real call instances) reaches two walls with no
/// intermediate pin pre-pass — the pass must add one pin per wall.
#[test]
fn two_level_nesting_chains_pins_through_each_wall() {
    let r = lowered(
        "var g: int = 1\nchip Inner() -> (r: int) { out r = g + 1 }\nchip Outer() -> (r: int) {\n  let i = Inner()\n  out r = i.r\n}\nlet o = Outer()\n",
    );
    let outer = r.module.chips.values().next().unwrap();
    let outer_added: Vec<_> = outer
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .collect();
    assert_eq!(outer_added.len(), 1, "outer wall gets one new input pin");
    let inner = outer.chips.values().next().unwrap();
    let inner_added: Vec<_> = inner
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .collect();
    assert_eq!(inner_added.len(), 1, "inner wall gets one new input pin");
    assert_pin_adjacent(&r.module);
}

/// `in trigger: exec` at root feeding directly into a `chip on trigger`
/// anon chip's interior exec chain is a raw Exec-typed crossing.
#[test]
fn exec_crossing_gets_exec_pin() {
    let r = lowered("in trigger: exec\nchip on trigger { var x: int = 0\n  x = 1 }\n");
    let chip = r.module.chips.values().next().unwrap();
    let exec_pins: Vec<_> = chip
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .filter(|n| n.ports.outputs.iter().any(|p| p.ty == Type::Exec))
        .collect();
    assert_eq!(exec_pins.len(), 1, "the exec crossing gets its own exec-typed pin");
    assert_pin_adjacent(&r.module);
}

/// `g = 1` inside a chip's exec handler (global var, not a declared ref
/// param): the Var_Set's VarRef wire is a raw crossing back to root's
/// Pseudo_Var. After the pass, the interior chain is
/// Pseudo_Var -> pin -> Var_Set.VarRef, and the chip's scope_captures is
/// recomputed to match a fresh `compute_scope_captures`.
#[test]
fn varref_crossing_gets_local_pin_and_captures_update() {
    let r = lowered("var g: int = 1\nchip C() { in run: exec\n  on run { g = 1 } }\nlet c = C()\n");
    let chip = r.module.chips.values().next().unwrap();
    let varref_pin = chip.nodes.values().find(|n| {
        n.gate_class == gc::MICROCHIP_INPUT
            && n.ports.outputs.iter().any(|p| matches!(p.ty, Type::Ref(_)))
    });
    assert!(varref_pin.is_some(), "the VarRef crossing gets a local pin");
    let pin_id = varref_pin.unwrap().id;
    assert!(
        chip.wires
            .iter()
            .any(|w| w.source.node_id == pin_id && w.target.port == WirePort::VarRef),
        "interior wire chain runs pin -> Var_Set.VarRef"
    );
    let recomputed = crate::lower::call::compute_scope_captures(chip);
    let mut stored = chip.scope_captures.clone();
    let mut recomputed_sorted = recomputed.clone();
    stored.sort_by_key(|id| id.0);
    recomputed_sorted.sort_by_key(|id| id.0);
    assert_eq!(
        stored, recomputed_sorted,
        "scope_captures matches a fresh compute_scope_captures"
    );
    assert_pin_adjacent(&r.module);
}

/// Constructed by hand rather than via `lowered(src)`: in practice, real
/// Wirescript source rarely leaves a genuine cross-module `_Literal`-sourced
/// wire lying around by the time `lower()` finishes — top-level const
/// `let`s get cloned into any chip that references them, chip-call constant
/// arguments get folded via a dedicated `ConstFold` path, and
/// `materialize_unfoldable_constants`/`inline_orphan_literals` absorb same-
/// module literal uses — so this exercises the exclusion rule directly
/// against a minimal, deliberately-shaped module instead of fishing for an
/// increasingly contrived source program.
#[test]
fn literal_source_crossings_are_left_alone() {
    use crate::ir::{GateIO, Literal, Node, NodeKind, PortRef, PortSpec, Wire, port_registry::WirePort};
    use std::sync::Arc;

    let lit_id = NodeId::fresh();
    let consumer_id = NodeId::fresh();
    let chip_id = NodeId::fresh();

    let mut child = Module::new_chip_body("Child", "Child");
    let mut consumer_props = crate::collections::HashMap::default();
    consumer_props.insert(crate::intern::intern("InputA"), Literal::Int(0));
    child.nodes.insert(
        consumer_id,
        Node {
            id: consumer_id,
            kind: NodeKind::Gate,
            gate_class: "BrickComponentType_WireGraph_Expr_MathAdd",
            properties: Arc::new(consumer_props),
            ports: Arc::new(GateIO {
                inputs: vec![PortSpec {
                    name: crate::intern::intern("InputA"),
                    ty: Type::Int,
                }],
                outputs: vec![PortSpec {
                    name: crate::intern::intern("Output"),
                    ty: Type::Int,
                }],
            }),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        },
    );

    let mut root = Module::new("root");
    let mut lit_props = crate::collections::HashMap::default();
    lit_props.insert(*crate::intern::sym::VALUE, Literal::Int(7));
    root.nodes.insert(
        lit_id,
        Node {
            id: lit_id,
            kind: NodeKind::Gate,
            gate_class: gc::LITERAL,
            properties: Arc::new(lit_props),
            ports: Arc::new(GateIO {
                inputs: vec![],
                outputs: vec![PortSpec {
                    name: crate::intern::intern("Output"),
                    ty: Type::Int,
                }],
            }),
            source_range: Default::default(),
            chip_id: None,
            chain_id: None,
            scope_id: crate::ir::ROOT_SCOPE_ID,
            note: None,
        },
    );
    root.chips.insert(chip_id, child);
    root.wires.push(Wire {
        source: PortRef {
            node_id: lit_id,
            port: WirePort::Output,
        },
        target: PortRef {
            node_id: consumer_id,
            port: WirePort::InputA,
        },
    });

    let nodes_before = total_node_count(&root);
    let wires_before = total_wire_count(&root);
    synthesize_boundary_pins(&mut root);
    assert_eq!(total_node_count(&root), nodes_before, "no pin synthesized for a literal source");
    assert_eq!(total_wire_count(&root), wires_before, "the literal-sourced wire is untouched");
    assert_eq!(root.wires.len(), 1);
    assert_eq!(root.wires[0].source.node_id, lit_id);
    assert_eq!(root.wires[0].target.node_id, consumer_id);
}

/// Every named chip call gets `template_key = Some(chip_name)` from ordinary
/// lowering (`call.rs` sets it unconditionally, not just for repeats).
/// Adding a pin mutates the module's content, so the pass must invalidate it.
#[test]
fn templated_module_receiving_pin_loses_template_key() {
    // Control: no crossing, so the pass never touches this chip and
    // `template_key` keeps the value ordinary lowering assigned it.
    let baseline = lowered("chip Foo() -> (r: int) { out r = 1 }\nlet f = Foo()\n");
    let baseline_chip = baseline.module.chips.values().next().unwrap();
    assert!(
        baseline_chip.template_key.is_some(),
        "sanity: template_key stays set when there's no crossing to pin"
    );

    let r = lowered("var g: int = 1\nchip Foo() -> (r: int) { out r = g + 1 }\nlet f = Foo()\n");
    let chip = r.module.chips.values().next().unwrap();
    assert!(chip.template_key.is_none(), "receiving a new pin invalidates template_key");
    assert_pin_adjacent(&r.module);
}

/// A self-contained program with nothing to cross: the pass — which
/// `lower()` runs unconditionally — must synthesize no pin and relocate no
/// wire on its real, first run, leaving every wire module-local.
#[test]
fn no_crossings_is_a_no_op() {
    let r = lowered("var g: int = 1\nout r = g + 2\n");
    assert!(
        r.module.chips.is_empty(),
        "fixture sanity: a flat program has no chip walls to cross"
    );
    assert!(
        !r.module.wires.is_empty(),
        "fixture sanity: there are wires, so the checks below aren't vacuous"
    );
    assert_eq!(
        boundary_pin_count(&r.module),
        0,
        "a program with no crossing must not gain a synthesized boundary pin"
    );
    // Every wire stayed where lowering put it: both endpoints resolve in the
    // module that holds the wire.
    for w in &r.module.wires {
        assert!(
            r.module.nodes.contains_key(&w.source.node_id)
                && r.module.nodes.contains_key(&w.target.node_id),
            "wire {w:?} must keep both endpoints in its own module"
        );
    }
    assert_pin_adjacent(&r.module);
}

#[test]
fn scope_pins_are_labeled_with_the_identifier() {
    let r = lowered("var score: int = 0\narray log: string[]\nin go: exec\non go {\n  chip {\n    score = score + 1\n    log.push(\"x\")\n  }\n}\n");
    let mut labels: Vec<String> = Vec::new();
    collect_pin_labels(&r.module, &mut labels);
    assert!(labels.iter().any(|l| l == "score"), "got {labels:?}");
    assert!(labels.iter().any(|l| l == "log"), "got {labels:?}");
}

#[test]
fn pin_labels_are_never_internal_notes_or_empty() {
    let r = lowered("var score: int = 0\nin go: exec\non go {\n  chip {\n    score = score + 1\n    PrintToConsole(\"${score}\")\n  }\n}\n");
    let mut labels: Vec<String> = Vec::new();
    collect_pin_labels(&r.module, &mut labels);
    assert!(!labels.is_empty());
    for l in &labels {
        assert!(!l.is_empty(), "empty pin label");
        assert!(!l.contains(' '), "internal note leaked as a pin label: {l:?}");
        assert!(!l.starts_with("Math"), "gate class leaked as a pin label: {l:?}");
    }
}

/// Reproduces the exact leak the label policy exists to close: `arr[0]`
/// lowers to an `ARRAY_GET` gate carrying `Node.note == Some("array get")`
/// and no `NAME_LABEL`/`PORT_LABEL`. Bound with `let` (not a `var`), its
/// `Value` output is used directly rather than through a named Var_Get, so
/// when the nested chip consumes it the crossing's source node has no
/// identifier at all.
#[test]
fn array_element_crossing_never_leaks_the_internal_note() {
    let r = lowered(
        "array arr: int[]\nin go: exec\non go {\n  let v = arr[0]\n  chip {\n    PrintToConsole(\"${v}\")\n  }\n}\n",
    );
    let mut labels: Vec<String> = Vec::new();
    collect_pin_labels(&r.module, &mut labels);
    assert!(!labels.is_empty(), "expected at least one synthesized pin");
    for l in &labels {
        assert!(!l.is_empty(), "empty pin label");
        assert!(!l.contains(' '), "internal note leaked as a pin label: {l:?}");
    }
    let mut seen = std::collections::HashSet::new();
    for l in &labels {
        assert!(seen.insert(l.clone()), "duplicate pin label {l:?}");
    }
    assert_pin_adjacent(&r.module);
}

#[test]
fn pin_labels_are_unique_within_a_module() {
    let r = lowered("var a: int = 0\nin go: exec\non go {\n  chip {\n    a = (a + 1) * (a + 2)\n  }\n}\n");
    fn check(m: &crate::ir::Module) {
        let mut seen = std::collections::HashSet::new();
        for n in m.nodes.values() {
            if n.note == Some("boundary_pin") {
                if let Some(crate::ir::Literal::String(s)) =
                    n.properties.get(&*crate::intern::sym::PORT_LABEL)
                {
                    assert!(seen.insert(s.clone()), "duplicate pin label {s:?}");
                }
            }
        }
        for c in m.chips.values() {
            check(c);
        }
    }
    check(&r.module);
}

#[test]
fn pass_is_idempotent() {
    let mut r = lowered(
        "chip Producer() -> (r: int) { out r = 5 }\nchip Consumer(v: int) -> (r2: int) { out r2 = v }\nlet p = Producer()\nlet c = Consumer(p.r)\n",
    );
    synthesize_boundary_pins(&mut r.module);
    assert_pin_adjacent(&r.module);
    let nodes_after_one = total_node_count(&r.module);
    let wires_after_one = total_wire_count(&r.module);
    synthesize_boundary_pins(&mut r.module);
    assert_eq!(total_node_count(&r.module), nodes_after_one, "second run adds no nodes");
    assert_eq!(total_wire_count(&r.module), wires_after_one, "second run adds no wires");
    assert_pin_adjacent(&r.module);
}

/// A synthesized pin must not reuse a label a DECLARED port in the same
/// module already carries. Here the chip declares an output `score` and its
/// body reads the outer `score` var, so the crossing derives that same
/// identifier — two ports named `score` on one chip until the taken-name
/// set counts declared ports too.
#[test]
fn a_pin_never_reuses_a_declared_ports_label() {
    let r = lowered(
        "var score: int = 0\nin go: exec\nchip C(t: exec) -> (score: int) {\n  var h: int = 0\n  on t { h = h + score }\n  out score = h\n}\nlet c = C(go)\nout o = c\n",
    );
    let chip = r.module.chips.values().next().expect("one chip module");

    let is_port = |n: &&crate::ir::Node| {
        matches!(
            n.kind,
            crate::ir::NodeKind::Input | crate::ir::NodeKind::Output
        )
    };
    let declared: Vec<String> = chip
        .nodes
        .values()
        .filter(is_port)
        .filter(|n| is_declared_port(n))
        .filter_map(|n| port_label(n))
        .collect();
    assert!(
        declared.iter().any(|l| l == "score"),
        "fixture must declare a port named score; got {declared:?}"
    );

    let pins: Vec<String> = chip
        .nodes
        .values()
        .filter(is_port)
        .filter(|n| is_boundary_pin(n))
        .filter_map(|n| port_label(n))
        .collect();
    assert_eq!(pins.len(), 1, "fixture expects one synthesized pin");
    assert!(
        !declared.contains(&pins[0]),
        "pin {:?} collides with a declared port; declared {declared:?}",
        pins[0]
    );
}
