use super::*;
use crate::ir::gate_class as gc;

#[test]
fn opaque_lowers_to_rerouter_and_blocks_literal_inline() {
    let r = compile_ok("let a = Opaque(2)\nout y = a + 3");
    let rerouters: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::REROUTER)
        .collect();
    assert_eq!(rerouters.len(), 1, "Opaque must emit exactly one rerouter");
    let rer = rerouters[0].id;
    // A literal drives the rerouter input; the rerouter output drives the add.
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == rer),
        "rerouter input must be wired"
    );
    assert!(
        r.module.wires.iter().any(|w| w.source.node_id == rer),
        "rerouter output must be wired (not inlined away)"
    );
    // The MathAdd gate is real: Opaque blocked constant folding of 2 + 3.
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class.contains("MathAdd")),
        "add must remain a real gate"
    );
}

#[test]
fn opaque_mixed_variant_compare_typechecks() {
    let r = compile_ok("out y = Opaque(1) == Opaque(\"1\")");
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::COMPARE_EQUAL),
        "mixed-variant == through Opaque must lower to CompareEqual"
    );
}

#[test]
fn opaque_not_pruned_when_consumed() {
    // Dead-gate pruning must not treat the rerouter as a pass-through to elide.
    let r = compile_ok("in t: exec\nvar v: int = 0\non t { v = Opaque(7) }");
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::REROUTER)
    );
}

/// Error-fallback `Any` (the result of a void array method like
/// `arr.clear()`) must NOT satisfy operator overloads the way a real
/// `Opaque(...)` probe does: the void operand is WS072.
#[test]
fn any_fallback_still_rejected_by_operators() {
    let r = compile("var arr: int[]\nin t: exec\nvar y: int = 0\non t { y = arr.clear() + 3 }");
    assert!(
        has_error(&r, "WS072"),
        "void-result + int must be WS072, got: {:?}",
        r.diagnostics
    );
}

#[test]
fn opaque_emits_to_world() {
    // End-to-end: a standalone rerouter node must survive layout + emit.
    let src = "let a = Opaque(2)\nout y = a + 3";
    let r = compile_ok(src);
    let lr = crate::layout::layout(&r.module);
    let opts = crate::EmitOptions::default();
    crate::build_world(&r.module, &Default::default(), &lr, &opts)
        .expect("emit must handle rerouter nodes");
}

#[test]
fn opaque_scalar_inputs_materialize_as_constant_gates() {
    // Silent-miscompile: a scalar literal wired to a dataless port
    // (the rerouter's RER_Input) was inlined into nonexistent component data
    // at emit and silently dropped in-game. It must materialize as a REAL
    // pure gate carrying the value as unwired-input data defaults (MathAdd
    // for numbers, LogicalOR for bools, String_Concatenate for strings) —
    // NOT a Variable, whose Value output stays null until an exec write.
    for (src, feeder) in [
        ("out y = Opaque(2) == Opaque(3)", "MathAdd"),
        ("out y = Opaque(true) == Opaque(false)", "LogicalOR"),
        ("out y = Opaque(0.5) == Opaque(0.25)", "MathAdd"),
        (
            "out y = Opaque(\"a\") == Opaque(\"A\")",
            "String_Concatenate",
        ),
    ] {
        let r = compile_ok(src);
        let rerouter_ids: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::REROUTER)
            .map(|n| n.id)
            .collect();
        assert_eq!(rerouter_ids.len(), 2, "{src}: two rerouters");
        for rid in rerouter_ids {
            let feeder_id = r
                .module
                .wires
                .iter()
                .find(|w| w.target.node_id == rid)
                .map(|w| w.source.node_id)
                .expect("rerouter input wired");
            let feeder_class = r.module.nodes[&feeder_id].gate_class;
            assert!(
                feeder_class.contains(feeder),
                "{src}: rerouter fed by {feeder} constant gate, got {feeder_class}"
            );
            assert!(
                feeder_class != gc::VAR,
                "{src}: must not use a Variable (null until written in-game)"
            );
        }
    }
}
