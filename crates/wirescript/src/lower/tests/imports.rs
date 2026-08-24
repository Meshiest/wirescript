//! Import-shape regressions.

use super::*;
use crate::ir::NodeKind;
use std::collections::HashSet;

fn is_pure(gate_class: &str) -> bool {
    gate_class == "_Literal" || gate_class.starts_with("BrickComponentType_WireGraph_Expr_")
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
        has_gate_class(&r, "BrickComponentType_WireGraph_Exec_Var_Increment"),
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
        has("BrickComponentType_WireGraph_Expr_MathMultiply"),
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
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathMultiply"),
        "the third module's body must be inlined"
    );
    assert!(
        has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"),
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
    assert!(has_gate(&r, "BrickComponentType_WireGraph_Expr_MathAdd"));
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
