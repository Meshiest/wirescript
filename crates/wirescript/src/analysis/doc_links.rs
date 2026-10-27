//! Links from hovers and diagnostics into the published docs. The table is
//! `data/doc_links.json`, generated from the docs by
//! `scripts/gen_doc_links.mjs` (`just doc-links`); `just doc-check` fails
//! while it is stale.

use crate::collections::HashMap;
use serde::Deserialize;
use std::sync::OnceLock;

/// The published docs. Editors that bundle the docs map links under this
/// prefix to their own copy.
pub const DOCS_URL: &str = "https://wirescript.brickadia.dev/docs/";

#[derive(Default, Deserialize)]
#[serde(default)]
struct DocLinks {
    diagnostics: HashMap<String, String>,
    builtins: HashMap<String, String>,
    environments: HashMap<String, String>,
    keywords: HashMap<String, String>,
    annotations: HashMap<String, String>,
    types: HashMap<String, String>,
    topics: HashMap<String, String>,
}

fn links() -> &'static DocLinks {
    static LINKS: OnceLock<DocLinks> = OnceLock::new();
    LINKS.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/doc_links.json")).unwrap_or_default()
    })
}

/// What a link points at.
#[derive(Clone, Copy, Debug)]
pub enum Doc<'a> {
    /// A diagnostic code (`WS012`).
    Diagnostic(&'a str),
    /// A builtin call or event by its surface name (`SetLocation`, `Clock`).
    Builtin(&'a str),
    /// A keyword (`if`, `on`, `chip`, `mod`, `fn`).
    Keyword(&'a str),
    /// An annotation's name, without the `@`.
    Annotation(&'a str),
    /// An environment group as a program names it (`Sky`, `Ground`).
    Environment(&'a str),
    /// A built-in type name (`int`, `zone`).
    Type(&'a str),
    /// A page section by the generator's topic name (`arrayMethods`).
    Topic(&'a str),
}

/// The docs URL for `doc`, if the docs cover it.
pub fn doc_url(doc: Doc<'_>) -> Option<String> {
    let l = links();
    let (map, key) = match doc {
        Doc::Diagnostic(k) => (&l.diagnostics, k),
        Doc::Builtin(k) => (&l.builtins, k),
        Doc::Environment(k) => (&l.environments, k),
        Doc::Keyword(k) => (&l.keywords, k),
        Doc::Annotation(k) => (&l.annotations, k),
        Doc::Type(k) => (&l.types, k),
        Doc::Topic(k) => (&l.topics, k),
    };
    map.get(key).map(|path| format!("{DOCS_URL}{path}"))
}

/// `hover` with a link to `doc` on a line of its own, when the docs cover it.
pub(crate) fn with_doc_link(hover: String, doc: Doc<'_>) -> String {
    match doc_url(doc) {
        Some(url) => format!("{hover}\n\n[Docs]({url})"),
        None => hover,
    }
}

#[cfg(test)]
mod tests;
