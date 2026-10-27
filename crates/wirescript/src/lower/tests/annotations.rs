//! `@label` / `@closed` / doc comments land on IR nodes as pseudo-properties.

use super::compile_ok_auto;
use crate::intern::sym;
use crate::ir::{Literal, NodeKind};

fn lower_src(src: &str) -> crate::ir::Module {
    compile_ok_auto(src).module
}

fn chip_nodes(m: &crate::ir::Module) -> Vec<&crate::ir::Node> {
    m.nodes
        .values()
        .filter(|n| n.kind == NodeKind::Chip)
        .collect()
}

fn label_of(n: &crate::ir::Node) -> Option<&Literal> {
    n.properties.get(&*sym::NAME_LABEL)
}

/// Which node a label row reads.
enum Find {
    /// The program's one root-level chip node.
    Chip,
    /// The root module's first output node.
    Output,
    /// The port node (anywhere in the tree) whose `PortLabel` is this name.
    Port(&'static str),
}

/// `@label(...)` sets the display `NAME_LABEL`, folding a constant expression
/// the way `FormatText` renders it; a `chip let` without one is labelled with
/// its binding names.
#[test]
fn label_annotations_set_the_display_name() {
    let rows: &[(&str, &str, Find, &str)] = &[
        // `chip let x = ...` has no name of its own; it displays the binding name.
        (
            "chip let takes its binding name",
            "@closed chip let sphereCheckSize = 5",
            Find::Chip,
            "sphereCheckSize",
        ),
        (
            "chip let with several bindings joins their names",
            "chip let a = 1, b = 2",
            Find::Chip,
            "a, b",
        ),
        (
            "an explicit @label wins over the derived name",
            "@label(\"Reach\") chip let sphereCheckSize = 5",
            Find::Chip,
            "Reach",
        ),
        (
            "anon chip label folds arithmetic",
            "@label(1 + 2) chip { }",
            Find::Chip,
            "3",
        ),
        // A folded float label reads the 3-decimal, trailing-zero-trimmed way
        // FormatText renders a float, not `f64::to_string()`. (An uncalled named
        // chip has no instance node, so the anon-chip form carries the label.)
        (
            "a float label rounds by the format render law",
            "@label(0.33333) chip { }",
            Find::Chip,
            "0.333",
        ),
        (
            "out carries an explicit string label",
            "@label(\"hi\") out v: int = 0",
            Find::Output,
            "hi",
        ),
        (
            "an out label expression folds via a constant let",
            "let title = \"Score\"\n@label(title) out v: int = 0",
            Find::Output,
            "Score",
        ),
        (
            "an input's label sits beside its port label",
            "@label(\"Fire!\") in t: exec\nout done = t",
            Find::Port("t"),
            "Fire!",
        ),
        // A statement-level `@label(...) in p` inside a NAMED chip's body.
        (
            "a port declared inside a named chip keeps its label",
            "chip Foo(x: int) -> (r: int) {\n@label(\"Pretty\") in p: int\nout r = x + p\n}\nlet a = Foo(1)\nout result = a.r",
            Find::Port("p"),
            "Pretty",
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, find, want) in rows {
        let m = lower_src(src);
        let node = match find {
            Find::Chip => {
                let chips = chip_nodes(&m);
                if chips.len() != 1 {
                    failures.push(format!("{label}: {} chip nodes, want 1\n{src}", chips.len()));
                    continue;
                }
                Some(chips[0])
            }
            Find::Output => m.outputs.iter().find_map(|id| m.nodes.get(id)),
            Find::Port(name) => m.nodes_deep().find(|n| {
                matches!(n.properties.get(&*sym::PORT_LABEL), Some(Literal::String(s)) if s == name)
            }),
        };
        let got = node.and_then(label_of);
        if got != Some(&Literal::String((*want).into())) {
            failures.push(format!("{label}: label {got:?}, want {want:?}\n{src}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}

/// `@label` is legal directly on `var` and overrides the name-derived default.
#[test]
fn var_can_carry_explicit_label() {
    let m = lower_src("@label(\"HP\") var hp: int = 0");
    assert!(
        m.nodes
            .values()
            .any(|n| matches!(label_of(n), Some(Literal::String(s)) if s == "HP")),
        "var node labeled \"HP\""
    );
}

#[test]
fn anon_chip_gets_label_closed_and_doc_props() {
    let m = lower_src(
        "/// Keeps the score.\n@label(\"Score Tracker\") @closed chip { var a: int = 0 }",
    );
    let chips = chip_nodes(&m);
    assert_eq!(chips.len(), 1);
    let n = chips[0];
    assert_eq!(label_of(n), Some(&Literal::String("Score Tracker".into())));
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
    let m = lower_src("chip { var a: int = 0 }\nchip { var b: int = 0 }");
    let chips = chip_nodes(&m);
    assert_eq!(chips.len(), 2);
    for n in chips {
        assert!(n.properties.get(&*sym::CHIP_CLOSED).is_none());
        assert!(
            n.properties.get(&crate::intern::intern("_open")).is_none(),
            "_open is retired"
        );
    }
}

#[test]
fn stmt_level_annotated_nested_chip_gets_doc() {
    // A statement-level chip decl that also carries an annotation (`@label(...)`)
    // keeps its doc comment: the pending doc is keyed by the chip decl's own range
    // start, not the `@` token. The chip sits in an `on run { ... }` block so it is
    // lowered through `lower_block`'s ordinary block-statement path.
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
    let n = m
        .nodes_deep()
        .filter(|n| n.kind == NodeKind::Chip)
        .find(|n| matches!(label_of(n), Some(Literal::String(s)) if s == "Inner"))
        .expect("chip node labeled \"Inner\" (searched root module + all module.chips)");
    assert_eq!(
        n.properties.get(&*sym::DOC_TEXT),
        Some(&Literal::String("Inner doc.".into())),
        "doc comment on a statement-level annotated chip must survive lowering"
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
