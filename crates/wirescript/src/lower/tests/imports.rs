//! Import-shape regressions.

use super::*;
use crate::collections::HashSet;
use crate::ir::NodeKind;

fn is_pure(gate_class: &str) -> bool {
    gate_class == crate::ir::gate_class::LITERAL
        || gate_class.starts_with("BrickComponentType_Expr_")
}

/// The pure gates, anywhere in the module tree, that no wire touches.
fn orphan_pure_gates(r: &LowerResult) -> Vec<&'static str> {
    let wired: HashSet<crate::ir::NodeId> = r
        .module
        .wires_deep()
        .flat_map(|w| [w.source.node_id, w.target.node_id])
        .collect();
    r.module
        .nodes_deep()
        .filter(|n| n.kind == NodeKind::Gate && is_pure(n.gate_class) && !wired.contains(&n.id))
        .map(|n| n.gate_class)
        .collect()
}

/// An imported namespace member whose bare name matches a LOCAL exec-input
/// trigger must not clobber that input. Regression: `import * as C` where the
/// imported module has `let start: <record>` registered a `Binding::Record`
/// under the bare name `start`, shadowing the importer's own `in start: exec`.
/// `on start` then resolved to that record instead of the Input and SILENTLY
/// dropped the entire handler body (0 gates, no diagnostic). Every other
/// namespace-member kind was `is_none()`-guarded against this; `let` was not.
#[test]
fn imported_let_does_not_clobber_local_input_trigger() {
    let lib = "let start: { v: int } = { v: 1 }";
    let main = "\
import * as C from \"lib\"
in start: exec
var hit: int = 0
on start { hit = hit + 1 }";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    assert!(
        has_gate_deep(&r, crate::ir::gate_class::VAR_INCREMENT),
        "the `on start` handler body was dropped: an imported `let start` clobbered \
         the local `in start: exec`, so the trigger resolved to the imported value"
    );
}

/// A module imported via BOTH a namespace (`import * as x`) AND a named import
/// materializes its top-level `let`s twice — the namespace copy is unreferenced,
/// so a constant ships as a gate wired to nothing unless pruned.
/// `prune_dead_pure_gates` must drop that orphan.
#[test]
fn namespace_plus_named_import_leaves_no_orphan_constant() {
    let lib = "\
let PAD = \"xxxxxxxx\"
mod pick(n: int) -> string {
  return PAD.Substring(0, n)
}
mod greet() -> string {
  return \"hi\"
}";
    let main = "\
import * as lib from \"lib\"
import { pick } from \"lib\"
in n: int
out r = pick(n)
out g = lib.greet()";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    let orphans = orphan_pure_gates(&r);
    assert!(
        orphans.is_empty(),
        "double-import (namespace + named) left orphaned pure gate(s): {orphans:?}"
    );
}

/// A user's connected-but-unused pure computation is NOT a compiler-generated
/// orphan and must survive (the prune only removes fully-disconnected gates).
#[test]
fn unused_let_computation_is_kept() {
    let r = compile_ok(
        "\
var x: int = 5
in player: character
on player { let y = x * 2 + 1 }",
    );
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathMultiply"),
        "unused `let y = x * 2 + 1` should keep its MathMultiply (not DCE'd)"
    );
}

/// A namespace import inside an imported module must travel with the
/// declarations that call through it. `main` imports `useFoo` from `second`,
/// whose body calls `Foo.blah(...)` from a third file — without the namespace
/// the call resolved to nothing and silently lowered to an `_Unsupported`
/// placeholder that does nothing at runtime, with no diagnostic.
#[test]
fn namespace_import_travels_two_modules_deep() {
    // `n` comes from an input so constant folding cannot erase the arithmetic
    // this asserts on — a folded `useFoo(5)` would collapse to a literal and
    // stop proving the imported bodies were inlined at all.
    let r = compile_multi(
        "import { useFoo } from \"second\"\nin n: int\nout result = useFoo(n)",
        &[
            (
                "second",
                "import * as Foo from \"third\"\n\
                 mod useFoo(n: int) -> int {\n\
                   return Foo.blah(n) + 1\n\
                 }",
            ),
            ("third", "mod blah(n: int) -> int {\n  return n * 2\n}"),
        ],
    );
    assert_no_errors(&r);
    assert!(
        !has_unsupported(&r.module),
        "the nested namespace call must resolve; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // `n * 2` from the third module and the `+ 1` from the second must both
    // survive — a dropped namespace loses the multiply entirely.
    assert_eq!(
        count_class(&r.module, "BrickComponentType_Expr_MathMultiply"),
        1,
        "the third module's body must be inlined"
    );
    assert_eq!(
        count_class(&r.module, "BrickComponentType_Expr_MathAdd"),
        1,
        "the second module's arithmetic must survive"
    );
}

// ---------- `const` across files ----------

/// The library every `const`-across-files test below imports from. `12345` is
/// the control literal in each baked array: it proves the array itself really
/// was baked, so a missing constant reads as a missing ELEMENT rather than as
/// a whole gate that quietly never appeared.
const CONST_LIB: &str = "\
const SLOTS = 4
const CHAN = \"libchan\"
const mod triple(n: const int) -> int { return n * 3 }
const P = { x: 11, y: 22 }
const { x: PX, y: PY } = P";

/// The `InitialValue` of every array gate with one, anywhere in the tree.
fn baked_arrays(r: &LowerResult) -> Vec<Vec<crate::ir::Literal>> {
    r.module
        .nodes_deep()
        .filter_map(
            |n| match n.properties.get(&crate::intern::sym::INITIAL_VALUE) {
                Some(crate::ir::Literal::Array(items)) => Some(items.clone()),
                _ => None,
            },
        )
        .collect()
}

/// An imported constant is compile-time in the importer: every row's array
/// initializer bakes the constants it names, beside the `12345` control.
#[test]
fn an_imported_constant_bakes_into_an_initializer() {
    let rows: &[(&str, &str, &[i64])] = &[
        (
            "a named import of a top-level `const`",
            "import { SLOTS } from \"lib\"\nvar m: int[] = [SLOTS, 12345]",
            &[4, 12345],
        ),
        (
            "an imported `const mod` answers a call from a constant-only position, not as gates",
            "import { triple } from \"lib\"\nvar m: int[] = [triple(5), 12345]",
            &[15, 12345],
        ),
        (
            "a DESTRUCTURED `const` crossing a named import: `resolve::decl_name` is `None` for \
             every destructuring form, so the lookup was WS012 \"'PX' not found in 'lib'\"; \
             `resolve::decl_names` answers every name a declaration introduces",
            "import { PX, PY } from \"lib\"\nvar m: int[] = [PX, PY, 12345]",
            &[11, 22, 12345],
        ),
        (
            "an alias of one destructured name renames THAT name and keeps its own field",
            "import { PX as QX } from \"lib\"\nvar m: int[] = [QX, 12345]",
            &[11, 12345],
        ),
        (
            "import-all pushes every importable declaration, sharing the multi-name duplicate check",
            "import \"lib\"\nvar m: int[] = [PX, PY, 12345]",
            &[11, 22, 12345],
        ),
        (
            "`ns.NAME`: `build_const_env` seeds each namespace's constants under `ns.NAME` keys and \
             `const_eval`'s FieldAccess arm resolves them",
            "import * as lib from \"lib\"\nvar m: int[] = [lib.SLOTS, 12345]",
            &[4, 12345],
        ),
    ];
    let mut failures = Vec::new();
    for (label, entry, want) in rows {
        let r = compile_multi(entry, &[("lib", CONST_LIB)]);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        if !errors.is_empty() {
            failures.push(format!("{label}: errors {errors:?}"));
            continue;
        }
        let want: Vec<crate::ir::Literal> =
            want.iter().map(|&v| crate::ir::Literal::Int(v)).collect();
        let got = baked_arrays(&r);
        if got != [want.clone()] {
            failures.push(format!("{label}: baked {got:?}, want [{want:?}]"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
    // The same namespaced constant used as a runtime value lowers to real gates.
    let r = compile_multi(
        "import * as lib from \"lib\"\nout r = lib.SLOTS + 1",
        &[("lib", CONST_LIB)],
    );
    assert_no_errors(&r);
    assert!(!has_unsupported(&r.module));
}

/// A constant-only slot (a custom-event channel name) fed by an imported
/// `const`: the shape that silently baked an EMPTY value twice in this
/// feature's history, so assert the baked string, not just that it compiled.
#[test]
fn an_imported_const_bakes_into_a_constant_only_slot() {
    let r = compile_multi(
        "import { CHAN } from \"lib\"\nin go: exec\non go { SendCustomEvent(CHAN, 1) }",
        &[("lib", CONST_LIB)],
    );
    assert_no_errors(&r);
    let baked: Vec<String> = r
        .module
        .nodes
        .values()
        .filter_map(
            |n| match n.properties.get(&crate::intern::intern_static("EventName")) {
                Some(crate::ir::Literal::String(s)) => Some(s.clone()),
                _ => None,
            },
        )
        .collect();
    assert_eq!(baked, vec!["libchan".to_string()]);
}

/// Nested imports: `main` imports `mid`, `mid` imports `leaf`. A `const` and a
/// `const mod` from `leaf` — and a `const` in `mid` DERIVED from a leaf one —
/// must all still be compile-time two files away, including through the
/// constant-only slot.
#[test]
fn consts_stay_compile_time_through_a_nested_import() {
    let leaf = "\
const LEAF_N = 6
const LEAF_CH = \"leafchan\"
const mod leafDouble(n: const int) -> int { return n * 2 }";
    let mid = "\
import { LEAF_N, LEAF_CH, leafDouble } from \"leaf\"
const MID_N = LEAF_N + 1
const mod midTriple(n: const int) -> int { return leafDouble(n) + n }";
    let entry = "\
import { MID_N, midTriple, LEAF_N, LEAF_CH } from \"mid\"
var m: int[] = [LEAF_N, MID_N, midTriple(4), 12345]
in go: exec
on go { SendCustomEvent(LEAF_CH, 1) }";
    let r = compile_multi(entry, &[("leaf", leaf), ("mid", mid)]);
    assert_no_errors(&r);
    let ints = |v: &[i64]| {
        v.iter()
            .map(|&i| crate::ir::Literal::Int(i))
            .collect::<Vec<_>>()
    };
    assert_eq!(baked_arrays(&r), [ints(&[6, 7, 12, 12345])]);
    assert!(
        r.module.nodes.values().any(|n| matches!(
            n.properties.get(&crate::intern::intern_static("EventName")),
            Some(crate::ir::Literal::String(s)) if s == "leafchan"
        )),
        "a const two imports away must still bake into a constant-only slot"
    );
}

/// A namespaced record VALUE folds inside a (record-)array initializer
/// (`var arr: T[] = [Other.value]`), the multi-file record-value form of the
/// namespaced-const fold above. This was a false WS003 "elements must be
/// constant literals" because the namespaced member never reached the env.
#[test]
fn a_namespaced_record_value_folds_in_a_record_array_init() {
    let lib = "type DoubleInt = { a: int, b: int }\nlet value = { a: 10, b: 20 }";
    let r = compile_multi(
        "import * as Other from \"test2\"\nvar arr: Other.DoubleInt[] = [Other.value]",
        &[("test2", lib)],
    );
    assert_no_errors(&r);
    assert!(!has_unsupported(&r.module));
    // One array per field, each baked with that field's value.
    let mut columns: Vec<(String, Option<crate::ir::Literal>)> =
        nodes_of(&r.module, crate::ir::gate_class::ARRAY_VAR)
            .into_iter()
            .map(|n| {
                let label = match n.properties.get(&crate::intern::sym::NAME_LABEL) {
                    Some(crate::ir::Literal::String(s)) => s.clone(),
                    _ => String::new(),
                };
                (
                    label,
                    n.properties
                        .get(&crate::intern::sym::INITIAL_VALUE)
                        .cloned(),
                )
            })
            .collect();
    columns.sort_by(|a, b| a.0.cmp(&b.0));
    let column = |v: i64| Some(crate::ir::Literal::Array(vec![crate::ir::Literal::Int(v)]));
    assert_eq!(
        columns,
        [
            ("arr.a".to_string(), column(10)),
            ("arr.b".to_string(), column(20))
        ]
    );
}

/// Two DIFFERENT modules declaring the same top-level name, merged by plain or
/// named import, collide with WS013 rather than silently dropping one and
/// aliasing both onto a single storage gate. A DIAMOND import (one module
/// reached through two others) stays a single shared declaration.
#[test]
fn a_name_from_two_modules_is_ws013_and_a_diamond_is_not() {
    use crate::resolve::{MemLoader, resolve};
    let rows: &[(&str, &[(&str, &str)], &str, bool)] = &[
        (
            "plain imports",
            &[("m1.ws", "var g: int = 1"), ("m2.ws", "var g: int = 2")],
            "import \"m1\"\nimport \"m2\"",
            true,
        ),
        (
            "named imports",
            &[("m1.ws", "var dat: int = 1"), ("m3.ws", "var dat: int = 2")],
            "import { dat } from \"m1\"\nimport { dat } from \"m3\"",
            true,
        ),
        (
            "a diamond",
            &[
                ("util.ws", "var shared: int = 5"),
                ("dm1.ws", "import \"util\""),
                ("dm2.ws", "import \"util\""),
            ],
            "import \"dm1\"\nimport \"dm2\"",
            false,
        ),
    ];
    let mut failures = Vec::new();
    for (label, files, entry, collides) in rows {
        let loader = MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let resolved = resolve(entry, "main", &loader);
        let tc = typecheck(
            &resolved.ast,
            "main",
            &crate::typecheck::CeSlotMap::default(),
        );
        // A collision reports WS013 at any severity; a diamond has no WS013 error.
        let found = if *collides {
            crate::test_support::codes(&tc.diagnostics)
        } else {
            crate::test_support::error_codes(&tc.diagnostics)
        };
        if found.contains(&"WS013") != *collides {
            failures.push(format!("{label}: WS013 expected {collides}, got {found:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// A generic mod called through a namespace import infers its type parameters
/// from the arguments, as the flat import does: `u.id(a)` is an `int` and
/// `u.id(s)` a `string`, each reading its own argument.
#[test]
fn a_generic_mod_called_through_a_namespace_infers_its_type_params() {
    let lib = "mod id<T>(x: T) -> T { return x }\nmod pick<T: int | float>(c: bool, a: T, b: T) -> T { return if c then a else b }";
    let main = "import * as u from \"lib\"\nin a: int\nin s: string\nin f: float\nin c: bool\n\
                var n: int = 0\nvar t: string = \"\"\nvar g: float = 0.0\nin go: exec\n\
                on go {\n  n = u.id(a) + 1\n  t = u.id(s)\n  g = u.pick(c, f, 2.5)\n}";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r.module));
    let m = &r.module;
    let source = |id: crate::ir::NodeId, port: WirePort| {
        m.wires
            .iter()
            .find(|w| w.target.node_id == id && w.target.port == port)
            .map(|w| w.source.node_id)
    };
    let input = |name: &str| {
        *m.nodes
            .iter()
            .find(|(_, n)| {
                n.kind == crate::ir::NodeKind::Input
                    && matches!(n.properties.get(&*crate::intern::sym::PORT_LABEL), Some(crate::ir::Literal::String(s)) if s == name)
            })
            .map(|(id, _)| id)
            .expect("input")
    };
    let port_ty = |n: &crate::ir::Node, port: WirePort| {
        n.ports
            .inputs
            .iter()
            .chain(&n.ports.outputs)
            .find(|p| p.name == port.sym())
            .map(|p| p.ty.clone())
    };
    // `u.id(a) + 1`: an int add of `a` itself.
    let add = super::nodes_of(m, "BrickComponentType_Expr_MathAdd");
    assert_eq!(add.len(), 1);
    assert_eq!(port_ty(add[0], WirePort::Output), Some(Type::Int));
    assert_eq!(source(add[0].id, WirePort::InputA), Some(input("a")));
    // `t = u.id(s)`: the string itself, stored as a string.
    let sets = super::nodes_of(m, crate::ir::gate_class::VAR_SET);
    let store_t = sets
        .iter()
        .find(|n| source(n.id, WirePort::Value) == Some(input("s")))
        .expect("t = s");
    assert_eq!(port_ty(store_t, WirePort::Value), Some(Type::String));
    // `u.pick(c, f, 2.5)`: a float Select between `f` and 2.5 on `c`.
    let select = super::nodes_of(m, crate::ir::gate_class::SELECT);
    assert_eq!(select.len(), 1);
    assert_eq!(port_ty(select[0], WirePort::Output), Some(Type::Float));
    let operands = [
        source(select[0].id, WirePort::InputA),
        source(select[0].id, WirePort::InputB),
    ];
    assert!(operands.contains(&Some(input("f"))), "{operands:?}");
    assert_eq!(source(select[0].id, WirePort::BSelectB), Some(input("c")));
}

/// The `Exec_Var_Set` writes whose `Value` is wired from a gate of `class`.
fn sets_fed_by(r: &LowerResult, class: &str) -> usize {
    r.module
        .wires
        .iter()
        .filter(|w| {
            w.target.port == WirePort::Value
                && r.module.nodes[&w.target.node_id].gate_class == crate::ir::gate_class::VAR_SET
                && r.module.nodes[&w.source.node_id].gate_class == class
        })
        .count()
}

/// Whether some `Exec_Var_Set` bakes the string `s`.
fn sets_string(r: &LowerResult, s: &str) -> bool {
    r.module.nodes.values().any(|n| {
        n.gate_class == crate::ir::gate_class::VAR_SET
            && matches!(n.properties.get(&crate::intern::sym::VALUE), Some(crate::ir::Literal::String(v)) if v == s)
    })
}

const ABS_LIB: &str = "mod abs(x: int) -> string { if x < 0 { return \"neg\" }\n return \"pos\" }\n\
                       mod useabs(x: int) -> string { return abs(x) }";

/// A named import pulls the module's private `abs` in beside `useabs`, which
/// calls it. The importer never named that `abs`, so its own `abs(fl)` is the
/// builtin, while `useabs` keeps calling the module's own.
#[test]
fn a_private_mod_named_like_a_builtin_does_not_replace_it_in_the_importer() {
    let main = "var k: int = -3\nvar fl: float = -1.5\nvar fo: float = 0.0\nvar so: string = \"\"\n\
                in go: exec\nimport { useabs } from \"lib\"\non go { fo = abs(fl)\n so = useabs(k) }";
    let r = compile_multi(main, &[("lib", ABS_LIB)]);
    assert_no_errors(&r);
    assert_eq!(
        sets_fed_by(&r, "BrickComponentType_Expr_MathAbs"),
        1,
        "fo = abs(fl) stores the builtin's result"
    );
    assert!(
        sets_string(&r, "neg") && sets_string(&r, "pos"),
        "useabs still calls the module's abs"
    );
}

/// The importer's own `mod abs` does not replace the builtin inside an
/// imported helper's body, which cannot name it.
#[test]
fn the_importers_mod_does_not_replace_a_builtin_inside_an_imported_body() {
    let lib = "mod helperabs(x: float) -> float { return abs(x) }";
    let main = "var k: int = -3\nvar fl: float = -1.5\nvar fo: float = 0.0\nvar so: string = \"\"\n\
                in go: exec\nimport { helperabs } from \"lib\"\nmod abs(x: int) -> string { return \"mine\" }\n\
                on go { fo = helperabs(fl)\n so = abs(k) }";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    assert_eq!(
        sets_fed_by(&r, "BrickComponentType_Expr_MathAbs"),
        1,
        "helperabs calls the builtin"
    );
    assert!(
        sets_string(&r, "mine"),
        "the importer's own call is its own mod"
    );
}

/// The members of an `import * as` module are reached as `u.f`; a bare
/// `abs(fl)` in the importer is the builtin, and the module's members still
/// call their sibling.
#[test]
fn a_namespace_member_named_like_a_builtin_does_not_replace_it() {
    let lib = "mod abs(x: float) -> float { return x * 2.0 }\nmod twice(x: float) -> float { return abs(x) }";
    for (main, sibling_calls) in [
        (
            "var fl: float = -1.5\nvar fo: float = 0.0\nin go: exec\nimport * as u from \"lib\"\non go { fo = abs(fl) }",
            0,
        ),
        (
            "var fl: float = -1.5\nvar fo: float = 0.0\nvar f2: float = 0.0\nin go: exec\nimport * as u from \"lib\"\n\
             on go { fo = abs(fl)\n f2 = u.twice(fl) }",
            1,
        ),
        (
            "var fl: float = -1.5\nvar fo: float = 0.0\nvar f2: float = 0.0\nin go: exec\nimport { twice } from \"lib\"\n\
             on go { fo = abs(fl)\n f2 = twice(fl) }",
            1,
        ),
    ] {
        let r = compile_multi(main, &[("lib", lib)]);
        assert_no_errors(&r);
        assert_eq!(
            sets_fed_by(&r, "BrickComponentType_Expr_MathAbs"),
            1,
            "{main}"
        );
        assert_eq!(
            sets_fed_by(&r, "BrickComponentType_Expr_MathMultiply"),
            sibling_calls,
            "{main}"
        );
    }
}

/// A whole-module import names every declaration of the module, so its `abs`
/// is the one called, as before.
#[test]
fn a_whole_module_import_names_its_mod_over_the_builtin() {
    let main = "var k: int = -3\nvar so: string = \"\"\nin go: exec\nimport \"lib\"\non go { var t = abs(k)\n so = t }";
    let r = compile_multi(main, &[("lib", ABS_LIB)]);
    assert_no_errors(&r);
    assert!(!has_gate_deep(&r, "BrickComponentType_Expr_MathAbs"));
    assert!(sets_string(&r, "neg"));
    // Named by a named import too.
    let main = "var k: int = -3\nvar so: string = \"\"\nin go: exec\nimport { abs } from \"lib\"\non go { so = abs(k) }";
    let r = compile_multi(main, &[("lib", ABS_LIB)]);
    assert_no_errors(&r);
    assert!(!has_gate_deep(&r, "BrickComponentType_Expr_MathAbs"));
    assert!(sets_string(&r, "neg"));
}

/// Constant evaluation follows the same rule: a module's private `const mod
/// abs` does not decide a constant `if` on `abs(-3)` in the importer.
#[test]
fn a_private_const_mod_named_like_a_builtin_does_not_fold_in_the_importer() {
    let lib = "const mod abs(x: int) -> int { return 99 }\nmod useabs() -> int { return abs(5) }";
    let main = "import { useabs } from \"lib\"\nvar n: int = 0\nvar m: int = 0\nin go: exec\n\
                on go { if abs(-3) == 99 { n = 1 } else { n = 2 }\n m = useabs() }";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    let ints: Vec<i64> = r
        .module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::VAR_SET)
        .filter_map(|n| match n.properties.get(&crate::intern::sym::VALUE) {
            Some(crate::ir::Literal::Int(v)) => Some(*v),
            _ => None,
        })
        .collect();
    assert!(ints.contains(&2), "the `else` store survives: {ints:?}");
    assert!(
        ints.contains(&99),
        "useabs still calls the module's const mod: {ints:?}"
    );
}

/// The floats a program's `Var_Set` gates store.
fn float_sets(r: &LowerResult) -> Vec<f64> {
    r.module
        .nodes
        .values()
        .filter(|n| n.gate_class == crate::ir::gate_class::VAR_SET)
        .filter_map(|n| match n.properties.get(&crate::intern::sym::VALUE) {
            Some(crate::ir::Literal::Float(v)) => Some(*v),
            _ => None,
        })
        .collect()
}

/// A `self` mod named like a builtin receiver method of another receiver type
/// (`Length` is a string method) is the one `vv.Length()` calls on a vector,
/// and a named import pulls in the `self` mods its body calls by method
/// syntax: `v.dist2(v)` in an imported body was a placeholder.
#[test]
fn a_method_call_reaches_its_self_mod_in_every_file() {
    let main = "var vv: vector = Vec(1.0, 2.0, 3.0)\nvar fo: float = 0.0\nin go: exec\n\
                mod Length(self: vector) -> float { return 777.0 }\non go { fo = vv.Length() }";
    let r = compile_ok(main);
    assert!(!has_gate_deep(&r, "BrickComponentType_Expr_String_Length"));
    assert_eq!(float_sets(&r), [777.0]);
    let lib = "mod Length(self: vector) -> float { return 777.0 }\n\
               mod dist2(self: vector, o: vector) -> float { return 5.0 }\n\
               mod pub1(v: vector) -> float { return v.Length() + v.dist2(v) }";
    let main = "import { pub1 } from \"lib\"\nvar vv: vector = Vec(1.0, 2.0, 3.0)\nvar fo: float = 0.0\n\
                in go: exec\non go { fo = pub1(vv) }";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r.module));
    assert!(!has_gate_deep(&r, "BrickComponentType_Expr_String_Length"));
    assert_eq!(float_sets(&r), [782.0]);
}

/// A module's private `self` mod, present because a named import's body
/// calls it, answers that body's method calls and no others: in the importer
/// `fl.abs()` is the builtin's `WS036`, `vv.Length()` the string method's
/// `WS003`, and `fl.twice()` a `WS036` naming where it is declared. Importing the
/// `self` mod by name or the whole module makes the importer's calls reach
/// it, and a named import used only by method syntax is not unused.
#[test]
fn a_private_self_mod_answers_only_its_own_module() {
    let lib = "mod abs(self: float) -> float { return self * 3.0 }\n\
               mod Length(self: vector) -> float { return 777.0 }\n\
               mod twice(self: float) -> float { return self * 2.0 }\n\
               mod pub2(v: float, w: vector) -> float { return v.abs() + w.Length() + v.twice() }";
    let head = "var fl: float = -1.5\nvar vv: vector = Vec(3.0, 4.0, 0.0)\nvar fo: float = 0.0\nin go: exec\n";
    let program = |import: &str, body: &str| format!("{import}\n{head}on go {{ {body} }}");
    let error_codes = |r: &LowerResult| -> Vec<String> {
        crate::test_support::error_codes(&r.diagnostics)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    for (body, code, says) in [
        ("fo = fl.abs()", "WS036", "takes no receiver"),
        ("fo = vv.Length()", "WS003", "Magnitude(vv)"),
        (
            "fo = fl.twice()",
            "WS036",
            "`twice` is not declared in `main` and `main` does not import it",
        ),
        (
            "fo = pub2(fl, vv)
 fo = fl.abs()",
            "WS036",
            "takes no receiver",
        ),
    ] {
        let r = compile_multi(
            &program("import { pub2 } from \"lib\"", body),
            &[("lib", lib)],
        );
        assert_eq!(error_codes(&r), [code], "`{body}`");
        let d = r.diagnostics.iter().find(|d| d.code == code).unwrap();
        assert!(d.message.contains(says), "`{body}`: {}", d.message);
    }
    // The library's own body still reaches all three.
    let r = compile_multi(
        &program("import { pub2 } from \"lib\"", "fo = pub2(fl, vv)"),
        &[("lib", lib)],
    );
    assert_no_errors(&r);
    assert!(!has_unsupported(&r.module));
    assert!(!has_gate_deep(&r, "BrickComponentType_Expr_String_Length"));
    assert!(!has_gate_deep(&r, "BrickComponentType_Expr_MathAbs"));
    // A private plain mod follows the same rule.
    let r = compile_multi(
        &program("import { pub2 } from \"lib\"", "fo = twice(fl)"),
        &[("lib", lib)],
    );
    assert_eq!(error_codes(&r), ["WS002"]);
    // Named by the importer: its own calls reach the `self` mod.
    for import in ["import { Length } from \"lib\"", "import \"lib\""] {
        let r = compile_multi(&program(import, "fo = vv.Length()"), &[("lib", lib)]);
        assert_no_errors(&r);
        crate::test_support::assert_no_code(&r.diagnostics, "WS014");
        assert!(
            !has_gate_deep(&r, "BrickComponentType_Expr_String_Length"),
            "{import}"
        );
        assert_eq!(float_sets(&r), [777.0], "{import}");
    }
}

/// A `mod` declared in both the importer and its library is the one `WS013`,
/// not also a `WS036` located in the library; and a type alias imported by
/// name and used only to annotate an `out` is not `WS014` unused.
#[test]
fn a_duplicate_mod_is_one_error_and_an_annotation_uses_its_alias() {
    let lib = "mod twice(self: float) -> float { return self * 2.0 }\n\
               mod pubt(v: float) -> float { return v.twice() }";
    let main = "import { pubt } from \"lt\"\nvar fl: float = 1.5\nvar fo: float = 0.0\nin go: exec\n\
                mod twice(self: float) -> float { return self * 7.0 }\non go { fo = pubt(fl) + fl.twice() }";
    let r = compile_multi(main, &[("lt", lib)]);
    assert_eq!(
        crate::test_support::error_codes(&r.diagnostics),
        ["WS013"],
        "{:?}",
        r.diagnostics
    );
    let lib = "type P = { a: int, b: float }\nmod libmk(k: int) -> P { return { a: k, b: 2.5 } }";
    for main in [
        "import { libmk, P } from \"lr\"\nvar k: int = 3\nin go: exec\non go { out z: P = libmk(k) }",
        "import { libmk, P } from \"lr\"\nvar k: int = 3\nin go: exec\nout z: P = libmk(k)",
    ] {
        let r = compile_multi(main, &[("lr", lib)]);
        assert!(r.diagnostics.is_empty(), "{main}: {:?}", r.diagnostics);
    }
}

/// The effect analysis decides by the same rule: a library's private
/// effectful `Length(self: vector)` is not what the importer's `s.Length()`
/// on a string calls, so the importer's `if` expression stays a pure
/// `Select` of the string method. It built an `Exec_Branch` around the call
/// when the private `self` mod counted.
#[test]
fn a_private_self_mod_does_not_make_the_importers_method_call_effectful() {
    let lib = "var cnt: int = 0\nmod Length(self: vector) -> float { cnt = cnt + 1\n return 777.0 }\n\
               mod vlen(v: vector) -> float { return v.Length() }";
    let main = "import { vlen } from \"lv\"\nvar vv: vector = Vec(1.0, 2.0, 3.0)\nvar s: string = \"abc\"\n\
                var fo: float = 0.0\nvar n: int = 0\nin go: exec\nin go2: exec\nin c: bool\n\
                on go { fo = vlen(vv) }\non go2 { n = if c then s.Length() else 0 }";
    let r = compile_multi(main, &[("lv", lib)]);
    assert_no_errors(&r);
    assert!(has_gate_deep(&r, "BrickComponentType_Expr_String_Length"));
    assert!(has_gate_deep(&r, "BrickComponentType_Expr_Select"));
    assert!(!has_gate_deep(&r, "BrickComponentType_Exec_Branch"));
    assert_eq!(
        float_sets(&r),
        [777.0],
        "the library's own call still reaches its mod"
    );
}

/// A library body calls only what its own file declares or imports, by method
/// and by plain call alike: the importer's mods are not visible there, a
/// sibling module's are not either, and the error is located in the library
/// and says what to do there. Declared in the library, both calls compile.
/// A constant computed by a `const mod` its file neither declares nor imports
/// is one error, the call's `WS002`; it is not also `WS046` saying the mod is
/// not declared `const mod`.
#[test]
fn a_const_mod_the_file_cannot_name_is_one_error() {
    let lib = "const mod sq(v: int) -> int { return v * v }
const mod quad(v: int) -> int { return sq(sq(v)) }";
    let head = "import { quad } from \"lib\"
var n: int = 0
in go: exec
mod m(a: const int) -> int { return a }
";
    for body in [
        "const Q = sq(2)
on go { n = Q }",
        "on go {
const Q = sq(2)
n = Q
}",
        "on go { n = m(sq(2)) }",
    ] {
        let r = compile_multi(&format!("{head}{body}"), &[("lib", lib)]);
        assert_eq!(
            crate::test_support::error_codes(&r.diagnostics),
            ["WS002"],
            "{body}: {:?}",
            r.diagnostics
        );
    }
}

#[test]
fn a_library_body_does_not_reach_its_importers_mods() {
    let lib = "mod pubh(v: float) -> float { return v.hook() }\n\
               mod pubp(v: float) -> float { return hookp(v) }";
    let head = "var fl: float = 1.5\nvar fo: float = 0.0\nin go: exec\n\
                mod hook(self: float) -> float { return self * 9.0 }\n\
                mod hookp(v: float) -> float { return v * 9.0 }\n";
    let errors = |r: &LowerResult| -> Vec<crate::diagnostic::Diagnostic> {
        r.diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .cloned()
            .collect()
    };
    // A whole-module import brings both bodies, so it reports both calls.
    for (import, call, expect) in [
        (
            "import { pubh } from \"lib\"",
            "pubh(fl)",
            &[("WS036", "hook")][..],
        ),
        (
            "import { pubp } from \"lib\"",
            "pubp(fl)",
            &[("WS002", "hookp")][..],
        ),
        (
            "import \"lib\"",
            "pubh(fl)",
            &[("WS036", "hook"), ("WS002", "hookp")][..],
        ),
    ] {
        let r = compile_multi(
            &format!("{import}\n{head}on go {{ fo = {call} }}"),
            &[("lib", lib)],
        );
        let errs = errors(&r);
        assert_eq!(errs.len(), expect.len(), "{import} {call}: {errs:?}");
        for (e, (code, name)) in errs.iter().zip(expect) {
            assert_eq!(
                (e.code.as_ref(), &*e.range.file),
                (*code, "lib.ws"),
                "{import} {call}"
            );
            let m = &e.message;
            assert!(
                m.contains(&format!(
                    "`{name}` is not declared in `lib.ws` and `lib.ws` does not import it"
                )),
                "{m}"
            );
            assert!(m.contains("`main` imports `lib.ws`"), "{m}");
        }
    }
    // A sibling module the importer also imports.
    let a = "mod pa(v: float) -> float { return pb(v) }";
    let b = "mod pb(v: float) -> float { return v * 2.0 }";
    let main = "import \"a\"\nimport \"b\"\nvar fl: float = 1.5\nvar fo: float = 0.0\nin go: exec\non go { fo = pa(fl) }";
    let errs = errors(&compile_multi(main, &[("a", a), ("b", b)]));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(
        (errs[0].code.as_ref(), &*errs[0].range.file),
        ("WS002", "a.ws")
    );
    assert!(
        errs[0].message.contains("Import it into `a.ws`"),
        "{}",
        errs[0].message
    );
    // Declared in the library, both answer.
    let lib = format!(
        "mod hook(self: float) -> float {{ return self * 9.0 }}\n\
         mod hookp(v: float) -> float {{ return v * 9.0 }}\n{lib}"
    );
    let main = "import { pubh, pubp } from \"lib\"\nvar fl: float = 1.5\nvar fo: float = 0.0\nin go: exec\n\
                on go { fo = pubh(fl) + pubp(fl) }";
    let r = compile_multi(main, &[("lib", &lib)]);
    assert_no_errors(&r);
    assert!(!has_unsupported(&r.module));
}

/// A library names the top-level values (constants, `let`s, storage, ports)
/// it declares or imports, as it names types and mods: a library body reading
/// a name only its importer declares is `WS002`, in a value, a constant-only
/// slot, a `Sleep` delay or a constant's initializer. It read the importer's
/// value before. An importer reads a library's value as it did.
#[test]
fn a_library_names_the_values_it_declares_or_imports() {
    let codes = |main: &str, deps: &[(&str, &str)]| {
        super::shadow_differential::outcome(main, deps, false).codes
    };
    let main = |import: &str| {
        format!(
            "{import}\nconst K = 4\nconst CH = \"evt\"\nlet L = 5\nvar V: int = 6\nin go: exec\nvar x: int = 0\nout ox: int = x\non go {{ x = mk() }}"
        )
    };
    for body in [
        "mod mk() -> int { return K + 1 }",
        "mod mk() -> int { return L + 1 }",
        "mod mk() -> int { return V + 1 }",
        "mod mk() -> int { SendCustomEvent(CH, 1)\nreturn 1 }",
        "mod mk() -> int { await SleepTicks(_, delay = K)\nreturn 1 }",
        "const J = K + 1\nmod mk() -> int { return J }",
    ] {
        for import in ["import { mk } from \"lk\"", "import \"lk\""] {
            let got = codes(&main(import), &[("lk", body)]);
            assert!(
                got.contains(&"WS002".to_string()),
                "{import} / {body}: {got:?}"
            );
        }
    }
    // Its own constants and its imports' are its to read, and an importer
    // reads what it imports.
    let own = [("lk", "const K2 = 7\nmod mk() -> int { return K2 + 1 }")];
    assert!(
        codes(&main("import { mk } from \"lk\""), &own).is_empty(),
        "{:?}",
        codes(&main("import { mk } from \"lk\""), &own)
    );
    let through = [
        (
            "lk",
            "import { K3 } from \"base\"\nmod mk() -> int { return K3 + 1 }",
        ),
        ("base", "const K3 = 9"),
    ];
    assert!(
        codes(&main("import { mk } from \"lk\""), &through).is_empty(),
        "{:?}",
        codes(&main("import { mk } from \"lk\""), &through)
    );
    let lib = [("lk", "const K4 = 3\nmod mk() -> int { return K4 }")];
    let read = "\nvar y: int = 0\nout oy: int = y\non go { y = K4 }";
    for import in ["import \"lk\"", "import { mk, K4 } from \"lk\""] {
        assert!(
            codes(&format!("{}{read}", main(import)), &lib).is_empty(),
            "{import}"
        );
    }
    let unnamed = codes(
        &format!("{}{read}", main("import { mk } from \"lk\"")),
        &lib,
    );
    assert!(!unnamed.contains(&"WS002".to_string()), "{unnamed:?}");
}

// ── Named arguments, defaults and `fn` across imports ──

/// Lower `main_src` with in-memory imported files, folding off so argument
/// values stay visible on the gates they reach.
fn lower_files(
    main_src: &str,
    files: &[(&str, &str)],
) -> (Vec<crate::diagnostic::Diagnostic>, LowerResult) {
    let files: crate::collections::HashMap<String, String> = files
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    let loader = crate::resolve::MemLoader { files };
    let crate::compile::FrontEnd {
        resolved,
        typecheck: tc,
        lowered,
        ..
    } = crate::compile::front_end(main_src, "main.ws", None, FoldMode::ForceOff, &loader);
    let mut diags = resolved.diagnostics;
    diags.extend(tc.diagnostics);
    diags.extend(lowered.diagnostics.iter().cloned());
    (diags, lowered)
}

fn assert_clean(diags: &[crate::diagnostic::Diagnostic]) {
    assert!(
        diags.is_empty(),
        "{:?}",
        diags
            .iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

fn int_inputs(r: &LowerResult, class: &str, port: &str) -> Vec<i64> {
    let port = crate::intern::intern(port);
    let mut out: Vec<i64> = nodes_of(&r.module, class)
        .iter()
        .filter_map(|n| match n.properties.get(&port) {
            Some(crate::ir::Literal::Int(v)) => Some(*v),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

const BASE: &str = "\
var total: int = 0
fn add(x: int, y: int = 1) -> int {
  total += x
  return x + y
}
mod scale(v: int, by: int = 2) -> int {
  return v * by
}
";

const MUL: &str = "BrickComponentType_Expr_MathMultiply";

/// A named argument binds the `mod` parameter it names. It was dropped, so
/// `scale(n, by = 3)` multiplied by the default 2.
#[test]
fn a_named_mod_argument_binds_its_parameter() {
    let (diags, r) = lower_files(
        "mod scale(v: int, by: int = 2) -> int { return v * by }\nin go: exec\nin n: int\non go {\n  let a = scale(n, by = 3)\n  let b = scale(n)\n  let c = scale(n, 5)\n  BroadcastChatMessage(\"${a} ${b} ${c}\")\n}\n",
        &[],
    );
    assert_clean(&diags);
    assert_eq!(int_inputs(&r, MUL, "InputB"), vec![2, 3, 5]);
}

/// A namespaced call takes the member's defaults and named arguments, as a
/// flat call does. `b.add(n)` was WS022 for the omitted defaulted parameter.
#[test]
fn a_namespaced_call_takes_defaults_and_named_arguments() {
    let (diags, r) = lower_files(
        "import * as b from \"base\"\nin go: exec\nin n: int\non go {\n  let p = b.scale(n)\n  let q = b.scale(n, by = 5)\n  let s = b.add(n)\n  let t = b.add(n, y = 7)\n  BroadcastChatMessage(\"${p} ${q} ${s} ${t}\")\n}\n",
        &[("base.ws", BASE)],
    );
    assert_clean(&diags);
    assert_eq!(int_inputs(&r, MUL, "InputB"), vec![2, 5]);
    let call = crate::ir::gate_class::FUNCTION_CALL;
    assert_eq!(nodes_of(&r.module, call).len(), 2);
    assert_eq!(
        int_inputs(&r, call, "Input1"),
        vec![7],
        "only the named call supplies y"
    );
}

/// A `fn` reached through a chain of imports, from another `fn`, a `mod` and
/// a chip, compiles to one plane per `fn` and one call per call site, and the
/// storage it uses stays a single var.
#[test]
fn a_fn_reached_through_nested_imports_is_one_plane_and_one_var() {
    let mid = "\
import { add } from \"base\"
fn twice(x: int) -> int {
  return add(add(x))
}
mod viaMod(x: int) -> int {
  return add(x, 10)
}
chip Counter(tick: exec) -> (value: int) {
  var n: int = 0
  on tick {
    n = add(n)
  }
  out value = n
}
";
    let (diags, r) = lower_files(
        "import \"mid\"\nin go: exec\non go {\n  let a = twice(3) + viaMod(1)\n  BroadcastChatMessage(\"${a}\")\n}\nlet c = Counter(go)\nout v: int = c.value\n",
        &[("base.ws", BASE), ("mid.ws", mid)],
    );
    assert_clean(&diags);
    // twice (1) + add twice inside twice (2) + add in viaMod (1) + add in Counter (1)
    assert_eq!(
        nodes_of(&r.module, crate::ir::gate_class::FUNCTION_CALL).len(),
        5
    );
    let total_vars = nodes_of(&r.module, "BrickComponentType_Var")
        .iter()
        .filter(|n| matches!(n.properties.get(&*crate::intern::sym::NAME_LABEL), Some(crate::ir::Literal::String(s)) if s == "total"))
        .count();
    assert_eq!(total_vars, 1, "the imported fn's storage is one var");
}
