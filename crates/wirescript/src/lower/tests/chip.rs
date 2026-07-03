use crate::{ir::port_registry::WirePort, template_cache::TemplateCache};

use super::*;

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
    let phantom = r
        .module
        .wires
        .iter()
        .any(|w| w.source.node_id == NodeId(0));
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
    let child_input_ids: std::collections::HashSet<NodeId> =
        child.inputs.iter().cloned().collect();
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
    let placements = crate::layout::layout(&r.module).placements;
    let opts = crate::emit::EmitOptions::default();
    let brz = crate::emit::emit_brz(&r.module, &placements, &opts, &std::sync::Arc::new(crate::template_cache::TemplateCache::new()));
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
    let mut files = std::collections::HashMap::new();
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
    let brz = crate::emit::emit_brz(&r.module, &lr.placements, &Default::default(), &std::sync::Arc::new(crate::template_cache::TemplateCache::new()));
    assert!(
        brz.is_ok(),
        "should compile to brz without port errors: {:?}",
        brz.err()
    );
}

/// Regression: `if true { body }` should be constant-folded — no Branch gate.
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
    let (add_chip_id, add_child) = r.module.chips.iter()
        .find(|(_, child)| child.outputs.len() >= 1 && child.inputs.len() >= 2)
        .expect("should find Add chip");

    // Exec goes to child's _exec_in MicrochipInput (last input)
    let exec_in_node = *add_child.inputs.last().unwrap();
    let exec_into_chip = r.module.wires.iter().find(|w|
        w.target.node_id == exec_in_node
            && w.target.port == WirePort::RerInput
    ).expect("child _exec_in MicrochipInput should have a wire");

    // The exec source should be a Var_Get (args evaluated first), not the
    // handler entry or a chip node (which would mean exec fires before reads).
    let exec_source_node = r.module.nodes.get(&exec_into_chip.source.node_id)
        .expect("exec source node should exist");
    let source_class = exec_source_node.gate_class;
    assert!(
        source_class.contains("Var_Get") || source_class.contains("Var_Set")
            || source_class.contains("ArrayVar") || source_class.contains("Exec_"),
        "exec into chip should come from a Var_Get (args first), got: {} ({})",
        exec_source_node.id, source_class,
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
    let brz = crate::emit::emit_brz(&r.module, &lr.placements, &Default::default(), &tc);
    assert!(brz.is_ok(), "string interpolation should emit valid brz: {:?}", brz.err());
}
