//! Under `@layout("code")` the source's own-line `//` comments ride along
//! into the emitted plane as left-anchored `Component_TextDisplay` labels.
//! Trailing comments (code then `//` on the same line) do not.

mod common;

use brdb::schema::BrdbValue;
use wirescript::{CompileInput, FoldMode};

const SRC: &str = "@layout(\"code\")\n\
                   \n\
                   // a standalone note\n\
                   var counter: int = 0\n\
                   in tick: exec\n\
                   on tick { counter = counter + 1 } // a trailing note\n";

/// Every `Component_TextDisplay` text in the serialized save, with the
/// anchor's X component when it carries one.
fn label_texts(brz: &[u8]) -> Vec<(String, Option<f32>)> {
    let mut out = Vec::new();
    for c in common::world_components(brz) {
        // TextDisplay is the only struct here with both Text and Face.
        let (Some(BrdbValue::String(text)), Some(BrdbValue::Enum(_))) =
            (c.get("Text"), c.get("Face"))
        else {
            continue;
        };
        let anchor_x = match c.get("Anchor") {
            Some(BrdbValue::Struct(s)) => match s.get("X") {
                Some(BrdbValue::F32(x)) => Some(*x),
                _ => None,
            },
            _ => None,
        };
        out.push((text.clone(), anchor_x));
    }
    out
}

#[test]
fn own_line_comments_render_as_left_anchored_labels() {
    let cr = wirescript::compile::compile(CompileInput {
        source: SRC,
        file: "comments.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");

    let labels = label_texts(&cr.brz);
    let note = labels
        .iter()
        .find(|(t, _)| t == "a standalone note")
        .unwrap_or_else(|| panic!("own-line comment should render as a label; got {labels:#?}"));
    assert_eq!(
        note.1,
        Some(0.0),
        "comment labels anchor at the left edge of their row"
    );
    assert!(
        !labels.iter().any(|(t, _)| t.contains("a trailing note")),
        "trailing comments stay out of the plane; got {labels:#?}"
    );
}

/// Without `@layout("code")` the plane keeps the topological layout and
/// carries no comment labels.
#[test]
fn dag_layout_renders_no_comment_labels() {
    let src = SRC.replace("@layout(\"code\")\n", "");
    let cr = wirescript::compile::compile(CompileInput {
        source: &src,
        file: "comments_dag.ws",
        module_name: None,
        fold_mode: FoldMode::Auto,
    })
    .expect("should compile to brz");

    let labels = label_texts(&cr.brz);
    assert!(
        !labels.iter().any(|(t, _)| t.contains("a standalone note")),
        "dag layout renders no comment labels; got {labels:#?}"
    );
}
