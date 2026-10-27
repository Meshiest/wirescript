use super::*;
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;

#[test]
fn table_loads() {
    assert_eq!(
        doc_url(Doc::Diagnostic("WS012")).as_deref(),
        Some("https://wirescript.brickadia.dev/docs/diagnostics.html#names-declarations--imports"),
    );
    assert_eq!(
        doc_url(Doc::Builtin("SetLocation")).as_deref(),
        Some("https://wirescript.brickadia.dev/docs/builtins.html#setlocation"),
    );
    assert!(doc_url(Doc::Annotation("layout")).is_some());
    assert!(doc_url(Doc::Diagnostic("WS-EMIT")).is_none());
}

/// The docs cover nearly every builtin (a heading, a table row or a
/// signature block in builtins.md); a drop means the generator stopped
/// finding them, or new builtins went undocumented.
#[test]
fn builtins_link_to_their_docs() {
    let mut missing: Vec<&str> = calls()
        .keys()
        .copied()
        .filter(|n| doc_url(Doc::Builtin(n)).is_none())
        .collect();
    missing.sort_unstable();
    assert!(
        missing.len() * 20 <= calls().len(),
        "{} of {} builtins have no docs link: {}",
        missing.len(),
        calls().len(),
        missing.join(", ")
    );
}

#[test]
fn hover_link_is_its_own_line() {
    let h = with_doc_link("```wirescript\nint\n```".into(), Doc::Type("int"));
    assert!(
        h.ends_with("\n\n[Docs](https://wirescript.brickadia.dev/docs/types.html#primitive-types)"),
        "{h}"
    );
    assert_eq!(with_doc_link("x".into(), Doc::Type("nope")), "x");
}

#[test]
fn events_with_their_own_section_link_there() {
    assert!(find_event("Clock").is_some());
    assert!(doc_url(Doc::Builtin("Clock")).is_some());
}
