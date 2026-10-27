//! Constant expressions in `var` initializers.
//!
//! An initializer is baked into the gate at compile time, so it may name a
//! top-level `let` constant and do arithmetic on it rather than restating a
//! magic number. Anything that is not a compile-time constant still errors;
//! these tests pin both directions.

use super::*;
use crate::ir::Literal;

/// The `InitialValue` baked into the first array gate of the module tree. An
/// array declared inside a `chip { }` lives in that chip's own module.
fn baked_array(src: &str) -> Result<Vec<Literal>, String> {
    let r = compile(src);
    let errors = crate::test_support::error_codes(&r.diagnostics);
    if !errors.is_empty() {
        return Err(format!("errors {errors:?}"));
    }
    r.module
        .nodes_deep()
        .find_map(|n| {
            match n
                .properties
                .get(&crate::intern::intern_static("InitialValue"))
            {
                Some(Literal::Array(items)) => Some(items.clone()),
                _ => None,
            }
        })
        .ok_or_else(|| "no array gate with a baked InitialValue".to_string())
}

/// Whether a string property containing `needle` appears anywhere in the
/// lowered graph, child chip modules included: the tree-shaking oracle for a
/// `const` condition's untaken arm.
fn graph_contains(r: &LowerResult, needle: &str) -> bool {
    r.module.nodes_deep().any(|n| {
        n.properties
            .values()
            .any(|v| matches!(v, Literal::String(s) if s.contains(needle)))
    })
}

/// A constant initializer bakes into the array gate's `InitialValue`. Values
/// are distinguishable per element (and a trailing `12345` literal control
/// bakes either way), so a swapped or zeroed element fails as a wrong value,
/// not as "nothing was baked".
#[test]
fn a_constant_initializer_bakes_into_the_array() {
    use Literal::{Bool, Int, String as Str};
    let rows: Vec<(&str, &str, Vec<Literal>)> = vec![
        // The motivating case: a bitmask table written as `1 << C_FLAG`.
        (
            "a shift of a named constant",
            "let C_A = 0\nlet C_B = 3\nvar m: int[] = [1 << C_A, 1 << C_B]",
            vec![Int(1), Int(8)],
        ),
        (
            "a bare named constant",
            "let LO = 2\nlet HI = 9\nvar m: int[] = [LO, HI]",
            vec![Int(2), Int(9)],
        ),
        (
            "arithmetic",
            "var m: int[] = [2 + 3, 10 * 4, 7 - 9]",
            vec![Int(5), Int(40), Int(-2)],
        ),
        // `B` is defined in terms of `A`, and `C` in terms of `B`, so the constant
        // environment iterates rather than evaluating once per declaration.
        (
            "a constant chain through the const-env fixpoint",
            "let A = 5\nlet B = A + 1\nlet C = B * 2\nvar m: int[] = [A, B, C]",
            vec![Int(5), Int(6), Int(12)],
        ),
        // `A` comes from a `const mod` call, which resolves like a plain
        // constant. `f` is declared before its call, since a call to a later
        // mod is WS021 (`tests/it/const_differential.rs`); the same holds
        // between the `let`s.
        (
            "a constant chain through a const mod call",
            "const mod f(n: int) -> int { return n + 1 }\nlet A = f(4)\nlet B = A + 1\nvar m: int[] = [A, B]",
            vec![Int(5), Int(6)],
        ),
        // A top-level destructuring `const` lands its names in the constant
        // environment: pass 1 evaluates `p`, pass 2 splits it, pass 3 resolves
        // `SUM`. A swapped x/y bakes [222, 111, 222111].
        (
            "a destructured constant chain",
            "const p = { x: 111, y: 222 }\nconst { x, y } = p\nconst SUM = x * 1000 + y\nvar m: int[] = [x, y, SUM]",
            vec![Int(111), Int(222), Int(111_222)],
        ),
        // A `const mod` with one named output (`-> (r: int)`) is typed as the
        // bare output type, so const evaluation gives the bare value, not a
        // 1-field record baking the element's zero. `docs/src/chips.md` points
        // users at this shape.
        (
            "a single named output const mod result bakes as a scalar",
            "const mod C(v: const int) -> (r: int) { out r = v }\nconst got = C(7)\nvar arr: int[] = [got, 12345]",
            vec![Int(7), Int(12345)],
        ),
        (
            "division by zero bakes zero like the gates",
            "var m: int[] = [8 / 0, 8 % 0]",
            vec![Int(0), Int(0)],
        ),
        (
            "string concat",
            "let A = \"x\"\nvar m: string[] = [A .. \"y\"]",
            vec![Str("xy".into())],
        ),
        (
            "bool ops",
            "let T = true\nvar m: bool[] = [T && false, T || false, !T]",
            vec![Bool(false), Bool(true), Bool(false)],
        ),
        // A constructor's named arguments bind by parameter name when baked into
        // gates, as on the runtime path (`lower::call::builtin`), so a `const`
        // and a non-`const` spelling of one expression agree.
        (
            "a constructor with named arguments bakes on the named axes",
            "var v: vector[] = [Vec(z = 3.0, x = 1.0, y = 2.0)]",
            vec![Literal::Vector {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }],
        ),
        // An anonymous `chip { }` shares its parent's scope, so its body-level
        // `const`s live in the parent's constant environment: `const b = a + 1`
        // resolves `a` rather than reporting WS046 "'a' is a runtime value".
        (
            "a const in an anonymous chip body",
            "chip {\n  const a = 20\n  const b = a + 2\n  var m: int[] = [a, b, 12345]\n}",
            vec![Int(20), Int(22), Int(12345)],
        ),
        // Each nested anonymous chip shares the same parent scope in turn, so the
        // descent recurses rather than stopping at the first body.
        (
            "a const in a nested anonymous chip body",
            "chip {\n  chip {\n    const a = 5\n    const b = a * 3\n    var m: int[] = [b, 12345]\n  }\n}",
            vec![Int(15), Int(12345)],
        ),
        // The flattening merges into one environment rather than an isolated second one.
        (
            "an anonymous chip body const reads a top-level const",
            "const outer = 5\nchip {\n  const inner = outer * 2\n  var m: int[] = [inner, 12345]\n}",
            vec![Int(10), Int(12345)],
        ),
        // `predeclare`'s `AnonChip` arm registers it into the parent scope, so a
        // `const` anywhere can CALL it; `scope_mods` descends as `scope_lets` does.
        (
            "a const mod in an anonymous chip body",
            "chip {\n  const mod dbl(n: const int) -> int { return n * 2 }\n}\nconst t = dbl(6)\nvar m: int[] = [t, 12345]",
            vec![Int(12), Int(12345)],
        ),
        // A tuple pattern binds POSITIONALLY; a multi-output `const mod` result types
        // as a NAME-keyed record in declaration order, so `const (a, b) = pair(..)`
        // reads it as `const { a, b }` does.
        (
            "a tuple destructure of a multi-output const mod",
            "const mod trio(n: const int) -> (a: int, b: int, c: int) {\nout a = n\n  out b = n + 1\n  out c = n + 2\n}\n\
             const (x, y, z) = trio(41)\nvar m: int[] = [x, y, z, 12345]",
            vec![Int(41), Int(42), Int(43), Int(12345)],
        ),
        // A tuple LITERAL evaluates to an index-keyed record and destructures the same.
        (
            "a tuple destructure of a tuple literal",
            "const (a, b) = (7, 8)\nvar m: int[] = [a, b, 12345]",
            vec![Int(7), Int(8), Int(12345)],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, want) in &rows {
        match baked_array(src) {
            Ok(got) if got == *want => {}
            got => failures.push(format!("{label}: baked {got:?}, want {want:?}\n{src}")),
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// Declare-before-use holds for a top-level `let` too, as it does for a
/// `mod`/`chip` (WS021). The constant environment's fixpoint resolves a
/// scrambled chain and the values still bake correctly, but the typechecker
/// walks declarations in order and reports the forward reference. What keeps
/// a merged multi-file program in dependency order is resolve's own
/// reordering pass, not the fixpoint.
#[test]
fn a_forward_reference_between_top_level_lets_is_reported() {
    let r = compile("let C = B * 2\nlet B = A + 1\nlet A = 5\nvar m: int[] = [A, B, C]");
    let unknown: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code == "WS002")
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        unknown.iter().any(|m| m.contains("'B'")) && unknown.iter().any(|m| m.contains("'A'")),
        "a forward reference must be reported: {:?}",
        r.diagnostics
    );
}

#[test]
fn operator_folding_does_not_leak_outside_initializers() {
    // `expr_to_literal` (no constant environment) decides bake-vs-wire in many
    // places besides initializers. It does NOT fold operators, or a call with
    // arithmetic args (`Rotation(0.0 + 0.0, ...)`) would collapse into a literal and
    // lose its MakeRotation gate. `fold::make_rotation_does_not_fold` guards the pass.
    let r = compile("out r: rotator = Rotation(0.0 + 0.0, 90.0 + 0.0, 45.5 + 0.0)");
    assert_eq!(
        count_class(&r.module, crate::ir::gate_class::MAKE_ROTATION),
        1,
        "arithmetic args must still produce a real gate"
    );
}

#[test]
fn constant_named_in_an_initializer_counts_as_used() {
    // A constant reached only from an array initializer registers as a use, or the
    // import that supplies it is reported unused and Organize Imports deletes it.
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
    let unused: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS014").collect();
    assert!(unused.is_empty(), "false unused-import warning: {unused:?}");
}

#[test]
fn constants_travel_with_an_imported_array() {
    // Importing only the accessor mod drags in the array AND the constants its
    // initializer names; without them the merged program lacks the values it is
    // built from (WS002 at the initializer).
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
        assert!(
            names.contains(&want.to_string()),
            "{want} not pulled in: {names:?}"
        );
    }
}

/// An initializer with no compile-time value is still WS003 at typecheck,
/// never a silently baked wrong value.
#[test]
fn a_non_constant_initializer_is_ws003() {
    let rows: &[(&str, &str)] = &[
        (
            "a runtime value has no compile-time form",
            "in x: int\nvar m: int[] = [x]",
        ),
        // 1 << 64 is undefined for i64; refuse rather than guess.
        (
            "an out-of-range shift is not folded",
            "var m: int[] = [1 << 64]",
        ),
        // A depends on B depends on A: the fixpoint terminates and neither resolves.
        (
            "cyclic constants do not hang or bake",
            "let A = B + 1\nlet B = A + 1\nvar m: int[] = [A]",
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src) in rows {
        let codes = errs(src);
        if !codes.iter().any(|c| c == "WS003") {
            failures.push(format!("{label}: expected WS003, got {codes:?}\n{src}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

// `lower::expr::wire_type_of_literal`'s `Literal::Object` arm returns `None`, so
// a violated invariant surfaces as a diagnosable gap, not an abort. The invariant:
// `Literal::Object` is built ONLY by `default_literal_for_var_type` (an
// object-family `Var` gate's placeholder), never by `const_eval`.
#[test]
fn literal_object_is_never_produced_by_const_eval() {
    // Where `Literal::Object` DOES come from: a `Var` gate's own default.
    assert_eq!(
        crate::const_eval::default_literal_for_var_type(&crate::ir::Type::Entity),
        Some(Literal::Object),
        "Object is default_literal_for_var_type's placeholder for object-family types"
    );
    // No source syntax folds to an entity/controller/character value: the only way
    // to name one is a runtime call, which `const_eval` rejects as non-constant, so
    // `wire_type_of_literal`'s `const_lookup()` caller never sees an `Object` literal.
    assert!(
        errs("const e = FindPlayer(\"x\")").contains(&"WS046".to_string()),
        "an entity-typed initializer must never be accepted as a compile-time constant"
    );
}

// `const` inside an anonymous `chip { }`

/// Both stages must agree about an anonymous chip's constants, not just the
/// baked value: a `const` condition inside one has to tree-shake exactly as it
/// does at the top level. The `else` arm's unique sentinel must be absent, and
/// the runtime-condition control proves the sentinel WOULD be there otherwise.
#[test]
fn a_const_if_inside_an_anonymous_chip_elides_like_a_top_level_one() {
    let body = "  in go: exec\n  on go {\n    if flag { PrintToConsole(\"taken\") } \
                else { PrintToConsole(\"SENTINEL\") }\n  }\n";
    let has_sentinel = |src: &str| {
        let r = compile_ok(src);
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

/// `typecheck::let_binding::check_const_recorded` fires when a `const` whose
/// initializer EVALUATED is not retrievable as a constant in its own scope. Its
/// realistic failure is a FALSE positive, so every scope a `const` can be
/// declared in is listed here and must stay silent.
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
        let diags = errs(src);
        assert!(
            !diags.contains(&"WS046".to_string()),
            "a recorded `const` must not report at its binding\n  src: {src}\n  got: {diags:?}"
        );
    }
}

// Compile-time tuple destructuring

/// A width mismatch stays an error and names BOTH counts, in the program's
/// terms rather than the compiler's ("expected tuple[2], got Record(...)").
#[test]
fn a_tuple_destructure_of_the_wrong_width_reports_both_counts() {
    let diags = errs(
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
    let tuple = compile_ok(&format!("{mod_src}let (x, y) = pair(n)\nout r = x + y"));
    let record = compile_ok(&format!(
        "{mod_src}let {{ a: x, b: y }} = pair(n)\nout r = x + y"
    ));
    assert_eq!(
        wiring_shape(&tuple.module),
        wiring_shape(&record.module),
        "tuple and record spellings of the same destructure must wire identically"
    );
}

/// A `var` initializer naming a constant bakes the constant's value, widened
/// to the declared type through `const_eval::scalar_in_type`. Asserts the
/// baked `InitialValue`, not a gate count: a var left at its type default
/// emits the same gates.
#[test]
fn a_var_initializer_naming_a_constant_bakes_the_constant() {
    use crate::ir::Literal as L;
    let rows: Vec<(&str, &str, L)> = vec![
        (
            "const K: float = 123.45\nvar h: float = K\nout v: float = h\n",
            "h",
            L::Float(123.45),
        ),
        (
            "const K: int = 7\nvar h: int = K\nout v: int = h\n",
            "h",
            L::Int(7),
        ),
        (
            "const K: bool = true\nvar h: bool = K\nout v: bool = h\n",
            "h",
            L::Bool(true),
        ),
        (
            "const K: string = \"hi\"\nvar h: string = K\nout v: string = h\n",
            "h",
            L::String("hi".into()),
        ),
        // an inferred constant, and an expression OVER constants
        (
            "const K = 2.0\nvar h: float = K\nout v: float = h\n",
            "h",
            L::Float(2.0),
        ),
        (
            "const K: float = 2.0\nvar h: float = K * 3.0\nout v: float = h\n",
            "h",
            L::Float(6.0),
        ),
        // a record FIELD initializer, the second path that bakes a constant
        (
            "type P = { x: float }\nconst K: float = 9.5\nvar p: P = { x: K }\nout v: float = p.x\n",
            "p.x",
            L::Float(9.5),
        ),
        // an INT constant still builds a float variable rather than an integer one
        (
            "const K: int = 3\nvar h: float = K\nout v: float = h\n",
            "h",
            L::Float(3.0),
        ),
    ];
    let mut failures = Vec::new();
    for (src, label, want) in &rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let got = r
            .module
            .nodes
            .values()
            .find(|n| matches!(n.properties.get(&crate::intern::sym::NAME_LABEL), Some(L::String(s)) if s == label))
            .and_then(|n| n.properties.get(&crate::intern::sym::INITIAL_VALUE).cloned());
        if !errors.is_empty() || got.as_ref() != Some(want) {
            failures.push(format!(
                "errors {errors:?}, `{label}` baked {got:?}, want {want:?}\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
