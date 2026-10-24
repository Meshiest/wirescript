//! A stored record's array or map field whose element is itself a record
//! (`type H = { pts: P[] }`, `type H = { m: Map<K, P> }`) is stored as parallel
//! per-field columns, exactly like a top-level `var pts: P[]` /
//! `var m: Map<K, P>`. These tests pin which column each container gate is
//! wired to.

use super::*;
use crate::ir::gate_class as gc;

/// The `NAME_LABEL` of a storage gate.
fn label(r: &LowerResult, node: crate::ir::NodeId) -> String {
    match r.module.nodes[&node]
        .properties
        .get(&crate::intern::sym::NAME_LABEL)
    {
        Some(crate::ir::Literal::String(s)) => s.clone(),
        other => panic!("storage gate {node:?} has no name label: {other:?}"),
    }
}

/// Every container op, as `(gate class, label of the storage it is wired to)`,
/// sorted. A container op reaches its storage through `ArrayVarRef` /
/// `MapVarRef`.
fn storage_ops(r: &LowerResult) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = r
        .module
        .wires
        .iter()
        .filter(|w| matches!(w.target.port, WirePort::ArrayVarRef | WirePort::MapVarRef))
        .filter(|w| {
            matches!(
                r.module.nodes[&w.source.node_id].gate_class,
                gc::ARRAY_VAR | gc::MAP_VAR
            )
        })
        .map(|w| {
            (
                r.module.nodes[&w.target.node_id].gate_class.to_string(),
                label(r, w.source.node_id),
            )
        })
        .collect();
    out.sort();
    out
}

/// For every `Var_Set` writing the var labelled `var`: the gate class feeding
/// its `Value` and the label of the column that gate reads.
fn reads_into(r: &LowerResult, var: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for set in r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::VAR_SET)
    {
        let Some(target) = source_of(r, set.id, WirePort::VarRef) else {
            continue;
        };
        if label(r, target) != var {
            continue;
        }
        let value = source_of(r, set.id, WirePort::Value).expect("Var_Set Value must be wired");
        let reader = &r.module.nodes[&value];
        let column = source_of(r, value, WirePort::ArrayVarRef)
            .or_else(|| source_of(r, value, WirePort::MapVarRef))
            .map(|c| label(r, c))
            .unwrap_or_default();
        out.push((reader.gate_class.to_string(), column));
    }
    out.sort();
    out
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = v
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    out.sort();
    out
}

#[track_caller]
fn assert_clean_columns(r: &LowerResult) {
    assert_no_errors(r);
    crate::test_support::assert_no_code(&r.diagnostics, "WSP001");
    assert!(!has_unsupported(&r.module), "no placeholder gate");
    assert!(
        !has_gate_deep(r, gc::SPLIT_VECTOR),
        "a record column is never read through a vector split"
    );
}

/// The labels of every `class` storage gate, sorted.
fn labels_of(r: &LowerResult, class: &str) -> Vec<String> {
    let mut out: Vec<String> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .map(|n| label(r, n.id))
        .collect();
    out.sort();
    out
}

const PH: &str = "type P = { x: int, y: int }
type H = { pts: P[], n: int }
var h: H
var o: int = 0
in go: exec
";

#[test]
fn record_array_field_push_fans_across_the_columns() {
    let r = compile(&format!(
        "{PH}on go {{ let p: P = {{ x: 1, y: 2 }}; h.pts.push(p); h.pts.push({{ x: 3, y: 4 }}) }}\n"
    ));
    assert_clean_columns(&r);
    assert_eq!(
        labels_of(&r, gc::ARRAY_VAR),
        ["h.pts.x", "h.pts.y"],
        "one column per element field"
    );
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
        ])
    );
}

#[test]
fn record_array_field_reads_index_the_right_column() {
    let r = compile(&format!(
        "{PH}on go {{
  o = h.pts[0].y
  o = h.pts.x[1]
  let q = h.pts[2]
  o = q.x
  o = h.pts.length()
}}
"
    ));
    assert_clean_columns(&r);
    assert_eq!(
        reads_into(&r, "o"),
        pairs(&[
            (gc::ARRAY_GET, "h.pts.y"),
            (gc::ARRAY_GET, "h.pts.x"),
            (gc::ARRAY_GET, "h.pts.x"),
            (gc::ARRAY_GET_LENGTH, "h.pts.x"),
        ])
    );
}

#[test]
fn record_array_field_writes_target_the_right_column() {
    let r = compile(&format!(
        "{PH}on go {{
  h.pts[0] = {{ x: 3, y: 4 }}
  h.pts.x[1] = 5
  h.pts[2].y = 6
}}
"
    ));
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_SET_AT_INDEX, "h.pts.x"),
            (gc::ARRAY_SET_AT_INDEX, "h.pts.y"),
            (gc::ARRAY_SET_AT_INDEX, "h.pts.x"),
            (gc::ARRAY_SET_AT_INDEX, "h.pts.y"),
        ])
    );
}

/// `for p in h.pts` has no single driver ref, the same as `for p in pts` on a
/// top-level record array: a clean WS082, never a loop over one column.
#[test]
fn record_array_field_foreach_is_ws082() {
    let r = compile(&format!("{PH}on go {{ for p in h.pts {{ o = p.y }} }}\n"));
    crate::test_support::assert_code(&r.diagnostics, "WS082");
}

/// Sorting one column of the record array field reorders every sibling column,
/// as `pts.x.sort()` does on a top-level record array.
#[test]
fn record_array_field_column_sort_moves_the_whole_row() {
    let r = compile(&format!("{PH}on go {{ h.pts.x.sort() }}\n"));
    assert_clean_columns(&r);
    assert!(!has_gate(&r, gc::ARRAY_SORT), "{:?}", storage_ops(&r));
    let sort = find_gate(&r, gc::ARRAY_SORT_MULTIPLE);
    let mut cols: Vec<(WirePort, String)> = r
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.node_id == sort
                && r.module.nodes[&w.source.node_id].gate_class == gc::ARRAY_VAR
        })
        .map(|w| (w.target.port, label(&r, w.source.node_id)))
        .collect();
    cols.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(
        cols,
        [
            (WirePort::ArrayVarRef, "h.pts.x".to_string()),
            (WirePort::ArrayVarRef2, "h.pts.y".to_string())
        ],
        "sorted by the x column, carrying y"
    );
}

/// A record VARIABLE with two plain array fields is not a record array: sorting
/// one field sorts that field alone, never its siblings.
#[test]
fn record_var_array_field_sort_sorts_only_that_field() {
    let r = compile_ok(
        "type H = { xs: int[], ys: int[], n: int }
var h: H
in go: exec
on go { h.xs.sort() }
",
    );
    assert!(
        !has_gate(&r, gc::ARRAY_SORT_MULTIPLE),
        "{:?}",
        storage_ops(&r)
    );
    assert_eq!(storage_ops(&r), pairs(&[(gc::ARRAY_SORT, "h.xs")]));
}

/// Copying a whole record copies each column of its record array field.
#[test]
fn record_copy_copies_each_column_of_a_record_array_field() {
    let r = compile(&format!("{PH}var h2: H\non go {{ h2 = h }}\n"));
    assert_clean_columns(&r);
    let copies: Vec<(String, String)> = storage_ops(&r)
        .into_iter()
        .filter(|(c, _)| c == gc::ARRAY_COPY_FROM)
        .collect();
    assert_eq!(
        copies,
        pairs(&[
            (gc::ARRAY_COPY_FROM, "h2.pts.x"),
            (gc::ARRAY_COPY_FROM, "h2.pts.y")
        ])
    );
    let sources: Vec<String> = r
        .module
        .wires
        .iter()
        .filter(|w| w.target.port == WirePort::SourceRef)
        .map(|w| label(&r, w.source.node_id))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(sources, ["h.pts.x", "h.pts.y"]);
}

#[test]
fn record_array_field_record_initializer_bakes_the_columns() {
    let r = compile_ok(
        "type P = { x: int, y: int }
type H = { pts: P[] }
var h: H = { pts: [{ x: 1, y: 2 }, { x: 3, y: 4 }] }
",
    );
    for (col, want) in [("h.pts.x", [1, 3]), ("h.pts.y", [2, 4])] {
        let gate = r
            .module
            .nodes
            .values()
            .find(|n| n.gate_class == gc::ARRAY_VAR && label(&r, n.id) == col)
            .unwrap_or_else(|| panic!("no column {col}"));
        assert_eq!(
            gate.properties.get(&crate::intern::sym::INITIAL_VALUE),
            Some(&crate::ir::Literal::Array(
                want.iter().map(|v| crate::ir::Literal::Int(*v)).collect()
            )),
            "{col}"
        );
    }
}

const PM: &str = "type P = { x: int, y: int }
type H = { m: Map<string, P>, n: int }
var h: H
var o: int = 0
in go: exec
";

#[test]
fn record_map_field_writes_and_reads_use_the_columns() {
    let r = compile(&format!(
        "{PM}on go {{
  h.m[\"a\"] = {{ x: 1, y: 2 }}
  h.m[\"b\"].x = 3
  o = h.m[\"a\"].y
  let q = h.m[\"b\"]
  o = q.x
  o = h.m.length()
}}
"
    ));
    assert_clean_columns(&r);
    assert_eq!(
        labels_of(&r, gc::MAP_VAR),
        ["h.m.x", "h.m.y"],
        "one column per element field"
    );
    let sets: Vec<(String, String)> = storage_ops(&r)
        .into_iter()
        .filter(|(c, _)| c == gc::MAP_SET)
        .collect();
    assert_eq!(
        sets,
        pairs(&[
            (gc::MAP_SET, "h.m.x"),
            (gc::MAP_SET, "h.m.y"),
            (gc::MAP_SET, "h.m.x")
        ])
    );
    assert_eq!(
        reads_into(&r, "o"),
        pairs(&[
            (gc::MAP_GET, "h.m.y"),
            (gc::MAP_GET, "h.m.x"),
            (gc::MAP_GET_LENGTH, "h.m.x"),
        ])
    );
}

/// The same forms through a namespace member (`import * as u` of a module that
/// declares `var h: H`) land on that module's columns.
#[test]
fn namespaced_record_array_field_uses_the_member_columns() {
    let r = compile_multi(
        "import * as u from \"lib\"
var o: int = 0
in go: exec
on go {
  u.h.pts.push({ x: 1, y: 2 })
  o = u.h.pts[0].y
  u.h.pts.x[0] = 5
}
",
        &[(
            "lib",
            "type P = { x: int, y: int }\ntype H = { pts: P[] }\nvar h: H\n",
        )],
    );
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
            (gc::ARRAY_GET, "h.pts.y"),
            (gc::ARRAY_SET_AT_INDEX, "h.pts.x"),
        ])
    );
}

/// A record var whose array field holds plain values keeps its one ArrayVar per
/// field: `h.xs.push(3); o = h.xs[0]` and `h.n` lower exactly as before.
#[test]
fn record_var_scalar_array_field_is_unchanged() {
    let r = compile_ok(
        "type H = { xs: int[], n: int }
var h: H
var o: int = 0
in go: exec
on go { h.xs.push(3); o = h.xs[0]; h.n = 2; o = h.n }
",
    );
    assert!(!has_unsupported(&r.module));
    let mut classes: Vec<&str> = r.module.nodes.values().map(|n| n.gate_class).collect();
    classes.sort();
    let mut want = vec![
        gc::ARRAY_VAR,
        gc::VAR,
        gc::VAR,
        gc::MICROCHIP_INPUT,
        gc::ARRAY_PUSH,
        gc::ARRAY_GET,
        gc::VAR_SET,
        gc::VAR_SET,
        gc::VAR_GET,
        gc::VAR_SET,
    ];
    want.sort();
    assert_eq!(classes, want);
    assert_eq!(r.module.wires.len(), 14);
    assert_eq!(
        storage_ops(&r),
        pairs(&[(gc::ARRAY_PUSH, "h.xs"), (gc::ARRAY_GET, "h.xs")])
    );
    assert_eq!(
        reads_into(&r, "o"),
        pairs(&[(gc::ARRAY_GET, "h.xs"), (gc::VAR_GET, "")])
    );
}

/// Every container op on a column record field keeps the columns in lockstep.
#[test]
fn record_array_field_pop_insert_remove_clear_fan_across_the_columns() {
    let r = compile(&format!(
        "{PH}on go {{
  h.pts.insert(0, {{ x: 3, y: 4 }})
  let q = h.pts.pop()
  o = q.y
  h.pts.remove(0)
  h.pts.clear()
}}
"
    ));
    assert_clean_columns(&r);
    let mut want = Vec::new();
    for class in [
        gc::ARRAY_INSERT,
        gc::ARRAY_POP,
        gc::ARRAY_REMOVE_AT_INDEX,
        gc::ARRAY_CLEAR,
    ] {
        want.push((class, "h.pts.x"));
        want.push((class, "h.pts.y"));
    }
    assert_eq!(storage_ops(&r), pairs(&want));
    assert_eq!(reads_into(&r, "o"), pairs(&[(gc::ARRAY_POP, "h.pts.y")]));
}

/// A record array field of a NESTED record (`h.inner.pts`) is columns too.
#[test]
fn nested_record_array_field_uses_the_columns() {
    let r = compile(
        "type P = { x: int, y: int }
type I = { pts: P[] }
type H = { inner: I, n: int }
var h: H
var o: int = 0
in go: exec
on go {
  h.inner.pts.push({ x: 1, y: 2 })
  o = h.inner.pts[0].y
  h.inner.pts.x[0] = 5
}
",
    );
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_PUSH, "h.inner.pts.x"),
            (gc::ARRAY_PUSH, "h.inner.pts.y"),
            (gc::ARRAY_GET, "h.inner.pts.y"),
            (gc::ARRAY_SET_AT_INDEX, "h.inner.pts.x"),
        ])
    );
}

/// An alias naming the array type (`type Pts = P[]`) is columns, both as a
/// record field and as a top-level var.
#[test]
fn aliased_record_array_is_stored_as_columns() {
    let field = compile(
        "type P = { x: int, y: int }
type Pts = P[]
type H = { pts: Pts, n: int }
var h: H
var o: int = 0
in go: exec
on go {
  h.pts.push({ x: 1, y: 2 })
  o = h.pts[0].y
  o = h.pts.x[0]
}
",
    );
    assert_clean_columns(&field);
    assert_eq!(
        storage_ops(&field),
        pairs(&[
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
            (gc::ARRAY_GET, "h.pts.y"),
            (gc::ARRAY_GET, "h.pts.x"),
        ])
    );
    let top = compile(
        "type P = { x: int, y: int }
type Pts = P[]
var pts: Pts
var o: int = 0
in go: exec
on go { pts.push({ x: 1, y: 2 }); o = pts[0].y }
",
    );
    assert_clean_columns(&top);
    assert_eq!(
        storage_ops(&top),
        pairs(&[
            (gc::ARRAY_PUSH, "pts.x"),
            (gc::ARRAY_PUSH, "pts.y"),
            (gc::ARRAY_GET, "pts.y"),
        ])
    );
}

/// The storage gate labelled `label_text`.
fn storage(r: &LowerResult, label_text: &str) -> crate::ir::NodeId {
    r.module
        .nodes
        .values()
        .find(|n| {
            matches!(n.gate_class, gc::ARRAY_VAR | gc::MAP_VAR | gc::VAR)
                && label(r, n.id) == label_text
        })
        .unwrap_or_else(|| panic!("no storage labelled {label_text}"))
        .id
}

fn initial(r: &LowerResult, label_text: &str) -> Option<crate::ir::Literal> {
    r.module.nodes[&storage(r, label_text)]
        .properties
        .get(&crate::intern::sym::INITIAL_VALUE)
        .cloned()
}

#[test]
fn record_var_container_field_initializers_bake() {
    use crate::ir::Literal as L;
    let r = compile_ok(
        "type P = { x: int, y: int }
type H = { xs: int[], m: Map<string, int>, pm: Map<string, P>, n: int }
var h: H = { xs: [1, 2], m: { \"a\": 3 }, pm: { \"b\": { x: 4, y: 5 } }, n: 6 }
",
    );
    assert_eq!(
        initial(&r, "h.xs"),
        Some(L::Array(vec![L::Int(1), L::Int(2)]))
    );
    assert_eq!(
        initial(&r, "h.m"),
        Some(L::Map(vec![(L::String("a".into()), L::Int(3))]))
    );
    assert_eq!(
        initial(&r, "h.pm.x"),
        Some(L::Map(vec![(L::String("b".into()), L::Int(4))]))
    );
    assert_eq!(
        initial(&r, "h.pm.y"),
        Some(L::Map(vec![(L::String("b".into()), L::Int(5))]))
    );
    assert_eq!(initial(&r, "h.n"), Some(L::Int(6)));
}

/// `h = { .. }` with container fields rebuilds each container like
/// `h.xs = [..]` / `h.pts = [..]` / `h.m = { .. }`, and the scalar fields are
/// still written.
#[test]
fn whole_record_literal_assign_rebuilds_container_fields() {
    let r = compile(
        "type P = { x: int, y: int }
type H = { pts: P[], xs: int[], m: Map<string, int>, pm: Map<string, P>, n: int }
var h: H
in go: exec
on go { h = { pts: [{ x: 5, y: 6 }], xs: [7, 8], m: { \"a\": 1 }, pm: { \"b\": { x: 2, y: 3 } }, n: 2 } }
",
    );
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_CLEAR, "h.pts.x"),
            (gc::ARRAY_CLEAR, "h.pts.y"),
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
            (gc::ARRAY_CLEAR, "h.xs"),
            (gc::ARRAY_PUSH, "h.xs"),
            (gc::ARRAY_PUSH, "h.xs"),
            (gc::MAP_CLEAR, "h.m"),
            (gc::MAP_SET, "h.m"),
            (gc::MAP_CLEAR, "h.pm.x"),
            (gc::MAP_CLEAR, "h.pm.y"),
            (gc::MAP_SET, "h.pm.x"),
            (gc::MAP_SET, "h.pm.y"),
        ])
    );
    assert_eq!(written_var_labels(&r), ["h.n"]);
}

/// A record map assigned a literal (`m = { k: rec }`) rebuilds its columns.
#[test]
fn record_map_literal_assign_rebuilds_the_columns() {
    let r = compile(
        "type P = { x: int, y: int }
var m: Map<string, P>
in go: exec
on go { m = { \"a\": { x: 1, y: 2 } } }
",
    );
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::MAP_CLEAR, "m.x"),
            (gc::MAP_CLEAR, "m.y"),
            (gc::MAP_SET, "m.x"),
            (gc::MAP_SET, "m.y"),
        ])
    );
}

/// A nested record literal reaches the nested container field.
#[test]
fn whole_record_literal_assign_reaches_a_nested_container_field() {
    let r = compile(
        "type P = { x: int, y: int }
type I = { pts: P[] }
type H = { inner: I, n: int }
var h: H
in go: exec
on go { h = { inner: { pts: [{ x: 1, y: 2 }] }, n: 0 } }
",
    );
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_CLEAR, "h.inner.pts.x"),
            (gc::ARRAY_CLEAR, "h.inner.pts.y"),
            (gc::ARRAY_PUSH, "h.inner.pts.x"),
            (gc::ARRAY_PUSH, "h.inner.pts.y"),
        ])
    );
}

/// A non-literal plain container field is WS027, as `h.xs = ys` is; a column
/// record field copies each column, as `pts = pts2` does.
#[test]
fn whole_record_literal_assign_non_literal_container_fields() {
    let r = compile(
        "type H = { xs: int[], n: int }
var h: H
var ys: int[]
in go: exec
on go { h = { xs: ys, n: 2 } }
",
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS027"
            && d.severity == crate::diagnostic::Severity::Error
            && d.message.contains("h.xs.copyFrom(src)")),
        "{:?}",
        r.diagnostics
    );
    let c = compile(&format!(
        "{PH}var pts2: P[]\non go {{ h = {{ pts: pts2, n: 1 }} }}\n"
    ));
    assert_clean_columns(&c);
    let copies: Vec<(String, String)> = storage_ops(&c)
        .into_iter()
        .filter(|(k, _)| k == gc::ARRAY_COPY_FROM)
        .collect();
    assert_eq!(
        copies,
        pairs(&[
            (gc::ARRAY_COPY_FROM, "h.pts.x"),
            (gc::ARRAY_COPY_FROM, "h.pts.y")
        ])
    );
}

/// Choosing between two records that hold record arrays has no single
/// reference per column to select, so it is WS071 like `if c then pts else pts2`.
#[test]
fn record_if_expr_with_a_column_record_field_is_ws071() {
    let r = compile(&format!(
        "{PH}var h2: H\nin c: bool\non go {{
  let w = if c then h else h2
  o = w.pts[0].y
}}
"
    ));
    assert!(has_error(&r, "WS071"), "{:?}", r.diagnostics);
}

/// A spread fills every field it provides (containers copy per column, as
/// `pts = qs` does), and the explicit fields override it.
#[test]
fn whole_record_literal_assign_with_a_spread() {
    let r = compile(&format!(
        "{PH}var h2: H\non go {{ h = {{ ...h2, pts: [{{ x: 1, y: 2 }}] }} }}\n"
    ));
    assert_clean_columns(&r);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_CLEAR, "h.pts.x"),
            (gc::ARRAY_CLEAR, "h.pts.y"),
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
        ])
    );
    assert_eq!(written_var_labels(&r), ["h.n"]);

    let s = compile(&format!(
        "{PH}var h2: H\non go {{ h = {{ ...h2, n: 1 }} }}\n"
    ));
    assert_clean_columns(&s);
    assert_eq!(
        storage_ops(&s),
        pairs(&[
            (gc::ARRAY_COPY_FROM, "h.pts.x"),
            (gc::ARRAY_COPY_FROM, "h.pts.y")
        ])
    );
    assert_eq!(written_var_labels(&s), ["h.n"]);

    let t = compile(
        "type H = { xs: int[], n: int }
var h: H
var h2: H
in go: exec
on go { h = { ...h2, xs: [1] } }
",
    );
    assert_clean_columns(&t);
    assert_eq!(
        storage_ops(&t),
        pairs(&[(gc::ARRAY_CLEAR, "h.xs"), (gc::ARRAY_PUSH, "h.xs")])
    );
    assert_eq!(written_var_labels(&t), ["h.n"]);
}

/// A generic alias of an array (`type Arr<T> = T[]`) is columns too.
#[test]
fn generic_aliased_record_array_is_stored_as_columns() {
    for (decl, col) in [
        ("var a: Arr<P>\n", "a"),
        ("type H = { pts: Arr<P>, n: int }\nvar h: H\n", "h.pts"),
    ] {
        let r = compile(&format!(
            "type P = {{ x: int, y: int }}\ntype Arr<T> = T[]\n{decl}var o: int = 0\nin go: exec\n\
             on go {{ {col}.push({{ x: 1, y: 2 }}); o = {col}[0].y }}\n"
        ));
        assert_clean_columns(&r);
        let (x, y) = (format!("{col}.x"), format!("{col}.y"));
        assert_eq!(
            storage_ops(&r),
            pairs(&[
                (gc::ARRAY_PUSH, x.as_str()),
                (gc::ARRAY_PUSH, y.as_str()),
                (gc::ARRAY_GET, y.as_str())
            ]),
            "{decl}"
        );
    }
}

/// A handler-local record var resets to its initializer every run, like a
/// plain local `var xs: int[] = [..]`, and a non-constant field is wired.
#[test]
fn local_record_var_initializer_runs_every_time() {
    let r = compile(
        "type P = { x: int, y: int }
type H = { xs: int[], m: Map<string, int>, pts: P[], n: int }
var o: int = 0
in k: int
in go: exec
on go {
  var h: H = { xs: [1, k], m: { \"a\": k }, pts: [{ x: k, y: 2 }], n: k }
  o = h.n
}
",
    );
    assert_clean_columns(&r);
    crate::test_support::assert_no_diags(&r.diagnostics);
    assert_eq!(
        storage_ops(&r),
        pairs(&[
            (gc::ARRAY_CLEAR, "h.xs"),
            (gc::ARRAY_PUSH, "h.xs"),
            (gc::ARRAY_PUSH, "h.xs"),
            (gc::MAP_CLEAR, "h.m"),
            (gc::MAP_SET, "h.m"),
            (gc::ARRAY_CLEAR, "h.pts.x"),
            (gc::ARRAY_CLEAR, "h.pts.y"),
            (gc::ARRAY_PUSH, "h.pts.x"),
            (gc::ARRAY_PUSH, "h.pts.y"),
        ])
    );
    assert!(written_var_labels(&r).contains(&"h.n".to_string()));

    let p = compile_ok(
        "type P = { x: int, y: int }
var o: int = 0
in k: int
in go: exec
on go { var p: P = { x: k, y: 1 }\n o = p.x }
",
    );
    let written = written_var_labels(&p);
    assert!(
        written.contains(&"p.x".to_string()) && written.contains(&"p.y".to_string()),
        "{written:?}"
    );
}

/// A `static` local record var keeps its baked initializer and is never reset.
#[test]
fn static_local_record_var_initializes_once() {
    use crate::ir::Literal as L;
    let r = compile_ok(
        "type H = { xs: int[], n: int }
var o: int = 0
in go: exec
on go { static var h: H = { xs: [1], n: 2 }\n h.xs.push(3)\n o = h.n }
",
    );
    assert!(!has_gate(&r, gc::ARRAY_CLEAR));
    assert_eq!(initial(&r, "h.xs"), Some(L::Array(vec![L::Int(1)])));
    assert_eq!(initial(&r, "h.n"), Some(L::Int(2)));
    assert!(!written_var_labels(&r).contains(&"h.n".to_string()));
}

/// A top-level record initializer whose container field can't bake warns, as
/// `var ws: int[] = ys` does.
#[test]
fn top_level_record_initializer_with_a_non_literal_container_warns() {
    let r = compile(
        "type H = { xs: int[], n: int }
var ys: int[] = [1, 2]
var h: H = { xs: ys, n: 1 }
",
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.message.contains("'var h'")
                && d.message.contains("not a compile-time constant")),
        "{:?}",
        r.diagnostics
    );
    let ok = compile("type H = { xs: int[], n: int }\nvar h: H = { xs: [1, 2], n: 1 }\n");
    crate::test_support::assert_no_diags(&ok.diagnostics);
}

/// The scalar field values of a whole-literal assign are read before any
/// container field is rebuilt: `n: h.xs.length()` sees the old contents.
#[test]
fn whole_record_literal_assign_reads_scalars_before_rebuilding_containers() {
    let r = compile_ok(
        "type H = { xs: int[], n: int }
var h: H
in go: exec
on go { h = { xs: [5], n: h.xs.length() } }
",
    );
    let len = find_gate(&r, gc::ARRAY_GET_LENGTH);
    let clear = find_gate(&r, gc::ARRAY_CLEAR);
    assert!(
        wired_reachable(&r, len, clear),
        "GetLength must run before the Clear"
    );
}

/// The gate feeding the `Value` of the `Var_Set` that writes `label_text`.
fn written_from(r: &LowerResult, label_text: &str) -> crate::ir::NodeId {
    let var = storage(r, label_text);
    let set = r
        .module
        .wires
        .iter()
        .find(|w| {
            w.source.node_id == var
                && w.target.port == WirePort::VarRef
                && r.module.nodes[&w.target.node_id].gate_class == gc::VAR_SET
        })
        .map(|w| w.target.node_id)
        .unwrap_or_else(|| panic!("no Var_Set writes {label_text}"));
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == set && w.target.port == WirePort::Value)
        .map(|w| w.source.node_id)
        .expect("the Var_Set's Value must be wired")
}

/// A scalar field reading `.Value` off a record whose `Value` is a real field
/// is that field, never the record's first port or an unrelated exec wire.
#[test]
fn record_literal_dot_value_field_reads_the_field() {
    let r = compile_ok(
        "type R = { k: int, Value: int }
type P = { x: int, y: int }
var p: P
in go: exec
in z: int
mod mk(v: int) -> R { return { k: v + 100, Value: v + 7 } }
on go { p = { x: mk(z).Value, y: 1 } }
",
    );
    let src = written_from(&r, "p.x");
    let node = &r.module.nodes[&src];
    assert_eq!(
        node.gate_class, "BrickComponentType_Expr_MathAdd",
        "p.x must be written from `v + 7`, not the exec input: {node:?}"
    );

    let w = compile_ok(
        "type R = { Value: int, k: int, z: int }
type P = { x: int, y: int }
var rs: R[]
var p: P
in go: exec
on go { p = { x: rs.pop().Value, y: 5 } }
",
    );
    assert_eq!(gate_count(&w, gc::ARRAY_POP), 3, "one Pop per column");
    let pop = written_from(&w, "p.x");
    assert_eq!(w.module.nodes[&pop].gate_class, gc::ARRAY_POP);
    let column = w
        .module
        .wires
        .iter()
        .find(|e| e.target.node_id == pop && e.target.port == WirePort::ArrayVarRef)
        .map(|e| label(&w, e.source.node_id));
    assert_eq!(column.as_deref(), Some("rs.Value"));
}

/// Each call in a record literal field runs exactly once.
#[test]
fn record_literal_field_calls_run_once() {
    for (src, class, want) in [
        (
            "type P = { x: int, y: bool }\nvar p: P\nvar m: Map<int, int>\nin go: exec\n\
             on go { p = { x: m.get(3).Value, y: m.get(4).Found } }\n",
            gc::MAP_GET,
            2,
        ),
        (
            "type P = { x: int, y: bool }\nvar p: P\nvar a: int[]\nin go: exec\n\
             on go { p = { x: a.pop().Value, y: a.pop().IsEmpty } }\n",
            gc::ARRAY_POP,
            2,
        ),
        (
            "type P = { x: int, y: int }\nvar p: P\nvar a: int[]\nin c: bool\nin go: exec\n\
             on go { p = { x: if c then a.pop().Value else 0, y: 1 } }\n",
            gc::ARRAY_POP,
            1,
        ),
        (
            "type P = { x: int, y: bool }\nvar a: int[]\nvar o: int = 0\nin go: exec\n\
             on go { let q = { x: a.pop().Value, y: a.pop().IsEmpty }\n o = q.x }\n",
            gc::ARRAY_POP,
            2,
        ),
        // Not once for the record probe and again for the value.
        (
            "type P = { x: int, y: int }\nvar p: P\nvar xs: int[]\nin go: exec\non go { p = { x: xs.pop(), y: 1 } }\n",
            gc::ARRAY_POP,
            1,
        ),
    ] {
        let r = compile_ok(src);
        assert_eq!(gate_count(&r, class), want, "{src}");
    }
}

/// `h = { ...h, n: 1 }` copies nothing onto itself.
#[test]
fn self_spread_copies_nothing() {
    let r = compile_ok(
        "type H = { xs: int[], n: int }
var h: H
in go: exec
on go { h = { ...h, n: 1 } }
",
    );
    assert!(!has_gate(&r, gc::ARRAY_COPY_FROM));
    assert_eq!(written_var_labels(&r), ["h.n"]);
    let s = compile_ok("type H = { xs: int[], n: int }\nvar h: H\nin go: exec\non go { h = h }\n");
    assert!(!has_gate(&s, gc::ARRAY_COPY_FROM) && !has_gate(&s, gc::VAR_SET));
}

/// An explicit field a later spread overrides is still evaluated, as the
/// `let` form evaluates it.
#[test]
fn overridden_field_is_still_evaluated() {
    let r = compile_ok(
        "type P = { x: int, y: int }
var p: P
var a: int[]
var b: int[]
in go: exec
mod mk(v: int) -> P { return { x: v, y: v } }
on go { p = { x: a.pop(), ...mk(b.pop()) } }
",
    );
    let mut popped: Vec<String> = r
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.port == WirePort::ArrayVarRef
                && r.module.nodes[&w.target.node_id].gate_class == gc::ARRAY_POP
        })
        .map(|w| label(&r, w.source.node_id))
        .collect();
    popped.sort();
    assert_eq!(popped, ["a", "b"]);
}

/// A spread of a multi-output gate result supplies its ports, and an earlier
/// spread is lowered once.
#[test]
fn spread_of_a_gate_result_and_an_earlier_spread_lower_once() {
    let r = compile_ok(
        "type R = { Value: int, Found: bool }
var r: R
var m: Map<int, int>
var b: int[]
in go: exec
mod mk(v: int) -> R { return { Value: v, Found: true } }
on go { r = { ...mk(b.pop()), ...m.get(3) } }
",
    );
    assert_eq!(gate_count(&r, gc::ARRAY_POP), 1);
    assert_eq!(gate_count(&r, gc::MAP_GET), 1);
    let get = find_gate(&r, gc::MAP_GET);
    assert_eq!(written_from(&r, "r.Value"), get);
    assert_eq!(written_from(&r, "r.Found"), get);
}

/// A map literal may fill a `Map` field alongside a spread.
#[test]
fn map_literal_field_with_a_spread_rebuilds_the_map() {
    let r = compile(
        "type H = { m: Map<string, int>, n: int }
var h: H
var h2: H
in go: exec
on go { h = { ...h2, m: { \"a\": 1 } } }
",
    );
    assert_clean_columns(&r);
    crate::test_support::assert_no_code(&r.diagnostics, "WS026");
    assert_eq!(
        storage_ops(&r),
        pairs(&[(gc::MAP_CLEAR, "h.m"), (gc::MAP_SET, "h.m")])
    );
    assert_eq!(written_var_labels(&r), ["h.n"]);
}

/// A top-level record initializer whose map field can't bake warns, by the
/// same rule a map var's initializer bakes by.
#[test]
fn top_level_record_initializer_with_a_non_constant_map_field_warns() {
    let r = compile(
        "type H = { m: Map<string, int>, n: int }
in k: int
var h: H = { m: { \"a\": k }, n: 1 }
",
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.message.contains("'var h'")
                && d.message.contains("not a compile-time constant")),
        "{:?}",
        r.diagnostics
    );
    assert_eq!(initial(&r, "h.m"), None);
}

/// Sorting a nested record array's column carries EVERY column of the root
/// record array, not only the nested record's siblings.
#[test]
fn nested_column_sort_carries_every_root_column() {
    let r = compile_ok(
        "type P = { x: int, y: int }
type Q = { p: P, k: int }
var qs: Q[]
in go: exec
on go { qs.p.x.sort() }
",
    );
    assert!(!has_gate(&r, gc::ARRAY_SORT));
    let sort = find_gate(&r, gc::ARRAY_SORT_MULTIPLE);
    let mut cols: Vec<(WirePort, String)> = r
        .module
        .wires
        .iter()
        .filter(|w| {
            w.target.node_id == sort
                && r.module.nodes[&w.source.node_id].gate_class == gc::ARRAY_VAR
        })
        .map(|w| (w.target.port, label(&r, w.source.node_id)))
        .collect();
    cols.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(cols.len(), 3, "{cols:?}");
    assert_eq!(
        cols[1],
        (WirePort::ArrayVarRef, "qs.p.x".to_string()),
        "{cols:?}"
    );
    let carried: Vec<&str> = cols.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(carried, ["qs.k", "qs.p.x", "qs.p.y"]);
}
