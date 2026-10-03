//! Generics past the type-parameter combination cap, checked against a
//! hand-monomorphized copy. Typecheck checks only some combinations of a
//! generic's type params once their product passes the cap, so the
//! instantiation a call asks for is checked because the call demands it, and
//! lowering types its values from that check. Each program here compiles to
//! the same gates as the copy with the type params written out: the same gate
//! classes, the same port types (every var's variant), the same baked
//! literals, and the same wires.

use super::generics::{both_folds, emit_ok};
use super::*;
use crate::ir::{Module, Type};

const S6: &str = "int | float | string | bool | vector | color";

const PRELUDE: &str = "enum Hue { Red, Green, Blue }\ntype Box<T> = { v: T }\nvar k: int = 3\nvar k2: int = 4\n\
var fl: float = 1.5\nvar fo: float = 0.0\nvar n: int = 0\nvar t0: int = 0\nvar c: Hue = Hue.Green\n\
var d: Hue = Hue.Red
var flag: bool = true
var st: string = \"s\"
var so: string = \"\"
var hs: Hue[]
in go: exec
\nmod two(x: float) -> (lo: float, hi: float) { emit lo = x
 emit hi = x * 2.0 }
";

/// Every gate as its class, port types and properties (labels aside), and
/// every wire as `class.port -> class.port`, across the chip tree, sorted.
pub(super) fn shape(r: &LowerResult) -> Vec<String> {
    fn walk(m: &Module, out: &mut Vec<String>) {
        let class = |id: &crate::ir::NodeId| m.nodes.get(id).map_or("pin", |n| n.gate_class);
        for n in m.nodes.values() {
            let mut ports: Vec<String> = n
                .ports
                .inputs
                .iter()
                .chain(n.ports.outputs.iter())
                .map(|p| format!("{}:{:?}", crate::intern::resolve(p.name), p.ty))
                .collect();
            ports.sort();
            let mut props: Vec<String> = n
                .properties
                .iter()
                .map(|(k, v)| (crate::intern::resolve(*k), v))
                .filter(|(k, _)| !k.contains("Label") && !k.contains("Doc"))
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect();
            props.sort();
            out.push(format!("{} {} {}", n.gate_class, ports.join(" "), props.join(" ")));
        }
        for w in &m.wires {
            out.push(format!(
                "{}.{:?} -> {}.{:?}",
                class(&w.source.node_id),
                w.source.port,
                class(&w.target.node_id),
                w.target.port
            ));
        }
        for c in m.chips.values() {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(&r.module, &mut out);
    out.sort();
    out
}

/// Assert `generic` and `mono` compile, in both fold modes, to the same shape
/// with the same diagnostics, none of them an error.
fn assert_same_as_monomorph(generic: &str, mono: &str, what: &str) {
    let codes = |r: &LowerResult| r.diagnostics.iter().map(|d| d.code.clone()).collect::<Vec<_>>();
    for ((mode, g), (_, m)) in both_folds(generic).into_iter().zip(both_folds(mono)) {
        let what = format!("{what}, {mode}");
        for (r, which) in [(&g, "generic"), (&m, "monomorphized copy")] {
            assert!(
                r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
                "{what} ({which}): {:?}",
                r.diagnostics
            );
            assert!(!has_gate(r, "_Unsupported"), "{what} ({which})");
            emit_ok(r).unwrap_or_else(|e| panic!("{what} ({which}): {e}"));
        }
        assert_eq!(codes(&g), codes(&m), "{what}: diagnostics");
        let (gs, ms) = (shape(&g), shape(&m));
        let only_generic: Vec<&String> = gs.iter().filter(|s| !ms.contains(s)).collect();
        let only_mono: Vec<&String> = ms.iter().filter(|s| !gs.contains(s)).collect();
        assert_eq!(gs, ms, "{what}\nonly in the generic: {only_generic:#?}\nonly in the copy: {only_mono:#?}");
    }
}

/// Whether typecheck checked a body under the substitution `key` only because
/// a call demanded it: the combinations of the bounds leave it out.
fn demanded(src: &str, key: &[Type]) -> bool {
    let parsed = crate::parser::parse(src, "test");
    let tc = typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    tc.demanded_instantiations.iter().any(|k| *k.3 == *key)
}

fn scalar(t: &str) -> Type {
    match t {
        "int" => Type::Int,
        "float" => Type::Float,
        other => panic!("{other}"),
    }
}

/// A generic mod over `A`, `B` (bounded by `bounds`), two six-type params and
/// `H: Hue | int`, run with `body`, called at the enum instance and at the
/// int instance of `H`; its copy writes `A` and `B` out as `a_ty`, `b_ty`.
/// `body` names `A`/`B` only as `A[]`, `<A>` and `<B>`. Whether either
/// instantiation is past the cap: checked only because its call demanded it.
fn mod_case(bounds: (&str, &str), body: &str, result: &str, args: &str, (a_ty, b_ty): (&str, &str)) -> bool {
    let (a_bound, b_bound) = bounds;
    let generic = format!(
        "{PRELUDE}mod f<A: {a_bound}, B: {b_bound}, C: {S6}, D: {S6}, H: Hue | int>(a: A, b: B, cc: C, dd: D, h: H) \
         -> (r: float, e: H) {{ {body}\n emit e = h\n emit r = {result} }}\n\
         on go {{ let x = f({args}, c)\n fo = x.r\n d = x.e\n let y = f({args}, k)\n n = y.e + 0 }}\n"
    );
    let mono_body = body
        .replace("A[]", &format!("{a_ty}[]"))
        .replace("<A>", &format!("<{a_ty}>"))
        .replace("<B>", &format!("<{b_ty}>"));
    let one = |name: &str, h: &str| {
        format!(
            "mod {name}(a: {a_ty}, b: {b_ty}, cc: int, dd: int, h: {h}) -> (r: float, e: {h}) \
             {{ {mono_body}\n emit e = h\n emit r = {result} }}\n"
        )
    };
    let mono = format!(
        "{PRELUDE}{}{}on go {{ let x = f_hue({args}, c)\n fo = x.r\n d = x.e\n let y = f_int({args}, k)\n n = y.e + 0 }}\n",
        one("f_hue", "Hue"),
        one("f_int", "int")
    );
    let what = format!("`{body}` at {a_ty}, {b_ty}");
    let hue = Type::Enum { name: "Hue".into(), args: vec![] };
    let int_key = [scalar(a_ty), scalar(b_ty), Type::Int, Type::Int, Type::Int];
    let hue_key = [scalar(a_ty), scalar(b_ty), Type::Int, Type::Int, hue];
    let past_cap = demanded(&generic, &int_key) || demanded(&generic, &hue_key);
    assert_same_as_monomorph(&generic, &mono, &what);
    past_cap
}

const INSTANCES: [(&str, (&str, &str)); 3] =
    [("k, fl, k, k", ("int", "float")), ("k, k2, k, k", ("int", "int")), ("fl, k, k, k", ("float", "int"))];

const MIXED: (&str, &str) = ("float | int", "int | float");

/// [`mod_case`] at every instance; whether any of them reaches the cap.
fn past_cap(bounds: (&str, &str), body: &str, result: &str, instances: &[(&str, (&str, &str))]) -> bool {
    instances
        .iter()
        .map(|(args, types)| mod_case(bounds, body, result, args, *types))
        .fold(false, |any, past| any || past)
}

/// `a + b` over an `int` and a `float` is a `float`, though every combination
/// typecheck checked had the type of `a`.
#[test]
fn a_value_depending_on_two_type_params_past_the_cap_takes_both() {
    for (body, result) in [
        ("var t = a + b", "t"),
        ("var t = a + b\n var u = t * a\n var w = u + 1", "w"),
        ("var u = -a + b", "u"),
        ("var cc2 = a < b\n var u = if cc2 then a else b", "u"),
        ("var u = a * a + b * b\n var m = min(a, b)", "u + m"),
    ] {
        assert!(past_cap(MIXED, body, result, &INSTANCES), "`{body}` never reaches the cap");
    }
}

/// Both params `float | int` and both arguments `int`: every combination
/// typecheck checked typed `a + b` as `float`, and this one is `int`, so the
/// division after it is an integer one (`7 / 2` is 3).
#[test]
fn a_type_every_checked_combination_agreed_on_is_not_assumed_past_the_cap() {
    for (body, result) in [("var t = a + b\n var u = t / 2", "u"), ("var t = a / b", "t")] {
        let instances = [("k, k2, k, k", ("int", "int")), ("k, fl, k, k", ("int", "float"))];
        assert!(past_cap(("float | int", "float | int"), body, result, &instances), "`{body}` never reaches the cap");
    }
}

/// An `if` joining an `A` and a `B` is their widening past the cap.
#[test]
fn an_if_join_past_the_cap_takes_both_branches() {
    assert!(past_cap(MIXED, "var t = if flag then a else b", "t", &INSTANCES));
}

/// A chain of `let`s, a `let` named like a top-level var, a tuple, an array
/// of `A`, a record of `A`/`B`, and a body of constants, past the cap.
#[test]
fn lets_containers_records_and_constants_past_the_cap_keep_their_types() {
    for (body, result) in [
        ("let t = a * b\n var u = t", "u"),
        ("let t = a * b\n let w = t + 1\n var u = w", "u"),
        ("let t0 = a * b\n var u = t0", "u"),
        ("let (p, q) = (a, b)\n var u = p + q", "u"),
        ("var xs: A[]\n xs.push(a)\n var u = xs[0] + b", "u"),
        ("let bx: Box<A> = { v: a }\n var u = bx.v + b", "u"),
        ("var bx: Box<B> = { v: b }\n var u = bx.v + a", "u"),
        ("var t = 2 * 3\n var s = 1.5 + 1.0\n var q = \"x\"", "t + s"),
        ("var u = abs(a) + max(a, b)\n var q = sqrt(b)", "u + q"),
    ] {
        assert!(past_cap(MIXED, body, result, &INSTANCES), "`{body}` never reaches the cap");
    }
}

/// A `match` past the cap on an enum declared outside the generic, directly
/// or through a `let` of the body, chooses between its arms instead of
/// storing the first.
#[test]
fn a_match_past_the_cap_on_an_outer_enum_chooses_its_arm() {
    for body in [
        "var u = match c { Hue.Red => a, _ => b }",
        "let e2 = c\n var u = match e2 { Hue.Red => a, Hue.Green => b, _ => a * b }",
    ] {
        assert!(past_cap(MIXED, body, "u", &INSTANCES), "`{body}` never reaches the cap");
    }
}

/// Calls, container methods, refs and enum values past the cap: each takes
/// the type the call, method, ref or variant gives, not `any`.
#[test]
fn calls_methods_refs_and_enum_values_past_the_cap_keep_their_types() {
    for (body, result) in [
        ("let (l, h2) = two(b)\n var u = l + h2 + a", "u"),
        ("var xs: A[]\n xs.push(a)\n var l = xs.length()\n var u = l + b", "u"),
        ("var u = Select(flag, a, a) + b", "u"),
        ("var u = a + b\n var w = &u\n var z = *w + a", "z"),
        ("hs.push(c)\n var e3 = hs[0]\n d = e3\n var u = a * b", "u"),
        ("var e2 = Hue.Blue\n d = e2\n var i = c.Discriminant\n var u = a + b + i", "u"),
        ("var t = c is Hue.Red\n var u = if t then a else b", "u"),
    ] {
        assert!(past_cap(MIXED, body, result, &INSTANCES), "`{body}` never reaches the cap");
    }
}

/// The same past the cap in a generic `chip`. An enum cannot cross a chip's
/// port (`WS049`), so its other instance of `H` is a `string`.
#[test]
fn a_generic_chip_past_the_cap_matches_its_monomorphized_copy() {
    for (body, result) in [
        ("var t = a + b", "t"),
        ("var t = if flag then a else b", "t"),
        ("let t = a * b\n var u = t", "u"),
        ("var t = 2 * 3\n var s = 1.5", "t + s"),
    ] {
        let mut past_cap = false;
        for (args, (a_ty, b_ty)) in INSTANCES {
            let generic = format!(
                "{PRELUDE}chip f<A: float | int, B: int | float, C: {S6}, D: {S6}, H: int | string>(a: A, b: B, cc: C, dd: D, h: H) \
                 -> (r: float, e: H) {{ {body}\n out e = h\n out r = {result} }}\n\
                 let x = f({args}, k)\nlet y = f({args}, st)\non go {{ fo = x.r\n n = x.e\n so = y.e }}\n"
            );
            let one = |name: &str, h: &str| {
                format!(
                    "chip {name}(a: {a_ty}, b: {b_ty}, cc: int, dd: int, h: {h}) -> (r: float, e: {h}) \
                     {{ {body}\n out e = h\n out r = {result} }}\n"
                )
            };
            let mono = format!(
                "{PRELUDE}{}{}let x = f_int({args}, k)\nlet y = f_str({args}, st)\non go {{ fo = x.r\n n = x.e\n so = y.e }}\n",
                one("f_int", "int"),
                one("f_str", "string")
            );
            past_cap |= demanded(&generic, &[scalar(a_ty), scalar(b_ty), Type::Int, Type::Int, Type::Int]);
            assert_same_as_monomorph(&generic, &mono, &format!("chip `{body}` at {a_ty}, {b_ty}"));
        }
        assert!(past_cap, "chip `{body}` never reaches the cap");
    }
}

/// A mod declared without type params, called from a generic body with an
/// `int` for its `float` parameter, divides as the type checker resolved it:
/// as floats, exactly as when it is called from a plain body. So does the
/// generic body on that mod's `float` outputs, which carry the `int` port.
#[test]
fn a_plain_mod_called_from_a_generic_body_keeps_its_own_operators() {
    let callee = "mod g(x: float, y: float) -> float { var t = x / 2\n var u = if flag then x else y\n var w = -x\n return t + u + w }\n";
    for body in ["emit r = g(a, k2)", "let (l, h2) = two(a)\n emit r = l / 2 + h2"] {
        let generic = format!(
            "{PRELUDE}{callee}mod f<A: float | int, H: Hue | int>(a: A, h: H) -> (r: float, e: H) {{ emit e = h\n {body} }}\n\
             on go {{ let x = f(k, c)\n fo = x.r\n d = x.e\n let y = f(k, k)\n n = y.e + 0 }}\n"
        );
        let mono = format!(
            "{PRELUDE}{callee}mod f_hue(a: int, h: Hue) -> (r: float, e: Hue) {{ emit e = h\n {body} }}\n\
             mod f_int(a: int, h: int) -> (r: float, e: int) {{ emit e = h\n {body} }}\n\
             on go {{ let x = f_hue(k, c)\n fo = x.r\n d = x.e\n let y = f_int(k, k)\n n = y.e + 0 }}\n"
        );
        assert_same_as_monomorph(&generic, &mono, &format!("`{body}` under a generic caller"));
    }
}

const WIDE_PRELUDE: &str = "enum Hue { Red, Green, Blue }\nenum S { Sa(int), Sb(float), Sc }\n\
type PH = { h: Hue, q: int }\nvar k: int = 3\nvar k2: int = 4\nvar fl: float = 1.5\nvar n: int = 0\n\
var fo: float = 0.0\nvar so: string = \"\"\nvar st: string = \"ab,c\"\nvar vv: vector = Vec(1.0, 2.0, 3.0)\n\
var vo: vector = Vec(0.0, 0.0, 0.0)\nvar col: color = Color(1.0, 0.5, 0.25, 1.0)\n\
var qv: quat\nvar qo: quat\nvar bo: bool = true\n\
var c: Hue = Hue.Green\nvar d: Hue = Hue.Red\nvar s: S = S.Sa(4)\nvar hs: Hue[]\nvar ss: S[]\n\
var mh: Map<int, Hue>\nvar ph: PH = { h: Hue.Red, q: 1 }\nin go: exec\n\
mod id<T: int | float>(x: T) -> T { return x }\n\
mod g2<T: int | float, U: int | float | string>(x: T, y: U) -> T { return id<T>(x) }\n\
mod g3<T: int | float, U: int | float | string>(x: T, y: U) -> T { return g2(x, y) }\n";

/// `body` in a generic whose combinations are capped, called at `f<int, int,
/// int, int, int>`, which the sampled combinations leave out, and in a copy
/// with those types written out (`A`..`D` and `H` in `<..>` and `: ..`
/// positions). Asserts the instantiation is checked because the call demands
/// it, and that both compile to the same gates in both fold modes.
fn body_case(body: &str) {
    let generic = format!(
        "{WIDE_PRELUDE}mod f<A: float | int, B: int | float, C: {S6}, D: {S6}, H: Hue | int>(a: A, b: B, cc: C, dd: D, h: H) \
         {{ {body} }}\non go {{ f(k, k2, k, k2, k) }}\n"
    );
    let mut mono_body = body.to_string();
    for p in ["A", "B", "C", "D", "H"] {
        mono_body = mono_body
            .replace(&format!("<{p}>"), "<int>")
            .replace(&format!("<{p}, "), "<int, ")
            .replace(&format!(", {p}>"), ", int>")
            .replace(&format!(": {p}\n"), ": int\n")
            .replace(&format!(": {p} "), ": int ");
    }
    let mono = format!(
        "{WIDE_PRELUDE}mod f(a: int, b: int, cc: int, dd: int, h: int) {{ {mono_body} }}\non go {{ f(k, k2, k, k2, k) }}\n"
    );
    assert!(demanded(&generic, &vec![Type::Int; 5]), "`{body}` does not reach the cap");
    assert_same_as_monomorph(&generic, &mono, &format!("`{body}`"));
}

/// Builtin methods on string, vector, color and quat receivers, and a string
/// index, past the cap: each value keeps the type the method gives it.
#[test]
fn receiver_methods_past_the_cap_keep_their_types() {
    for body in [
        "var v = st.ToUpper()\n so = v",
        "var v = st.Length()\n var u = v + a\n n = u",
        "var v = st.Contains(\"b\")\n var u = if v then a else b\n n = u",
        "var v = st.Trim()\n n = v.Length()",
        "var v = st.ToUpper()\n if v == \"AB,C\" { n = 1 }",
        "var v = st.Length() > a\n bo = v",
        "var v = st .. st.ToUpper()\n so = v",
        "var v = st[0]\n so = v",
        "var v = st[a]\n so = v",
        "var v = vv.Normalize()\n vo = v * 2.0",
        "var v = vv.Cross(vv)\n vo = v",
        "var v = vv.ToRotation()\n qo = v",
        "var v = qv.Invert()\n qo = v",
        "var v = qv.Slerp(qv, 0.5)\n qo = v",
        "var v = qv.AngleTo(qv) + a\n fo = v",
        "var v = col.ToHex()\n so = v",
    ] {
        body_case(body);
    }
}

/// A `match` or `if let` past the cap on an enum or payload enum read from a
/// loop element or a record field tests its value, and a loop element binds,
/// stores and pushes like any enum value.
#[test]
fn enum_scrutinees_and_loop_elements_past_the_cap_keep_their_types() {
    for body in [
        "for x in hs { n = match x { Hue.Red => 1, _ => 2 } }",
        "for kk, v in mh { n = match v { Hue.Red => 1, _ => 2 } }",
        "n = match ph.h { Hue.Red => 1, _ => 2 }",
        "if let Hue.Blue = ph.h { n = 3 }",
        "for x in ss { if let S.Sa(z) = x { n = z } }",
        "for x in ss { match x { S.Sa(z) => { n = z }, S.Sb(w) => { fo = w }, _ => { n = 0 } } }",
        "let w = match s { S.Sa(z) => z, _ => 0 }\n var v = w\n n = v",
        "for x in hs { var v = x\n d = v }",
        "var ys: Hue[]\n for x in hs { ys.push(x) }",
    ] {
        body_case(body);
    }
}

/// A mod declared inside the generic body, plain or generic, `null`, block
/// expressions and `await` captures past the cap.
#[test]
fn body_declarations_null_blocks_and_awaits_past_the_cap_keep_their_types() {
    for body in [
        "mod inner(x: int) -> int { return x + 1 }\n var v = inner(k) + a\n n = v",
        "mod inner<T: int | float>(x: T) -> T { return x }\n var v = inner(a) + b\n n = v",
        "mod inner(x: int) -> string { return \"s\" }\n var v = inner(k)\n so = v",
        "var v: A = null\n v = null\n n = v",
        "var v: string = null\n so = v",
        "var v = { let q = a * 2\n q }\n n = v",
        "var v = { let q = st .. \"x\"\n q }\n so = v",
        "let q = await st on go\n var v = q\n so = v",
        "let q: int = await CustomEvent(\"x\")\n var v = q + a\n n = v",
        "await Sleep(_, delay = 1.0)\n var v = a + b\n n = v",
    ] {
        body_case(body);
    }
}

/// Generic calls two and three deep under a capped body, with inferred and
/// explicit type arguments: each nested instantiation is demanded under the
/// key it is reached with.
#[test]
fn nested_generic_calls_past_the_cap_keep_their_types() {
    for body in [
        "var v = id(a) + b\n n = v",
        "var v = g2(a, st)\n n = v",
        "var v = g3(a, st)\n n = v",
        "var v = g3<A, string>(a, st)\n n = v",
        "var v = g2<A, B>(a, b) + id<B>(b)\n n = v",
    ] {
        body_case(body);
    }
}

/// Calls into an `import * as` namespace past the cap, plain and generic.
#[test]
fn namespace_calls_past_the_cap_keep_their_types() {
    let lib = "mod li(x: int) -> int { return x + 1 }\nmod ls(x: int) -> string { return \"s${x}\" }\n\
               mod gid<A, C, B: int | string>(a: A, cc: C, b: B) -> B { let t = b\n return t }\n";
    let deps = [("u", lib)];
    for body in [
        "var v = u.li(k) + a\n n = v",
        "var v = u.ls(k)\n so = v",
        "var v = u.gid(a, b, st)\n so = v",
        "var v = u.gid(a, st, k)\n n = v",
    ] {
        let generic = format!(
            "import * as u from \"u\"\n{WIDE_PRELUDE}mod f<A: float | int, B: int | float, C: {S6}, D: {S6}, H: Hue | int>\
             (a: A, b: B, cc: C, dd: D, h: H) {{ {body} }}\non go {{ f(k, k2, k, k2, k) }}\n"
        );
        let mono = format!(
            "import * as u from \"u\"\n{WIDE_PRELUDE}mod f(a: int, b: int, cc: int, dd: int, h: int) {{ {body} }}\n\
             on go {{ f(k, k2, k, k2, k) }}\n"
        );
        let loader = crate::resolve::MemLoader {
            files: deps.iter().map(|(k, v)| (format!("{k}.ws"), v.to_string())).collect(),
        };
        let resolved = crate::resolve::resolve(&generic, "main", &loader);
        let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
        assert!(tc.demanded_instantiations.iter().any(|k| *k.3 == *vec![Type::Int; 5]), "`{body}` does not reach the cap");
        let (g, m) = (compile_multi(&generic, &deps), compile_multi(&mono, &deps));
        for (r, which) in [(&g, "generic"), (&m, "copy")] {
            assert!(
                r.diagnostics.iter().all(|d| d.severity != crate::diagnostic::Severity::Error),
                "`{body}` ({which}): {:?}",
                r.diagnostics
            );
            assert!(!has_gate(r, "_Unsupported"), "`{body}` ({which})");
        }
        assert_eq!(shape(&g), shape(&m), "`{body}`");
    }
}

/// Receiver methods in a generic chip past the cap.
#[test]
fn a_generic_chip_past_the_cap_keeps_method_types() {
    for (body, result) in [
        ("var t = st.ToUpper()\n so = t", "1.0"),
        ("var t = st.Length() + a", "t"),
        ("var t = vv.Normalize()\n vo = t", "1.0"),
        ("var t = col.ToHex()\n so = t", "1.0"),
    ] {
        let generic = format!(
            "{WIDE_PRELUDE}chip f<A: float | int, B: int | float, C: {S6}, D: {S6}, H: int | string>(a: A, b: B, cc: C, dd: D, h: H) \
             -> (r: float) {{ {body}\n out r = {result} }}\nlet x = f(k, fl, k, k2, k)\non go {{ fo = x }}\n"
        );
        let mono = format!(
            "{WIDE_PRELUDE}chip f(a: int, b: float, cc: int, dd: int, h: int) -> (r: float) {{ {body}\n out r = {result} }}\n\
             let x = f(k, fl, k, k2, k)\non go {{ fo = x }}\n"
        );
        assert!(demanded(&generic, &[Type::Int, Type::Float, Type::Int, Type::Int, Type::Int]), "chip `{body}` does not reach the cap");
        assert_same_as_monomorph(&generic, &mono, &format!("chip `{body}`"));
    }
}

/// A chain 24 operators deep over a method result, past the cap, compiles in
/// time linear in its depth: each type is read from the instantiation's own
/// check, never re-derived from its operands.
#[test]
fn a_deep_expression_past_the_cap_compiles_promptly() {
    let mut chain = "st.Length()".to_string();
    for _ in 0..24 {
        chain = format!("({chain} + a)");
    }
    let body = format!("var v = {chain}\n n = v");
    let start = std::time::Instant::now();
    body_case(&body);
    let elapsed = start.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(5), "depth 24 took {elapsed:?}");
}
