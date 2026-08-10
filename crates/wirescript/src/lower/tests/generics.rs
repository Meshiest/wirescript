//! Monomorphization of generic `mod` calls (Task 2.5): a generic mod inlined
//! at a concrete call site must emit CONCRETE gates (`box<int>` → int gates,
//! `box<vector>` → vector gates), never leaking `Type::Param` (or the wrong
//! `Any`/last-combo variant it defaulted to before) into emit.

use super::*;
use crate::ir::{Module, Type};

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
    module
        .nodes
        .values()
        .any(|n| n.ports.inputs.iter().chain(n.ports.outputs.iter()).any(|p| ty_has_param(&p.ty)))
        || module.chips.values().any(any_param_port)
}

/// The `Value`-port types of every `Pseudo_Var` gate in the module tree — one
/// per inlined `var stored: T`, so this reads out each monomorph's concrete
/// storage type.
fn pseudo_var_value_types(module: &Module) -> Vec<Type> {
    let mut out = Vec::new();
    fn walk(module: &Module, out: &mut Vec<Type>) {
        for n in module.nodes.values() {
            if n.gate_class.contains("Pseudo_Var") {
                if let Some(p) = n
                    .ports
                    .inputs
                    .iter()
                    .chain(n.ports.outputs.iter())
                    .find(|p| crate::intern::resolve(p.name) == "Value")
                {
                    out.push(p.ty.clone());
                }
            }
        }
        for c in module.chips.values() {
            walk(c, out);
        }
    }
    walk(module, &mut out);
    out
}

/// Output-port types of every gate whose class contains `needle`, across the
/// whole module tree. Used to read out an operator's / if-expr's emitted variant.
fn gate_output_types(module: &Module, needle: &str) -> Vec<Type> {
    let mut out = Vec::new();
    fn walk(module: &Module, needle: &str, out: &mut Vec<Type>) {
        for n in module.nodes.values() {
            if n.gate_class.contains(needle) {
                out.extend(n.ports.outputs.iter().map(|p| p.ty.clone()));
            }
        }
        for c in module.chips.values() {
            walk(c, needle, out);
        }
    }
    walk(module, needle, &mut out);
    out
}

fn emit_ok(r: &LowerResult) -> Result<(), String> {
    let lr = crate::layout::layout(&r.module);
    let opts = crate::emit::EmitOptions::default();
    crate::emit::emit_brz(
        &r.module,
        &lr,
        &opts,
        &std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
    )
    .map(|_| ())
    .map_err(|e| format!("{e:?}"))
}

#[test]
fn generic_mod_monomorphizes_to_concrete_gates() {
    // A generic mod with a T-typed storage gate + T return. Called at `int`
    // and at `vector`, each inline must emit a concrete int / vector Variable
    // gate — and EMIT cleanly. A leaked `Type::Param` (or the pre-fix `Any`
    // default) would give both monomorphs a wrong (Number-defaulted) variant.
    let src = "mod boxed<T>(v: T) -> T { static var stored: T = v\n return stored }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let a = boxed(n)\n  let b = boxed(vec)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);

    // The real monomorphization proof: emit succeeds (a leaked Param trips
    // emit's debug_assert / produces a bad variant).
    emit_ok(&r).expect("emit must succeed for a monomorphized generic");

    // No Type::Param survives anywhere.
    assert!(
        !any_param_port(&r.module),
        "no port may still carry Type::Param after monomorphization"
    );

    // Exactly the two monomorphs, one int-typed and one vector-typed.
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "the two `stored` gates must be concrete int + vector, got {vals:?}"
    );
}

#[test]
fn generic_mod_bounded_param_monomorphizes() {
    // Bounded `<T: Numeric>` selecting between two args. Called at int and at
    // float — the `chosen: T` storage gate must come out int / float.
    let src = "mod pick<T: Numeric>(c: bool, a: T, b: T) -> T {\n\
               static var chosen: T = a\n  return chosen\n}\n\
               in go: exec\nin i: int\nin f: float\n\
               on go {\n  let x = pick(true, i, i)\n  let y = pick(false, f, f)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(vals, vec![Type::Float, Type::Int]);
}

#[test]
fn generic_mod_buffer_and_array_annotations_monomorphize() {
    // `T`-typed buffer + `T[]` array declared inside a generic mod body: both
    // annotation-resolution leak points must monomorphize to concrete element
    // types (int here), not `Any`.
    let src = "mod work<T>(x: T) -> T {\n\
               buffer prev: T = x\n  var items: T[] = []\n  items.push(x)\n  return prev\n}\n\
               in go: exec\nin n: int\non go {\n  let a = work(n)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
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
        .find(|n| n.gate_class == crate::ir::gate_class::PSEUDO_ARRAY_VAR)
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
fn nested_generic_mod_forwarding_monomorphizes() {
    // Core composition pattern: `outer<T>` forwards its own `T`-typed value
    // into `inner<T>`. `inner`'s `static var s: T` must monomorphize to the
    // type flowing at the OUTER call site (int, then vector) — NOT the stale
    // last-mask-member type P2.4 left in `type_of_expr` (which silently
    // collapsed BOTH monomorphs to Prefab).
    let src = "mod inner<T>(v: T) -> T { static var s: T = v\n return s }\n\
               mod outer<T>(v: T) -> T { let r = inner(v)\n return r }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let a = outer(n)\n  let b = outer(vec)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    // Two `inner` monomorphs (one per outer call), one int + one vector.
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "nested inner<T> gates must monomorphize to int + vector, got {vals:?}"
    );
}

#[test]
fn nested_generic_mod_bounded_forwarding_monomorphizes() {
    // Same, bounded — the pre-fix bug collapsed both to `Color` (last Numeric
    // mask member) instead of int / float.
    let src = "mod inner<T: Numeric>(v: T) -> T { static var s: T = v\n return s }\n\
               mod outer<T: Numeric>(v: T) -> T { let r = inner(v)\n return r }\n\
               in go: exec\nin i: int\nin f: float\n\
               on go {\n  let a = outer(i)\n  let b = outer(f)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(vals, vec![Type::Float, Type::Int]);
}

#[test]
fn generic_mod_multi_return_monomorphizes() {
    // Two `return`s + a single `-> T` output routes through the multi-return
    // `mod_return_var` PseudoVar, whose `out_type` is the other annotation leak
    // point. Called at int and vector, both ret-val gates must be concrete.
    let src = "mod choose<T>(c: bool, a: T, b: T) -> T {\n\
               if c { return a }\n  return b\n}\n\
               in go: exec\nin i: int\nin v: vector\n\
               on go {\n  let x = choose(true, i, i)\n  let y = choose(false, v, v)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    // Each monomorph's ret-val PseudoVar carries the concrete type.
    let vals = pseudo_var_value_types(&r.module);
    assert!(
        vals.contains(&Type::Int) && vals.contains(&Type::Vector),
        "multi-return ret-val gates must monomorphize to int + vector, got {vals:?}"
    );
}

#[test]
fn generic_mod_polymorphic_recursion_terminates() {
    // A generic mod that (transitively) calls itself would make
    // monomorphization non-terminating. The existing WS020 recursion guard
    // (in `lower_chip_call`) blocks the self-call BEFORE the monomorph inline
    // re-enters, so lowering terminates with an error instead of looping.
    let src =
        "mod rec<T>(v: T) -> T { return rec(v) }\nin n: int\nin go: exec\non go { let a = rec(n) }\n";
    let r = compile(src); // must return (no hang / stack overflow)
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS020"),
        "recursive generic mod must be rejected by the WS020 recursion guard, got {:?}",
        r.diagnostics
    );
}

#[test]
fn generic_body_invalid_for_a_member_is_rejected_above_the_combo_cap() {
    // Combo-cap escape (correctness review): two UNbounded params make
    // 11×11 = 121 mask combos, over MAX_BODY_CHECK_COMBOS (64). The old capped
    // fallback checked a single all-first-member combo (every param bound to
    // `bool`, the most permissive member for arithmetic), so a body op invalid
    // for other members slipped through with NO diagnostic and lowering then
    // monomorphized it into a broken gate. `a + b` is not a valid op for every
    // instantiation, so the decl must be rejected even above the cap.
    let src = "mod combine<A, B>(a: A, b: B) -> A {\n  var x: A = a\n  x = a + b\n  return x\n}\n\
               on CharacterSpawned(ch) {\n  let e: entity = ch\n  let r = combine(e, e)\n}\n";
    let tc = typecheck(&parse(src, "test").ast, "test");
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS004"),
        "a generic body op invalid for some mask member must be rejected even \
         above the combo cap, got {:?}",
        tc.diagnostics.iter().map(|d| d.code.to_string()).collect::<Vec<_>>()
    );
    // Sanity (no over-rejection): a body that is valid for EVERY instantiation
    // must still type-check clean above the cap. `first<A, B>` also has 121
    // combos (> cap) but its body only returns `a`, so no combo can fail.
    let ok = "mod first<A, B>(a: A, b: B) -> A {\n  return a\n}\n\
              on CharacterSpawned(ch) {\n  let r = first(ch, 1)\n}\n";
    let tco = typecheck(&parse(ok, "test").ast, "test");
    assert!(
        !tco.diagnostics.iter().any(|d| d.severity == crate::diagnostic::Severity::Error),
        "a generic with an always-valid body must type-check clean above the cap, got {:?}",
        tco.diagnostics
    );
}

#[test]
fn non_generic_mod_lowering_unchanged() {
    // Guard sanity: a non-generic mod pushes no MonoFrame, so its `var s: int`
    // still lowers to a concrete int gate exactly as before — the generic path
    // is fully gated on non-empty `type_params`.
    let src = "mod keep(v: int) -> int { static var s: int = 0\n s = v\n return s }\n\
               in go: exec\nin n: int\non go { let a = keep(n) }\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    assert_eq!(pseudo_var_value_types(&r.module), vec![Type::Int]);
}

#[test]
fn generic_body_operator_and_if_use_call_monomorph_variant() {
    // Regression (correctness review): operators (`v * v`) and if-exprs
    // (`if c then a else b`) inside a generic body must emit the CALL's
    // monomorph variant — not the last mask-member the per-mask-member body
    // check leaves in `op_resolutions` / `type_of_expr` (Numeric → Color,
    // an unbounded `T` → Prefab). `sq<int>` must be an int MathMultiply,
    // `sq<vector>` a vector one; `pick<int>` an int Select — never Color/Prefab.
    let src = "mod sq<T: Numeric>(v: T) -> T { return v * v }\n\
               mod pick<T>(c: bool, a: T, b: T) -> T { return if c then a else b }\n\
               in go: exec\nin n: int\nin vec: vector\nin flag: bool\n\
               on go {\n  let a = sq(n)\n  let b = sq(vec)\n  let d = pick(flag, n, n)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));

    // The two MathMultiply monomorphs: one int, one vector — never Color.
    let mut muls = gate_output_types(&r.module, "MathMultiply");
    muls.sort_by_key(|t| format!("{t:?}"));
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
/// to a string. Two instances collapse to one emitted grid iff they share a key
/// — so the count is the number of distinct emitted grids. Recurses into nested
/// chips (like `any_param_port` / `pseudo_var_value_types`) so a collapse of a
/// nested generic chip's grids is caught too.
fn chip_template_keys(module: &Module) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    fn walk(module: &Module, out: &mut std::collections::HashSet<String>) {
        for c in module.chips.values() {
            if let Some(k) = c.template_key {
                out.insert(crate::intern::resolve(k).to_string());
            }
            walk(c, out);
        }
    }
    walk(module, &mut out);
    out
}

/// Data-carrying (non-Exec) rerouter port types across the whole module tree —
/// the MicrochipInput/Output boundary ports of every chip instance. Reads out
/// each monomorph's boundary typing (the `_exec_in`/`_exec_out` rerouters are
/// Exec-typed and excluded).
fn rerouter_data_types(module: &Module) -> Vec<Type> {
    let mut out = Vec::new();
    fn walk(module: &Module, out: &mut Vec<Type>) {
        for n in module.nodes.values() {
            if n.gate_class == crate::ir::gate_class::MICROCHIP_INPUT
                || n.gate_class == crate::ir::gate_class::MICROCHIP_OUTPUT
            {
                for p in n.ports.inputs.iter().chain(n.ports.outputs.iter()) {
                    if p.ty != Type::Exec {
                        out.push(p.ty.clone());
                    }
                }
            }
        }
        for c in module.chips.values() {
            walk(c, out);
        }
    }
    walk(module, &mut out);
    out
}

#[test]
fn generic_chip_monomorphizes_per_instantiation() {
    // A generic *chip* (a physical microchip, NOT an inline mod) instantiated at
    // two different types must emit TWO distinct microchip templates — one
    // monomorphized to int, one to vector — never collapsing onto a single
    // shared grid (the silent cross-wiring the old WS034 guard rejected). This
    // is the chip analog of `generic_mod_monomorphizes_to_concrete_gates`.
    let src = "chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let a = Boxed(n)\n  let b = Boxed(vec)\n}\n";

    // WS034 is gone: the program type-checks clean (no errors at all).
    let parsed = crate::parser::parse(src, "test");
    let tc = crate::typecheck::typecheck(&parsed.ast, "test");
    let errs: Vec<String> = tc
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string())
        .collect();
    assert!(errs.is_empty(), "generic chip must type-check clean now: {errs:?}");

    let r = compile(src);
    assert_no_errors(&r);
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
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
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
fn generic_chip_same_type_dedups_to_one_template() {
    // Two calls at the SAME type must share ONE template (grid dedup holds):
    // both instances carry the same monomorph `template_key`, so emit collapses
    // them into one grid — exactly the non-generic dedup behavior, re-keyed on
    // the concrete monomorph rather than the bare name.
    let src = "chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
               in go: exec\nin n: int\nin m: int\n\
               on go {\n  let a = Boxed(n)\n  let b = Boxed(m)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    let keys = chip_template_keys(&r.module);
    assert_eq!(
        keys.len(),
        1,
        "two int calls must share ONE template key, got {keys:?}"
    );
    // Both instances are present (two chip nodes) and both int.
    assert_eq!(
        pseudo_var_value_types(&r.module),
        vec![Type::Int, Type::Int],
        "both `stored` gates must be int"
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
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    // Flattened: no residual child modules.
    assert!(
        r.module.chips.is_empty(),
        "@flat must leave no child module: {:?}",
        r.module.chips.len()
    );
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "flat generic chip must monomorphize storage to int + vector, got {vals:?}"
    );
}

#[test]
fn generic_chip_const_arg_keys_apart_by_type() {
    // A generic chip with a foldable CONST arg (`k: int = 7`) shared by two
    // instantiations at DIFFERENT `T`. Both fold the same literal at the same
    // index, so the const-fold `template_key` suffix (`\x01 1:Int(7)`) is
    // identical — only the monomorph key BASE (`Pair<Int>` vs `Pair<Vector>`)
    // tells them apart. This is the sole test that exercises the const-fold key
    // guard (`fold_key = key.clone()`): reverting that base to the bare name
    // makes these two monomorphs collide onto one grid, and only this test
    // catches it. `v` is a var arg (wired, not folded); `k` is the const.
    let src = "chip Pair<T>(v: T, k: int) -> (r: T) { var stored: T = v\n out r = stored }\n\
               in go: exec\nin a: int\nin b: vector\n\
               on go {\n  let x = Pair(a, 7)\n  let y = Pair(b, 7)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let keys = chip_template_keys(&r.module);
    assert_eq!(
        keys.len(),
        2,
        "same-const/different-type instances must be TWO distinct grids, got {keys:?}"
    );
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "the two `stored` gates must be concrete int + vector, got {vals:?}"
    );
}

#[test]
fn generic_chip_multi_type_param_keys_by_param_order() {
    // Two type params instantiated with the SAME pair of concrete types in
    // swapped order (`Two<int, vector>` vs `Two<vector, int>`) must key apart —
    // the monomorph key renders params in declaration order, so it is
    // order-sensitive and deterministic. Two distinct grids, and each stores its
    // params at the right per-instance types.
    let src = "chip Two<T, U>(a: T, b: U) -> (x: T, y: U) { var sa: T = a\n var sb: U = b\n\
               out x = sa\n out y = sb }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let p = Two(n, vec)\n  let q = Two(vec, n)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let keys = chip_template_keys(&r.module);
    assert_eq!(
        keys.len(),
        2,
        "Two<int,vector> and Two<vector,int> must be two distinct templates, got {keys:?}"
    );
    // Each instance stores one int + one vector; two instances -> two of each.
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Int, Type::Vector, Type::Vector],
        "both instances must store their params at the right per-type, got {vals:?}"
    );
}

#[test]
fn nested_generic_chip_monomorphizes_to_two_grids() {
    // A generic chip instantiated INSIDE another (non-generic) chip's body must
    // still monomorphize per type: the two `Boxed` grids live nested under
    // `Outer`'s child module, so this only holds if the monomorph keying (and
    // the recursive `chip_template_keys` walk) sees through the nesting.
    let src = "chip Boxed<T>(v: T) -> (r: T) { var stored: T = v\n out r = stored }\n\
               chip Outer(n: int, vec: vector) -> (a: int, b: vector) {\n\
               let x = Boxed(n)\n  let y = Boxed(vec)\n  out a = x.r\n  out b = y.r\n}\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go {\n  let o = Outer(n, vec)\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    // Distinct keys across the whole tree: `Outer` + the two `Boxed` monomorphs.
    let keys = chip_template_keys(&r.module);
    let boxed_keys: Vec<_> = keys.iter().filter(|k| k.starts_with("Boxed<")).collect();
    assert_eq!(
        boxed_keys.len(),
        2,
        "nested Boxed<int> + Boxed<vector> must be two distinct grids, got {keys:?}"
    );
    // The nested storage gates come out concrete int + vector.
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "nested `stored` gates must monomorphize to int + vector, got {vals:?}"
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
    let r = compile(src);
    assert_no_errors(&r);
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
fn self_receiver_matches_plain_call() {
    // `a.dist(b)` and `dist(a, b)` must lower to the same shape: exactly one
    // VecDotProduct gate either way (the receiver is just positional arg 0).
    let method = compile(
        "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
         in a: vector\nin b: vector\nin go: exec\non go { let d = a.dist(b) }\n",
    );
    let plain = compile(
        "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
         in a: vector\nin b: vector\nin go: exec\non go { let d = dist(a, b) }\n",
    );
    assert_eq!(
        gate_output_types(&method.module, "VecDotProduct").len(),
        gate_output_types(&plain.module, "VecDotProduct").len(),
        "method-call and plain-call desugar must emit the same VecDotProduct count"
    );
}

#[test]
fn generic_self_receiver_infers_type_from_receiver() {
    // A generic `self`-mod: the receiver drives `T`. Called on an int and on a
    // vector, each inline monomorphizes its `stored: T` gate to the concrete
    // receiver type — proving `T` is inferred THROUGH the receiver.
    let src = "mod boxed<T>(self: T) -> T { static var stored: T = self\n return stored }\n\
               in n: int\nin vec: vector\nin go: exec\n\
               on go {\n  let a = n.boxed()\n  let b = vec.boxed()\n}\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed for a monomorphized generic self-mod");
    assert!(!any_param_port(&r.module), "no Type::Param may survive");
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "the receiver must drive T; got {vals:?}"
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
    let tc = typecheck(&parse(src, "test").ast, "test");
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS036"),
        "a non-self method call must be WS036; got {:?}",
        tc.diagnostics
    );
    let r = compile(src);
    assert!(
        gate_output_types(&r.module, "VecDotProduct").is_empty(),
        "a non-self mod must not be inlined as a method call"
    );
}

// ---------- container-var receiver self-mods (Bug A regression) ----------

#[test]
fn container_array_self_receiver_desugars_to_mod_call() {
    // A self-mod whose receiver is an array VAR must NOT be hijacked by the
    // array-method lowering branch. Before the `is_array_method` gate was added
    // there, `arr.firstOr(9)` (firstOr NOT an array method) routed into
    // `lower_array_method` → `_Unsupported`, diverging from typecheck. It must
    // desugar to `firstOr(arr, 9)` and inline the body's real gates.
    let method = "mod firstOr(self: int[], fallback: int) -> int { let n = self.length()\n return n + fallback }\n\
                  var arr: int[] = [1, 2, 3]\nvar rr: int = 0\nin go: exec\non go { rr = arr.firstOr(9) }\n";
    let plain = "mod firstOr(self: int[], fallback: int) -> int { let n = self.length()\n return n + fallback }\n\
                 var arr: int[] = [1, 2, 3]\nvar rr: int = 0\nin go: exec\non go { rr = firstOr(arr, 9) }\n";
    let rm = compile(method);
    assert_no_errors(&rm);
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
    let r = compile(src);
    assert_no_errors(&r);
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
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed for a generic container self-mod");
    assert!(!any_param_port(&r.module), "no Type::Param may survive");
    assert!(
        gate_output_types(&r.module, "Unsupported").is_empty(),
        "a generic container self-mod must not lower to _Unsupported"
    );
    assert_eq!(
        gate_count(&r, "BrickComponentType_WireGraph_Exec_ArrayVar_GetLength"),
        2,
        "each monomorph must inline its own GetLength gate"
    );
}

#[test]
fn compound_arg_forwarded_into_nested_generic_call_uses_monomorph() {
    // Whole-branch review Critical: a COMPOUND arg (`a + b`, an if-expr, a
    // nested generic call) forwarded into another generic call from INSIDE a
    // generic body must monomorphize the inner call to the CURRENT type — not
    // the stale last-mask-member `type_of_expr` holds (`Numeric` → Color). Here
    // `outer<int>` forwards `a + b` (int) and `id(a)` (int) into `Box`, whose
    // `stored: U` gate must come out INT, never Color/Vector.
    let src = "chip Box<U>(x: U) -> (r: U) { var stored: U = x\n out r = stored }\n\
               mod id<V>(x: V) -> V { return x }\n\
               mod outer<T: Numeric>(a: T, b: T) -> T {\n\
                 let p = Box(a + b)\n  let q = Box(id(a))\n  return p }\n\
               in go: exec\nin n: int\n\
               on go { let z = outer(n, n) }\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    // Every Box `stored` gate is int — never the stale Numeric-mask Color/Vector.
    let vals = pseudo_var_value_types(&r.module);
    assert!(
        !vals.is_empty() && vals.iter().all(|t| *t == Type::Int),
        "forwarded-compound-arg Box monomorphs must all be int, got {vals:?}"
    );
}

#[test]
fn explicit_type_args_pin_return_only_type_param() {
    // `zero<T>()` has `T` only in the return, so inference can't derive it —
    // an explicit type argument is the ONLY way to call it. `zero<int>()` vs
    // `zero<vector>()` must monomorphize the `z: T` storage to int / vector.
    let src = "mod zero<T: Numeric>() -> T { static var z: T = z\n return z }\n\
               in go: exec\nin n: int\nin vec: vector\n\
               on go { let a = zero<int>()\n let b = zero<vector>() }\n";
    let r = compile(src);
    assert_no_errors(&r);
    emit_ok(&r).expect("emit must succeed");
    assert!(!any_param_port(&r.module));
    let mut vals = pseudo_var_value_types(&r.module);
    vals.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(
        vals,
        vec![Type::Int, Type::Vector],
        "explicit type args must monomorphize the two `z` gates to int + vector, got {vals:?}"
    );
}

#[test]
fn explicit_type_args_error_cases() {
    let errs = |s: &str| {
        crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
            .diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.to_string())
            .collect::<Vec<_>>()
    };
    // out-of-mask: string isn't in Numeric
    assert!(errs("mod z<T: Numeric>() -> T { static var v: T = v\n return v }\nout r: int = z<string>()\n")
        .contains(&"WS033".to_string()));
    // arity mismatch
    assert!(errs("mod p<T>(a: T) -> T { return a }\nin x: int\nout r: int = p<int, float>(x)\n")
        .contains(&"WS033".to_string()));
    // type arguments on a non-generic function
    assert!(errs("mod f(a: int) -> int { return a }\nin x: int\nout r: int = f<int>(x)\n")
        .contains(&"WS033".to_string()));
}

#[test]
fn type_args_do_not_break_comparison_parsing() {
    // The `<...>(` type-argument form must NOT hijack a `<`/`>` comparison. A
    // plain `a < b` and a `(a < b)` chain still parse + typecheck as comparisons.
    let ok = "in a: int\nin b: int\nin c: int\nout r: bool = (a < b)\nout s: bool = a < b\n";
    let d = crate::typecheck::typecheck(&crate::parser::parse(ok, "t").ast, "t");
    assert!(
        d.diagnostics.iter().all(|x| x.severity != crate::diagnostic::Severity::Error),
        "comparison must still parse+check clean: {:?}",
        d.diagnostics
    );
}

#[test]
fn non_callable_call_errors_ws038() {
    let errs = |s: &str| {
        crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
            .diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.to_string())
            .collect::<Vec<_>>()
    };
    // Calling a non-callable value (a var / let / array / input) is a hard
    // error, not a silent `_Unsupported` gate reading 0. An index typo is the
    // common trigger.
    let typo = "on CharacterSpawned(ch) {\n  var xs: int[] = [1, 2, 3]\n  let r = xs(0)\n}\n";
    assert!(errs(typo).contains(&"WS038".to_string()), "xs(i) must be WS038: {:?}", errs(typo));
    // `a < b > (c)` parses as an explicit-type-argument call on `a`; since `a`
    // is not callable it now surfaces as a clear WS038 instead of a silently
    // dropped comparison. (`a < b` / `(a < b)` without a trailing `(` stay
    // comparisons — see `type_args_do_not_break_comparison_parsing`.)
    let misparse = "on CharacterSpawned(ch) {\n  let a = 1\n  let b = 2\n  let c = 3\n  let r = a < b > (c)\n}\n";
    assert!(errs(misparse).contains(&"WS038".to_string()), "a<b>(c) must surface as WS038: {:?}", errs(misparse));
    // A real callable is unaffected.
    let ok = "mod dbl(n: int) -> int { return n * 2 }\nin x: int\nstatic var rv: int = 0\nout r: int = rv\nin go: exec\non go { rv = dbl(x) }\n";
    assert!(errs(ok).is_empty(), "a real mod call must be clean: {:?}", errs(ok));
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
    let r = compile(&src);
    assert_no_errors(&r);
    emit_ok(&r).expect("builtins/types test program must emit");
}

#[test]
fn receiver_call_validates_receiver_type() {
    let errs = |s: &str| {
        crate::typecheck::typecheck(&crate::parser::parse(s, "t").ast, "t")
            .diagnostics
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.to_string())
            .collect::<Vec<_>>()
    };
    // A receiver whose type doesn't match `self` is a WS003 error — user mod/chip
    // calls now coerce each arg (incl. the receiver bound as arg 0) against its
    // parameter, exactly like the wire layer's PortsAreCompatible.
    let bad = "mod lensq(self: vector, k: float) -> float { return self.Dot(self) * k }\n\
               in x: int\nstatic var rv: float = 0.0\nout r: float = rv\nin go: exec\non go { rv = x.lensq(2.0) }\n";
    assert!(errs(bad).contains(&"WS003".to_string()), "int receiver on self:vector must error: {:?}", errs(bad));
    // The same mismatch on a plain (non-receiver) call is caught too.
    let bad_plain = "mod lensq(self: vector, k: float) -> float { return self.Dot(self) * k }\n\
                     in x: int\nstatic var rv: float = 0.0\nout r: float = rv\nin go: exec\non go { rv = lensq(x, 2.0) }\n";
    assert!(errs(bad_plain).contains(&"WS003".to_string()));
    // A receiver that WIDENS to `self`'s type (character -> entity) is accepted —
    // the coercion walks the applicable type options, it isn't an exact match.
    let widen = "mod who(self: entity) -> string { return self.GetDisplayName() }\n\
                 in c: character\nstatic var rv: string = \"\"\nout r: string = rv\nin go: exec\non go { rv = c.who() }\n";
    assert!(errs(widen).is_empty(), "character receiver on self:entity must widen cleanly: {:?}", errs(widen));
}

#[test]
fn type_args_on_builtin_warn() {
    let r = crate::typecheck::typecheck(
        &crate::parser::parse("in a: float\nin b: float\nout r: float = Blend<int>(a, b, 0.5)\n", "t").ast,
        "t",
    );
    assert!(
        r.diagnostics.iter().any(|d| d.code == "WS037"
            && d.severity == crate::diagnostic::Severity::Warning),
        "explicit type args on a builtin must warn WS037: {:?}",
        r.diagnostics
    );
    // A plain builtin call does NOT warn.
    let r2 = crate::typecheck::typecheck(
        &crate::parser::parse("in a: float\nin b: float\nout r: float = Blend(a, b, 0.5)\n", "t").ast,
        "t",
    );
    assert!(!r2.diagnostics.iter().any(|d| d.code == "WS037"));
}

// ---- Task 10: max generic parameters / cartesian body-check cap
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
    let r = compile(src);
    assert_no_errors(&r);
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
    let r = compile(src);
    assert_no_errors(&r);
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
    let r = compile(src);
    assert_no_errors(&r);
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
    let tc = typecheck(&parse(src, "test").ast, "test");
    assert!(
        tc.diagnostics.iter().any(|d| d.code == "WS011"),
        "a single-param-only op invalid for some Numeric member must be \
         caught even when the combo check is truncated, got {:?}",
        tc.diagnostics.iter().map(|d| d.code.to_string()).collect::<Vec<_>>()
    );
}
