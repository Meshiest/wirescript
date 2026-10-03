//! Import-shape regressions.

use super::*;
use crate::ir::NodeKind;
use std::collections::HashSet;

fn is_pure(gate_class: &str) -> bool {
    gate_class == "_Literal" || gate_class.starts_with("BrickComponentType_Expr_")
}

/// Node ids that appear in any wire, as source or target.
fn wired_nodes(r: &LowerResult) -> HashSet<crate::ir::NodeId> {
    let mut set = HashSet::default();
    fn walk(m: &crate::ir::Module, set: &mut HashSet<crate::ir::NodeId>) {
        for w in &m.wires {
            set.insert(w.source.node_id);
            set.insert(w.target.node_id);
        }
        for c in m.chips.values() {
            walk(c, set);
        }
    }
    walk(&r.module, &mut set);
    set
}

fn orphan_pure_gates(r: &LowerResult) -> Vec<&'static str> {
    let wired = wired_nodes(r);
    fn walk<'a>(
        m: &'a crate::ir::Module,
        wired: &HashSet<crate::ir::NodeId>,
        out: &mut Vec<&'static str>,
    ) {
        for n in m.nodes.values() {
            if n.kind == NodeKind::Gate && is_pure(n.gate_class) && !wired.contains(&n.id) {
                out.push(n.gate_class);
            }
        }
        for c in m.chips.values() {
            walk(c, wired, out);
        }
    }
    let mut out = Vec::new();
    walk(&r.module, &wired, &mut out);
    out
}

/// True if any module in the tree contains a gate of `class`.
fn has_gate_class(r: &LowerResult, class: &str) -> bool {
    fn walk(m: &crate::ir::Module, class: &str) -> bool {
        m.nodes.values().any(|n| n.gate_class == class) || m.chips.values().any(|c| walk(c, class))
    }
    walk(&r.module, class)
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
        has_gate_class(&r, "BrickComponentType_Exec_Var_Increment"),
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
    let r = compile(
        "\
var x: int = 5
in player: character
on player { let y = x * 2 + 1 }",
    );
    assert_no_errors(&r);
    let has = |cls: &str| r.module.nodes.values().any(|n| n.gate_class == cls);
    assert!(
        has("BrickComponentType_Expr_MathMultiply"),
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
        !has_gate(&r, "_Unsupported"),
        "the nested namespace call must resolve; gates: {:?}",
        r.module
            .nodes
            .values()
            .map(|n| n.gate_class)
            .collect::<Vec<_>>()
    );
    // `n * 2` from the third module and the `+ 1` from the second must both
    // survive — a dropped namespace loses the multiply entirely.
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathMultiply"),
        "the third module's body must be inlined"
    );
    assert!(
        has_gate(&r, "BrickComponentType_Expr_MathAdd"),
        "the second module's arithmetic must survive"
    );
}

/// A namespaced `mod` must expose its declared return type. Typing the call as
/// `Any` made any arithmetic on the result fail operator resolution, dropping
/// the whole expression to an unsupported gate.
#[test]
fn namespaced_mod_call_keeps_its_return_type() {
    let r = compile_multi(
        "import * as Foo from \"third\"\nout result = Foo.blah(5) + 1",
        &[("third", "mod blah(n: int) -> int {\n  return n * 2\n}")],
    );
    assert_no_errors(&r);
    assert!(
        !has_gate(&r, "_Unsupported"),
        "a namespaced mod's return type must resolve the '+' overload"
    );
    assert!(has_gate(&r, "BrickComponentType_Expr_MathAdd"));
}

// ---------- `const` across files ----------

/// The `InitialValue` baked into the first array gate of a cross-file program,
/// child chip modules included.
fn baked_array_multi(entry: &str, deps: &[(&str, &str)]) -> Vec<crate::ir::Literal> {
    let r = compile_multi(entry, deps);
    assert_no_errors(&r);
    fn find(m: &crate::ir::Module) -> Option<Vec<crate::ir::Literal>> {
        for n in m.nodes.values() {
            if let Some(crate::ir::Literal::Array(items)) =
                n.properties.get(&crate::intern::intern_static("InitialValue"))
            {
                return Some(items.clone());
            }
        }
        m.chips.values().find_map(find)
    }
    find(&r.module).expect("no array gate with a baked InitialValue")
}

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

/// A named import of a top-level `const` is compile-time in the importer.
#[test]
fn a_named_imported_const_is_compile_time() {
    assert_eq!(
        baked_array_multi(
            "import { SLOTS } from \"lib\"\nvar m: int[] = [SLOTS, 12345]",
            &[("lib", CONST_LIB)]
        ),
        vec![crate::ir::Literal::Int(4), crate::ir::Literal::Int(12345)]
    );
}

/// An imported `const mod` answers a call made from a constant-only position,
/// rather than lowering as ordinary gates.
#[test]
fn an_imported_const_mod_answers_a_const_position_call() {
    assert_eq!(
        baked_array_multi(
            "import { triple } from \"lib\"\nvar m: int[] = [triple(5), 12345]",
            &[("lib", CONST_LIB)]
        ),
        vec![crate::ir::Literal::Int(15), crate::ir::Literal::Int(12345)]
    );
}

/// A DESTRUCTURED top-level `const` crossing a named import. This could not be
/// imported at all: `resolve::decl_name` answers "what is this declaration
/// CALLED", which is `None` for every destructuring binding form, so the named
/// import's lookup found nothing and reported WS012 "'PX' not found in 'lib'".
/// `resolve::decl_names` answers the right question — every name it INTRODUCES
/// — via the same `const_eval::bound_names` the constant environment uses.
#[test]
fn a_destructured_const_survives_a_named_import() {
    assert_eq!(
        baked_array_multi(
            "import { PX, PY } from \"lib\"\nvar m: int[] = [PX, PY, 12345]",
            &[("lib", CONST_LIB)]
        ),
        vec![
            crate::ir::Literal::Int(11),
            crate::ir::Literal::Int(22),
            crate::ir::Literal::Int(12345)
        ]
    );
}

/// Aliasing one name out of a destructured `const` renames THAT name only and
/// keeps it bound to its own field — an alias that renamed the wrong position
/// would silently import the sibling's value.
#[test]
fn a_destructured_const_may_be_imported_under_an_alias() {
    assert_eq!(
        baked_array_multi(
            "import { PX as QX } from \"lib\"\nvar m: int[] = [QX, 12345]",
            &[("lib", CONST_LIB)]
        ),
        vec![crate::ir::Literal::Int(11), crate::ir::Literal::Int(12345)]
    );
}

/// `import "lib"` (import-all) pushes every importable declaration and only
/// consults `decl_name` to skip duplicates, sharing the multi-name duplicate
/// check pinned below.
#[test]
fn a_destructured_const_survives_an_import_all() {
    assert_eq!(
        baked_array_multi(
            "import \"lib\"\nvar m: int[] = [PX, PY, 12345]",
            &[("lib", CONST_LIB)]
        ),
        vec![
            crate::ir::Literal::Int(11),
            crate::ir::Literal::Int(22),
            crate::ir::Literal::Int(12345)
        ]
    );
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
        .filter_map(|n| match n.properties.get(&crate::intern::intern_static("EventName")) {
            Some(crate::ir::Literal::String(s)) => Some(s.clone()),
            _ => None,
        })
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
    let deps = [("leaf", leaf), ("mid", mid)];
    assert_eq!(
        baked_array_multi(entry, &deps),
        vec![
            crate::ir::Literal::Int(6),
            crate::ir::Literal::Int(7),
            crate::ir::Literal::Int(12),
            crate::ir::Literal::Int(12345)
        ]
    );
    let r = compile_multi(entry, &deps);
    assert!(
        r.module.nodes.values().any(|n| matches!(
            n.properties.get(&crate::intern::intern_static("EventName")),
            Some(crate::ir::Literal::String(s)) if s == "leafchan"
        )),
        "a const two imports away must still bake into a constant-only slot"
    );
}

/// NAMESPACE access to an imported constant (`import * as ns` then `ns.NAME`)
/// is compile-time: `build_const_env` evaluates each imported namespace's
/// constants in isolation and seeds them under `"ns.NAME"` keys, and
/// `const_eval`'s `FieldAccess` arm resolves a namespaced member from those, so
/// a namespaced const bakes into a constant-only slot exactly like a named
/// import. (It also still works as a runtime value — see below.)
#[test]
fn a_namespaced_const_folds_at_compile_time() {
    let entry = "import * as lib from \"lib\"\nvar m: int[] = [lib.SLOTS, 12345]";
    assert_eq!(
        baked_array_multi(entry, &[("lib", CONST_LIB)]),
        vec![crate::ir::Literal::Int(4), crate::ir::Literal::Int(12345)]
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
    assert!(!has_gate_class(&r, "_Unsupported"));
}

/// The same namespaced constant used as a RUNTIME value is fine, which is what
/// makes the limit above a missing const-eval feature rather than a broken
/// import.
#[test]
fn a_namespaced_const_still_works_as_a_runtime_value() {
    let r = compile_multi(
        "import * as lib from \"lib\"\nout r = lib.SLOTS + 1",
        &[("lib", CONST_LIB)],
    );
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
}

/// Two DIFFERENT modules declaring the same top-level name, merged via plain
/// `import`, collide with WS013 rather than silently dropping one and
/// aliasing both onto a single storage gate. A DIAMOND import (the same module
/// reached via two paths) still dedups without a false duplicate.
#[test]
fn plain_import_same_name_from_two_modules_is_ws013() {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: [
            ("m1.ws".to_string(), "var g: int = 1".to_string()),
            ("m2.ws".to_string(), "var g: int = 2".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let resolved = resolve("import \"m1\"\nimport \"m2\"", "main", &loader);
    let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
    let codes: Vec<&str> = tc.diagnostics.iter().map(|d| d.code.as_ref()).collect();
    assert!(
        codes.contains(&"WS013"),
        "cross-module name collision must be WS013, got {codes:?}"
    );
}

#[test]
fn named_import_same_name_from_two_modules_is_ws013() {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: [
            ("m1.ws".to_string(), "var dat: int = 1".to_string()),
            ("m3.ws".to_string(), "var dat: int = 2".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let resolved = resolve(
        "import { dat } from \"m1\"\nimport { dat } from \"m3\"",
        "main",
        &loader,
    );
    let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
    let codes: Vec<&str> = tc.diagnostics.iter().map(|d| d.code.as_ref()).collect();
    assert!(
        codes.contains(&"WS013"),
        "cross-module named-import collision must be WS013, got {codes:?}"
    );
}

/// A genuine diamond import (one module reached through two others) must NOT
/// trip the collision check: `util` stays a single shared declaration.
#[test]
fn diamond_import_of_one_module_does_not_collide() {
    use crate::resolve::{MemLoader, resolve};
    let loader = MemLoader {
        files: [
            ("util.ws".to_string(), "var shared: int = 5".to_string()),
            ("dm1.ws".to_string(), "import \"util\"".to_string()),
            ("dm2.ws".to_string(), "import \"util\"".to_string()),
        ]
        .into_iter()
        .collect(),
    };
    let resolved = resolve("import \"dm1\"\nimport \"dm2\"", "main", &loader);
    let tc = typecheck(&resolved.ast, "main", &crate::typecheck::CeSlotMap::default());
    let errors: Vec<&str> = tc
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.as_ref())
        .collect();
    assert!(
        !errors.contains(&"WS013"),
        "diamond import must not false-collide, got {errors:?}"
    );
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
    assert!(!has_gate(&r, "_Unsupported"));
    let m = &r.module;
    let source = |id: crate::ir::NodeId, port: WirePort| {
        m.wires.iter().find(|w| w.target.node_id == id && w.target.port == port).map(|w| w.source.node_id)
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
        n.ports.inputs.iter().chain(&n.ports.outputs).find(|p| p.name == port.sym()).map(|p| p.ty.clone())
    };
    // `u.id(a) + 1`: an int add of `a` itself.
    let add = super::const_params::nodes_of(m, "BrickComponentType_Expr_MathAdd");
    assert_eq!(add.len(), 1);
    assert_eq!(port_ty(add[0], WirePort::Output), Some(Type::Int));
    assert_eq!(source(add[0].id, WirePort::InputA), Some(input("a")));
    // `t = u.id(s)`: the string itself, stored as a string.
    let sets = super::const_params::nodes_of(m, crate::ir::gate_class::VAR_SET);
    let store_t = sets.iter().find(|n| source(n.id, WirePort::Value) == Some(input("s"))).expect("t = s");
    assert_eq!(port_ty(store_t, WirePort::Value), Some(Type::String));
    // `u.pick(c, f, 2.5)`: a float Select between `f` and 2.5 on `c`.
    let select = super::const_params::nodes_of(m, crate::ir::gate_class::SELECT);
    assert_eq!(select.len(), 1);
    assert_eq!(port_ty(select[0], WirePort::Output), Some(Type::Float));
    let operands = [source(select[0].id, WirePort::InputA), source(select[0].id, WirePort::InputB)];
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
    assert_eq!(sets_fed_by(&r, "BrickComponentType_Expr_MathAbs"), 1, "fo = abs(fl) stores the builtin's result");
    assert!(sets_string(&r, "neg") && sets_string(&r, "pos"), "useabs still calls the module's abs");
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
    assert_eq!(sets_fed_by(&r, "BrickComponentType_Expr_MathAbs"), 1, "helperabs calls the builtin");
    assert!(sets_string(&r, "mine"), "the importer's own call is its own mod");
}

/// The members of an `import * as` module are reached as `u.f`; a bare
/// `abs(fl)` in the importer is the builtin, and the module's members still
/// call their sibling.
#[test]
fn a_namespace_member_named_like_a_builtin_does_not_replace_it() {
    let lib = "mod abs(x: float) -> float { return x * 2.0 }\nmod twice(x: float) -> float { return abs(x) }";
    for (main, sibling_calls) in [
        ("var fl: float = -1.5\nvar fo: float = 0.0\nin go: exec\nimport * as u from \"lib\"\non go { fo = abs(fl) }", 0),
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
        assert_eq!(sets_fed_by(&r, "BrickComponentType_Expr_MathAbs"), 1, "{main}");
        assert_eq!(sets_fed_by(&r, "BrickComponentType_Expr_MathMultiply"), sibling_calls, "{main}");
    }
}

/// A whole-module import names every declaration of the module, so its `abs`
/// is the one called, as before.
#[test]
fn a_whole_module_import_names_its_mod_over_the_builtin() {
    let main = "var k: int = -3\nvar so: string = \"\"\nin go: exec\nimport \"lib\"\non go { var t = abs(k)\n so = t }";
    let r = compile_multi(main, &[("lib", ABS_LIB)]);
    assert_no_errors(&r);
    assert!(!has_gate_class(&r, "BrickComponentType_Expr_MathAbs"));
    assert!(sets_string(&r, "neg"));
    // Named by a named import too.
    let main = "var k: int = -3\nvar so: string = \"\"\nin go: exec\nimport { abs } from \"lib\"\non go { so = abs(k) }";
    let r = compile_multi(main, &[("lib", ABS_LIB)]);
    assert_no_errors(&r);
    assert!(!has_gate_class(&r, "BrickComponentType_Expr_MathAbs"));
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
    assert!(ints.contains(&99), "useabs still calls the module's const mod: {ints:?}");
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
    let r = compile(main);
    assert_no_errors(&r);
    assert!(!has_gate_class(&r, "BrickComponentType_Expr_String_Length"));
    assert_eq!(float_sets(&r), [777.0]);
    let lib = "mod Length(self: vector) -> float { return 777.0 }\n\
               mod dist2(self: vector, o: vector) -> float { return 5.0 }\n\
               mod pub1(v: vector) -> float { return v.Length() + v.dist2(v) }";
    let main = "import { pub1 } from \"lib\"\nvar vv: vector = Vec(1.0, 2.0, 3.0)\nvar fo: float = 0.0\n\
                in go: exec\non go { fo = pub1(vv) }";
    let r = compile_multi(main, &[("lib", lib)]);
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(!has_gate_class(&r, "BrickComponentType_Expr_String_Length"));
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
        r.diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.to_string())
            .collect()
    };
    for (body, code, says) in [
        ("fo = fl.abs()", "WS036", "takes no receiver"),
        ("fo = vv.Length()", "WS003", "Magnitude(vv)"),
        ("fo = fl.twice()", "WS036", "`twice` is not declared in `main` and `main` does not import it"),
        ("fo = pub2(fl, vv)
 fo = fl.abs()", "WS036", "takes no receiver"),
    ] {
        let r = compile_multi(&program("import { pub2 } from \"lib\"", body), &[("lib", lib)]);
        assert_eq!(error_codes(&r), [code], "`{body}`");
        let d = r.diagnostics.iter().find(|d| d.code == code).unwrap();
        assert!(d.message.contains(says), "`{body}`: {}", d.message);
    }
    // The library's own body still reaches all three.
    let r = compile_multi(&program("import { pub2 } from \"lib\"", "fo = pub2(fl, vv)"), &[("lib", lib)]);
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
    assert!(!has_gate_class(&r, "BrickComponentType_Expr_String_Length"));
    assert!(!has_gate_class(&r, "BrickComponentType_Expr_MathAbs"));
    // A private plain mod follows the same rule.
    let r = compile_multi(&program("import { pub2 } from \"lib\"", "fo = twice(fl)"), &[("lib", lib)]);
    assert_eq!(error_codes(&r), ["WS002"]);
    // Named by the importer: its own calls reach the `self` mod.
    for import in ["import { Length } from \"lib\"", "import \"lib\""] {
        let r = compile_multi(&program(import, "fo = vv.Length()"), &[("lib", lib)]);
        assert_no_errors(&r);
        assert!(!r.diagnostics.iter().any(|d| d.code == "WS014"), "{import}: {:?}", r.diagnostics);
        assert!(!has_gate_class(&r, "BrickComponentType_Expr_String_Length"), "{import}");
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
    let codes: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::diagnostic::Severity::Error)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["WS013"], "{:?}", r.diagnostics);
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
    assert!(has_gate_class(&r, "BrickComponentType_Expr_String_Length"));
    assert!(has_gate_class(&r, "BrickComponentType_Expr_Select"));
    assert!(!has_gate_class(&r, "BrickComponentType_Exec_Branch"));
    assert_eq!(float_sets(&r), [777.0], "the library's own call still reaches its mod");
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
    for body in ["const Q = sq(2)
on go { n = Q }", "on go {
const Q = sq(2)
n = Q
}", "on go { n = m(sq(2)) }"] {
        let r = compile_multi(&format!("{head}{body}"), &[("lib", lib)]);
        let codes: Vec<&str> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.code.as_ref())
            .collect();
        assert_eq!(codes, ["WS002"], "{body}: {:?}", r.diagnostics);
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
        ("import { pubh } from \"lib\"", "pubh(fl)", &[("WS036", "hook")][..]),
        ("import { pubp } from \"lib\"", "pubp(fl)", &[("WS002", "hookp")][..]),
        ("import \"lib\"", "pubh(fl)", &[("WS036", "hook"), ("WS002", "hookp")][..]),
    ] {
        let r = compile_multi(&format!("{import}\n{head}on go {{ fo = {call} }}"), &[("lib", lib)]);
        let errs = errors(&r);
        assert_eq!(errs.len(), expect.len(), "{import} {call}: {errs:?}");
        for (e, (code, name)) in errs.iter().zip(expect) {
            assert_eq!((e.code.as_ref(), &*e.range.file), (*code, "lib.ws"), "{import} {call}");
            let m = &e.message;
            assert!(m.contains(&format!("`{name}` is not declared in `lib.ws` and `lib.ws` does not import it")), "{m}");
            assert!(m.contains("`main` imports `lib.ws`"), "{m}");
        }
    }
    // A sibling module the importer also imports.
    let a = "mod pa(v: float) -> float { return pb(v) }";
    let b = "mod pb(v: float) -> float { return v * 2.0 }";
    let main = "import \"a\"\nimport \"b\"\nvar fl: float = 1.5\nvar fo: float = 0.0\nin go: exec\non go { fo = pa(fl) }";
    let errs = errors(&compile_multi(main, &[("a", a), ("b", b)]));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!((errs[0].code.as_ref(), &*errs[0].range.file), ("WS002", "a.ws"));
    assert!(errs[0].message.contains("Import it into `a.ws`"), "{}", errs[0].message);
    // Declared in the library, both answer.
    let lib = format!(
        "mod hook(self: float) -> float {{ return self * 9.0 }}\n\
         mod hookp(v: float) -> float {{ return v * 9.0 }}\n{lib}"
    );
    let main = "import { pubh, pubp } from \"lib\"\nvar fl: float = 1.5\nvar fo: float = 0.0\nin go: exec\n\
                on go { fo = pubh(fl) + pubp(fl) }";
    let r = compile_multi(main, &[("lib", &lib)]);
    assert_no_errors(&r);
    assert!(!has_gate(&r, "_Unsupported"));
}

/// A library names the top-level values (constants, `let`s, storage, ports)
/// it declares or imports, as it names types and mods: a library body reading
/// a name only its importer declares is `WS002`, in a value, a constant-only
/// slot, a `Sleep` delay or a constant's initializer. It read the importer's
/// value before. An importer reads a library's value as it did.
#[test]
fn a_library_names_the_values_it_declares_or_imports() {
    let codes = |main: &str, deps: &[(&str, &str)]| super::shadow_differential::outcome(main, deps, false).codes;
    let main = |import: &str| {
        format!("{import}\nconst K = 4\nconst CH = \"evt\"\nlet L = 5\nvar V: int = 6\nin go: exec\nvar x: int = 0\nout ox: int = x\non go {{ x = mk() }}")
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
            assert!(got.contains(&"WS002".to_string()), "{import} / {body}: {got:?}");
        }
    }
    // Its own constants and its imports' are its to read, and an importer
    // reads what it imports.
    let own = [("lk", "const K2 = 7\nmod mk() -> int { return K2 + 1 }")];
    assert!(codes(&main("import { mk } from \"lk\""), &own).is_empty(), "{:?}", codes(&main("import { mk } from \"lk\""), &own));
    let through = [("lk", "import { K3 } from \"base\"\nmod mk() -> int { return K3 + 1 }"), ("base", "const K3 = 9")];
    assert!(codes(&main("import { mk } from \"lk\""), &through).is_empty(), "{:?}", codes(&main("import { mk } from \"lk\""), &through));
    let lib = [("lk", "const K4 = 3\nmod mk() -> int { return K4 }")];
    let read = "\nvar y: int = 0\nout oy: int = y\non go { y = K4 }";
    for import in ["import \"lk\"", "import { mk, K4 } from \"lk\""] {
        assert!(codes(&format!("{}{read}", main(import)), &lib).is_empty(), "{import}");
    }
    let unnamed = codes(&format!("{}{read}", main("import { mk } from \"lk\"")), &lib);
    assert!(!unnamed.contains(&"WS002".to_string()), "{unnamed:?}");
}
