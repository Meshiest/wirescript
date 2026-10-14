//! The playground's Monarch grammar (`crates/wasm/playground/monarch.js`) is
//! the source the other editor grammars are generated from, so it has to agree
//! with the language: every keyword the lexer reserves and every primitive
//! type highlights, and every builtin and event it highlights exists.

use std::collections::BTreeSet;

/// The strings of the array property `name` in the grammar.
fn list(grammar: &str, name: &str) -> BTreeSet<String> {
    let start = grammar.find(&format!("\n  {name}: [")).unwrap_or_else(|| panic!("no `{name}` list in monarch.js"));
    let body = &grammar[start..];
    let body = &body[body.find('[').unwrap() + 1..body.find("],").unwrap()];
    body.lines()
        .map(|l| l.split("//").next().unwrap())
        .flat_map(|l| l.split(','))
        .map(|w| w.trim().trim_matches('\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn grammar() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../wasm/playground/monarch.js");
    std::fs::read_to_string(path).expect("read monarch.js")
}

#[test]
fn the_grammar_covers_every_keyword_and_primitive_type() {
    let g = grammar();
    let words: BTreeSet<String> =
        ["keywords", "constants", "typeKeywords"].iter().flat_map(|n| list(&g, n)).collect();
    let missing: Vec<&str> = crate::lexer::KEYWORDS.iter().copied().filter(|k| !words.contains(*k)).collect();
    assert!(missing.is_empty(), "keywords monarch.js does not highlight: {missing:?}");
    let types = list(&g, "typeKeywords");
    let missing: Vec<&str> =
        crate::types::resolve::PRIMITIVES.iter().map(|(n, _)| *n).filter(|t| !types.contains(*t)).collect();
    assert!(missing.is_empty(), "primitive types monarch.js does not highlight: {missing:?}");
}

#[test]
fn the_grammar_highlights_only_real_builtins_and_events() {
    let g = grammar();
    let calls = crate::catalog::calls::calls();
    let unknown: Vec<String> = list(&g, "builtins").into_iter().filter(|b| !calls.contains_key(b.as_str())).collect();
    assert!(unknown.is_empty(), "monarch.js builtins that are no call: {unknown:?}");
    let events = crate::catalog::events::events();
    let unknown: Vec<String> = list(&g, "events").into_iter().filter(|e| !events.contains_key(e.as_str())).collect();
    assert!(unknown.is_empty(), "monarch.js events that are no event: {unknown:?}");
    let missing: Vec<&str> = events.keys().copied().filter(|e| !list(&g, "events").contains(*e)).collect();
    assert!(missing.is_empty(), "events monarch.js does not highlight: {missing:?}");
}
