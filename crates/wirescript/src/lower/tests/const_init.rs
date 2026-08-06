//! Constant expressions in `var` / `array` initializers.
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
    for n in r.module.nodes.values() {
        if let Some(Literal::Array(items)) = n.properties.get(&crate::intern::intern_static("InitialValue")) {
            return items.clone();
        }
    }
    panic!("no array gate with a baked InitialValue");
}

/// Typecheck errors only (the gate that rejects a non-constant element).
fn errors(src: &str) -> Vec<String> {
    let parsed = crate::parser::parse(src, "test");
    typecheck(&parsed.ast, "test")
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
