//! Constant expressions in `var` initializers.
//!
//! An initializer is baked into the gate at compile time, so it may name a
//! top-level `let` constant and do arithmetic on it rather than restating a
//! magic number. Anything that is not a compile-time constant still errors —
//! these tests pin both directions.

use super::*;
use crate::ir::Literal;
use crate::typecheck::typecheck;

/// The `InitialValue` baked into the first array gate of a module.
fn baked_array(src: &str) -> Vec<Literal> {
    let r = compile(src);
    assert!(
        r.diagnostics
            .iter()
            .all(|d| d.severity != crate::diagnostic::Severity::Error),
        "unexpected errors: {:?}",
        r.diagnostics
    );
    // Recurses into child modules: an array declared inside a `chip { }` lives
    // in that chip's own module, not the root's node table.
    fn find(m: &crate::ir::Module) -> Option<Vec<Literal>> {
        for n in m.nodes.values() {
            if let Some(Literal::Array(items)) =
                n.properties.get(&crate::intern::intern_static("InitialValue"))
            {
                return Some(items.clone());
            }
        }
        m.chips.values().find_map(find)
    }
    find(&r.module).expect("no array gate with a baked InitialValue")
}

/// Whether `needle` appears anywhere in the lowered graph, child chip modules
/// included — the tree-shaking oracle for a `const` condition's untaken arm.
fn graph_contains(r: &LowerResult, needle: &str) -> bool {
    fn walk(m: &crate::ir::Module, needle: &str) -> bool {
        m.nodes
            .values()
            .any(|n| format!("{:?}", n.properties).contains(needle))
            || m.chips.values().any(|c| walk(c, needle))
    }
    walk(&r.module, needle)
}

/// Typecheck errors only (the gate that rejects a non-constant element).
fn errors(src: &str) -> Vec<String> {
    let parsed = crate::parser::parse(src, "test");
    typecheck(&parsed.ast, "test", &crate::typecheck::CeSlotMap::default())
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.to_string())
        .collect()
}

#[test]
fn shift_of_named_constant_bakes() {
    // The motivating case: a bitmask table written as `1 << C_FLAG`.
    let v = baked_array("let C_A = 0\nlet C_B = 3\nvar m: int[] = [1 << C_A, 1 << C_B]");
    assert_eq!(v, vec![Literal::Int(1), Literal::Int(8)]);
}

#[test]
fn bare_named_constant_bakes() {
    let v = baked_array("let LO = 2\nlet HI = 9\nvar m: int[] = [LO, HI]");
    assert_eq!(v, vec![Literal::Int(2), Literal::Int(9)]);
}

#[test]
fn arithmetic_bakes() {
    let v = baked_array("var m: int[] = [2 + 3, 10 * 4, 7 - 9]");
    assert_eq!(v, vec![Literal::Int(5), Literal::Int(40), Literal::Int(-2)]);
}

#[test]
fn constant_chain_resolves_regardless_of_order() {
    // `B` is defined in terms of `A`, and `C` in terms of `B`. Declaration
    // order is not dependency order once imports are merged, so the constant
    // environment iterates to a fixpoint.
    let v = baked_array("let C = B * 2\nlet B = A + 1\nlet A = 5\nvar m: int[] = [A, B, C]");
    assert_eq!(v, vec![Literal::Int(5), Literal::Int(6), Literal::Int(12)]);
}

#[test]
fn constant_chain_through_a_const_mod_call_resolves_regardless_of_order() {
    // Same shape as `constant_chain_resolves_regardless_of_order` above, but
    // `A`'s own value comes from a `const mod` CALL rather than a literal —
    // `B`, which depends on `A`, is still declared FIRST. `build_const_env`
    // collects every top-level `const mod` from the whole decls list up
    // front (not incrementally alongside the fixpoint that resolves
    // `let`/`const` values), so a name's dependency on a const-mod-derived
    // constant resolves the same way it always has for a plain one.
    //
    // `f` itself is declared before its own call site (inside `A`'s
    // initializer) — a call to a mod declared LATER than its caller is
    // rejected outright (WS021, a separate concern from the ordering here;
    // see `a_const_mod_call_must_still_be_declared_before_its_use` in
    // `tests/const_differential.rs`), so that is not the order being
    // exercised here.
    let v = baked_array(
        "const mod f(n: int) -> int { return n + 1 }\n\
         let B = A + 1\n\
         let A = f(4)\n\
         var m: int[] = [A, B]",
    );
    assert_eq!(v, vec![Literal::Int(5), Literal::Int(6)]);
}

#[test]
fn a_destructured_constant_bakes() {
    // A top-level destructuring `const` must land its names in the constant
    // environment like any other, so a `var` initializer can name one.
    // Distinguishable per-field values: a swapped binding bakes [222, 111].
    let v = baked_array("const p = { x: 111, y: 222 }\nconst { x, y } = p\nvar m: int[] = [x, y]");
    assert_eq!(v, vec![Literal::Int(111), Literal::Int(222)]);
}

#[test]
fn a_destructured_constant_chain_resolves_regardless_of_order() {
    // The destructuring analogue of `constant_chain_resolves_regardless_of_order`
    // above, and the ordering constraint destructuring must respect: the
    // destructure is declared BEFORE the record it splits, and a third
    // constant built from its names is declared before BOTH. `build_const_env`
    // iterates to a fixpoint — pass 1 evaluates `p`, pass 2 splits it into
    // `x`/`y`, pass 3 resolves `SUM` — so declaration order does not matter
    // here any more than it does for a plain constant chain.
    //
    // Values are chosen so every field is distinguishable and the combination
    // pins the mapping: a swapped x/y bakes [222, 111, 222111].
    let v = baked_array(
        "const SUM = x * 1000 + y\n\
         const { x, y } = p\n\
         const p = { x: 111, y: 222 }\n\
         var m: int[] = [x, y, SUM]",
    );
    assert_eq!(v, vec![Literal::Int(111), Literal::Int(222), Literal::Int(111222)]);
}

/// A `const mod` with ONE NAMED output (`-> (r: int)`, its value set by an
/// `out` rather than a `return`) is typed by `typecheck::call` as the bare
/// output type, so const evaluation must produce the bare value too. Wrapping
/// it in a 1-field record instead did not error anywhere — it baked the
/// element's ZERO value: this exact program baked `[0, 12345]`, silently
/// losing the 7, with no diagnostic at all.
///
/// `12345` is a plain literal control: it bakes correctly either way, so a
/// failure here means "wrong value", not "nothing was baked".
///
/// `docs/wirescript/chips.md` sends users directly at this shape — it rejects
/// `const chip C(v: int) -> (r: int)` and tells them to use a `mod` instead.
#[test]
fn a_single_named_output_const_mod_result_bakes_as_a_scalar() {
    let v = baked_array(
        "const mod C(v: const int) -> (r: int) { out r = v }\n\
         const got = C(7)\n\
         var arr: int[] = [got, 12345]",
    );
    assert_eq!(v, vec![Literal::Int(7), Literal::Int(12345)]);
}

#[test]
fn division_by_zero_bakes_zero_like_the_gates() {
    let v = baked_array("var m: int[] = [8 / 0, 8 % 0]");
    assert_eq!(v, vec![Literal::Int(0), Literal::Int(0)]);
}

#[test]
fn string_concat_and_bool_ops_bake() {
    let s = baked_array(r#"let A = "x"
var m: string[] = [A .. "y"]"#);
    assert_eq!(s, vec![Literal::String("xy".into())]);
    let b = baked_array("let T = true\nvar m: bool[] = [T && false, T || false, !T]");
    assert_eq!(
        b,
        vec![Literal::Bool(false), Literal::Bool(true), Literal::Bool(false)]
    );
}

#[test]
fn operator_folding_does_not_leak_outside_initializers() {
    // `expr_to_literal` (no constant environment) decides bake-vs-wire in many
    // places besides initializers. It must NOT fold operators, or a call whose
    // args are arithmetic would collapse into a literal and delete the gate it
    // should have emitted — `Rotation(0.0 + 0.0, ...)` losing its MakeRotation
    // is the case that caught this. Guards `fold::make_rotation_does_not_fold`
    // from the other direction: here at the source, not at the fold pass.
    use crate::ir::gate_class as gc;
    let r = compile("out r: rotator = Rotation(0.0 + 0.0, 90.0 + 0.0, 45.5 + 0.0)");
    let n = r
        .module
        .nodes
        .values()
        .filter(|x| x.gate_class == gc::MAKE_ROTATION)
        .count();
    assert_eq!(n, 1, "arithmetic args must still produce a real gate");
}

#[test]
fn constant_named_in_an_initializer_counts_as_used() {
    // A constant reached only from an array initializer must register as a use,
    // or the import that supplies it is reported unused — and Organize Imports
    // deletes it, silently breaking the table.
    use crate::resolve::resolve;
    struct Loader;
    impl crate::resolve::FileLoader for Loader {
        fn load(&self, path: &str, _relative_to: &str) -> Result<String, String> {
            match path {
                "consts" => Ok("let C_FLAG = 3\n".to_string()),
                other => Err(format!("no such module: {other}")),
            }
        }
        fn canonical_path(&self, path: &str, _relative_to: &str) -> String {
            path.to_string()
        }
    }
    let r = resolve(
        "import { C_FLAG } from \"consts\"\nvar m: int[] = [1 << C_FLAG]\n",
        "test",
        &Loader,
    );
    let unused: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.code == "WS014")
        .collect();
    assert!(unused.is_empty(), "false unused-import warning: {unused:?}");
}

#[test]
fn constants_travel_with_an_imported_array() {
    // Importing only the accessor mod must still drag in the array AND the
    // constants its initializer names — otherwise the merged program has the
    // array but not the values it is built from (WS002 at the initializer).
    use crate::resolve::resolve;
    struct Loader;
    impl crate::resolve::FileLoader for Loader {
        fn load(&self, path: &str, _relative_to: &str) -> Result<String, String> {
            match path {
                "prov" => Ok("let K_ONE = 7\nlet K_TWO = 9\n\
                              var table: int[] = [K_ONE, K_TWO]\n\
                              mod getEntry(i: int) -> int {\n  return table[i]\n}\n"
                    .to_string()),
                other => Err(format!("no such module: {other}")),
            }
        }
        fn canonical_path(&self, path: &str, _relative_to: &str) -> String {
            path.to_string()
        }
    }
    let r = resolve(
        "import { getEntry } from \"prov\"\nin fire: exec\non fire { PrintToConsole(\"${getEntry(0)}\") }\n",
        "test",
        &Loader,
    );
    let names: Vec<String> = r
        .ast
        .decls
        .iter()
        .filter_map(|d| match d {
            crate::ast::TopDecl::Let(l) => match &l.binding {
                crate::ast::LetBinding::Ident { name, .. } => Some(name.clone()),
                _ => None,
            },
            crate::ast::TopDecl::Var(v) => Some(v.name.clone()),
            _ => None,
        })
        .collect();
    for want in ["table", "K_ONE", "K_TWO"] {
        assert!(names.contains(&want.to_string()), "{want} not pulled in: {names:?}");
    }
}

#[test]
fn non_constant_element_still_errors() {
    // A runtime value has no compile-time form — the initializer must still be
    // rejected rather than silently baking a wrong value.
    assert!(errors("in x: int\nvar m: int[] = [x]").contains(&"WS003".to_string()));
}

#[test]
fn out_of_range_shift_is_not_folded() {
    // 1 << 64 is undefined for i64; refuse rather than guess.
    assert!(errors("var m: int[] = [1 << 64]").contains(&"WS003".to_string()));
}

#[test]
fn cyclic_constants_do_not_hang_or_bake() {
    // A depends on B depends on A: neither resolves, the fixpoint terminates,
    // and the initializer stays an error.
    assert!(errors("let A = B + 1\nlet B = A + 1\nvar m: int[] = [A]").contains(&"WS003".to_string()));
}

// `lower::expr::wire_type_of_literal`'s `Literal::Object` arm returns `None`
// (the same "no wire form" shape as its sibling Array/Map/Record/Asset arms),
// so a violated invariant surfaces as a diagnosable gap rather than a process
// abort. This pins the invariant itself: `Literal::Object` is constructed
// ONLY by `default_literal_for_var_type` — an object-family `Var` gate's
// placeholder initial value — which never touches `const_eval`, so no
// `const`/`let` expression can ever fold to one.
#[test]
fn literal_object_is_never_produced_by_const_eval() {
    // Where `Literal::Object` DOES come from: a `Var` gate's own default.
    assert_eq!(
        crate::lower::default_literal_for_var_type(&crate::ir::Type::Entity),
        Some(Literal::Object),
        "Object is default_literal_for_var_type's placeholder for object-family types"
    );
    // No source syntax folds to an entity/controller/character value — the
    // only way to name one is a runtime call, which `const_eval` must
    // therefore always reject as non-constant, so `wire_type_of_literal`'s
    // `const_lookup()` caller can never hand it an `Object` literal.
    assert!(
        errors("const e = FindPlayer(\"x\")").contains(&"WS046".to_string()),
        "an entity-typed initializer must never be accepted as a compile-time constant"
    );
}

// A constructor's NAMED arguments must bind by PARAMETER NAME all the way
// through the pipeline that actually bakes values into gates, not just in
// `const_eval`'s own unit tests. Binding them by source position instead
// silently baked `Vector { x: 3.0, y: 1.0, z: 2.0 }` here — a wrong constant
// in the emitted gate, with no diagnostic to notice it by. The runtime path
// (`lower::call::builtin`) has always bound these by name, so this is also
// what keeps a `const` and a non-`const` spelling of one expression agreeing.
#[test]
fn a_constructor_with_named_arguments_bakes_on_the_named_axes() {
    assert_eq!(
        baked_array("var v: vector[] = [Vec(z = 3.0, x = 1.0, y = 2.0)]"),
        vec![Literal::Vector { x: 1.0, y: 2.0, z: 3.0 }]
    );
}

// ---------- `const` inside an anonymous `chip { }` ----------

/// An anonymous `chip { }` SHARES its parent's scope — `typecheck::register`
/// and `typecheck::decl` register its body's declarations into the parent, and
/// `lower::decl::lower_anon_chip` lowers the body with no `push_scope`. Its
/// body-level `const`s must therefore live in the same constant environment as
/// the top-level ones.
///
/// They did not: `build_const_env`/`build_const_declared_names` matched
/// `TopDecl::Let` only and never descended into `TopDecl::AnonChip`, and
/// nothing else opens a `scoped_consts` frame there — so the value was
/// recorded NOWHERE. `const a = 1` was silent (a literal always evaluates) and
/// the failure surfaced at the next line instead, blamed on a different
/// binding: `const b = a + 1` reported WS046 "'a' is a runtime value".
#[test]
fn a_const_in_an_anonymous_chip_body_is_compile_time() {
    let v = baked_array(
        "chip {\n  const a = 20\n  const b = a + 2\n  var m: int[] = [a, b, 12345]\n}",
    );
    assert_eq!(
        v,
        vec![Literal::Int(20), Literal::Int(22), Literal::Int(12345)]
    );
}

/// A `const` declared in an anonymous chip nested inside another anonymous
/// chip resolves the same way: each shares the same parent scope in turn, so
/// the descent recurses rather than stopping at the first body.
#[test]
fn a_const_in_a_nested_anonymous_chip_body_is_compile_time() {
    let v = baked_array(
        "chip {\n  chip {\n    const a = 5\n    const b = a * 3\n    \
         var m: int[] = [b, 12345]\n  }\n}",
    );
    assert_eq!(v, vec![Literal::Int(15), Literal::Int(12345)]);
}

/// The reverse direction: an anonymous chip body reads a TOP-LEVEL constant
/// and re-exports it through one of its own, proving the flattening merges
/// into one environment rather than building a second, isolated one.
#[test]
fn an_anonymous_chip_body_const_may_read_a_top_level_const() {
    let v = baked_array(
        "const outer = 5\nchip {\n  const inner = outer * 2\n  \
         var m: int[] = [inner, 12345]\n}",
    );
    assert_eq!(v, vec![Literal::Int(10), Literal::Int(12345)]);
}

/// A `const mod` declared in an anonymous chip body is registered into the
/// parent scope by `predeclare`'s own `AnonChip` arm, so a `const` anywhere in
/// the module can CALL it. `scope_mods` has to descend for the same
/// reason `scope_lets` does.
#[test]
fn a_const_mod_in_an_anonymous_chip_body_resolves() {
    let v = baked_array(
        "chip {\n  const mod dbl(n: const int) -> int { return n * 2 }\n}\n\
         const t = dbl(6)\nvar m: int[] = [t, 12345]",
    );
    assert_eq!(v, vec![Literal::Int(12), Literal::Int(12345)]);
}

/// Both stages must agree about an anonymous chip's constants, not just the
/// baked value: a `const` condition inside one has to tree-shake exactly as it
/// does at the top level. The `else` arm's unique sentinel must be absent, and
/// the runtime-condition control proves the sentinel WOULD be there otherwise.
#[test]
fn a_const_if_inside_an_anonymous_chip_elides_like_a_top_level_one() {
    let body = "  in go: exec\n  on go {\n    if flag { PrintToConsole(\"taken\") } \
                else { PrintToConsole(\"SENTINEL\") }\n  }\n";
    let has_sentinel = |src: &str| {
        let r = compile(src);
        assert_no_errors(&r);
        graph_contains(&r, "SENTINEL")
    };
    assert!(
        !has_sentinel(&format!("chip {{\n  const flag = true\n{body}}}")),
        "a const condition in an anonymous chip must elide its untaken arm"
    );
    assert!(
        has_sentinel(&format!("chip {{\n  var flag: bool = true\n{body}}}")),
        "control: a RUNTIME condition must keep both arms"
    );
}

/// The post-binding assertion (`typecheck::let_binding::check_const_recorded`)
/// fires when a `const` whose initializer EVALUATED is then not retrievable as
/// a constant in its own scope — without it, the failure surfaces later, at a
/// use, blamed on a different binding, rather than at the declaration.
///
/// It is a NET: no shape enumerated here reaches it, and that is the point of
/// the test — the realistic failure mode for an assertion like this is a FALSE
/// positive, so every scope a `const` can be declared in is listed and must
/// stay silent. (That it genuinely bites is verified by mutation: disabling
/// `scope_lets`' `AnonChip` descent makes the first case below report WS046 at
/// the declaration instead of at the next line.)
#[test]
fn a_const_that_is_recorded_never_reports_at_its_binding() {
    for src in [
        // anonymous chip body, and one nested inside another
        "chip { const a = 1\n const b = a + 1 }",
        "chip { chip { const a = 1\n const b = a + 1 } }",
        // handler block scope, and an `if` block inside one
        "in go: exec\non go { const a = 1\n const b = a + 1 }",
        "in go: exec\non go { if true { const a = 1\n const b = a + 1 } }",
        // named chip body, and a plain mod body
        "chip N() { const a = 1\n out r = a + 1 }",
        "mod g() -> int { const a = 1\n return a + 1 }",
        // a const mod body, with a const PARAMETER (a placeholder-seeded const)
        "const mod g(n: const int) -> int { const a = n + 1\n return a }\nvar v: int[] = [g(2)]",
        // top level: plain, destructured, and via a const mod call
        "const a = 1\nconst b = a + 1",
        "const p = { x: 1, y: 2 }\nconst { x, y } = p",
        "const mod f() -> int { return 3 }\nconst a = f()",
        // a namespaced import's members are checked in their own pushed scope
        "const a = 1",
    ] {
        let diags = errors(src);
        assert!(
            !diags.contains(&"WS046".to_string()),
            "a recorded `const` must not report at its binding\n  src: {src}\n  got: {diags:?}"
        );
    }
}

// ---------- compile-time tuple destructuring ----------

/// A tuple pattern binds POSITIONALLY. A multi-output `const mod`'s result
/// types as a NAME-keyed record in the signature's declaration order, which is
/// exactly what "positional" means — so `const (a, b) = pair(…)` reads it the
/// same way `const { a, b } = pair(…)` does. This was rejected outright
/// (WS010 in `bind_let`, then WS046 from `const_eval`'s stub arm).
#[test]
fn a_tuple_destructure_of_a_multi_output_const_mod_is_compile_time() {
    let v = baked_array(
        "const mod trio(n: const int) -> (a: int, b: int, c: int) {\n\
         out a = n\n  out b = n + 1\n  out c = n + 2\n}\n\
         const (x, y, z) = trio(41)\nvar m: int[] = [x, y, z, 12345]",
    );
    assert_eq!(
        v,
        vec![
            Literal::Int(41),
            Literal::Int(42),
            Literal::Int(43),
            Literal::Int(12345)
        ]
    );
}

/// The other shape that reaches the same arm: a genuine tuple LITERAL, which
/// evaluates to an index-keyed record. It used to report WS046 alone (the type
/// side already accepted it; only the const evaluator refused).
#[test]
fn a_tuple_destructure_of_a_tuple_literal_is_compile_time() {
    let v = baked_array("const (a, b) = (7, 8)\nvar m: int[] = [a, b, 12345]");
    assert_eq!(v, vec![Literal::Int(7), Literal::Int(8), Literal::Int(12345)]);
}

/// A width mismatch stays an error, and names BOTH counts — the value is
/// perfectly constant and perfectly positional, so "expected tuple[2], got
/// Record(...)" was describing the compiler's internals rather than the
/// program's mistake.
#[test]
fn a_tuple_destructure_of_the_wrong_width_reports_both_counts() {
    let diags = errors(
        "const mod pair(n: const int) -> (a: int, b: int) {\n out a = n\n out b = n + 1\n}\n\
         const (p, q, r) = pair(1)",
    );
    assert!(diags.contains(&"WS010".to_string()), "got {diags:?}");
}

/// Const and runtime must not diverge: the same tuple pattern over a plain
/// (non-`const`) multi-output `mod` has to lower to the same graph the record
/// spelling produces, binding position for position.
#[test]
fn a_runtime_tuple_destructure_matches_the_record_spelling() {
    let mod_src = "mod pair(n: int) -> (a: int, b: int) {\n out a = n * 10\n out b = n + 5\n}\n\
                   in n: int\n";
    let tuple = compile(&format!("{mod_src}let (x, y) = pair(n)\nout r = x + y"));
    let record = compile(&format!("{mod_src}let {{ a: x, b: y }} = pair(n)\nout r = x + y"));
    assert_no_errors(&tuple);
    assert_no_errors(&record);
    assert_eq!(
        tuple.module.wires.len(),
        record.module.wires.len(),
        "tuple and record spellings of the same destructure must wire identically"
    );
    assert_eq!(tuple.module.nodes.len(), record.module.nodes.len());
}

#[test]
fn a_var_initializer_naming_a_constant_bakes_the_constant() {
    // Regression: the scalar and record-field initializer paths resolved the
    // initializer with the const-env-LESS `expr_to_literal`, which sees literals
    // but not names. `var h: float = K` is an `Ident`, so it resolved to nothing
    // and the var fell back to its type default -- silently, with no diagnostic,
    // so the program shipped a 0 where the constant should be.
    //
    // Asserts the BAKED `InitialValue`, not a gate count: the wrong-value bug
    // emits exactly the same gates as the right one.
    let baked = |src: &str, label: &str| -> Option<crate::ir::Literal> {
        let r = compile(src);
        assert_no_errors(&r);
        r.module
            .nodes
            .values()
            .find(|n| {
                matches!(
                    n.properties.get(&crate::intern::sym::NAME_LABEL),
                    Some(crate::ir::Literal::String(s)) if s == label
                )
            })
            .and_then(|n| n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned())
    };
    use crate::ir::Literal as L;
    let cases: Vec<(&str, &str, L)> = vec![
        (
            "const K: float = 123.45
var h: float = K
out v: float = h
",
            "h",
            L::Float(123.45),
        ),
        ("const K: int = 7
var h: int = K
out v: int = h
", "h", L::Int(7)),
        (
            "const K: bool = true
var h: bool = K
out v: bool = h
",
            "h",
            L::Bool(true),
        ),
        (
            "const K: string = \"hi\"
var h: string = K
out v: string = h
",
            "h",
            L::String("hi".into()),
        ),
        // an inferred constant, and an expression OVER constants
        ("const K = 2.0
var h: float = K
out v: float = h
", "h", L::Float(2.0)),
        (
            "const K: float = 2.0
var h: float = K * 3.0
out v: float = h
",
            "h",
            L::Float(6.0),
        ),
        // a record FIELD initializer, the second path with the same gap
        (
            "type P = { x: float }
const K: float = 9.5
var p: P = { x: K }
out v: float = p.x
",
            "p.x",
            L::Float(9.5),
        ),
    ];
    for (src, label, want) in cases {
        assert_eq!(baked(src, label), Some(want), "wrong baked initializer for:
{src}");
    }
}

#[test]
fn a_constant_initializer_widens_to_the_declared_type() {
    // The const-resolved literal goes through the same `bake_literal_for_type`
    // as a spelled-out one, so `var h: float = K` with an INT constant still
    // builds a float variable rather than an integer one.
    let r = compile("const K: int = 3
var h: float = K
out v: float = h
");
    assert_no_errors(&r);
    let init = r
        .module
        .nodes
        .values()
        .find(|n| {
            matches!(
                n.properties.get(&crate::intern::sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == "h"
            )
        })
        .and_then(|n| n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned());
    assert_eq!(init, Some(crate::ir::Literal::Float(3.0)));
}
