//! Typecheck records what each method call, index access and loop source
//! resolved to, for lowering to replay.

use super::*;
use crate::parser::parse;

fn tc(src: &str) -> TypeCheckResult {
    let p = parse(src, "test");
    assert!(
        p.diagnostics.is_empty(),
        "parse diagnostics: {:?}",
        p.diagnostics
    );
    typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
}

/// The key for the source text `needle` (first occurrence), outside any generic.
fn key(src: &str, needle: &str) -> ResolutionKey {
    let start = src
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not in source"));
    (
        Arc::from("test"),
        start,
        start + needle.len(),
        Arc::default(),
    )
}

#[test]
fn container_methods_record_their_target_and_receiver() {
    let src = "var xs: int[]\nvar m: Map<string, float>\nin go: exec\non go { xs.push(1)\n m.set(\"a\", 2.0) }";
    let r = tc(src).resolutions;
    assert_eq!(
        r.method_targets.get(&key(src, "xs.push(1)")),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::Int },
            method: "push".into()
        })
    );
    let recv = src.find("xs.push").unwrap();
    assert_eq!(
        r.container_kinds
            .get(&(Arc::from("test"), recv, recv + 2, Arc::default())),
        Some(&ContainerKind::Array { elem: Type::Int }),
        "the receiver `xs`, not its declaration"
    );
    assert_eq!(
        r.method_targets.get(&key(src, "m.set(\"a\", 2.0)")),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Map {
                key: Type::String,
                value: Type::Float
            },
            method: "set".into()
        })
    );
}

#[test]
fn builtin_and_self_receivers_record_their_target() {
    let src = "mod twice(self: int) -> int { return self * 2 }\nvar n: int = 0\nvar v: vector = Vec(1.0, 2.0, 3.0)\nin go: exec\non go { n = n.twice()\n let d = v.Dot(v) }";
    let r = tc(src).resolutions;
    assert_eq!(
        r.method_targets.get(&key(src, "n.twice()")),
        Some(&MethodTarget::SelfMod("twice".into()))
    );
    assert_eq!(
        r.method_targets.get(&key(src, "v.Dot(v)")),
        Some(&MethodTarget::Builtin("Dot".into()))
    );
}

/// A builtin or `self` receiver call whose arguments do not fit is an error and
/// records no target: lowering never replays it.
#[test]
fn a_receiver_call_with_arguments_that_do_not_fit_records_no_target() {
    let src = "mod twice(self: int, k: int) -> int { return self * k }\nvar n: int = 0\nvar v: vector = Vec(1.0, 2.0, 3.0)\n\
               in go: exec\non go { n = n.twice(\"s\")\n let d = v.Dot(\"s\") }";
    let r = tc(src);
    crate::test_support::assert_code_count(&r.diagnostics, "WS003", 2);
    assert_eq!(
        r.resolutions
            .method_targets
            .get(&key(src, "n.twice(\"s\")")),
        None
    );
    assert_eq!(
        r.resolutions.method_targets.get(&key(src, "v.Dot(\"s\")")),
        None
    );
}

#[test]
fn index_access_assignment_targets_dual_args_and_loops_record_container_kinds() {
    let src = "var rows: int[][]\nvar all: int[]\nvar m: Map<string, int>\nvar n: int = 0\nin go: exec\n\
               on go { n = rows[0][1]\n rows[1][0] = 5\n m[\"k\"] = 3\n all.append(rows[1])\n for v in all { n = v } }";
    let r = tc(src).resolutions;
    let arr = |e: Type| ContainerKind::Array { elem: e };
    assert_eq!(
        r.container_kinds.get(&key(src, "rows[0]")),
        Some(&arr(Type::Int)),
        "the read's inner object"
    );
    let second = src.find("rows[1][0]").unwrap();
    assert_eq!(
        r.container_kinds.get(&(
            Arc::from("test"),
            second,
            second + "rows[1]".len(),
            Arc::default()
        )),
        Some(&arr(Type::Int)),
        "the write target's object"
    );
    let mk = src.find("m[\"k\"]").unwrap();
    assert_eq!(
        r.container_kinds
            .get(&(Arc::from("test"), mk, mk + 1, Arc::default())),
        Some(&ContainerKind::Map {
            key: Type::String,
            value: Type::Int
        })
    );
    let arg = src.find("append(rows[1])").unwrap() + "append(".len();
    assert_eq!(
        r.container_kinds.get(&(
            Arc::from("test"),
            arg,
            arg + "rows[1]".len(),
            Arc::default()
        )),
        Some(&arr(Type::Int)),
        "a dual-array source argument"
    );
    let loop_src = src.find("in all").unwrap() + "in ".len();
    assert_eq!(
        r.container_kinds
            .get(&(Arc::from("test"), loop_src, loop_src + 3, Arc::default())),
        Some(&arr(Type::Int)),
        "a for source"
    );
}

#[test]
fn an_erroring_call_records_nothing() {
    let src = "var n: int = 0\nin go: exec\non go { n = n.nosuch() }";
    let r = tc(src).resolutions;
    assert!(r.method_targets.get(&key(src, "n.nosuch()")).is_none());
}

#[test]
fn a_generic_body_records_one_entry_per_combination() {
    let src = "mod pushAll<T: int | string>(rows: T[][], v: T) { rows[0].push(v) }\n\
               var a: int[][]\nvar b: string[][]\nin go: exec\non go { pushAll(a, 1)\n pushAll(b, \"x\") }";
    let r = tc(src).resolutions;
    let call = src.find("rows[0].push(v)").unwrap();
    let k = |t: Type| {
        (
            Arc::from("test"),
            call,
            call + "rows[0].push(v)".len(),
            Arc::<[Type]>::from(vec![t]),
        )
    };
    assert_eq!(
        r.method_targets.get(&k(Type::Int)),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::Int },
            method: "push".into()
        })
    );
    assert_eq!(
        r.method_targets.get(&k(Type::String)),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::String },
            method: "push".into()
        })
    );
}

/// A NON-generic mod nested inside a generic mod's body (`Stmt::ChipDecl`) is
/// itself body-checked once per OUTER combination, so its own resolutions must
/// key by the enclosing substitution, not by its own (empty) one — otherwise
/// every outer pass would overwrite the same `(range, [])` entry and only the
/// last-checked outer type would survive.
#[test]
fn a_nested_mod_records_under_the_enclosing_generic_substitution() {
    let src = "mod pushAll<T: int | string>(rows: T[][], v: T) {\n\
                   mod inner(r: T[][], x: T) { r[0].push(x) }\n\
                   inner(rows, v)\n\
               }\n\
               var a: int[][]\nvar b: string[][]\nin go: exec\n\
               on go { pushAll(a, 1)\n pushAll(b, \"x\") }";
    let r = tc(src).resolutions;
    let call = src.find("r[0].push(x)").unwrap();
    let k = |t: Type| {
        (
            Arc::from("test"),
            call,
            call + "r[0].push(x)".len(),
            Arc::<[Type]>::from(vec![t]),
        )
    };
    assert_eq!(
        r.method_targets.get(&k(Type::Int)),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::Int },
            method: "push".into()
        }),
        "the Int outer pass must not be overwritten by the String pass"
    );
    assert_eq!(
        r.method_targets.get(&k(Type::String)),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::String },
            method: "push".into()
        })
    );
}

/// A generic mod nested inside another generic mod's body: the inner body is
/// checked once per (outer combo, inner combo) pair, so its key is the full
/// concatenated stack, outermost first.
#[test]
fn a_generic_nested_in_a_generic_keys_by_the_concatenated_substitution_stack() {
    let src = "mod outer<T: int | string>(v: T) {\n\
                   mod inner<U: int | float>(xs: U[], w: U) { xs.push(w) }\n\
                   var us: int[]\n\
                   inner(us, 1)\n\
               }\n\
               in go: exec\non go { outer(1)\n outer(\"x\") }";
    let r = tc(src).resolutions;
    let call = src.find("xs.push(w)").unwrap();
    let k = |outer: Type, inner: Type| {
        (
            Arc::from("test"),
            call,
            call + "xs.push(w)".len(),
            Arc::<[Type]>::from(vec![outer, inner]),
        )
    };
    // `inner`'s own type param `U` is bound to `Int` at THIS call site
    // (`inner(us, 1)`), so every outer pass records the SAME inner combo
    // (`Int`) but under a DIFFERENT outer prefix — both must survive.
    assert_eq!(
        r.method_targets.get(&k(Type::Int, Type::Int)),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::Int },
            method: "push".into()
        }),
        "outer=Int, inner=Int"
    );
    assert_eq!(
        r.method_targets.get(&k(Type::String, Type::Int)),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Array { elem: Type::Int },
            method: "push".into()
        }),
        "outer=String, inner=Int must not be overwritten by the outer=Int pass"
    );
}

/// A range in `src`: `len` bytes from `offset` past the first `needle`.
fn span(src: &str, needle: &str, offset: usize, len: usize) -> (Arc<str>, usize, usize) {
    let at = src
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not in source"))
        + offset;
    (Arc::from("test"), at, at + len)
}

/// The type of the expression at `range` in the instantiation keyed by `subst`.
fn instance_type(
    r: &Resolutions,
    range: &(Arc<str>, usize, usize),
    subst: &[Type],
) -> Option<Type> {
    r.generic_expr_types
        .get(&(range.0.clone(), range.1, range.2, Arc::from(subst)))
        .cloned()
}

/// Whether the body of the mod declared at `decl` was checked under `subst`.
fn body_checked(r: &Resolutions, src: &str, decl: &str, subst: &[Type]) -> bool {
    let at = src.find(decl).unwrap();
    r.checked_bodies
        .iter()
        .any(|(_, start, _, key)| *start == at && **key == *subst)
}

const CAPPED: &str = "enum Hue { Red, Green }\nmod f<A, C, B: int | Hue>(a: A, cc: C, b: B) -> B { let t = [b]\n return b }\n";

/// Past the combination cap a generic body is checked for a sample of
/// combinations; `f<int, int, int>` differs from the sampled representative
/// in two positions, so only its call's demand checks it. The call records
/// the type arguments lowering instantiates with.
#[test]
fn an_instantiation_past_the_cap_is_checked_for_its_call() {
    let src = format!("{CAPPED}var k: int = 0\nin go: exec\non go {{ k = f(k, k, k) }}");
    let r = tc(&src);
    let instance = vec![Type::Int; 3];
    assert_eq!(r.demanded_instantiations.len(), 1);
    assert!(body_checked(&r.resolutions, &src, "mod f", &instance));
    let res = &r.resolutions;
    assert_eq!(
        instance_type(res, &span(&src, "return b", 7, 1), &instance),
        Some(Type::Int)
    );
    assert_eq!(
        instance_type(res, &span(&src, "[b]", 0, 3), &instance),
        Some(Type::Array(Box::new(Type::Int)))
    );
    let call = span(&src, "f(k, k, k)", 0, 10);
    assert_eq!(
        res.call_type_args
            .get(&(call.0, call.1, call.2, Arc::default())),
        Some(&instance)
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
}

/// An instantiation is checked once however many calls demand it, and a
/// combination the bounds already cover is not checked again.
#[test]
fn each_demanded_instantiation_is_checked_once() {
    let src = format!(
        "{CAPPED}var k: int = 0\nvar q: bool = false\nin go: exec\nin go2: exec\n\
         on go {{ k = f(k, k, k)\n k = f(k, k, k) }}\non go2 {{ k = f(k, k, k)\n k = f(q, q, k) }}"
    );
    let r = tc(&src);
    assert_eq!(
        r.demanded_instantiations.len(),
        1,
        "`f<bool, bool, int>` is the sampled representative"
    );
    assert!(body_checked(
        &r.resolutions,
        &src,
        "mod f",
        &[Type::Bool, Type::Bool, Type::Int]
    ));
}

/// A demanded instantiation's own calls demand theirs, under the composed
/// key: `g<int, string, int>` calls `f(y, x, z)`, which is `f<string, int,
/// int>`, past the cap too.
#[test]
fn a_demanded_instantiation_demands_the_instantiations_it_calls() {
    let src = format!(
        "{CAPPED}mod g<X, Y, Z: int | Hue>(x: X, y: Y, z: Z) -> Z {{ return f(y, x, z) }}\n\
         var k: int = 0\nvar s: string = \"\"\nin go: exec\non go {{ k = g(k, s, k) }}"
    );
    let r = tc(&src);
    let res = &r.resolutions;
    let g = vec![Type::Int, Type::String, Type::Int];
    let f = vec![Type::String, Type::Int, Type::Int];
    assert_eq!(r.demanded_instantiations.len(), 2);
    assert!(body_checked(res, &src, "mod g", &g) && body_checked(res, &src, "mod f", &f));
    let call = span(&src, "f(y, x, z)", 0, 10);
    assert_eq!(
        res.call_type_args
            .get(&(call.0, call.1, call.2, Arc::from(g))),
        Some(&f)
    );
    assert_eq!(
        instance_type(res, &span(&src, "[b]", 0, 3), &f),
        Some(Type::Array(Box::new(Type::Int)))
    );
}

/// A generic declared inside a generic body keys its body by the enclosing
/// instantiation's arguments, then its own, and is checked where it is
/// declared: `h` names the enclosing `cap`.
#[test]
fn a_generic_declared_in_a_demanded_body_is_checked_for_its_calls() {
    let src = format!(
        "{CAPPED}mod g<X, Y, Z: int | Hue>(x: X, y: Y, z: Z) -> Z {{\n\
         let cap = z\n\
         mod h<P, Q: int | Hue>(p: P, q: Q) -> Q {{ let w = cap\n return q }}\n\
         return h(y, z) }}\n\
         var k: int = 0\nvar s: string = \"\"\nin go: exec\non go {{ k = g(k, s, k) }}"
    );
    let r = tc(&src);
    let key = vec![Type::Int, Type::String, Type::Int, Type::String, Type::Int];
    assert!(body_checked(&r.resolutions, &src, "mod h", &key));
    assert_eq!(
        instance_type(&r.resolutions, &span(&src, "w = cap", 4, 3), &key),
        Some(Type::Int)
    );
    assert_eq!(
        instance_type(&r.resolutions, &span(&src, "h(y, z)", 2, 1), &key[..3]),
        Some(Type::String)
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
}

/// Explicit type arguments are the ones recorded and checked.
#[test]
fn explicit_type_arguments_are_the_demanded_instantiation() {
    let src = format!(
        "{CAPPED}var k: int = 0\nin go: exec\non go {{ k = f<int, string, int>(k, \"x\", k) }}"
    );
    let r = tc(&src);
    let instance = vec![Type::Int, Type::String, Type::Int];
    let call = span(&src, "f<int", 0, "f<int, string, int>(k, \"x\", k)".len());
    assert_eq!(
        r.resolutions
            .call_type_args
            .get(&(call.0, call.1, call.2, Arc::default())),
        Some(&instance)
    );
    assert!(body_checked(&r.resolutions, &src, "mod f", &instance));
}

/// A body error only one demanded instantiation has is reported, once.
#[test]
fn a_demanded_instantiation_reports_its_own_errors() {
    let src = "mod f<A, C, B: int | string>(a: A, cc: C, b: B) -> int { return b * 2 }\n\
               var k: int = 0\nvar s: string = \"\"\nin go: exec\non go { k = f(k, k, s)\n k = f(k, k, s) }";
    let r = tc(src);
    let at = src.find("b * 2").unwrap();
    let errors: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.range.start.offset == at)
        .collect();
    assert_eq!(errors.len(), 1, "{:?}", r.diagnostics);
}

/// Past the bound on demanded instantiations the rest are reported, not
/// checked and not dropped silently.
#[test]
fn too_many_demanded_instantiations_is_an_error() {
    let params: Vec<String> = (0..11).map(|i| format!("T{i}: int | float")).collect();
    let args: Vec<String> = (0..11).map(|i| format!("a{i}: T{i}")).collect();
    let mut src = format!(
        "mod f<{}>({}) -> int {{ return 1 }}\nvar k: int = 0\nvar x: float = 0.0\nin go: exec\non go {{\n",
        params.join(", "),
        args.join(", ")
    );
    let mut calls = 0;
    for pattern in 0u32..2048 {
        if pattern.count_ones() < 2 {
            continue;
        }
        let picks: Vec<&str> = (0..11)
            .map(|i| if pattern >> i & 1 == 1 { "x" } else { "k" })
            .collect();
        src.push_str(&format!(" k = f({})\n", picks.join(", ")));
        calls += 1;
        if calls == instances::MAX_DEMANDED_INSTANTIATIONS + 64 {
            break;
        }
    }
    src.push('}');
    let r = tc(&src);
    assert_eq!(
        r.demanded_instantiations.len(),
        instances::MAX_DEMANDED_INSTANTIATIONS
    );
    let over: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS096").collect();
    assert_eq!(
        over.len(),
        1,
        "{:?}",
        r.diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}

/// The bound is on all generics together, so WS096 reports the total the calls
/// demand and the generics demanding the most, not the one whose call came
/// past the bound.
#[test]
fn too_many_demanded_instantiations_names_the_largest_demanders() {
    let params: Vec<String> = (0..11).map(|i| format!("T{i}: int | float")).collect();
    let args: Vec<String> = (0..11).map(|i| format!("a{i}: T{i}")).collect();
    let mut src = String::new();
    for name in ["big", "small"] {
        src.push_str(&format!(
            "mod {name}<{}>({}) -> int {{ return 1 }}\n",
            params.join(", "),
            args.join(", ")
        ));
    }
    src.push_str("var k: int = 0\nvar x: float = 0.0\nin go: exec\non go {\n");
    let patterns: Vec<String> = (0u32..2048)
        .filter(|p| p.count_ones() >= 2)
        .map(|p| {
            (0..11)
                .map(|i| if p >> i & 1 == 1 { "x" } else { "k" })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect();
    for p in &patterns[..900] {
        src.push_str(&format!(" k = big({p})\n"));
    }
    for p in &patterns[..200] {
        src.push_str(&format!(" k = small({p})\n"));
    }
    src.push('}');
    let r = tc(&src);
    let over: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS096").collect();
    assert_eq!(over.len(), 1);
    let msg = &over[0].message;
    let number_after = |text: &str| -> usize {
        let at = msg
            .find(text)
            .unwrap_or_else(|| panic!("no `{text}` in {msg}"))
            + text.len();
        msg[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap()
    };
    let (total, big, small) = (
        number_after("at least "),
        number_after("`big` "),
        number_after("`small` "),
    );
    assert!(big > small && small > 100, "{msg}");
    assert_eq!(total, big + small, "{msg}");
    assert!(total > instances::MAX_DEMANDED_INSTANTIATIONS, "{msg}");
    assert!(msg.find("`big`") < msg.find("`small`"), "{msg}");
}
