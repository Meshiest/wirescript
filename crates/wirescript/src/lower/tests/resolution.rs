//! The recorded method/container resolutions as lowering reads them, and the
//! `container_ref` resolver's steps.

use super::*;
use crate::ir::gate_class as gc;
use crate::typecheck::{ContainerKind, MethodTarget};
use std::cell::RefCell;

fn only(r: &LowerResult, class: &str) -> crate::ir::NodeId {
    let v: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == class)
        .map(|n| n.id)
        .collect();
    assert_eq!(v.len(), 1, "expected one {class}, got {}", v.len());
    v[0]
}

/// One method call as lowering reached it: the key in effect, the table
/// entries stored under exactly that key (no generic fallback), and what the
/// public lookups answered.
#[derive(Debug)]
pub(in crate::lower) struct Probe {
    start: usize,
    subst: std::sync::Arc<[Type]>,
    target: Option<MethodTarget>,
    receiver: Option<ContainerKind>,
    api_target: Option<MethodTarget>,
    api_receiver: Option<ContainerKind>,
    fallback: bool,
    mono_depth: usize,
}

thread_local! {
    static PROBES: RefCell<Option<Vec<Probe>>> = const { RefCell::new(None) };
}

/// Called by `lower_call` for every `recv.name(..)`; records only while
/// [`probed`] is collecting on this thread.
pub(in crate::lower) fn probe_method_call(ctx: &LowerCtx<'_>, call: &Expr) {
    let collecting = PROBES.with(|p| p.borrow().is_some());
    if !collecting {
        return;
    }
    let Expr::Call { callee, .. } = call else {
        return;
    };
    let Expr::FieldAccess { obj, .. } = callee.as_ref() else {
        return;
    };
    let probe = Probe {
        start: call.range().start.offset,
        subst: std::sync::Arc::clone(&ctx.resolution_subst),
        target: ctx
            .resolutions
            .method_targets
            .get(&ctx.resolution_key(call))
            .cloned(),
        receiver: ctx
            .resolutions
            .container_kinds
            .get(&ctx.resolution_key(obj))
            .cloned(),
        api_target: ctx.method_target(call),
        api_receiver: ctx.container_kind(obj),
        fallback: ctx.resolution_fallback_allowed(),
        mono_depth: ctx.mono_stack.len(),
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
    let start = src
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not in source"));
    probes.iter().filter(|p| p.start == start).collect()
}

fn array_method(elem: Type, method: &str) -> MethodTarget {
    MethodTarget::Container {
        kind: ContainerKind::Array { elem },
        method: method.into(),
    }
}

/// Asserts the call was lowered once under each key in `keys`, and that each
/// time the tables held its entry under exactly that key.
fn assert_hits(
    src: &str,
    probes: &[Probe],
    needle: &str,
    method: &str,
    keys: &[(Vec<Type>, Type)],
) {
    let got = at(src, probes, needle);
    let substs: Vec<&[Type]> = got.iter().map(|p| &*p.subst).collect();
    let want: Vec<&[Type]> = keys.iter().map(|(k, _)| k.as_slice()).collect();
    assert_eq!(substs, want, "`{needle}` lowered under these keys");
    for (p, (_, elem)) in got.iter().zip(keys) {
        assert_eq!(
            p.target,
            Some(array_method(elem.clone(), method)),
            "`{needle}` target under {:?}",
            p.subst
        );
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

/// `add1`'s body was type-checked once, non-generically, with the empty key, so
/// inlined inside `outer<T>` it finds its entry under that key and lowers
/// exactly as it does from a non-generic caller: the generic caller's frame is
/// active, but the generic fallback is not used.
#[test]
fn a_top_level_mod_inlined_in_a_generic_body_uses_the_empty_key() {
    let src = "mod add1(xs: int[]) { xs.push(1) }\nmod outer<T: int | string>(rows: int[][], t: T) { add1(rows[0]) }\n\
               var a: int[][]\nin go: exec\non go { outer(a, 5) }";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    let push = only(&r, gc::ARRAY_PUSH);
    assert_eq!(
        class_of(&r, source_of(&r, push, WirePort::ArrayVarRef).unwrap()),
        gc::ARRAY_GET
    );
    assert_hits(src, &probes, "xs.push(1)", "push", &[(vec![], Type::Int)]);
    let got = at(src, &probes, "xs.push(1)");
    assert_eq!(got.len(), 1, "one probe: {probes:?}");
    assert_eq!(got[0].mono_depth, 1, "the generic caller's frame is active");
    assert!(
        !got[0].fallback,
        "a non-generic body must not use the generic fallback"
    );
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
        &[
            (vec![Type::Int], Type::Int),
            (vec![Type::String], Type::String),
        ],
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
        &[
            (vec![Type::Int, Type::Int], Type::Int),
            (vec![Type::String, Type::Int], Type::Int),
        ],
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
    assert_hits(
        src,
        &probes,
        "xs.length()",
        "length",
        &[(vec![Type::Int], Type::Int)],
    );
}

#[test]
fn a_generic_calls_arguments_lower_under_the_callers_frame() {
    // `xs.length()` is the non-generic caller's expression: it lowers with no
    // mono frame and never guesses a missing entry.
    let src = "var xs: int[]\nvar n: int = 0\n\
               mod id<A: int | float>(a: A) -> A { return a }\n\
               in go: exec\non go { n = id(xs.length()) }";
    let (r, probes) = probed(src);
    assert_no_errors(&r);
    let got = at(src, &probes, "xs.length()");
    assert_eq!(got.len(), 1, "one probe: {probes:?}");
    assert!(got[0].subst.is_empty());
    assert_eq!(
        got[0].mono_depth, 0,
        "the callee's frame must not be active while its arguments lower"
    );
    assert!(
        !got[0].fallback,
        "a non-generic caller's argument must not use the generic fallback"
    );
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
        &[
            (vec![Type::Int], Type::Int),
            (vec![Type::String], Type::String),
        ],
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
    let tc = typecheck(&parsed.ast, &crate::typecheck::CeSlotMap::default());
    let Some(TopDecl::Chip(chip)) = parsed.ast.decls.first() else {
        panic!("expected the chip first")
    };
    PROBES.with(|p| *p.borrow_mut() = Some(Vec::new()));
    crate::lower::compile_chip_template(chip, &tc, "test", &Arc::new(TemplateCache::new()));
    let probes = PROBES.with(|p| p.borrow_mut().take()).expect("collecting");
    let got = at(src, &probes, "xs.push(1)");
    assert_eq!(got.len(), 1, "one probe: {probes:?}");
    let p = got[0];
    assert!(p.subst.is_empty());
    assert_eq!(p.target, None, "no entry under the empty key");
    assert_eq!(p.api_target, Some(array_method(Type::Int, "push")));
    assert_eq!(
        p.api_receiver,
        Some(ContainerKind::Array { elem: Type::Int })
    );
}

#[test]
fn method_and_index_on_one_receiver_agree() {
    // A var holding an array ref is used directly: every method, index write
    // and index read takes cur's held ref (a read of cur).
    let r = compile_ok(
        "var rows: int[][]\nvar cur: *int[]\nvar n: int = 0\nin go: exec\n\
         on go { cur = rows[0]\n cur.push(1)\n n = cur.length()\n cur[0] = 2\n n = cur[0] }",
    );
    for class in [gc::ARRAY_PUSH, gc::ARRAY_GET_LENGTH, gc::ARRAY_SET_AT_INDEX] {
        let g = only(&r, class);
        assert_eq!(
            class_of(&r, source_of(&r, g, WirePort::ArrayVarRef).unwrap()),
            gc::VAR_GET,
            "{class}"
        );
    }
    let reads: Vec<_> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_GET)
        .map(|n| n.id)
        .collect();
    assert!(reads.iter().any(
        |g| source_of(&r, *g, WirePort::ArrayVarRef).map(|s| class_of(&r, s)) == Some(gc::VAR_GET)
    ));
}

#[test]
fn const_container_reads_are_unchanged() {
    // a constant index folds to no gate; a runtime index reads
    // the materialized array.
    let folded = compile_ok("const T = [1, 2]\nvar n: int = 0\nin go: exec\non go { n = T[1] }");
    assert_eq!(count_class(&folded.module, gc::ARRAY_GET), 0);
    let runtime = compile_ok(
        "const T = [1, 2]\nvar i: int = 0\nvar n: int = 0\nin go: exec\non go { n = T[i] }",
    );
    let g = only(&runtime, gc::ARRAY_GET);
    assert_eq!(
        class_of(
            &runtime,
            source_of(&runtime, g, WirePort::ArrayVarRef).unwrap()
        ),
        gc::ARRAY_VAR
    );
}

#[test]
fn container_ref_steps_resolve_each_receiver_shape() {
    // Static binding: var, `in` port, record field, runtime-bound parameter.
    let r = compile_ok(
        "type R = { xs: int[] }\nvar rec: R\nvar a: int[]\nin ia: int[]\nvar n: int = 0\n\
         mod rd(p: int[]) -> int { return p[0] }\nvar rows: int[][]\nin go: exec\n\
         on go { n = a[0]\n n = ia[0]\n n = rec.xs[0]\n n = rd(rows[1]) }",
    );
    let sources: Vec<&'static str> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_GET)
        .filter_map(|n| source_of(&r, n.id, WirePort::ArrayVarRef).map(|s| class_of(&r, s)))
        .collect();
    assert!(
        sources.contains(&gc::ARRAY_VAR),
        "static var / record field: {sources:?}"
    );
    assert!(
        sources.contains(&gc::MICROCHIP_INPUT),
        "in port: {sources:?}"
    );
    assert!(
        sources.contains(&gc::ARRAY_GET),
        "runtime-bound parameter reads its element: {sources:?}"
    );
    // Nothing: a field holding a placeholder is still WS044.
    let e =
        compile("var n: int = 0\nin go: exec\non go { let r = { t: [n, 2] }\n n = r.t.length() }");
    crate::test_support::assert_code(&e.diagnostics, "WS044");
}

/// The gate class wired into `class`'s single instance at `port`.
fn ref_source_of(r: &LowerResult, class: &str, port: WirePort) -> &'static str {
    class_of(
        r,
        source_of(r, only(r, class), port)
            .unwrap_or_else(|| panic!("{class} has no {port:?} wire")),
    )
}

const NS_LIB: (&str, &str) = ("lib", "var xs: int[]\nvar m: Map<string, int>");

/// A write into a namespace member's container emits its set gate, with the
/// member's own storage as the ref.
#[test]
fn a_namespace_member_container_write_emits_its_set() {
    let rows: &[(&str, &str, &str, WirePort, &str)] = &[
        (
            "an array index write",
            "on go { u.xs[1] = 2 }",
            gc::ARRAY_SET_AT_INDEX,
            WirePort::ArrayVarRef,
            gc::ARRAY_VAR,
        ),
        (
            "a map subscript write",
            "on go { u.m[\"b\"] = 3 }",
            gc::MAP_SET,
            WirePort::MapVarRef,
            gc::MAP_VAR,
        ),
        (
            "SetArrayElement",
            "on go { SetArrayElement(u.xs, 1, 2) }",
            gc::ARRAY_SET_AT_INDEX,
            WirePort::ArrayVarRef,
            gc::ARRAY_VAR,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, body, class, port, want) in rows {
        let r = compile_multi(
            &format!("import * as u from \"lib\"\nin go: exec\n{body}"),
            &[NS_LIB],
        );
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let sets: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == class)
            .map(|n| n.id)
            .collect();
        let got = match sets.as_slice() {
            [one] => source_of(&r, *one, port).map(|s| class_of(&r, s)),
            _ => None,
        };
        if !errors.is_empty() || got != Some(want) {
            failures.push(format!(
                "{label}: errors {errors:?}, {} x {class}, ref from {got:?}, want {want}",
                sets.len()
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn an_unresolved_index_write_target_is_ws044() {
    let r = compile_multi(
        "import * as u from \"lib\"\nin go: exec\non go { u.nope[0] = 1 }",
        &[NS_LIB],
    );
    crate::test_support::assert_code(&r.diagnostics, "WS044");
    assert_eq!(count_class(&r.module, gc::ARRAY_SET_AT_INDEX), 0);
}

#[test]
fn index_writes_take_their_ref_from_each_receiver_shape() {
    let port = compile_ok("in ia: int[]\nin go: exec\non go { ia[0] = 1 }");
    assert_eq!(
        ref_source_of(&port, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef),
        gc::MICROCHIP_INPUT
    );

    let field =
        compile_ok("type R = { xs: int[] }\nvar rec: R\nin go: exec\non go { rec.xs[0] = 1 }");
    assert_eq!(
        ref_source_of(&field, gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef),
        gc::ARRAY_VAR
    );

    let element = compile_ok("var ms: Map<string, int>[]\nin go: exec\non go { ms[0][\"q\"] = 3 }");
    assert_eq!(
        ref_source_of(&element, gc::MAP_SET, WirePort::MapVarRef),
        gc::ARRAY_GET
    );
}

const P: &str = "type P = { x: int, y: int }\n";

/// A write into one field of a record container indexes that field's own
/// column storage, named by its flattened path, whatever reaches it: the
/// container, a parameter bound to it, a namespace, an enclosing record or a
/// column of a column.
#[test]
fn a_record_column_write_uses_the_column() {
    const ARR: (&str, WirePort, &str) =
        (gc::ARRAY_SET_AT_INDEX, WirePort::ArrayVarRef, gc::ARRAY_VAR);
    const MAP: (&str, WirePort, &str) = (gc::MAP_SET, WirePort::MapVarRef, gc::MAP_VAR);
    let q = "type Q = { p: P }\n";
    let lib = ("lib", "type P = { x: int, y: int }\nvar pts: P[]");
    let rows: Vec<(&str, String, &[(&str, &str)], (&str, WirePort, &str), &str)> = vec![
        (
            "an array column index write",
            format!("{P}var pts: P[]\nin go: exec\non go {{ pts.x[0] = 5 }}"),
            &[],
            ARR,
            "pts.x",
        ),
        (
            "a map column subscript write",
            format!("{P}var pm: Map<string, P>\nin go: exec\non go {{ pm.x[\"k\"] = 5 }}"),
            &[],
            MAP,
            "pm.x",
        ),
        (
            "through a `ps: P[]` parameter",
            format!(
                "{P}var pts: P[]\nmod g(ps: P[]) {{ ps.x[0] = 5 }}\nin go: exec\non go {{ g(pts) }}"
            ),
            &[],
            ARR,
            "pts.x",
        ),
        (
            "through a `ps: *P[]` parameter",
            format!(
                "{P}var pts: P[]\nmod g(ps: *P[]) {{ ps.x[0] = 5 }}\nin go: exec\non go {{ g(pts) }}"
            ),
            &[],
            ARR,
            "pts.x",
        ),
        (
            "through a map parameter",
            format!(
                "{P}var pm: Map<string, P>\nmod g(m: Map<string, P>) {{ m.x[\"k\"] = 5 }}\nin go: exec\non go {{ g(pm) }}"
            ),
            &[],
            MAP,
            "pm.x",
        ),
        (
            "a namespace record array column",
            "import * as u from \"lib\"\nin go: exec\non go { u.pts.x[0] = 5 }".to_string(),
            std::slice::from_ref(&lib),
            ARR,
            "pts.x",
        ),
        (
            "a column of a record array inside a record",
            format!(
                "{P}type H = {{ pts: P[] }}\nvar h: H\nin go: exec\non go {{ h.pts.x[0] = 5 }}"
            ),
            &[],
            ARR,
            "h.pts.x",
        ),
        (
            "a column of an array column",
            format!("{P}{q}var qs: Q[]\nin go: exec\non go {{ qs.p.x[0] = 5 }}"),
            &[],
            ARR,
            "qs.p.x",
        ),
        (
            "a column of a map column",
            format!("{P}{q}var qm: Map<string, Q>\nin go: exec\non go {{ qm.p.x[\"k\"] = 5 }}"),
            &[],
            MAP,
            "qm.p.x",
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, deps, (class, port, want_class), want_label) in &rows {
        let r = if deps.is_empty() {
            compile(src)
        } else {
            compile_multi(src, deps)
        };
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let sets: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == *class)
            .map(|n| n.id)
            .collect();
        let src_node = match sets.as_slice() {
            [one] => source_of(&r, *one, *port),
            _ => None,
        };
        let got_class = src_node.map(|s| class_of(&r, s));
        let got_label = src_node.and_then(|s| {
            match r.module.nodes[&s]
                .properties
                .get(&crate::intern::sym::NAME_LABEL)
            {
                Some(crate::ir::Literal::String(l)) => Some(l.clone()),
                _ => None,
            }
        });
        if !errors.is_empty()
            || got_class != Some(*want_class)
            || got_label.as_deref() != Some(*want_label)
        {
            failures.push(format!(
                "{label}: errors {errors:?}, {} x {class}, ref from {got_class:?} {got_label:?}, want {want_class} {want_label:?}\n{src}",
                sets.len()
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

#[test]
fn dual_array_arguments_and_loop_sources_resolve_through_the_record() {
    let r = compile_ok(
        "var rows: int[][]\nvar all: int[]\nvar cur: *int[]\nvar n: int = 0\nin go: exec\n\
         on go { all.append(rows[1])\n cur = rows[0]\n for v in cur { n = v } }",
    );
    // append's source array comes off rows[1].
    assert_eq!(
        ref_source_of(&r, gc::ARRAY_APPEND, WirePort::SourceRef),
        gc::ARRAY_GET
    );
    // The loop shell's driver pin takes cur's held ref (a Var_Get), not cur's own VarRef.
    let shell = r
        .module
        .nodes
        .values()
        .find(|n| crate::ir::gate_class::is_plane_shell(n.gate_class))
        .expect("a for shell");
    assert_eq!(
        class_of(&r, source_of(&r, shell.id, WirePort::ArrayVarRef).unwrap()),
        gc::VAR_GET
    );
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
            match r.module.nodes[&s]
                .properties
                .get(&crate::intern::sym::NAME_LABEL)
            {
                Some(crate::ir::Literal::String(l)) => l.clone(),
                other => panic!("unlabelled spread source: {other:?}"),
            }
        })
        .collect();
    sources.sort();
    assert_eq!(sources, ["pts.x", "xs"]);
}

/// An enum-element `in` port is a container source for the dual-array and
/// parallel-array methods: the method takes it straight from the port, with no
/// placeholder.
#[test]
fn an_enum_container_in_port_is_a_method_source() {
    let color = "enum Color { Red, Green, Blue }\n";
    let rows: &[(&str, &str, &str, WirePort)] = &[
        (
            "a dual array source",
            "in ia: Color[]\nin ib: Color[]\nin go: exec\non go { ib.append(ia) }",
            gc::ARRAY_APPEND,
            WirePort::SourceRef,
        ),
        (
            "an enum-valued map copyFrom source",
            "in ma: Map<string, Color>\nin mb: Map<string, Color>\nin go: exec\non go { mb.copyFrom(ma) }",
            gc::MAP_COPY_FROM,
            WirePort::SourceRef,
        ),
        (
            "a sortMultiple parallel array",
            "in ia: Color[]\nvar k: int[]\nin go: exec\non go { k.sortMultiple(ia) }",
            gc::ARRAY_SORT_MULTIPLE,
            WirePort::ArrayVarRef2,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, body, class, port) in rows {
        let src = format!("{color}{body}");
        let r = compile(&src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let gates: Vec<_> = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == class)
            .map(|n| n.id)
            .collect();
        let got = match gates.as_slice() {
            [one] => source_of(&r, *one, port).map(|s| class_of(&r, s)),
            _ => None,
        };
        if !errors.is_empty() || got != Some(gc::MICROCHIP_INPUT) || has_unsupported(&r.module) {
            failures.push(format!(
                "{label}: errors {errors:?}, {} x {class}, {port:?} from {got:?}\n{src}",
                gates.len()
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());

    let r = compile_ok(&format!(
        "{color}in ia: Color[]\nvar k: int[]\nin go: exec\non go {{ k.sortMultiple(ia) }}"
    ));
    assert_eq!(
        source_of(&r, only(&r, gc::ARRAY_SORT_MULTIPLE), WirePort::BDescending),
        None
    );
}

#[test]
fn each_generic_instantiation_uses_its_own_element_type() {
    // A push's `Value` input is always `any`; a pop's `Value` output carries
    // the element type the dispatch was handed.
    let r = compile_ok(
        "mod popAll<T: int | string>(rows: T[][], v: T) { rows[0].push(v)\n let p = rows[0].pop() }\n\
         var a: int[][]\nvar b: string[][]\nin go: exec\non go { popAll(a, 1)\n popAll(b, \"x\") }",
    );
    let mut value_types: Vec<String> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == gc::ARRAY_POP)
        .map(|n| {
            n.ports
                .outputs
                .iter()
                .find(|p| p.name == *crate::intern::sym::VALUE)
                .map(|p| p.ty.to_string())
                .unwrap_or_default()
        })
        .collect();
    value_types.sort();
    assert_eq!(value_types, vec!["int".to_string(), "string".to_string()]);
}

#[test]
fn builtin_and_self_receivers_still_lower() {
    let r = compile_ok(
        "mod twice(self: int) -> int { return self * 2 }\nvar n: int = 1\nvar v: vector = Vec(1.0, 0.0, 0.0)\n\
         var d: float = 0.0\nin go: exec\non go { n = n.twice()\n d = v.Dot(v) }",
    );
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class.contains("Dot")),
        "the builtin receiver lowered"
    );
    assert!(
        r.module
            .nodes
            .values()
            .any(|n| n.gate_class.contains("MathMultiply")),
        "the self-mod receiver's body (`self * 2`) lowered"
    );
    assert!(
        !r.module
            .nodes
            .values()
            .any(|n| n.gate_class == gc::UNSUPPORTED),
        "neither method call fell back to the _Unsupported placeholder"
    );
}
