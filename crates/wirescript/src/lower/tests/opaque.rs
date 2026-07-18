use super::*;
use crate::ir::gate_class as gc;

fn no_errors(r: &LowerResult) {
    assert!(
        r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
}

#[test]
fn opaque_lowers_to_rerouter_and_blocks_literal_inline() {
    let r = compile("let a = Opaque(2)\nout y = a + 3");
    no_errors(&r);
    let rerouters: Vec<_> = r.module.nodes.values()
        .filter(|n| n.gate_class == gc::REROUTER)
        .collect();
    assert_eq!(rerouters.len(), 1, "Opaque must emit exactly one rerouter");
    let rer = rerouters[0].id;
    // A literal drives the rerouter input; the rerouter output drives the add.
    assert!(r.module.wires.iter().any(|w| w.target.node_id == rer),
        "rerouter input must be wired");
    assert!(r.module.wires.iter().any(|w| w.source.node_id == rer),
        "rerouter output must be wired (not inlined away)");
    // The MathAdd gate is real: Opaque blocked constant folding of 2 + 3.
    assert!(r.module.nodes.values().any(|n| n.gate_class.contains("MathAdd")),
        "add must remain a real gate");
}

#[test]
fn opaque_mixed_variant_compare_typechecks() {
    let r = compile("out y = Opaque(1) == Opaque(\"1\")");
    no_errors(&r);
    assert!(r.module.nodes.values().any(|n| n.gate_class == gc::COMPARE_EQUAL),
        "mixed-variant == through Opaque must lower to CompareEqual");
}

#[test]
fn opaque_not_pruned_when_consumed() {
    // Dead-gate pruning must not treat the rerouter as a pass-through to elide.
    let r = compile("in t: exec\nvar v: int = 0\non t { v = Opaque(7) }");
    no_errors(&r);
    assert!(r.module.nodes.values().any(|n| n.gate_class == gc::REROUTER));
}

#[test]
fn any_fallback_still_rejected_by_operators() {
    // Regression for the reviewed Critical: error-fallback `Any` (e.g. the
    // result of a void array method like `arr.clear()`) must NOT satisfy
    // operator overloads the way a real `Opaque(...)` probe does.
    //
    // NB: `compile()`'s `LowerResult.diagnostics` only carries forward
    // Warning-severity typecheck diagnostics (by design, to isolate
    // lowering-pass diagnostics) — a WS004 here is Error-severity, so we
    // typecheck directly and inspect `TypeCheckResult.diagnostics` instead
    // of routing through `compile()`.
    let src = "array arr: int[]\nin t: exec\nvar y: int = 0\non t { y = arr.clear() + 3 }";
    let parsed = crate::parser::parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = crate::typecheck::typecheck(&parsed.ast, "test");
    assert!(
        tc.diagnostics
            .iter()
            .any(|d| d.severity == crate::diagnostic::Severity::Error),
        "void-result + int must error, got: {:?}",
        tc.diagnostics
    );
}

#[test]
fn opaque_emits_to_world() {
    // End-to-end: a standalone rerouter node must survive layout + emit.
    let src = "let a = Opaque(2)\nout y = a + 3";
    let r = compile(src);
    no_errors(&r);
    let lr = crate::layout::layout(&r.module);
    let opts = crate::EmitOptions::default();
    let cache = std::sync::Arc::new(crate::template_cache::TemplateCache::new());
    crate::build_world(&r.module, &lr, &opts, &cache).expect("emit must handle rerouter nodes");
}

#[test]
fn nofold_marks_every_node_in_subtree() {
    let r = compile("@nofold let a = 1 + 2\nout y = a");
    no_errors(&r);
    let marked = r.module.nodes.values()
        .filter(|n| n.properties.contains_key(&*crate::intern::sym::NO_FOLD))
        .count();
    assert!(marked >= 1, "nodes lowered under @nofold must carry _nofold");
}

#[test]
fn nofold_does_not_leak_to_siblings() {
    let r = compile("@nofold let a = 1 + 2\nlet b = 3 + 4\nout y = a + b");
    no_errors(&r);
    // At least one unmarked node must exist (b's adder or the literals).
    assert!(r.module.nodes.values()
        .any(|n| !n.properties.contains_key(&*crate::intern::sym::NO_FOLD)));
}

#[test]
fn nofold_allowed_on_handler_and_inside_chip() {
    let r = compile("in t: exec\nchip C() -> (z: int) {\n  @nofold let inner = 1 + 1\n  out z = inner\n}\nvar v: int = 0\n@nofold on t { v = 1 }\nlet c = C()\nout y = c");
    no_errors(&r);
}

#[test]
fn nofold_prop_never_reaches_brick_data() {
    let r = compile("@nofold let a = 1 + 2\nout y = a");
    no_errors(&r);
    let lr = crate::layout::layout(&r.module);
    let opts = crate::EmitOptions::default();
    let cache = std::sync::Arc::new(crate::template_cache::TemplateCache::new());
    // If emit tried to write `_nofold` as a component field it would error
    // (unknown schema field) or corrupt data; building cleanly is the gate.
    crate::build_world(&r.module, &lr, &opts, &cache).expect("emit must skip _nofold");
}

#[test]
fn nofold_noop_sites_warn() {
    // `@nofold` that parses but has no effect must WARN, not silently drop.
    // Parse directly: the shared compile() helper rejects any parse diagnostic.
    let warn_count = |src: &str| {
        let parsed = crate::parser::parse(src, "test");
        assert!(
            parsed
                .diagnostics
                .iter()
                .all(|d| d.severity != crate::diagnostic::Severity::Error),
            "unexpected errors for {src:?}: {:?}",
            parsed.diagnostics
        );
        parsed
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == crate::diagnostic::Severity::Warning
                    && d.message.contains("@nofold has no effect")
            })
            .count()
    };
    assert_eq!(warn_count("@nofold chip { let x = 1 + 1 }"), 1, "anon chip");
    assert_eq!(warn_count("in t: exec
@nofold chip on t { }"), 1, "anon chip-on");
    assert_eq!(warn_count("@nofold in t: exec
on t { }"), 1, "in decl");
    // Effective sites must NOT warn.
    assert_eq!(warn_count("@nofold let a = 1 + 2
out y = a"), 0, "let");
    assert_eq!(
        warn_count("@nofold chip C() -> (z: int) { out z = 1 }
let c = C()
out y = c"),
        0,
        "named chip"
    );
}

#[test]
fn nofold_captured_event_marks_nodes() {
    // Regression: `@nofold let c = on Trigger { ... }` parses to
    // `TopDecl::Event` (a captured event), not `TopDecl::Let` — the bare-
    // `@nofold` dispatch (both the parser and the lowering wrap) must still
    // tag it instead of silently dropping the annotation. `compile()` itself
    // asserts parse diagnostics are empty, so a clean compile also proves no
    // "@nofold has no effect" warning fired.
    let r = compile("@nofold let c = on RoundStart { PrintToConsole(\"hi\") }");
    no_errors(&r);
    let marked = r.module.nodes.values()
        .filter(|n| n.properties.contains_key(&*crate::intern::sym::NO_FOLD))
        .count();
    assert!(
        marked >= 1,
        "nodes lowered under an @nofold captured-event must carry _nofold"
    );
}

#[test]
fn nofold_await_binding_marks_nodes() {
    // Regression: `@nofold let x = await sig` parses to `TopDecl::Await`
    // (an await binding), not `TopDecl::Let` — same silent-drop risk as the
    // captured-event form above, but reached through the statement-level
    // dispatch (inside a handler body) instead of the top-level one.
    let src = "in run: exec\n\
               on run {\n\
                 let sig: exec\n\
                 emit sig = 7\n\
                 @nofold let x = await sig\n\
                 PrintToConsole(\"${x}\")\n\
               }";
    let r = compile(src);
    no_errors(&r);
    let marked = r.module.nodes.values()
        .filter(|n| n.properties.contains_key(&*crate::intern::sym::NO_FOLD))
        .count();
    assert!(
        marked >= 1,
        "nodes lowered under an @nofold await-binding must carry _nofold"
    );
}

#[test]
fn nofold_chip_boundary_ports_marked() {
    // Regression: `ModuleBuilder::add_input`/`add_output` bypass
    // `LowerCtx::add_gate`/`add_event`, so a `@nofold` chip's own
    // MicrochipInput/MicrochipOutput boundary (rerouter) nodes never got
    // `_nofold` even though every gate in the chip's body did.
    let r = compile("@nofold chip C(a: int) -> (z: int) { out z = a }\nlet c = C(1)\nout y = c");
    no_errors(&r);
    assert_eq!(r.module.chips.len(), 1, "expected exactly one chip child module");
    let child = r.module.chips.values().next().unwrap();
    let boundary: Vec<_> = child.inputs.iter().chain(child.outputs.iter()).collect();
    assert!(!boundary.is_empty(), "chip must have boundary nodes to check");
    for id in boundary {
        let node = &child.nodes[id];
        assert!(
            node.properties.contains_key(&*crate::intern::sym::NO_FOLD),
            "boundary node {:?} ({}) of an @nofold chip must carry _nofold",
            id,
            node.gate_class
        );
    }
}

#[test]
fn unannotated_chip_boundary_ports_not_marked() {
    // Counterpart to `nofold_chip_boundary_ports_marked`: an ordinary chip's
    // boundary nodes must NOT pick up `_nofold` from nowhere.
    let r = compile("chip C(a: int) -> (z: int) { out z = a }\nlet c = C(1)\nout y = c");
    no_errors(&r);
    assert_eq!(r.module.chips.len(), 1, "expected exactly one chip child module");
    let child = r.module.chips.values().next().unwrap();
    let boundary: Vec<_> = child.inputs.iter().chain(child.outputs.iter()).collect();
    assert!(!boundary.is_empty(), "chip must have boundary nodes to check");
    for id in boundary {
        let node = &child.nodes[id];
        assert!(
            !node.properties.contains_key(&*crate::intern::sym::NO_FOLD),
            "boundary node {:?} ({}) of an unannotated chip must NOT carry _nofold",
            id,
            node.gate_class
        );
    }
}

#[test]
fn module_level_nofold_marks_everything() {
    // `@nofold` + blank line at the top of the file = whole-module scope
    // (same rule as module doc comments).
    let r = compile("@nofold

var x: int = 0
let b = 1 + 2
out y = b");
    no_errors(&r);
    let unmarked: Vec<_> = r.module.nodes.values()
        .filter(|n| !n.properties.contains_key(&*crate::intern::sym::NO_FOLD))
        .map(|n| n.gate_class)
        .collect();
    assert!(unmarked.is_empty(), "module-level @nofold must mark every node, unmarked: {unmarked:?}");
}

#[test]
fn attached_nofold_stays_decl_scoped() {
    // No blank line: binds to the next declaration only.
    let r = compile("@nofold
let a = 1 + 2
let b = 3 + 4
out y = a + b");
    no_errors(&r);
    assert!(r.module.nodes.values()
        .any(|n| !n.properties.contains_key(&*crate::intern::sym::NO_FOLD)),
        "decl-scoped @nofold must not mark sibling decls");
    assert!(r.module.nodes.values()
        .any(|n| n.properties.contains_key(&*crate::intern::sym::NO_FOLD)),
        "the annotated decl's nodes must still be marked");
}

#[test]
fn nofold_var_marks_predeclared_gate() {
    // Var gates are created in pass 1 (predeclare), not lower_decl — the
    // decl's @nofold must still reach them.
    let r = compile("@nofold var x: int = 0
in t: exec
on t { x = 1 }
out y: int = x");
    no_errors(&r);
    let var_marked = r.module.nodes.values().any(|n| {
        n.gate_class.contains("Pseudo_Var")
            && n.properties.contains_key(&*crate::intern::sym::NO_FOLD)
    });
    assert!(var_marked, "@nofold var's Variable gate must carry _nofold");
}

#[test]
fn nofold_var_nested_in_chip_and_mod_marks_gate() {
    // Re-review Critical: var gates predeclared inside chip/mod/anon-chip
    // bodies must honor a var-level @nofold (all pre_declare_var sites wrap).
    let marked_var = |r: &LowerResult| {
        fn walk(m: &crate::ir::Module) -> bool {
            m.nodes.values().any(|n| {
                n.gate_class.contains("Pseudo_Var")
                    && n.properties.contains_key(&*crate::intern::sym::NO_FOLD)
            }) || m.chips.values().any(walk)
        }
        walk(&r.module)
    };
    let r = compile(
        "in t: exec
chip C() -> (z: int) {
  @nofold var x: int = 0
  out z = 1
}
let c = C()
out y = c",
    );
    no_errors(&r);
    assert!(marked_var(&r), "@nofold var inside a named chip body must stamp its Variable gate");

    let r = compile("in t: exec
chip {
  @nofold var w: int = 0
  on t { w = 1 }
}");
    no_errors(&r);
    assert!(marked_var(&r), "@nofold var inside an anonymous chip must stamp its Variable gate");
}

#[test]
fn nofold_out_marks_port_node() {
    // Final-review Minor: `@nofold out` must stamp the port node created in
    // pass-1 predeclare, not just the value gates.
    let r = compile("var x: int = 0
in t: exec
on t { x = 1 }
@nofold out y = x + 1");
    no_errors(&r);
    let port_marked = r.module.nodes.values().any(|n| {
        n.gate_class.contains("MicrochipOutput")
            && n.properties.contains_key(&*crate::intern::sym::NO_FOLD)
    });
    assert!(port_marked, "@nofold out's port node must carry _nofold");
}

#[test]
fn opaque_scalar_inputs_materialize_as_constant_gates() {
    // Silent-miscompile family #9: a scalar literal wired to a dataless port
    // (the rerouter's RER_Input) was inlined into nonexistent component data
    // at emit and silently dropped in-game. It must materialize as a REAL
    // pure gate carrying the value as unwired-input data defaults (MathAdd
    // for numbers, LogicalOR for bools, String_Concatenate for strings) —
    // NOT a Variable, whose Value output stays null until an exec write.
    for (src, feeder) in [
        ("out y = Opaque(2) == Opaque(3)", "MathAdd"),
        ("out y = Opaque(true) == Opaque(false)", "LogicalOR"),
        ("out y = Opaque(0.5) == Opaque(0.25)", "MathAdd"),
        ("out y = Opaque(\"a\") == Opaque(\"A\")", "String_Concatenate"),
    ] {
        let r = compile(src);
        no_errors(&r);
        let rerouter_ids: Vec<_> = r.module.nodes.values()
            .filter(|n| n.gate_class == gc::REROUTER)
            .map(|n| n.id)
            .collect();
        assert_eq!(rerouter_ids.len(), 2, "{src}: two rerouters");
        for rid in rerouter_ids {
            let feeder_id = r.module.wires.iter()
                .find(|w| w.target.node_id == rid)
                .map(|w| w.source.node_id)
                .expect("rerouter input wired");
            let feeder_class = r.module.nodes[&feeder_id].gate_class;
            assert!(
                feeder_class.contains(feeder),
                "{src}: rerouter fed by {feeder} constant gate, got {feeder_class}"
            );
            assert!(
                !feeder_class.contains("Pseudo_Var"),
                "{src}: must not use a Variable (null until written in-game)"
            );
        }
    }
}
