//! `emit` as a value and `.exec` on a call: the two halves of forking an exec
//! chain. One exec output driving two exec inputs runs both, so a fork is a
//! fan-out from the chain point the caller is already standing on.

use super::*;

/// The port feeding the `Exec` pin of the `Exec_Var_Set` that writes the
/// `Pseudo_Var` labelled `label`. Two writes sharing one source port ARE the
/// fork; a write whose source is another write's `ExecOut` is a sequence.
fn write_exec_source(r: &LowerResult, label: &str) -> PortRef {
    let set = var_set_writing(r, label);
    r.module
        .wires
        .iter()
        .find(|w| w.target == set.port(WirePort::Exec))
        .unwrap_or_else(|| panic!("no exec wire into the `{label}` write"))
        .source
}

/// The single `Exec_Var_Set` whose `VarRef` comes from the `Pseudo_Var`
/// labelled `label`.
fn var_set_writing(r: &LowerResult, label: &str) -> crate::ir::NodeId {
    let var = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.gate_class == gc::PSEUDO_VAR
                && matches!(
                    n.properties.get(&crate::intern::sym::NAME_LABEL),
                    Some(Literal::String(s)) if s == label
                )
        })
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("no `Pseudo_Var` labelled `{label}`"));
    let sets: Vec<crate::ir::NodeId> = r
        .module
        .wires
        .iter()
        .filter(|w| w.source.node_id == var && w.target.port == WirePort::VarRef)
        .map(|w| w.target.node_id)
        .filter(|id| r.module.nodes[id].gate_class == gc::VAR_SET)
        .collect();
    assert_eq!(sets.len(), 1, "expected one write of `{label}`, got {sets:?}");
    sets[0]
}

fn assert_no_placeholder(r: &LowerResult) {
    assert!(
        !has_gate(r, crate::ir::gate_class::UNSUPPORTED),
        "lowering fell to an `_Unsupported` placeholder: {:?}",
        r.diagnostics
    );
    assert!(
        r.diagnostics.iter().all(|d| d.code != "WSP001"),
        "unexpected WSP001: {:?}",
        r.diagnostics
    );
}

const LATER: &str = "\
var m: int = 0
var p: int = 0
var q: int = 0
mod later() {
  m = 7
}
in start: exec
";

#[test]
fn call_exec_binds_without_advancing_the_chain() {
    let r = compile(&format!("{LATER}on start {{\n  let d = later().exec\n  p = 1\n}}"));
    assert_no_errors(&r);
    assert_no_placeholder(&r);
    assert_eq!(
        write_exec_source(&r, "m"),
        write_exec_source(&r, "p"),
        "`.exec` must leave the chain where it was, so the next statement \
         takes the SAME exec source the call did"
    );
}

#[test]
fn an_ordinary_call_still_sequences() {
    let r = compile(&format!("{LATER}on start {{\n  later()\n  p = 1\n}}"));
    assert_no_errors(&r);
    assert_no_placeholder(&r);
    let m_set = var_set_writing(&r, "m");
    assert_ne!(
        write_exec_source(&r, "m"),
        write_exec_source(&r, "p"),
        "a call without `.exec` splices into the chain"
    );
    assert_eq!(
        write_exec_source(&r, "p"),
        m_set.port(WirePort::ExecOut),
        "the statement after an ordinary call runs off the call's completion"
    );
}

#[test]
fn awaiting_a_forked_call_rejoins_downstream_of_it() {
    let r = compile(&format!(
        "{LATER}on start {{\n  let d = later().exec\n  p = 1\n  await d\n  q = 2\n}}"
    ));
    assert_no_errors(&r);
    assert_no_placeholder(&r);
    let m_set = var_set_writing(&r, "m");
    let q_set = var_set_writing(&r, "q");
    assert!(
        wired_reachable(&r, m_set, q_set),
        "the awaited continuation must be downstream of the forked call"
    );
    assert!(
        !wired_reachable(&r, m_set, var_set_writing(&r, "p")),
        "the forked branch must not drive the statement after the fork"
    );
}

#[test]
fn a_discarded_fork_still_emits_the_call() {
    let r = compile(&format!("{LATER}on start {{\n  let _ = later().exec\n  p = 1\n}}"));
    assert_no_errors(&r);
    assert_no_placeholder(&r);
    assert_eq!(
        write_exec_source(&r, "m"),
        write_exec_source(&r, "p"),
        "`let _ =` only discards the binding; the call still forks and fires"
    );
}

#[test]
fn emit_as_a_value_is_the_current_chain_point() {
    let r = compile(
        "var p: int = 0\nin start: exec\non start {\n  p = 1\n  let d = SleepTicks(emit, delay = 2)\n  await d\n}",
    );
    assert_no_errors(&r);
    assert_no_placeholder(&r);
    let buffer = find_gate(&r, gc::BUFFER_TICKS);
    let src = r
        .module
        .wires
        .iter()
        .find(|w| w.target == buffer.port(WirePort::Input))
        .expect("the delayed value must be wired in")
        .source;
    assert_eq!(
        src,
        var_set_writing(&r, "p").port(WirePort::ExecOut),
        "`emit` names the exec chain point the statement before it left"
    );
}

#[test]
fn a_forked_chip_call_shares_the_chain_point_and_rejoins_through_its_exec_out() {
    // A physical microchip crosses the boundary through rerouter pins rather
    // than inlining, so its fork is a separate path from the inlined mod above.
    let r = compile(
        "var m: int = 0\n\
         var p: int = 0\n\
         chip Later() -> (count: int) {\n\
         m = m + 1\n\
         out count = m.Value\n\
         }\n\
         in start: exec\n\
         on start {\n\
         let d = Later().exec\n\
         p = 1\n\
         await d\n\
         }",
    );
    assert_no_errors(&r);
    assert_no_placeholder(&r);
    let (chip_node, body) = r.module.chips.iter().next().expect("one chip instance");
    // The auto-exec pins by their role inside the body: the input whose value
    // drives an `Exec` pin, and the output an `ExecOut` feeds. Positional
    // lookup would pick up a captured-variable boundary pin instead, since
    // those are appended after the exec pair.
    let exec_in = body
        .inputs
        .iter()
        .copied()
        .find(|id| {
            body.wires.iter().any(|w| {
                w.source == id.port(WirePort::RerOutput) && w.target.port == WirePort::Exec
            })
        })
        .expect("the chip body's exec entry pin");
    let exec_out = body
        .outputs
        .iter()
        .copied()
        .find(|id| {
            body.wires.iter().any(|w| {
                w.target == id.port(WirePort::RerInput) && w.source.port == WirePort::ExecOut
            })
        })
        .expect("the chip body's exec completion pin");
    let entry = write_exec_source(&r, "p");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.target == exec_in.port(WirePort::RerInput) && w.source == entry),
        "the chip's exec pin and the next statement must share one exec source \
         (chip {chip_node:?})"
    );
    let armed_get = r
        .module
        .nodes
        .iter()
        .find(|(_, n)| n.gate_class == gc::VAR_GET)
        .map(|(id, _)| *id)
        .expect("the await reads its armed flag");
    assert_eq!(
        r.module
            .wires
            .iter()
            .find(|w| w.target == armed_get.port(WirePort::Exec))
            .map(|w| w.source),
        Some(exec_out.port(WirePort::RerOutput)),
        "`await` on the forked binding resumes off the chip's completion exec"
    );
}
