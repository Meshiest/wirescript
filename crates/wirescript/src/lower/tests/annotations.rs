//! `@label` / `@closed` / doc comments land on IR nodes as pseudo-properties.

use crate::intern::sym;
use crate::ir::{Literal, NodeKind};

fn lower_src(src: &str) -> crate::ir::Module {
    let parsed = crate::parser::parse(src, "test");
    assert!(
        parsed.diagnostics.is_empty(),
        "parse diags: {:?}",
        parsed.diagnostics
    );
    let tc = crate::typecheck::typecheck(&parsed.ast, "test");
    let r = crate::lower::lower(crate::lower::LowerInput {
        ast: &parsed.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file: "test",
        module_name: None,
        template_cache: std::sync::Arc::new(crate::template_cache::TemplateCache::new()),
        doc_comments: &parsed.doc_comments,
    });
    assert!(
        r.diagnostics
            .iter()
            .all(|d| !matches!(d.severity, crate::diagnostic::Severity::Error)),
        "lower diags: {:?}",
        r.diagnostics
    );
    r.module
}

fn chip_nodes(m: &crate::ir::Module) -> Vec<&crate::ir::Node> {
    m.nodes.values().filter(|n| n.kind == NodeKind::Chip).collect()
}

/// Recursively collect every `Chip` node across this module and all nested
/// chip sub-modules (`module.chips`).
fn all_chip_nodes<'a>(m: &'a crate::ir::Module, out: &mut Vec<&'a crate::ir::Node>) {
    out.extend(m.nodes.values().filter(|n| n.kind == NodeKind::Chip));
    for child in m.chips.values() {
        all_chip_nodes(child, out);
    }
}

/// Recursively find the first node (anywhere in `module.chips`) whose
/// `PortLabel` property equals `label`.
fn find_node_with_port_label<'a>(
    m: &'a crate::ir::Module,
    label: &str,
) -> Option<&'a crate::ir::Node> {
    for n in m.nodes.values() {
        if matches!(
            n.properties.get(&*sym::PORT_LABEL),
            Some(Literal::String(s)) if s == label
        ) {
            return Some(n);
        }
    }
    for child in m.chips.values() {
        if let Some(n) = find_node_with_port_label(child, label) {
            return Some(n);
        }
    }
    None
}

#[test]
fn anon_chip_gets_label_closed_and_doc_props() {
    let m = lower_src(
        "/// Keeps the score.\n@label(\"Score Tracker\") @closed chip { var a: int = 0 }",
    );
    let chips = chip_nodes(&m);
    assert_eq!(chips.len(), 1);
    let n = chips[0];
    assert_eq!(
        n.properties.get(&*sym::NAME_LABEL),
        Some(&Literal::String("Score Tracker".into()))
    );
    assert_eq!(
        n.properties.get(&*sym::CHIP_CLOSED),
        Some(&Literal::Bool(true))
    );
    assert_eq!(
        n.properties.get(&*sym::DOC_TEXT),
        Some(&Literal::String("Keeps the score.".into()))
    );
}

#[test]
fn chip_let_labels_the_chip_with_the_binding_name() {
    // `chip let x = ...` has no name of its own; it should display the binding
    // name so the compiled let-chip is identifiable in the wire graph.
    let m = lower_src("@closed chip let sphereCheckSize = 5");
    let chips = chip_nodes(&m);
    assert_eq!(chips.len(), 1);
    assert_eq!(
        chips[0].properties.get(&*sym::NAME_LABEL),
        Some(&Literal::String("sphereCheckSize".into()))
    );

    // Multiple bindings (`chip let a = 1, b = 2`) join their names.
    let m2 = lower_src("chip let a = 1, b = 2");
    assert_eq!(
        chip_nodes(&m2)[0].properties.get(&*sym::NAME_LABEL),
        Some(&Literal::String("a, b".into()))
    );

    // An explicit `@label(...)` still wins over the derived name.
    let m3 = lower_src("@label(\"Reach\") chip let sphereCheckSize = 5");
    assert_eq!(
        chip_nodes(&m3)[0].properties.get(&*sym::NAME_LABEL),
        Some(&Literal::String("Reach".into()))
    );
}

#[test]
fn named_chip_instances_inherit_decl_annotations_and_doc() {
    let m = lower_src(
        "/// Adds one.\n\
         @closed chip Foo(x: int) -> (r: int) { out r = x + 1 }\n\
         let a = Foo(1)\n\
         let b = Foo(2)\n\
         out o = a.r + b.r",
    );
    let chips = chip_nodes(&m);
    assert_eq!(chips.len(), 2, "two Foo instances");
    for n in chips {
        assert_eq!(
            n.properties.get(&*sym::CHIP_CLOSED),
            Some(&Literal::Bool(true))
        );
        assert_eq!(
            n.properties.get(&*sym::DOC_TEXT),
            Some(&Literal::String("Adds one.".into()))
        );
    }
}

#[test]
fn default_chip_has_no_closed_or_open_prop() {
    let m = lower_src("chip { var a: int = 0 }\nopen chip { var b: int = 0 }");
    for n in chip_nodes(&m) {
        assert!(n.properties.get(&*sym::CHIP_CLOSED).is_none());
        assert!(
            n.properties.get(&crate::intern::intern("_open")).is_none(),
            "_open is retired"
        );
    }
}

#[test]
fn port_label_prop_set_alongside_port_label() {
    let m = lower_src("@label(\"Fire!\") in t: exec\nout done = t");
    let node = m
        .inputs
        .iter()
        .filter_map(|id| m.nodes.get(id))
        .find(|n| {
            matches!(
                n.properties.get(&*sym::PORT_LABEL),
                Some(Literal::String(s)) if s == "t"
            )
        })
        .expect("input node for t");
    assert_eq!(
        node.properties.get(&*sym::NAME_LABEL),
        Some(&Literal::String("Fire!".into()))
    );
}

#[test]
fn stmt_level_annotated_nested_chip_gets_doc() {
    // Regression: a statement-level chip decl that ALSO carries an
    // annotation (`@label(...)`) used to lose its doc comment, because
    // `parse_block` keyed the pending doc by the `@` token's offset rather
    // than the chip decl's own range start (which is what lowering looks
    // doc comments up by). The nested chip lives inside an `on run { ... }`
    // handler block (rather than directly in Outer's top-level body) so the
    // statement is lowered via `lower_block`'s ordinary block-statement path.
    let m = lower_src(
        "chip Outer(x: int) -> (r: int) {\n\
         in run: exec\n\
         on run {\n\
         /// Inner doc.\n\
         @label(\"Inner\") chip { var a: int = 0 }\n\
         }\n\
         out r = x\n\
         }\n\
         let o = Outer(1)\n\
         out result = o.r",
    );
    let mut chips = Vec::new();
    all_chip_nodes(&m, &mut chips);
    let n = chips
        .iter()
        .find(|n| {
            matches!(
                n.properties.get(&*sym::NAME_LABEL),
                Some(Literal::String(s)) if s == "Inner"
            )
        })
        .expect("chip node labeled \"Inner\" (searched root module + all module.chips)");
    assert_eq!(
        n.properties.get(&*sym::DOC_TEXT),
        Some(&Literal::String("Inner doc.".into())),
        "doc comment on a statement-level annotated chip must survive lowering"
    );
}

#[test]
fn stmt_level_annotated_in_inside_named_chip_gets_label() {
    // Coverage gap noted in review: `@label(...) in p: ...` declared as a
    // statement inside a *named* chip's body (not just at top level) must
    // still stamp NAME_LABEL on the resulting port node.
    let m = lower_src(
        "chip Foo(x: int) -> (r: int) {\n\
         @label(\"Pretty\") in p: int\n\
         out r = x + p\n\
         }\n\
         let a = Foo(1)\n\
         out result = a.r",
    );
    let n = find_node_with_port_label(&m, "p")
        .expect("port node for 'p' inside Foo's chip module");
    assert_eq!(
        n.properties.get(&*sym::NAME_LABEL),
        Some(&Literal::String("Pretty".into()))
    );
}

#[test]
fn multi_line_doc_joins_with_newlines() {
    let m = lower_src("/// Line one.\n/// Line two.\nchip { var a: int = 0 }");
    let chips = chip_nodes(&m);
    assert_eq!(
        chips[0].properties.get(&*sym::DOC_TEXT),
        Some(&Literal::String("Line one.\nLine two.".into()))
    );
}
