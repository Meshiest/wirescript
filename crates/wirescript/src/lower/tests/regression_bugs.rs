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
/// exactly like the original.
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
/// `in start`).
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
/// program: its body lowers and wires to its own trigger.
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

/// A `let` aliasing an INPUT-port array/map (`in a: int[]` then `let x = a`)
/// must resolve for `x[i]` and methods, like a `var` alias.
#[test]
fn let_alias_of_input_array_resolves_index() {
    let r = compile("in a: int[]\nin go: exec\non go { let x = a\n let v = x[0] }");
    assert!(
        !has_unsupported(&r),
        "aliased input-array index lowered to _Unsupported: {:?}",
        r.diagnostics
    );
    assert_eq!(
        count_class(&r.module, "BrickComponentType_WireGraph_Exec_ArrayVar_Get"),
        1,
        "x[0] must lower to an ArrayVar_Get on the aliased input"
    );
}

/// Every node id used as a wire SOURCE anywhere in the module tree.
fn wire_sources(m: &crate::ir::Module) -> crate::collections::HashSet<crate::ir::NodeId> {
    let mut out: crate::collections::HashSet<crate::ir::NodeId> =
        m.wires.iter().map(|w| w.source.node_id).collect();
    for c in m.chips.values() {
        out.extend(wire_sources(c));
    }
    out
}

/// Root-module var gates carrying `name`, by `NAME_LABEL`.
fn var_ids_named(m: &crate::ir::Module, name: &str) -> Vec<crate::ir::NodeId> {
    let mut ids: Vec<crate::ir::NodeId> = m
        .nodes
        .iter()
        .filter(|(_, n)| n.gate_class == "BrickComponentType_WireGraphPseudo_Var")
        .filter(|(_, n)| {
            matches!(
                n.properties.get(&*sym::NAME_LABEL),
                Some(crate::ir::Literal::String(s)) if s == name || s.starts_with(&format!("{name}."))
            )
        })
        .map(|(id, _)| *id)
        .collect();
    ids.sort();
    ids
}

/// A `chip` with a `*T` param, called twice with different vars: the second
/// instance is stamped from the template cache and must remap its captured
/// externals to its own argument, or every call site writes through to the
/// first argument and the later vars go unwired.
#[test]
fn chip_ref_param_rebinds_per_call_site() {
    let r = compile(
        "chip M(self: *float, x: float) -> float {\n  self = x + 2\n  return self * 3.0\n}\n\
         var ta: float = 1.0\nvar tb: float = 2.0\nin foo: float\n\
         var output: float = 0.0\nout result: float = output\n\
         in go: exec\non go {\n  output += M(ta, foo)\n  output += M(tb, foo)\n}",
    );
    let sources = wire_sources(&r.module);
    for name in ["ta", "tb"] {
        let ids = var_ids_named(&r.module, name);
        assert!(!ids.is_empty(), "no var gate named {name}");
        assert!(
            ids.iter().any(|id| sources.contains(id)),
            "`{name}` was passed by ref to a chip but its var gate is wired to \
             nothing, so the instance rebound to the other call site's var"
        );
    }
}

/// Same for a record `*T` param, where the capture is one entry per field.
#[test]
fn chip_record_ref_param_rebinds_per_call_site() {
    let r = compile(
        "type P = { a: float, b: float }\n\
         chip M(self: *P, x: float) -> float {\n  self.a = x + 2\n  return self.a * self.b\n}\n\
         var ta: P = { a: 1.0, b: 1.5 }\nvar tb: P = { a: 2.0, b: 2.5 }\nin foo: float\n\
         var output: float = 0.0\nout result: float = output\n\
         in go: exec\non go {\n  output += M(ta, foo)\n  output += M(tb, foo)\n}",
    );
    let sources = wire_sources(&r.module);
    for name in ["ta", "tb"] {
        let ids = var_ids_named(&r.module, name);
        assert!(!ids.is_empty(), "no var gates for record {name}");
        assert!(
            ids.iter().all(|id| sources.contains(id)),
            "record `{name}` was passed by ref to a chip but some of its field vars \
             are wired to nothing, so the instance rebound to the other call site's record"
        );
    }
}
