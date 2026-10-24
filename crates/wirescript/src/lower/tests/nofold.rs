//! `@nofold`: the `_nofold` barrier property on every gate a declaration
//! lowers to, its scope, and the sites where it has no effect.

use super::*;
use crate::ir::gate_class as gc;

fn is_marked(n: &crate::ir::Node) -> bool {
    n.properties.contains_key(&*crate::intern::sym::NO_FOLD)
}

/// The gates lowered under an `@nofold` declaration carry `_nofold`, however
/// the declaration is reached. Each row: the exact number of marked gates in
/// the whole module tree (when pinned), and a class that must be among them.
#[test]
fn nofold_marks_the_gates_its_declaration_lowers() {
    let rows: &[(&str, &str, Option<usize>, Option<&str>)] = &[
        (
            "a let's subtree",
            "@nofold let a = 1 + 2\nout y = a",
            Some(1),
            Some("BrickComponentType_Expr_MathAdd"),
        ),
        // `@nofold let c = on Trigger { .. }` parses to `TopDecl::Event` (a
        // captured event), not `TopDecl::Let`: the bare-`@nofold` dispatch (parser
        // and lowering wrap) must still tag it instead of dropping the annotation.
        (
            "a captured event",
            "@nofold let c = on RoundStart() { PrintToConsole(\"hi\") }",
            Some(2),
            Some(gc::PRINT_TO_CONSOLE),
        ),
        // `@nofold let x = await sig` parses to `TopDecl::Await`, reached through
        // the statement-level dispatch inside a handler body.
        (
            "an await binding",
            "in run: exec\non run {\n  let sig: exec\n  emit sig = 7\n  @nofold let x = await sig\n  PrintToConsole(\"${x}\")\n}",
            Some(6),
            None,
        ),
        (
            "a handler and a let inside a chip",
            "in t: exec\nchip C() -> (z: int) {\n  @nofold let inner = 1 + 1\n  out z = inner\n}\nvar v: int = 0\n@nofold on t { v = 1 }\nlet c = C()\nout y = c",
            Some(2),
            Some(gc::VAR_SET),
        ),
        // Var gates are created in pass 1 (predeclare), not lower_decl.
        (
            "a var's predeclared gate",
            "@nofold var x: int = 0\nin t: exec\non t { x = 1 }\nout y: int = x",
            Some(1),
            Some(gc::VAR),
        ),
        // Every `pre_declare_var` site inside a chip/mod/anon-chip body wraps.
        (
            "a var inside a named chip body",
            "in t: exec\nchip C() -> (z: int) {\n  @nofold var x: int = 0\n  out z = 1\n}\nlet c = C()\nout y = c",
            None,
            Some(gc::VAR),
        ),
        (
            "a var inside an anonymous chip",
            "in t: exec\nchip {\n  @nofold var w: int = 0\n  on t { w = 1 }\n}",
            None,
            Some(gc::VAR),
        ),
        // The port node created in pass-1 predeclare, not just the value gates.
        (
            "an out's port node",
            "var x: int = 0\nin t: exec\non t { x = 1 }\n@nofold out y = x + 1",
            Some(2),
            Some(gc::MICROCHIP_OUTPUT),
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, count, class) in rows {
        let r = compile(src);
        let errors = crate::test_support::error_codes(&r.diagnostics);
        let marked: Vec<&str> = r
            .module
            .nodes_deep()
            .filter(|n| is_marked(n))
            .map(|n| n.gate_class)
            .collect();
        let count_ok = count.is_none_or(|n| marked.len() == n);
        let class_ok = class.is_none_or(|c| marked.contains(&c));
        if !errors.is_empty() || !count_ok || !class_ok {
            failures.push(format!("{label}: errors {errors:?}, marked {marked:?}, want {count:?} incl. {class:?}\n{src}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// `ModuleBuilder::add_input`/`add_output` bypass `LowerCtx::add_gate`/
/// `add_event`, so a `@nofold` chip's own MicrochipInput/MicrochipOutput
/// boundary nodes once missed `_nofold` while every body gate had it. An
/// ordinary chip's boundary nodes never pick it up from nowhere.
#[test]
fn a_chips_boundary_ports_are_marked_exactly_when_the_chip_is() {
    let rows: &[(&str, bool)] = &[
        (
            "@nofold chip C(a: int) -> (z: int) { out z = a }\nlet c = C(1)\nout y = c",
            true,
        ),
        (
            "chip C(a: int) -> (z: int) { out z = a }\nlet c = C(1)\nout y = c",
            false,
        ),
    ];
    let mut failures = Vec::new();
    for &(src, want) in rows {
        let r = compile_ok(src);
        if r.module.chips.len() != 1 {
            failures.push(format!(
                "{src}: {} chip modules, want 1",
                r.module.chips.len()
            ));
            continue;
        }
        let child = r.module.chips.values().next().unwrap();
        let boundary: Vec<_> = child.inputs.iter().chain(child.outputs.iter()).collect();
        if boundary.is_empty() {
            failures.push(format!("{src}: the chip has no boundary nodes to check"));
        }
        for id in boundary {
            let node = &child.nodes[id];
            if is_marked(node) != want {
                failures.push(format!(
                    "{src}: boundary node {id:?} ({}) marked: {}, want {want}",
                    node.gate_class, !want
                ));
            }
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// `@nofold` + blank line at the top of the file = whole-module scope (the
/// rule module doc comments follow).
#[test]
fn module_level_nofold_marks_everything() {
    let r = compile_ok("@nofold\n\nvar x: int = 0\nlet b = 1 + 2\nout y = b");
    let unmarked: Vec<_> = r
        .module
        .nodes_deep()
        .filter(|n| !is_marked(n))
        .map(|n| n.gate_class)
        .collect();
    assert!(
        unmarked.is_empty(),
        "module-level @nofold must mark every node, unmarked: {unmarked:?}"
    );
}

/// With no blank line, `@nofold` binds to the next declaration only.
#[test]
fn attached_nofold_stays_decl_scoped() {
    let r = compile_ok("@nofold\nlet a = 1 + 2\nlet b = 3 + 4\nout y = a + b");
    assert_eq!(
        r.module.nodes_deep().filter(|n| is_marked(n)).count(),
        1,
        "the annotated decl's one gate is marked"
    );
    assert!(
        r.module.nodes_deep().any(|n| !is_marked(n)),
        "decl-scoped @nofold must not mark sibling decls"
    );
}

/// If emit tried to write `_nofold` as a component field it would error
/// (unknown schema field) or corrupt data; building cleanly is the gate.
#[test]
fn nofold_prop_never_reaches_brick_data() {
    let r = compile_ok("@nofold let a = 1 + 2\nout y = a");
    let lr = crate::layout::layout(&r.module);
    let opts = crate::EmitOptions::default();
    crate::build_world(&r.module, &Default::default(), &lr, &opts).expect("emit must skip _nofold");
}

/// `@nofold` that parses but has no effect WARNS, never silently drops; an
/// effective site does not warn. Parsed directly: the shared compile helpers
/// reject any parse diagnostic.
#[test]
fn nofold_noop_sites_warn() {
    let rows: &[(&str, &str, usize)] = &[
        ("anon chip", "@nofold chip { let x = 1 + 1 }", 1),
        ("anon chip-on", "in t: exec\n@nofold chip on t { }", 1),
        ("in decl", "@nofold in t: exec\non t { }", 1),
        ("let", "@nofold let a = 1 + 2\nout y = a", 0),
        (
            "named chip",
            "@nofold chip C() -> (z: int) { out z = 1 }\nlet c = C()\nout y = c",
            0,
        ),
    ];
    let mut failures = Vec::new();
    for &(label, src, want) in rows {
        let parsed = crate::parser::parse(src, "test");
        let errors = crate::test_support::error_codes(&parsed.diagnostics);
        let warns = parsed
            .diagnostics
            .iter()
            .filter(|d| {
                d.severity == crate::diagnostic::Severity::Warning
                    && d.message.contains("@nofold has no effect")
            })
            .count();
        if !errors.is_empty() || warns != want {
            failures.push(format!(
                "{label}: errors {errors:?}, {warns} no-effect warnings, want {want}\n{src}"
            ));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
