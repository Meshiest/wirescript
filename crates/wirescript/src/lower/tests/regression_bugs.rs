//! Regressions for three lowering bugs reported together (array aliasing,
//! imported-namespace tuple access, and imported `on` handlers).

use super::*;

fn has_unsupported(r: &LowerResult) -> bool {
    fn walk(m: &crate::ir::Module) -> bool {
        m.nodes.values().any(|n| n.gate_class == "_Unsupported") || m.chips.values().any(walk)
    }
    walk(&r.module)
}

fn count_class(m: &crate::ir::Module, class: &str) -> usize {
    let mut n = m.nodes.values().filter(|x| x.gate_class == class).count();
    for c in m.chips.values() {
        n += count_class(c, class);
    }
    n
}

/// A `let` aliasing an array var must resolve for `x[i]` (and array methods),
/// exactly like the original. It used to bind as a plain value and the index
/// lowered to an `_Unsupported` placeholder.
#[test]
fn let_alias_of_array_var_resolves_index() {
    let r = compile("var a = [0]\nin go: exec\non go { let ar = a\n let v = ar[0] }");
    assert!(
        !has_unsupported(&r),
        "aliased array index lowered to _Unsupported: {:?}",
        r.diagnostics
    );
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
        1,
        "ar[0] must lower to an ArrayVar_Get on the aliased array"
    );
}

/// `let t = Other.member; t.0` where the namespace member's bare name is ALSO
/// owned by the importer (`in start`): the member must still be reachable as
/// `Other.member` (captured into the namespace map without clobbering the local
/// `in start`). Previously `Other.start` fell through to the local input and
/// `test.0` lowered to `_Unsupported`.
#[test]
fn imported_namespace_tuple_pick_resolves() {
    let r = compile_multi(
        "in start: exec\n\
         import * as Other from \"test2\"\n\
         on start { let test = Other.start\n let x = test.0 }",
        &[("test2", "let start = (1,2,3,4)")],
    );
    assert!(
        !has_unsupported(&r),
        "imported-namespace tuple pick lowered to _Unsupported: {:?}",
        r.diagnostics
    );
}

/// A top-level `on` handler in an imported file runs as part of the importing
/// program: its body lowers and wires to its own trigger. It used to be dropped
/// (for `on <expr>` it left a dangling trigger gate with no body).
#[test]
fn imported_on_handler_generates() {
    let r = compile_multi(
        "import \"lib\"\non ReadBrickGrid() { BroadcastChatMessage(\"main\") }",
        &[("lib", "on ReadBrickGrid() { BroadcastChatMessage(\"lib\") }")],
    );
    assert_eq!(
        count_class(
            &r.module,
            "BrickComponentType_WireGraph_Exec_Gamemode_BroadcastChatMessage"
        ),
        2,
        "both the local and the imported handler bodies must generate"
    );
    assert!(
        !has_unsupported(&r),
        "imported handler left an _Unsupported node: {:?}",
        r.diagnostics
    );
}
