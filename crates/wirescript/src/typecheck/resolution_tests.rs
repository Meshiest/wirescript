//! Typecheck records what each method call, index access and loop source
//! resolved to, for lowering to replay.

use super::*;
use crate::parser::parse;

fn tc(src: &str) -> TypeCheckResult {
    let p = parse(src, "test");
    assert!(p.diagnostics.is_empty(), "parse diagnostics: {:?}", p.diagnostics);
    typecheck(&p.ast, "test", &crate::typecheck::CeSlotMap::default())
}

/// The key for the source text `needle` (first occurrence), outside any generic.
fn key(src: &str, needle: &str) -> ResolutionKey {
    let start = src.find(needle).unwrap_or_else(|| panic!("`{needle}` not in source"));
    (Arc::from("test"), start, start + needle.len(), Vec::new())
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
        r.container_kinds.get(&(Arc::from("test"), recv, recv + 2, Vec::new())),
        Some(&ContainerKind::Array { elem: Type::Int }),
        "the receiver `xs`, not its declaration"
    );
    assert_eq!(
        r.method_targets.get(&key(src, "m.set(\"a\", 2.0)")),
        Some(&MethodTarget::Container {
            kind: ContainerKind::Map { key: Type::String, value: Type::Float },
            method: "set".into()
        })
    );
}

#[test]
fn builtin_and_self_receivers_record_their_target() {
    let src = "mod twice(self: int) -> int { return self * 2 }\nvar n: int = 0\nvar v: vector = Vec(1.0, 2.0, 3.0)\nin go: exec\non go { n = n.twice()\n let d = v.Dot(v) }";
    let r = tc(src).resolutions;
    assert_eq!(r.method_targets.get(&key(src, "n.twice()")), Some(&MethodTarget::SelfMod("twice".into())));
    assert_eq!(r.method_targets.get(&key(src, "v.Dot(v)")), Some(&MethodTarget::Builtin("Dot".into())));
}

#[test]
fn index_access_assignment_targets_dual_args_and_loops_record_container_kinds() {
    let src = "var rows: int[][]\nvar all: int[]\nvar m: Map<string, int>\nvar n: int = 0\nin go: exec\n\
               on go { n = rows[0][1]\n rows[1][0] = 5\n m[\"k\"] = 3\n all.append(rows[1])\n for v in all { n = v } }";
    let r = tc(src).resolutions;
    let arr = |e: Type| ContainerKind::Array { elem: e };
    assert_eq!(r.container_kinds.get(&key(src, "rows[0]")), Some(&arr(Type::Int)), "the read's inner object");
    let second = src.find("rows[1][0]").unwrap();
    assert_eq!(
        r.container_kinds.get(&(Arc::from("test"), second, second + "rows[1]".len(), Vec::new())),
        Some(&arr(Type::Int)),
        "the write target's object"
    );
    let mk = src.find("m[\"k\"]").unwrap();
    assert_eq!(
        r.container_kinds.get(&(Arc::from("test"), mk, mk + 1, Vec::new())),
        Some(&ContainerKind::Map { key: Type::String, value: Type::Int })
    );
    let arg = src.find("append(rows[1])").unwrap() + "append(".len();
    assert_eq!(
        r.container_kinds.get(&(Arc::from("test"), arg, arg + "rows[1]".len(), Vec::new())),
        Some(&arr(Type::Int)),
        "a dual-array source argument"
    );
    let loop_src = src.find("in all").unwrap() + "in ".len();
    assert_eq!(
        r.container_kinds.get(&(Arc::from("test"), loop_src, loop_src + 3, Vec::new())),
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
    let k = |t: Type| (Arc::from("test"), call, call + "rows[0].push(v)".len(), vec![t]);
    assert_eq!(
        r.method_targets.get(&k(Type::Int)),
        Some(&MethodTarget::Container { kind: ContainerKind::Array { elem: Type::Int }, method: "push".into() })
    );
    assert_eq!(
        r.method_targets.get(&k(Type::String)),
        Some(&MethodTarget::Container { kind: ContainerKind::Array { elem: Type::String }, method: "push".into() })
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
    let k = |t: Type| (Arc::from("test"), call, call + "r[0].push(x)".len(), vec![t]);
    assert_eq!(
        r.method_targets.get(&k(Type::Int)),
        Some(&MethodTarget::Container { kind: ContainerKind::Array { elem: Type::Int }, method: "push".into() }),
        "the Int outer pass must not be overwritten by the String pass"
    );
    assert_eq!(
        r.method_targets.get(&k(Type::String)),
        Some(&MethodTarget::Container { kind: ContainerKind::Array { elem: Type::String }, method: "push".into() })
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
        (Arc::from("test"), call, call + "xs.push(w)".len(), vec![outer, inner])
    };
    // `inner`'s own type param `U` is bound to `Int` at THIS call site
    // (`inner(us, 1)`), so every outer pass records the SAME inner combo
    // (`Int`) but under a DIFFERENT outer prefix — both must survive.
    assert_eq!(
        r.method_targets.get(&k(Type::Int, Type::Int)),
        Some(&MethodTarget::Container { kind: ContainerKind::Array { elem: Type::Int }, method: "push".into() }),
        "outer=Int, inner=Int"
    );
    assert_eq!(
        r.method_targets.get(&k(Type::String, Type::Int)),
        Some(&MethodTarget::Container { kind: ContainerKind::Array { elem: Type::Int }, method: "push".into() }),
        "outer=String, inner=Int must not be overwritten by the outer=Int pass"
    );
}
