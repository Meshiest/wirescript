//! Monomorphization of generic `mod` calls: a generic mod inlined
//! at a concrete call site must emit CONCRETE gates (`box<int>` -> int gates,
//! `box<vector>` -> vector gates), never leaking `Type::Param` (or an
//! incorrectly-defaulted `Any`/last-combo variant) into emit.

use super::*;
use crate::ir::{Module, Type};
use crate::test_support::assert_code;

/// Does any port anywhere in the module tree still carry a `Type::Param`?
/// After monomorphization of a fully-applied generic call the answer must be
/// no — a leaked `Param` trips emit's `var_type_to_wire_variant` debug_assert.
fn any_param_port(module: &Module) -> bool {
    fn ty_has_param(t: &Type) -> bool {
        match t {
            Type::Param(_) => true,
            Type::Array(i) | Type::Ref(i) => ty_has_param(i),
            Type::Map(k, v) => ty_has_param(k) || ty_has_param(v),
            Type::Union(o) | Type::Tuple(o) => o.iter().any(ty_has_param),
            Type::Record(f) => f.iter().any(|(_, t)| ty_has_param(t)),
            _ => false,
        }
    }
    module.nodes_deep().any(|n| {
        n.ports
            .inputs
            .iter()
            .chain(n.ports.outputs.iter())
            .any(|p| ty_has_param(&p.ty))
    })
}

/// The `Value`-port type of every `Pseudo_Var` gate `nodes` holds: one per
/// inlined `var stored: T`, so this reads out each monomorph's storage type.
fn var_value_types<'a>(nodes: impl Iterator<Item = &'a crate::ir::Node>) -> Vec<Type> {
    nodes
        .filter(|n| n.gate_class == gc::VAR)
        .filter_map(|n| {
            n.ports
                .inputs
                .iter()
                .chain(n.ports.outputs.iter())
                .find(|p| crate::intern::resolve(p.name) == "Value")
        })
        .map(|p| p.ty.clone())
        .collect()
}

/// [`var_value_types`] across the whole module tree.
fn pseudo_var_value_types(module: &Module) -> Vec<Type> {
    var_value_types(module.nodes_deep())
}

/// Output-port types of every gate whose class contains `needle`, across the
/// whole module tree. Used to read out an operator's / if-expr's emitted variant.
fn gate_output_types(module: &Module, needle: &str) -> Vec<Type> {
    module
        .nodes_deep()
        .filter(|n| n.gate_class.contains(needle))
        .flat_map(|n| n.ports.outputs.iter().map(|p| p.ty.clone()))
        .collect()
}

/// `types` in one canonical order. `Type` has no `Ord`, so the order is its
/// `Debug` rendering: what matters is that two lists compare as multisets.
fn sorted(mut types: Vec<Type>) -> Vec<Type> {
    types.sort_by_key(|t| format!("{t:?}"));
    types
}

pub(super) fn emit_ok(r: &LowerResult) -> Result<(), String> {
    let lr = crate::layout::layout(&r.module);
    let opts = crate::emit::EmitOptions::default();
    crate::emit::emit_brz(&r.module, &Default::default(), &lr, &opts)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// A `(label, source, storage types, distinct chip templates)` row: the source
/// lowers with no error and emits, no port keeps a `Type::Param`, its
/// `Pseudo_Var` gates hold exactly the listed types (in any order) and, when
/// given, its chip instances share exactly that many templates.
type MonoRow<'a> = (&'a str, String, Vec<Type>, Option<usize>);

#[track_caller]
fn check_monomorphs(rows: &[MonoRow]) {
    let mut failures = Vec::new();
    for (label, src, vars, templates) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        if let Err(e) = emit_ok(&r) {
            failures.push(format!("{label}: emit failed: {e}"));
        }
        if any_param_port(&r.module) {
            failures.push(format!("{label}: a port still carries Type::Param"));
        }
        let got = sorted(pseudo_var_value_types(&r.module));
        if got != sorted(vars.clone()) {
            failures.push(format!("{label}: storage types {got:?}, want {vars:?}"));
        }
        if let Some(want) = templates {
            let keys = chip_template_keys(&r.module);
            if keys.len() != *want {
                failures.push(format!(
                    "{label}: {} templates {keys:?}, want {want}",
                    keys.len()
                ));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Each generic instantiation lowers its `T`-typed storage to the concrete
/// type flowing at that call site, through every way a call binds `T`.
#[test]
fn generic_storage_monomorphizes_per_instantiation() {
    use Type::{Bool, Float, Int, String as Str, Vector};
    let rows: Vec<MonoRow> = vec![
        (
            "a T-typed storage gate + T return, called at `int` and at `vector`: a leaked \
             `Type::Param` would give both a wrong (Number-defaulted) variant",
            "mod boxed<T>(v: T) -> T { static var stored: T = v\n return stored }\n\
             in go: exec\nin n: int\nin vec: vector\non go {\n  let a = boxed(n)\n  let b = boxed(vec)\n}\n"
                .into(),
            vec![Int, Vector],
            None,
        ),
        (
            "a bounded `<T: Numeric>` selecting between two args, called at int and at float",
            "mod pick<T: Numeric>(c: bool, a: T, b: T) -> T {\nstatic var chosen: T = a\n  return chosen\n}\n\
             in go: exec\nin i: int\nin f: float\non go {\n  let x = pick(true, i, i)\n  let y = pick(false, f, f)\n}\n"
                .into(),
            vec![Float, Int],
            None,
        ),
        (
            "`outer<T>` forwards its `T` into `inner<T>`: the stale last-mask-member type left in \
             `type_of_expr` collapsed BOTH monomorphs to Prefab",
            "mod inner<T>(v: T) -> T { static var s: T = v\n return s }\nmod outer<T>(v: T) -> T { let r = inner(v)\n return r }\n\
             in go: exec\nin n: int\nin vec: vector\non go {\n  let a = outer(n)\n  let b = outer(vec)\n}\n"
                .into(),
            vec![Int, Vector],
            None,
        ),
        (
            "the bounded forwarding form collapsed both to `Color` (the last Numeric mask member)",
            "mod inner<T: Numeric>(v: T) -> T { static var s: T = v\n return s }\n\
             mod outer<T: Numeric>(v: T) -> T { let r = inner(v)\n return r }\n\
             in go: exec\nin i: int\nin f: float\non go {\n  let a = outer(i)\n  let b = outer(f)\n}\n"
                .into(),
            vec![Float, Int],
            None,
        ),
        (
            "two `return`s route through the multi-return `mod_return_var`, whose `out_type` is the \
             other annotation leak point",
            "mod choose<T>(c: bool, a: T, b: T) -> T {\nif c { return a }\n  return b\n}\n\
             in go: exec\nin i: int\nin v: vector\non go {\n  let x = choose(true, i, i)\n  let y = choose(false, v, v)\n}\n"
                .into(),
            vec![Int, Vector],
            None,
        ),
        (
            "`zero<T>()` has `T` only in the return: an explicit type argument is the only way to call it",
            "mod zero<T: Numeric>() -> T { static var z: T\n return z }\n\
             in go: exec\nin n: int\nin vec: vector\non go { let a = zero<int>()\n let b = zero<vector>() }\n"
                .into(),
            vec![Int, Vector],
            None,
        ),
        (
            "a generic `self`-mod: the receiver drives `T`",
            "mod boxed<T>(self: T) -> T { static var stored: T = self\n return stored }\n\
             in n: int\nin vec: vector\nin go: exec\non go {\n  let a = n.boxed()\n  let b = vec.boxed()\n}\n"
                .into(),
            vec![Int, Vector],
            None,
        ),
        (
            "a COMPOUND arg (`a + b`, a nested generic call) forwarded from inside a generic body \
             monomorphizes to the CURRENT type, never the stale Numeric-mask Color/Vector",
            "chip Box<U>(x: U) -> (r: U) { var stored: U = x\n out r = stored }\nmod id<V>(x: V) -> V { return x }\n\
             mod outer<T: Numeric>(a: T, b: T) -> T {\n  let p = Box(a + b)\n  let q = Box(id(a))\n  return p }\n\
             in go: exec\nin n: int\non go { let z = outer(n, n) }\n"
                .into(),
            vec![Int, Int],
            None,
        ),
        (
            "two generic CHIP calls at the SAME type share ONE template (grid dedup holds, re-keyed on \
             the concrete monomorph)",
            "chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
             in go: exec\nin n: int\nin m: int\non go {\n  let a = Boxed(n)\n  let b = Boxed(m)\n}\n"
                .into(),
            vec![Int, Int],
            Some(1),
        ),
        (
            "a shared foldable CONST arg (`k = 7`) gives both instances the same const-fold key suffix: \
             only the monomorph key base tells `Pair<Int>` from `Pair<Vector>` (the sole guard of \
             `fold_key = key.clone()`)",
            "chip Pair<T>(v: T, k: int) -> (r: T) { var stored: T = v\n out r = stored }\n\
             in go: exec\nin a: int\nin b: vector\non go {\n  let x = Pair(a, 7)\n  let y = Pair(b, 7)\n}\n"
                .into(),
            vec![Int, Vector],
            Some(2),
        ),
        (
            "`Two<int, vector>` vs `Two<vector, int>`: the key renders params in declaration order",
            "chip Two<T, U>(a: T, b: U) -> (x: T, y: U) { var sa: T = a\n var sb: U = b\nout x = sa\n out y = sb }\n\
             in go: exec\nin n: int\nin vec: vector\non go {\n  let p = Two(n, vec)\n  let q = Two(vec, n)\n}\n"
                .into(),
            vec![Int, Int, Vector, Vector],
            Some(2),
        ),
        (
            "`B` inlined inside `A` reads both `A`'s `T` and its own `U`",
            "in n: int\nin s: string\nin f: float\nin g: bool\n\
             mod A<T: int | string, V: float | bool>(v: T, w: V) -> (r: T) {\n  mod B<U: float | bool>(t: T, u: U) -> (r2: T) {\n    \
             var st: T = t\n    var su: U = u\n    return st\n  }\n  return B(v, w)\n}\nout a = A(n, f)\nout b = A(s, g)\n"
                .into(),
            vec![Bool, Float, Int, Str],
            None,
        ),
        (
            "`helper` declared in `A` reads `A`'s `T` when called from `B`, whose own `T` shadows it",
            "in n: int\nin s: string\nin f: float\n\
             mod A<T: int | string>(v: T, w: float) -> (r: T) {\n  mod helper(x: T) -> (r2: T) {\n    var st: T = x\n    return st\n  }\n  \
             mod B<T: float | bool>(u: T) -> (r3: T) {\n    let z = helper(v)\n    return u\n  }\n  let bb = B(w)\n  return v\n}\n\
             out a = A(n, f)\nout b = A(s, f)\n"
                .into(),
            vec![Int, Str],
            None,
        ),
        (
            "the same shape with `B<U>`: nothing shadows `T`, so the caller's and lexical frames agree",
            "in n: int\nin s: string\nin f: float\n\
             mod A<T: int | string>(v: T, w: float) -> (r: T) {\n  mod helper(x: T) -> (r2: T) {\n    var st: T = x\n    return st\n  }\n  \
             mod B<U: float | bool>(u: U) -> (r3: U) {\n    let z = helper(v)\n    return u\n  }\n  let bb = B(w)\n  return v\n}\n\
             out a = A(n, f)\nout b = A(s, f)\n"
                .into(),
            vec![Int, Str],
            None,
        ),
        (
            "a top-level non-generic `helper` inlined from `outer<T>` pushes no frame, and lowers as when \
             called directly (a non-generic mod pushes no MonoFrame at all)",
            "in n: int\nin s: string\nin k: int\nmod helper(x: int) -> (r: int) {\n  var st: int = x\n  return st\n}\n\
             mod outer<T: int | string>(v: T, x: int) -> (r: int) {\n  return helper(x)\n}\n\
             out a = outer(n, k)\nout b = outer(s, k)\nout c = helper(k)\n"
                .into(),
            vec![Int, Int, Int],
            None,
        ),
    ];
    check_monomorphs(&rows);
}

/// A type parameter bound only through a composite argument (an array, a ref
/// to one, a generic alias record, a map, a forwarded record or element, a
/// multi-output result) or an explicit type argument binds per call. Each
/// row's storage includes the caller's own vars.
#[test]
fn a_type_param_bound_through_a_composite_argument_binds_per_call() {
    use Type::{Int, String as Str};
    let forwarded = |inner: &str, body: &str| {
        format!(
            "var ss: string[]\nin s: string\nin i: int\nvar n: string = \"\"\nvar k: int = 0\n{inner}\n\
             mod outer<A: string | int>(x: A) -> A {{\n{body}\n}}\nin go: exec\non go {{\n  n = outer(s)\n  k = outer(i)\n}}\n"
        )
    };
    let rows: Vec<MonoRow> = vec![
        (
            "`A` appears only as the element type of `xs`",
            "var ss: string[]\nvar is: int[]\nvar n: string = \"\"\nvar k: int = 0\n\
             mod f<A: string | int>(xs: A[]) -> A {\n  var t: A = xs[0]\n  return t\n}\nin go: exec\non go {\n  n = f(ss)\n  k = f(is)\n}\n"
                .into(),
            vec![Int, Int, Str, Str],
            None,
        ),
        (
            "a `*A[]` reference parameter",
            "var ss: string[]\nvar n: string = \"\"\nmod f<A: string | int>(xs: *A[]) -> A {\n  var t: A = xs[0]\n  return t\n}\n\
             in go: exec\non go {\n  n = f(ss)\n}\n"
                .into(),
            vec![Str, Str],
            None,
        ),
        (
            "a generic alias record `Box<A>`",
            "type Box<T> = { v: T, w: int }\nin s: string\nvar n: string = \"\"\n\
             mod f<A: string | int>(b: Box<A>) -> A {\n  var t: A = b.v\n  return t\n}\nin go: exec\non go {\n  n = f({ v: s, w: 1 })\n}\n"
                .into(),
            vec![Str, Str],
            None,
        ),
        (
            "`outer`'s record param forwarded on: its type-map entry is whichever combination typecheck \
             checked last, so the inner call reads the record's own bindings",
            "in s: string\nin i: int\nvar n: string = \"\"\nvar k: int = 0\n\
             mod inner<B: string | int>(b: { v: B, w: int }) -> B {\n  var t: B = b.v\n  return t\n}\n\
             mod outer<A: string | int>(b: { v: A, w: int }) -> A {\n  return inner(b)\n}\n\
             in go: exec\non go {\n  n = outer({ v: s, w: 1 })\n  k = outer({ v: i, w: 1 })\n}\n"
                .into(),
            vec![Int, Int, Str, Str],
            None,
        ),
        (
            "an element `rs[0]` forwarded from a generic caller",
            "var srows: string[][]\nvar irows: int[][]\nvar n: string = \"\"\nvar k: int = 0\n\
             mod inner<B: string | int>(xs: B[]) -> B {\n  var t: B = xs[0]\n  return t\n}\n\
             mod outer<A: string | int>(rs: A[][]) -> A {\n  return inner(rs[0])\n}\n\
             in go: exec\non go {\n  n = outer(srows)\n  k = outer(irows)\n}\n"
                .into(),
            vec![Int, Int, Str, Str],
            None,
        ),
        (
            "a `Map<K, V>` argument, directly and as an element forwarded from a generic caller",
            "var m: Map<string, int>\nvar ms: Map<string, int>[]\nvar k: int = 0\nvar j: int = 0\n\
             mod f<K: string | int, V: int | string>(mm: Map<K, V>, key: K) -> V {\n  var t: V = mm.get(key).Value\n  return t\n}\n\
             mod outer<K: string | int, V: int | string>(rows: Map<K, V>[], key: K) -> V {\n  return f(rows[0], key)\n}\n\
             in go: exec\non go {\n  k = f(m, \"a\")\n  j = outer(ms, \"a\")\n}\n"
                .into(),
            vec![Int; 4],
            None,
        ),
        (
            "`r.v` holds `ss.pop()`, bound by its first output port: `B` comes from its `Value` output, \
             not from that port's type",
            "var ss: string[]\nin s: string\nin i: int\nvar n: string = \"\"\nvar k: int = 0\n\
             mod inner<B: string | int>(b: { v: { IsEmpty: bool, Value: B }, w: int }) -> B {\n  var t: B = b.v.Value\n  return t\n}\n\
             mod outer<A: string | int>(x: A) -> A {\n  let r = { v: ss.pop(), w: 1 }\n  let q = inner(r)\n  return x\n}\n\
             in go: exec\non go {\n  n = outer(s)\n  k = outer(i)\n}\n"
                .into(),
            vec![Int, Str, Str, Str],
            None,
        ),
        (
            "the multi-output result's field `r.v` passed on its own",
            "var ss: string[]\nin s: string\nin i: int\nvar n: string = \"\"\nvar k: int = 0\n\
             mod inner<B: string | int>(v: { IsEmpty: bool, Value: B }) -> B {\n  var t: B = v.Value\n  return t\n}\n\
             mod outer<A: string | int>(x: A) -> A {\n  let r = { v: ss.pop(), w: 1 }\n  let q = inner(r.v)\n  return x\n}\n\
             in go: exec\non go {\n  n = outer(s)\n  k = outer(i)\n}\n"
                .into(),
            vec![Int, Str, Str, Str],
            None,
        ),
        (
            "a record literal forwarded from a generic caller",
            forwarded("mod inner<B: string | int>(b: { v: B, w: int }) -> B {\n var t: B = b.v\n return t\n}", "return inner({ v: x, w: 1 })"),
            vec![Int, Int, Str, Str],
            None,
        ),
        (
            "a tuple element forwarded from a generic caller",
            forwarded("mod inner<B: string | int>(b: B) -> B {\n var t: B = b\n return t\n}", "let tp = (x, 1)\n return inner(tp.0)"),
            vec![Int, Int, Str, Str],
            None,
        ),
        (
            "`zero<A>()` has no argument to infer `X` from: its type argument alone fixes it, so `inner`'s `B` is `A`",
            "in s: string\nin i: int\nvar n: string = \"\"\nvar k: int = 0\n\
             mod zero<X: string | int>() -> X {\n var z: X\n return z\n}\nmod inner<B: string | int>(b: B) -> B {\n var t: B = b\n return t\n}\n\
             mod outer<A: string | int>(x: A) -> A {\n return inner(zero<A>())\n}\nin go: exec\non go {\n  n = outer(s)\n  k = outer(i)\n}\n"
                .into(),
            vec![Int, Int, Int, Str, Str, Str],
            None,
        ),
        (
            "`id<T>(v)` is `outer`'s expression: its `T` is `outer`'s (string), not `inner`'s own `T` (int)",
            "mod id<X: int | string>(v: X) -> X {\n var t: X = v\n return t\n}\n\
             mod inner<T: int | string, S: int | string>(a: T, b: S) -> S {\n var u: S = b\n return u\n}\n\
             mod outer<T: int | string, U: int | string>(v: T, u: U) -> T {\n return inner(u, id<T>(v))\n}\n\
             in s: string\nin i: int\nvar o1: string = \"\"\nin go: exec\non go {\n  o1 = outer(s, i)\n}\n"
                .into(),
            vec![Str; 3],
            None,
        ),
        (
            "`exec = fire<T>(..)` is `outer`'s expression too: `fire`'s `X` is `outer`'s `T`, `inner`'s `u` its own `T`",
            "in s: string\nin i: int\nin go: exec\nvar o1: int = 0\n\
             mod fire<X: int | string>(v: X) -> (done: exec) {\n var t: X = v\n emit done\n}\n\
             mod inner<T: int | string>(a: T) -> T {\n var u: T = a\n return u\n}\n\
             mod outer<T: int | string, U: int | string>(v: T, u: U) -> U {\n  return inner(u, exec = fire<T>(v, exec = go))\n}\n\
             on go { o1 = outer(s, i) }\n"
                .into(),
            vec![Int, Int, Str],
            None,
        ),
    ];
    check_monomorphs(&rows);
}

#[test]
fn generic_mod_buffer_and_array_annotations_monomorphize() {
    // `T`-typed buffer + `T[]` array declared inside a generic mod body: both
    // annotation-resolution leak points must monomorphize to concrete element
    // types (int here), not `Any`.
    let src = "mod work<T>(x: T) -> T {\n\
               buffer prev: T = x\n  var items: T[] = []\n  items.push(x)\n  return prev\n}\n\
               in go: exec\nin n: int\non go {\n  let a = work(n)\n}\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));

    // Buffer gate: Input/Output typed int.
    let buf = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::BUFFER_TICKS)
        .expect("a buffer gate");
    assert!(
        buf.ports
            .inputs
            .iter()
            .chain(buf.ports.outputs.iter())
            .any(|p| p.ty == Type::Int),
        "buffer of a generic `T` must monomorphize to int"
    );

    // Array gate: ArrayVarRef element type is int.
    let arr = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::ARRAY_VAR)
        .expect("an array gate");
    let elem = arr
        .ports
        .outputs
        .iter()
        .find(|p| crate::intern::resolve(p.name) == "ArrayVarRef")
        .map(|p| &p.ty);
    assert!(
        matches!(elem, Some(Type::Ref(inner)) if matches!(inner.as_ref(), Type::Array(e) if **e == Type::Int)),
        "array of a generic `T` must monomorphize to int[], got {elem:?}"
    );
}

#[test]
fn generic_mod_polymorphic_recursion_terminates() {
    // A generic mod that (transitively) calls itself would make
    // monomorphization non-terminating. The existing WS020 recursion guard
    // (in `lower_chip_call`) blocks the self-call BEFORE the monomorph inline
    // re-enters, so lowering terminates with an error instead of looping.
    let src = "mod rec<T>(v: T) -> T { return rec(v) }\nin n: int\nin go: exec\non go { let a = rec(n) }\n";
    let r = compile(src); // must return (no hang / stack overflow)
    assert_code(&r.diagnostics, "WS020");
}

#[test]
fn generic_body_invalid_for_a_member_is_rejected_above_the_combo_cap() {
    // Combo-cap escape: two UNbounded params make
    // 11×11 = 121 mask combos, over MAX_BODY_CHECK_COMBOS (64). The old capped
    // fallback checked a single all-first-member combo (every param bound to
    // `bool`, the most permissive member for arithmetic), so a body op invalid
    // for other members slipped through with NO diagnostic and lowering then
    // monomorphized it into a broken gate. `a + b` is not a valid op for every
    // instantiation, so the decl must be rejected even above the cap.
    let src = "mod combine<A, B>(a: A, b: B) -> A {\n  var x: A = a\n  x = a + b\n  return x\n}\n\
               on CharacterSpawned() -> (ch) {\n  let e: entity = ch\n  let r = combine(e, e)\n}\n";
    let tc = typecheck(
        &parse(src, "test").ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert_code(&tc.diagnostics, "WS004");
    // Sanity (no over-rejection): a body that is valid for EVERY instantiation
    // must still type-check clean above the cap. `first<A, B>` also has 121
    // combos (> cap) but its body only returns `a`, so no combo can fail.
    let ok = "mod first<A, B>(a: A, b: B) -> A {\n  return a\n}\n\
              on CharacterSpawned() -> (ch) {\n  let r = first(ch, 1)\n}\n";
    let tco = typecheck(
        &parse(ok, "test").ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert!(
        crate::test_support::error_codes(&tco.diagnostics).is_empty(),
        "a generic with an always-valid body must type-check clean above the cap, got {:?}",
        tco.diagnostics
    );
}

#[test]
fn generic_body_operator_and_if_use_call_monomorph_variant() {
    // Regression: operators (`v * v`) and if-exprs
    // (`if c then a else b`) inside a generic body must emit the CALL's
    // monomorph variant — not the last mask-member the per-mask-member body
    // check leaves in `op_resolutions` / `type_of_expr` (Numeric -> Color,
    // an unbounded `T` -> Prefab). `sq<int>` must be an int MathMultiply,
    // `sq<vector>` a vector one; `pick<int>` an int Select — never Color/Prefab.
    let src = "mod sq<T: Numeric>(v: T) -> T { return v * v }\n\
               mod pick<T>(c: bool, a: T, b: T) -> T { return if c then a else b }\n\
               in go: exec\nin n: int\nin vec: vector\nin flag: bool\n\
               on go {\n  let a = sq(n)\n  let b = sq(vec)\n  let d = pick(flag, n, n)\n}\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));

    // The two MathMultiply monomorphs: one int, one vector — never Color.
    let muls = sorted(gate_output_types(&r.module, "MathMultiply"));
    assert_eq!(
        muls,
        vec![Type::Int, Type::Vector],
        "MathMultiply monomorphs must be int + vector, got {muls:?}"
    );

    // The if-expr Select for `pick<int>` must be int-typed — never the stale
    // last-mask-member the per-combo body check leaves behind for an unbounded
    // `T`. `pick` is only called at int here, so EVERY Select must be int.
    let sels = gate_output_types(&r.module, "Select");
    assert!(
        !sels.is_empty() && sels.iter().all(|t| *t == Type::Int),
        "pick<int> Select must be int-typed (not a stale default), got {sels:?}"
    );
}

/// Every distinct microchip `template_key` in the WHOLE module tree, resolved
/// to a string. Two instances collapse to one emitted grid iff they share a key,
/// so the count is the number of distinct emitted grids, nested chips included.
fn chip_template_keys(module: &Module) -> crate::collections::HashSet<String> {
    module
        .modules_deep()
        .into_iter()
        .filter_map(|m| m.template_key)
        .map(|k| crate::intern::resolve(k).to_string())
        .collect()
}

/// Data-carrying (non-Exec) rerouter port types across the whole module tree —
/// the MicrochipInput/Output boundary ports of every chip instance. Reads out
/// each monomorph's boundary typing (the `_exec_in`/`_exec_out` rerouters are
/// Exec-typed and excluded).
fn rerouter_data_types(module: &Module) -> Vec<Type> {
    module
        .nodes_deep()
        .filter(|n| n.gate_class == gc::MICROCHIP_INPUT || n.gate_class == gc::MICROCHIP_OUTPUT)
        .flat_map(|n| n.ports.inputs.iter().chain(n.ports.outputs.iter()))
        .filter(|p| p.ty != Type::Exec)
        .map(|p| p.ty.clone())
        .collect()
}

#[test]
fn generic_chip_monomorphizes_per_instantiation() {
    // A generic chip (a physical microchip, not an inline mod) instantiated at
    // two types emits two distinct microchip templates, one per type, never
    // one shared grid that cross-wires them. It type-checks clean: `compile_ok`
    // merges typecheck's errors too.
    let src = "chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let a = Boxed(n)\n  let b = Boxed(vec)\n}\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for a monomorphized generic chip");
    assert!(
        !any_param_port(&r.module),
        "no port may still carry Type::Param after chip monomorphization"
    );

    // Two distinct child-module template keys: the int + vector instances did
    // NOT collapse onto one shared template / emitted grid.
    let keys = chip_template_keys(&r.module);
    assert_eq!(
        keys.len(),
        2,
        "int + vector instances must be two distinct templates, got {keys:?}"
    );

    // Each monomorph's `stored` gate is concrete int / vector — proof the body
    // came out per-type, not shared.
    let vals = sorted(pseudo_var_value_types(&r.module));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "the two `stored` gates must be concrete int + vector, got {vals:?}"
    );

    // Boundary ports monomorphize too: the `v`/`r` rerouters are int in one
    // instance and vector in the other — never `Any` (the pre-seed default) or
    // `Type::Param`.
    let rer = rerouter_data_types(&r.module);
    assert!(
        rer.iter().any(|t| *t == Type::Int) && rer.iter().any(|t| *t == Type::Vector),
        "boundary ports must include both int and vector, got {rer:?}"
    );
    assert!(
        !rer.iter().any(|t| *t == Type::Any),
        "no boundary port may stay `Any` after monomorphization, got {rer:?}"
    );
}

#[test]
fn generic_flat_chip_monomorphizes() {
    // A `@flat` program flattens every chip body onto one grid AFTER lowering,
    // so the two instances are built (monomorphized) on the instance path and
    // then inlined. The per-type storage must survive: int + vector, no leaked
    // Param, and emit clean.
    let src = "@flat\n\n\
               chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let a = Boxed(n)\n  let b = Boxed(vec)\n}\n";
    check_monomorphs(&[(
        "a flat generic chip",
        src.into(),
        vec![Type::Int, Type::Vector],
        None,
    )]);
    let r = compile_ok(src);
    assert!(
        r.module.chips.is_empty(),
        "@flat must leave no child module: {:?}",
        r.module.chips.len()
    );
}

#[test]
fn nested_generic_chip_monomorphizes_to_two_grids() {
    // A generic chip instantiated INSIDE another (non-generic) chip's body must
    // still monomorphize per type: the two `Boxed` grids live nested under
    // `Outer`'s child module, so this only holds if the monomorph keying (and
    // the recursive `chip_template_keys` walk) sees through the nesting.
    // `out b = y`, not `y.r`: a single-output chip hands back the output's
    // value itself, so `y` IS the vector and `y.r` reads a field a `vector`
    // does not have. (`x.r` passes only because an `int` has no fields to
    // check the name against.)
    let src = "chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
               chip Outer(n: int, v3: vector) -> (a: int, b: vector) {\n\
               let x = Boxed(n)\n  let y = Boxed(v3)\n  out a = x\n  out b = y\n}\n\
               in go: exec\nin n: int\nin v3: vector\n\
               on go {\n  let o = Outer(n, v3)\n}\n";
    check_monomorphs(&[(
        "nested Boxed<int> + Boxed<vector>",
        src.into(),
        vec![Type::Int, Type::Vector],
        None,
    )]);
    // Distinct keys across the whole tree: `Outer` + the two `Boxed` monomorphs.
    let keys = chip_template_keys(&compile_ok(src).module);
    let boxed_keys: Vec<_> = keys.iter().filter(|k| k.starts_with("Boxed<")).collect();
    assert_eq!(
        boxed_keys.len(),
        2,
        "nested Boxed<int> + Boxed<vector> must be two distinct grids, got {keys:?}"
    );
}

// ---------- `self`-receiver (UFCS) desugar ----------

#[test]
fn self_receiver_method_desugars_to_mod_call() {
    // `a.dist(b)` on a user `self`-mod desugars to `dist(a, b)`: the inlined
    // body's `self.Dot(o)` lowers to a real VecDotProduct gate — NOT the
    // `_Unsupported` placeholder the pre-feature dangling-method path emitted.
    let src = "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
               in a: vector\nin b: vector\nin go: exec\n\
               on go { let d = a.dist(b) }\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for a resolved self-mod method call");
    assert!(
        !gate_output_types(&r.module, "VecDotProduct").is_empty(),
        "the inlined self-mod body must emit a real VecDotProduct gate"
    );
    assert!(
        gate_output_types(&r.module, "Unsupported").is_empty(),
        "a resolved self-mod method call must not emit an _Unsupported placeholder"
    );
}

#[test]
fn non_self_mod_method_call_is_a_typecheck_error() {
    // `f`'s first param is not `self`, so `v.f(w)` is not a method call. It is a
    // hard typecheck error (WS036), and lowering must NOT inline it as a method
    // (no VecDotProduct from f's body).
    let src = "mod f(a: vector, o: vector) -> float { return a.Dot(o) }\n\
               in v: vector\nin w: vector\nin go: exec\n\
               on go { let d = v.f(w) }\n";
    let tc = typecheck(
        &parse(src, "test").ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert_code(&tc.diagnostics, "WS036");
    let r = compile(src);
    assert!(
        gate_output_types(&r.module, "VecDotProduct").is_empty(),
        "a non-self mod must not be inlined as a method call"
    );
}

// ---------- container-var receiver self-mods ----------

#[test]
fn container_array_self_receiver_desugars_to_mod_call() {
    // A self-mod whose receiver is an array VAR must NOT be hijacked by the
    // array-method lowering branch. Before the `is_array_method` gate was added
    // there, `arr.firstOr(9)` (firstOr NOT an array method) routed into
    // `lower_array_method` -> `_Unsupported`, diverging from typecheck. It must
    // desugar to `firstOr(arr, 9)` and inline the body's real gates.
    let method = "mod firstOr(self: int[], fallback: int) -> int { let n = self.length()\n return n + fallback }\n\
                  var arr: int[] = [1, 2, 3]\nvar rr: int = 0\nin go: exec\non go { rr = arr.firstOr(9) }\n";
    let plain = "mod firstOr(self: int[], fallback: int) -> int { let n = self.length()\n return n + fallback }\n\
                 var arr: int[] = [1, 2, 3]\nvar rr: int = 0\nin go: exec\non go { rr = firstOr(arr, 9) }\n";
    let rm = compile_ok(method);
    emit_ok(&rm).expect("emit must succeed for an array-var self-mod method call");
    assert!(
        gate_output_types(&rm.module, "Unsupported").is_empty(),
        "an array-var self-mod method call must not lower to _Unsupported"
    );
    assert!(
        !gate_output_types(&rm.module, "ArrayVar_GetLength").is_empty(),
        "the inlined body must emit a real ArrayVar_GetLength gate"
    );
    assert!(
        !gate_output_types(&rm.module, "MathAdd").is_empty(),
        "the inlined body must emit a real MathAdd gate"
    );
    // Identical body-gate profile to the plain call.
    let rp = compile(plain);
    assert_eq!(
        gate_output_types(&rm.module, "ArrayVar_GetLength").len(),
        gate_output_types(&rp.module, "ArrayVar_GetLength").len(),
        "method-call desugar must match the plain call's gate profile"
    );
}

#[test]
fn container_map_self_receiver_desugars_to_mod_call() {
    // The map counterpart: a self-mod on a map VAR must not be hijacked by the
    // map-method branch. `m.bump(5)` desugars to `bump(m, 5)` and inlines the
    // body. (The body avoids calling a map method on `self` — map PARAMETERS
    // aren't method-callable inside a mod body, a separate pre-existing gap
    // unrelated to receiver dispatch.)
    let src = "mod bump(self: Map<string, int>, d: int) -> int { return d + 1 }\n\
               var m: Map<string, int>\nvar rr: int = 0\nin go: exec\non go { rr = m.bump(5) }\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for a map-var self-mod method call");
    assert!(
        gate_output_types(&r.module, "Unsupported").is_empty(),
        "a map-var self-mod method call must not lower to _Unsupported"
    );
    assert!(
        !gate_output_types(&r.module, "MathAdd").is_empty(),
        "the inlined body must emit a real MathAdd gate"
    );
}

#[test]
fn generic_container_self_receiver_monomorphizes() {
    // A generic `self: T[]` receiver: `T` is pinned by the array's element
    // type. Called on int[] and vector[], each inline emits a real GetLength
    // (no _Unsupported, no leaked Param).
    let src = "mod lenOf<T>(self: T[]) -> int { return self.length() }\n\
               var ai: int[] = [1, 2]\nvar av: vector[] = []\n\
               var r1: int = 0\nvar r2: int = 0\nin go: exec\n\
               on go {\n  r1 = ai.lenOf()\n  r2 = av.lenOf()\n}\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for a generic container self-mod");
    assert!(!any_param_port(&r.module), "no Type::Param may survive");
    assert!(
        gate_output_types(&r.module, "Unsupported").is_empty(),
        "a generic container self-mod must not lower to _Unsupported"
    );
    assert_eq!(
        count_class(&r.module, gc::ARRAY_GET_LENGTH),
        2,
        "each monomorph must inline its own GetLength gate"
    );
}

#[test]
fn explicit_type_args_error_cases() {
    // out-of-mask: string isn't in Numeric
    assert!(
        errs("mod z<T: Numeric>() -> T { static var v: T\n return v }\nout r: int = z<string>()\n")
            .contains(&"WS033".to_string())
    );
    // arity mismatch
    assert!(
        errs("mod p<T>(a: T) -> T { return a }\nin x: int\nout r: int = p<int, float>(x)\n")
            .contains(&"WS033".to_string())
    );
}

#[test]
fn type_args_do_not_break_comparison_parsing() {
    // The `<...>(` type-argument form must NOT hijack a `<`/`>` comparison. A
    // plain `a < b` and a `(a < b)` chain still parse + typecheck as comparisons.
    let ok = "in a: int\nin b: int\nin c: int\nout r: bool = (a < b)\nout s: bool = a < b\n";
    let d = crate::typecheck::typecheck(
        &crate::parser::parse(ok, "t").ast,
        "t",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert!(
        d.diagnostics
            .iter()
            .all(|x| x.severity != crate::diagnostic::Severity::Error),
        "comparison must still parse+check clean: {:?}",
        d.diagnostics
    );
}

#[test]
fn non_callable_call_errors_ws038() {
    // Calling a non-callable value (a var / let / array / input) is a hard
    // error, not a silent `_Unsupported` gate reading 0. An index typo is the
    // common trigger.
    let typo = "on CharacterSpawned() -> (ch) {\n  var xs: int[] = [1, 2, 3]\n  let r = xs(0)\n}\n";
    assert!(
        errs(typo).contains(&"WS038".to_string()),
        "xs(i) must be WS038: {:?}",
        errs(typo)
    );
    // `a < b > (c)` parses as an explicit-type-argument call on `a`; since `a`
    // is not callable it now surfaces as a clear WS038 instead of a silently
    // dropped comparison. (`a < b` / `(a < b)` without a trailing `(` stay
    // comparisons — see `type_args_do_not_break_comparison_parsing`.)
    let misparse = "on CharacterSpawned() -> (ch) {\n  let a = 1\n  let b = 2\n  let c = 3\n  let r = a < b > (c)\n}\n";
    assert!(
        errs(misparse).contains(&"WS038".to_string()),
        "a<b>(c) must surface as WS038: {:?}",
        errs(misparse)
    );
    // A real callable is unaffected.
    let ok = "mod dbl(n: int) -> int { return n * 2 }\nin x: int\nstatic var rv: int = 0\nout r: int = rv\nin go: exec\non go { rv = dbl(x) }\n";
    assert!(
        errs(ok).is_empty(),
        "a real mod call must be clean: {:?}",
        errs(ok)
    );
}

#[test]
fn builtins_and_types_program_compiles() {
    // The `examples/builtins_types_test.ws` self-checking program is also a
    // compile-time type test: it exercises math builtins, coercion, native
    // string equality, vector receiver methods, generics (inference + explicit
    // type arguments),
    // and `self`-receiver dispatch together. It only type-checks + emits if every
    // builtin's argument/result types line up and each receiver binds a
    // compatible `self` — so this guards the whole type surface end to end.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/builtins_types_test.ws"
    );
    let src = std::fs::read_to_string(path).expect("read builtins_types_test.ws");
    let r = compile_ok(&src);
    emit_ok(&r).expect("builtins/types test program must emit");
}

#[test]
fn receiver_call_validates_receiver_type() {
    // A receiver whose type doesn't match `self` is a WS003 error — user mod/chip
    // calls coerce each arg (incl. the receiver bound as arg 0) against its
    // parameter, exactly like the wire layer's PortsAreCompatible.
    let bad = "mod lensq(self: vector, k: float) -> float { return self.Dot(self) * k }\n\
               in x: int\nstatic var rv: float = 0.0\nout r: float = rv\nin go: exec\non go { rv = x.lensq(2.0) }\n";
    assert!(
        errs(bad).contains(&"WS003".to_string()),
        "int receiver on self:vector must error: {:?}",
        errs(bad)
    );
    // The same mismatch on a plain (non-receiver) call is caught too.
    let bad_plain = "mod lensq(self: vector, k: float) -> float { return self.Dot(self) * k }\n\
                     in x: int\nstatic var rv: float = 0.0\nout r: float = rv\nin go: exec\non go { rv = lensq(x, 2.0) }\n";
    assert!(errs(bad_plain).contains(&"WS003".to_string()));
    // A receiver that WIDENS to `self`'s type (character -> entity) is accepted —
    // the coercion walks the applicable type options, it isn't an exact match.
    let widen = "mod who(self: entity) -> string { return self.GetDisplayName() }\n\
                 in c: character\nstatic var rv: string = \"\"\nout r: string = rv\nin go: exec\non go { rv = c.who() }\n";
    assert!(
        errs(widen).is_empty(),
        "character receiver on self:entity must widen cleanly: {:?}",
        errs(widen)
    );
}

#[test]
fn type_args_on_builtin_warn() {
    let r = crate::typecheck::typecheck(
        &crate::parser::parse(
            "in a: float\nin b: float\nout r: float = Blend<int>(a, b, 0.5)\n",
            "t",
        )
        .ast,
        "t",
        &crate::typecheck::CeSlotMap::default(),
    );
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == "WS037" && d.severity == crate::diagnostic::Severity::Warning),
        "explicit type args on a builtin must warn WS037: {:?}",
        r.diagnostics
    );
    // A plain builtin call does NOT warn.
    let r2 = crate::typecheck::typecheck(
        &crate::parser::parse(
            "in a: float\nin b: float\nout r: float = Blend(a, b, 0.5)\n",
            "t",
        )
        .ast,
        "t",
        &crate::typecheck::CeSlotMap::default(),
    );
    crate::test_support::assert_no_code(&r2.diagnostics, "WS037");
}

// ---- max generic parameters / cartesian body-check cap
// (`MAX_BODY_CHECK_COMBOS = 64` in typecheck.rs). A bounded generic's body
// is checked once per member of the cartesian product of its type params'
// masks; over the cap the check falls back to a representative combo (each
// mask's first member) with every param individually varied across its
// whole mask (see the `capped` branch in typecheck.rs). These confirm the
// cap boundary (64), the truncation path just past it (128), and a wildly
// over-cap unbounded case all compile without hanging or panicking — and
// that truncation is still SOUND (still catches a real per-param error). ----

#[test]
fn six_scalar_type_params_at_body_check_cap_compiles_clean() {
    // Scalar mask size 2, 6 params -> 2^6 = 64 combos, EXACTLY
    // MAX_BODY_CHECK_COMBOS: the full cartesian product still runs (no
    // truncation at the boundary). Body is trivially valid for every combo.
    let src = "mod combo6<A: Scalar, B: Scalar, C: Scalar, D: Scalar, E: Scalar, F: Scalar>(a: A, b: B, c: C, d: D, e: E, f: F) -> A { return a }\n\
               in go: exec\nin n: int\nin fl: float\n\
               on go { let r = combo6(n, fl, n, fl, n, fl) }\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for the 64-combo (at-cap) generic");
    assert!(
        !any_param_port(&r.module),
        "no Type::Param may survive monomorphization"
    );
}

#[test]
fn seven_scalar_type_params_over_cap_truncates_without_error() {
    // 2^7 = 128 combos, over the 64 cap — the truncation (representative +
    // per-param variation) path kicks in. Body is valid for every member of
    // every param individually, so truncation must not manufacture a
    // spurious error, and checking must terminate promptly (no hang).
    let src = "mod combo7<A: Scalar, B: Scalar, C: Scalar, D: Scalar, E: Scalar, F: Scalar, G: Scalar>(a: A, b: B, c: C, d: D, e: E, f: F, g: G) -> A { return a }\n\
               in go: exec\nin n: int\nin fl: float\n\
               on go { let r = combo7(n, fl, n, fl, n, fl, n) }\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for the truncated (128-combo) generic");
    assert!(!any_param_port(&r.module));
}

#[test]
fn three_unbounded_type_params_variant_mask_truncates_without_error() {
    // Unbounded <A, B, C> = the full Variant mask (11 members) each -> 11^3
    // = 1331 combos, far over the cap — the same truncation path, at a much
    // larger multiple. Body only returns `a`, valid for every combination.
    let src = "mod triple<A, B, C>(a: A, b: B, c: C) -> A { return a }\n\
               in go: exec\nin n: int\nin s: string\nin v: vector\n\
               on go { let r = triple(n, s, v) }\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for the unbounded 3-param generic");
    assert!(!any_param_port(&r.module));
}

#[test]
fn truncated_combo_check_still_catches_single_param_op_error() {
    // 3 `Numeric` params (mask size 6) -> 6^3 = 216 combos, over the cap —
    // the SAME truncation path as the tests above, but here the body uses
    // an op ('&', no rule for vector/rotator/quat/color) on ONLY param A.
    // Truncation varies each param across its whole mask while the others
    // hold their first member (int), so this must still be caught:
    // truncation is a coverage optimization, not a soundness hole.
    let src = "mod combo3<A: Numeric, B: Numeric, C: Numeric>(a: A, b: B, c: C) -> A { let x = a & a\n return a }\n\
               in go: exec\nin n: int\non go { let r = combo3(n, n, n) }\n";
    let tc = typecheck(
        &parse(src, "test").ast,
        "test",
        &crate::typecheck::CeSlotMap::default(),
    );
    // A single-param-only op invalid for some Numeric member is caught even
    // when the combo check is truncated.
    assert_code(&tc.diagnostics, "WS011");
}

/// Unary negate inside a generic mod (and one generic mod calling another over a
/// negated value) monomorphizes to a real MathNegate per concrete type. The
/// operator table keys negate as `-u` but the AST spelling is `-`; the
/// monomorphizer resolved the raw `-` and previously found no rule, falling back
/// to a stale type instead of emitting the gate.
#[test]
fn generic_negate_monomorphizes() {
    let src = "mod neg<T: int | float>(x: T) -> T { return -x }\n\
               mod useit<T: int | float>(x: T) -> T { let n = neg(x)\n return n }\n\
               in gi: int\nin gf: float\nout ri = useit(gi)\nout rf = useit(gf)\n";
    let r = compile_ok(src);
    assert!(
        !has_unsupported(&r.module),
        "negate must resolve, not placeholder"
    );
    // int and float monomorphs each emit one negate.
    assert_eq!(count_class(&r.module, gc::MATH_NEGATE), 2);
}

/// Generic PASSTHROUGH BUILTINS (`Sleep`/`SleepTicks`/`Select`/`Swap`, declared
/// in `catalog/calls/flow.rs`) are a second monomorphization surface, separate
/// from user `mod<T>` calls: their `T`-typed ports were left as the CallSpec's
/// bare `Type::Param("T")` when the argument was a runtime value rather than an
/// inlined constant (`lower/call/builtin.rs`'s `ports.inputs`/`ports.outputs`
/// build used the declared type verbatim). `await SleepTicks(_, delay = N)` -
/// the documented anonymous-placeholder await form, `_` standing for the armed
/// flag - is the case that surfaced it: `_` binds the generic `input` port to a
/// `bool`, and that `Type::Param` reached `var_type_to_wire_value` and tripped
/// its debug_assert (a release build silently defaulted the field to
/// `WireValue::Number(0.0)` instead).
#[test]
fn await_sleep_ticks_placeholder_monomorphizes_input_and_output_to_bool() {
    let src = "in start: exec\non start {\n  await SleepTicks(_, delay = 10)\n}\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed for `await SleepTicks(_, ...)`");
    assert!(
        !any_param_port(&r.module),
        "no port may still carry Type::Param after monomorphization"
    );

    // The armed flag `_` binds is a `bool` (typecheck/stmt.rs declares it
    // `Type::Bool` for the await's exec expression), so the BufferTicks gate's
    // generic `Input`/`Output` ports must both monomorphize to `Type::Bool` -
    // not fall back to `Any` (which would silently re-default to `Number(0.0)`
    // at emit, the same wrong answer the leaked `Param` produced).
    let buf = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == crate::ir::gate_class::BUFFER_TICKS)
        .expect("a BufferTicks gate for SleepTicks");
    let value_ports: Vec<&Type> = buf
        .ports
        .inputs
        .iter()
        .chain(buf.ports.outputs.iter())
        .filter(|p| {
            let name = crate::intern::resolve(p.name);
            name == "Input" || name == "Output"
        })
        .map(|p| &p.ty)
        .collect();
    assert!(
        !value_ports.is_empty(),
        "BufferTicks must have Input/Output ports"
    );
    assert!(
        value_ports.iter().all(|t| **t == Type::Bool),
        "SleepTicks(_, ...)'s Input/Output ports must monomorphize to bool, got {value_ports:?}"
    );
}

/// Per chip instance: its template key, its data boundary-port types, and its
/// `Pseudo_Var` value types, each sorted. Reads out what one instance's body
/// was actually built for.
fn chip_instance_types(module: &Module) -> Vec<(String, Vec<Type>, Vec<Type>)> {
    let mut out: Vec<(String, Vec<Type>, Vec<Type>)> = module
        .modules_deep()
        .into_iter()
        .skip(1)
        .map(|c| {
            let key = c
                .template_key
                .map(|k| crate::intern::resolve(k).to_string())
                .unwrap_or_default();
            let rer = c
                .nodes
                .values()
                .filter(|n| {
                    n.gate_class == gc::MICROCHIP_INPUT || n.gate_class == gc::MICROCHIP_OUTPUT
                })
                .flat_map(|n| n.ports.inputs.iter().chain(n.ports.outputs.iter()))
                .filter(|p| p.ty != Type::Exec)
                .map(|p| p.ty.clone())
                .collect();
            (key, sorted(rer), sorted(var_value_types(c.nodes.values())))
        })
        .collect();
    out.sort_by_key(|(_, rer, _)| format!("{rer:?}"));
    out
}

#[test]
fn chip_in_generic_mod_keys_apart_per_outer_instantiation() {
    // A NON-generic chip declared inside a generic mod's body, whose pins and
    // storage are typed by the mod's `T`. Each outer instantiation lowers the
    // chip body under a different `T`, so the two instances must be two
    // templates, each typed for its own `T`. Keyed on the chip's own
    // (empty) substitution alone, the `string` call reused the body built for
    // `int`.
    let src = "in go: exec\nin n: int\nin s: string\n\
               mod outer<T: int | string>(v: T) -> (r: T) {\n\
                 chip inner(x: T) -> (y: T) {\n\
                   var stored: T = x\n\
                   out y = stored\n\
                 }\n\
                 let got = inner(v)\n\
                 return got\n\
               }\n\
               out a = outer(n)\nout b = outer(s)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    let keys = chip_template_keys(&r.module);
    let inst = chip_instance_types(&r.module);
    assert_eq!(
        keys.len(),
        2,
        "the int + string instantiations must be two templates, got {keys:?} / {inst:?}"
    );
    assert_eq!(
        inst.len(),
        2,
        "two `inner` instances expected, got {inst:?}"
    );
    let types: Vec<(Vec<Type>, Vec<Type>)> = inst
        .iter()
        .map(|(_, rer, vars)| (rer.clone(), vars.clone()))
        .collect();
    assert_eq!(
        types,
        vec![
            (vec![Type::Int; 4], vec![Type::Int]),
            (vec![Type::String; 4], vec![Type::String]),
        ],
        "each instance must be typed for its own `T`, got {inst:?}"
    );
}

#[test]
fn top_level_non_generic_chip_still_shares_one_template() {
    // A top-level non-generic chip has an empty resolution key, so two calls
    // share one template exactly as before.
    let src = "chip inner(x: int) -> (y: int) {\n\
                 var stored: int = x\n\
                 out y = stored\n\
               }\n\
               in n: int\nin m: int\n\
               out a = inner(n)\nout b = inner(m)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    let keys = chip_template_keys(&r.module);
    assert_eq!(
        keys.len(),
        1,
        "two calls to a top-level chip must share one template, got {keys:?}"
    );
    let inst = chip_instance_types(&r.module);
    assert_eq!(
        inst.len(),
        2,
        "two `inner` instances expected, got {inst:?}"
    );
    // The key is exactly the pre-fix shape: no resolution-key segment.
    let key = keys.iter().next().unwrap();
    assert!(
        !key.contains("\u{1}r["),
        "a top-level chip's key must carry no resolution segment, got {key:?}"
    );
}

#[test]
fn generic_chip_in_generic_mod_sees_outer_and_own_params() {
    // A generic chip declared inside a generic mod resolves both the outer `T`
    // (from the enclosing instantiation) and its own `U` (from this call).
    let src = "in n: int\nin s: string\nin f: float\n\
               mod outer<T: int | string>(v: T, w: float) -> (r: T) {\n\
                 chip inner<U>(x: T, u: U) -> (y: T, z: U) {\n\
                   out y = x\n\
                   out z = u\n\
                 }\n\
                 let (got, _) = inner(v, w)\n\
                 return got\n\
               }\n\
               out a = outer(n, f)\nout b = outer(s, f)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    let keys = chip_template_keys(&r.module);
    let inst = chip_instance_types(&r.module);
    assert_eq!(
        keys.len(),
        2,
        "one template per outer instantiation, got {keys:?} / {inst:?}"
    );
    let rer: Vec<Vec<Type>> = inst.iter().map(|(_, rer, _)| rer.clone()).collect();
    let mut int_inst = vec![Type::Float; 4];
    int_inst.extend(vec![Type::Int; 4]);
    let mut string_inst = vec![Type::Float; 4];
    string_inst.extend(vec![Type::String; 4]);
    assert_eq!(
        rer,
        vec![int_inst, string_inst],
        "pins must carry outer `T` and own `U`, got {inst:?}"
    );
}

/// The instance types of `chip_instance_types` with the keys dropped, each
/// instance's lists already sorted and the instances sorted.
fn instance_type_sets(module: &Module) -> Vec<(Vec<Type>, Vec<Type>)> {
    chip_instance_types(module)
        .into_iter()
        .map(|(_, rer, vars)| (rer, vars))
        .collect()
}

/// The `ArrayVar` storage types across the whole module tree, sorted.
fn array_var_types(module: &Module) -> Vec<Type> {
    sorted(
        module
            .nodes_deep()
            .filter(|n| n.gate_class == gc::ARRAY_VAR)
            .flat_map(|n| n.ports.outputs.iter().map(|p| p.ty.clone()))
            .collect(),
    )
}

#[test]
fn two_level_generic_nesting_types_chip_per_instantiation() {
    // `c` is declared two generic bodies deep: it uses `A`'s `T` and `B`'s `U`.
    // Each `A` instantiation must type both, not only the innermost `U`.
    let src = "in n: int\nin s: string\nin f: float\nin g: bool\n\
               mod A<T: int | string, V: float | bool>(v: T, w: V) -> (r: T) {\n\
                 mod B<U: float | bool>(t: T, u: U) -> (r2: T) {\n\
                   chip c(x: T, z: U) -> (y: T, q: U) {\n\
                     var st: T = x\n\
                     out y = st\n\
                     out q = z\n\
                   }\n\
                   let (gg, _) = c(t, u)\n\
                   return gg\n\
                 }\n\
                 return B(v, w)\n\
               }\n\
               out a = A(n, f)\nout b = A(s, g)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    let keys = chip_template_keys(&r.module);
    assert_eq!(
        keys.len(),
        2,
        "one template per `A` instantiation, got {keys:?}"
    );
    let mut int_float = vec![Type::Float; 4];
    int_float.extend(vec![Type::Int; 4]);
    let mut string_bool = vec![Type::Bool; 4];
    string_bool.extend(vec![Type::String; 4]);
    assert_eq!(
        instance_type_sets(&r.module),
        vec![
            (string_bool, vec![Type::String]),
            (int_float, vec![Type::Int])
        ],
        "`c` must carry the outer `T` and the middle `U` per instantiation"
    );
}

#[test]
fn own_type_param_shadows_the_enclosing_one() {
    // `inner<T>` inside `outer<T>`: the inner `T` is bound by the call
    // (`float`), not by the outer instantiation, for a chip and an inline mod.
    let src = "in n: int\nin s: string\nin f: float\n\
               mod outer<T: int | string>(v: T, w: float) -> (r: T) {\n\
                 chip inner<T>(x: T) -> (y: T) {\n\
                   var st: T = x\n\
                   out y = st\n\
                 }\n\
                 mod twin<T>(x: T) -> (y: T) {\n\
                   var mt: T = x\n\
                   return mt\n\
                 }\n\
                 let q = inner(w)\n\
                 let p = twin(w)\n\
                 return v\n\
               }\n\
               out a = outer(n, f)\nout b = outer(s, f)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    let inst = instance_type_sets(&r.module);
    assert_eq!(
        inst,
        vec![(vec![Type::Float; 4], vec![Type::Float]); 2],
        "the chip's own `T` must win over the outer one"
    );
    assert_eq!(
        pseudo_var_value_types(&r.module)
            .iter()
            .filter(|t| **t != Type::Float)
            .count(),
        0,
        "the inline mod's own `T` must win over the outer one too"
    );
}

#[test]
fn array_push_in_nested_chip_pins_element_type_per_instantiation() {
    // A `T[]` in a chip declared inside `B` inside `outer`: the element type
    // comes from `outer`'s `T`, per instantiation.
    let src = "in go: exec\nin n: int\nin s: string\n\
               mod outer<T: int | string>(v: T) -> (r: int) {\n\
                 mod B<U: float | bool>(t: T, u: U) -> (r2: int) {\n\
                   chip inner(x: T) -> (len: int) {\n\
                     var xs: T[]\n\
                     xs.push(x)\n\
                     emit len = xs.length()\n\
                   }\n\
                   let q = inner(t, exec = go)\n\
                   return q.len\n\
                 }\n\
                 return B(v, 1.0)\n\
               }\n\
               out a = outer(n)\nout b = outer(s)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    assert_eq!(chip_template_keys(&r.module).len(), 2);
    assert_eq!(
        array_var_types(&r.module),
        vec![
            Type::Ref(Box::new(Type::Array(Box::new(Type::Int)))),
            Type::Ref(Box::new(Type::Array(Box::new(Type::String)))),
        ],
        "each instance's array must hold its own `T`"
    );
}

#[test]
fn chip_in_non_generic_mod_ignores_a_generic_callers_frame() {
    // `pick` is declared in a non-generic top-level `helper`, so its key is
    // empty and every call shares one template. Inlining `helper` from a
    // generic caller must not hand `pick` that caller's frame: its pin types
    // are the same whichever call builds the shared template first.
    let body = "in n: int\nin s: string\n\
                mod helper(x: int) -> (r: int) {\n\
                  chip pick(d: bool, x: int) -> (y: int) {\n\
                    out y = if d then x else 0\n\
                  }\n\
                  return pick(true, x)\n\
                }\n\
                mod outer<T: int | string>(v: T, x: int) -> (r: int) {\n\
                  return helper(x)\n\
                }\n";
    let direct_first =
        format!("{body}out a = helper(n)\nout b = outer(s, n)\nout c = outer(n, n)\n");
    let generic_first =
        format!("{body}out b = outer(s, n)\nout c = outer(n, n)\nout a = helper(n)\n");
    let r1 = compile(&direct_first);
    let r2 = compile(&generic_first);
    assert_no_errors(&r1);
    assert_no_errors(&r2);
    assert_eq!(chip_template_keys(&r1.module).len(), 1);
    assert_eq!(chip_template_keys(&r2.module).len(), 1);
    assert_eq!(
        instance_type_sets(&r1.module),
        instance_type_sets(&r2.module),
        "the shared template's pins must not depend on call order"
    );
}

/// The `Pseudo_Var` value types, sorted, with the caller's own storage
/// (`var n: string`, `var k: int`, ...) left in.
fn sorted_var_types(module: &Module) -> Vec<Type> {
    sorted(pseudo_var_value_types(module))
}

#[test]
fn type_param_inferred_from_a_nested_array_arg_binds() {
    let src = "var rows: string[][]\nvar r0: string[]\nvar n: string = \"\"\n\
               mod f<A: string | int>(xs: A[][]) -> A {\n\
                 var t: A = xs[0][0]\n\
                 return t\n\
               }\n\
               in go: exec\n\
               on go {\n  rows.push(r0)\n  n = f(rows)\n}\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    assert_eq!(
        sorted_var_types(&r.module),
        vec![Type::String, Type::String]
    );
    assert!(
        !gate_output_types(&r.module, "ArrayVar_Get").contains(&Type::Int),
        "both element reads must be string-typed, got {:?}",
        gate_output_types(&r.module, "ArrayVar_Get")
    );
}

#[test]
fn generic_chip_type_param_inferred_only_from_an_array_arg_binds() {
    let src = "var ss: string[]\nvar is: int[]\nin go: exec\n\
               chip C<A: string | int>(xs: A[]) -> (o: A) {\n\
                 var t: A\n\
                 on go { t = xs[0] }\n\
                 out o = t\n\
               }\n\
               out a: string = C(ss)\nout b: int = C(is)\n";
    let r = compile_ok(src);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let arr = |t: Type| Type::Ref(Box::new(Type::Array(Box::new(t))));
    assert_eq!(
        instance_type_sets(&r.module),
        vec![
            (
                vec![Type::Int, Type::Int, arr(Type::Int), arr(Type::Int)],
                vec![Type::Int]
            ),
            (
                vec![
                    arr(Type::String),
                    arr(Type::String),
                    Type::String,
                    Type::String
                ],
                vec![Type::String]
            ),
        ]
    );
}

#[test]
fn capped_generic_at_an_unchecked_combination_types_its_element_reads() {
    // Seven `Scalar` params exceed the body-check cap, so typecheck checks the
    // all-`int` combination and each single-param variation only. Both calls
    // pick an unchecked combination, and `r[0]`'s receiver is a `let`, so the
    // element type must come from the instantiation, not from the last
    // combination checked.
    assert_capped_element_reads("let r = rows[0]\n return r[0]");
}

#[test]
fn capped_generic_at_an_unchecked_combination_types_a_nested_element_read() {
    // `rows[0][0]`: the receiver `rows[0]` has no binding of its own.
    assert_capped_element_reads("return rows[0][0]");
}

/// Lowers `body` as a seven-`Scalar`-param generic at two unchecked
/// combinations, one with `A = float` and one with `A = int`, and checks
/// every element read is typed for its own `A`.
fn assert_capped_element_reads(body: &str) {
    let src = format!(
        "mod f<A: Scalar, B: Scalar, C: Scalar, D: Scalar, E: Scalar, F: Scalar, G: Scalar>(\
           rows: A[][], b: B, c: C, d: D, e: E, f: F, g: G) -> A {{\n {body}\n}}\n\
         var fr: float[][]\nvar ir: int[][]\nin fl: float\nin n: int\n\
         var x: float = 0.0\nvar y: int = 0\n\
         in go: exec\n\
         on go {{\n  x = f(fr, fl, n, n, n, n, n)\n  y = f(ir, fl, fl, n, n, n, n)\n}}\n"
    );
    let r = compile_ok(&src);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let gets = sorted(
        gate_output_types(&r.module, "ArrayVar_Get")
            .into_iter()
            .filter(|t| *t != Type::Bool && *t != Type::Exec)
            .collect(),
    );
    assert_eq!(
        gets,
        vec![
            Type::Array(Box::new(Type::Float)),
            Type::Array(Box::new(Type::Int)),
            Type::Float,
            Type::Int,
        ],
        "each instantiation's reads must carry its own element type"
    );
}

const HUE_GENERIC: &str = "enum Hue { Red, Green, Blue }\nenum S { A(int), B }\n\
var c: Hue = Hue.Red\nvar d: Hue = Hue.Green\nvar n: int = 0\nvar k: int = 3\nvar hs: Hue[]\n\
var ks: int[]\nvar s: S = S.B\nvar s2: S = S.B\nin go: exec\n";

/// The class of the gate driving the `Value` of every `Var_Set` into the var
/// labelled `label` (a payload-free enum's tag slot by the enum's own name),
/// which must all agree; `_Literal` for an inlined constant. A value driven
/// from an exec pin fails.
fn stored_from(r: &LowerResult, label: &str) -> &'static str {
    let classes: Vec<&'static str> = stored_values(r, var_labelled(r, label))
        .into_iter()
        .map(|(class, _)| class)
        .collect();
    assert!(!classes.is_empty(), "a store into `{label}`");
    assert!(
        classes.iter().all(|c| *c == classes[0]),
        "`{label}` is stored from {classes:?}"
    );
    classes[0]
}

/// For every `Var_Set` into `var`: the class of the gate driving its `Value`
/// and that gate's node, or `_Literal` and `None` for an inlined constant.
/// A value driven from an exec pin (a stored exec chain) fails.
pub(super) fn stored_values(r: &LowerResult, var: NodeId) -> Vec<(&'static str, Option<NodeId>)> {
    let driver = |node, port| {
        r.module
            .wires
            .iter()
            .find(|w| w.target.node_id == node && w.target.port == port)
    };
    r.module
        .nodes
        .values()
        .filter(|n| {
            n.gate_class == gc::VAR_SET
                && driver(n.id, WirePort::VarRef).map(|w| w.source.node_id) == Some(var)
        })
        .map(|set| match driver(set.id, WirePort::Value) {
            Some(w) => {
                let from = &r.module.nodes[&w.source.node_id];
                let ty = from
                    .ports
                    .find_output(w.source.port.sym())
                    .map(|p| p.ty.clone());
                assert_ne!(
                    ty,
                    Some(Type::Exec),
                    "a value stored from the exec pin {:?} of {}",
                    w.source.port,
                    from.gate_class
                );
                (from.gate_class, Some(from.id))
            }
            None => {
                assert!(
                    set.properties.contains_key(&*sym::VALUE),
                    "the stored value is wired or inlined"
                );
                ("_Literal", None)
            }
        })
        .collect()
}

/// The label of the var `get` reads, when `get` is a `Var_Get`; a
/// payload-free enum's tag slot by the enum's own name.
pub(super) fn read_label(r: &LowerResult, get: NodeId) -> Option<String> {
    let var = r
        .module
        .wires
        .iter()
        .find(|w| w.target.node_id == get && w.target.port == WirePort::VarRef)?;
    match r.module.nodes[&var.source.node_id]
        .properties
        .get(&*sym::NAME_LABEL)
    {
        Some(crate::ir::Literal::String(s)) => {
            Some(s.strip_suffix(".__disc").unwrap_or(s).to_string())
        }
        _ => None,
    }
}

/// The var labelled `label`, or a payload-free enum's tag slot.
pub(super) fn var_labelled(r: &LowerResult, label: &str) -> NodeId {
    let tag = format!("{label}.__disc");
    r.module
        .nodes
        .values()
        .find(|n| {
            n.gate_class == gc::VAR
                && matches!(n.properties.get(&*sym::NAME_LABEL), Some(crate::ir::Literal::String(s)) if *s == label || *s == tag)
        })
        .unwrap_or_else(|| panic!("a var labelled `{label}`"))
        .id
}

/// What every store into the var labelled `target` reads: the label of the
/// var each `Var_Get` driving it reads.
pub(super) fn stored_reads(r: &LowerResult, target: &str) -> Vec<Option<String>> {
    let values = stored_values(r, var_labelled(r, target));
    assert!(!values.is_empty(), "a store into `{target}`");
    values
        .into_iter()
        .map(|(class, node)| {
            assert_eq!(class, gc::VAR_GET, "`{target}` is stored from a var read");
            read_label(r, node.expect("a wired value"))
        })
        .collect()
}

#[test]
fn a_generic_over_a_payload_free_enum_keeps_the_value() {
    for (body, decls, feed) in [
        (
            "mod f<T: Hue | int>(x: T) -> T { return x }\non go { d = f(c) }",
            &["d"][..],
            gc::VAR_GET,
        ),
        (
            "mod f<T: Hue | int>(x: T) -> T { var t: T = x\n return t }\non go { d = f(c)\n n = f(k) }",
            &["d", "n"],
            gc::VAR_GET,
        ),
        (
            "mod f<T: Hue | int>(x: T) -> T { return x }\non go { d = f<Hue>(c)\n n = f<int>(k) }",
            &["d", "n"],
            gc::VAR_GET,
        ),
        (
            "mod first<T: Hue | int>(xs: T[]) -> T { return xs[0] }\non go { d = first(hs) }",
            &["d"],
            gc::ARRAY_GET,
        ),
        (
            "mod first<T: Hue | int>(xs: T[]) -> T { return xs[0] }\non go { d = first(hs)\n n = first(ks) }",
            &["d", "n"],
            gc::ARRAY_GET,
        ),
        (
            "mod first(xs: Hue[]) -> Hue { return xs[k] }\non go { d = first(hs) }",
            &["d"],
            gc::ARRAY_GET,
        ),
        (
            "mod f<T: S | int>(x: T) -> T { return x }\non go { s2 = f(s) }",
            &["s2"],
            gc::VAR_GET,
        ),
    ] {
        let src = format!("{HUE_GENERIC}{body}\n");
        let r = compile_ok(&src);
        assert!(!has_unsupported(&r.module), "{body}: {:?}", r.diagnostics);
        emit_ok(&r).expect("emit must succeed");
        for decl in decls {
            assert_eq!(stored_from(&r, decl), feed, "{body}: {decl}");
        }
    }
}

#[test]
fn a_record_element_returned_from_a_mod_is_the_element() {
    let src = "type P = { x: int, y: int }\nvar ps: P[]\nvar n: int = 0\nin go: exec\n\
               mod first(xs: P[]) -> P { return xs[n] }\non go { let p = first(ps)\n n = p.y }\n";
    let r = compile_ok(src);
    assert!(!has_unsupported(&r.module), "{:?}", r.diagnostics);
    assert_eq!(stored_from(&r, "n"), gc::ARRAY_GET);
}

#[test]
fn a_generic_body_that_needs_an_enum_is_rejected_for_its_other_members() {
    for (body, code) in [
        (
            "mod m<T: Hue | int>(a: T) -> int { return match a { Red => 1, _ => 2 } }\non go { n = m(c) }",
            "WS120",
        ),
        (
            "mod m<T: Hue | int>(a: T) -> bool { return a is Hue.Red }\non go { let q = m(c) }",
            "WS066",
        ),
        (
            "mod m<T: Hue | int>(a: T) -> int { return a.Discriminant }\non go { n = m(c) }",
            "WS066",
        ),
        (
            "mod m<T: Hue | int>(a: T, b: T) -> bool { return a == b }\non go { let q = m(c, d) }",
            "WS004",
        ),
        (
            "mod m<T>(a: T) -> T { return a }\non go { d = m(c) }",
            "WS033",
        ),
    ] {
        let src = format!("{HUE_GENERIC}{body}\n");
        let codes = errs(&src);
        assert!(codes.iter().any(|c| c == code), "{body}: {codes:?}");
    }
}

#[test]
fn a_payload_free_enum_is_one_value_in_a_generic_body_whatever_the_bound_order() {
    for bound in ["Hue | int", "int | Hue"] {
        let src = format!(
            "{HUE_GENERIC}mod pick<T: {bound}>(b: bool, x: T, y: T) -> T {{ return if b then x else y }}\n\
             mod send<T: {bound}>(x: T) {{ SendCustomEvent(\"k\", x) }}\n\
             on go {{ d = pick(n > 0, c, d)\n n = pick(true, k, n)\n send(c)\n send(k) }}\n"
        );
        let r = compile_ok(&src);
        assert!(!has_unsupported(&r.module), "{bound}: {:?}", r.diagnostics);
        emit_ok(&r).expect("emit must succeed");
        assert_eq!(stored_from(&r, "d"), gc::SELECT, "{bound}");
        assert_eq!(stored_from(&r, "n"), gc::SELECT, "{bound}");
    }
}

#[test]
fn an_enum_or_record_a_mod_returns_reaches_the_caller() {
    for (body, decls, feed) in [
        (
            "mod f() -> Hue { return Hue.Blue }\non go { d = f() }",
            &["d"][..],
            "_Literal",
        ),
        (
            "mod f(x: Hue) -> Hue { return match x { Red => Hue.Blue, _ => x } }\non go { d = f(c) }",
            &["d"][..],
            gc::SELECT,
        ),
        (
            "mod f(b: bool, x: Hue, y: Hue) -> Hue { return if b then x else y }\non go { d = f(n > 0, c, d) }",
            &["d"][..],
            gc::SELECT,
        ),
        (
            "mod f<T: Hue | int>(b: bool, x: T, y: T) -> T { return if b then x else y }\non go { d = f(n > 0, c, d) }",
            &["d"][..],
            gc::SELECT,
        ),
        (
            "mod f() -> S { return S.A(k) }\non go { s2 = f() }",
            &["s2"][..],
            "_Literal",
        ),
        (
            "mod f(x: Hue) -> (r: Hue) { emit r = x }\non go { d = f(c) }",
            &["d"][..],
            gc::VAR_GET,
        ),
        (
            "mod f(x: Hue) -> (r: Hue) { out r = x }\non go { d = f(c) }",
            &["d"][..],
            gc::VAR_GET,
        ),
        (
            "mod f<T: Hue | int>(x: T) -> (r: T) { emit r = x }\non go { d = f(c)\n n = f(k) }",
            &["d", "n"][..],
            gc::VAR_GET,
        ),
        (
            "mod f(x: Hue) -> (r: Hue, e: exec) { emit r = x\n emit e }\non go { let q = f(c)\n d = q.r }",
            &["d"][..],
            gc::VAR_GET,
        ),
        (
            "mod f(x: Hue) -> { v: Hue } { return { v: x } }\non go { d = f(c).v }",
            &["d"][..],
            gc::VAR_GET,
        ),
        (
            "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\nvar q: P = { x: 3, y: 4 }\n\
          mod f<T: P | int>(x: T) -> (r: T) { emit r = x }\non go { q = f(p)\n n = f(k) }",
            &["q.x", "n"][..],
            gc::VAR_GET,
        ),
        (
            "mod f<T: Hue | int>(x: T) -> { v: T } { return { v: x } }\non go { let r = f(c)\n d = r.v }",
            &["d"][..],
            gc::VAR_GET,
        ),
        (
            "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\n\
          mod f(a: P) -> { inner: P } { return { inner: a } }\non go { n = f(p).inner.y }",
            &["n"][..],
            gc::VAR_GET,
        ),
        (
            "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\n\
          mod f(a: P) -> { inner: P } { return { inner: a } }\non go { let r = f(p)\n n = r.inner.y }",
            &["n"][..],
            gc::VAR_GET,
        ),
        (
            "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\nmod f(a: P) -> { inner: P } { return { inner: a } }\n\
          mod g(q: { inner: P }) -> int { return q.inner.y }\non go { n = g(f(p)) }",
            &["n"][..],
            gc::VAR_GET,
        ),
        (
            "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\nvar q: P = { x: 3, y: 4 }\n\
          mod f(b: bool, a: P, c: P) -> P { return if b then a else c }\non go { q = f(n > 0, p, q) }",
            &["q.x"][..],
            gc::SELECT,
        ),
    ] {
        let src = format!("{HUE_GENERIC}{body}\n");
        let r = compile_ok(&src);
        assert!(!has_unsupported(&r.module), "{body}: {:?}", r.diagnostics);
        emit_ok(&r).expect("emit must succeed");
        for decl in decls {
            assert_eq!(stored_from(&r, decl), feed, "{body}: {decl}");
        }
    }
    let src = format!(
        "{HUE_GENERIC}mod f() -> Hue {{ return Hue.Blue }}
on go {{ d = f() }}
"
    );
    let r = compile(&src);
    let set = r
        .module
        .nodes
        .values()
        .find(|n| n.gate_class == gc::VAR_SET)
        .expect("the store");
    assert_eq!(
        set.properties.get(&*sym::VALUE),
        Some(&crate::ir::Literal::Int(2)),
        "Blue's tag is stored"
    );
}

#[test]
fn a_generic_over_an_enum_with_payloads_keeps_every_slot() {
    for (body, feed) in [
        (
            "mod f<T: S | int>(x: T) -> T { var t: T = x\n return t }\non go { s2 = f(s)\n n = f(k) }",
            gc::VAR_GET,
        ),
        (
            "var ss: S[]\nmod first<T: S | int>(xs: T[]) -> T { return xs[0] }\non go { s2 = first(ss) }",
            gc::ARRAY_GET,
        ),
        (
            "mod f<T: S | int>(b: bool, x: T, y: T) -> T { return if b then x else y }\non go { s2 = f(n > 0, s, s2) }",
            gc::SELECT,
        ),
    ] {
        let src = format!("{HUE_GENERIC}{body}\n");
        let r = compile_ok(&src);
        assert!(!has_unsupported(&r.module), "{body}: {:?}", r.diagnostics);
        emit_ok(&r).expect("emit must succeed");
        if body.contains("n = f(k)") {
            assert_eq!(stored_from(&r, "n"), gc::VAR_GET, "{body}");
        }
        let s2_vars = r
            .module
            .nodes
            .values()
            .filter(|n| n.gate_class == gc::VAR)
            .filter(|n| matches!(n.properties.get(&*sym::NAME_LABEL), Some(crate::ir::Literal::String(l)) if l.starts_with("s2.")))
            .count();
        assert_eq!(s2_vars, 2, "s2 is a tag and one payload slot");
        for slot in ["s2.__disc", "s2.__A_0"].map(|label| var_labelled(&r, label)) {
            let set = r
                .module
                .wires
                .iter()
                .find(|w| {
                    w.source.node_id == slot
                        && w.target.port == WirePort::VarRef
                        && r.module.nodes[&w.target.node_id].gate_class == gc::VAR_SET
                })
                .map(|w| w.target.node_id)
                .unwrap_or_else(|| panic!("{body}: every slot of s2 is stored"));
            let value = r
                .module
                .wires
                .iter()
                .find(|w| w.target.node_id == set && w.target.port == WirePort::Value);
            let class = value.map(|w| r.module.nodes[&w.source.node_id].gate_class);
            assert_eq!(class, Some(feed), "{body}");
        }
    }
}

/// `src` compiled with folding off and with the production default.
pub(super) fn both_folds(src: &str) -> [(&'static str, LowerResult); 2] {
    [("fold off", compile(src)), ("fold auto", compile_auto(src))]
}

pub(super) fn assert_clean(r: &LowerResult, what: &str) {
    assert!(r.diagnostics.is_empty(), "{what}: {:?}", r.diagnostics);
    assert!(!has_unsupported(&r.module), "{what}");
    emit_ok(r).unwrap_or_else(|e| panic!("{what}: {e}"));
}

/// A generic called with no type arguments from inside another generic's
/// body takes the outer instantiation's type: under `g<int>` it is `f<int>`,
/// so the caller stores the int it passed, never the exec pin an enum value
/// stashed as a record would leave behind.
#[test]
fn a_generic_called_from_another_generic_keeps_each_instances_type() {
    let forms = [
        (
            "return",
            "mod f<T: $B>(x: T) -> T { return x }\nmod g<U: $B>(y: U) -> U { return $F(y) }",
        ),
        (
            "emit",
            "mod f<T: $B>(x: T) -> (r: T) { emit r = x }\nmod g<U: $B>(y: U) -> (r: U) { emit r = $F(y) }",
        ),
        (
            "let",
            "mod f<T: $B>(x: T) -> T { return x }\nmod g<U: $B>(y: U) -> U { let t = $F(y)\n return t }",
        ),
        (
            "if",
            "mod f<T: $B>(b: bool, x: T, y: T) -> T { return if b then x else y }\n\
             mod g<U: $B>(y: U) -> U { return $F(k > 0, y, y) }",
        ),
    ];
    for bound in ["int | Hue", "Hue | int"] {
        for (form, mods) in forms {
            for calls in ["d = g(c)\n n = g(k)", "n = g(k)", "d = g(c)"] {
                let program = |f: &str| {
                    let mods = mods.replace("$B", bound).replace("$F", f);
                    format!("{HUE_GENERIC}{mods}\non go {{ {calls} }}\n")
                };
                let (src, oracle) = (program("f"), program("f<U>"));
                for ((mode, r), (_, o)) in both_folds(&src).into_iter().zip(both_folds(&oracle)) {
                    let what = format!("{form}, {bound}, `{calls}`, {mode}");
                    assert_clean(&r, &what);
                    for (target, source) in [("n", "k"), ("d", "c")] {
                        if !calls.contains(&format!("{target} = ")) {
                            continue;
                        }
                        let got = stored_values(&r, var_labelled(&r, target));
                        let want = stored_values(&o, var_labelled(&o, target));
                        let classes = |v: &[(&'static str, Option<NodeId>)]| {
                            v.iter().map(|(c, _)| *c).collect::<Vec<_>>()
                        };
                        assert_eq!(
                            classes(&got),
                            classes(&want),
                            "{what}: `{target}` as `f<U>` stores it"
                        );
                        if form != "if" {
                            assert_eq!(
                                stored_reads(&r, target),
                                vec![Some(source.to_string())],
                                "{what}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// An instantiation the capped combinations of a mod's type params leave out
/// is checked for its call and typed by that check: `b: B` is `int` in
/// `f<int, int, int>`, never the last combination's `Hue`.
#[test]
fn an_instantiation_past_the_combination_cap_keeps_its_types() {
    let bodies = [
        "-> (r: B) { emit r = b }",
        "-> (r: B) { out r = b }",
        "-> B { return b }",
        "-> B { let t = b\n return t }",
    ];
    let calls = [
        (
            "n = f(k, k, k)",
            "n",
            "k",
            vec![Type::Int, Type::Int, Type::Int],
        ),
        (
            "n = f(\"x\", \"y\", k)",
            "n",
            "k",
            vec![Type::String, Type::String, Type::Int],
        ),
        (
            "d = f(k, k, c)",
            "d",
            "c",
            vec![
                Type::Int,
                Type::Int,
                Type::Enum {
                    name: "Hue".into(),
                    args: vec![],
                },
            ],
        ),
    ];
    for bound in ["int | Hue", "Hue | int"] {
        for body in bodies {
            for (call, target, source, instance) in &calls {
                let src = format!(
                    "{HUE_GENERIC}mod f<A, C, B: {bound}>(a: A, cc: C, b: B) {body}\non go {{ {call} }}\n"
                );
                let parsed = crate::parser::parse(&src, "test");
                let tc = typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
                assert!(
                    tc.demanded_instantiations
                        .iter()
                        .any(|k| *k.3 == **instance),
                    "the combinations cover {instance:?}, so `{call}` does not reach the cap"
                );
                for (mode, r) in both_folds(&src) {
                    let what = format!("{bound}, `{body}`, `{call}`, {mode}");
                    assert_clean(&r, &what);
                    assert_eq!(
                        stored_reads(&r, target),
                        vec![Some(source.to_string())],
                        "{what}"
                    );
                }
            }
        }
    }
}

/// `out r = x` in a generic body drives the output with the value of the
/// instantiation being lowered: the int instance is not an enum.
#[test]
fn an_out_in_a_generic_body_takes_each_instances_value() {
    for bound in ["int | Hue", "Hue | int"] {
        let src = format!(
            "{HUE_GENERIC}mod f<T: {bound}>(x: T) -> (r: T) {{ out r = x }}\nout oc: Hue\n\
             on go {{ emit oc = f(c)\n n = f(k) }}\n"
        );
        for (mode, r) in both_folds(&src) {
            let what = format!("{bound}, {mode}");
            assert_clean(&r, &what);
            assert_eq!(stored_reads(&r, "n"), vec![Some("k".to_string())], "{what}");
            let out = r.module.outputs[0];
            let driver = r
                .module
                .wires
                .iter()
                .find(|w| w.target.node_id == out && w.target.port == WirePort::RerInput)
                .unwrap_or_else(|| panic!("{what}: `oc` is wired"));
            assert_eq!(
                read_label(&r, driver.source.node_id),
                Some("c".to_string()),
                "{what}"
            );
        }
    }
}

/// A field read straight off a call whose one output is a record (`f(p).y`)
/// is that record's field, whichever way the mod sets its output, generic or
/// not, one level deep or through a nested record, and when the field is a
/// payload-free enum.
#[test]
fn a_field_read_off_a_calls_record_output_is_that_field() {
    let prelude = "enum Hue { Red, Green, Blue }\ntype P = { x: int, y: int }\ntype R = { h: Hue, n: int }\n\
                   type Q = { inner: P }\nvar p: P = { x: 1, y: 2 }\nvar r1: R = { h: Hue.Blue, n: 7 }\n\
                   var q2: Q = { inner: { x: 3, y: 4 } }\nvar m: int = 0\nvar k: int = 3\nvar d: Hue = Hue.Red\n\
                   in go: exec\n";
    for (decl, stmt, target, source) in [
        (
            "mod f<T: P | int>(x: T) -> (r: T) { emit r = x }",
            "m = f(p).y",
            "m",
            "p.y",
        ),
        (
            "mod f<T: P | int>(x: T) -> (r: T) { out r = x }",
            "m = f(p).y",
            "m",
            "p.y",
        ),
        (
            "mod f<T: P | int>(x: T) -> T { return x }",
            "m = f(p).y",
            "m",
            "p.y",
        ),
        (
            "mod f<T: P | int>(x: T) -> (r: T) { emit r = x }",
            "m = f(p).y\n k = f(k)",
            "k",
            "k",
        ),
        (
            "mod f<T: P | int>(x: T) -> (r: T) { emit r = x }",
            "m = f(p).y\n k = f(k)",
            "m",
            "p.y",
        ),
        (
            "mod f(x: P) -> (r: P) { emit r = x }",
            "m = f(p).y",
            "m",
            "p.y",
        ),
        (
            "mod f(x: P) -> (r: P) { out r = x }",
            "m = f(p).x",
            "m",
            "p.x",
        ),
        (
            "mod f<T: R | int>(x: T) -> (r: T) { emit r = x }",
            "m = f(r1).n",
            "m",
            "r1.n",
        ),
        (
            "mod f<T: R | int>(x: T) -> (r: T) { emit r = x }",
            "d = f(r1).h",
            "d",
            "r1.h",
        ),
        (
            "mod f(x: R) -> (r: R) { emit r = x }",
            "d = f(r1).h\n m = f(r1).n",
            "d",
            "r1.h",
        ),
        (
            "mod f(x: R) -> (r: R) { emit r = x }",
            "d = f(r1).h\n m = f(r1).n",
            "m",
            "r1.n",
        ),
        (
            "mod f<T: Q | int>(x: T) -> (r: T) { emit r = x }",
            "m = f(q2).inner.y",
            "m",
            "q2.inner.y",
        ),
        (
            "mod f(x: Q) -> (r: Q) { out r = x }",
            "m = f(q2).inner.x",
            "m",
            "q2.inner.x",
        ),
    ] {
        let src = format!("{prelude}{decl}\non go {{ {stmt} }}\n");
        for (mode, r) in both_folds(&src) {
            let what = format!("`{decl}`, `{stmt}`, {mode}");
            assert_clean(&r, &what);
            assert_eq!(
                stored_reads(&r, target),
                vec![Some(source.to_string())],
                "{what}"
            );
        }
    }
    let src = format!(
        "{prelude}mod f<T: P | int>(x: T) -> (r: T) {{ emit r = x }}\nout o: int = f(p).y\n"
    );
    for (mode, r) in both_folds(&src) {
        assert_clean(&r, mode);
        let out = *r.module.outputs.last().expect("the output");
        let driver = r
            .module
            .wires
            .iter()
            .find(|w| w.target.node_id == out && w.target.port == WirePort::RerInput)
            .unwrap_or_else(|| panic!("{mode}: `o` is wired"));
        let from = &r.module.nodes[&driver.source.node_id];
        assert_eq!(
            from.properties.get(&*sym::NAME_LABEL),
            Some(&crate::ir::Literal::String("p.y".into())),
            "{mode}: `o` reads p.y, from {}",
            from.gate_class
        );
    }
}

/// A body whose combinations were cut short never saw some type params vary,
/// so typecheck gives no type for their instantiations and no template
/// either. Each instance still keeps its own value, taken from the mod's
/// declared output type and from the argument bindings.
#[test]
fn a_truncated_generic_body_keeps_each_instances_type() {
    let params = "a: A, b: B, cc: C, dd: D, e: E, ff: F, g: G, h: H";
    for body in [
        "-> (r: H) { emit r = h }",
        "-> H { return h }",
        "-> (r: H) { out r = h }",
    ] {
        let src = format!(
            "{HUE_GENERIC}mod f<A, B, C, D, E, F, G, H: Hue | int>({params}) {body}\n\
             on go {{ d = f(k, k, k, k, k, k, k, c)\n n = f(k, k, k, k, k, k, k, k) }}\n"
        );
        for (mode, r) in both_folds(&src) {
            let what = format!("`{body}`, {mode}");
            assert_clean(&r, &what);
            assert_eq!(stored_reads(&r, "d"), vec![Some("c".to_string())], "{what}");
            assert_eq!(stored_reads(&r, "n"), vec![Some("k".to_string())], "{what}");
        }
    }
}

/// A place given for a `*T` field of a generic record binds `T` to the place's
/// value type, as a var given for a `*T` parameter does: `get({ p: a })` reads
/// `a` and `put({ p: a }, 7)` writes it.
#[test]
fn a_place_for_a_generic_ref_field_binds_the_value_type() {
    let r = compile_ok(
        "type Box<T> = { p: *T }\nvar a: int = 0\nvar f: float = 0.0\nvar n: int = 0\nvar m: float = 0.0\n\
         mod get<T>(q: Box<T>) -> T { return q.p }\n\
         mod put<T>(q: Box<T>, v: T) { q.p = v }\n\
         in go: exec\n\
         on go {\n  n = get({ p: a })\n  m = get({ p: f })\n  put({ p: a }, 7)\n}",
    );
    assert!(!has_gate(&r, gc::UNSUPPORTED));
    assert_eq!(stored_reads(&r, "n"), vec![Some("a".to_string())]);
    assert_eq!(stored_reads(&r, "m"), vec![Some("f".to_string())]);
    assert!(
        written_var_labels(&r).contains(&"a".to_string()),
        "{:?}",
        written_var_labels(&r)
    );
}

/// A generic mod named like a builtin is the one a call names, in typecheck
/// as in lowering: `clamp(i, 0, 10)` instantiates the program's own
/// `clamp<int>`, which typecheck checked for that call.
#[test]
fn a_generic_mod_named_like_a_builtin_is_the_one_called() {
    let src = "var i: int = 0\nvar n: int = 0\nin go: exec\n\
               mod clamp<T: int | float>(v: T, lo: T, hi: T) -> T { if v < lo { return lo }\n if v > hi { return hi }\n return v }\n\
               on go { n = clamp(i, 0, 10) }\n";
    let parsed = crate::parser::parse(src, "test");
    let tc = typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default());
    let call = src.find("clamp(i, 0, 10)").unwrap();
    let key = (
        std::sync::Arc::from("test"),
        call,
        call + "clamp(i, 0, 10)".len(),
        std::sync::Arc::default(),
    );
    assert_eq!(
        tc.resolutions.call_type_args.get(&key),
        Some(&vec![Type::Int])
    );
    for (mode, r) in both_folds(src) {
        assert_clean(&r, mode);
        assert!(
            !has_gate(&r, gc::MATH_CLAMP),
            "{mode}: the builtin was called"
        );
        assert!(
            has_gate(&r, "BrickComponentType_Expr_CompareLess"),
            "{mode}: the mod's body was not inlined"
        );
    }
    // A plain mod named like a builtin types its call by its own output: the
    // `var` holding it is a string var, not the builtin's number.
    let src = "var k: int = -3\nvar so: string = \"\"\nin go: exec\nmod abs(x: int) -> string { return \"neg\" }\n\
               on go { var t = abs(k)\n so = t }\n";
    for (mode, r) in both_folds(src) {
        assert_clean(&r, mode);
        let numeric_var = r.module.nodes.values().any(|n| {
            n.gate_class == gc::VAR
                && n.ports
                    .inputs
                    .iter()
                    .chain(&n.ports.outputs)
                    .any(|p| p.ty == Type::Float)
        });
        assert!(!numeric_var, "{mode}: `t` holds the mod's string");
    }
}

/// A type parameter given a record by its instantiation names that record
/// wherever an annotation of one is read: `var u: A = a` holds `a`'s fields,
/// and an output `r: D` written from both branches stores each field. Both
/// were WS071 ("a record, not one value").
#[test]
fn a_type_parameter_given_a_record_declares_the_record() {
    let head = "type P = { x: int, y: int }\nvar p: P = { x: 1, y: 2 }\nvar q: P = { x: 0, y: 0 }\nvar k: int = 1\n\
                var n: int = 0\nvar flag: bool = true\nvar ps: P[]\nin go: exec\n";
    let var_src = format!(
        "{head}mod f<A: int | P>(a: A) -> A {{\n  var u: A = a\n  return u\n}}\non go {{\n  q = f(p)\n  n = f(k)\n}}"
    );
    let out_src = format!(
        "{head}mod f<D: int | P>(e: D, xs: D[]) -> (r: D) {{\n  if flag {{ emit r = e }} else {{ emit r = xs[0] }}\n}}\non go {{ q = f(p, ps) }}"
    );
    for src in [var_src, out_src] {
        for r in [compile_ok(&src), compile_ok_folded(&src)] {
            for field in ["q.x", "q.y"] {
                assert_eq!(
                    stored_values(&r, var_labelled(&r, field)).len(),
                    1,
                    "one store per field of `q`: {field}\n{src}"
                );
            }
        }
    }
}

/// A `...tuple` spread into a generic infers its type parameters from the
/// tuple's elements, as written-out arguments do; it was WS033 ("cannot infer
/// type parameter") and an `_Unsupported` call.
#[test]
fn a_tuple_spread_into_a_generic_infers_its_parameters() {
    for (decls, call) in [
        (
            "var n: int = 0\nout onn: int = n\n",
            "let t = (3, 4)\n  n = g(...t)",
        ),
        (
            "var f: float = 0.0\nout of: float = f\n",
            "let t = (1.5, 2.5)\n  f = g(...t)",
        ),
        ("var n: int = 0\nout onn: int = n\n", "n = g(...(3, 4))"),
    ] {
        let src = format!(
            "in go: exec\n{decls}mod g<T: int | float>(a: T, b: T) -> T {{ return a + b }}\non go {{\n  {call}\n}}"
        );
        let r = compile_ok(&src);
        assert!(!has_unsupported(&r.module), "{src}");
        assert_eq!(
            gate_count(&r, "BrickComponentType_Expr_MathAdd"),
            1,
            "{src}"
        );
    }
}

/// A program spelling a type through an alias of a generic instantiation
/// (`type IP = Pair<int>`, `type OI = Opt<int>`) compiles to the save its
/// twin spelling the instantiation does, in every position: a `var`, a
/// parameter of a `mod` and a `chip`, an annotated output, a record
/// assignment, an array element, a return type and an alias of the alias. A
/// record alias was one `any` wire (`Var f64`, its fields read through
/// `SplitColor`) and an enum alias had no tag.
#[test]
fn an_alias_of_a_generic_instantiation_is_its_instantiation() {
    let record = "type Pair<T> = { a: T, b: T }\ntype IP = Pair<int>\ntype IP2 = IP\nvar n: int = 0\nvar k: int = 3\nin go: exec\n";
    let enumd = "enum Opt<T> { Some(T), None }\ntype OI = Opt<int>\nvar n: int = 0\nvar k: int = 3\nin go: exec\n";
    let cases: &[(&str, &str)] = &[
        (record, "var q: @ = { a: 1, b: 2 }\non go { n = q.a + q.b }"),
        (
            record,
            "mod f(p: @) -> int { return p.a * p.b }\non go { n = f({ a: k, b: 2 }) }",
        ),
        (
            record,
            "chip C(p: @) -> (r: int) { out r = p.a * p.b }\non go { n = C({ a: k, b: 2 }) }",
        ),
        (record, "out z: @ = { a: k, b: 2 }"),
        (
            record,
            "var q: @ = { a: 1, b: 2 }\non go { q = { a: k, b: q.a }\nn = q.b }",
        ),
        (
            record,
            "var qs: @[]\non go { qs.push({ a: k, b: 2 })\nn = qs[0].b }",
        ),
        (
            record,
            "mod mk(x: int) -> @ { return { a: x, b: 7 } }\non go { let r = mk(k)\nn = r.b }",
        ),
        (
            enumd,
            "var o: @ = Opt.None\non go { o = Opt.Some(k)\nmatch o { Some(v) => { n = v } None => { n = 9 } } }",
        ),
        (
            enumd,
            "mod f(o: @) -> int { return match o { Some(v) => v, None => 9 } }\non go { n = f(Opt.Some(k)) }",
        ),
    ];
    let save = |src: &str, fold: bool| {
        let fold_mode = if fold {
            crate::FoldMode::ForceOn
        } else {
            crate::FoldMode::ForceOff
        };
        let input = crate::CompileInput {
            source: src,
            file: "main",
            module_name: None,
            fold_mode,
        };
        let loader = crate::resolve::MemLoader {
            files: Default::default(),
        };
        let out = crate::compile_with_loader(input, Default::default(), &loader)
            .unwrap_or_else(|e| panic!("{e:?}\n{src}"));
        super::shadow_differential::save_components(&out.brz)
    };
    let errors = |src: &str| {
        let input = crate::CompileInput {
            source: src,
            file: "main",
            module_name: None,
            fold_mode: Default::default(),
        };
        match crate::compile(input) {
            Err(crate::CompileError::HasErrors(errors)) => errors
                .iter()
                .map(|d| format!("{} {}", d.code, d.message))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    };
    for (head, body) in cases {
        let (alias, written) = if head.starts_with("enum") {
            ("OI", "Opt<int>")
        } else {
            ("IP", "Pair<int>")
        };
        let spellings: &[&str] = if alias == "IP" {
            &[alias, "IP2"]
        } else {
            &[alias]
        };
        for spelled in spellings {
            let src = format!("{head}{}", body.replace('@', spelled));
            let twin = format!("{head}{}", body.replace('@', written));
            for fold in [false, true] {
                assert_eq!(
                    save(&src, fold),
                    save(&twin, fold),
                    "fold {fold}\n{src}\n---\n{twin}"
                );
            }
        }
    }
    // Written to an output with no type, it is WS105 like its twin, and the
    // hint names the alias declared first.
    let src = format!("{record}var q: IP = {{ a: 1, b: 2 }}\non go {{ out z = q }}");
    let e = errors(&src);
    assert!(
        e.iter()
            .any(|m| m.contains("WS105") && m.contains("`out z: IP = ...`")),
        "{e:?}"
    );
    let src = "type Zed = { x: int }\ntype Abc = { x: int }\nin go: exec\nvar q: Zed = { x: 1 }\non go { out z = q }";
    let e = errors(src);
    assert!(
        e.iter()
            .any(|m| m.contains("WS105") && m.contains("`out z: Zed = ...`")),
        "{e:?}"
    );
}

/// A library's alias of a generic instantiation (`type IP = Pair<int>`) is
/// that instantiation where the library is imported, by name or as a
/// namespace, in a constant, a `var`, a parameter and through the generic
/// itself (`u.Pair<int>`): the importer compiles to the save of its twin
/// declaring the record itself. An integer for a `float` field of one is that
/// float. Each import was `WS002 unknown generic type 'Pair'`.
#[test]
fn a_library_alias_of_a_generic_instantiation_imports() {
    let lib = "type Pair<T> = { a: T, b: T }\ntype IP = Pair<int>\ntype FP = Pair<float>\nconst C: IP = { a: 1, b: 2 }\n\
               const D: FP = { a: 3, b: 4 }\nmod sum(p: IP) -> int { return p.a + p.b }\n";
    let tail = "var n: int = 0\nvar x: float = 0.0\nin go: exec\nin k: int\n";
    let ip = "{ a: int, b: int }";
    let cases: Vec<(String, String)> = vec![
        (
            format!("import {{ C }} from \"lib\"\n{tail}on go {{ n = C.a + C.b + k }}"),
            format!("{tail}const C: {ip} = {{ a: 1, b: 2 }}\non go {{ n = C.a + C.b + k }}"),
        ),
        (
            format!("import {{ D }} from \"lib\"\n{tail}on go {{ x = (D.a + D.b) / 2 }}"),
            format!(
                "{tail}const D: {{ a: float, b: float }} = {{ a: 3, b: 4 }}\non go {{ x = (D.a + D.b) / 2 }}"
            ),
        ),
        (
            format!(
                "import {{ IP }} from \"lib\"\n{tail}var q: IP = {{ a: 1, b: 2 }}\non go {{ n = q.a + q.b + k }}"
            ),
            format!("{tail}var q: {ip} = {{ a: 1, b: 2 }}\non go {{ n = q.a + q.b + k }}"),
        ),
        (
            format!(
                "import * as u from \"lib\"\n{tail}var q: u.IP = {{ a: 1, b: 2 }}\non go {{ n = q.a + q.b + k }}"
            ),
            format!("{tail}var q: {ip} = {{ a: 1, b: 2 }}\non go {{ n = q.a + q.b + k }}"),
        ),
        (
            format!(
                "import * as u from \"lib\"\n{tail}var q: u.Pair<int> = {{ a: 1, b: 2 }}\non go {{ n = q.a + q.b + k }}"
            ),
            format!("{tail}var q: {ip} = {{ a: 1, b: 2 }}\non go {{ n = q.a + q.b + k }}"),
        ),
        (
            format!("import * as u from \"lib\"\n{tail}on go {{ n = u.sum({{ a: k, b: 2 }}) }}"),
            format!(
                "{tail}mod sum(p: {ip}) -> int {{ return p.a + p.b }}\non go {{ n = sum({{ a: k, b: 2 }}) }}"
            ),
        ),
        (
            format!("import {{ sum }} from \"lib\"\n{tail}on go {{ n = sum({{ a: k, b: 2 }}) }}"),
            format!(
                "{tail}mod sum(p: {ip}) -> int {{ return p.a + p.b }}\non go {{ n = sum({{ a: k, b: 2 }}) }}"
            ),
        ),
    ];
    let save = |src: &str, fold: bool| {
        let fold_mode = if fold {
            crate::FoldMode::ForceOn
        } else {
            crate::FoldMode::ForceOff
        };
        let input = crate::CompileInput {
            source: src,
            file: "main",
            module_name: None,
            fold_mode,
        };
        let loader = crate::resolve::MemLoader {
            files: [("lib.ws".to_string(), lib.to_string())]
                .into_iter()
                .collect(),
        };
        let out = crate::compile_with_loader(input, Default::default(), &loader)
            .unwrap_or_else(|e| panic!("{e:?}\n{src}"));
        super::shadow_differential::save_components(&out.brz)
    };
    for (src, twin) in &cases {
        for fold in [false, true] {
            assert_eq!(
                save(src, fold),
                save(twin, fold),
                "fold {fold}\n{src}\n---\n{twin}"
            );
        }
    }
}

// ---------- generic declarations: typecheck and alias lowering ----------

#[test]
fn generic_mod_type_params_resolve_not_ws002() {
    // T is a recognized type param -> NO WS002 for T (sig + body annotation)
    let e = errs("mod pick<T>(a: T, b: T) -> T {\n  let x: T = a\n  return x\n}\n");
    assert!(
        !e.contains(&"WS002".to_string()),
        "T should resolve, not WS002: {e:?}"
    );
    // a bound param resolves too
    let e2 = errs("mod clamp<T: Numeric>(v: T) -> T { return v }\n");
    assert!(
        !e2.contains(&"WS002".to_string()),
        "bounded T should resolve: {e2:?}"
    );
    // an actually-unknown type still errors WS002
    let e3 = errs("mod bad<T>(a: T) -> T {\n  let y: Q = a\n  return a\n}\n");
    assert!(
        e3.contains(&"WS002".to_string()),
        "unknown Q should be WS002: {e3:?}"
    );
    // type params do NOT leak out: a non-generic mod using `T` still errors
    let e4 = errs("mod plain(a: T) -> int { return 0 }\n");
    assert!(
        e4.contains(&"WS002".to_string()),
        "T outside a generic mod should be WS002: {e4:?}"
    );
}

#[test]
fn generic_mod_call_inference() {
    // Error diagnostics as (code, message) pairs — used where the message text
    // itself is load-bearing (proving the *concrete* substituted type).
    let err_msgs = |s: &str| {
        crate::typecheck::typecheck(
            &crate::parser::parse(s, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        )
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| (d.code.to_string(), d.message))
        .collect::<Vec<_>>()
    };
    let pick = "in flag: bool\nmod pick<T>(c: bool, a: T, b: T) -> T { return a }\n";
    // inference succeeds: pick(flag, 1, 2) => T=int, no error
    assert!(
        errs(&format!("{pick}let x = pick(flag, 1, 2)\n")).is_empty(),
        "int inference should be clean: {:?}",
        errs(&format!("{pick}let x = pick(flag, 1, 2)\n"))
    );
    // result type is CONCRETELY float (not any, not a leaked `Param("T")`):
    // int + float safe-widens T to float, and assigning the result to a
    // vector-typed var errors, AND the error MESSAGE names `Float` as the
    // actual type. (A `let` annotation mismatch is only a WS016 *warning*
    // here — `var`'s initializer coercion is the hard WS003 error, so
    // that's the construct that proves the result type.) Asserting the message
    // (not just the WS003 code) is deliberate: `coerce(Param("T"), Vector)` is
    // ALSO a Mismatch -> WS003, so a regression that stopped substituting and
    // leaked a raw `Param` would still emit WS003 — only the "got Float" text
    // proves substitution actually produced the concrete widened `float`.
    let vecerr = err_msgs(&format!("{pick}var v: vector = pick(flag, 1, 2.0)\n"));
    assert!(
        vecerr
            .iter()
            .any(|(c, m)| c == "WS003" && m.contains("float")),
        "int+float widened result into vector var should WS003 whose message names Float \
         (proves result is concrete float, not any / leaked Param): {vecerr:?}"
    );
    // widening: int + float have no strict-equality agreement but DO share a
    // safe widening (int -> float) -> T=float, clean (NOT a WS033 conflict).
    assert!(
        errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n")).is_empty(),
        "int+float should widen to float, not conflict: {:?}",
        errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n"))
    );
    // incompatible: int vs vector share no common widening -> WS033
    let e = errs(&format!(
        "in vecArg: vector\n{pick}let x = pick(flag, 1, vecArg)\n"
    ));
    assert!(
        e.iter().any(|c| c == "WS033"),
        "int/vector incompatible should be WS033: {e:?}"
    );
    // out of mask: string is not Numeric. (Named `onlyNumeric`, not `clamp` —
    // `clamp` collides with the builtin math function of the same name, which
    // `find_call` resolves before user symbols and so never reaches this
    // user-mod inference path at all.)
    let only_numeric = "mod onlyNumeric<T: Numeric>(v: T) -> T { return v }\n";
    let e2 = errs(&format!("{only_numeric}let x = onlyNumeric(\"hi\")\n"));
    assert!(
        e2.iter().any(|c| c == "WS033"),
        "string out of Numeric mask should be WS033: {e2:?}"
    );
    // vector inference: pick(flag, va, vb) => T=vector, clean. (`pick` already
    // declares `in flag: bool`; only `va`/`vb` are new here — redeclaring
    // `flag` would itself be a duplicate-decl error unrelated to inference.)
    let vecsrc = format!("in va: vector\nin vb: vector\n{pick}let r = pick(flag, va, vb)\n");
    assert!(
        errs(&vecsrc).is_empty(),
        "vector inference should be clean: {:?}",
        errs(&vecsrc)
    );
    // Ref-param inference: a `*T` param resolves to `Ref(Param(T))`, but a var
    // arg auto-derefs to its inner type — the Ref layers must be aligned before
    // collecting or `T` is never pinned. Ref params are core idiomatic
    // Wirescript (`mod inc(v: *int)`), so this MUST infer cleanly (no WS033).
    let e3 = errs("mod passRef<T>(v: *T) { }\nvar x: int = 0\npassRef(x)\n");
    assert!(
        !e3.iter().any(|c| c == "WS033"),
        "ref-param generic call should infer cleanly (no WS033): {e3:?}"
    );
    // ...and a ref param that's actually written through (the `inc` shape) works too.
    let e4 = errs("mod inc<T>(v: *T) { v = v }\nvar y: int = 0\ninc(y)\n");
    assert!(
        !e4.iter().any(|c| c == "WS033"),
        "ref-param write-through generic call should infer cleanly (no WS033): {e4:?}"
    );
}

#[test]
fn generic_widening_inference() {
    let pick = "in flag: bool\nmod pick<T>(c: bool, a: T, b: T) -> T { return a }\n";
    // int + float widens to float -> clean
    assert!(
        errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n")).is_empty(),
        "int+float should widen to float: {:?}",
        errs(&format!("{pick}let x = pick(flag, 1, 2.0)\n"))
    );
    // int + vector don't widen -> WS033
    let e = errs(&format!("in v: vector\n{pick}let x = pick(flag, 1, v)\n"));
    assert!(
        e.iter().any(|c| c == "WS033"),
        "int+vector incompatible should be WS033: {e:?}"
    );
    // character + entity widens to entity -> clean
    let obj = "in flag: bool\nin ch: character\nin en: entity\nmod pick2<T>(c: bool, a: T, b: T) -> T { return a }\n";
    assert!(
        errs(&format!("{obj}let r = pick2(flag, ch, en)\n")).is_empty(),
        "character+entity should widen to entity: {:?}",
        errs(&format!("{obj}let r = pick2(flag, ch, en)\n"))
    );
}

#[test]
fn generic_body_checked_per_mask_member() {
    // `a + 1` is valid for every Scalar member (int + 1 -> int, float + 1 -> float),
    // so the generic body checks CLEAN once we check per concrete member.
    let ok = "mod addOne<T: Scalar>(a: T) -> T { return a + 1 }\n";
    assert!(
        errs(ok).is_empty(),
        "a + 1 valid for all Scalar members should be clean: {:?}",
        errs(ok)
    );
    // `Dot()` takes two vectors — invalid for every Scalar member (int/float
    // aren't vectors) — so the body fails per-member checking and surfaces
    // an error at the DEFINITION. (`a.length()` is NOT a counterexample here
    // — method-call resolution doesn't check the receiver type strictly, so
    // it silently passes on int/float too; `Dot()`'s WS003 "expected Vector,
    // got Int/Float" argument-type check is a genuine per-member failure.)
    let bad = "mod wrong<T: Scalar>(a: T) -> float { return a.Dot(a) }\n";
    assert!(
        !errs(bad).is_empty(),
        "member-invalid op should error at the generic definition: {:?}",
        errs(bad)
    );
    // the error surfaces ONCE (not once per member) — dedup check:
    assert!(
        errs(bad).len() <= 2,
        "member errors should be deduped, got {:?}",
        errs(bad)
    );
    // a non-generic mod is unaffected
    assert!(errs("mod plain(a: int) -> int { return a + 1 }\n").is_empty());
}

#[test]
fn if_expr_and_builtins_widen() {
    let errs = |s: &str| {
        crate::typecheck::typecheck(
            &crate::parser::parse(s, "t").ast,
            "t",
            &crate::typecheck::CeSlotMap::default(),
        )
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| (d.code.to_string(), d.message.clone()))
        .collect::<Vec<_>>()
    };
    let codes = |s: &str| errs(s).into_iter().map(|(c, _)| c).collect::<Vec<_>>();

    // if-expr: int/float branches widen to float -> clean.
    assert!(
        codes("in c: bool\nlet x = if c then 1 else 2.0\n").is_empty(),
        "if int/float should widen to float: {:?}",
        errs("in c: bool\nlet x = if c then 1 else 2.0\n")
    );
    // Same, branches swapped: widening resolves to `float` either way,
    // proven by the assignment check below rather than by luck of which
    // branch is which.
    assert!(
        codes("in c: bool\nlet y = if c then 2.0 else 1\n").is_empty(),
        "if float/int (branches swapped) should also widen to float: {:?}",
        errs("in c: bool\nlet y = if c then 2.0 else 1\n")
    );
    // result is float: assigning to a vector annotation errors "got Float".
    // (`var`'s initializer coercion is the hard WS003 error -- a `let`
    // annotation mismatch is only a WS016 warning here, per the `pick`
    // tests above, so `var` is what proves the concrete result type.)
    // Proves the *swapped* branch order also yields Float (old else-wins
    // code would have produced Int here, whose WS003 message would say
    // "Int", not "Float").
    let swapped_vec_err = errs("in c: bool\nvar v: vector = if c then 2.0 else 1\n");
    assert!(
        swapped_vec_err
            .iter()
            .any(|(cd, m)| cd == "WS003" && m.contains("float")),
        "if-expr result (else-wins branch order) should still be Float under widening: {:?}",
        swapped_vec_err
    );
    let vec_err = errs("in c: bool\nvar v: vector = if c then 1 else 2.0\n");
    assert!(
        vec_err
            .iter()
            .any(|(cd, m)| cd == "WS003" && m.contains("float")),
        "if-expr result should be Float: {:?}",
        vec_err
    );
    // incompatible branches still error (WS003, no common widening).
    assert!(
        codes("in c: bool\nin v: vector\nlet x = if c then 1 else v\n")
            .iter()
            .any(|c| c == "WS003"),
        "int/vector branches should still error"
    );

    // Builtin half: `union_output_type` is used by the math-variant gates
    // whose declared output is `Type::Union` -- confirmed (via catalog/calls.rs)
    // to be `Blend`, `lerp`, and `Easing` (all built from `blend_variant()` ==
    // Union([Float, Int, Vector, Rotator, Quat, Color])). `Tween`'s output is a
    // *Record* wrapping a union field, not a bare Union, so it doesn't go
    // through this path. Use `lerp(a, b, t)` -- the plainest signature.
    assert!(
        codes("let x = lerp(1, 2.0, 0.5)\n").is_empty(),
        "lerp(int, float, float) should widen to float, clean: {:?}",
        errs("let x = lerp(1, 2.0, 0.5)\n")
    );
    // incompatible: int vs vector share no common widening -> WS033 (both
    // ARE individually valid blend-variant members, so this only fails at
    // the join step, not at argument-type checking).
    let e = errs("in vecArg: vector\nlet x = lerp(1, vecArg, 0.5)\n");
    assert!(
        e.iter().any(|(c, _)| c == "WS033"),
        "lerp(int, vector) incompatible should be WS033: {e:?}"
    );
    // Blend, receiver form: `a.Blend(b, alpha)` -- same widening, same gate.
    assert!(
        codes("in a: int\nlet x = a.Blend(2.0, 0.5)\n").is_empty(),
        "a.Blend(2.0, ...) should widen to float, clean: {:?}",
        errs("in a: int\nlet x = a.Blend(2.0, 0.5)\n")
    );
}

#[test]
fn generic_chip_type_checks_clean() {
    // Generic *chips* (physical microchips) are monomorphized per distinct
    // type instantiation at lowering time (one template per `(name, subst)`),
    // so a generic chip decl type-checks clean — the old WS034 hard error is
    // gone. (The per-type lowering + no-shared-grid proof lives in
    // `lower::tests::generics::generic_chip_monomorphizes_per_instantiation`.)
    let gchip = "chip Box<T>(v: T) -> (r: T) { out r = v }\n";
    assert!(
        errs(gchip).is_empty(),
        "generic chip must type-check clean now: {:?}",
        errs(gchip)
    );
    // a generic MOD is likewise clean -- mods inline+monomorphize per call.
    let gmod = "mod pick<T>(a: T) -> T { return a }\n";
    assert!(
        errs(gmod).is_empty(),
        "generic mod must be allowed: {:?}",
        errs(gmod)
    );
    // a non-generic chip is allowed (same syntax, no `<T>`).
    let plain = "chip Box(v: int) -> (r: int) { out r = v }\n";
    assert!(
        errs(plain).is_empty(),
        "non-generic chip must be allowed: {:?}",
        errs(plain)
    );
}

#[test]
fn generic_type_aliases_instantiate() {
    // Pair<int> resolves to { a: int, b: int } — `.a` used where an `int` is
    // expected is clean.
    let ok = "type Pair<T> = { a: T, b: T }\nin p: Pair<int>\nout r: int = p.a\n";
    assert!(
        errs(ok).is_empty(),
        "Pair<int>.a should be int, clean: {:?}",
        errs(ok)
    );
    // Pair<string> resolves to { a: string, b: string } — `.a` no longer
    // coerces to `int` (string doesn't coerce to a numeric), proving `T`
    // actually substituted per instantiation rather than resolving to `any`
    // (which would coerce to anything and stay clean).
    let bad = "type Pair<T> = { a: T, b: T }\nin p: Pair<string>\nout r: int = p.a\n";
    assert!(
        !errs(bad).is_empty(),
        "Pair<string>.a should be string, mismatched against an int out port (T=string): {:?}",
        errs(bad)
    );
    // Not fully applied.
    let bare = "type Pair<T> = { a: T, b: T }\nlet p: Pair = { a: 1, b: 2 }\n";
    assert!(
        !errs(bare).is_empty(),
        "bare generic alias should error: {:?}",
        errs(bare)
    );
    // Wrong arity.
    let arity = "type Pair<T> = { a: T, b: T }\nlet p: Pair<int, float> = { a: 1, b: 2 }\n";
    assert!(
        !errs(arity).is_empty(),
        "arity mismatch should error: {:?}",
        errs(arity)
    );
    // Recursive alias errors (does not hang).
    let recursive = "type L<T> = { head: T, tail: L<T> }\nin x: L<int>\n";
    assert!(
        !errs(recursive).is_empty(),
        "recursive generic alias should error: {:?}",
        errs(recursive)
    );
    // A DOUBLY self-referencing alias would re-expand each occurrence — a
    // depth-only guard blows up exponentially; the in-progress cycle guard must
    // cut it off so this terminates with a diagnostic rather than hanging.
    let tree = "type Tree<T> = { l: Tree<T>, r: Tree<T> }\nin x: Tree<int>\n";
    assert!(
        !errs(tree).is_empty(),
        "doubly-recursive generic alias should error, not hang: {:?}",
        errs(tree)
    );
    // A non-generic alias still works.
    let plain = "type P = { x: int }\nlet q: P = { x: 1 }\n";
    assert!(
        errs(plain).is_empty(),
        "non-generic alias should still work: {:?}",
        errs(plain)
    );
}

/// Regression: a generic-alias record type used at a PORT or a chip param
/// must dissolve into real per-field wire gates at LOWERING — not silently
/// degrade to a single `any` port whose field accesses emit
/// `_Unsupported`/`SplitColor` swizzle gates with zero diagnostics.
/// typecheck alone can't catch this (it was green while the IR was broken), so
/// this asserts on the emitted IR.
#[test]
fn generic_alias_record_port_lowers_to_real_gates() {
    // Top-level record PORT: `Pair<int>` -> two int sub-ports feeding a MathAdd.
    let r = compile_ok("type Pair<T> = { a: T, b: T }\nin p: Pair<int>\nout r: int = p.a + p.b\n");
    assert!(
        !has_unsupported(&r.module),
        "generic-alias port field access must not lower to _Unsupported"
    );
    assert!(
        !has_gate(&r, gc::SPLIT_COLOR),
        "generic-alias port field access must not lower to a SplitColor swizzle"
    );
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "p.a + p.b on a Pair<int> port must lower to a real MathAdd gate"
    );
    // The port must have dissolved into one sub-port per field (2), not stayed
    // a single collapsed `any` port.
    assert_eq!(
        gate_count(&r, "BrickComponentType_Internal_MicrochipInput"),
        2,
        "Pair<int> port should expand into two int sub-ports"
    );

    // Chip param form (the `resolved_record` site). The generic case must
    // produce IR identical in shape to the non-generic one.
    let rg = compile_ok(
        "type Pt<T> = { foo: T, bar: T }\nchip Sum(p: Pt<int>) -> (r: int) { out r = p.foo + p.bar }\nin a: int\nout total: int = Sum({ foo: a, bar: 2 })\n",
    );
    assert!(
        !has_unsupported(&rg.module),
        "root of generic chip-param program must have no _Unsupported"
    );
    // The chip body lowers `p.foo + p.bar` to a real MathAdd (and, above, no
    // `_Unsupported` field-access gate anywhere in the tree).
    assert_eq!(
        count_class(
            only_chip(&rg, "the Sum instance"),
            "BrickComponentType_Expr_MathAdd"
        ),
        1
    );
}

#[test]
fn deeply_nested_generic_decls_terminate_quickly() {
    // The per-mask-member body check is a whole-PATH budget, not per-decl, so
    // nesting generic decls can't multiply the combo count (`13^d` -> a 17s+
    // typecheck / LSP hang). Depth-6 nested generic mods must type-check clean
    // and near-instantly (this test would time out conspicuously in the suite
    // if the budget regressed).
    let src = "mod l0<A>(a: A) -> A {\n\
               mod l1<B>(b: B) -> B {\n\
               mod l2<C>(c: C) -> C {\n\
               mod l3<D>(d: D) -> D {\n\
               mod l4<E>(e: E) -> E {\n\
               mod l5<F>(f: F) -> F { return f }\n return e }\n return d }\n\
               return c }\n return b }\n return a }\n\
               in go: exec\nin n: int\non go { let z = l0(n) }\n";
    let parsed = crate::parser::parse(src, "t");
    let tc = crate::typecheck::typecheck(&parsed.ast, "t", &crate::typecheck::CeSlotMap::default());
    let errs: Vec<String> = tc
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string())
        .collect();
    assert!(
        errs.is_empty(),
        "deep nested generics must check clean: {errs:?}"
    );
}

#[test]
fn the_unbounded_parameter_hint_names_the_one_the_receiver_comes_from() {
    let src = "var n: int = 0\nvar s: string = \"ab\"\nin go: exec\nmod id<A>(v: A) -> A { return v }\n\
               mod f<T, U>(v: T, w: U) -> int { return id(v).Length() }\non go { n = f(s, s) }\n";
    let result = crate::compile(crate::CompileInput {
        source: src,
        file: "main",
        module_name: None,
        fold_mode: crate::FoldMode::Auto,
    });
    let Err(crate::CompileError::HasErrors(errors)) = result else {
        panic!("compiles")
    };
    assert!(
        errors
            .iter()
            .any(|d| d.code == "WS003" && d.message.contains("<T: string>")),
        "{errors:?}"
    );
    let fixed = src.replace("mod f<T, U>", "mod f<T: string, U>");
    assert!(
        super::declaring_scope::save_dump(&fixed, &[], true).is_ok(),
        "the suggested bound does not compile"
    );
    // A callee whose result is its second argument's type gives the
    // parameter that argument has.
    let src = "var n: int = 0\nvar s: string = \"ab\"\nin go: exec\nmod two<A, B>(a: A, b: B) -> B { return b }\n\
               mod f<T, U>(v: T, w: U) -> int { return two(v, w).Length() }\non go { n = f(s, s) }\n";
    let result = crate::compile(crate::CompileInput {
        source: src,
        file: "main",
        module_name: None,
        fold_mode: crate::FoldMode::Auto,
    });
    let Err(crate::CompileError::HasErrors(errors)) = result else {
        panic!("compiles")
    };
    assert!(
        errors
            .iter()
            .any(|d| d.code == "WS003" && d.message.contains("<U: string>")),
        "{errors:?}"
    );
    let fixed = src.replace("mod f<T, U>", "mod f<T, U: string>");
    assert!(
        super::declaring_scope::save_dump(&fixed, &[], true).is_ok(),
        "the suggested bound does not compile"
    );
}
