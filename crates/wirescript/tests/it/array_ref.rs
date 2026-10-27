use wirescript::ir::{Module, gate_class};
use wirescript::{FoldMode, FsLoader, Severity};

use crate::common;

/// Gate count of `class` in `m`, including nested chip modules.
fn count_gate_class(m: &Module, class: &str) -> usize {
    let mut n = 0;
    m.walk(&mut |_, sub| n += sub.nodes.values().filter(|x| x.gate_class == class).count());
    n
}

/// `var arr: T[] = build(...)` copies the array the call returns: the callee's
/// pushes run, then a `CopyFrom` fills `arr`, with no warning.
#[test]
fn array_var_initialized_from_a_mod_call_copies_the_result() {
    let src = "\
mod build(n: int) -> int[] {
  var t: int[]
  if n >= 1 { t.push(10) }
  if n >= 2 { t.push(20) }
  return t
}
in go: exec
on go {
  var r: int[] = build(2)
  BroadcastChatMessage(\"\" .. r.length())
}";
    let wirescript::FrontEnd {
        resolved,
        typecheck: tc,
        lowered,
        ..
    } = wirescript::front_end(src, "test.ws", None, FoldMode::ForceOff, &FsLoader);
    assert!(
        resolved
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "resolve errors: {:?}",
        resolved.diagnostics
    );
    assert!(
        tc.diagnostics.iter().all(|d| d.severity != Severity::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    assert!(
        lowered.diagnostics.is_empty(),
        "unexpected lower diagnostics: {:?}",
        lowered.diagnostics
    );
    assert_eq!(count_gate_class(&lowered.module, gate_class::ARRAY_PUSH), 2);
    assert_eq!(
        count_gate_class(&lowered.module, gate_class::ARRAY_COPY_FROM),
        1
    );
}

/// A method on a container read out of a map (`m.get(k).Value.push(v)`) acts
/// on that container: the map read's value is the method gate's ref. A field
/// path from a call or index is a runtime value, not a name to resolve.
#[test]
fn method_on_a_container_read_from_a_map() {
    let src = "var row: int[]\nvar m: Map<string, int[]>\nvar mm: Map<string, Map<int, int>>\nvar inner: Map<int, int>\nin go: exec\nvar n: int = 0\non go {\n  m.set(\"a\", row)\n  m.get(\"a\").Value.push(9)\n  n = m.get(\"a\").Value.length()\n  mm.set(\"b\", inner)\n  mm.get(\"b\").Value.set(1, 2)\n}\n";
    let (_, tc, lowered) = common::run_stages(src, "test", FoldMode::Auto);
    assert!(tc.diagnostics.is_empty(), "{:?}", tc.diagnostics);
    assert!(
        lowered
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "{:?}",
        lowered.diagnostics
    );
    let m = lowered.module;
    let class = |id| m.nodes[&id].gate_class;
    let fed_by_map_read = |gate: &str, port: &str| {
        m.wires.iter().any(|w| {
            class(w.target.node_id) == gate
                && w.target.port.as_str() == port
                && class(w.source.node_id) == gate_class::MAP_GET
        })
    };
    assert!(fed_by_map_read(
        "BrickComponentType_Exec_ArrayVar_Push",
        "ArrayVarRef"
    ));
    assert!(fed_by_map_read(
        "BrickComponentType_Exec_ArrayVar_GetLength",
        "ArrayVarRef"
    ));
    assert!(fed_by_map_read(gate_class::MAP_SET, "MapVarRef"));
}
