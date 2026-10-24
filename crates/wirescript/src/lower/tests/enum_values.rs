//! Enum values crossing ports, outputs, containers and calls; `emit` of an
//! output or signal where it has nothing to fire; and the diagnostics for a
//! record, tuple or enum used where one value goes or indexed.

use super::ref_records::var_node;
use super::refs::{gates, source_port_of};
use super::*;
use crate::ir::NodeId;
use crate::test_support::report_failures;

const MATH_ADD: &str = "BrickComponentType_Expr_MathAdd";

#[test]
fn a_record_literal_can_be_a_match_arm() {
    let src = "type P = { x: int, y: int }\nenum E { A, B }\nvar e: E = E.A\nin go: exec\nvar n: int = 0\n\
               on go { let s = match e { A => { x: 1, y: 2 }, B => { x: 3, y: 4 } }\n n = s.x + s.y }";
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module));
}

#[test]
fn a_game_enum_crosses_a_port_as_its_value() {
    let src = "in ease: EasingFunction\nvar v: EasingFunction = EasingFunction.Bounce\nin go: exec\n\
               on go { v = ease }\nout d: int = ease.Discriminant\nout o = v\n";
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module));
    let input = r.module.inputs[0];
    let set = gates(&r, gc::VAR_SET)[0];
    assert_eq!(
        source_port_of(&r, set, WirePort::Value),
        Some((input, WirePort::RerOutput)),
        "v = ease stores it"
    );
    let outs: Vec<NodeId> = r.module.outputs.clone();
    for out in outs {
        assert!(
            source_port_of(&r, out, WirePort::RerInput).is_some(),
            "every output is wired"
        );
    }
}

#[test]
fn a_payload_free_enum_port_is_matched() {
    let src = "enum U { X, Y }\nin u: U\nvar n: int = 0\nin go: exec\non go { n = match u { X => 1, Y => 2 } }\n";
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module));
    let input = r.module.inputs[0];
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == input && w.source.port == WirePort::RerOutput),
        "the tag is read off the port"
    );
}

const HUE: &str =
    "enum Hue { Red, Green, Blue }\nvar c: Hue = Hue.Red\nin go: exec\nin go2: exec\n";

/// What drives the only output's rerouter, after the module emits.
fn enum_out_driver(src: &str) -> (LowerResult, (NodeId, WirePort)) {
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module), "{src}");
    assert_eq!(
        r.module.outputs.len(),
        1,
        "one pin for the enum output: {src}"
    );
    assert_emits(&r.module, &crate::layout::layout(&r.module));
    let out = r.module.outputs[0];
    let driver = source_port_of(&r, out, WirePort::RerInput)
        .unwrap_or_else(|| panic!("the output is wired: {src}"));
    (r, driver)
}

#[test]
fn an_emit_drives_a_payload_free_enum_output_with_its_tag() {
    for (body, feed) in [
        ("emit oc = c", gc::VAR_GET),
        ("emit oc = Hue.Blue", MATH_ADD),
        ("emit oc = f(c)", gc::VAR_GET),
        ("emit oc = f<Hue>(c)", gc::VAR_GET),
        ("emit oc = g(c)", gc::VAR_GET),
        ("let y = f(c)\n emit oc = y", gc::VAR_GET),
    ] {
        let src = format!(
            "{HUE}mod f<T: Hue | int>(x: T) -> (r: T) {{ return x }}\n\
             mod g(x: Hue) -> Hue {{ return x }}\nout oc: Hue\non go {{ {body} }}\n"
        );
        let (r, (node, _)) = enum_out_driver(&src);
        assert_eq!(class_of(&r, node), feed, "{body}");
    }
}

#[test]
fn a_payload_free_enum_output_written_from_several_sites_is_backed() {
    for tail in [
        "out oc: Hue\non go { emit oc = c }\non go2 { emit oc = Hue.Blue }\n",
        "out oc: Hue = Hue.Green\non go { emit oc = c }\n",
        "out oc: Hue\non go { if c is Hue.Red { emit oc = Hue.Green } }\n",
        "on go { out oc: Hue = c }\non go2 { out oc: Hue = Hue.Green }\n",
    ] {
        let src = format!("{HUE}{tail}");
        let (r, (node, port)) = enum_out_driver(&src);
        assert_eq!(
            (class_of(&r, node), port),
            (gc::VAR, WirePort::Value),
            "{tail}"
        );
        assert!(
            !gates(&r, gc::VAR_SET).is_empty(),
            "each write stores the tag: {tail}"
        );
    }
    let src = format!("{HUE}out oc: Hue = Hue.Green\non go {{ emit oc = c }}\n");
    let (r, (backing, _)) = enum_out_driver(&src);
    assert_eq!(
        r.module.nodes[&backing]
            .properties
            .get(&*sym::INITIAL_VALUE),
        Some(&crate::ir::Literal::Int(1)),
        "the default seeds the backing var with Green's tag"
    );
}

/// A bare `emit` of a value output has nothing to fire, wherever the output
/// is (top level, named or anonymous chip, mod, several pins): `WS057`, never
/// the internal `WS094` or the exec stored as the value. An exec or `any`
/// output, and a value `emit`, compile; an `any` output takes the exec it fires.
#[test]
fn a_bare_emit_of_a_value_output_is_ws057_in_every_scope() {
    let rejected = [
        format!("{HUE}out oc: Hue\non go {{ emit oc }}\n"),
        "type P = { x: int, y: int }\nin go: exec\nout p: P\non go { emit p }\n".to_string(),
        format!("{HUE}var d: Hue = Hue.Red\nmod f() -> (r: Hue) {{ emit r }}\non go {{ d = f() }}\n"),
        "type P = { x: int, y: int }\nvar q: P = { x: 0, y: 0 }\nin go: exec\nmod f() -> (r: P) { emit r }\non go { q = f() }\n"
            .to_string(),
        "var n: int = 0\nin go: exec\nmod f() -> (r: int) { emit r }\non go { n = f() }\n".to_string(),
        "var n: int = 0\nin go: exec\nout r: int\non go { emit r }\n".to_string(),
        "var n: int = 0\nin go: exec\nout r: int = 5\non go { emit r }\n".to_string(),
        "var n: int = 0\nin go: exec\nchip ch(x: int) -> (r: int) { on x { emit r } }\non go { n = ch(n) }\n".to_string(),
        "var n: int = 0\nin go: exec\nchip ch(x: int) -> (r: int) { out r = 2\n on x { emit r } }\non go { n = ch(n) }\n".to_string(),
        "var n: int = 0\nin go: exec\nchip { in a: exec\n out r: int\n on a { emit r } }\non go { n += 1 }\n".to_string(),
        "var n: int = 0\nin go: exec\nchip { in a: exec\n out r: int = 3\n on a { emit r } }\non go { n += 1 }\n".to_string(),
        "var n: int = 0\nin go: exec\nout r: float\non go { emit r }\n".to_string(),
    ];
    let accepted = [
        "var n: int = 0\nin go: exec\nmod m() -> (done: exec) { emit done }\non go { m()\n n = 1 }\n",
        "var n: int = 0\nin go: exec\nout r: int\non go { emit r = n }\n",
        "var n: int = 0\nin go: exec\nout o: exec\non go { emit o }\n",
        "var n: int = 0\nin go: exec\nchip { in a: exec\n out o: exec\n on a { emit o } }\non go { n += 1 }\n",
        "var n: int = 0\nin go: exec\nout r: any\non go { emit r }\n",
    ];
    let mut failures = Vec::new();
    for src in &rejected {
        let r = compile(src);
        if !has_error(&r, "WS057") || has_error(&r, "WS094") {
            failures.push(format!("want WS057 alone: {}\n{src}", messages(&r)));
        }
    }
    for src in accepted {
        let r = compile(src);
        if !crate::test_support::error_codes(&r.diagnostics).is_empty() {
            failures.push(format!("want no error: {}\n{src}", messages(&r)));
        }
    }
    report_failures(&failures, rejected.len() + accepted.len());
    let r = compile("in go: exec\nout r: any\non go { emit r }\n");
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| r.module.outputs.contains(&w.target.node_id)),
        "{}",
        messages(&r)
    );
}

/// An `emit` of a local signal where no exec chain runs, bare or with a
/// value, is `WS007`: nothing would ever fire it. On a handler's chain it
/// compiles.
#[test]
fn an_emit_of_a_signal_outside_an_exec_context_is_ws007() {
    for emit in ["emit sig = 5", "emit sig"] {
        let src = format!(
            "in go: exec\nvar n: int = 0\nchip C() {{\n let sig: exec\n {emit}\n on sig {{ n = 1 }}\n}}\nC()\n"
        );
        let r = compile(&src);
        assert!(has_error(&r, "WS007"), "`{emit}`: {}", messages(&r));
    }
    compile_ok(
        "in go: exec\nvar n: int = 0\nlet sig: exec\non go { emit sig }\non sig { n = 1 }\n",
    );
}

/// Indexing a record, tuple or enum says what to write instead, and names the
/// value it indexes without an internal placeholder.
#[test]
fn indexing_a_record_tuple_or_enum_says_how_to_read_it() {
    for (decl, read, hint) in [
        (
            "type P = { a: int, b: int }\nvar r: P = { a: 1, b: 2 }",
            "r[0]",
            "`r.a`",
        ),
        (
            "type P = { a: int }\nvar r: P = { a: 1 }\nmod f(x: P) -> P { return x }",
            "f(r)[0]",
            "(`.a`)",
        ),
        ("var r: { int: Hue } = { int: Hue.Red }", "r[1]", "`r.int`"),
        (
            "var r: (int, int) = (1, 2)",
            "r[0]",
            "an element by its position (`r.0`)",
        ),
        (
            "const r = (1, 2)",
            "r[1]",
            "an element by its position (`r.0`)",
        ),
        ("var r: Hue = Hue.Red", "r[0]", "`match`"),
    ] {
        let src =
            format!("enum Hue {{ Red, Green }}\n{decl}\nin go: exec\non go {{ let v = {read} }}\n");
        let r = compile(&src);
        let ws044: Vec<&str> = r
            .diagnostics
            .iter()
            .filter(|d| d.code == "WS044")
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(ws044.len(), 1, "{read}: {}", messages(&r));
        assert!(ws044[0].contains(hint), "{read}: {}", ws044[0]);
        assert!(!ws044[0].contains("<expr>"), "{read}: {}", ws044[0]);
    }
}

#[test]
fn an_any_value_assigned_to_a_record_is_an_error() {
    let r = compile(
        "type P = { x: int, y: int }\nvar q: P = { x: 1, y: 2 }\nin a: any\nin go: exec\non go { q = a }\n",
    );
    assert!(has_error(&r, "WS003"), "{}", messages(&r));
    assert!(!has_error(&r, "WS094"), "{}", messages(&r));
}

#[test]
fn an_element_of_a_port_array_of_enum_tags_is_that_enum() {
    let src = "enum C { Red, Green }\nin cs: C[]\nvar s: C = C.Red\nvar n: int = 0\nin go: exec\n\
               on go { s = cs[0]\n n = cs[1].Discriminant + match cs[0] { Red => 1, Green => 2 } }";
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module));
    let s = var_node(&r, src, "var s");
    let set = gates(&r, gc::VAR_SET)
        .into_iter()
        .find(|g| source_port_of(&r, *g, WirePort::VarRef).is_some_and(|(n, _)| n == s))
        .expect("s = cs[0] stores");
    let (get, _) = source_port_of(&r, set, WirePort::Value).unwrap();
    assert_eq!(class_of(&r, get), gc::ARRAY_GET);
}

const EASE: &str = "in ease: EasingFunction\nvar e: EasingFunction = EasingFunction.Linear\nvar es: EasingFunction[]\n\
var tw: float = 0.0\nvar b: bool = false\nvar n: int = 0\nin c: bool\nin go: exec\n";

/// Every error and warning message, for asserting none leaks an internal name.
fn messages(r: &LowerResult) -> String {
    r.diagnostics
        .iter()
        .map(|d| format!("[{}] {}", d.code, d.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What drives `port` of the only gate of `class`.
fn driver(r: &LowerResult, class: &str, port: WirePort) -> (&'static str, WirePort) {
    let g = gates(r, class);
    assert_eq!(g.len(), 1, "one {class}");
    let (src, p) =
        source_port_of(r, g[0], port).unwrap_or_else(|| panic!("{class}.{port:?} is wired"));
    (class_of(r, src), p)
}

#[test]
fn a_payload_free_enum_is_one_value_wherever_one_is_expected() {
    // Builtin arguments, named and positional, from the port and from a var.
    for (body, feed) in [
        (
            "tw = Tween(10.0, 1.0, function = ease)",
            gc::MICROCHIP_INPUT,
        ),
        ("tw = Tween(10.0, 1.0, ease)", gc::MICROCHIP_INPUT),
        ("tw = Tween(10.0, 1.0, function = e)", gc::VAR_GET),
        (
            "let x = ease\n tw = Tween(10.0, 1.0, function = x)",
            gc::MICROCHIP_INPUT,
        ),
    ] {
        let r = compile_ok(&format!("{EASE}on go {{ {body} }}"));
        assert!(!has_unsupported(&r.module), "{body}");
        let (class, _) = driver(&r, "BrickComponentType_Tween", WirePort::Function);
        assert_eq!(class, feed, "{body}");
    }
    // Stored into an element, chosen by `if`, and sent as event data.
    for body in [
        "es.push(ease)\n es[0] = ease",
        "e = if c then ease else e",
        "let x = ease\n e = x",
        "SendCustomEvent(\"k\", ease)",
    ] {
        let r = compile_ok(&format!("{EASE}on go {{ {body} }}"));
        assert!(!has_unsupported(&r.module), "{body}");
        let input = r.module.inputs[0];
        assert!(
            r.module
                .wires
                .iter()
                .any(|w| w.source.node_id == input && w.source.port == WirePort::RerOutput),
            "{body}: the port's tag is read"
        );
    }
    // A mod parameter and `is` read the tag too.
    let r = compile_ok(&format!(
        "{EASE}mod ident(v: EasingFunction) -> int {{ return v.Discriminant }}\non go {{ n = ident(ease)\n b = ease is EasingFunction.Sine }}"
    ));
    assert!(!has_unsupported(&r.module));
    // A chip takes its tag as a value, or the var behind a `*` parameter.
    let r = compile_ok(&format!(
        "{EASE}chip C(g: exec, x: int) {{ on g {{ n = x }} }}\n\
         chip D(g: exec, v: *EasingFunction) {{ on g {{ v = EasingFunction.Sine }} }}\n\
         C(go, ease.Discriminant)\nD(go, e)"
    ));
    assert!(!has_unsupported(&r.module));
    let input = r.module.inputs[0];
    assert!(
        r.module
            .wires
            .iter()
            .any(|w| w.source.node_id == input && w.source.port == WirePort::RerOutput)
    );
    // Interpolating one is a type error, reported once and without internals.
    let r = compile(&format!(
        "{EASE}var s: string = \"\"\non go {{ s = \"${{ease}}\" }}"
    ));
    assert!(has_error(&r, "WS003"), "{}", messages(&r));
    assert!(!has_error(&r, "WS071"), "{}", messages(&r));
}

#[test]
fn no_message_names_an_enums_internal_slots() {
    for src in [
        "enum E { A, B(int) }\nvar v: E = E.A\nvar n: int = 0\nin go: exec\non go { n = v + 1 }",
        "enum U { X, Y }\nvar us: U[]\nin go: exec\nchip K(g: exec, cs: U[]) { }\non go { K(go, us) }",
        "enum E { A, B(int) }\nvar v: E = E.A\nin go: exec\non go { PrintToConsole(v) }",
    ] {
        let r = compile(src);
        assert!(!messages(&r).contains("__"), "{}", messages(&r));
    }
    // A chip argument typecheck already rejected reports once.
    let r = compile(
        "enum U { X, Y }\nvar us: U[]\nin go: exec\nchip K(g: exec, cs: U[]) { }\non go { K(go, us) }",
    );
    assert!(has_error(&r, "WS049"), "{}", messages(&r));
    assert!(!has_error(&r, "WS071"), "{}", messages(&r));
}

#[test]
fn a_record_is_an_enum_only_when_its_type_is() {
    // A user record whose one field is spelled like an enum's tag is still a
    // record: one value is expected, and it has several wires (or its shape).
    let r = compile(
        "type Weird = { __disc: int }\nvar w: Weird = { __disc: 5 }\nvar n: int = 0\nin go: exec\non go { n = w }",
    );
    assert!(has_error(&r, "WS003"), "{}", messages(&r));
    let r = compile(
        "type Weird2 = { __disc: int, other: int }\nvar w: Weird2 = { __disc: 5, other: 9 }\nvar n: int = 0\nin go: exec\non go { n = w }",
    );
    let m = messages(&r);
    assert!(has_error(&r, "WS003"), "{m}");
    assert!(!m.contains("Discriminant"), "a record is not an enum: {m}");
    assert!(!m.contains("w.__"), "the hint names a readable field: {m}");
    assert!(
        m.contains("`w.other`"),
        "the hint names a readable field: {m}"
    );
}

/// A call with a value output and an exec output, used as one enum value, is
/// its value output: typecheck coerces it that way, and the `int` twin reads
/// it that way.
#[test]
fn a_call_with_a_value_and_an_exec_output_used_as_one_enum_is_its_value() {
    use super::generics::{assert_clean, both_folds, read_label, stored_reads};
    let decl = "var d: Hue = Hue.Red\nvar k: int = 3\nvar n: int = 0\n\
                mod f(x: Hue) -> (r: Hue, e: exec) { emit r = x\n emit e }\n\
                mod fi(x: int) -> (r: int, e: exec) { emit r = x\n emit e }\n";
    for (extra, stmt, target, source) in [
        ("", "d = f(c)", "d", "c"),
        ("", "n = fi(k)", "n", "k"),
        (
            "mod g(x: Hue) -> Hue { return f(x) }\n",
            "d = g(c)",
            "d",
            "c",
        ),
        (
            "mod g(x: Hue) -> (r: Hue) { emit r = f(x) }\n",
            "d = g(c)",
            "d",
            "c",
        ),
        (
            "mod g(x: Hue) -> (r: Hue) { out r = f(x) }\n",
            "d = g(c)",
            "d",
            "c",
        ),
    ] {
        let src = format!("{HUE}{decl}{extra}on go {{ {stmt} }}\n");
        for (mode, r) in both_folds(&src) {
            let what = format!("`{extra}{stmt}`, {mode}");
            assert_clean(&r, &what);
            assert_eq!(
                stored_reads(&r, target),
                vec![Some(source.to_string())],
                "{what}"
            );
        }
    }
    let src = format!("{HUE}{decl}out oc: Hue\non go {{ emit oc = f(c) }}\n");
    let (r, (node, _)) = enum_out_driver(&src);
    assert_eq!(
        read_label(&r, node),
        Some("c".to_string()),
        "`oc` is driven by c"
    );
    // Annotated `let`, and every write into a container of the enum.
    let src = format!("{HUE}{decl}on go {{ let q: Hue = f(c)\n d = q }}\n");
    for (mode, r) in both_folds(&src) {
        assert_clean(&r, mode);
        assert_eq!(
            stored_reads(&r, "d"),
            vec![Some("c".to_string())],
            "let, {mode}"
        );
    }
    for (stmt, class) in [
        ("hs.push(f(c))", gc::ARRAY_PUSH),
        ("hs.insert(0, f(c))", gc::ARRAY_INSERT),
        ("hs[0] = f(c)", gc::ARRAY_SET_AT_INDEX),
        ("hs = [f(c)]", gc::ARRAY_PUSH),
        ("m[1] = f(c)", gc::MAP_SET),
        ("m.set(1, f(c))", gc::MAP_SET),
    ] {
        let src = format!("{HUE}{decl}var hs: Hue[]\nvar m: Map<int, Hue>\non go {{ {stmt} }}\n");
        for (mode, r) in both_folds(&src) {
            let what = format!("`{stmt}`, {mode}");
            assert_clean(&r, &what);
            let write = r
                .module
                .nodes
                .values()
                .find(|n| n.gate_class == class)
                .unwrap_or_else(|| panic!("{what}: the write"));
            let (value, _) = source_port_of(&r, write.id, WirePort::Value)
                .unwrap_or_else(|| panic!("{what}: its value is wired"));
            assert_eq!(read_label(&r, value), Some("c".to_string()), "{what}");
        }
    }
}

/// A mod's one record output named like its record's only field is still
/// the record: `-> (inner: Q)` with `type Q = { inner: P }` hands the caller
/// a `Q`, never the `P` inside it.
#[test]
fn a_record_output_named_like_its_records_field_is_the_record() {
    use super::generics::{assert_clean, both_folds, stored_reads};
    let prelude = "type P = { x: int, y: int }\ntype Q = { inner: P }\nvar p: P = { x: 1, y: 2 }\n\
                   var q: Q = { inner: { x: 0, y: 0 } }\nvar n: int = 0\nin go: exec\n";
    for decl in [
        "mod f(a: P) -> (inner: Q) { return { inner: a } }",
        "mod f(a: P) -> (inner: Q) { emit inner = { inner: a } }",
        "mod f(a: P) -> (inner: Q) { out inner = { inner: a } }",
        "mod f(a: P) -> (o: Q) { return { inner: a } }",
    ] {
        for stmt in ["q = f(p)", "let t = f(p)\n q = t"] {
            let src = format!("{prelude}{decl}\non go {{ {stmt} }}\n");
            for (mode, r) in both_folds(&src) {
                let what = format!("`{decl}`, `{stmt}`, {mode}");
                assert_clean(&r, &what);
                assert_eq!(
                    stored_reads(&r, "q.inner.x"),
                    vec![Some("p.x".to_string())],
                    "{what}"
                );
                assert_eq!(
                    stored_reads(&r, "q.inner.y"),
                    vec![Some("p.y".to_string())],
                    "{what}"
                );
            }
        }
        let src = format!(
            "{prelude}{decl}\nmod g(z: Q) -> int {{ return z.inner.y }}\non go {{ n = g(f(p)) }}\n"
        );
        for (mode, r) in both_folds(&src) {
            assert_clean(&r, mode);
            assert_eq!(
                stored_reads(&r, "n"),
                vec![Some("p.y".to_string())],
                "`{decl}`, {mode}"
            );
        }
    }
}

/// A compile that reports an error drops the internal error beside it:
/// a program typecheck rejects is not a compiler bug.
#[test]
fn no_internal_error_follows_a_reported_one() {
    let src = format!(
        "{HUE}var k: int = 3\nvar n: int = 0\nmod f<T>(x: T) -> T {{ return x }}\n\
         mod g<U: int | Hue>(y: U) -> U {{ return f(y) }}\non go {{ n = g(k)\n let e = g(c) }}\n"
    );
    let r = compile(&src);
    assert!(has_error(&r, "WS033"), "{}", messages(&r));
    assert!(!has_error(&r, "WS094"), "{}", messages(&r));
}

/// Every wire as `class.port -> class.port`, sorted, with the gate classes.
fn wire_shape(r: &LowerResult) -> Vec<String> {
    let class = |id: &NodeId| r.module.nodes.get(id).map_or("?", |n| n.gate_class);
    let mut shape: Vec<String> = r
        .module
        .wires
        .iter()
        .map(|w| {
            format!(
                "{}.{:?} -> {}.{:?}",
                class(&w.source.node_id),
                w.source.port,
                class(&w.target.node_id),
                w.target.port
            )
        })
        .chain(r.module.nodes.values().map(|n| n.gate_class.to_string()))
        .collect();
    shape.sort();
    shape
}

/// An enum element passed straight to a call binds exactly as the same
/// element bound by a `let` first does: a mod or generic mod parameter, a
/// builtin argument, a container method's value, at a constant or runtime
/// index, and a map value. It was an `_Unsupported` placeholder.
#[test]
fn an_enum_element_passed_straight_to_a_call_binds_as_through_a_let() {
    let prelude = "enum Hue { Red, Green, Blue }\nvar hs: Hue[]\nvar m: Map<int, Hue>\nvar n: int = 0\n\
                   var k: int = 0\nvar t: Hue = Hue.Red\nin go: exec\n\
                   mod disc(h: Hue) -> int { return h.Discriminant }\n\
                   mod same<T: Hue | int>(h: T) -> T { return h }\n";
    for (direct, via_let) in [
        ("n = disc(hs[0])", "let x = hs[0]\n n = disc(x)"),
        ("n = disc(hs[k])", "let x = hs[k]\n n = disc(x)"),
        ("n = disc(m[k])", "let x = m[k]\n n = disc(x)"),
        (
            "n = disc(m.get(k).Value)",
            "let x = m.get(k).Value\n n = disc(x)",
        ),
        ("t = same(hs[k])", "let x = hs[k]\n t = same(x)"),
        (
            "t = same(hs[0])\n k = same(k)",
            "let x = hs[0]\n t = same(x)\n k = same(k)",
        ),
        (
            "SendCustomEvent(\"c\", hs[k])",
            "let x = hs[k]\n SendCustomEvent(\"c\", x)",
        ),
        ("hs.push(hs[k])", "let x = hs[k]\n hs.push(x)"),
        (
            "n = if hs[k] is Hue.Green then 1 else 0",
            "let x = hs[k]\n n = if x is Hue.Green then 1 else 0",
        ),
    ] {
        let program = |body: &str| {
            format!("{prelude}on go {{\n hs.push(Hue.Green)\n m[1] = Hue.Blue\n {body}\n}}\n")
        };
        assert_binds_as_through_a_let(&program(direct), &program(via_let), direct);
    }
    // An element with several slots: an enum with a payload, and a record
    // passed to a generic parameter.
    let prelude = "enum S { A(int), B }\ntype P = { x: int, y: int }\nvar ss: S[]\nvar ps: P[]\n\
                   var n: int = 0\nvar k: int = 0\nin go: exec\n\
                   mod val(s: S) -> int { return match s { A(v) => v, B => 0 } }\n\
                   mod same<T: P | int>(p: T) -> T { return p }\n";
    for (direct, via_let) in [
        ("n = val(ss[k])", "let x = ss[k]\n n = val(x)"),
        ("n = same(ps[k]).y", "let x = ps[k]\n n = same(x).y"),
    ] {
        let program = |body: &str| {
            format!(
                "{prelude}on go {{\n ss.push(S.A(5))\n ps.push({{ x: 1, y: 2 }})\n {body}\n}}\n"
            )
        };
        assert_binds_as_through_a_let(&program(direct), &program(via_let), direct);
    }
}

/// `a` and `b` compile to the same gates and wires, with the same
/// diagnostics, in both fold modes, and `a` has no placeholder.
fn assert_binds_as_through_a_let(a: &str, b: &str, direct: &str) {
    use super::generics::both_folds;
    for ((mode, r), (_, l)) in both_folds(a).into_iter().zip(both_folds(b)) {
        let what = format!("`{direct}`, {mode}");
        assert!(!has_unsupported(&r.module), "{what}: {}", messages(&r));
        let codes = |r: &LowerResult| {
            r.diagnostics
                .iter()
                .map(|d| d.code.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(codes(&r), codes(&l), "{what}");
        assert!(!codes(&r).iter().any(|c| c == "WSP001"), "{what}");
        assert_eq!(wire_shape(&r), wire_shape(&l), "{what}");
    }
}

/// The fix `WS013` offers for an enum output's continuous `out` is one of
/// its variants, never `0`.
#[test]
fn a_continuous_enum_out_beside_an_emit_suggests_a_variant() {
    let r = compile(&format!(
        "{HUE}out oc: Hue = c\non go {{ emit oc = Hue.Blue }}\n"
    ));
    let msg = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS013")
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(msg.contains("`out oc = Hue.Red`"), "{}", messages(&r));
    assert!(!msg.contains("= 0"), "{msg}");
}

/// A record used as one value names one of its fields to read instead, in a
/// sentence that reads.
#[test]
fn a_record_used_as_one_value_suggests_a_field() {
    let r = compile(
        "var t: string = \"\"\nin go: exec\non go { let p = { x: 1, y: 2 }\n t = \"p=\" .. p }\n",
    );
    let msg = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS003")
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("so read one of its fields (`p.x`)"),
        "{}",
        messages(&r)
    );
    // The field named is the first one the type declares.
    let r = compile(
        "var s: string = \"\"\nin go: exec\ntype P = { y: int, x: int }\nvar p: P = { y: 1, x: 2 }\non go { s = \"${p}\" }\n",
    );
    let msg = r
        .diagnostics
        .iter()
        .find(|d| d.code == "WS003")
        .map(|d| d.message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("so read one of its fields (`p.y`)"),
        "{}",
        messages(&r)
    );
}

/// A shorthand field of an int var is an int, as the named field is, so an
/// `if` or `match` choosing between a shorthand literal and a named one joins,
/// and reads the chosen values.
#[test]
fn a_shorthand_and_a_named_literal_join_as_values() {
    let r = compile(
        "enum E { A, B }\nvar e: E = E.A\nin c: bool\nvar x: int = 1\nvar y: int = 2\n\
         var n: int = 0\nvar m: int = 0\nin go: exec\n\
         on go {\n  let q = if c then { x, y } else { x: 3, y: 4 }\n  n = q.x + q.y\n\
           let s = match e { A => { x, y }, B => { x: 3, y: 4 } }\n  m = s.x + s.y\n}",
    );
    crate::test_support::assert_no_diags(&r.diagnostics);
    assert!(!has_unsupported(&r.module));
    let selects = gate_count(&r, gc::SELECT);
    assert_eq!(selects, 4, "one select per field of each join");
}

/// An exec output or parameter declared through a `type` alias of `exec` is
/// an exec one: a mod's, a chip's and a top-level `out`, alone or beside a
/// value output, compile exactly as when written `exec`.
#[test]
fn an_exec_declared_through_an_alias_is_exec() {
    for program in [
        "var n: int = 0\nin go: exec\nmod f() -> (d: $S) { emit d }\non go { f()\n n = 1 }\n",
        "var n: int = 0\nin go: exec\nout o: $S\nmod f() -> (d: $S) { n = 2\n emit d }\non go { f()\n emit o }\n",
        "var n: int = 0\nin go: exec\nmod f(x: int) -> (r: int, d: $S) { emit r = x\n emit d }\non go { n = f(3)\n n = 4 }\n",
        "var n: int = 0\nin go: exec\nchip f() -> (d: $S) { emit d }\non go { f()\n n = 1 }\n",
        "var n: int = 0\nin go: exec\nchip f(g: $S) { on g { n = 5 } }\non go { f(go) }\n",
    ] {
        let aliased = format!("type Sig = exec\n{}", program.replace("$S", "Sig"));
        let plain = program.replace("$S", "exec");
        for ((mode, a), (_, p)) in super::generics::both_folds(&aliased)
            .into_iter()
            .zip(super::generics::both_folds(&plain))
        {
            assert!(
                a.diagnostics.is_empty(),
                "{aliased} ({mode}): {}",
                messages(&a)
            );
            assert_eq!(
                super::generic_differential::shape(&a),
                super::generic_differential::shape(&p),
                "{aliased} ({mode})"
            );
        }
    }
}

/// `f(c).r`, naming a mod's one enum output, is that output: pushed, bound,
/// assigned or passed on exactly as `f(c)` is, for a plain mod and for the
/// enum and int instances of a generic one.
#[test]
fn a_calls_one_enum_output_read_by_its_name_is_the_calls_value() {
    let program = "enum Hue { Red, Green, Blue }\nvar c: Hue = Hue.Blue\nvar d: Hue = Hue.Red\nvar n: int = 0\n\
                   var hs: Hue[]\nin go: exec\nmod f(e: Hue) -> (r: Hue) { emit r = e }\n\
                   mod g<T: int | Hue>(e: T) -> (r: T) { emit r = e }\nmod h(x: Hue) -> int { return x.Discriminant }\n\
                   on go { hs.push(f(c)$R)\n let q = f(c)$R\n d = q\n n = q.Discriminant\n d = f(c)$R\n\
                    d = g(c)$R\n n = g(n)$R\n n = h(f(c)$R) }\n";
    let named = program.replace("$R", ".r");
    let direct = program.replace("$R", "");
    for ((mode, a), (_, b)) in super::generics::both_folds(&named)
        .into_iter()
        .zip(super::generics::both_folds(&direct))
    {
        assert!(a.diagnostics.is_empty(), "{mode}: {}", messages(&a));
        assert!(b.diagnostics.is_empty(), "{mode}: {}", messages(&b));
        assert_eq!(
            super::generic_differential::shape(&a),
            super::generic_differential::shape(&b),
            "{mode}"
        );
    }
}

/// `.Discriminant` and `is` read off a call's enum result, off its one enum
/// output by name, or off an `if` choosing an enum, compile as they do off
/// the value bound to a `let` first: the tag, never a placeholder.
#[test]
fn a_calls_enum_result_reads_its_tag_as_a_bound_one_does() {
    let head = "enum Hue { Red, Green, Blue }\nvar c: Hue = Hue.Blue\nvar d: Hue = Hue.Red\nvar n: int = 0\n\
                var flag: bool = true\nin go: exec\nmod f(e: Hue) -> Hue { return e }\n\
                mod g(e: Hue) -> (r: Hue) { emit r = e }\n";
    for (direct, bound) in [
        ("n = f(c).Discriminant", "let x = f(c)\n n = x.Discriminant"),
        (
            "n = if f(c) is Hue.Blue then 1 else 2",
            "let x = f(c)\n n = if x is Hue.Blue then 1 else 2",
        ),
        (
            "if f(c) is Hue.Red { n = 1 }",
            "let x = f(c)\n if x is Hue.Red { n = 1 }",
        ),
        (
            "n = g(c).r.Discriminant",
            "let x = g(c).r\n n = x.Discriminant",
        ),
        (
            "d = if g(c).r is Hue.Blue then Hue.Green else c",
            "let x = g(c).r\n d = if x is Hue.Blue then Hue.Green else c",
        ),
        (
            "n = (if flag then c else d).Discriminant",
            "let x = if flag then c else d\n n = x.Discriminant",
        ),
    ] {
        let program = |body: &str| format!("{head}on go {{ {body} }}\n");
        for ((mode, a), (_, b)) in super::generics::both_folds(&program(direct))
            .into_iter()
            .zip(super::generics::both_folds(&program(bound)))
        {
            assert!(
                a.diagnostics.is_empty(),
                "`{direct}`, {mode}: {}",
                messages(&a)
            );
            assert!(!has_unsupported(&a.module), "`{direct}`, {mode}");
            assert_eq!(
                super::generic_differential::shape(&a),
                super::generic_differential::shape(&b),
                "`{direct}`, {mode}"
            );
        }
    }
    // A payload enum's tag. The bound form also reads the payload slots, which
    // nothing uses, so only the reads differ.
    let payload = "enum S { A(int), B(float), C }\nvar s: S = S.A(4)\nvar n: int = 0\nin go: exec\n\
                   mod p(e: S) -> S { return e }\n";
    for body in ["n = p(s).Discriminant", "if p(s) is S.B { n = 9 }"] {
        for (mode, r) in super::generics::both_folds(&format!("{payload}on go {{ {body} }}\n")) {
            assert!(
                r.diagnostics.is_empty(),
                "`{body}`, {mode}: {}",
                messages(&r)
            );
            assert!(!has_unsupported(&r.module), "`{body}`, {mode}");
        }
    }
}
