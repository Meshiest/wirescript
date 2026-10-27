//! Records whose `*T` fields name storage (places) or carry refs (values):
//! every route a record takes keeps each field's kind, so a place passed to a
//! `*T` parameter wires its var's `VarRef`, a ref value wires the ref, a place
//! reads its var's value and arithmetic on a ref value is WS004.

use super::refs::{driver_through_pins, gates, source_port_of};
use super::*;
use crate::ir::gate_class as gc;
use crate::ir::{Module, NodeId};

const MATH_ADD: &str = "BrickComponentType_Expr_MathAdd";

/// Every node of `class` anywhere in the chip tree, in source order.
fn in_tree(m: &Module, class: &str) -> Vec<NodeId> {
    let mut v = nodes_of(m, class);
    v.sort_by_key(|n| (n.source_range.start.offset, n.source_range.end.offset));
    v.into_iter().map(|n| n.id).collect()
}

/// The `Var` gate declared by the source line starting `decl` (`"var a"`).
pub(super) fn var_node(r: &LowerResult, src: &str, decl: &str) -> NodeId {
    let at = src.find(decl).expect("declaration in source");
    r.module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::VAR && n.source_range.start.offset == at)
        .expect("the var's gate")
        .id
}

/// What finally drives the `VarRef` of the only `Var_Increment` in the tree.
fn increment_target(r: &LowerResult) -> (NodeId, &'static str, WirePort) {
    let inc = in_tree(&r.module, gc::VAR_INCREMENT);
    assert_eq!(inc.len(), 1, "one increment: {inc:?}");
    let (class, port) = driver_through_pins(&r.module, inc[0], WirePort::VarRef);
    let node = find_driver_node(&r.module, inc[0], WirePort::VarRef);
    (node, class, port)
}

/// The node `driver_through_pins` stops at.
fn find_driver_node(root: &Module, node: NodeId, port: WirePort) -> NodeId {
    let src = root
        .wires_deep()
        .find(|w| w.target.node_id == node && w.target.port == port)
        .map(|w| w.source.node_id)
        .expect("driven");
    match root.find_deep(src).expect("known node").gate_class {
        gc::MICROCHIP_INPUT | gc::MICROCHIP_OUTPUT => {
            find_driver_node(root, src, WirePort::RerInput)
        }
        _ => src,
    }
}

/// No wire anywhere in the tree feeds a var's VALUE into a `VarRef`.
fn no_value_into_varref(root: &Module) {
    for m in root.modules_deep() {
        for w in &m.wires {
            assert!(
                !(w.target.port == WirePort::VarRef
                    && w.source.port == WirePort::Value
                    && m.nodes
                        .get(&w.source.node_id)
                        .is_some_and(|n| n.gate_class == gc::VAR)),
                "a var's value wired into a VarRef: {w:?}"
            );
        }
    }
}

const CPU: &str = "type Cpu = { regs: int[], cpsr: *int, skip: *bool }\n\
var regs: int[]\nvar cpsr: int = 0\nvar skip: bool = false\nvar n: int = 0\n\
mod M7(x: *int) { x = x + 1 }\nin go: exec\n";

#[test]
fn a_place_bundle_through_a_let_projection_names_its_var() {
    let src = format!(
        "{CPU}on go {{ let cpu = {{ regs, cpsr, skip }}\n let o = {{ c: cpu }}\n let s = o.c\n \
         M7(s.cpsr)\n n = s.cpsr + 1 }}"
    );
    let r = compile_ok(&src);
    let (node, class, port) = increment_target(&r);
    assert_eq!((class, port), (gc::VAR, WirePort::VarRef));
    assert_eq!(node, var_node(&r, &src, "var cpsr"));
    // The pure-looking read is a read of cpsr's value.
    let add = gates(&r, MATH_ADD)[0];
    let (get, _) = source_port_of(&r, add, WirePort::InputA).unwrap();
    assert_eq!(class_of(&r, get), gc::VAR_GET);
    assert_eq!(
        source_port_of(&r, get, WirePort::VarRef),
        Some((var_node(&r, &src, "var cpsr"), WirePort::VarRef))
    );
    no_value_into_varref(&r.module);
}

#[test]
fn a_tuple_pick_of_a_place_bundle_names_its_var() {
    let src = format!(
        "{CPU}on go {{ let cpu = {{ regs, cpsr, skip }}\n let t = (cpu, 1)\n let s = t.0\n M7(s.cpsr) }}"
    );
    let r = compile_ok(&src);
    let (node, class, port) = increment_target(&r);
    assert_eq!(
        (node, class, port),
        (var_node(&r, &src, "var cpsr"), gc::VAR, WirePort::VarRef)
    );
}

#[test]
fn a_place_through_a_multi_output_mod_result_passes_its_ref() {
    let src = format!(
        "{CPU}mod mk() -> (c: Cpu, k: int) {{ return {{ c: {{ regs, cpsr, skip }}, k: 1 }} }}\n\
         on go {{ let t = mk()\n M7(t.c.cpsr) }}"
    );
    let r = compile_ok(&src);
    let (node, class, port) = increment_target(&r);
    assert_eq!(
        (node, class, port),
        (var_node(&r, &src, "var cpsr"), gc::VAR, WirePort::VarRef)
    );
    no_value_into_varref(&r.module);
    // A call's result is a value: its `*T` field is the ref, not a number.
    let r = compile(&format!(
        "{CPU}mod mk() -> (c: Cpu, k: int) {{ return {{ c: {{ regs, cpsr, skip }}, k: 1 }} }}\n\
         on go {{ let t = mk()\n n = t.c.cpsr + 1 }}"
    ));
    assert!(has_error(&r, "WS004"), "{:?}", r.diagnostics);
}

const REC: &str = "type R = { x: *int }\nvar a: int = 0\nvar x: int = 0\nvar h: *int\nvar n: int = 0\n\
mod M7(x: *int) { x = x + 1 }\nin go: exec\n";

#[test]
fn a_ref_value_field_of_a_mod_result_is_not_a_number() {
    for ret in ["{ x: &a }", "{ x: h }"] {
        let r = compile(&format!(
            "{REC}mod mk() -> R {{ return {ret} }}\non go {{ let q = mk()\n n = q.x + 1 }}"
        ));
        assert!(has_error(&r, "WS004"), "{ret}: {:?}", r.diagnostics);
        let r = compile(&format!(
            "{REC}mod mk() -> R {{ return {ret} }}\non go {{ n = mk().x + 1 }}"
        ));
        assert!(has_error(&r, "WS004"), "{ret}: {:?}", r.diagnostics);
    }
    let src = format!(
        "{REC}mod mk() -> R {{ return {{ x: &a }} }}\non go {{ let q = mk()\n n = *q.x + 1 }}"
    );
    let r = compile_ok(&src);
    let add = gates(&r, MATH_ADD)[0];
    let (get, _) = source_port_of(&r, add, WirePort::InputA).unwrap();
    assert_eq!(class_of(&r, get), gc::VAR_GET);
    assert_eq!(
        source_port_of(&r, get, WirePort::VarRef),
        Some((var_node(&r, &src, "var a"), WirePort::VarRef))
    );
}

#[test]
fn a_place_a_call_returns_leaves_as_its_ref() {
    for kw in ["mod", "chip"] {
        for call in ["M7(mk().x)", "let q = mk()\n M7(q.x)"] {
            let src = format!("{REC}{kw} mk() -> R {{ return {{ x }} }}\non go {{ {call} }}");
            let r = compile_ok(&src);
            let (node, class, port) = increment_target(&r);
            assert_eq!(
                (node, class, port),
                (var_node(&r, &src, "var x"), gc::VAR, WirePort::VarRef),
                "{kw} {call}"
            );
            no_value_into_varref(&r.module);
        }
    }
}

#[test]
fn a_ref_value_bound_to_a_record_params_field_is_read_through() {
    // `q.p` is a place inside the mod; the caller's ref names which one.
    let src = "type R = { p: *int }\nvar a: int = 0\nvar n: int = 0\nmod f(q: R) { n = q.p + 1 }\n\
         in go: exec\non go { let rr = { p: &a }\n f(rr) }"
        .to_string();
    let r = compile_ok(&src);
    let add = gates(&r, MATH_ADD)[0];
    let (get, _) = source_port_of(&r, add, WirePort::InputA).unwrap();
    assert_eq!(
        class_of(&r, get),
        gc::VAR_GET,
        "a read through the ref, not the ref itself"
    );
    assert_eq!(
        source_port_of(&r, get, WirePort::VarRef),
        Some((var_node(&r, &src, "var a"), WirePort::VarRef))
    );

    // A ref-holding var: the ref it holds, read at the call.
    let src = "type R = { p: *int }\nvar a: int = 0\nvar h: *int\nvar n: int = 0\n\
               mod f(q: R) { n = q.p + 1 }\nin go: exec\non go { h = &a\n f({ p: h }) }";
    let r = compile_ok(src);
    let add = gates(&r, MATH_ADD)[0];
    let (get, _) = source_port_of(&r, add, WirePort::InputA).unwrap();
    assert_eq!(class_of(&r, get), gc::VAR_GET);
    let (ref_src, _) = source_port_of(&r, get, WirePort::VarRef).unwrap();
    assert_eq!(
        class_of(&r, ref_src),
        gc::VAR_GET,
        "the ref comes off a read of h"
    );
    assert_eq!(
        source_port_of(&r, ref_src, WirePort::VarRef),
        Some((var_node(&r, src, "var h"), WirePort::VarRef))
    );

    // Writing the field writes the var the ref names.
    let src = "type R = { p: *int }\nvar a: int = 0\nmod M7(x: *int) { x = x + 1 }\n\
               mod f(q: R) { M7(q.p) }\nin go: exec\non go { f({ p: &a }) }";
    let r = compile_ok(src);
    let (node, class, port) = increment_target(&r);
    assert_eq!(
        (node, class, port),
        (var_node(&r, src, "var a"), gc::VAR, WirePort::VarRef)
    );

    // A pure read has no chain to read through on.
    let r = compile(
        "type R = { p: *int }\nvar a: int = 0\nmod g(q: R) -> int { return q.p + 1 }\n\
         let rr: R = { p: &a }\nout o: int = g(rr)",
    );
    assert!(has_error(&r, "WS006"), "{:?}", r.diagnostics);
}

#[test]
fn a_ref_holding_var_in_a_record_literal_is_a_ref_value() {
    let head = "type R = { p: *int }\nvar a: int = 0\nvar h: *int\nvar n: int = 0\n\
                mod M7(x: *int) { x = x + 1 }\nin go: exec\n";
    let src = format!("{head}on go {{ h = &a\n let rr: R = {{ p: h }}\n M7(rr.p) }}");
    let r = compile_ok(&src);
    let (node, class, _) = increment_target(&r);
    assert_eq!(class, gc::VAR_GET, "M7 writes the var h points at, not h");
    assert_eq!(
        source_port_of(&r, node, WirePort::VarRef),
        Some((var_node(&r, &src, "var h"), WirePort::VarRef))
    );
    let r = compile(&format!(
        "{head}on go {{ h = &a\n let rr: R = {{ p: h }}\n n = rr.p + 1 }}"
    ));
    assert!(has_error(&r, "WS004"), "{:?}", r.diagnostics);
    let src = format!("{head}on go {{ h = &a\n let rr: R = {{ p: h }}\n *rr.p = 9 }}");
    let r = compile_ok(&src);
    let set = gates(&r, gc::VAR_SET).into_iter().find(|s| {
        source_port_of(&r, *s, WirePort::VarRef)
            .is_some_and(|(n, _)| class_of(&r, n) == gc::VAR_GET)
    });
    assert!(set.is_some(), "`*rr.p = 9` writes through the ref h holds");
}

#[test]
fn a_namespaced_record_let_keeps_its_ref_values() {
    let dep = (
        "u",
        "type R = { p: *int }\nvar a: int = 0\nlet rr: R = { p: &a }\n",
    );
    let r = compile_multi(
        "import * as u from \"u\"\nvar n: int = 0\nin go: exec\non go { n = u.rr.p + 1 }",
        &[dep],
    );
    assert!(has_error(&r, "WS004"), "{:?}", r.diagnostics);
    let r = compile_multi(
        "import * as u from \"u\"\nvar n: int = 0\nin go: exec\non go { n = *u.rr.p + 1 }",
        &[dep],
    );
    assert_no_errors(&r);
    let add = gates(&r, MATH_ADD)[0];
    let (get, _) = source_port_of(&r, add, WirePort::InputA).unwrap();
    assert_eq!(class_of(&r, get), gc::VAR_GET);
    let (a, port) = source_port_of(&r, get, WirePort::VarRef).unwrap();
    assert_eq!((class_of(&r, a), port), (gc::VAR, WirePort::VarRef));
}

#[test]
fn a_ref_value_in_a_chip_record_pin_is_read_through() {
    // Pure: no chain to read through.
    let r = compile(
        "type R = { p: *int }\nvar a: int = 0\nvar h: *int\nchip C(q: R) -> (o: int) { out o = q.p + 1 }\n\
         in go: exec\non go { h = &a }\nlet c = C({ p: h })\nout r: int = c",
    );
    assert!(has_error(&r, "WS006"), "{:?}", r.diagnostics);
    // On a chain, each ref-value form reaches the pin and is read through.
    for (arg, feed) in [
        ("{ p: &a }", gc::VAR),
        ("{ p: hs[0] }", gc::ARRAY_GET),
        ("{ p: e }", gc::ARRAY_GET),
    ] {
        let src = format!(
            "type R = {{ p: *int }}\nvar a: int = 0\nvar hs: (*int)[]\nvar n: int = 0\n\
             chip C(go: exec, q: R) {{ on go {{ n = q.p + 1 }} }}\nin go: exec\n\
             on go {{ hs.push(&a)\n let e = hs[0]\n C(go, {arg}) }}"
        );
        let r = compile_ok(&src);
        let add = in_tree(&r.module, MATH_ADD)[0];
        let chip = r
            .module
            .chips
            .values()
            .find(|c| c.nodes.contains_key(&add))
            .expect("the chip");
        let get = chip
            .wires
            .iter()
            .find(|w| w.target.node_id == add && w.target.port == WirePort::InputA)
            .unwrap()
            .source
            .node_id;
        assert_eq!(chip.nodes[&get].gate_class, gc::VAR_GET, "{arg}");
        let (class, _) = driver_through_pins(&r.module, get, WirePort::VarRef);
        assert_eq!(class, feed, "{arg}");
    }
}

#[test]
fn a_record_ref_output_of_a_record_ref_param_is_its_refs() {
    let src = "type P = { a: int, b: int }\nvar x: int = 0\nvar y: int = 0\n\
               chip S2(q: *P) -> (r: *P) { out r = &q }\nmod M7(v: *int) { v = v + 1 }\n\
               var r0: P = { a: 0, b: 0 }\nin go: exec\non go { let s = S2(r0)\n M7(s.a) }";
    let r = compile_ok(src);
    let (_, class, port) = increment_target(&r);
    assert_eq!((class, port), (gc::VAR, WirePort::VarRef));
}

#[test]
fn a_destructured_place_is_a_place_and_a_destructured_ref_a_ref() {
    let src = format!(
        "{CPU}on go {{ let cpu = {{ regs, cpsr, skip }}\n let {{ cpsr: cp }} = cpu\n M7(cp)\n n = cp + 1 }}"
    );
    let r = compile_ok(&src);
    let (node, class, port) = increment_target(&r);
    assert_eq!(
        (node, class, port),
        (var_node(&r, &src, "var cpsr"), gc::VAR, WirePort::VarRef)
    );
    let r = compile(&format!(
        "{CPU}var h: *int\non go {{ let cpu = {{ regs, cpsr, skip }}\n let {{ cpsr: cp }} = cpu\n h = cp }}"
    ));
    assert!(
        has_error(&r, "WS003"),
        "a place is not a ref value: {:?}",
        r.diagnostics
    );
    // Through a tuple pattern, a nested record keeps its places.
    let src = format!(
        "{CPU}on go {{ let cpu = {{ regs, cpsr, skip }}\n let (c, k) = (cpu, 1)\n M7(c.cpsr)\n n = c.cpsr + k }}"
    );
    let r = compile_ok(&src);
    let (node, class, port) = increment_target(&r);
    assert_eq!(
        (node, class, port),
        (var_node(&r, &src, "var cpsr"), gc::VAR, WirePort::VarRef)
    );
    // A destructured ref value stays a ref, nested or not.
    for body in [
        "let { p } = { p: &a }\n n = p + 1",
        "let { o } = { o: { p: &a } }\n n = o.p + 1",
        "let { k, ...more } = { k: 1, p: &a }\n n = more.p + k",
    ] {
        let r = compile(&format!("{CPU}var a: int = 0\non go {{ {body} }}"));
        assert!(has_error(&r, "WS004"), "{body}: {:?}", r.diagnostics);
    }
}

#[test]
fn a_record_chosen_by_if_or_match_chooses_the_refs_of_its_places() {
    let head = format!("{CPU}var cpsr2: int = 0\nenum E {{ A, B }}\nin c: bool\nvar e: E = E.A\n");
    for pick in ["if c then a else b", "match e { A => a, B => b }"] {
        let src = format!(
            "{head}mod f(a: Cpu, b: Cpu) {{ let s = {pick}\n M7(s.cpsr) }}\n\
             on go {{ f({{ regs, cpsr, skip }}, {{ regs, cpsr: cpsr2, skip }}) }}"
        );
        let r = compile_ok(&src);
        let (node, class, _) = increment_target(&r);
        assert_eq!(class, gc::SELECT, "{pick}");
        for input in [WirePort::InputA, WirePort::InputB] {
            let (v, port) = source_port_of(&r, node, input).unwrap();
            assert_eq!(
                (class_of(&r, v), port),
                (gc::VAR, WirePort::VarRef),
                "{pick}: a ref, not a value"
            );
        }
        let r = compile(&format!(
            "{head}mod f(a: Cpu, b: Cpu) {{ let s = {pick}\n n = s.cpsr + 1 }}\n\
             on go {{ f({{ regs, cpsr, skip }}, {{ regs, cpsr: cpsr2, skip }}) }}"
        ));
        assert!(has_error(&r, "WS004"), "{pick}: {:?}", r.diagnostics);
    }
}

#[test]
fn a_ref_value_bound_to_a_record_params_field_is_written_through() {
    for (kw, call) in [("mod", "bump({ p: &a })"), ("chip", "bump(go, { p: &a })")] {
        let head = if kw == "mod" {
            "mod bump(q: R)"
        } else {
            "chip bump(g: exec, q: R)"
        };
        let body = if kw == "mod" {
            "q.p = 5"
        } else {
            "on g { q.p = 5 }"
        };
        let src = format!(
            "type R = {{ p: *int }}\nvar a: int = 0\n{head} {{ {body} }}\nin go: exec\non go {{ {call} }}"
        );
        let r = compile_ok(&src);
        let set = in_tree(&r.module, gc::VAR_SET);
        assert_eq!(set.len(), 1, "{kw}");
        assert_eq!(
            driver_through_pins(&r.module, set[0], WirePort::VarRef),
            (gc::VAR, WirePort::VarRef),
            "{kw}: the write lands on a"
        );
    }
}

#[test]
fn a_destructured_namespace_member_names_its_var() {
    let dep = (
        "u",
        "type Cpu = { regs: int[], cpsr: *int, skip: *bool }\nvar regs: int[]\nvar cpsr: int = 0\n\
         var skip: bool = false\nlet cpu = { regs, cpsr, skip }\nlet { cpsr: cp } = cpu\n\
         let o = { c: cpu }\nlet { c } = o\n",
    );
    for body in ["M7(u.cp)", "M7(u.c.cpsr)", "h = &u.cp\n *h = *h + 1"] {
        let r = compile_multi(
            &format!(
                "import * as u from \"u\"\nvar h: *int\nmod M7(v: *int) {{ v = v + 1 }}\nin go: exec\non go {{ {body} }}"
            ),
            &[dep],
        );
        assert_no_errors(&r);
        let (_, class, port) = increment_target(&r);
        let (class, port) = if class == gc::VAR_GET {
            (gc::VAR, WirePort::VarRef)
        } else {
            (class, port)
        };
        assert_eq!((class, port), (gc::VAR, WirePort::VarRef), "{body}");
        no_value_into_varref(&r.module);
    }
}

#[test]
fn a_ref_of_a_record_var_is_a_record_of_its_refs() {
    let src = "type P = { a: int, b: int }\nvar r: P = { a: 0, b: 0 }\nvar n: int = 0\n\
               mod M7(x: *int) { x = x + 1 }\nin go: exec\non go { let x = &r\n M7(x.a)\n n = *x.b }";
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module));
    let (_, class, port) = increment_target(&r);
    assert_eq!((class, port), (gc::VAR, WirePort::VarRef));
    let get = gates(&r, gc::VAR_GET).into_iter().last().unwrap();
    let (b, port) = source_port_of(&r, get, WirePort::VarRef).unwrap();
    assert_eq!(
        (class_of(&r, b), port),
        (gc::VAR, WirePort::VarRef),
        "`*x.b` reads r.b through its ref"
    );
}

#[test]
fn a_deref_of_an_enum_payload_ref_goes_through_the_ref() {
    let src = "enum E { A(*int), B }\nvar a: int = 0\nvar n: int = 0\nvar e: E = E.B\nin go: exec\n\
               on go { e = E.A(&a)\n match e { A(p) => { *p = 9\n n = *p + 1 }, B => { } } }";
    let r = compile_ok(src);
    let a = var_node(&r, src, "var a");
    let set9 = gates(&r, gc::VAR_SET)
        .into_iter()
        .find(|s| r.module.nodes[s].source_range.start.offset == src.find("*p = 9").unwrap())
        .expect("the write");
    let (src_node, _) = source_port_of(&r, set9, WirePort::VarRef).unwrap();
    assert_ne!(
        src_node, a,
        "not a itself: the ref read off the payload slot"
    );
    assert_eq!(
        class_of(&r, src_node),
        gc::VAR_GET,
        "the ref comes off a read of the payload slot"
    );
    let add = gates(&r, MATH_ADD)[0];
    let (get, _) = source_port_of(&r, add, WirePort::InputA).unwrap();
    assert_eq!(class_of(&r, get), gc::VAR_GET);
    let (inner, _) = source_port_of(&r, get, WirePort::VarRef).unwrap();
    assert_eq!(
        class_of(&r, inner),
        gc::VAR_GET,
        "`*p` reads through the ref, not the slot"
    );
}

#[test]
fn a_ref_value_pin_read_sits_at_the_use() {
    let src = "type R = { p: *int }\nvar a: int = 0\nvar h: *int\nvar n: int = 0\n\
               chip C(g: exec, q: R) { on g { n = q.p + 1 } }\nin go: exec\non go { h = &a\n C(go, { p: h }) }";
    let r = compile_ok(src);
    let use_at = src.find("{ p: h }").unwrap();
    let get = gates(&r, gc::VAR_GET)[0];
    let at = r.module.nodes[&get].source_range.start.offset;
    assert!(
        at >= use_at,
        "the read of h is placed at its use, not at `type R`: {at}"
    );
}

#[test]
fn a_destructured_tuple_of_vars_names_them() {
    let src = "var x: int = 0\nvar y: int = 0\nmod M7(v: *int) { v = v + 1 }\nin go: exec\n\
               on go { let (p, q) = (x, y)\n M7(q) }";
    let r = compile_ok(src);
    let (node, class, port) = increment_target(&r);
    assert_eq!(
        (node, class, port),
        (var_node(&r, src, "var y"), gc::VAR, WirePort::VarRef)
    );
}

/// A named field filled from a var is a place exactly as the shorthand is:
/// `return { x: a }` into a `*int` field is accepted as `return { x }` is, and
/// both bind the returned field to the var they name.
#[test]
fn a_named_place_field_returns_like_the_shorthand() {
    let head = "type R = { x: *int }\nvar x: int = 0\nvar a: int = 0\n\
                mod M7(v: *int) { v = v + 1 }\nin go: exec\nout r: int = a + x\n";
    for (body, target) in [("return { x }", "var x"), ("return { x: a }", "var a")] {
        let src = format!("{head}mod mk() -> R {{ {body} }}\non go {{ M7(mk().x) }}");
        let r = compile_ok(&src);
        let (node, _, port) = increment_target(&r);
        assert_eq!(
            (node, port),
            (var_node(&r, &src, target), WirePort::VarRef),
            "{body}"
        );
    }
    // A value (a plain parameter) names no var: still WS003.
    let r = compile(
        "type R = { x: *int }\nmod mk(v: int) -> R { return { x: v } }\n\
         in go: exec\nvar n: int = 0\non go { n = *mk(3).x }",
    );
    assert!(has_error(&r, "WS003"), "{:?}", r.diagnostics);
}
