use super::*;
use crate::ir::gate_class as gc;

#[test]
fn reading_an_output_port_wires_from_its_rerouter() {
    // A port read is the same wire a namespaced `L.count` read already emits:
    // the output rerouter's `RER_Output` feeding the consumer.
    let src = "in a: int\n\
               out y: int = a + 1\n\
               out z: int = y * 2";
    let r = compile_ok(src);
    let mul = find_gate(&r, "BrickComponentType_Expr_MathMultiply");
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
    let r = compile_ok(src);
    let var = find_gate(&r, gc::VAR);
    let port = find_gate(&r, gc::MICROCHIP_OUTPUT);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == var && w.target.node_id == port),
        "the port's value must come from the var, not from itself; wires: {:?}",
        r.module.wires
    );
}

/// `analyze_cycles` runs from `compile.rs`, not from `lower()`, so WS005 is
/// read from it directly rather than from `r.diagnostics`.
#[test]
fn ws005_reports_only_barrier_free_output_cycles() {
    let rows: &[(&str, &str, bool)] = &[
        // An unbacked port read inside its own value closes `out -> add -> out`,
        // which is genuinely combinational; WS005 names Buffer and Queue as the fix.
        (
            "a self-referential output is a barrier-free cycle",
            "in a: int\nout x: int = x + 1",
            true,
        ),
        // `BufferTicks` is in `BARRIER_CLASSES`: the escape hatch WS005 points at.
        (
            "a self-referential buffer still compiles",
            "in go: bool\nbuffer prev: bool = !prev\nout o: bool = prev",
            false,
        ),
        // A `Var_Set` takes the var's `VarRef` as an INPUT and never wires back
        // into the var node, so no strongly connected component forms.
        (
            "a backed output that reads itself is exec gated, not a cycle",
            "in go: exec\non Clock(interval = 0.2) {\nout n: int = n + 1\n}\non go {\nout n: int = 0\n}",
            false,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let cyc = crate::analyze::analyze_cycles(&compile(src).module);
        let got = cyc.diagnostics.iter().any(|d| d.code == "WS005");
        if got != want {
            failures.push(format!(
                "{label}: WS005 {got}, want {want}: {:?}\n{src}",
                crate::test_support::codes(&cyc.diagnostics)
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A single unconditional handler-body `out` is one boundary port driven
/// straight by its value, with no backing var, and keeps its side annotation.
#[test]
fn a_top_level_handler_may_declare_its_own_output_port() {
    let r = compile_ok("on Clock(interval = 0.2) {\n@top out flash: bool = Toggle()\n}");
    assert_eq!(
        count_class(&r.module, gc::MICROCHIP_OUTPUT),
        1,
        "exactly one boundary port; nodes: {:?}",
        r.module.nodes
    );
    assert_eq!(
        count_class(&r.module, gc::VAR),
        0,
        "a single unconditional site needs no backing var; nodes: {:?}",
        r.module.nodes
    );
    let toggle = find_gate(&r, gc::EXEC_TOGGLE);
    let port = find_gate(&r, gc::MICROCHIP_OUTPUT);
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == toggle && w.target.node_id == port),
        "Toggle must drive the hoisted port; wires: {:?}",
        r.module.wires
    );
    let side = r.module.nodes[&port]
        .properties
        .get(&*crate::intern::sym::REROUTE_SIDE)
        .cloned();
    assert_eq!(side, Some(crate::ir::Literal::String("top".into())));
}

/// Handler-body `out` bindings hoist to one port per name; a name written
/// from more than one site, or under a branch, is backed by a var with one
/// Var_Set per site. Exact counts over the whole module tree.
#[test]
fn handler_out_bindings_hoist_to_one_port_per_name() {
    let rows: &[(&str, &str, &[(&str, usize)])] = &[
        // `let e = on Trigger { ... }` carries its body in `TopDecl::Event::captured_body`,
        // lowered handler-style by `lower_event_decl`; its `out`s hoist like a plain handler's.
        (
            "a captured event body's out binding is hoisted",
            "let e = on Clock(interval = 0.2) {\nout flash: bool = Toggle()\n}",
            &[(gc::MICROCHIP_OUTPUT, 1), (gc::VAR, 0)],
        ),
        // Pass 1a runs after the whole pass-1 loop, so the explicit declaration
        // is found wherever in the file it sits.
        (
            "a handler-declared name binds to an existing top-level out, not a second port",
            "on Clock(interval = 0.2) {\nout flash: bool = Toggle()\n}\n@top out flash: bool",
            &[(gc::MICROCHIP_OUTPUT, 1)],
        ),
        // Two direct wires into one rerouter is a load-time fan-in; like `emit`,
        // each site writes a backing var instead.
        (
            "two handler sites share one backing var",
            "in go: exec\non Clock(interval = 0.2) {\nout n: int = 1\n}\non go {\nout n: int = 2\n}",
            &[(gc::VAR, 1), (gc::VAR_SET, 2)],
        ),
        // The write is gated by the branch exec instead of driving the rerouter continuously.
        (
            "a single site inside a branch is backed",
            "in a: int\non Clock(interval = 0.2) {\nif a > 1 {\nout n: int = a\n}\n}",
            &[(gc::VAR, 1)],
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}\n{src}"));
            continue;
        }
        for &(class, n) in want {
            let got = count_class(&r.module, class);
            if got != n {
                failures.push(format!("{label}: {got} x {class}, want {n}\n{src}"));
            }
        }
        if label.starts_with("a captured event") {
            let toggle = find_gate(&r, gc::EXEC_TOGGLE);
            let port = find_gate(&r, gc::MICROCHIP_OUTPUT);
            if !r
                .module
                .wires
                .iter()
                .any(|w| w.source.node_id == toggle && w.target.node_id == port)
            {
                failures.push(format!(
                    "{label}: Toggle does not drive the hoisted port\n{src}"
                ));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// WS073: an `out` in a handler with no port to bind to is reported, never
/// dropped (a dropped binding takes the port, its driver and any diagnostic
/// with it).
#[test]
fn an_out_binding_with_no_port_reports_ws073() {
    let rows: &[(&str, &str, bool)] = &[
        // A handler nested in a chip is deliberately not hoisted.
        (
            "a handler inside an anonymous chip",
            "chip {\non Clock(interval = 0.2) {\nout flash: bool = Toggle()\n}\n}",
            true,
        ),
        // The signature declared the port, so the handler's write reaches a port that exists.
        (
            "a chip signature output written from a handler in its body",
            "chip Foo() -> (r: int) {\non Clock(interval = 0.2) {\nout r = Cycle(count = 4)\n}\n}\nout v: int = Foo()",
            false,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let r = compile(src);
        let got = r.diagnostics.iter().any(|d| d.code == "WS073");
        if got != want {
            failures.push(format!(
                "{label}: WS073 {got}, want {want}: {:?}\n{src}",
                crate::test_support::codes(&r.diagnostics)
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
