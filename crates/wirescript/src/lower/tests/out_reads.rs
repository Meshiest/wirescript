use super::*;

#[test]
fn reading_an_output_port_wires_from_its_rerouter() {
    // A port read is the same wire a namespaced `L.count` read already emits:
    // the output rerouter's `RER_Output` feeding the consumer.
    let src = "in a: int\n\
               out y: int = a + 1\n\
               out z: int = y * 2";
    let r = compile(src);
    assert_no_errors(&r);
    let mul = find_gate(&r, "BrickComponentType_WireGraph_Expr_MathMultiply");
    assert!(
        r.module.wires.iter().any(|w| {
            w.target.node_id == mul
                && w.source.port == crate::ir::port_registry::WirePort::RerOutput
        }),
        "`y` must be read from its output rerouter; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn a_same_named_var_still_wins_over_the_output_port() {
    // The port must not capture its own initializer: a same-named var wins the
    // first resolution tier, so the value comes from the var.
    let src = "var count: int = 0\n\
               in go: bool\n\
               out count: int = count";
    let r = compile(src);
    assert_no_errors(&r);
    let var = find_gate(&r, "BrickComponentType_WireGraphPseudo_Var");
    let port = find_gate(&r, "BrickComponentType_Internal_MicrochipOutput");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == var && w.target.node_id == port),
        "the port's value must come from the var, not from itself; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn a_self_referential_output_is_reported_as_a_barrier_free_cycle() {
    // An unbacked port read inside its own value closes `out -> add -> out`,
    // which is genuinely combinational. WS005 already reports barrier-free
    // cycles and names Buffer and Queue as the fix, so no new diagnostic
    // exists for this. `analyze_cycles` runs from `compile.rs`, not from
    // `lower()`, so it must be invoked directly here rather than read off
    // `r.diagnostics`.
    let r = compile("in a: int\nout x: int = x + 1");
    let cyc = crate::analyze::analyze_cycles(&r.module);
    assert!(
        cyc.diagnostics.iter().any(|d| d.code == "WS005"),
        "expected WS005 for a self-referential port; diagnostics: {:?}",
        cyc.diagnostics
    );
}

#[test]
fn a_self_referential_buffer_still_compiles() {
    // `BufferTicks` is in `BARRIER_CLASSES`, so this loop is legal and must
    // stay legal. It is the escape hatch WS005 points at.
    let r = compile("in go: bool\nbuffer prev: bool = !prev\nout o: bool = prev");
    let cyc = crate::analyze::analyze_cycles(&r.module);
    assert!(
        !cyc.diagnostics.iter().any(|d| d.code == "WS005"),
        "a buffered loop must not report WS005; diagnostics: {:?}",
        cyc.diagnostics
    );
}

#[test]
fn a_top_level_handler_may_declare_its_own_output_port() {
    // A single unconditional site: two nodes, one wire, no backing var.
    let src = "on Clock(interval = 0.2) {\n\
               @top out flash: bool = Toggle()\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_Internal_MicrochipOutput"),
        1,
        "exactly one boundary port; nodes: {:?}",
        r.module.nodes
    );
    let toggle = find_gate(&r, "BrickComponentType_WireGraph_Exec_Toggle");
    let port = find_gate(&r, "BrickComponentType_Internal_MicrochipOutput");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == toggle && w.target.node_id == port),
        "Toggle must drive the hoisted port; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn a_handler_declared_port_keeps_its_side_annotation() {
    let r = compile("on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}");
    assert_no_errors(&r);
    let side = r
        .module
        .nodes
        .values()
        .find(|n| n.kind == crate::ir::NodeKind::Output)
        .and_then(|n| n.properties.get(&*crate::intern::sym::REROUTE_SIDE))
        .cloned();
    assert_eq!(side, Some(crate::ir::Literal::String("top".into())));
}

#[test]
fn a_handler_declared_port_binds_to_an_existing_top_level_out() {
    // Pass 1a runs after the whole pass-1 loop, so the explicit declaration is
    // found no matter where in the file it sits. One port, not two.
    let src = "on Clock(interval = 0.2) {\n\
               out flash: bool = Toggle()\n\
               }\n\
               @top out flash: bool";
    let r = compile(src);
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_Internal_MicrochipOutput"),
        1,
        "a handler-declared name must bind to the explicit port, not mint a \
         second one; nodes: {:?}",
        r.module.nodes
    );
}

#[test]
fn an_out_binding_with_no_port_reports_ws073() {
    // A handler nested in a chip is deliberately not hoisted, so an `out` in
    // its body has no port to bind to. That must be reported rather than
    // dropped, since a dropped binding takes the port, its driver and any
    // diagnostic with it.
    let src = "chip {\n\
               on Clock(interval = 0.2) {\n\
               out flash: bool = Toggle()\n\
               }\n\
               }";
    let r = compile(src);
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS073"),
        "expected WS073 for an unhoistable `out`; diagnostics: {:?}",
        r.diagnostics
    );
}

#[test]
fn a_chip_signature_output_written_from_a_handler_is_not_ws073() {
    // The signature declared the port, so writing it from a handler inside the
    // chip's own body reaches a port that exists and is not an unhoistable
    // `out`.
    let src = "chip Foo() -> (r: int) {\n\
               on Clock(interval = 0.2) {\n\
               out r = Cycle(count = 4)\n\
               }\n\
               }\n\
               out v: int = Foo()";
    let r = compile(src);
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS073"),
        "a declared signature output must not report WS073; diagnostics: {:?}",
        r.diagnostics
    );
}

#[test]
fn a_captured_event_bodys_out_binding_is_hoisted() {
    // `let e = on Trigger { ... }` (a captured event) carries a handler body
    // in `TopDecl::Event::captured_body`, lowered handler-style by
    // `lower_event_decl`. Its `out` bindings are top-level handler bindings
    // and must be hoisted by pass 1a exactly like a plain `on` handler, not
    // reported as WS073.
    let src = "let e = on Clock(interval = 0.2) {\n\
               out flash: bool = Toggle()\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_Internal_MicrochipOutput"),
        1,
        "exactly one boundary port; nodes: {:?}",
        r.module.nodes
    );
    let toggle = find_gate(&r, "BrickComponentType_WireGraph_Exec_Toggle");
    let port = find_gate(&r, "BrickComponentType_Internal_MicrochipOutput");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == toggle && w.target.node_id == port),
        "Toggle must drive the hoisted port; wires: {:?}",
        r.module.wires
    );
}

#[test]
fn two_handler_sites_share_one_backing_var() {
    // Two direct wires into one rerouter is a load-time fan-in. `emit` already
    // solves this with a backing PseudoVar and a Var_Set per site; a
    // handler-body `out` is the same kind of value driver.
    let src = "in go: exec\n\
               on Clock(interval = 0.2) {\n\
               out n: int = 1\n\
               }\n\
               on go {\n\
               out n: int = 2\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_Var"),
        1,
        "one backing var; nodes: {:?}",
        r.module.nodes
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        2,
        "one Var_Set per site; nodes: {:?}",
        r.module.nodes
    );
}

#[test]
fn a_conditional_handler_site_gets_a_backing_var() {
    // A single site inside a branch still needs the var, so the write is gated
    // by the branch exec instead of driving the rerouter continuously.
    let src = "in a: int\n\
               on Clock(interval = 0.2) {\n\
               if a > 1 {\n\
               out n: int = a\n\
               }\n\
               }";
    let r = compile(src);
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_Var"),
        1,
        "a branched site must be backed; nodes: {:?}",
        r.module.nodes
    );
}

#[test]
fn a_single_unconditional_site_stays_a_direct_wire() {
    // The `Toggle()` case must not grow a var. Two nodes, one wire.
    let r = compile("on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}");
    assert_no_errors(&r);
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraphPseudo_Var"),
        0,
        "a single unconditional site needs no backing var; nodes: {:?}",
        r.module.nodes
    );
}

#[test]
fn a_backed_output_that_reads_itself_is_not_a_cycle() {
    // A `Var_Set` takes the var's `VarRef` as an INPUT and never wires back
    // into the var node, so no strongly connected component forms. The circuit
    // is exec gated and works, and WS005 correctly stays quiet.
    let src = "in go: exec\n\
               on Clock(interval = 0.2) {\n\
               out n: int = n + 1\n\
               }\n\
               on go {\n\
               out n: int = 0\n\
               }";
    let r = compile(src);
    let cyc = crate::analyze::analyze_cycles(&r.module);
    assert!(
        !cyc.diagnostics.iter().any(|d| d.code == "WS005"),
        "a backed self-read is exec gated, not combinational; diagnostics: {:?}",
        cyc.diagnostics
    );
}
