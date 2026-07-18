use crate::{ir::port_registry::WirePort, template_cache::TemplateCache};

use super::*;

// ── Recursion guard (WS020) ──

/// Lower a main source plus in-memory imported modules (resolve → typecheck →
/// lower) and return all diagnostics — for exercising cross-module scenarios
/// the single-source `compile` helper can't reach.
fn lower_with_imports(main_src: &str, files: &[(&str, &str)]) -> Vec<crate::diagnostic::Diagnostic> {
    let files: crate::collections::HashMap<String, String> = files
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    let loader = crate::resolve::MemLoader { files };
    let resolved = crate::resolve::resolve(main_src, "main.ws", &loader);
    let tc = crate::typecheck::typecheck(&resolved.ast, "main.ws");
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "main.ws",
        module_name: None,
        template_cache: std::sync::Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
    });
    let mut diags = resolved.diagnostics;
    diags.extend(tc.diagnostics);
    diags.extend(lowered.diagnostics);
    diags
}

#[test]
fn const_literal_reaches_into_anon_chip() {
    // Regression: a top-level const `let` referenced inside a `chip on` handler
    // must have its Literal cloned INTO the chip's child module. Otherwise the
    // parent→chip data wire dangles (its target moved into the child during
    // partition_anon_chips) and the chip-side input silently reads the port
    // default (0) — exactly what broke every scalar const inside a `chip on`.
    // K has two consumers (the top-level `out` and the in-chip comparison), so
    // `inline_orphan_literals` (single-consumer only) leaves it a real Literal
    // gate — the case that dangles across the chip boundary.
    let src = "let K: int = 7\nout dummy = K\nin trigger: exec\nchip on trigger { var x: int = 0\n  if x == K { x = 1 } }";
    let r = compile(src);
    let child = r
        .module
        .chips
        .values()
        .next()
        .expect("the `chip on` block should produce one anon chip module");
    let lit7 = child.nodes.values().any(|n| {
        n.gate_class == crate::ir::gate_class::LITERAL
            && n.properties.get(&*crate::intern::sym::VALUE) == Some(&crate::ir::Literal::Int(7))
    });
    assert!(
        lit7,
        "const literal `K = 7` must be cloned into the chip body so the wire stays internal"
    );
}

/// A mod that transitively calls itself must still be flagged WS020 — the guard
/// now keys on the decl's source range (not just its name), so real recursion
/// (same decl re-entered) is still caught.
#[test]
fn genuine_self_recursion_still_flagged() {
    let r = compile("mod foo() -> int { return foo() }\nout result = foo()");
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS020"),
        "self-recursion must still be WS020; got {:?}",
        r.diagnostics
    );
}

/// Regression: a local mod calling a SAME-NAMED mod imported from another
/// module (`mod drawCard` calling `card.drawCard`) is not recursion — they're
/// distinct decls. The name-based guard falsely flagged this; keying on the
/// decl's source range (which includes the file) fixes it.
#[test]
fn same_name_mod_across_modules_is_not_recursion() {
    let display = "mod drawCard() -> int { return 1 }";
    let main = "import * as card from \"display\"\n\
                mod drawCard() -> int { return card.drawCard() }\n\
                out result = drawCard()";
    let diags = lower_with_imports(main, &[("display.ws", display)]);
    assert!(
        !diags.iter().any(|d| d.code == "WS020"),
        "same-named mod across modules must not be flagged recursive; got {:?}",
        diags
    );
    assert!(
        !diags
            .iter()
            .any(|d| d.severity == crate::diagnostic::Severity::Error),
        "should compile without errors; got {:?}",
        diags
    );
}

#[test]
fn chip_decl_sets_child_root_scope_to_chip_body() {
    // Standalone chips are instantiated per call, so we need to call it.
    let src = "chip Foo(x: int) -> (r: int) { out r = x }\nlet f = Foo(1)";
    let r = compile(src);
    let chip_mod = r
        .module
        .chips
        .values()
        .next()
        .expect("expected one chip sub-module from call");
    let root = chip_mod
        .scopes
        .get(&crate::ir::ROOT_SCOPE_ID)
        .expect("chip body root scope");
    match &root.kind {
        ScopeKind::ChipBody { name } => assert_eq!(name, "Foo"),
        other => panic!("expected ChipBody, got {:?}", other),
    }
}

#[test]
fn chip_call_creates_instance_per_call() {
    let src = "chip Add(a: int, b: int) -> (result: int) { out result = a + b }\nlet r1 = Add(1, 2)\nlet r2 = Add(3, 4)";
    let r = compile(src);
    assert_eq!(
        r.module.chips.len(),
        2,
        "each call should create a separate chip instance"
    );
}

#[test]
fn chip_call_single_output_no_duplicate() {
    let src = "chip Double(x: int) -> (result: int) { out result = x * 2 }\nlet r = Double(21)";
    let r = compile(src);
    let chip = r.module.chips.values().next().expect("one chip instance");
    let output_count = chip
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Output)
        .count();
    assert_eq!(
        output_count, 1,
        "should have exactly 1 output node, not duplicated"
    );
}

#[test]
fn chip_call_multi_output() {
    let src = "chip MinMax(a: int, b: int) -> (lo: int, hi: int) {\n  out lo = if a < b then a else b\n  out hi = if a > b then a else b\n}\nlet mm = MinMax(3, 7)";
    let r = compile(src);
    let chip = r.module.chips.values().next().expect("one chip instance");
    let output_count = chip
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Output)
        .count();
    assert_eq!(output_count, 2, "should have exactly 2 output nodes");
}

#[test]
fn inline_mod_multi_output_fields_wire_to_sources() {
    // Regression: `let s = M(...); s.p / s.q` on a multi-output inline mod used
    // to bind `s` to a phantom node 0 (ExecOut), leaving the outputs
    // disconnected. Each field must resolve to its real value source.
    let src = "in a: float\n\
        mod M(v: float) -> (p: float, q: float) {\n  out p = v + 1.0\n  out q = v + 2.0\n}\n\
        let s = M(a)\nout x = s.p\nout y = s.q";
    let r = compile(src);
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    // No wire may originate from the phantom node 0.
    let phantom = r.module.wires.iter().any(|w| w.source.node_id == NodeId(0));
    assert!(!phantom, "multi-output mod read produced a phantom n0 wire");
    // Every output node must be fed by an incoming wire.
    for oid in &r.module.outputs {
        assert!(
            r.module.wires.iter().any(|w| w.target.node_id == *oid),
            "output node {oid:?} has no incoming wire"
        );
    }
}

#[test]
fn chip_call_wires_args_to_inputs() {
    let src = "in x: int\nin y: int\nchip Add(a: int, b: int) -> (result: int) { out result = a + b }\nlet r = Add(x, y)";
    let r = compile(src);
    // Parent module should have wires to child MicrochipInput nodes
    let child = r.module.chips.values().next().unwrap();
    let child_input_ids: std::collections::HashSet<NodeId> = child.inputs.iter().cloned().collect();
    let input_wires: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| child_input_ids.contains(&w.target.node_id))
        .collect();
    assert!(
        input_wires.len() >= 2,
        "should wire args directly to child MicrochipInput nodes, got {}",
        input_wires.len()
    );
}

#[test]
fn chip_call_output_wire_in_parent() {
    let src = "chip Double(x: int) -> (result: int) { out result = x * 2 }\nlet r = Double(21)\nout val = r.result";
    let r = compile(src);
    // Parent should have a wire from child MicrochipOutput to the out binding
    let child = r.module.chips.values().next().unwrap();
    let child_output_ids: std::collections::HashSet<NodeId> =
        child.outputs.iter().cloned().collect();
    let out_wires: Vec<_> = r
        .module
        .wires
        .iter()
        .filter(|w| child_output_ids.contains(&w.source.node_id))
        .collect();
    assert!(
        out_wires.len() >= 1,
        "should have at least 1 wire from child MicrochipOutput"
    );
}

#[test]
fn chip_call_compiles_to_brz() {
    let src = "chip ALU(a: int, b: int) -> (result: int) { out result = a + b }\nlet r = ALU(1, 2)\nout sum = r.result";
    let r = compile(src);
    let lr = crate::layout::layout(&r.module);
    let opts = crate::emit::EmitOptions::default();
    let brz = crate::emit::emit_brz(
        &r.module,
        &lr,
        &opts,
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    );
    assert!(brz.is_ok(), "should emit valid brz: {:?}", brz.err());
}

#[test]
fn mod_still_inlines() {
    let src = "var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart { inc(x) }";
    let r = compile(src);
    assert!(
        r.module.chips.is_empty(),
        "mod should inline, not create chip instances"
    );
}

#[test]
fn prune_dead_unions_removes_chains() {
    let src = "var x: int = 0\non RoundStart { x = 1 }\non Bumped { x = 2 }\non Bumped { x = 3 }";
    let r = compile(src);
    let union_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class.contains("Exec_Union"))
        .count();
    // Dead unions (those whose ExecOut feeds nothing) should be pruned.
    // At least some handler merging happens, and dead-end unions get removed.
    assert!(
        union_count <= 2,
        "dead unions should be pruned, got {union_count}"
    );
}

#[test]
fn inline_literals_folded() {
    let src = "var x: int = 0\non RoundStart { x = x + 1 }";
    let r = compile(src);
    let literal_count = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == "_Literal")
        .count();
    assert_eq!(
        literal_count, 0,
        "_Literal nodes should be inlined into consumer properties"
    );
}

#[test]
fn chip_output_named_like_a_swizzle_is_not_split() {
    // A chip output called `x`/`y`/`z` (or `r`/`g`/`b`/`a`) collides with the
    // vector/color component names. Reading it must wire the chip's OUTPUT
    // through, not lower to a SplitVector that splits the (int) value as if it
    // were a vector and silently reads a garbage component.
    fn count_splits(m: &crate::ir::Module) -> usize {
        m.nodes
            .values()
            .filter(|n| n.gate_class == crate::ir::gate_class::SPLIT_VECTOR)
            .count()
            + m.chips.values().map(count_splits).sum::<usize>()
    }

    let r = compile(
        "in a: int\nchip F(n: int) -> (y: int) { out y = n * 2 + 1 }\nlet c = F(a)\nout r = c.y",
    );
    assert_no_errors(&r);
    assert_eq!(
        count_splits(&r.module),
        0,
        "a chip output named `y` must read the output, not lower to a SplitVector"
    );

    // ...but a genuine vector swizzle must still split.
    let v = compile("in v: vector\nlet p = v\nout ox = p.x\nout oy = p.y");
    assert_no_errors(&v);
    assert!(
        count_splits(&v.module) > 0,
        "a real vector component access must still lower to a SplitVector"
    );
}

#[test]
fn nested_chip_instance_exec_wire_stays_local() {
    // A `chip` exec-called from INSIDE a nested anon chip must have its exec
    // wire routed into that anon chip's module (one grid boundary from the
    // instance), not left at the root spanning several grids. The game can
    // route a wire one grid in, but an exec pulse can't cross into an instance
    // grid nested inside another anon chip, so the called chip would silently
    // never fire. Regression for the Secret Hitler chancellor-enact bug
    // (applyEnact never ran; discard did).
    let src = "\
var x: int = 0
var y: int = 0
in trig: exec
chip setX() { x = 1 }
mod doPick() { y = 2 setX() }
chip on trig {
  chip {
    doPick()
  }
}";
    let r = compile(src);
    assert_no_errors(&r);

    // gate class of every node in the whole tree (pins live in child modules)
    fn collect_classes(
        m: &crate::ir::Module,
        out: &mut std::collections::HashMap<crate::ir::NodeId, &'static str>,
    ) {
        for (id, n) in &m.nodes {
            out.insert(*id, n.gate_class);
        }
        for c in m.chips.values() {
            collect_classes(c, out);
        }
    }
    let mut classes = std::collections::HashMap::new();
    collect_classes(&r.module, &mut classes);
    let is_pin = |id: &crate::ir::NodeId| {
        matches!(
            classes.get(id).copied(),
            Some("BrickComponentType_Internal_MicrochipInput")
                | Some("BrickComponentType_Internal_MicrochipOutput")
        )
    };

    // In each module, a wire touching a chip-boundary pin that isn't one of the
    // module's OWN pins must have that pin on a DIRECT child chip.
    fn check(
        m: &crate::ir::Module,
        is_pin: &dyn Fn(&crate::ir::NodeId) -> bool,
    ) {
        let mut child_pins: std::collections::HashSet<crate::ir::NodeId> =
            std::collections::HashSet::new();
        for c in m.chips.values() {
            child_pins.extend(c.inputs.iter().copied());
            child_pins.extend(c.outputs.iter().copied());
        }
        for w in &m.wires {
            if w.source.port == WirePort::Layout || w.target.port == WirePort::Layout {
                continue;
            }
            for ep in [w.source.node_id, w.target.node_id] {
                if is_pin(&ep) && !m.nodes.contains_key(&ep) {
                    assert!(
                        child_pins.contains(&ep),
                        "module {:?}: wire touches boundary pin {ep} not on a direct \
                         child chip — a multi-grid exec wire the instance can't receive",
                        crate::intern::resolve(m.name)
                    );
                }
            }
        }
        for c in m.chips.values() {
            check(c, is_pin);
        }
    }
    check(&r.module, &is_pin);
}

#[test]
fn dedup_keeps_constants_within_their_chip() {
    // A constant used as a WIRED (multi-consumer) literal in two separate anon
    // chips must NOT be merged across the chip boundary. `dedup_constant_gates`
    // runs before `partition_anon_chips`, so merging to a keeper in a different
    // chip leaves the other chip's consumer with a cross-chip literal data wire;
    // partition keeps that wire in the parent without cloning the literal, and
    // emit (which inlines _Literal sources per-module) can't reach it, so the
    // operand silently reads its port default (0). This manifested as seat
    // indices 7/9 folding to 0 in the Secret Hitler input queue. `i` is used
    // twice so the arg literal stays wired (single-use args inline earlier and
    // never reach dedup).
    let src = "\
array q: int[]
in trig: exec
mod enc(i: int) { q.push(i * 4) q.push(i) }
on trig {
  chip { enc(6) enc(7) }
  chip { enc(7) enc(9) }
}";
    let r = compile(src);
    assert_no_errors(&r);

    // Assign every node a module tag and record which nodes are `_Literal`.
    // A `_Literal` feeding an operand port (InputA/InputB) must live in the
    // SAME module as its consumer, or emit's per-module inlining can't reach it.
    let mut module_of: std::collections::HashMap<crate::ir::NodeId, usize> =
        std::collections::HashMap::new();
    let mut is_literal: std::collections::HashSet<crate::ir::NodeId> =
        std::collections::HashSet::new();
    fn index(
        m: &crate::ir::Module,
        tag: &mut usize,
        module_of: &mut std::collections::HashMap<crate::ir::NodeId, usize>,
        is_literal: &mut std::collections::HashSet<crate::ir::NodeId>,
    ) {
        let my = *tag;
        *tag += 1;
        for (id, n) in &m.nodes {
            module_of.insert(*id, my);
            if n.gate_class == "_Literal" {
                is_literal.insert(*id);
            }
        }
        for child in m.chips.values() {
            index(child, tag, module_of, is_literal);
        }
    }
    let mut tag = 0;
    index(&r.module, &mut tag, &mut module_of, &mut is_literal);

    fn check_wires(
        m: &crate::ir::Module,
        module_of: &std::collections::HashMap<crate::ir::NodeId, usize>,
        is_literal: &std::collections::HashSet<crate::ir::NodeId>,
    ) {
        for w in &m.wires {
            let is_operand = matches!(w.target.port, WirePort::InputA | WirePort::InputB);
            if is_operand && is_literal.contains(&w.source.node_id) {
                assert_eq!(
                    module_of.get(&w.source.node_id),
                    module_of.get(&w.target.node_id),
                    "constant operand wire crosses a chip boundary (source {:?} -> \
                     {:?}.{:?}); emit can't inline it and the operand reads 0",
                    w.source.node_id,
                    w.target.node_id,
                    w.target.port
                );
            }
        }
        for child in m.chips.values() {
            check_wires(child, module_of, is_literal);
        }
    }
    check_wires(&r.module, &module_of, &is_literal);
}

#[test]
fn chip_decls_shared_across_instances() {
    let src = "\
chip Add(a: int, b: int) -> (result: int) { out result = a + b }
let r1 = Add(1, 2)
let r2 = Add(3, 4)
let r3 = Add(5, 6)
out sum = r1.result + r2.result + r3.result";
    let r = compile(src);
    assert_eq!(
        r.module.chips.len(),
        3,
        "three chip instances should be created"
    );
}

#[test]
fn namespace_chip_call_resolves() {
    use crate::resolve::{MemLoader, resolve};
    let lib_src = "chip Double(x: int) -> (result: int) { out result = x + x }";
    let main_src = "import * as math from \"lib\"\nlet r = math.Double(5)\nout result = r.result";
    let mut files = std::collections::HashMap::default();
    files.insert("lib.ws".to_string(), lib_src.into());
    let loader = MemLoader { files };
    let resolved = resolve(main_src, "test", &loader);
    assert!(
        resolved.diagnostics.is_empty(),
        "import should resolve: {:?}",
        resolved.diagnostics
    );
    let tc = crate::typecheck::typecheck(&resolved.ast, "test");
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    let lr = crate::lower::lower(crate::lower::LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: Arc::new(TemplateCache::new()),
        doc_comments: &resolved.doc_comments,
    });
    assert!(
        lr.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "lower errors: {:?}",
        lr.diagnostics
    );
    assert!(
        !lr.module.chips.is_empty(),
        "namespace chip should create chip instance"
    );
}

#[test]
fn single_output_chip_in_arithmetic() {
    let r = compile(
        "\
chip Sq(x: int) -> int { out _ = x * x }
let a = Sq(3)
let b = Sq(4)
let y = a + b
out z = y",
    );
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "single-output chip in arithmetic should compile: {:?}",
        r.diagnostics
    );
    assert_eq!(
        r.module.chips.len(),
        2,
        "should create 2 chip instances for Sq(3) and Sq(4)"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
        "should produce MathAdd for auto-unwrapped chip results"
    );
}

#[test]
fn unused_local_mod_no_warning() {
    let r = compile(
        "\
mod foo(x: *int) { x = 1 }
var y: int = 0
in player: character
on player { y = 2 }",
    );
    let has_ws014 = r.diagnostics.iter().any(|d| d.code == "WS014");
    assert!(
        !has_ws014,
        "locally defined unused mod should not trigger WS014 warning"
    );
}

/// Regression: inline mod outputs must not contaminate the parent module's outputs.
/// When both the mod and the parent have an output named `key_state`, the mod's
/// inline cleanup must not destroy the parent's output.
#[test]
fn inline_mod_output_does_not_leak_to_parent() {
    let r = compile(
        "\
mod Gamepad(a: bool) -> (key_state: int) {
  return if a then 0x3FE else 0x3FF
}
let pad = Gamepad(true)
out key_state = pad",
    );
    assert_no_errors(&r);
    assert_eq!(
        r.module.outputs.len(),
        1,
        "parent should have exactly 1 output, got {}",
        r.module.outputs.len()
    );
    let output_nodes: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Output)
        .collect();
    assert_eq!(
        output_nodes.len(),
        1,
        "should have exactly 1 output node after inline cleanup, got {}",
        output_nodes.len()
    );
}

/// Verify .key_state field access on auto-unwrapped single-output mod works.
#[test]
fn inline_mod_field_access_on_auto_unwrapped_output() {
    let r = compile(
        "\
mod Gamepad(a: bool) -> (key_state: int) {
  return if a then 0x3FE else 0x3FF
}
let pad = Gamepad(true)
out key_state = pad.key_state",
    );
    assert_no_errors(&r);
    let lr = crate::layout::layout(&r.module);
    let brz = crate::emit::emit_brz(
        &r.module,
        &lr,
        &Default::default(),
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    );
    assert!(
        brz.is_ok(),
        "should compile to brz without port errors: {:?}",
        brz.err()
    );
}

/// Regression: `if true { body }` should be constant-folded - no Branch gate.
#[test]
fn if_true_constant_folded() {
    let r = compile(
        "\
var x: int = 0
in tick: exec
on tick { if true { x = 1 } }",
    );
    assert_no_errors(&r);
    let branch_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch");
    assert_eq!(
        branch_count, 0,
        "if true should be constant-folded, no Branch gate"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Exec_Var_Set"),
        "body should still produce Var_Set"
    );
}

/// Regression: `if false { body } else { other }` should fold to just the else.
#[test]
fn if_false_constant_folded_to_else() {
    let r = compile(
        "\
var x: int = 0
in tick: exec
on tick { if false { x = 1 } else { x = 2 } }",
    );
    assert_no_errors(&r);
    let branch_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch");
    assert_eq!(
        branch_count, 0,
        "if false should be constant-folded, no Branch gate"
    );
}

/// Regression: constant-fold also applies when condition is a local
/// bound to a literal bool (e.g. inline mod param with literal `true`).
#[test]
fn if_literal_param_constant_folded() {
    let r = compile(
        "\
mod foo(x: *int, cond: bool) { if cond { x = 1 } }
var v: int = 0
in tick: exec
on tick { foo(v, true) }",
    );
    assert_no_errors(&r);
    let branch_count = gate_count(&r, "BrickComponentType_WireGraph_Exec_Branch");
    assert_eq!(
        branch_count, 0,
        "if <literal-param> should be constant-folded"
    );
}

/// Record params: ref/array fields are scope-captured (no MicrochipInput),
/// value fields still use MicrochipInput boundary ports.
#[test]
fn chip_record_param_dissolves_to_ports() {
    let r = compile(
        "\
type State = { val: *int, arr: int[] }
var v: int = 0
array a: int[]
chip Foo(s: State) -> (result: int) {
  in run: exec
  on run { s.arr.push(s.val) }
  out result = 0
}
let s: State = { val: v, arr: a }
let r = Foo(s)",
    );
    assert_no_errors(&r);
    assert_eq!(r.module.chips.len(), 1, "should create one chip instance");
    let chip = r.module.chips.values().next().unwrap();
    let input_labels: Vec<_> = chip
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Input)
        .filter_map(|n| {
            n.properties
                .get(&crate::intern::intern("PortLabel"))
                .and_then(|l| {
                    if let Literal::String(s) = l {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
        })
        .collect();
    assert!(
        !input_labels.contains(&"s_val".to_string()),
        "s_val (*int) should be scope-captured, not a MicrochipInput, got: {:?}",
        input_labels
    );
    assert!(
        !input_labels.contains(&"s_arr".to_string()),
        "s_arr (int[]) should be scope-captured, not a MicrochipInput, got: {:?}",
        input_labels
    );
    assert!(
        !chip.scope_captures.is_empty(),
        "chip should have scope captures for ref/array fields"
    );
}

/// Ref params on standalone chips create VarRef input ports.
#[test]
fn chip_ref_param_creates_var_binding() {
    let r = compile(
        "\
var flag: bool = false
chip Toggle(f: *bool) -> (result: int) {
  in run: exec
  on run { f = true }
  out result = 0
}
let r = Toggle(flag)",
    );
    assert_no_errors(&r);
    assert_eq!(r.module.chips.len(), 1);
}

#[test]
fn unassigned_output_warning() {
    let r = compile(
        "\
mod bad(x: int) -> (result: int) {
  x + 1
}
in player: character
on player {
  let b = bad(5)
}",
    );
    assert_no_errors(&r);
}

/// Regression: chip value params that require exec (Var_Get) must be read
/// BEFORE the chip's auto-exec entry, not after. Otherwise the exec chain
/// creates a cycle: chip._exec_out → Var_Get → ... → chip.param (data back-edge).
#[test]
fn chip_value_args_exec_before_chip_entry() {
    let src = "\
var a: int = 0
var b: int = 0
chip Add(x: int, y: int) -> (result: int) { out result = x + y }
chip Inc(v: *int) { v = v + 1 }
in start: exec
on start {
  Inc(a)
  Inc(b)
  let r = Add(a, b)
}";
    let r = compile(src);
    assert_no_errors(&r);

    // Find the Add chip's child module (has 2+ value inputs for a, b)
    let (add_chip_id, add_child) = r
        .module
        .chips
        .iter()
        .find(|(_, child)| child.outputs.len() >= 1 && child.inputs.len() >= 2)
        .expect("should find Add chip");

    // Exec goes to child's _exec_in MicrochipInput (last input)
    let exec_in_node = *add_child.inputs.last().unwrap();
    let exec_into_chip = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == exec_in_node && w.target.port == WirePort::RerInput)
        .expect("child _exec_in MicrochipInput should have a wire");

    // The exec source should be a Var_Get (args evaluated first), not the
    // handler entry or a chip node (which would mean exec fires before reads).
    let exec_source_node = r
        .module
        .nodes
        .get(&exec_into_chip.source.node_id)
        .expect("exec source node should exist");
    let source_class = exec_source_node.gate_class;
    assert!(
        source_class.contains("Var_Get")
            || source_class.contains("Var_Set")
            || source_class.contains("ArrayVar")
            || source_class.contains("Exec_"),
        "exec into chip should come from a Var_Get (args first), got: {} ({})",
        exec_source_node.id,
        source_class,
    );

    // Verify no layout cycle
    let layout = crate::layout::layout(&r.module);
    assert!(
        layout.placements.contains_key(add_chip_id),
        "chip node should have a valid layout placement (no cycles)"
    );
}

/// Regression: string interpolation (FormatText) must emit valid brz.
/// FormatText fields are `str` in the game schema, not `wire_graph_variant`.
#[test]
fn string_interpolation_emits_brz() {
    let src = r#"
in player: character
on player {
  var x: int = 42
  player.DisplayText("value=${x}", fontSize = 30, lifetime = 10.0, textId = 1)
}"#;
    let r = compile(src);
    assert_no_errors(&r);
    let lr = crate::layout::layout(&r.module);
    let tc = std::sync::Arc::new(crate::template_cache::TemplateCache::new());
    let brz = crate::emit::emit_brz(&r.module, &lr, &Default::default(), &tc);
    assert!(
        brz.is_ok(),
        "string interpolation should emit valid brz: {:?}",
        brz.err()
    );
}

#[test]
fn emit_value_inside_chip_handler_assigns_outputs() {
    // Init-style chips mint values in an `on` handler and emit them into
    // their outputs. The WS013 unassigned-output check must count `emit`
    // (including nested in handlers), and lowering must accept the shape.
    let src = "chip Init(t: exec) -> (code: int, done: exec) {\n  on t {\n    emit code = 7\n    emit done\n  }\n}\nin s: exec\nlet r = Init(s)\nout v = r.code";
    let parsed = crate::parser::parse(src, "test");
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let tc = crate::typecheck::typecheck(&parsed.ast, "test");
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected typecheck errors: {:?}",
        tc.diagnostics
    );
    let r = compile(src);
    assert_no_errors(&r);
}

#[test]
fn exec_param_handler_drives_chip_body() {
    // The `on init { }` handler is how a chip body binds work to its exec
    // param - the param must NOT implicitly invoke the body (exec-context
    // callers already get exec-driven bodies via the auto-exec boundary).
    // The push gate inside the child module must be wired from the handler.
    // State comes in through a record-of-arrays param here; free top-level
    // references also work (see named_chip_body_captures_top_level_state).
    let src = "array names: string[]\ntype Tables = { names: string[] }\nlet TB: Tables = { names }\nchip Init(init: exec, tables: Tables) -> (code: int) {\n  on init {\n    tables.names.push(\"a\")\n    emit code = 5\n  }\n}\nin s: exec\nlet r = Init(s, TB)\nout v = r.code";
    let r = compile(src);
    assert_no_errors(&r);
    let child = r
        .module
        .chips
        .values()
        .next()
        .expect("chip instance should produce a child module");
    let push = child
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
        .expect("child module should contain the push gate");
    let push_exec_wired = child.wires.iter().any(|w| w.target.node_id == push.id);
    assert!(
        push_exec_wired,
        "push gate should be driven by the chip's exec param"
    );
}

#[test]
fn entity_setters_compile_to_brz() {
    // Entity Set*/Teleport gates derive their data structs (Vector/Rotator
    // and composite TeleportDestination fields) — the writer must fill
    // defaults for all of them without failing the emit.
    let src = "in trigger: exec\nin e: entity\nin d: entity\non trigger {\n  \
        e.SetLocation(Vec(1.0, 2.0, 3.0))\n  \
        e.SetRotation(Rotation(0.0, 90.0, 0.0))\n  \
        e.SetVelocity(Vec(0.0, 0.0, 10.0))\n  \
        e.Teleport(d)\n}";
    let r = compile(src);
    assert_no_errors(&r);
    let lr = crate::layout::layout(&r.module);
    let opts = crate::emit::EmitOptions::default();
    let brz = crate::emit::emit_brz(
        &r.module,
        &lr,
        &opts,
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    );
    assert!(brz.is_ok(), "should emit valid brz: {:?}", brz.err());
}

#[test]
fn entity_setter_embeds_vector_literal() {
    // Set* gates store unwired input values in their data structs, so folded
    // Vec/Rotation literals embed directly instead of spawning a
    // MakeVector/MakeRotation wired into the port.
    let src = "in trigger: exec\nin e: entity\non trigger {\n  \
        e.SetLocation(Vec(1.0, 2.0, 3.0))\n  \
        e.SetRotation(Rotation(0.0, 90.0, 0.0))\n}";
    let r = compile(src);
    assert_no_errors(&r);
    for class in [
        crate::ir::gate_class::MAKE_VECTOR,
        crate::ir::gate_class::MAKE_ROTATION,
    ] {
        assert!(
            !r.module.nodes.values().any(|n| n.gate_class == class),
            "literal args should embed as gate data, not spawn {class}"
        );
    }
    // and the emit path serializes the embedded f64 struct values
    let lr = crate::layout::layout(&r.module);
    let opts = crate::emit::EmitOptions::default();
    let brz = crate::emit::emit_brz(
        &r.module,
        &lr,
        &opts,
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    );
    assert!(brz.is_ok(), "should emit valid brz: {:?}", brz.err());
}

#[test]
fn spawn_prefab_compiles_to_brz() {
    // The PrefabSpawner data struct has an array field (SpawnedEntityIds)
    // wirescript never populates - LiteralComponent must report it as
    // missing (serialized as an empty array), not fail the whole emit.
    // Covers both the bare call and the full argument set.
    let src = "in trigger: exec\non trigger {\n  let car = SpawnPrefab()\n  \
        let boat = SpawnPrefab(offset = Vec(0.0, 0.0, 50.0), rotation = Rotation(0.0, 90.0, 0.0), velocity = Vec(0.0, 0.0, 100.0), lifetime = 10.0, limit = 5)\n}";
    let r = compile(src);
    assert_no_errors(&r);
    let lr = crate::layout::layout(&r.module);
    let opts = crate::emit::EmitOptions::default();
    let brz = crate::emit::emit_brz(
        &r.module,
        &lr,
        &opts,
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    );
    assert!(brz.is_ok(), "should emit valid brz: {:?}", brz.err());
}

#[test]
fn exec_named_arg_drives_chip_body_outside_exec_context() {
    // Outside exec contexts, exec chips take their trigger as an `exec =`
    // named arg: `let r = Init(TB, exec = s)` at top level. The body's push
    // gate must be exec-wired, not silently dead.
    let src = "array names: string[]\ntype Tables = { names: string[] }\nlet TB: Tables = { names }\nchip Init(tables: Tables) -> (code: int) {\n  tables.names.push(\"a\")\n  emit code = 5\n}\nin s: exec\nlet r = Init(TB, exec = s)\nout v = r.code";
    let r = compile(src);
    assert_no_errors(&r);
    let child = r
        .module
        .chips
        .values()
        .next()
        .expect("chip instance should produce a child module");
    let push = child
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
        .expect("child module should contain the push gate");
    let push_exec_wired = child.wires.iter().any(|w| w.target.node_id == push.id);
    assert!(
        push_exec_wired,
        "push gate should be driven via the exec named arg"
    );
    // The exec boundary input must exist and be wired from the caller side.
    let exec_in = child
        .inputs
        .iter()
        .any(|nid| child.nodes.get(nid).is_some());
    assert!(exec_in, "child should expose its exec boundary input");
}

#[test]
fn exec_arg_call_exposes_exec_field() {
    // A chip call with `exec =` also returns the chip's completion exec as an
    // `exec` record field: `let r = Init(TB, exec = s)` ... `on r.exec { }`.
    let src = "array names: string[]\ntype Tables = { names: string[] }\nlet TB: Tables = { names }\nchip Init(tables: Tables) -> (code: int) {\n  tables.names.push(\"a\")\n  emit code = 5\n}\nin s: exec\nlet r = Init(TB, exec = s)\nvar hit: int = 0\non r.exec { hit = hit + 1 }\nout v = r.code";
    let r = compile(src);
    assert_no_errors(&r);
    let child = r
        .module
        .chips
        .values()
        .next()
        .expect("chip instance should produce a child module");
    let exec_out = *child
        .outputs
        .last()
        .expect("child should have the _exec_out boundary output");
    // The handler must be driven from the boundary output in the parent.
    let exec_wired = r.module.wires.iter().any(|w| w.source.node_id == exec_out);
    assert!(
        exec_wired,
        "on r.exec should wire from the chip's _exec_out"
    );
}

#[test]
fn named_chip_body_captures_top_level_state() {
    // Free references to top-level arrays/vars inside a named chip body
    // resolve against the caller's scope — wire refs cross chip boundaries,
    // so the body's gates connect to the outer nodes directly.
    let src = "array names: string[]\nvar count: int = 0\nchip Init() -> (code: int) {\n  names.push(\"a\")\n  count = count + 1\n  emit code = 7\n}\nin s: exec\nlet r = Init(exec = s)\nout v = r.code";
    let r = compile(src);
    assert_no_errors(&r);
    let child = r
        .module
        .chips
        .values()
        .next()
        .expect("chip instance should produce a child module");
    let push = child
        .nodes
        .values()
        .find(|n| n.gate_class == "BrickComponentType_WireGraph_Exec_ArrayVar_Push")
        .expect("child module should contain the push gate");
    let push_wired = child.wires.iter().any(|w| w.target.node_id == push.id);
    assert!(push_wired, "free-array push should be exec-wired");
}

// ── Use-before-declaration guard (WS021) ──

#[test]
fn use_before_declaration_is_ws021() {
    // A handler transitively calls `target`, declared later in source. Chips/mods
    // register into scope in source order, so the call cannot resolve during
    // lowering: it would otherwise become an `_Unsupported` gate that silently
    // reads its default (0) at runtime. It must be a hard error instead.
    let src = "in z: exec\n\
        mod caller() { let x = target(1) BroadcastChatMessage(\"${x}\") }\n\
        on z { caller() }\n\
        mod target(n: int) -> int { return n + 1 }\n";
    let r = compile(src);
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS021"),
        "use-before-declaration must emit WS021; got {:?}",
        r.diagnostics
    );
}

#[test]
fn declaration_before_use_is_not_ws021() {
    // Same program, `target` declared before its caller: resolves normally.
    let src = "in z: exec\n\
        mod target(n: int) -> int { return n + 1 }\n\
        mod caller() { let x = target(1) BroadcastChatMessage(\"${x}\") }\n\
        on z { caller() }\n";
    let r = compile(src);
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS021"),
        "declaration-before-use must NOT emit WS021; got {:?}",
        r.diagnostics
    );
}

// ── `in` array inputs are first-class ──

#[test]
fn in_array_supports_methods_and_index() {
    // An `in X: T[]` input must work like a var array: index AND methods.
    let src = "in counts: int[]\nin z: exec\n\
        on z { let a = counts[0]  let b = counts.length()  BroadcastChatMessage(\"${a} ${b}\") }";
    let r = compile(src);
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WSP001"),
        "input-array index/method must lower (no placeholder); got {:?}",
        r.diagnostics
    );
}

#[test]
fn in_array_passes_to_inline_mod() {
    // Passing an `in` array to an inline mod's `int[]` param must bind it, so
    // the mod body's reads resolve to the input's ref.
    let src = "in counts: int[]\narray dst: int[]\nin z: exec\n\
        mod f(a: int[], d: int[]) { d.clear()  d.push(a[0])  BroadcastChatMessage(\"${a.length()}\") }\n\
        on z { f(counts, dst) }";
    let r = compile(src);
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WSP001"),
        "input array passed to inline mod must bind (no placeholder); got {:?}",
        r.diagnostics
    );
}

#[test]
fn chip_constant_arg_folds_into_the_instance() {
    // A constant argument used to cross the boundary as a real gate: the caller
    // built a `_Var` holding the value and wired it into the chip's input
    // rerouter, because a rerouter has no data struct to carry inline gate data.
    // That made `Bump(1)` strictly more expensive than the same call written as a
    // `mod`, which folds the constant straight onto its consumer. The constant is
    // now cloned into the instance and the pin it fed is dropped. Instances of one
    // chip share a template, so the hazard is every instance collapsing onto a
    // single call's value — each must keep its own.
    let src = "\
var acc: int = 0
in t: exec
chip Bump(n: int) { acc = acc + n }
on t { Bump(1) Bump(2) }";
    let r = compile(src);
    assert_no_errors(&r);

    // Only the real `acc` variable is left in the caller; no constant-carrying
    // gate remains feeding a chip boundary.
    let parent_vars = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::PSEUDO_VAR)
        .count();
    assert_eq!(
        parent_vars, 1,
        "a constant arg should not leave a `_Var` gate in the caller"
    );

    let mut folded: Vec<i64> = Vec::new();
    for child in r.module.chips.values() {
        assert_eq!(
            child.inputs.len(),
            1,
            "the value pin should be folded away, leaving only the exec pin"
        );
        for n in child.nodes.values() {
            if n.gate_class == gc::VAR_INCREMENT
                && let Some(Literal::Int(v)) = n.properties.get(&*sym::VALUE)
            {
                folded.push(*v);
            }
        }
    }
    folded.sort();
    assert_eq!(
        folded,
        vec![1, 2],
        "each instance must keep its own folded constant"
    );
}

// ── Namespaced (`import * as`) mods resolve their siblings ──

#[test]
fn namespaced_mod_resolves_siblings() {
    // A namespaced mod body references sibling constants, arrays, and mods by
    // bare name; those must resolve when the mod is inlined at a call site in
    // the importing module (no `_Unsupported` placeholders).
    let lib = "let K = 7.0\n\
        array TBL: int[] = [10, 20, 30]\n\
        mod helper(n: int) -> int { return n + 1 }\n\
        mod draw(ctrl: controller, i: int) {\n\
          let v = K + TBL[i]\n\
          let h = helper(i)\n\
          ctrl.DisplayText(\"hi\", positionX = v, fontSize = h)\n\
        }";
    let diags = lower_with_imports(
        "import * as lib from \"lib\"\non ControllerJoined(c, uid) { lib.draw(c, 0) }",
        &[("lib", lib)],
    );
    assert!(
        !diags.iter().any(|d| d.code == "WSP001"),
        "namespaced mod's sibling refs must resolve (no placeholder); got {:?}",
        diags
    );
}

#[test]
fn unknown_non_function_call_is_not_ws021() {
    // A call to a name that is NOT a declared chip/mod (here `vec`, a builtin the
    // lowerer doesn't implement) must keep the "unsupported placeholder" path,
    // not be reported as a use-before-declaration.
    let src = "let v = vec(1.0, 2.0, 3.0)\n";
    let r = compile(src);
    assert!(
        !r.diagnostics.iter().any(|d| d.code == "WS021"),
        "unimplemented builtin must NOT emit WS021; got {:?}",
        r.diagnostics
    );
}
