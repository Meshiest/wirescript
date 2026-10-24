//! Ref values in lowering: `&x`, reads and writes through a ref, container
//! operations on a runtime ref, parameters bound to one, and the read-cache
//! invalidation a write through a ref needs.

use super::*;
use crate::ir::NodeId;
use crate::ir::gate_class as gc;

/// What drives `(node, port)`: the source node and port.
pub(super) fn source_port_of(
    r: &LowerResult,
    node: NodeId,
    port: WirePort,
) -> Option<(NodeId, WirePort)> {
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == node && w.target.port == port)
        .map(|w| (w.source.node_id, w.source.port))
}

/// Every root-module gate of `class`, in source order.
pub(super) fn gates(r: &LowerResult, class: &str) -> Vec<NodeId> {
    let mut v: Vec<&crate::ir::Node> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .collect();
    v.sort_by_key(|n| (n.source_range.start.offset, n.source_range.end.offset));
    v.into_iter().map(|n| n.id).collect()
}

#[test]
fn ref_of_a_var_wires_its_varref_into_the_consumer() {
    let r = compile_ok("var a: int = 0\nvar h: *int\nin go: exec\non go { h = &a }");
    let set = gates(&r, gc::VAR_SET)[0];
    let (src, port) = source_port_of(&r, set, WirePort::Value).expect("Value wired");
    assert_eq!(class_of(&r, src), gc::VAR);
    assert_eq!(port, WirePort::VarRef, "`&a` is a's VarRef, not its value");
}

#[test]
fn ref_of_an_array_is_its_arrayvarref() {
    let r = compile_ok("var row: int[]\nvar rows: int[][]\nin go: exec\non go { rows.push(&row) }");
    let push = gates(&r, gc::ARRAY_PUSH)[0];
    let (src, port) = source_port_of(&r, push, WirePort::Value).expect("Value wired");
    assert_eq!(class_of(&r, src), gc::ARRAY_VAR);
    assert_eq!(port, WirePort::ArrayVarRef);
}

#[test]
fn a_write_through_a_ref_takes_its_varref_from_a_read_of_the_holder() {
    let r = compile_ok("var a: int = 0\nvar h: *int\nin go: exec\non go { h = &a\n *h = 5 }");
    let sets = gates(&r, gc::VAR_SET);
    assert_eq!(sets.len(), 2, "h = &a, then *h = 5");
    let (src, _) = source_port_of(&r, sets[1], WirePort::VarRef).expect("VarRef wired");
    assert_eq!(
        class_of(&r, src),
        gc::VAR_GET,
        "the ref comes off a read of h"
    );
    let (holder, _) = source_port_of(&r, src, WirePort::VarRef).unwrap();
    assert_eq!(class_of(&r, holder), gc::VAR);
}

#[test]
fn a_read_through_a_ref_is_a_var_get_off_the_ref() {
    let r = compile_ok("var h: *int\nvar n: int = 0\nin go: exec\non go { n = *h }");
    let reads = gates(&r, gc::VAR_GET);
    assert_eq!(reads.len(), 2, "read h, then read through it");
    let chained: Vec<_> = reads
        .iter()
        .filter(|g| {
            source_port_of(&r, **g, WirePort::VarRef).map(|s| class_of(&r, s.0))
                == Some(gc::VAR_GET)
        })
        .collect();
    assert_eq!(
        chained.len(),
        1,
        "exactly one read takes its VarRef off the other read's Value"
    );
}

#[test]
fn an_increment_through_a_ref_is_a_var_increment() {
    let r = compile_ok("var h: *int\nin go: exec\non go { *h += 2 }");
    let inc = gates(&r, gc::VAR_INCREMENT);
    assert_eq!(inc.len(), 1, "`*h += 2` is one Var_Increment");
    let (src, _) = source_port_of(&r, inc[0], WirePort::VarRef).unwrap();
    assert_eq!(class_of(&r, src), gc::VAR_GET);
}

/// Reads of the var whose gate feeds the first `Var_Get`.
fn reads_of_first_var(r: &LowerResult) -> usize {
    let all = gates(r, gc::VAR_GET);
    let (var, _) = source_port_of(r, all[0], WirePort::VarRef).unwrap();
    all.iter()
        .filter(|g| source_port_of(r, **g, WirePort::VarRef).map(|s| s.0) == Some(var))
        .count()
}

#[test]
fn a_read_after_a_write_through_a_ref_is_fresh() {
    // `n = a` reads a; `*h = ...` may have written a; the
    // second `n = a` must read again rather than reuse the first read.
    let head = "var a: int = 0\nvar n: int = 0\nvar h: *int\nin go: exec\n";
    for body in [
        "on go { n = a\n *h = 5\n n = a }",
        "on go { n = a\n if n > 0 { *h = 1 }\n n = a }",
        "on go { n = a\n for i in 0..3 { *h = i }\n n = a }",
    ] {
        let r = compile_ok(&format!("{head}{body}"));
        assert_eq!(reads_of_first_var(&r), 2, "{body}");
    }
}

#[test]
fn an_address_taken_var_is_never_folded() {
    let r = compile_ok_folded(
        "var a: int = 3\nvar n: int = 0\nvar h: *int\nin go: exec\non go { h = &a\n *h = 5\n n = a + 1 }",
    );
    let sets = gates(&r, gc::VAR_SET);
    let (a_var, _) = source_port_of(&r, sets[0], WirePort::Value).unwrap();
    assert!(
        gates(&r, gc::VAR_GET)
            .iter()
            .any(|g| source_port_of(&r, *g, WirePort::VarRef).map(|s| s.0) == Some(a_var)),
        "`n = a + 1` must read a at runtime"
    );
}

/// Some chip-instance module (`W_*`), at any depth, holds a `Var_Set`.
fn instance_writes(m: &crate::ir::Module) -> bool {
    m.modules_deep().into_iter().skip(1).any(|c| {
        crate::intern::resolve(c.name).starts_with("W_")
            && c.nodes.values().any(|n| n.gate_class == gc::VAR_SET)
    })
}

#[test]
fn a_read_after_a_chip_write_in_a_loop_is_fresh() {
    // A chip instance lowers in its own module, so its writes draw no wire out
    // of the caller's storage and bump no caller counter. The loop must still
    // drop the pre-loop read of `a`.
    let head = "var a: int = 0\nvar n: int = 0\nvar h: *int\n";
    for chip in ["chip W() { *h = 5 }", "chip W() { a = 5 }"] {
        let r = compile_ok(&format!(
            "{head}{chip}\nin go: exec\non go {{ n = a\n for i in 0..3 {{ W() }}\n n = a }}"
        ));
        assert!(
            instance_writes(&r.module),
            "{chip}: W() lowers as a chip instance holding the write"
        );
        assert_eq!(reads_of_first_var(&r), 2, "{chip}");
    }
}

#[test]
fn a_method_on_an_array_element_takes_its_ref_from_the_element() {
    let r = compile_ok("var rows: int[][]\nin go: exec\non go { rows[0].push(7) }");
    let push = gates(&r, gc::ARRAY_PUSH)[0];
    let (src, _) = source_port_of(&r, push, WirePort::ArrayVarRef).unwrap();
    assert_eq!(
        class_of(&r, src),
        gc::ARRAY_GET,
        "the inner array's ref is the element"
    );
}

#[test]
fn an_element_read_two_levels_deep_goes_through_the_inner_ref() {
    let r = compile_ok("var rows: int[][]\nvar n: int = 0\nin go: exec\non go { n = rows[0][1] }");
    let reads = gates(&r, gc::ARRAY_GET);
    assert_eq!(reads.len(), 2);
    let (src, _) = source_port_of(&r, reads[1], WirePort::ArrayVarRef).unwrap();
    assert_eq!(src, reads[0]);
}

#[test]
fn an_element_write_two_levels_deep_goes_through_the_inner_ref() {
    let r = compile_ok("var rows: int[][]\nin go: exec\non go { rows[0][1] = 5 }");
    let set = gates(&r, gc::ARRAY_SET_AT_INDEX)[0];
    let (src, _) = source_port_of(&r, set, WirePort::ArrayVarRef).unwrap();
    assert_eq!(class_of(&r, src), gc::ARRAY_GET);
}

#[test]
fn a_map_of_array_refs_and_an_array_of_map_refs() {
    let r = compile_ok(
        "var grids: Map<string, int[]>\nvar ms: Map<string, int>[]\nin go: exec\n\
         on go { grids[\"g\"].push(1)\n ms[0].set(\"k\", 2)\n ms[0][\"j\"] = 3 }",
    );
    let push = gates(&r, gc::ARRAY_PUSH)[0];
    assert_eq!(
        class_of(
            &r,
            source_port_of(&r, push, WirePort::ArrayVarRef).unwrap().0
        ),
        gc::MAP_GET
    );
    for set in gates(&r, gc::MAP_SET) {
        assert_eq!(
            class_of(&r, source_port_of(&r, set, WirePort::MapVarRef).unwrap().0),
            gc::ARRAY_GET
        );
    }
}

#[test]
fn a_method_on_a_namespaced_const_still_reports_ws044() {
    let tbl = "const TABLE: int[] = [10, 20, 30]\nconst GRID: int[][] = [[1, 2], [3, 4]]\n";
    for body in ["n = S.TABLE.length()", "n = S.GRID[i].length()"] {
        let r = compile_multi(
            &format!(
                "import * as S from \"tbl\"\nvar i: int = 1\nvar n: int = 0\nin go: exec\n\
                 on go {{ {body} }}"
            ),
            &[("tbl", tbl)],
        );
        assert!(has_error(&r, "WS044"), "{body}: {:?}", r.diagnostics);
    }
}

#[test]
fn a_namespaced_var_element_is_a_runtime_container() {
    let r = compile_multi(
        "import * as V from \"vmod\"\nin go: exec\n\
         on go { V.rows[0].push(1)\n V.ms[0].set(\"k\", 2) }",
        &[("vmod", "var rows: int[][]\nvar ms: Map<string, int>[]\n")],
    );
    assert_no_errors(&r);
    let push = gates(&r, gc::ARRAY_PUSH)[0];
    assert_eq!(
        class_of(
            &r,
            source_port_of(&r, push, WirePort::ArrayVarRef).unwrap().0
        ),
        gc::ARRAY_GET
    );
    let set = gates(&r, gc::MAP_SET)[0];
    assert_eq!(
        class_of(&r, source_port_of(&r, set, WirePort::MapVarRef).unwrap().0),
        gc::ARRAY_GET
    );
}

#[test]
fn the_value_of_a_let_bound_map_get_is_a_runtime_container() {
    let r = compile(
        "var m: Map<string, int[]>\nvar all: int[]\nvar n: int = 0\nin go: exec\n\
         on go { let e = m.get(\"k\")\n n = e.Value[0]\n e.Value[1] = 5\n all.append(e.Value)\n e.Value.push(2) }",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert!(gates(&r, gc::UNSUPPORTED).is_empty());
    for class in [gc::ARRAY_GET, gc::ARRAY_SET_AT_INDEX, gc::ARRAY_PUSH] {
        let g = gates(&r, class)[0];
        let (src, _) = source_port_of(&r, g, WirePort::ArrayVarRef).unwrap();
        assert_eq!(
            class_of(&r, src),
            gc::MAP_GET,
            "{class} takes its ref from the map get"
        );
    }
    let append = gates(&r, gc::ARRAY_APPEND)[0];
    let (src, _) = source_port_of(&r, append, WirePort::SourceRef).unwrap();
    assert_eq!(class_of(&r, src), gc::MAP_GET);

    let r = compile(
        "var mm: Map<string, Map<string, int> >\nin go: exec\n\
         on go { let e = mm.get(\"k\")\n e.Value.set(\"j\", 2) }",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let set = gates(&r, gc::MAP_SET)[0];
    assert_eq!(
        class_of(&r, source_port_of(&r, set, WirePort::MapVarRef).unwrap().0),
        gc::MAP_GET
    );
}

#[test]
fn a_ref_value_binds_an_inline_mods_ref_param_to_its_wire() {
    let r = compile_ok(
        "var pixels: (*color)[]\nmod paint(c: *color) { c = Color(1.0, 0.0, 0.0) }\n\
         in go: exec\non go { paint(pixels[0]) }",
    );
    let set = gates(&r, gc::VAR_SET)[0];
    let (src, _) = source_port_of(&r, set, WirePort::VarRef).unwrap();
    assert_eq!(
        class_of(&r, src),
        gc::ARRAY_GET,
        "the write goes through the element's ref"
    );
}

#[test]
fn a_ref_holding_var_binds_the_ref_it_holds_not_itself() {
    let r = compile_ok("var h: *int\nmod set7(p: *int) { p = 7 }\nin go: exec\non go { set7(h) }");
    let set = gates(&r, gc::VAR_SET)[0];
    let (src, _) = source_port_of(&r, set, WirePort::VarRef).unwrap();
    assert_eq!(
        class_of(&r, src),
        gc::VAR_GET,
        "p names the var h points at, read off h"
    );
}

#[test]
fn a_plain_var_still_binds_a_ref_param_by_alias() {
    let r = compile_ok(
        "var a: int = 0\nmod set7(p: *int) { p = 7 }\nin go: exec\non go { set7(a)\n set7(&a) }",
    );
    let sets = gates(&r, gc::VAR_SET);
    assert_eq!(sets.len(), 2, "one write per call");
    for set in sets {
        let (src, port) = source_port_of(&r, set, WirePort::VarRef).unwrap();
        assert_eq!((class_of(&r, src), port), (gc::VAR, WirePort::VarRef));
    }
}

#[test]
fn a_dynamically_bound_param_forwards_to_a_nested_call() {
    let r = compile_ok(
        "var h: *int\nmod inner(p: *int) { p = 1 }\nmod outer(q: *int) { inner(q) }\n\
         in go: exec\non go { outer(h) }",
    );
    let set = gates(&r, gc::VAR_SET)[0];
    let (src, _) = source_port_of(&r, set, WirePort::VarRef).unwrap();
    assert_eq!(
        class_of(&r, src),
        gc::VAR_GET,
        "q's wire reaches inner's p unchanged"
    );
    // One read of `h`, straight from its var: a second read would deref the
    // ref `h` holds instead of forwarding it.
    let gets = gates(&r, gc::VAR_GET);
    assert_eq!(gets, [src], "one read of h");
    let (h, _) = source_port_of(&r, src, WirePort::VarRef).unwrap();
    assert_eq!(class_of(&r, h), gc::VAR);
}

#[test]
fn an_array_element_binds_an_array_param() {
    let r = compile_ok(
        "var rows: int[][]\nmod add1(xs: int[]) { xs.push(1) }\nin go: exec\non go { add1(rows[0]) }",
    );
    let push = gates(&r, gc::ARRAY_PUSH)[0];
    let (src, _) = source_port_of(&r, push, WirePort::ArrayVarRef).unwrap();
    assert_eq!(class_of(&r, src), gc::ARRAY_GET);
}

#[test]
fn a_ref_value_crosses_a_chip_boundary_as_a_pin() {
    let r = compile_ok("var h: *int\nchip set7(p: *int) { p = 7 }\nin go: exec\non go { set7(h) }");
    let chip = r.module.chips.values().next().expect("one chip");
    let set = chip
        .nodes
        .values()
        .find(|n| n.gate_class == gc::VAR_SET)
        .expect("Var_Set inside the chip")
        .id;
    let pin = chip
        .wires
        .iter()
        .find(|w| w.target.node_id == set && w.target.port == WirePort::VarRef)
        .expect("VarRef wired")
        .source
        .node_id;
    assert!(
        chip.inputs.contains(&pin),
        "the ref arrives on the chip's input pin"
    );
    let feed = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == pin)
        .expect("pin fed from the caller")
        .source
        .node_id;
    assert_eq!(
        class_of(&r, feed),
        gc::VAR_GET,
        "fed from a read of h, not h's own VarRef"
    );
}

#[test]
fn event_data_is_a_ref_both_ways() {
    let r = compile_ok(
        "var p0: color\nvar px: (*color)[]\nin go: exec\n\
         on CustomEvent(\"reg\") -> (r: *color) { px.push(r)\n *r = Color(1.0, 0.0, 0.0) }\n\
         on go { SendCustomEvent(\"reg\", &p0) }",
    );
    // Send: the data input carries p0's VarRef.
    let send = gates(&r, gc::SEND_CUSTOM_EVENT)[0];
    assert!(
        r.module.wires.iter().any(|w| w.target.node_id == send
            && w.source.port == WirePort::VarRef
            && class_of(&r, w.source.node_id) == gc::VAR),
        "the send's data comes from p0's VarRef"
    );
    // Receive: the write's VarRef is the event's data output.
    let set = gates(&r, gc::VAR_SET)[0];
    let (src, _) = source_port_of(&r, set, WirePort::VarRef).unwrap();
    assert_eq!(class_of(&r, src), gc::CUSTOM_EVENT);
}

#[test]
fn a_pure_read_of_a_chip_ref_param_bound_to_a_runtime_ref_is_ws006() {
    let r = compile("var h: *int\nchip Get(p: *int) -> (r: int) { out r = p + 1 }\nlet g = Get(h)");
    assert!(has_error(&r, "WS006"), "{:?}", r.diagnostics);
}

#[test]
fn an_array_literal_binds_no_array_param() {
    let r = compile("mod add1(xs: int[]) { xs.push(1) }\nin go: exec\non go { add1([1, 2]) }");
    assert!(has_error(&r, "WS044"), "{:?}", r.diagnostics);
}

/// What feeds the pin that drives the Var_Set's VarRef inside the only chip.
fn chip_ref_pin_feed(r: &LowerResult) -> Option<(NodeId, WirePort)> {
    let chip = r.module.chips.values().next().expect("one chip");
    let set = chip
        .nodes
        .values()
        .find(|n| n.gate_class == gc::VAR_SET)?
        .id;
    let pin = chip
        .wires
        .iter()
        .find(|w| w.target.node_id == set && w.target.port == WirePort::VarRef)?
        .source
        .node_id;
    assert!(
        chip.inputs.contains(&pin),
        "the ref arrives on an input pin"
    );
    r.module
        .wires
        .iter()
        .find(|w| w.target.node_id == pin)
        .map(|w| (w.source.node_id, w.source.port))
}

const RECORD_REF_FIELD: &str = "var a: int = 0\ntype R = { p: *int, n: int }\n";
const MAP_OF_REFS: &str = "var a: int = 0\nvar m: Map<string, *int>\n";

/// A ref held in a record field or a map value binds a mod's ref parameter
/// and feeds a chip's ref pin: the write's VarRef (through the pin, for the
/// chip) comes from where the ref lives, a's own VarRef or the map read.
#[test]
fn a_ref_held_in_a_field_or_map_value_binds_a_ref_param_or_pin() {
    let field_body = "let rr: R = { p: &a, n: 1 }\n M7(rr.p)";
    let map_body = "m.set(\"k\", &a)\n let e = m.get(\"k\")\n M7(e.Value)";
    let rows: &[(&str, &str, &str, bool, (&str, Option<WirePort>))] = &[
        (
            "a ref record field binds a mod's ref param",
            RECORD_REF_FIELD,
            field_body,
            false,
            (gc::VAR, Some(WirePort::VarRef)),
        ),
        (
            "a ref record field feeds a chip's ref pin",
            RECORD_REF_FIELD,
            field_body,
            true,
            (gc::VAR, Some(WirePort::VarRef)),
        ),
        (
            "a map value ref binds a mod's ref param",
            MAP_OF_REFS,
            map_body,
            false,
            (gc::MAP_GET, None),
        ),
        (
            "a map value ref feeds a chip's ref pin",
            MAP_OF_REFS,
            map_body,
            true,
            (gc::MAP_GET, None),
        ),
    ];
    let mut failures = Vec::new();
    for &(label, head, body, chip, (want_class, want_port)) in rows {
        let callee = if chip {
            "chip M7(q: *int) { q = 7 }"
        } else {
            "mod M7(q: *int) { q = 7 }"
        };
        let src = format!("{head}{callee}\nin go: exec\non go {{ {body} }}");
        let r = compile(&src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}\n{src}"));
            continue;
        }
        let fed = if chip {
            chip_ref_pin_feed(&r)
        } else {
            gates(&r, gc::VAR_SET)
                .first()
                .and_then(|set| source_port_of(&r, *set, WirePort::VarRef))
        };
        let got = fed.map(|(n, p)| (class_of(&r, n), p));
        let ok = got.is_some_and(|(c, p)| c == want_class && want_port.is_none_or(|w| w == p));
        if !ok {
            failures.push(format!("{label}: the write's ref comes from {got:?}, want {want_class} {want_port:?}\n{src}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn a_non_ref_field_never_reaches_a_ref_param() {
    // A scalar field of a record value, or a value map's read, has no ref on
    // its wire: binding it would feed a number into a `VarRef`.
    for body in [
        "let rr: R = { p: &a, n: 1 }\n M7(rr.n)",
        "mi.set(\"k\", 1)\n M7(mi.get(\"k\").Value)",
    ] {
        let r = compile(&format!(
            "{RECORD_REF_FIELD}var mi: Map<string, int>\nmod M7(q: *int) {{ q = 7 }}\nin go: exec\n\
             on go {{ {body} }}"
        ));
        assert!(has_error(&r, "WS008"), "{body}: {:?}", r.diagnostics);
    }
    let r = compile(&format!(
        "{MAP_OF_REFS}var n: int = 0\nin go: exec\non go {{ let e = m.get(\"k\")\n n = e.Value + 1 }}"
    ));
    assert!(
        has_error(&r, "WS004"),
        "a ref is not a number: {:?}",
        r.diagnostics
    );
}

/// The gate that finally drives `(node, port)`, following chip input and
/// output pins anywhere in the chip tree.
pub(super) fn driver_through_pins(
    root: &crate::ir::Module,
    node: NodeId,
    port: WirePort,
) -> (&'static str, WirePort) {
    let (src, src_port) = root
        .wires_deep()
        .find(|w| w.target.node_id == node && w.target.port == port)
        .map(|w| (w.source.node_id, w.source.port))
        .expect("driven");
    match root
        .find_deep(src)
        .map(|n| n.gate_class)
        .expect("a known node")
    {
        gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT => {
            driver_through_pins(root, src, WirePort::RerInput)
        }
        c => (c, src_port),
    }
}

#[test]
fn a_ref_output_is_driven_by_the_vars_varref() {
    let r = compile_ok("var b: int = 0\nout r: *int = &b");
    let out = r
        .module
        .nodes
        .values()
        .find(|n| n.kind == crate::ir::NodeKind::Output)
        .unwrap()
        .id;
    assert_eq!(
        driver_through_pins(&r.module, out, WirePort::RerInput),
        (gc::VAR, WirePort::VarRef),
        "`&b` is b's VarRef"
    );
}

#[test]
fn a_chip_ref_output_is_fed_from_the_ref_its_param_names() {
    let head = "var a: int = 0\nvar h: *int\nchip Src(x: *int) -> (r: *int) { out r = &x }\n\
                chip Dst(p: *int) { p = 9 }\nin go: exec\n";
    // What Dst's write takes its VarRef from, through Src's `r` pin.
    let target_of_write = |r: &LowerResult| {
        let dst = r
            .module
            .chips
            .values()
            .find_map(|c| c.nodes.values().find(|n| n.gate_class == gc::VAR_SET))
            .expect("Dst's write")
            .id;
        driver_through_pins(&r.module, dst, WirePort::VarRef)
    };
    // Bound statically, `&x` is the caller's storage: a's own VarRef.
    let r = compile_ok(&format!("{head}on go {{ let s = Src(a)\n Dst(s) }}"));
    assert_eq!(
        target_of_write(&r),
        (gc::VAR, WirePort::VarRef),
        "r carries a's ref, not its value"
    );
    // Bound dynamically, `&x` is the ref that arrived on x's pin: the read of h.
    let r = compile_ok(&format!("{head}on go {{ let s = Src(h)\n Dst(s) }}"));
    assert_eq!(target_of_write(&r).0, gc::VAR_GET);
}

#[test]
fn an_array_literal_binds_no_chip_array_param() {
    // A constant literal is an immutable container, so mutating it is WS044.
    let r = compile("chip Add1(xs: int[]) { xs.push(1) }\nin go: exec\non go { Add1([1, 2]) }");
    assert!(has_error(&r, "WS044"), "{:?}", r.diagnostics);
    // A literal with a runtime element names no storage at all: reported at
    // the call even when the body never uses the parameter.
    let r = compile("chip Keep(xs: int[]) { }\nin go: exec\nin n: int\non go { Keep([n, 2]) }");
    assert!(has_error(&r, "WS044"), "{:?}", r.diagnostics);
}

#[test]
fn a_dropped_ref_initializer_says_the_ref_starts_null() {
    // `&a` bakes; a var's held ref is only known at run time, so `= h` drops.
    let r = compile("var a: int = 0\nvar h: *int = &a\nvar g: *int = h");
    let w = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WSP001")
        .expect("the dropped initializer warns");
    assert!(
        w.message.contains("null ref") && w.message.contains("handler"),
        "{}",
        w.message
    );
}

/// An integer literal written through a `*float` ref is the float it names, as
/// in `var x: float = 0`: accepted, through a ref parameter and through a
/// ref-holding var alike: each write's value is a float literal on a `float`
/// port, so folding and the game read the same value.
#[test]
fn an_int_literal_written_through_a_float_ref_is_a_float() {
    let r = compile_ok(
        "var x: float = 1
var y: float = 1
var h: *float
         mod z(f: *float) { *f = 0 }
         in go: exec
         on go {
  z(&x)
  h = &y
  *h = -2
}",
    );
    let mut written = Vec::new();
    for (id, n) in &r.module.nodes {
        if n.gate_class != gc::VAR_SET {
            continue;
        }
        let port_ty = n
            .ports
            .inputs
            .iter()
            .find(|p| p.name == *crate::intern::sym::VALUE)
            .map(|p| &p.ty);
        let value = n
            .properties
            .get(&crate::intern::sym::VALUE)
            .cloned()
            .or_else(|| {
                r.module
                    .wires
                    .iter()
                    .find(|w| w.target.node_id == *id && w.target.port == WirePort::Value)
                    .and_then(|w| r.module.nodes.get(&w.source.node_id))
                    .filter(|src| src.gate_class == gc::LITERAL)
                    .and_then(|src| src.properties.get(&crate::intern::sym::VALUE).cloned())
            });
        if let Some(v) = value {
            written.push((v, port_ty.cloned()));
        }
    }
    let float_on_float = |want: f64| {
        written.iter().any(|(v, t)| {
            matches!(v, crate::ir::Literal::Float(n) if *n == want)
                && matches!(t, Some(Type::Float))
        })
    };
    assert!(float_on_float(0.0) && float_on_float(-2.0), "{written:?}");
}
