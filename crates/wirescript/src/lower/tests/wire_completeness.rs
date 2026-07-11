//! Wire-completeness fuzzer.
//!
//! The recurring bug class in chip/mod lowering is an OUTPUT that ships with no
//! incoming wire: the exec output orphaned on `return`, then the value output
//! orphaned when the enclosing module had an `out`. Both are silent - the graph
//! compiles, the port just reads its default at runtime.
//!
//! This walks a combinatorial matrix of the features that interact with output
//! wiring (function kind, return shape, body exec, a parent `out`, call context,
//! output arity) and asserts every declared MicrochipOutput in every emitted
//! chip is driven. A failure prints the exact program so the case is minimal.

use super::compile;
use crate::diagnostic::Severity;
use crate::ir::{Module, NodeId, NodeKind};

/// Recursively collect declared outputs (MicrochipOutput nodes) that have no
/// incoming wire, i.e. a return/output value that was dropped. `_exec_out`
/// counts too - an orphaned exec output is the same bug.
fn unwired_outputs(module: &Module, path: &str, acc: &mut Vec<String>) {
    for out_id in &module.outputs {
        let Some(node) = module.nodes.get(out_id) else {
            continue;
        };
        if node.kind != NodeKind::Output {
            continue;
        }
        let driven = module.wires.iter().any(|w| w.target.node_id == *out_id);
        if !driven {
            acc.push(format!("{path}: output {out_id} has no incoming wire"));
        }
    }
    for (chip_nid, child) in &module.chips {
        let name = crate::intern::resolve(child.name);
        unwired_outputs(child, &format!("{path} > chip {name}#{}", chip_id_num(*chip_nid)), acc);
    }
}

fn chip_id_num(n: NodeId) -> String {
    n.to_string()
}

/// Build one program from the matrix axes and return its source.
fn program(kind: &str, body: &str, parent_out: &str, call: &str, sig: &str) -> String {
    format!(
        "array a: int[]\n\
         {parent_out}var r: int = 0\n\
         {kind} f(x: int) -> {sig} {{\n\
         {body}\n\
         }}\n\
         in z: exec\n\
         on z {{\n\
         {call}\n\
         }}\n"
    )
}

#[test]
fn wire_completeness_matrix() {
    // (label, sig, body) — single-output return shapes.
    let single_bodies = [
        ("select_find", "int", "  let res = a.find(x)\n  return if res.Found then res.Index else -1"),
        ("early_find", "int", "  let res = a.find(x)\n  if res.Found { return res.Index }\n  return -1"),
        ("pure_select", "int", "  return if x > 0 then x else 0 - x"),
        ("plain_value", "int", "  return x + 1"),
        ("exec_then_value", "int", "  let l = a.length()\n  return l + x"),
    ];
    let kinds = ["chip", "mod"];
    let parent_outs = [("", "no_out"), ("out extra: int[] = a\n", "with_out")];
    let calls = [
        ("let s = f(1)", "let"),
        ("BroadcastChatMessage(\"${f(1)}\")", "interp"),
        ("r = f(1)", "assign"),
    ];

    let mut failures: Vec<String> = Vec::new();
    let mut compiled = 0usize;
    let mut skipped = 0usize;

    for kind in kinds {
        for (blabel, sig, body) in single_bodies {
            for (pout, plabel) in parent_outs {
                for (call, clabel) in calls {
                    let src = program(kind, body, pout, call, sig);
                    let r = compile(&src);
                    if r.diagnostics.iter().any(|d| d.severity == Severity::Error) {
                        skipped += 1;
                        continue;
                    }
                    compiled += 1;
                    let case = format!("{kind}/{blabel}/{plabel}/{clabel}");
                    let mut acc = Vec::new();
                    unwired_outputs(&r.module, &case, &mut acc);
                    if !acc.is_empty() {
                        failures.push(format!("--- {case} ---\n{src}\n{}", acc.join("\n")));
                    }
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "wire-completeness: {} case(s) with unwired outputs ({} compiled, {} skipped):\n\n{}",
        failures.len(),
        compiled,
        skipped,
        failures.join("\n\n")
    );
}

#[test]
fn wire_completeness_multi_output_and_nested() {
    // Higher-risk shapes: multi-output (`emit`) chips, multi-`return` (pseudo-var
    // path) with selects, and a chip that returns another chip's result - each
    // crossed with a parent `out` and an exec body.
    let cases: &[(&str, &str)] = &[
        (
            "multi_emit_pure",
            "chip f(x: int) -> (lo: int, hi: int) {\n  emit lo = x - 1\n  emit hi = x + 1\n}\n\
             on z {\n  let p = f(3)\n  r = p.lo + p.hi\n}",
        ),
        (
            "multi_emit_exec",
            "chip f(x: int) -> (lo: int, hi: int) {\n  let n = a.length()\n  emit lo = n\n  emit hi = x + n\n}\n\
             on z {\n  let p = f(3)\n  r = p.lo + p.hi\n}",
        ),
        (
            "multi_emit_find",
            "chip f(x: int) -> (idx: int, hit: int) {\n  let res = a.find(x)\n  emit idx = res.Index\n  emit hit = if res.Found then 1 else 0\n}\n\
             on z {\n  let p = f(3)\n  r = p.idx + p.hit\n}",
        ),
        (
            "multi_return_select",
            "mod f(x: int) -> int {\n  if x > 0 { return if x > 10 then 10 else x }\n  return 0 - 1\n}\n\
             on z {\n  r = f(3)\n}",
        ),
        (
            "chip_returns_chip",
            "chip inner(x: int) -> int {\n  let res = a.find(x)\n  return if res.Found then res.Index else -1\n}\n\
             mod outer(x: int) -> int {\n  return inner(x)\n}\n\
             on z {\n  r = outer(3)\n}",
        ),
        (
            "chip_in_interp",
            "chip inner(x: int) -> int {\n  let res = a.find(x)\n  return if res.Found then res.Index else -1\n}\n\
             on z {\n  BroadcastChatMessage(\"${inner(3)} ${inner(4)}\")\n}",
        ),
    ];

    let mut failures = Vec::new();
    for (label, snippet) in cases {
        // Always with a parent `out` present (the trigger for the value-drop bug).
        let src = format!("array a: int[]\nout extra: int[] = a\nvar r: int = 0\nin z: exec\n{snippet}\n");
        let r = compile(&src);
        if r.diagnostics.iter().any(|d| d.severity == Severity::Error) {
            failures.push(format!(
                "--- {label} (COMPILE ERROR) ---\n{src}\n{:?}",
                r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect::<Vec<_>>()
            ));
            continue;
        }
        let mut acc = Vec::new();
        unwired_outputs(&r.module, label, &mut acc);
        if !acc.is_empty() {
            failures.push(format!("--- {label} ---\n{src}\n{}", acc.join("\n")));
        }
    }
    assert!(
        failures.is_empty(),
        "wire-completeness (multi/nested): {} failing case(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// A chip called from N sites with N *distinct* scalar args must produce N
/// instances whose value-input is wired to a *distinct* source. If the second
/// instance's input collapses to the first caller's arg (or is unwired), every
/// call evaluates the same key -> the "everyone maps to slot 0" runtime bug.
#[test]
fn multi_call_site_distinct_inputs() {
    // Mirror the 2raab slotOfUser shape: module-level string array exposed as an
    // `out`, a chip that find()s it and returns via a select, called 3x with
    // distinct string args inside one exec handler.
    let src = "\
array userIds: string[]\n\
out ui: string[] = userIds\n\
chip slotOfUser(uid: string) -> int {\n\
  let res = userIds.find(uid)\n\
  return if res.Found then res.Index else -1\n\
}\n\
in z: exec\n\
on z {\n\
  let s0 = slotOfUser(\"aaa\")\n\
  let s1 = slotOfUser(\"bbb\")\n\
  let s2 = slotOfUser(\"ccc\")\n\
  BroadcastChatMessage(\"${s0} ${s1} ${s2}\")\n\
}\n";
    let r = compile(&src);
    assert!(
        !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect::<Vec<_>>()
    );

    // Every chip instance's declared outputs must be driven.
    let mut acc = Vec::new();
    unwired_outputs(&r.module, "multi_call_distinct", &mut acc);
    assert!(acc.is_empty(), "unwired outputs:\n{}", acc.join("\n"));

    // For each chip instance, find the parent wire feeding its value-input
    // (inputs[0]) and record the source port. The three must be distinct.
    let mut input_sources: Vec<(NodeId, Option<crate::ir::PortRef>)> = Vec::new();
    for (chip_nid, child) in &r.module.chips {
        let Some(&val_input) = child.inputs.first() else {
            continue;
        };
        let src_wire = r
            .module
            .wires
            .iter()
            .find(|w| w.target.node_id == val_input)
            .map(|w| w.source);
        input_sources.push((*chip_nid, src_wire));
    }

    assert_eq!(input_sources.len(), 3, "expected 3 chip instances, got {}", input_sources.len());
    for (nid, src) in &input_sources {
        assert!(src.is_some(), "chip instance {nid}'s value-input has no incoming wire");
    }
    let distinct: std::collections::HashSet<NodeId> =
        input_sources.iter().filter_map(|(_, s)| s.map(|p| p.node_id)).collect();
    assert_eq!(
        distinct.len(),
        3,
        "expected 3 distinct input sources, got {} (instances collapse to the same arg): {:?}",
        distinct.len(),
        input_sources
    );
}

/// Valid wire-endpoint IDs for `module`: its own nodes, its external
/// scope-captures, and the boundary (input/output/key) nodes of its direct
/// child chips — since parent wires legitimately cross into a chip's I/O.
fn valid_endpoints(module: &Module) -> std::collections::HashSet<NodeId> {
    let mut ok: std::collections::HashSet<NodeId> = module.nodes.keys().cloned().collect();
    ok.extend(module.scope_captures.iter().cloned());
    for (chip_key, child) in &module.chips {
        ok.insert(*chip_key);
        ok.extend(child.inputs.iter().cloned());
        ok.extend(child.outputs.iter().cloned());
    }
    ok
}

/// Recursively collect wires whose endpoints reference a node that exists
/// nowhere reachable from the module — i.e. a dangling wire left behind when
/// template instantiation failed to remap an endpoint.
fn dangling_wires(module: &Module, path: &str, acc: &mut Vec<String>) {
    let ok = valid_endpoints(module);
    for w in &module.wires {
        if !ok.contains(&w.source.node_id) {
            acc.push(format!("{path}: wire SOURCE {} references a non-existent node", w.source.node_id));
        }
        if !ok.contains(&w.target.node_id) {
            acc.push(format!("{path}: wire TARGET {} references a non-existent node", w.target.node_id));
        }
    }
    for (chip_key, child) in &module.chips {
        let name = crate::intern::resolve(child.name);
        dangling_wires(child, &format!("{path} > chip {name}#{chip_key}"), acc);
    }
}

/// A chip call nested inside an inline `mod` that is expanded multiple times.
/// Each expansion must re-instantiate the embedded chip AND keep the
/// parent<->chip boundary wires (arg -> uid input, output -> reader) pointing
/// at the fresh chip instance. If instantiation remaps the chip's internal IDs
/// but not the parent wires that cross into it, those wires dangle: rows 2..N
/// lose their `uid` feed, every find() reads the default, and all calls
/// collapse to index 0 (the 2raab "everyone on slot 0" bug).
#[test]
fn chip_nested_in_repeated_inline_mod_keeps_boundary_wires() {
    // `slotOf` is an inline mod wrapping a chip call; it is expanded 3x with
    // distinct args, mirroring the unrolled rosterRow(g, 0..N) pattern.
    let src = "\
array userIds: string[]\n\
out ui: string[] = userIds\n\
chip slotOfUser(uid: string) -> int {\n\
  let res = userIds.find(uid)\n\
  return if res.Found then res.Index else -1\n\
}\n\
mod slotOf(uid: string) -> int {\n\
  return slotOfUser(uid)\n\
}\n\
in z: exec\n\
on z {\n\
  let s0 = slotOf(\"aaa\")\n\
  let s1 = slotOf(\"bbb\")\n\
  let s2 = slotOf(\"ccc\")\n\
  BroadcastChatMessage(\"${s0} ${s1} ${s2}\")\n\
}\n";
    let r = compile(&src);
    assert!(
        !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect::<Vec<_>>()
    );

    let mut dangles = Vec::new();
    dangling_wires(&r.module, "nested", &mut dangles);
    assert!(
        dangles.is_empty(),
        "dangling parent<->chip boundary wires after inline-mod expansion:\n{}",
        dangles.join("\n")
    );

    let mut acc = Vec::new();
    unwired_outputs(&r.module, "nested", &mut acc);
    assert!(acc.is_empty(), "unwired outputs:\n{}", acc.join("\n"));
}

/// The real 2raab rosterRow shape: an inline mod expanded N times passes a
/// value *derived from its own param* (an array read `arr[i]`) into the chip,
/// rather than the param directly. Each expansion must recompute that derived
/// arg and feed a distinct source into its own chip instance. If the derived
/// arg gate is frozen/shared across expansions, every chip instance searches
/// the same key -> everyone collapses to slot 0.
#[test]
fn chip_arg_derived_from_mod_param_in_repeated_mod() {
    let src = "\
array userIds: string[]\n\
out ui: string[] = userIds\n\
chip slotOfUser(uid: string) -> int {\n\
  let res = userIds.find(uid)\n\
  return if res.Found then res.Index else -1\n\
}\n\
mod rosterRow(i: int, count: int) -> int {\n\
  if i >= count { return -1 }\n\
  let key = userIds[i]\n\
  return slotOfUser(key)\n\
}\n\
in z: exec\n\
on z {\n\
  let count = userIds.length()\n\
  let a = rosterRow(0, count)\n\
  let b = rosterRow(1, count)\n\
  let c = rosterRow(2, count)\n\
  BroadcastChatMessage(\"${a} ${b} ${c}\")\n\
}\n";
    let r = compile(&src);
    assert!(
        !r.diagnostics.iter().any(|d| d.severity == Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect::<Vec<_>>()
    );

    let mut dangles = Vec::new();
    dangling_wires(&r.module, "derived", &mut dangles);
    assert!(dangles.is_empty(), "dangling wires:\n{}", dangles.join("\n"));

    // Each of the 3 chip instances must have its `uid` input fed by a DISTINCT
    // source (the per-row array read), not collapsed to one.
    let mut input_sources: Vec<Option<NodeId>> = Vec::new();
    for (_chip_nid, child) in &r.module.chips {
        let Some(&val_input) = child.inputs.first() else { continue };
        let src = r.module.wires.iter().find(|w| w.target.node_id == val_input).map(|w| w.source.node_id);
        input_sources.push(src);
    }
    assert_eq!(input_sources.len(), 3, "expected 3 chip instances, got {}", input_sources.len());
    for s in &input_sources {
        assert!(s.is_some(), "a chip instance's uid input has no incoming wire");
    }
    let distinct: std::collections::HashSet<NodeId> = input_sources.iter().filter_map(|s| *s).collect();
    assert_eq!(
        distinct.len(),
        3,
        "chip uid inputs collapsed to {} distinct source(s) across 3 rows (expected 3): {:?}",
        distinct.len(),
        input_sources
    );
}
