//! The recorded method/container resolutions as lowering reads them, and the
//! `container_ref` resolver's steps.

use super::*;
use crate::ir::gate_class as gc;
use crate::typecheck::{ContainerKind, MethodTarget};
use std::cell::RefCell;

fn class_of(r: &LowerResult, id: crate::ir::NodeId) -> &'static str {
    r.module.nodes[&id].gate_class
}

/// The node wired into `(node, port)`.
fn source_of(r: &LowerResult, node: crate::ir::NodeId, port: WirePort) -> Option<crate::ir::NodeId> {
    r.module.wires.iter().find(|w| w.target.node_id == node && w.target.port == port).map(|w| w.source.node_id)
}

fn only(r: &LowerResult, class: &str) -> crate::ir::NodeId {
    let v: Vec<_> = r.module.nodes.values().filter(|n| n.gate_class == class).map(|n| n.id).collect();
    assert_eq!(v.len(), 1, "expected one {class}, got {}", v.len());
    v[0]
}

#[test]
fn a_non_generic_body_inside_a_generic_uses_the_empty_key() {
    // Review Focus 4: `add1`'s body was type-checked once with the empty key,
    // so inlined inside `outer<T>` it must still find its entry.
    let r = compile(
        "mod add1(xs: int[]) { xs.push(1) }\nmod outer<T: int | string>(rows: int[][], t: T) { add1(rows[0]) }\n\
         var a: int[][]\nin go: exec\non go { outer(a, 5) }",
    );
    assert_no_errors(&r);
    let push = only(&r, gc::ARRAY_PUSH);
    assert_eq!(class_of(&r, source_of(&r, push, WirePort::ArrayVarRef).unwrap()), gc::ARRAY_GET);
}

/// One method call as lowering reached it: the key in effect, the table
/// entries stored under exactly that key (no generic fallback), and what the
/// public lookups answered.
#[derive(Debug)]
pub(in crate::lower) struct Probe {
    start: usize,
    subst: Vec<Type>,
    target: Option<MethodTarget>,
    receiver: Option<ContainerKind>,
    api_target: Option<MethodTarget>,
    api_receiver: Option<ContainerKind>,
}

thread_local! {
    static PROBES: RefCell<Option<Vec<Probe>>> = const { RefCell::new(None) };
}

/// Called by `lower_call` for every `recv.name(..)`; records only while
/// [`probed`] is collecting on this thread.
pub(in crate::lower) fn probe_method_call(ctx: &LowerCtx, call: &Expr) {
    let collecting = PROBES.with(|p| p.borrow().is_some());
    if !collecting {
        return;
    }
    let Expr::Call { callee, .. } = call else { return };
    let Expr::FieldAccess { obj, .. } = callee.as_ref() else { return };
    let probe = Probe {
        start: call.range().start.offset,
        subst: ctx.resolution_subst.clone(),
        target: ctx.resolutions.method_targets.get(&ctx.resolution_key(call)).cloned(),
        receiver: ctx.resolutions.container_kinds.get(&ctx.resolution_key(obj)).cloned(),
        api_target: ctx.method_target(call),
        api_receiver: ctx.container_kind(obj),
    };
    PROBES.with(|p| p.borrow_mut().as_mut().expect("collecting").push(probe));
}

/// Lower `src` and return every method call lowering reached.
fn probed(src: &str) -> (LowerResult, Vec<Probe>) {
    PROBES.with(|p| *p.borrow_mut() = Some(Vec::new()));
    let r = compile(src);
    let probes = PROBES.with(|p| p.borrow_mut().take()).expect("collecting");
    (r, probes)
}

/// The probes at the call whose source text is `needle`.
fn at<'p>(src: &str, probes: &'p [Probe], needle: &str) -> Vec<&'p Probe> {
    let start = src.find(needle).unwrap_or_else(|| panic!("`{needle}` not in source"));
    probes.iter().filter(|p| p.start == start).collect()
}

fn array_method(elem: Type, method: &str) -> MethodTarget {
    MethodTarget::Container { kind: ContainerKind::Array { elem }, method: method.into() }
}

/// Asserts the call was lowered once under each key in `keys`, and that each
/// time the tables held its entry under exactly that key.
fn assert_hits(src: &str, probes: &[Probe], needle: &str, method: &str, keys: &[(Vec<Type>, Type)]) {
    let got = at(src, probes, needle);
    let substs: Vec<&Vec<Type>> = got.iter().map(|p| &p.subst).collect();
    let want: Vec<&Vec<Type>> = keys.iter().map(|(k, _)| k).collect();
    assert_eq!(substs, want, "`{needle}` lowered under these keys");
    for (p, (_, elem)) in got.iter().zip(keys) {
        assert_eq!(p.target, Some(array_method(elem.clone(), method)), "`{needle}` target under {:?}", p.subst);
        assert_eq!(
            p.receiver,
            Some(ContainerKind::Array { elem: elem.clone() }),
            "`{needle}` receiver under {:?}",
            p.subst
        );
        assert_eq!(p.api_target, p.target);
        assert_eq!(p.api_receiver, p.receiver);
    }
}

#[test]
fn a_top_level_mod_inlined_in_a_generic_body_finds_its_entry_under_the_empty_key() {
    let src = "mod add1(xs: int[]) { xs.push(1) }\nmod outer<T: int | string>(rows: int[][], t: T) { add1(rows[0]) }\n\
               var a: int[][]\nin go: exec\non go { outer(a, 5) }";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    assert_hits(src, &probes, "xs.push(1)", "push", &[(vec![], Type::Int)]);
}

#[test]
fn a_mod_nested_in_a_generic_body_keys_by_the_enclosing_instantiation() {
    let src = "mod pushAll<T: int | string>(rows: T[][], v: T) {\n\
                   mod inner(r: T[][], x: T) { r[0].push(x) }\n\
                   inner(rows, v)\n\
               }\n\
               var a: int[][]\nvar b: string[][]\nin go: exec\n\
               on go { pushAll(a, 1)\n pushAll(b, \"x\") }";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    assert_hits(
        src,
        &probes,
        "r[0].push(x)",
        "push",
        &[(vec![Type::Int], Type::Int), (vec![Type::String], Type::String)],
    );
}

#[test]
fn a_generic_nested_in_a_generic_keys_by_the_concatenated_stack() {
    let src = "mod outer<T: int | string>(v: T) {\n\
                   mod inner<U: int | float>(xs: U[], w: U) { xs.push(w) }\n\
                   var us: int[]\n\
                   inner(us, 1)\n\
               }\n\
               in go: exec\non go { outer(1)\n outer(\"x\") }";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    assert_hits(
        src,
        &probes,
        "xs.push(w)",
        "push",
        &[(vec![Type::Int, Type::Int], Type::Int), (vec![Type::String, Type::Int], Type::Int)],
    );
}

#[test]
fn an_argument_keeps_the_callers_key() {
    // `xs.length()` is `outer`'s expression even though `show` consumes it,
    // so it is keyed by `outer`'s instantiation, not by `show`'s empty key.
    let src = "mod show(n: int) -> int { return n + 1 }\n\
               mod outer<T: int | string>(xs: int[], t: T) -> int { return show(xs.length()) }\n\
               var a: int[]\nvar n: int = 0\nin go: exec\non go { n = outer(a, 5) }";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    assert_hits(src, &probes, "xs.length()", "length", &[(vec![Type::Int], Type::Int)]);
}

#[test]
fn an_instanced_generic_chip_seeds_its_body_key() {
    let src = "chip Acc<T: int | string>(v: T) {\n\
                   var xs: T[]\n\
                   in go: exec\n\
                   on go { xs.push(v) }\n\
               }\n\
               let a = Acc(1)\nlet b = Acc(\"x\")";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    assert_hits(
        src,
        &probes,
        "xs.push(v)",
        "push",
        &[(vec![Type::Int], Type::Int), (vec![Type::String], Type::String)],
    );
}

#[test]
fn an_estimated_generic_chip_body_resolves_through_the_fallback() {
    // `compile_chip_template` lowers a generic chip's body with no key and no
    // mono frame, while typecheck recorded that body only under its combo keys.
    let src = "chip Acc<T: int | string>(v: T) {\n\
                   var xs: int[]\n\
                   in go: exec\n\
                   on go { xs.push(1) }\n\
               }\n\
               let a = Acc(1)";
    let parsed = parse(src, "test");
    let tc = typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    let Some(TopDecl::Chip(chip)) = parsed.ast.decls.first() else { panic!("expected the chip first") };
    PROBES.with(|p| *p.borrow_mut() = Some(Vec::new()));
    crate::lower::compile_chip_template(chip, &tc, "test", &Arc::new(TemplateCache::new()));
    let probes = PROBES.with(|p| p.borrow_mut().take()).expect("collecting");
    let got = at(src, &probes, "xs.push(1)");
    assert_eq!(got.len(), 1, "one probe: {probes:?}");
    let p = got[0];
    assert!(p.subst.is_empty());
    assert_eq!(p.target, None, "no entry under the empty key");
    assert_eq!(p.api_target, Some(array_method(Type::Int, "push")));
    assert_eq!(p.api_receiver, Some(ContainerKind::Array { elem: Type::Int }));
}

#[test]
fn method_and_index_on_one_receiver_agree() {
    // Review Focus 1.
    let r = compile(
        "var rows: int[][]\nvar cur: *int[]\nvar n: int = 0\nin go: exec\n\
         on go { cur = rows[0]\n cur.push(1)\n cur[0] = 2\n n = cur[0] }",
    );
    assert_no_errors(&r);
    for class in [gc::ARRAY_PUSH, gc::ARRAY_SET_AT_INDEX] {
        let g = only(&r, class);
        assert_eq!(class_of(&r, source_of(&r, g, WirePort::ArrayVarRef).unwrap()), gc::VAR_GET, "{class}");
    }
    let reads: Vec<_> = r.module.nodes.values().filter(|n| n.gate_class == gc::ARRAY_GET).map(|n| n.id).collect();
    assert!(reads.iter().any(|g| source_of(&r, *g, WirePort::ArrayVarRef).map(|s| class_of(&r, s)) == Some(gc::VAR_GET)));
}

#[test]
fn const_container_reads_are_unchanged() {
    // Review Focus 2: a constant index folds to no gate; a runtime index reads
    // the materialized array.
    let folded = compile("const T = [1, 2]\nvar n: int = 0\nin go: exec\non go { n = T[1] }");
    assert_no_errors(&folded);
    assert_eq!(gate_count(&folded, gc::ARRAY_GET), 0);
    let runtime = compile("const T = [1, 2]\nvar i: int = 0\nvar n: int = 0\nin go: exec\non go { n = T[i] }");
    assert_no_errors(&runtime);
    let g = only(&runtime, gc::ARRAY_GET);
    assert_eq!(class_of(&runtime, source_of(&runtime, g, WirePort::ArrayVarRef).unwrap()), gc::ARRAY_VAR);
}

#[test]
fn container_ref_steps_resolve_each_receiver_shape() {
    // Static binding: var, `in` port, record field, runtime-bound parameter.
    let r = compile(
        "type R = { xs: int[] }\nvar rec: R\nvar a: int[]\nin ia: int[]\nvar n: int = 0\n\
         mod rd(p: int[]) -> int { return p[0] }\nvar rows: int[][]\nin go: exec\n\
         on go { n = a[0]\n n = ia[0]\n n = rec.xs[0]\n n = rd(rows[1]) }",
    );
    assert_no_errors(&r);
    let sources: Vec<&'static str> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_GET)
        .filter_map(|n| source_of(&r, n.id, WirePort::ArrayVarRef).map(|s| class_of(&r, s)))
        .collect();
    assert!(sources.contains(&gc::ARRAY_VAR), "static var / record field: {sources:?}");
    assert!(sources.contains(&gc::MICROCHIP_INPUT), "in port: {sources:?}");
    assert!(sources.contains(&gc::ARRAY_GET), "runtime-bound parameter reads its element: {sources:?}");
    // Nothing: a namespaced const in an unsupported position is still WS044.
    let e = compile("const T = [1, 2]\nlet r = { t: T }\nvar n: int = 0\nin go: exec\non go { n = r.t.length() }");
    assert!(e.diagnostics.iter().any(|d| d.code == "WS044"), "{:?}", e.diagnostics);
}

/// The gate class wired into `class`'s single instance at `port`.
fn ref_source_of(r: &LowerResult, class: &str, port: WirePort) -> &'static str {
    class_of(r, source_of(r, only(r, class), port).unwrap_or_else(|| panic!("{class} has no {port:?} wire")))
}

const NS_LIB: (&str, &str) = ("lib", "var xs: int[]\nvar m: Map<string, int>");

#[test]
fn a_namespace_member_array_index_write_emits_its_set() {
    let r = compile_multi("import * as u from \"lib\"\nin go: exec\non go { u.xs[1] = 2 }", &[NS_LIB]);
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);
}

#[test]
fn a_namespace_member_map_subscript_write_emits_its_set() {
    let r = compile_multi("import * as u from \"lib\"\nin go: exec\non go { u.m[\"b\"] = 3 }", &[NS_LIB]);
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::MAP_SET, WirePort::MapVarRef), gc::MAP_VAR);
}

#[test]
fn set_array_element_on_a_namespace_member_emits_its_set() {
    let r = compile_multi("import * as u from \"lib\"\nin go: exec\non go { SetArrayElement(u.xs, 1, 2) }", &[NS_LIB]);
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);
}

#[test]
fn an_unresolved_index_write_target_is_ws044() {
    let r = compile_multi("import * as u from \"lib\"\nin go: exec\non go { u.nope[0] = 1 }", &[NS_LIB]);
    assert!(r.diagnostics.iter().any(|d| d.code == "WS044"), "{:?}", r.diagnostics);
    assert_eq!(gate_count(&r, gc::ARRAY_SET_AT_INDEX), 0);
}

#[test]
fn index_writes_take_their_ref_from_each_receiver_shape() {
    let port = compile("in ia: int[]\nin go: exec\non go { ia[0] = 1 }");
    assert_no_errors(&port);
    assert_eq!(ref_source_of(&port, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::MICROCHIP_INPUT);

    let field = compile("type R = { xs: int[] }\nvar rec: R\nin go: exec\non go { rec.xs[0] = 1 }");
    assert_no_errors(&field);
    assert_eq!(ref_source_of(&field, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);

    let element = compile("var ms: Map<string, int>[]\nin go: exec\non go { ms[0][\"q\"] = 3 }");
    assert_no_errors(&element);
    assert_eq!(ref_source_of(&element, gc::MAP_SET, WirePort::MapVarRef), gc::ARRAY_GET);
}

/// The `NAME_LABEL` of the node wired into `class`'s single instance at `port`.
fn ref_source_label(r: &LowerResult, class: &str, port: WirePort) -> Option<String> {
    let src = source_of(r, only(r, class), port)?;
    match r.module.nodes[&src].properties.get(&crate::intern::sym::NAME_LABEL) {
        Some(crate::ir::Literal::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

const P: &str = "type P = { x: int, y: int }\n";

#[test]
fn a_record_array_column_index_write_uses_the_column() {
    let r = compile(&format!("{P}var pts: P[]\nin go: exec\non go {{ pts.x[0] = 5 }}"));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);
    assert_eq!(ref_source_label(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef).as_deref(), Some("pts.x"));
}

#[test]
fn a_record_map_column_subscript_write_uses_the_column() {
    let r = compile(&format!("{P}var pm: Map<string, P>\nin go: exec\non go {{ pm.x[\"k\"] = 5 }}"));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::MAP_SET, WirePort::MapVarRef), gc::MAP_VAR);
    assert_eq!(ref_source_label(&r, gc::MAP_SET, WirePort::MapVarRef).as_deref(), Some("pm.x"));
}

#[test]
fn column_writes_through_parameters_use_the_callers_column() {
    for param in ["ps: P[]", "ps: *P[]"] {
        let r = compile(&format!(
            "{P}var pts: P[]\nmod g({param}) {{ ps.x[0] = 5 }}\nin go: exec\non go {{ g(pts) }}"
        ));
        assert_no_errors(&r);
        assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR, "{param}");
        assert_eq!(
            ref_source_label(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef).as_deref(),
            Some("pts.x"),
            "{param}"
        );
    }
    let r = compile(&format!(
        "{P}var pm: Map<string, P>\nmod g(m: Map<string, P>) {{ m.x[\"k\"] = 5 }}\nin go: exec\non go {{ g(pm) }}"
    ));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::MAP_SET, WirePort::MapVarRef), gc::MAP_VAR);
    assert_eq!(ref_source_label(&r, gc::MAP_SET, WirePort::MapVarRef).as_deref(), Some("pm.x"));
}

#[test]
fn a_namespace_record_array_column_write_uses_the_column() {
    let lib = ("lib", "type P = { x: int, y: int }\nvar pts: P[]");
    let r = compile_multi("import * as u from \"lib\"\nin go: exec\non go { u.pts.x[0] = 5 }", &[lib]);
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);
    assert_eq!(ref_source_label(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef).as_deref(), Some("pts.x"));
}

#[test]
fn a_column_of_a_record_array_inside_a_record_index_write_uses_the_column() {
    let r = compile(&format!("{P}type H = {{ pts: P[] }}\nvar h: H\nin go: exec\non go {{ h.pts.x[0] = 5 }}"));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);
    assert_eq!(ref_source_label(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef).as_deref(), Some("h.pts.x"));
}

#[test]
fn a_column_of_a_column_index_write_uses_the_flattened_column() {
    let q = "type Q = { p: P }\n";
    let r = compile(&format!("{P}{q}var qs: Q[]\nin go: exec\non go {{ qs.p.x[0] = 5 }}"));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef), gc::ARRAY_VAR);
    assert_eq!(ref_source_label(&r, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef).as_deref(), Some("qs.p.x"));

    let m = compile(&format!("{P}{q}var qm: Map<string, Q>\nin go: exec\non go {{ qm.p.x[\"k\"] = 5 }}"));
    assert_no_errors(&m);
    assert_eq!(ref_source_of(&m, gc::MAP_SET, WirePort::MapVarRef), gc::MAP_VAR);
    assert_eq!(ref_source_label(&m, gc::MAP_SET, WirePort::MapVarRef).as_deref(), Some("qm.p.x"));
}

#[test]
fn dual_array_arguments_and_loop_sources_resolve_through_the_record() {
    let r = compile(
        "var rows: int[][]\nvar all: int[]\nvar cur: *int[]\nvar n: int = 0\nin go: exec\n\
         on go { all.append(rows[1])\n cur = rows[0]\n for v in cur { n = v } }",
    );
    assert_no_errors(&r);
    // append's source array comes off rows[1].
    assert_eq!(ref_source_of(&r, gc::ARRAY_APPEND, WirePort::SourceRef), gc::ARRAY_GET);
    // The loop shell's driver pin takes cur's held ref (a Var_Get), not cur's own VarRef.
    let shell = r.module.nodes.values().find(|n| crate::ir::gate_class::is_plane_shell(n.gate_class)).expect("a for shell");
    assert_eq!(class_of(&r, source_of(&r, shell.id, WirePort::ArrayVarRef).unwrap()), gc::VAR_GET);
}

#[test]
fn array_literal_spreads_resolve_through_the_record() {
    let r = compile_multi(
        &format!(
            "{P}import * as u from \"lib\"\nvar pts: P[]\nvar foo: int[]\nin go: exec\n\
             on go {{ foo = [0, ...u.xs, ...pts.x] }}"
        ),
        &[NS_LIB],
    );
    assert_no_errors(&r);
    let mut sources: Vec<String> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_APPEND)
        .filter_map(|n| source_of(&r, n.id, WirePort::SourceRef))
        .map(|s| {
            assert_eq!(class_of(&r, s), gc::ARRAY_VAR);
            match r.module.nodes[&s].properties.get(&crate::intern::sym::NAME_LABEL) {
                Some(crate::ir::Literal::String(l)) => l.to_string(),
                other => panic!("unlabelled spread source: {other:?}"),
            }
        })
        .collect();
    sources.sort();
    assert_eq!(sources, ["pts.x", "xs"]);
}

const COLOR: &str = "enum Color { Red, Green, Blue }
";

#[test]
fn an_enum_element_in_port_is_a_dual_array_source() {
    let r = compile(&format!("{COLOR}in ia: Color[]
in ib: Color[]
in go: exec
on go {{ ib.append(ia) }}"));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_APPEND, WirePort::SourceRef), gc::MICROCHIP_INPUT);
    assert_eq!(gate_count(&r, gc::UNSUPPORTED), 0);
}

#[test]
fn an_enum_valued_map_in_port_is_a_copy_from_source() {
    let r = compile(&format!(
        "{COLOR}in ma: Map<string, Color>
in mb: Map<string, Color>
in go: exec
on go {{ mb.copyFrom(ma) }}"
    ));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::MAP_COPY_FROM, WirePort::SourceRef), gc::MICROCHIP_INPUT);
}

#[test]
fn an_enum_element_in_port_is_a_sort_multiple_parallel_array() {
    let r = compile(&format!("{COLOR}in ia: Color[]
var k: int[]
in go: exec
on go {{ k.sortMultiple(ia) }}"));
    assert_no_errors(&r);
    assert_eq!(ref_source_of(&r, gc::ARRAY_SORT_MULTIPLE, WirePort::ArrayVarRef2), gc::MICROCHIP_INPUT);
    assert_eq!(source_of(&r, only(&r, gc::ARRAY_SORT_MULTIPLE), WirePort::BDescending), None);
}

#[test]
fn each_generic_instantiation_uses_its_own_element_type() {
    // A push's `Value` input is always `any`; a pop's `Value` output carries
    // the element type the dispatch was handed.
    let r = compile(
        "mod popAll<T: int | string>(rows: T[][], v: T) { rows[0].push(v)\n let p = rows[0].pop() }\n\
         var a: int[][]\nvar b: string[][]\nin go: exec\non go { popAll(a, 1)\n popAll(b, \"x\") }",
    );
    assert_no_errors(&r);
    let mut value_types: Vec<String> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_POP)
        .map(|n| {
            n.ports.outputs.iter().find(|p| p.name == *crate::intern::sym::VALUE).map(|p| p.ty.to_string()).unwrap_or_default()
        })
        .collect();
    value_types.sort();
    assert_eq!(value_types, vec!["int".to_string(), "string".to_string()]);
}

#[test]
fn record_container_methods_still_use_columns() {
    let r = compile(
        "type P = { x: int, y: int }\nvar pts: P[]\nvar n: int = 0\nin go: exec\n\
         on go { pts.push({ x: 1, y: 2 })\n n = pts.length() }",
    );
    assert_no_errors(&r);
    assert_eq!(gate_count(&r, gc::ARRAY_PUSH), 2, "one push per column");
}

#[test]
fn builtin_and_self_receivers_still_lower() {
    let r = compile(
        "mod twice(self: int) -> int { return self * 2 }\nvar n: int = 1\nvar v: vector = Vec(1.0, 0.0, 0.0)\n\
         var d: float = 0.0\nin go: exec\non go { n = n.twice()\n d = v.Dot(v) }",
    );
    assert_no_errors(&r);
    assert!(r.module.nodes.values().any(|n| n.gate_class.contains("Dot")), "the builtin receiver lowered");
    assert!(
        r.module.nodes.values().any(|n| n.gate_class.contains("MathMultiply")),
        "the self-mod receiver's body (`self * 2`) lowered"
    );
    assert!(
        !r.module.nodes.values().any(|n| n.gate_class == gc::UNSUPPORTED),
        "neither method call fell back to the _Unsupported placeholder"
    );
}
