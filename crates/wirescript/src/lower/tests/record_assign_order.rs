//! A whole-record assignment is a parallel assignment: every source value is
//! read before any target field or column is written, and the sources are
//! evaluated in source order. A handler-local record var with no initializer
//! resets to its zero value every run, as a plain scalar `var` does.

use super::*;
use crate::ir::NodeId;
use crate::ir::gate_class as gc;

/// The `NAME_LABEL` of a storage gate, if it has one.
fn label(r: &LowerResult, node: NodeId) -> Option<String> {
    match r.module.nodes[&node]
        .properties
        .get(&crate::intern::sym::NAME_LABEL)
    {
        Some(crate::ir::Literal::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The label of the storage gate `node` reaches through `port`.
fn storage_via(r: &LowerResult, node: NodeId, port: WirePort) -> String {
    source_of(r, node, port)
        .and_then(|s| label(r, s))
        .unwrap_or_else(|| {
            panic!(
                "{:?} reaches no labelled storage through {port:?}",
                r.module.nodes[&node]
            )
        })
}

/// The one exec chain the handler runs, in order, from the `go` input.
fn exec_chain(r: &LowerResult) -> Vec<NodeId> {
    let start = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::MICROCHIP_INPUT)
        .expect("an exec input")
        .id;
    let mut chain = vec![start];
    let mut at = start;
    loop {
        let next: Vec<NodeId> = r
            .module
            .wires
            .iter()
            .filter(|w| w.source.node_id == at && w.target.port == WirePort::Exec)
            .map(|w| w.target.node_id)
            .collect();
        match next.as_slice() {
            [] => return chain,
            [n] => {
                chain.push(*n);
                at = *n;
            }
            _ => panic!("the exec chain forks at {:?}", r.module.nodes[&at]),
        }
    }
}

fn pos(chain: &[NodeId], node: NodeId) -> usize {
    chain
        .iter()
        .position(|n| *n == node)
        .unwrap_or_else(|| panic!("{node:?} is not on the exec chain"))
}

/// Every gate of `class` wired to the storage labelled `storage` through
/// `port`.
fn ops_on(r: &LowerResult, class: &str, port: WirePort, storage: &str) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .filter(|n| {
            source_of(r, n.id, port)
                .and_then(|s| label(r, s))
                .as_deref()
                == Some(storage)
        })
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// The single `Var_Set` writing the var labelled `var`.
fn setter(r: &LowerResult, var: &str) -> NodeId {
    match ops_on(r, gc::VAR_SET, WirePort::VarRef, var).as_slice() {
        [s] => *s,
        other => panic!("expected one Var_Set writing {var}, found {other:?}"),
    }
}

/// The gate feeding the `Value` of `node`.
fn value_of(r: &LowerResult, node: NodeId) -> NodeId {
    source_of(r, node, WirePort::Value).expect("the Value must be wired")
}

/// The first position on `chain` of any `Var_Set`, container write or
/// `CopyFrom`.
fn first_write(r: &LowerResult, chain: &[NodeId]) -> usize {
    chain
        .iter()
        .position(|n| {
            matches!(
                r.module.nodes[n].gate_class,
                gc::VAR_SET
                    | gc::ARRAY_CLEAR
                    | gc::ARRAY_PUSH
                    | gc::ARRAY_COPY_FROM
                    | gc::MAP_CLEAR
                    | gc::MAP_SET
                    | gc::MAP_COPY_FROM
            )
        })
        .expect("a write")
}

/// `dst` is written from a `Var_Get` of `src`, and that read runs before the
/// assignment's first write.
fn assert_read_before_writes(r: &LowerResult, dst: &str, src: &str) {
    let chain = exec_chain(r);
    let get = value_of(r, setter(r, dst));
    assert_eq!(
        r.module.nodes[&get].gate_class,
        gc::VAR_GET,
        "{dst} must be written from a Var_Get"
    );
    assert_eq!(
        storage_via(r, get, WirePort::VarRef),
        src,
        "{dst} must read {src}"
    );
    assert!(
        pos(&chain, get) < first_write(r, &chain),
        "the read of {src} for {dst} runs after a write"
    );
}

const P: &str = "type P = { x: int, y: int }\nvar p: P\nin go: exec\n";

#[test]
fn record_literal_swap_reads_both_fields_before_writing() {
    for body in [
        "p = { x: p.y, y: p.x }",
        "let t = { x: p.y, y: p.x }\n p = t",
        "let t = { x: p.y, y: p.x }\n p = { ...t }",
    ] {
        let r = compile_ok(&format!("{P}on go {{ {body} }}\n"));
        assert_read_before_writes(&r, "p.x", "p.y");
        assert_read_before_writes(&r, "p.y", "p.x");
    }
}

#[test]
fn tuple_swap_reads_both_positions_before_writing() {
    let r = compile_ok("var t: (int, int)\nin go: exec\non go { t = (t.1, t.0) }\n");
    assert_read_before_writes(&r, "t.0", "t.1");
    assert_read_before_writes(&r, "t.1", "t.0");
}

#[test]
fn nested_record_swap_reads_every_field_before_writing() {
    let r = compile_ok(
        "type P = { x: int, y: int }
type Q = { a: P, n: int }
var q: Q
in go: exec
on go { q = { a: { x: q.a.y, y: q.n }, n: q.a.x } }
",
    );
    assert_read_before_writes(&r, "q.a.x", "q.a.y");
    assert_read_before_writes(&r, "q.a.y", "q.n");
    assert_read_before_writes(&r, "q.n", "q.a.x");

    let c = compile_ok(
        "type P = { x: int, y: int }
type Q = { a: P, n: int }
var q: Q
in go: exec
on go { q = { a: { x: 1, y: 2 }, n: q.a.x } }
",
    );
    assert_read_before_writes(&c, "q.n", "q.a.x");
}

/// A record assignment that reads nothing it writes keeps each read next to
/// its write, exactly as before the parallel-assignment rule.
#[test]
fn non_aliasing_record_assign_reads_each_field_at_its_write() {
    for body in [
        "p = { x: q.y, y: q.x }",
        "p = q",
        "p = { ...q }",
        "p = { x: q.x, y: 1 }",
    ] {
        let r = compile_ok(&format!("{P}var q: P\non go {{ {body} }}\n"));
        let chain = exec_chain(&r);
        for field in ["p.x", "p.y"] {
            let set = setter(&r, field);
            let Some(src) = source_of(&r, set, WirePort::Value) else {
                continue;
            };
            if r.module.nodes[&src].gate_class == gc::VAR_GET {
                assert_eq!(
                    pos(&chain, src) + 1,
                    pos(&chain, set),
                    "{body}: {field}'s read moved"
                );
            }
        }
    }
}

/// Two containers one assignment copies or spreads into each other have no
/// value to read first, so the cycle is WS087 naming both: swapped
/// record-array columns (through a literal or a `let`), two rebuilds
/// spreading each other, and two map columns.
#[test]
fn a_container_cycle_is_ws087() {
    let pts =
        "type P = { x: int, y: int }\ntype H = { pts: P[], qs: P[] }\nvar h: H\nin go: exec\n";
    let rows = [
        (format!("{pts}on go {{ h = {{ pts: h.qs, qs: h.pts }} }}\n"), Some(("h.pts", "h.qs"))),
        (format!("{pts}on go {{ let t = {{ pts: h.qs, qs: h.pts }}\n h = t }}\n"), Some(("h.pts", "h.qs"))),
        ("type H = { xs: int[], ys: int[] }\nvar h: H\nin go: exec\non go { h = { xs: [...h.ys], ys: [...h.xs] } }\n".to_string(), None),
        (
            "type P = { x: int, y: int }\ntype H = { pm: Map<int, P>, qm: Map<int, P> }\nvar h: H\nin go: exec\n\
             on go { h = { pm: h.qm, qm: h.pm } }\n"
                .to_string(),
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (src, names) in &rows {
        let r = compile(src);
        let Some(d) = r
            .diagnostics
            .iter()
            .find(|d| d.code == "WS087" && d.severity == crate::diagnostic::Severity::Error)
        else {
            failures.push(format!(
                "expected the error WS087, got {:?}\n{src}",
                r.diagnostics
            ));
            continue;
        };
        if let Some((a, b)) = names {
            if !d.message.contains(a) || !d.message.contains(b) {
                failures.push(format!(
                    "WS087 does not name {a} and {b}: {}\n{src}",
                    d.message
                ));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A column copied from a column the same assignment rebuilds copies the OLD
/// contents: the copy runs before the rebuild's `Clear`.
#[test]
fn copy_from_a_rebuilt_column_runs_before_the_rebuild() {
    let r = compile_ok(
        "type P = { x: int, y: int }
type H = { pts: P[], qs: P[] }
var h: H
in go: exec
on go { h = { pts: [{ x: 1, y: 2 }], qs: h.pts } }
",
    );
    let chain = exec_chain(&r);
    for col in ["x", "y"] {
        let copy = ops_on(
            &r,
            gc::ARRAY_COPY_FROM,
            WirePort::ArrayVarRef,
            &format!("h.qs.{col}"),
        );
        assert_eq!(copy.len(), 1);
        assert_eq!(
            storage_via(&r, copy[0], WirePort::SourceRef),
            format!("h.pts.{col}")
        );
        let clear = ops_on(
            &r,
            gc::ARRAY_CLEAR,
            WirePort::ArrayVarRef,
            &format!("h.pts.{col}"),
        );
        assert_eq!(clear.len(), 1);
        assert!(
            pos(&chain, copy[0]) < pos(&chain, clear[0]),
            "h.qs.{col} copied after h.pts.{col} was cleared"
        );
    }
}

/// A rebuild that spreads an array the same assignment rebuilds runs first,
/// so it appends the old contents.
#[test]
fn rebuild_spreading_a_rebuilt_array_runs_first() {
    let r = compile_ok(
        "type H = { xs: int[], ys: int[] }
var h: H
in go: exec
on go { h = { ys: [1], xs: [...h.ys] } }
",
    );
    let chain = exec_chain(&r);
    let append = ops_on(&r, gc::ARRAY_APPEND, WirePort::ArrayVarRef, "h.xs");
    assert_eq!(append.len(), 1);
    assert_eq!(storage_via(&r, append[0], WirePort::SourceRef), "h.ys");
    let clear = ops_on(&r, gc::ARRAY_CLEAR, WirePort::ArrayVarRef, "h.ys");
    assert!(
        pos(&chain, append[0]) < pos(&chain, clear[0]),
        "h.ys was cleared before h.xs appended it"
    );
}

/// A container literal's elements are evaluated in source order, before any
/// write: the first `pop` feeds `xs`, the second feeds `n`.
#[test]
fn container_literal_elements_evaluate_in_source_order() {
    let r = compile_ok(
        "type H = { xs: int[], n: int }
var h: H
var a: int[]
in go: exec
on go { h = { xs: [a.pop().Value], n: a.pop().Value } }
",
    );
    let chain = exec_chain(&r);
    let pops = ops_on(&r, gc::ARRAY_POP, WirePort::ArrayVarRef, "a");
    assert_eq!(pops.len(), 2);
    let (first, second) = if pos(&chain, pops[0]) < pos(&chain, pops[1]) {
        (pops[0], pops[1])
    } else {
        (pops[1], pops[0])
    };
    let push = ops_on(&r, gc::ARRAY_PUSH, WirePort::ArrayVarRef, "h.xs");
    assert_eq!(push.len(), 1);
    assert_eq!(value_of(&r, push[0]), first, "xs must take the first pop");
    assert_eq!(
        value_of(&r, setter(&r, "h.n")),
        second,
        "n must take the second pop"
    );
    assert!(
        pos(&chain, second) < first_write(&r, &chain),
        "every pop runs before the first write"
    );
}

/// A container literal that a later spread overrides is still evaluated, as
/// an overridden scalar field is, and the spread supplies the container.
#[test]
fn overridden_container_literal_is_still_evaluated() {
    let r = compile_ok(
        "type H = { xs: int[], n: int }
var h: H
var s: H
var a: int[]
in go: exec
on go { h = { xs: [a.pop().Value], ...s } }
",
    );
    assert_eq!(
        gate_count(&r, gc::ARRAY_POP),
        1,
        "the overridden element's pop must run"
    );
    let copy = ops_on(&r, gc::ARRAY_COPY_FROM, WirePort::ArrayVarRef, "h.xs");
    assert_eq!(copy.len(), 1);
    assert_eq!(storage_via(&r, copy[0], WirePort::SourceRef), "s.xs");
    assert!(ops_on(&r, gc::ARRAY_PUSH, WirePort::ArrayVarRef, "h.xs").is_empty());
}

/// A container element that reads a field the same assignment writes reads
/// the old value.
#[test]
fn container_element_reads_a_scalar_field_before_it_is_written() {
    let r = compile_ok(
        "type P = { x: int, y: int }
type H = { a: P, xs: int[] }
var h: H
in go: exec
on go { h = { a: { x: 5, y: 6 }, xs: [h.a.x] } }
",
    );
    let chain = exec_chain(&r);
    let push = ops_on(&r, gc::ARRAY_PUSH, WirePort::ArrayVarRef, "h.xs");
    assert_eq!(push.len(), 1);
    let get = value_of(&r, push[0]);
    assert_eq!(r.module.nodes[&get].gate_class, gc::VAR_GET);
    assert_eq!(storage_via(&r, get, WirePort::VarRef), "h.a.x");
    assert!(pos(&chain, get) < first_write(&r, &chain));
}

/// A handler-local record var with no initializer resets every field to its
/// zero value each run, before the body reads it.
#[test]
fn uninitialized_local_record_var_resets_every_run() {
    let r = compile_ok(
        "type P = { x: int, y: float }
var o: int = 0
in go: exec
on go {
  var p: P
  p.x = p.x + 1
  o = p.x
}
",
    );
    let chain = exec_chain(&r);
    let read = ops_on(&r, gc::VAR_GET, WirePort::VarRef, "p.x");
    assert!(!read.is_empty());
    for (field, zero) in [
        ("p.x", crate::ir::Literal::Int(0)),
        ("p.y", crate::ir::Literal::Float(0.0)),
    ] {
        let sets = ops_on(&r, gc::VAR_SET, WirePort::VarRef, field);
        let reset = sets
            .iter()
            .copied()
            .find(|s| pos(&chain, *s) < pos(&chain, read[0]))
            .unwrap_or_else(|| panic!("{field} is not reset before it is read"));
        assert!(
            source_of(&r, reset, WirePort::Value).is_none(),
            "{field}: the zero is a constant"
        );
        assert_eq!(
            r.module.nodes[&reset]
                .properties
                .get(&crate::intern::sym::VALUE),
            Some(&zero),
            "{field}"
        );
    }
}

/// A tuple, a nested record and an enum reset every leaf; an enum resets to
/// its first variant.
#[test]
fn uninitialized_local_tuple_nested_and_enum_vars_reset() {
    let r = compile_ok(
        "type P = { x: int, y: int }
type Q = { a: P, n: int }
enum S { Idle = 4, Run(int) }
in go: exec
on go {
  var t: (int, bool)
  var q: Q
  var s: S
}
",
    );
    let written = written_var_labels(&r);
    for leaf in [
        "t.0",
        "t.1",
        "q.a.x",
        "q.a.y",
        "q.n",
        "s.__disc",
        "s.__Run_0",
    ] {
        assert!(
            written.iter().any(|w| w == leaf),
            "{leaf} not reset: {written:?}"
        );
    }
    assert_eq!(
        written_value(&r, "s.__disc"),
        Some(crate::ir::Literal::Int(4))
    );
}

/// A `static` local record var keeps its value: no reset.
#[test]
fn uninitialized_static_local_record_var_is_not_reset() {
    let r = compile_ok(
        "type P = { x: int, y: int }\nin go: exec\non go { static var p: P\n p.x = p.x + 1 }\n",
    );
    assert!(ops_on(&r, gc::VAR_SET, WirePort::VarRef, "p.y").is_empty());
    assert_eq!(ops_on(&r, gc::VAR_SET, WirePort::VarRef, "p.x").len(), 1);
}

/// Every `Var_Get` on the chain runs before the first write.
fn assert_every_read_first(r: &LowerResult, what: &str) {
    let chain = exec_chain(r);
    let first = first_write(r, &chain);
    for n in &chain {
        if r.module.nodes[n].gate_class == gc::VAR_GET {
            assert!(pos(&chain, *n) < first, "{what}: a read runs after a write");
        }
    }
}

const BUMP: &str = "var q: int\nmod bump() -> int {\n  q = q + 10\n  return q\n}\nin go: exec\n";

/// A field naming storage is read in its place, before a later field's side
/// effect changes that storage.
#[test]
fn field_reads_before_a_later_side_effect() {
    let r = compile_ok(&format!(
        "type P = {{ x: int, y: int }}\nvar p: P\n{BUMP}on go {{ p = {{ y: q, x: bump() }} }}\n"
    ));
    let chain = exec_chain(&r);
    let bump = find_gate(&r, gc::VAR_INCREMENT);
    let get = value_of(&r, setter(&r, "p.y"));
    assert_eq!(storage_via(&r, get, WirePort::VarRef), "q");
    assert!(
        pos(&chain, get) < pos(&chain, bump),
        "p.y read q after bump() changed it"
    );

    let c = compile_ok(&format!(
        "type H = {{ xs: int[], n: int }}\nvar h: H\n{BUMP}on go {{ h = {{ xs: [q], n: bump() }} }}\n"
    ));
    let chain = exec_chain(&c);
    let push = ops_on(&c, gc::ARRAY_PUSH, WirePort::ArrayVarRef, "h.xs");
    let get = value_of(&c, push[0]);
    assert!(
        pos(&chain, get) < pos(&chain, find_gate(&c, gc::VAR_INCREMENT)),
        "xs read q after bump()"
    );
}

/// A field naming storage a later field's call writes reads the old value.
#[test]
fn field_reads_before_a_later_call_writes_it() {
    let r = compile_ok(
        "type P = { x: int, y: int }
var p: P
mod ty() -> int {
  p.y = p.y + 1
  return p.y
}
in go: exec
on go { p = { x: p.y, y: ty() } }
",
    );
    let chain = exec_chain(&r);
    let get = value_of(&r, setter(&r, "p.x"));
    assert_eq!(storage_via(&r, get, WirePort::VarRef), "p.y");
    for set in ops_on(&r, gc::VAR_SET, WirePort::VarRef, "p.y") {
        assert!(
            pos(&chain, get) < pos(&chain, set),
            "p.x read p.y after it was written"
        );
    }
}

/// A value built from a pure `let` that reads the target live is held before
/// the first write, so every field sees the old value.
#[test]
fn pure_let_reading_the_target_is_held_first() {
    for (src, fields) in [
        (
            "type V = { x: float, y: float }\nvar v: V\nlet len = sqrt(v.x * v.x + v.y * v.y)\nin go: exec\n\
             on go { v = { x: v.x / len, y: v.y / len } }\n",
            ["v.x", "v.y"],
        ),
        (
            "type P = { x: int, y: int }\nvar p: P\nlet s = p.x + p.y\nin go: exec\non go { p = { x: s, y: s } }\n",
            ["p.x", "p.y"],
        ),
    ] {
        let r = compile_ok(src);
        let chain = exec_chain(&r);
        let first = fields
            .iter()
            .map(|f| pos(&chain, setter(&r, f)))
            .min()
            .unwrap();
        for f in fields {
            let held = value_of(&r, setter(&r, f));
            assert_eq!(
                r.module.nodes[&held].gate_class,
                gc::VAR,
                "{src}: {f} must read a held value"
            );
            let hold = r
                .module
                .wires
                .iter()
                .find(|w| w.source.node_id == held && w.target.port == WirePort::VarRef)
                .map(|w| w.target.node_id)
                .expect("the held value is set");
            assert!(
                pos(&chain, hold) < first,
                "{src}: {f} is held after a write"
            );
        }
    }
}

/// Only true cycles are WS087: a copy onto its own storage is no write, and
/// sibling columns order column by column.
#[test]
fn acyclic_column_copies_are_ordered_not_reported() {
    let head = "type P = { x: int }\ntype I = { xs: P[], ys: P[] }\ntype L = { a: I, b: I }\nvar l: L\nvar q: int[]\nin go: exec\n";
    let r = compile_ok(&format!(
        "{head}on go {{ l = {{ a: {{ xs: l.a.xs, ys: l.b.xs }}, b: {{ xs: l.a.xs, ys: l.b.ys }} }} }}\n"
    ));
    let chain = exec_chain(&r);
    let into_a_ys = ops_on(&r, gc::ARRAY_COPY_FROM, WirePort::ArrayVarRef, "l.a.ys.x");
    let into_b_xs = ops_on(&r, gc::ARRAY_COPY_FROM, WirePort::ArrayVarRef, "l.b.xs.x");
    assert_eq!(
        gate_count(&r, gc::ARRAY_COPY_FROM),
        2,
        "no copy onto its own storage"
    );
    assert_eq!(
        storage_via(&r, into_a_ys[0], WirePort::SourceRef),
        "l.b.xs.x"
    );
    assert!(
        pos(&chain, into_a_ys[0]) < pos(&chain, into_b_xs[0]),
        "l.b.xs replaced before l.a.ys copied it"
    );

    for body in [
        "l = { a: { xs: l.b.xs, ys: [{ x: 1 }] }, b: { xs: [{ x: 2 }], ys: l.a.ys } }",
        "l = { a: { xs: l.b.xs, ys: [{ x: q.pop().Value }] }, b: { xs: [{ x: 2 }], ys: l.a.ys } }",
    ] {
        let r = compile_ok(&format!("{head}on go {{ {body} }}\n"));
        let chain = exec_chain(&r);
        for (dst, src) in [("l.a.xs.x", "l.b.xs.x"), ("l.b.ys.x", "l.a.ys.x")] {
            let copy = ops_on(&r, gc::ARRAY_COPY_FROM, WirePort::ArrayVarRef, dst);
            let clear = ops_on(&r, gc::ARRAY_CLEAR, WirePort::ArrayVarRef, src);
            assert!(
                pos(&chain, copy[0]) < pos(&chain, clear[0]),
                "{body}: {dst} copied after {src} was cleared"
            );
        }
    }
}

/// A cycle is reported without hiding the WS027 of a plain container field.
#[test]
fn container_cycle_keeps_other_diagnostics() {
    let r = compile(
        "type P = { x: int }
type I = { xs: int[], pts: P[], qs: P[] }
var h: I
var ys: int[]
in go: exec
on go { h = { pts: h.qs, qs: h.pts, xs: ys } }
",
    );
    crate::test_support::assert_code(&r.diagnostics, "WS087");
    crate::test_support::assert_code(&r.diagnostics, "WS027");
}

/// `xs = [...xs, ..]` keeps the old contents: no `Clear`, no append of itself.
#[test]
fn leading_self_spread_appends_in_place() {
    for (src, col) in [
        (
            "var xs: int[]\nin go: exec\non go { xs = [...xs, 1] }\n",
            "xs",
        ),
        (
            "type H = { xs: int[], n: int }\nvar h: H\nin go: exec\non go { h = { xs: [...h.xs, 1], n: 2 } }\n",
            "h.xs",
        ),
    ] {
        let r = compile_ok(src);
        assert!(
            ops_on(&r, gc::ARRAY_CLEAR, WirePort::ArrayVarRef, col).is_empty(),
            "{src}"
        );
        assert!(!has_gate(&r, gc::ARRAY_APPEND), "{src}");
        assert_eq!(
            ops_on(&r, gc::ARRAY_PUSH, WirePort::ArrayVarRef, col).len(),
            1,
            "{src}"
        );
    }
}

/// A self-spread anywhere but first would read the cleared array: WS088.
#[test]
fn misplaced_self_spread_is_reported() {
    for body in [
        "xs = [0, ...xs]",
        "xs = [...xs, ...xs]",
        "h = { xs: [1, ...h.xs] }",
    ] {
        let r = compile(&format!(
            "type H = {{ xs: int[] }}\nvar h: H\nvar xs: int[]\nin go: exec\non go {{ {body} }}\n"
        ));
        assert!(has_error(&r, "WS088"), "{body}: {:?}", r.diagnostics);
    }
}

/// An enum payload runs once in every form.
#[test]
fn enum_payload_is_evaluated_once() {
    for body in [
        "s = S.B(a.pop().Value)",
        "es.push(S.B(a.pop().Value))",
        "e = { es: [S.B(a.pop().Value)], k: 1 }",
    ] {
        let r = compile_ok(&format!(
            "enum S {{ A, B(int) }}\ntype E = {{ es: S[], k: int }}\nvar e: E\nvar es: S[]\nvar s: S\n\
             var a: int[]\nin go: exec\non go {{ {body} }}\n"
        ));
        assert_eq!(
            ops_on(&r, gc::ARRAY_POP, WirePort::ArrayVarRef, "a").len(),
            1,
            "{body}"
        );
    }
}

/// Aliases through a namespace, a mod and an if-expression read first too.
#[test]
fn aliases_through_namespace_mod_and_if_read_first() {
    let r = compile_multi(
        "import * as u from \"lib\"\nin go: exec\non go { u.p = { x: u.p.y, y: u.p.x } }\n",
        &[("lib", "type P = { x: int, y: int }\nvar p: P\n")],
    );
    assert_no_errors(&r);
    assert_every_read_first(&r, "namespace");
    for body in [
        "p = swapped(p)",
        "p = if c then { x: p.y, y: p.x } else p",
        "p = { x: gy(), y: p.x }",
    ] {
        let r = compile_ok(&format!(
            "{P}var c: bool\nmod swapped(q: P) -> P {{ return {{ x: q.y, y: q.x }} }}\n\
             mod gy() -> int {{ return p.y }}\non go {{ {body} }}\n"
        ));
        assert_every_read_first(&r, body);
    }
}

/// With nothing to reorder, a container element is still evaluated during its
/// rebuild, exactly as before.
#[test]
fn non_aliasing_container_element_keeps_its_place() {
    let r = compile_ok(
        "type H = { xs: int[], n: int }\nvar h: H\nvar k: int\nin go: exec\non go { h = { xs: [k], n: 1 } }\n",
    );
    let chain = exec_chain(&r);
    let get = find_gate(&r, gc::VAR_GET);
    assert!(pos(&chain, find_gate(&r, gc::ARRAY_CLEAR)) < pos(&chain, get));
}

/// How many `Var_Set`s in `m` write a value held in a hidden `_` var: a
/// `Var_Set` whose `Value` is such a var's `Value`.
fn held_writes(m: &crate::ir::Module) -> usize {
    m.wires
        .iter()
        .filter(|w| w.target.port == WirePort::Value && w.source.port == WirePort::Value)
        .filter(|w| {
            m.nodes
                .get(&w.target.node_id)
                .is_some_and(|n| n.gate_class == gc::VAR_SET)
        })
        .filter(|w| {
            m.nodes.get(&w.source.node_id).is_some_and(|n| {
                n.gate_class == gc::VAR
                    && matches!(n.properties.get(&crate::intern::sym::NAME_LABEL),
                        Some(crate::ir::Literal::String(s)) if s.starts_with('_'))
            })
        })
        .count()
}

/// A pure `let` read through a named chip's boundary is held before the
/// first write, whether the `let` sits outside the chip, inside it, or
/// behind a chip's output.
#[test]
fn pure_let_across_a_chip_boundary_is_held_first() {
    for src in [
        "type P = { x: int, y: int }\nvar p: P\nlet s = p.x + p.y\n\
         chip Doit(go: exec) {\n  on go {\n    p = { x: s, y: s }\n  }\n}\nin g: exec\nDoit(g)\n",
        "type P = { x: int, y: int }\nvar p: P\n\
         chip Doit(go: exec) {\n  let s = p.x + p.y\n  on go {\n    p = { x: s, y: s }\n  }\n}\nin g: exec\nDoit(g)\n",
    ] {
        let r = compile_ok(src);
        let chip = r.module.chips.values().next().expect("the Doit chip");
        assert_eq!(held_writes(chip), 2, "{src}");
    }
    let r = compile_ok(
        "type P = { x: int, y: int }
var p: P
chip Sum(a: int, b: int) -> int {
  return a + b
}
let s2 = Sum(p.x, p.y)
in go: exec
on go { p = { x: s2, y: s2 } }
",
    );
    assert_eq!(held_writes(&r.module), 2);
}

/// A chip output that reads nothing the assignment writes is not held.
#[test]
fn chip_output_reading_nothing_written_is_not_held() {
    let r = compile_ok(
        "type P = { a: int, b: int }\nin go: exec\nin u: int\nvar q: P = { a: 0, b: 0 }\n\
         chip Mk(x: int) -> (p: P) { out p = { a: x, b: x } }\non go { q = Mk(u) }\n",
    );
    assert_eq!(held_writes(&r.module), 0);
    assert_eq!(written_var_labels(&r), ["q.a", "q.b"]);
}

/// A container copied or spread when its field is written can't see a later
/// field's call that may change it first: WS089 by any route (a clear, `pop`,
/// alias, element, ref, or a name the mod writes). A call placed before the
/// copy, or one changing only another container, is fine.
#[test]
fn a_container_changed_by_a_later_call_is_ws089() {
    let h = "type P = { x: int }\ntype H = { ps: P[], n: int }\ntype G = { xs: int[], n: int }\n\
             var h: H\nvar g: G\nvar src: P[]\nvar ys: int[]\nvar other: int[]\n\
             mod popper() -> int {\n  src.clear()\n  return 1\n}\nin go: exec\n";
    let drains = "type H = { xs: int[], n: int }\nvar h: H\nvar ys: int[]\n\
                  mod drain() -> int {\n  ys.clear()\n  return 1\n}\n\
                  mod drainp(a: int[]) -> int {\n  a.clear()\n  return 1\n}\nin go: exec\n";
    let routes = "type H = { xs: int[], n: int }\nvar h: H\nvar row0: int[]\nvar rows: int[][]\nvar cur: *int[]\n\
                  mod f() -> int {\n  rows[0].push(1)\n  return 5\n}\n\
                  mod g(xs: int[]) -> int {\n  xs.push(1)\n  return 5\n}\n\
                  mod k() -> int {\n  cur.push(1)\n  return 5\n}\n\
                  mod l() -> int {\n  let a = row0\n  a.push(1)\n  return 5\n}\nin go: exec\n";
    let mut rows: Vec<(String, bool)> = vec![
        (
            format!("{h}on go {{ h = {{ ps: src, n: popper() }} }}\n"),
            true,
        ),
        (
            format!("{h}on go {{ g = {{ xs: [...ys], n: ys.pop().Value }} }}\n"),
            true,
        ),
        (
            format!("{h}on go {{ h = {{ n: popper(), ps: src }} }}\n"),
            false,
        ),
        (
            format!("{h}on go {{ g = {{ xs: [...ys], n: other.pop().Value }} }}\n"),
            false,
        ),
        (
            format!("{drains}on go {{ h = {{ xs: [...ys], n: drain() }} }}\n"),
            true,
        ),
        (
            format!("{drains}on go {{ h = {{ xs: [...ys], n: drainp(ys) }} }}\n"),
            true,
        ),
    ];
    for (local, call) in [
        ("", "f()"),
        ("", "g(cur)"),
        ("", "k()"),
        ("", "g(rows[0])"),
        ("", "l()"),
        ("var a: int[]\n ", "l()"),
    ] {
        rows.push((
            format!("{routes}on go {{ {local}h = {{ xs: [...row0], n: {call} }} }}\n"),
            true,
        ));
    }
    let mut failures = Vec::new();
    for (src, want) in &rows {
        let r = compile(src);
        if r.diagnostics.iter().any(|d| d.code == "WS089") != *want {
            failures.push(format!("WS089 expected {want}: {:?}\n{src}", r.diagnostics));
        }
    }
    let ns = compile_multi(
        "import * as L from \"lib\"\ntype H = { xs: int[], n: int }\nvar h: H\n\
         mod pushL() -> int {\n  L.arr.push(1)\n  return 5\n}\nin go: exec\non go { h = { xs: [...L.arr], n: pushL() } }\n",
        &[("lib", "var arr: int[]\n")],
    );
    if !ns.diagnostics.iter().any(|d| d.code == "WS089") {
        failures.push(format!("namespace: WS089 expected: {:?}", ns.diagnostics));
    }
    crate::test_support::report_failures(&failures, rows.len() + 1);
}

/// A record literal anywhere reads a field naming storage before a later
/// field's side effect: an element write, a push, a map set, a `let`, a call
/// argument and a record-valued `if`.
#[test]
fn record_literal_field_is_read_before_a_later_side_effect() {
    let head = "type P = { x: int, y: int }\nvar p: P\nvar ps: P[]\nvar m: Map<int, P>\nvar c: bool\n\
                mod take(r: P) -> int {\n  return r.x\n}\n";
    for body in [
        "ps[0] = { x: q, y: bump() }",
        "ps.push({ x: q, y: bump() })",
        "m[1] = { x: q, y: bump() }",
        "let r = { x: q, y: bump() }\n p = r",
        "p.x = take({ x: q, y: bump() })",
    ] {
        let r = compile_ok(&format!("{head}{BUMP}on go {{ {body} }}\n"));
        let chain = exec_chain(&r);
        let bump = pos(&chain, find_gate(&r, gc::VAR_INCREMENT));
        let read_first = ops_on(&r, gc::VAR_GET, WirePort::VarRef, "q")
            .into_iter()
            .filter(|g| pos(&chain, *g) < bump)
            .any(|g| {
                r.module
                    .wires
                    .iter()
                    .any(|w| w.source.node_id == g && w.source.port == WirePort::Value)
            });
        assert!(read_first, "{body}: x must read q before bump() runs");
    }
    // A record chosen by an `if` has no single store to run a branch's write
    // into on its branch alone, so a branch that writes is WS097 rather than a
    // `bump()` that runs whichever branch is taken.
    let r = compile(&format!(
        "{head}{BUMP}on go {{ p = if c then {{ x: q, y: bump() }} else {{ x: 0, y: 0 }} }}
"
    ));
    crate::test_support::assert_code(&r.diagnostics, "WS097");
}

/// A multi-output mod or chip call as a record-literal field keeps every
/// output, each under its own name, and a tuple returned to a mod's named
/// outputs fills them in declaration order.
#[test]
fn multi_output_call_field_keeps_every_output() {
    for decl in [
        "mod pairmk(px: string) -> (found: bool, val: string) {
  return (true, px)
}
",
        "chip pairmk(px: string) -> (found: bool, val: string) {
  out found = true
  out val = px
}
",
    ] {
        let r = compile_ok(&format!(
            "in s: string
var n: string = \"\"
{decl}in go: exec
             on go {{
  let r = {{ v: pairmk(s), w: 1 }}
  n = r.v.val
}}
"
        ));
        crate::test_support::assert_no_code(&r.diagnostics, "WSP001");
        let src = value_of(&r, setter(&r, "n"));
        let class = r.module.nodes.get(&src).map(|n| n.gate_class);
        assert!(
            matches!(class, None | Some(gc::MICROCHIP_INPUT)),
            "{decl}: n must read `val` (the input or the chip's output), got {class:?}"
        );
    }
    let r = compile_ok(
        "var o: int[]
mod six() -> (a: int, b: int, c: int, d: int, e: int, f: int) {
  return (1, 2, 3, 4, 5, 6)
}
         in go: exec
on go {
  let r = { v: six() }
  o = [r.v.a, r.v.b, r.v.c, r.v.d, r.v.e, r.v.f]
}
",
    );
    let chain = exec_chain(&r);
    let pushed: Vec<crate::ir::Literal> = chain
        .iter()
        .filter(|n| r.module.nodes[n].gate_class == gc::ARRAY_PUSH)
        .map(|n| {
            let holder = source_of(&r, *n, WirePort::Value).unwrap_or(*n);
            r.module.nodes[&holder].properties[&crate::intern::sym::VALUE].clone()
        })
        .collect();
    let want: Vec<crate::ir::Literal> = (1..=6).map(crate::ir::Literal::Int).collect();
    assert_eq!(pushed, want);
}

/// An `out` read in a record literal is read before a later field's call
/// changes what it reads.
#[test]
fn out_read_is_held_before_a_later_side_effect() {
    let head = "type T = { a: int, b: int }\nvar q: int = 0\nvar o2: T\nout oo = q * 2\n\
                mod bump() -> int {\n  q = q + 1\n  return 5\n}\nin go: exec\n";
    for body in [
        "let r = { a: oo, b: bump() }\n o2.a = r.a\n o2.b = r.b",
        "o2 = { a: oo, b: bump() }",
    ] {
        let r = compile_ok(&format!("{head}on go {{ {body} }}\n"));
        assert_eq!(held_writes(&r.module), 1, "{body}");
        let chain = exec_chain(&r);
        let bump = pos(&chain, find_gate(&r, gc::VAR_INCREMENT));
        let hold = chain
            .iter()
            .position(|n| {
                r.module.nodes[n].gate_class == gc::VAR_SET
                    && source_of(&r, *n, WirePort::VarRef)
                        .and_then(|v| label(&r, v))
                        .is_some_and(|l| l.starts_with('_'))
            })
            .expect("a hold");
        assert!(hold < bump, "{body}: `oo` is held after bump()");
    }
}

/// Each field is held once, before the first later call that may change it.
#[test]
fn a_field_is_held_once() {
    let r = compile_ok(
        "type T = { a: int, b: int, c: int }\nvar q: int = 0\nvar o: T\nlet l = q * 3\n\
         mod bump() -> int {\n  q = q + 1\n  return q\n}\nin go: exec\n\
         on go {\n  let r = { a: l, b: bump(), c: bump() }\n  o.a = r.a\n  o.b = r.b\n  o.c = r.c\n}\n",
    );
    let holds = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR && label(&r, n.id).is_some_and(|l| l.starts_with('_')))
        .count();
    assert_eq!(holds, 1);
    let s = compile_ok(&format!(
        "{P}let s = p.x + p.y\non go {{ p = {{ x: s, y: s }} }}\n"
    ));
    assert_eq!(
        held_writes(&s.module),
        2,
        "one hold of `s`, read by both fields"
    );
    let holds = s
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR && label(&s, n.id).is_some_and(|l| l.starts_with('_')))
        .count();
    assert_eq!(holds, 1);
}

/// A mod with several outputs and several `return (..)`s gives the caller the
/// values of the return that ran, not the first one written.
#[test]
fn multi_output_early_return_reaches_the_caller() {
    let r = compile_ok(
        "in go: exec\nin ia: int\nin bd: bool\nvar oa: int = 0\nvar ob: int = 0\n\
         mod g(a0: int, d0: bool) -> (zeta: int, alpha: int) {\n  if d0 { return (a0 + 1, 7) }\n  return (a0, 9)\n}\n\
         on go {\n  let (x, y) = g(ia, bd)\n  oa = x\n  ob = y\n}\n",
    );
    // Every output is stored by both returns, and the caller reads the storage.
    let alpha_sets: Vec<crate::ir::Literal> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_SET)
        .filter_map(|n| n.properties.get(&crate::intern::sym::VALUE).cloned())
        .collect();
    assert!(
        alpha_sets.contains(&crate::ir::Literal::Int(7))
            && alpha_sets.contains(&crate::ir::Literal::Int(9)),
        "both returns store `alpha`: {alpha_sets:?}"
    );
    let get = value_of(&r, setter(&r, "ob"));
    assert_eq!(
        r.module.nodes[&get].gate_class,
        gc::VAR_GET,
        "ob reads the return storage"
    );
}

/// A record-typed output among several is filled by a tuple return.
#[test]
fn tuple_return_fills_a_record_output_among_several() {
    let r = compile_ok(
        "type P = { x: int, y: int }\nin go: exec\nin ia: int\nin ib: int\nin fb: float\n\
         var oa: int = 0\nvar ox: int = 0\nvar ob: float = 0.0\n\
         mod f(a0: int, b0: int, f0: float) -> (a: int, p: P, b: float) {\n  return (a0, { x: b0, y: a0 }, f0)\n}\n\
         on go {\n  let (u, v, w) = f(ia, ib, fb)\n  oa = u\n  ox = v.x\n  ob = w\n}\n",
    );
    crate::test_support::assert_no_code(&r.diagnostics, "WSP001");
    let inputs: Vec<crate::ir::NodeId> = r.module.inputs.clone();
    let read: Vec<crate::ir::NodeId> = ["oa", "ox", "ob"]
        .iter()
        .map(|v| value_of(&r, setter(&r, v)))
        .collect();
    assert_eq!(
        read,
        inputs[1..4].to_vec(),
        "oa, ox, ob read ia, ib, fb in turn"
    );
}

/// The gate that fixes the value `setter` writes: the `Var_Get` it reads, or
/// the `Var_Set` of the hidden `_` var holding it.
fn fixed_by(r: &LowerResult, setter: NodeId) -> NodeId {
    let v = value_of(r, setter);
    match r.module.nodes[&v].gate_class {
        gc::VAR_GET => v,
        gc::VAR if label(r, v).is_some_and(|l| l.starts_with('_')) => r
            .module
            .wires
            .iter()
            .find(|w| {
                w.source.node_id == v
                    && w.target.port == WirePort::VarRef
                    && r.module.nodes[&w.target.node_id].gate_class == gc::VAR_SET
            })
            .map(|w| w.target.node_id)
            .expect("the held value is set"),
        other => panic!(
            "{:?} is written from a live {other}",
            r.module.nodes[&setter]
        ),
    }
}

/// Every `Var_Set` or `Var_Increment` writing the var labelled `var`, or with
/// `None` every one writing through a ref.
fn writes_of(r: &LowerResult, var: Option<&str>) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = r
        .module
        .nodes
        .values()
        .filter(|n| matches!(n.gate_class, gc::VAR_SET | gc::VAR_INCREMENT))
        .filter(|n| {
            let src = source_of(r, n.id, WirePort::VarRef).expect("a write names its storage");
            match var {
                Some(var) => label(r, src).as_deref() == Some(var),
                None => r
                    .module
                    .nodes
                    .get(&src)
                    .is_none_or(|s| s.gate_class != gc::VAR),
            }
        })
        .map(|n| n.id)
        .collect();
    out.sort();
    out
}

/// `o.a` is fixed before every write `writes` lists.
fn a_fixed_before(r: &LowerResult, writes: &[NodeId], what: &str) {
    assert_no_errors(r);
    assert!(!writes.is_empty(), "{what}: no write found");
    let fixed = fixed_by(r, setter(r, "o.a"));
    for w in writes {
        assert!(
            exec_reaches(&r.module, fixed, *w),
            "{what}: o.a is read after the call writes it"
        );
    }
}

const O: &str = "type T = { a: int, b: int }\nvar q: int = 0\nvar o: T\nin go: exec\n";

/// A field is read before a later field's call writes it, whatever route the
/// write takes: a namespace alias, a nested callee in another module, a
/// parameter a spread argument reaches, a ref value, a block-scoped name in
/// the callee.
#[test]
fn a_field_is_read_before_a_later_call_writes_it_by_any_route() {
    let lib = "var q: int = 0\nvar arr: int[]\nmod helper() -> int {\n  q = q + 1\n  return 5\n}\n\
               mod bump2() -> int {\n  return helper()\n}\n";
    for body in ["o = { a: L.q, b: bump() }", "o = { a: L.q, b: L.bump2() }"] {
        let r = compile_multi(
            &format!(
                "import * as L from \"lib\"\ntype T = {{ a: int, b: int }}\nvar o: T\n\
                 mod helper() -> int {{ return 5 }}\nmod bump() -> int {{\n  L.q = L.q + 1\n  return 5\n}}\nin go: exec\non go {{ {body} }}\n"
            ),
            &[("lib", lib)],
        );
        a_fixed_before(&r, &writes_of(&r, Some("q")), body);
    }
    let r = compile(&format!(
        "{O}mod two() -> (x: int, y: int) {{ return (1, 2) }}\n\
         mod setp(a0: int, b0: int, p: *int) -> int {{\n  p = p + a0 + b0\n  return 5\n}}\n\
         on go {{\n  let t = two()\n  o = {{ a: q, b: setp(...t, &q) }}\n}}\n"
    ));
    a_fixed_before(&r, &writes_of(&r, Some("q")), "spread arguments");
    for (decl, call) in [
        (
            "mod setp(p: *int) -> int {\n  p = p + 1\n  return 5\n}\n",
            "setp(h)",
        ),
        (
            "mod bumph() -> int {\n  *h = *h + 1\n  return 5\n}\n",
            "bumph()",
        ),
    ] {
        let r = compile(&format!(
            "{O}var h: *int\n{decl}on go {{\n  h = &q\n  o = {{ a: q, b: {call} }}\n}}\n"
        ));
        a_fixed_before(&r, &writes_of(&r, None), call);
    }
    let r = compile(&format!(
        "{O}in k: bool\nmod bump() -> int {{\n  if k {{\n    var q: int = 0\n    q = 3\n  }}\n  q = q + 1\n  return 5\n}}\n\
         on go {{ o = {{ a: q, b: bump() }} }}\n"
    ));
    a_fixed_before(&r, &[find_gate(&r, gc::VAR_INCREMENT)], "block-scoped var");
}

/// A call's argument is read before a later argument's call changes what it
/// reads.
#[test]
fn a_call_argument_is_read_before_a_later_argument_runs() {
    let r = compile_ok(
        "var q: int = 0\nvar oa: int = 0\nvar ob: int = 0\nlet l = q * 3\n\
         mod bump() -> int {\n  q = q + 1\n  return 5\n}\n\
         mod pair(a: int, b: int) -> (x: int, y: int) { return (a, b) }\nin go: exec\n\
         on go {\n  let (x, y) = pair(l, bump())\n  oa = x\n  ob = y\n}\n",
    );
    let fixed = fixed_by(&r, setter(&r, "oa"));
    assert!(
        exec_reaches(&r.module, fixed, find_gate(&r, gc::VAR_INCREMENT)),
        "`l` is read after bump()"
    );
}

/// A pure mod call, or a structural expression (`is`, `match`), next to a
/// copied container needs no hold and is no WS089: the program compiles.
#[test]
fn pure_calls_next_to_a_copied_container_add_no_gates() {
    let head = "enum Shape { Circle, Rect }\ntype P = { x: int }\ntype H = { xs: int[], n: int }\n\
                type G = { xs: int[], b: bool }\ntype R = { ps: P[], n: int }\ntype S = { xs: int[], n: int, m: int }\n\
                var h: H\nvar g: G\nvar r: R\nvar st: S\nvar st2: S\nvar src: P[]\nvar ys: int[]\n\
                var s: Shape = Shape.Circle\nmod count() -> int { return 3 }\nmod twice(v: int) -> int { return v * 2 }\n\
                mod calc(v: int) -> int { return v * 2 + 1 }\nin go: exec\nin k: int\n";
    for body in [
        "h = { xs: [...ys], n: count() }",
        "h = { xs: [...ys], n: twice(k) }",
        "g = { xs: [...ys], b: s is Shape.Rect }",
        "h = { xs: [...ys], n: match s { Shape.Circle => 1, Shape.Rect => 2 } }",
        "r = { ps: src, n: twice(k) }",
        "st = { ...st2, n: twice(k) }",
        "st = { ...st, n: twice(k) }",
        "st = { n: twice(k), ...st2 }",
        "st = { ...st, n: calc(k) }",
        "st = { ...st2, n: calc(k) }",
        "st = { n: calc(k), ...st2 }",
    ] {
        let r = compile_ok(&format!("{head}on go {{ {body} }}\n"));
        let hidden = r
            .module
            .nodes
            .values()
            .filter(|n| label(&r, n.id).is_some_and(|l| l.starts_with('_')))
            .count();
        assert_eq!(hidden, 0, "{body}");
    }
}

/// Every construct that evaluates several sources reads each one before a
/// later one runs code that changes what it reads: a chip's arguments, a
/// builtin's, an enum payload, a tuple.
#[test]
fn each_source_is_read_before_a_later_one_runs() {
    let head = "var q: int = 0\nvar oa: int = 0\nlet l = q * 3\nenum S { A, B(int, int) }\nvar s: S\n\
                mod bump() -> int {\n  q = q + 1\n  return 5\n}\nchip Sum(a: int, b: int) -> int {\n  return a + b\n}\n\
                in go: exec\n";
    for body in [
        "oa = Sum(l, bump())",
        "oa = min(l, bump())",
        "s = S.B(l, bump())",
        "let t = (l, bump())\n oa = t.0",
    ] {
        let r = compile_ok(&format!("{head}on go {{ {body} }}\n"));
        let held: Vec<NodeId> = r
            .module
            .nodes
            .values()
            .filter(|n| {
                n.gate_class == gc::VAR && label(&r, n.id).is_some_and(|l| l.starts_with('_'))
            })
            .map(|n| n.id)
            .collect();
        assert_eq!(held.len(), 1, "{body}: `l` is held once");
        let hold = ops_on(
            &r,
            gc::VAR_SET,
            WirePort::VarRef,
            &label(&r, held[0]).expect("labelled"),
        );
        assert!(
            exec_reaches(&r.module, hold[0], find_gate(&r, gc::VAR_INCREMENT)),
            "{body}: `l` is held after bump()"
        );
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.source == held[0].port(WirePort::Value)),
            "{body}: the held value is used"
        );
    }
}

/// The two `bump()` increments of `q`, first then second on the chain.
fn two_bumps(r: &LowerResult) -> (NodeId, NodeId) {
    let incs = writes_of(r, Some("q"));
    assert_eq!(incs.len(), 2, "two bumps");
    if exec_reaches(&r.module, incs[0], incs[1]) {
        (incs[0], incs[1])
    } else {
        (incs[1], incs[0])
    }
}

/// A source named again after a call is read again there, not reused from
/// before the call: each read sits between the two `bump()`s.
#[test]
fn a_source_named_after_a_call_is_read_after_it() {
    let head = "type F = { a: int, b: int, c: int, d: int }\nvar q: int = 0\nvar o: F\nvar oc: int = 0\n\
                let l = q * 3\nmod bump() -> int {\n  q = q + 1\n  return 5\n}\n\
                mod quad(a: int, b: int, c: int, d: int) -> (w: int, x: int) { return (a, c) }\nin go: exec\n";
    for (body, var) in [
        ("o = { a: q, b: bump(), c: q, d: bump() }", "o.c"),
        ("o = { a: l, b: bump(), c: l, d: bump() }", "o.c"),
        ("let (w, x) = quad(l, bump(), l, bump())\n oc = x", "oc"),
    ] {
        let r = compile_ok(&format!("{head}on go {{ {body} }}\n"));
        let (first, second) = two_bumps(&r);
        let c = fixed_by(&r, setter(&r, var));
        assert!(
            exec_reaches(&r.module, first, c),
            "{body}: c is read before the first bump()"
        );
        assert!(
            exec_reaches(&r.module, c, second),
            "{body}: c is read after the second bump()"
        );
    }
}

/// A call that emits a local signal may run that signal's handler, so a
/// source before it is held ahead of the emit.
#[test]
fn a_source_is_held_before_a_later_emit() {
    let head = "type T = { a: int, b: int }\nvar q: int = 0\nvar r: int = 0\nvar o: T\nlet l = q * 3\n\
                let sig: exec\non sig { q = q + 1 }\nin go: exec\n";
    for em in [
        "mod em() -> int {\n  emit sig\n  return 5\n}\n",
        "mod em() -> int {\n  r = r + 1\n  emit sig\n  return 5\n}\n",
    ] {
        for body in ["o = { a: q, b: em() }", "o = { a: l, b: em() }"] {
            let r = compile_ok(&format!("{head}{em}on go {{ {body} }}\n"));
            let handler = writes_of(&r, Some("q"));
            assert_eq!(handler.len(), 1);
            let a = fixed_by(&r, setter(&r, "o.a"));
            assert!(
                exec_reaches(&r.module, a, handler[0]),
                "{em}{body}: o.a is read after the emit"
            );
        }
    }
}

/// A loop over an array only reads it, so a call looping over the copied
/// array is no WS089.
#[test]
fn a_loop_over_the_copied_container_is_not_ws089() {
    compile_ok(
        "type H = { xs: int[], n: int }\nvar h: H\nvar row0: int[]\n\
         mod sum0() -> int {\n  var s: int = 0\n  for v in row0 { s = s + v }\n  return s\n}\n\
         in go: exec\non go { h = { xs: [...row0], n: sum0() } }\n",
    );
}

/// The hidden vars `l` is held in.
fn holds(r: &LowerResult) -> Vec<NodeId> {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR && label(r, n.id).is_some_and(|l| l.starts_with('_')))
        .map(|n| n.id)
        .collect()
}

/// Operands, container method arguments, interpolation slots, an if-expression's
/// condition and an index are each read before a later one runs code that
/// changes what they read; named arguments are evaluated in source order.
#[test]
fn every_construct_evaluates_in_source_order() {
    let head = "var q: int = 0\nvar oi: int = 0\nvar os: string = \"\"\nvar xs: int[]\nvar m: Map<int, int>\n\
                let l = q * 3\nmod bump() -> int {\n  q = q + 1\n  return 1\n}\nin go: exec\n";
    for body in [
        "oi = l + bump()",
        "xs.insert(l, bump())",
        "m.set(l, bump())",
        "os = \"${l} ${bump()}\"",
        "xs[l] = bump()",
        "m[l] = bump()",
        "oi = min(a = l, b = bump())",
    ] {
        let r = compile_ok(&format!("{head}on go {{ {body} }}\n"));
        let held = holds(&r);
        assert_eq!(held.len(), 1, "{body}: `l` is held once");
        let hold = ops_on(
            &r,
            gc::VAR_SET,
            WirePort::VarRef,
            &label(&r, held[0]).expect("labelled"),
        );
        assert!(
            exec_reaches(&r.module, hold[0], find_gate(&r, gc::VAR_INCREMENT)),
            "{body}: `l` is held after bump()"
        );
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.source == held[0].port(WirePort::Value)),
            "{body}: unused hold"
        );
    }
    let r = compile_ok(&format!("{head}on go {{ oi = min(b = bump(), a = l) }}\n"));
    assert!(
        holds(&r).is_empty(),
        "`b = bump()` is written first, so `l` is read after it"
    );
    // An if-expression's condition is read by its Branch, and a branch that
    // writes runs only when taken, so nothing needs holding.
    let r = compile_ok(&format!(
        "{head}on go {{ oi = if l > 3 then bump() else 0 }}\n"
    ));
    let held: Vec<_> = holds(&r)
        .into_iter()
        .filter(|n| label(&r, *n).as_deref() != Some("_if"))
        .collect();
    assert!(
        held.is_empty(),
        "the Branch reads `l > 3` before bump() can run"
    );
    let branch = find_gate(&r, gc::BRANCH);
    let inc = find_gate(&r, gc::VAR_INCREMENT);
    let start = |port: WirePort| {
        r.module
            .wires
            .iter()
            .find(|w| w.source == branch.port(port))
            .map(|w| w.target.node_id)
            .expect("a branch chain")
    };
    assert!(
        exec_reaches(&r.module, start(WirePort::ExecOutA), inc),
        "bump() runs on the taken branch"
    );
    assert!(
        !exec_reaches(&r.module, start(WirePort::ExecOutB), inc),
        "and not on the other"
    );
}

/// `q = q + d` reads `q` before `d` runs, so a `d` that changes `q` gets a read
/// and a set rather than an increment.
#[test]
fn a_self_increment_reads_the_target_first() {
    let head = "var q: int = 0\nmod bump() -> int {\n  q = q + 1\n  return 1\n}\nin go: exec\n";
    let r = compile_ok(&format!("{head}on go {{ q = q + bump() }}\n"));
    let inc = find_gate(&r, gc::VAR_INCREMENT);
    let set = setter(&r, "q");
    let sum = value_of(&r, set);
    let read = source_of(&r, sum, WirePort::InputA).expect("q is read");
    assert_eq!(r.module.nodes[&read].gate_class, gc::VAR_GET);
    assert!(
        exec_reaches(&r.module, read, inc) && exec_reaches(&r.module, inc, set),
        "q read, bump(), then the set"
    );
    let r = compile_ok(&format!("{head}on go {{ q = bump() + q }}\n"));
    assert_eq!(
        writes_of(&r, Some("q")).len(),
        2,
        "bump() runs first, so `q` is incremented"
    );
}

/// A chip's record argument is wired from the values it reads, and no hold is
/// left unused.
#[test]
fn a_chip_record_argument_leaves_no_unused_read() {
    let r = compile_ok(
        "type T = { a: int, b: int }\nvar q: int = 0\nvar oa: int = 0\nvar rv: T\n\
         mod bump() -> int {\n  q = q + 1\n  rv.a = rv.a + 1\n  return 5\n}\n\
         chip crec(t: T, b: int) -> (x: int, y: int) {\n  out x = t.a\n  out y = b\n}\n\
         in go: exec\non go {\n  let c = crec(rv, bump())\n  oa = c.x\n}\n",
    );
    for n in r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_GET)
    {
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.source == n.id.port(WirePort::Value)),
            "an unused read: {n:?}"
        );
    }
}

/// A record's fields are written in declaration order, however their names
/// hash.
#[test]
fn record_fields_are_written_in_declaration_order() {
    let names = ["f5", "f2", "f9", "f1", "f7", "f3", "f8", "f0"];
    let decl: Vec<String> = names.iter().map(|n| format!("{n}: int")).collect();
    for body in ["r = s", "r = { ...s }", "r = if c then s else s"] {
        let r = compile_ok(&format!(
            "type R = {{ {} }}\nvar r: R\nvar s: R\nvar c: bool\nin go: exec\non go {{ {body} }}\n",
            decl.join(", ")
        ));
        let chain = exec_chain(&r);
        let written: Vec<String> = chain
            .iter()
            .filter(|n| r.module.nodes[n].gate_class == gc::VAR_SET)
            .map(|n| storage_via(&r, *n, WirePort::VarRef))
            .collect();
        let want: Vec<String> = names.iter().map(|n| format!("r.{n}")).collect();
        assert_eq!(written, want, "{body}");
    }
}

/// An `emit` before an ordered construct still fires ahead of the construct's
/// holds, in source order.
#[test]
fn an_earlier_emit_fires_before_later_holds() {
    let head = "type T = { a: int, b: int }\nvar q: int = 0\nvar o: int = 0\nvar t: T\nlet l = q * 3\nlet done: exec\n\
                mod bump() -> int {\n  q = q + 1\n  return 5\n}\non done { q = 100 }\nin go: exec\n";
    for body in [
        "o = l + bump()",
        "t = { a: q, b: bump() }",
        "t = { a: l, b: bump() }",
    ] {
        let r = compile_ok(&format!("{head}on go {{\n  emit done\n  {body}\n}}\n"));
        let handler = ops_on(&r, gc::VAR_SET, WirePort::VarRef, "q");
        assert_eq!(handler.len(), 1);
        let reads: Vec<NodeId> = holds(&r)
            .into_iter()
            .flat_map(|v| {
                ops_on(
                    &r,
                    gc::VAR_SET,
                    WirePort::VarRef,
                    &label(&r, v).expect("labelled"),
                )
            })
            .chain(
                r.module
                    .nodes
                    .values()
                    .filter(|n| n.gate_class == gc::VAR_GET)
                    .map(|n| n.id),
            )
            .collect();
        assert!(!reads.is_empty(), "{body}: a read is held");
        for read in reads {
            assert!(
                !exec_reaches(&r.module, read, handler[0]),
                "{body}: the handler runs after a later read"
            );
        }
    }
}

/// `*e = v` evaluates the ref `e` before `v`, as `xs[i] = v` evaluates `i`.
#[test]
fn a_write_through_a_ref_evaluates_the_ref_first() {
    let r = compile_ok(
        "var a: int = 0\nvar b: int = 0\nvar h: *int\nmod retarget() -> int {\n  h = &b\n  return 7\n}\n\
         in go: exec\non go { *h = retarget() }\n",
    );
    let read = ops_on(&r, gc::VAR_GET, WirePort::VarRef, "h");
    assert_eq!(read.len(), 1);
    assert!(
        exec_reaches(&r.module, read[0], setter(&r, "h")),
        "h is read before retarget() changes it"
    );
    let r = compile_ok(
        "var a: int = 0\nvar i: int = 0\nvar refs: (*int)[]\nmod nexti() -> int {\n  i = i + 1\n  return 7\n}\n\
         in go: exec\non go { *refs[i] = nexti() }\n",
    );
    let get = find_gate(&r, gc::ARRAY_GET);
    assert!(
        exec_reaches(&r.module, get, find_gate(&r, gc::VAR_INCREMENT)),
        "refs[i] is read before nexti()"
    );
}
