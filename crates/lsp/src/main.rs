use std::collections::HashMap;
use std::sync::Mutex;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use wirescript::analysis::{
    asset_ref_at, collect_estimates, collect_inlay_hints, collect_symbols_for_file, definition_at,
    collection_kind, cursor_byte_offset, field_name_at, fill_match_arms_at, fill_record_at, find_asset_refs, find_enclosing_call, find_name_range,
    format_wirescript, hover_at, member_receiver_at, named_arg_value, param_names,
    prepare_rename_at, receiver_methods, record_field_names, references_at, references_to_export,
    rename_edit_text, resolve_symbol, semantic_tokens, swizzle_fields, type_str,
    user_receiver_methods, word_at, AssetRef, CollectionKind, CrossFile, InlayHintKind, RefNs,
    RefSite, RefTarget, ResourceEstimate, SemTokenKind, SymbolDef, TextRange, TypeMap,
    VarReadContextMap, byte_to_char_col, char_col_to_byte, char_col_to_utf16_col, line_text,
    utf16_col_to_char_col,
};
use wirescript::ast::{ImportKind, LetBinding, Script, TopDecl};
use wirescript::catalog::arrays::ARRAY_METHODS;
use wirescript::catalog::maps::MAP_METHODS;
use wirescript::catalog::calls::calls;
use wirescript::catalog::events::events;
use wirescript::lexer::KEYWORDS;
use wirescript::resolve::{resolve, resolve_parsed, FileLoader, FsLoader};
use wirescript::typecheck::typecheck_with_inference;
use wirescript::FoldMode;

struct CompileProgressNotification;
impl tower_lsp::lsp_types::notification::Notification for CompileProgressNotification {
    type Params = serde_json::Value;
    const METHOD: &'static str = "wirescript/compileProgress";
}

// Three column conventions meet in this file. The compiler's `Pos::col` is a 1-based BYTE
// column; `analysis::` takes and returns 0-based CHAR columns; the LSP protocol
// carries 0-based UTF-16 code units (this server declares `utf-16` in
// `initialize`, so that is not negotiable per client). They agree only while a
// line is pure ASCII, and slicing a line with the wrong one lands mid-character
// and panics. Convert at this boundary and nowhere else.

/// Editor position -> the 0-based char column `analysis::` expects.
fn lsp_col_to_char(source: &str, line: usize, character: u32) -> usize {
    utf16_col_to_char_col(line_text(source, line), character as usize)
}

/// A 0-based char column from `analysis::` -> the editor's UTF-16 column.
///
/// A column past the end of the line it names is left alone rather than
/// clamped: the two conventions agree for everything below U+10000, so passing
/// it through is right whenever the line could not be read (a closed file that
/// has since changed on disk), while clamping would collapse it onto the line's
/// end.
fn char_col_to_lsp(source: &str, line: usize, col: usize) -> u32 {
    let l = line_text(source, line);
    if l.chars().count() < col {
        return col as u32;
    }
    char_col_to_utf16_col(l, col) as u32
}

/// A byte offset within a line (what the raw-text scanners in this file, and
/// the lexer's `Pos::col`, produce) -> the editor's UTF-16 column.
fn byte_off_to_lsp(source: &str, line: usize, byte: usize) -> u32 {
    let l = line_text(source, line);
    if l.len() < byte {
        return byte as u32;
    }
    char_col_to_utf16_col(l, byte_to_char_col(l, byte)) as u32
}

/// Source text for `uri`, preferring the open document and falling back to
/// disk. Needed only to convert one result's columns for the editor.
fn source_for<'a>(docs: &'a HashMap<Url, DocState>, uri: &Url) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    match docs.get(uri) {
        Some(d) => Cow::Borrowed(d.source.as_str()),
        None => uri
            .to_file_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map_or(Cow::Borrowed(""), Cow::Owned),
    }
}

fn pos_to_lsp(source: &str, p: wirescript::diagnostic::Pos) -> Position {
    let line = p.line.saturating_sub(1);
    Position {
        line: line as u32,
        // `Pos::col` is a 1-based byte column.
        character: byte_off_to_lsp(source, line as usize, p.col.saturating_sub(1) as usize),
    }
}

fn range_to_lsp(source: &str, r: &wirescript::diagnostic::SourceRange) -> Range {
    Range {
        start: pos_to_lsp(source, r.start),
        end: pos_to_lsp(source, r.end),
    }
}

/// A [`TextRange`] carries 0-based BYTE columns, `ref_site_to_text_range`
/// builds it straight from a `SourceRange`, not the char columns the rest of
/// `analysis::` returns.
fn text_range_to_lsp(source: &str, r: &TextRange) -> Range {
    Range {
        start: Position {
            line: r.start_line as u32,
            character: byte_off_to_lsp(source, r.start_line, r.start_col),
        },
        end: Position {
            line: r.end_line as u32,
            character: byte_off_to_lsp(source, r.end_line, r.end_col),
        },
    }
}

/// One [`RefSite`] converted to the LSP-facing [`TextRange`] the edit path
/// uses: a coarse site (a whole-declaration/whole-statement span) is first
/// narrowed to the precise name token via `find_name_range` against ITS OWN
/// file's `source` — the only place source text enters this module, and a
/// bounded narrowing of one already-resolved site, never a textual search
/// (see the plan's Global Constraints). A precise site converts directly.
fn ref_site_to_text_range(source: &str, name: &str, site: &RefSite) -> TextRange {
    let range = if site.coarse {
        find_name_range(source, &site.range, name).unwrap_or_else(|| site.range.clone())
    } else {
        site.range.clone()
    };
    TextRange {
        start_line: range.start.line.saturating_sub(1) as usize,
        start_col: range.start.col.saturating_sub(1) as usize,
        end_line: range.end.line.saturating_sub(1) as usize,
        end_col: range.end.col.saturating_sub(1) as usize,
        is_shorthand: site.is_shorthand,
    }
}

/// The same field-name / keyword refusal `prepare_rename_at` applies before
/// it ever calls `references_at`, reused here so `references`/`rename` don't
/// fall through to `references_at`'s own (coarser) dispatch and return the
/// enclosing declaration's whole reference set for a cursor that's actually
/// on a record FIELD name or a lexer keyword — mirrors the `field_name_at`
/// guard `goto_definition` already applies for the same reason.
fn is_field_or_keyword(ast: &Script, source: &str, line: usize, col: usize) -> bool {
    if let Some(word) = word_at(source, line, col) {
        if KEYWORDS.contains(&word.as_str()) {
            return true;
        }
    }
    field_name_at(ast, source, line, col)
}

/// Deterministic order + belt-and-braces dedup: rename must never hand the
/// client two edits for the same site.
fn sort_and_dedup(mut results: Vec<(Url, TextRange)>) -> Vec<(Url, TextRange)> {
    results.sort_by(|a, b| {
        (a.0.as_str(), a.1.start_line, a.1.start_col, a.1.end_line, a.1.end_col).cmp(&(
            b.0.as_str(),
            b.1.start_line,
            b.1.start_col,
            b.1.end_line,
            b.1.end_col,
        ))
    });
    results.dedup();
    results
}

/// The top-level declaration in `ast` binding `name` in namespace `ns`, if
/// any — mirrors `analysis::definition::top_decl_name`'s construct coverage
/// (chip/mod, fn, `let <ident>`, event) plus `type` (importable per the
/// plan's Global Constraints, but not a target of that private helper since
/// `definition.rs` never jumps to a type-position use). Returns the whole
/// declaration's own range; callers narrow it to the name token via
/// `find_name_range`, exactly like `find_import_definition` does for
/// goto-definition.
fn top_level_decl_range(
    ast: &Script,
    name: &str,
    ns: RefNs,
) -> Option<wirescript::diagnostic::SourceRange> {
    for d in &ast.decls {
        match d {
            TopDecl::Chip(c) if ns == RefNs::Value && c.name == name => return Some(c.range.clone()),
            TopDecl::Fn(f) if ns == RefNs::Value && f.name == name => return Some(f.range.clone()),
            TopDecl::Let(l) if ns == RefNs::Value => {
                if let LetBinding::Ident { name: n, .. } = &l.binding {
                    if n == name {
                        return Some(l.range.clone());
                    }
                }
            }
            TopDecl::Event(e) if ns == RefNs::Value && e.name == name => return Some(e.range.clone()),
            TopDecl::TypeAlias(t) if ns == RefNs::Type && t.name == name => {
                return Some(t.range.clone());
            }
            _ => {}
        }
    }
    None
}

/// For an `Imported` target: find the `import { … }` specifier in the
/// CURRENT file that brought in `export_name`, resolve + parse the source
/// file it points at (mirrors `definition.rs::find_import_definition`'s path
/// resolution), then run `references_at` on the ORIGINAL declaration's own
/// name span there — giving the defining file's URI, source, and its own
/// decl + local-use sites. `None` when the current doc isn't open, no
/// matching import specifier exists, or the target file/decl can't be
/// resolved (e.g. deleted from disk).
fn find_defining_file_sites(
    docs: &HashMap<Url, DocState>,
    uri: &Url,
    export_name: &str,
    ns: RefNs,
) -> Option<(Url, String, Vec<RefSite>)> {
    let doc = docs.get(uri)?;
    let current_file = uri_to_file_string(uri);

    let import_path = doc.pre_resolve_ast.decls.iter().find_map(|d| {
        let TopDecl::Import(imp) = d else { return None };
        let ImportKind::Named(bindings) = &imp.kind else { return None };
        bindings.iter().any(|b| b.name == export_name).then(|| imp.path.clone())
    })?;

    let resolved_path = FsLoader.canonical_path(&import_path, &current_file);
    let d_file = if resolved_path.ends_with(".ws") {
        resolved_path
    } else {
        format!("{import_path}.ws")
    };

    // Prefer an already-open buffer for `D` over its on-disk content — it may
    // hold unsaved edits, and reusing its own `Url` (rather than a freshly
    // built `Url::from_file_path`) avoids tagging edits with a differently-
    // spelled URI for a file the client already has open under another
    // spelling (the same class of bug the respelled-URI dedup elsewhere in
    // this file guards against).
    let d_canonical = std::path::Path::new(&d_file)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(&d_file));
    if let Some((open_uri, open_doc)) = docs.iter().find(|(u, _)| {
        u.to_file_path().ok().map(|p| std::fs::canonicalize(&p).unwrap_or(p)).as_ref()
            == Some(&d_canonical)
    }) {
        let decl_range = top_level_decl_range(&open_doc.pre_resolve_ast, export_name, ns)?;
        let name_range =
            find_name_range(&open_doc.source, &decl_range, export_name).unwrap_or(decl_range);
        let line = name_range.start.line.saturating_sub(1) as usize;
        let col = name_range.start.col.saturating_sub(1) as usize;
        let open_file = uri_to_file_string(open_uri);
        let (_target, sites) =
            references_at(&open_doc.pre_resolve_ast, &open_doc.source, &open_file, line, col)?;
        return Some((open_uri.clone(), open_doc.source.clone(), sites));
    }

    let d_source = FsLoader.load(&import_path, &current_file).ok()?;
    let d_ast = wirescript::on_big_stack(|| wirescript::parse(&d_source, &d_file)).ast;

    let decl_range = top_level_decl_range(&d_ast, export_name, ns)?;
    let name_range = find_name_range(&d_source, &decl_range, export_name).unwrap_or(decl_range);
    let line = name_range.start.line.saturating_sub(1) as usize;
    let col = name_range.start.col.saturating_sub(1) as usize;

    let (_target, sites) = references_at(&d_ast, &d_source, &d_file, line, col)?;
    let d_uri = Url::from_file_path(&d_file).ok()?;
    Some((d_uri, d_source, sites))
}

/// Cross-file site collection for `references`/`rename`, driven entirely by
/// the AST-based resolver (`references_at`/`references_to_export`) — never a
/// textual scan. `target`/`current_sites` come from an initial
/// `references_at` call at the LSP cursor; `current_sites` already IS the
/// current file's own decl + local-use set.
///
/// - `Local`: only `current_sites`, tagged to `uri` — no cross-file scan runs
///   at all (locals never cross files per the plan's Global Constraints).
/// - `Exported`: the current file already IS the defining file `D`, so
///   `current_sites` doubles as `D`'s sites; every other `.ws` file (open
///   docs, regardless of directory, plus a same-directory disk scan for
///   closed ones — mirroring the pre-resolver behavior) is scanned via
///   `references_to_export` for import-specifier + import-bound uses.
/// - `Imported`: `find_defining_file_sites` resolves the real defining file
///   `D` (which may not be `uri`, and may not even be open) and its own
///   sites; the same sibling scan then runs over every other file, which
///   naturally includes the current (importer) file recomputing to the same
///   `current_sites` — cheap and correct, since `D` itself never matches
///   `references_to_export` (its binding has no `import_export` tag).
fn collect_references_across_files(
    docs: &HashMap<Url, DocState>,
    uri: &Url,
    target: &RefTarget,
    current_sites: &[RefSite],
) -> Vec<(Url, TextRange)> {
    let current_source = docs.get(uri).map(|d| d.source.as_str()).unwrap_or("");

    let export_name = match &target.cross_file {
        CrossFile::Local => {
            let results: Vec<(Url, TextRange)> = current_sites
                .iter()
                .map(|s| (uri.clone(), ref_site_to_text_range(current_source, &target.name, s)))
                .collect();
            return sort_and_dedup(results);
        }
        CrossFile::Exported { export_name } | CrossFile::Imported { export_name } => {
            export_name.clone()
        }
    };

    // The defining file `D`'s own decl + local-use sites.
    let defining = match &target.cross_file {
        CrossFile::Exported { .. } => {
            Some((uri.clone(), current_source.to_string(), current_sites.to_vec()))
        }
        CrossFile::Imported { .. } => find_defining_file_sites(docs, uri, &export_name, target.ns),
        CrossFile::Local => unreachable!("handled above"),
    };
    let Some((d_uri, d_source, d_sites)) = defining else {
        // Couldn't resolve the defining file (e.g. the imported file is
        // missing on disk) — degrade to this file's own sites rather than
        // returning nothing.
        let results: Vec<(Url, TextRange)> = current_sites
            .iter()
            .map(|s| (uri.clone(), ref_site_to_text_range(current_source, &target.name, s)))
            .collect();
        return sort_and_dedup(results);
    };

    // CRITICAL: narrow every cross-file coarse site against `export_name`, not
    // `target.name`. `D`'s text (and a sibling's specifier) spells the ORIGINAL
    // export name; narrowing a coarse `mod helper(){…}` decl against the
    // cursor's LOCAL name (e.g. an alias `assist`) would fail `find_name_range`
    // and fall back to the whole-decl range — replacing the entire declaration.
    // After the aliased-import-is-`Local` classification, `target.name` already
    // equals `export_name` on every path that reaches here, but binding to
    // `export_name` makes that invariant explicit and corruption-proof.
    let mut results: Vec<(Url, TextRange)> = d_sites
        .iter()
        .map(|s| (d_uri.clone(), ref_site_to_text_range(&d_source, &export_name, s)))
        .collect();

    let d_canonical = d_uri.to_file_path().ok().map(|p| std::fs::canonicalize(&p).unwrap_or(p));

    // Every OPEN document that isn't `D` itself: scan for import-specifier +
    // import-bound uses of `export_name`. (`D` naturally yields nothing here
    // even when left in, since its own binding carries no `import_export`
    // tag — the explicit skip is just to avoid the redundant parse.)
    for (doc_uri, doc_state) in docs.iter() {
        if let Some(dc) = &d_canonical {
            let doc_canon = doc_uri.to_file_path().ok().map(|p| std::fs::canonicalize(&p).unwrap_or(p));
            if doc_canon.as_ref() == Some(dc) {
                continue;
            }
        }
        let file = uri_to_file_string(doc_uri);
        let ast = wirescript::on_big_stack(|| wirescript::parse(&doc_state.source, &file)).ast;
        for s in references_to_export(&ast, &file, &export_name, target.ns) {
            results.push((doc_uri.clone(), ref_site_to_text_range(&doc_state.source, &export_name, &s)));
        }
    }

    // Canonical filesystem paths of the open docs, so the same-directory disk
    // scan below can skip them. Url equality can NOT decide "already open":
    // the client's URI spelling (e.g. `file:///c%3A/…`) differs from
    // `Url::from_file_path`'s (`file:///C:/…`), so a Url-keyed skip re-added
    // every open doc from disk. References then showed each site twice, and
    // rename emitted two identical TextEdits per site — overlapping edits the
    // editor refuses to apply, silently leaving those files un-renamed.
    let open_paths: std::collections::HashSet<std::path::PathBuf> = docs
        .keys()
        .filter_map(|u| u.to_file_path().ok())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();

    if let Ok(file_path) = uri.to_file_path() {
        if let Some(dir) = file_path.parent() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if !path.extension().map_or(false, |e| e == "ws") {
                    continue;
                }
                let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if open_paths.contains(&canonical) {
                    continue;
                }
                if d_canonical.as_ref() == Some(&canonical) {
                    continue;
                }
                let entry_uri = match Url::from_file_path(&path) {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                let src = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let file = path.to_string_lossy().to_string();
                let ast = wirescript::on_big_stack(|| wirescript::parse(&src, &file)).ast;
                for s in references_to_export(&ast, &file, &export_name, target.ns) {
                    results.push((entry_uri.clone(), ref_site_to_text_range(&src, &export_name, &s)));
                }
            }
        }
    }

    sort_and_dedup(results)
}

/// Find-references for an atom `:name`: every `:name` occurrence in every open
/// document plus every `.ws` file in the referencing file's directory. Atoms
/// are global (a name hashes to one xxHash64 value, with no scope), so this is
/// a plain name match across the workspace — no resolution needed.
fn collect_atom_references(docs: &HashMap<Url, DocState>, uri: &Url, name: &str) -> Vec<Location> {
    let mut out: Vec<Location> = Vec::new();
    for (doc_uri, doc_state) in docs.iter() {
        let file = uri_to_file_string(doc_uri);
        for r in wirescript::analysis::atom_references(&doc_state.source, &file, name) {
            out.push(Location { uri: doc_uri.clone(), range: range_to_lsp(&doc_state.source, &r) });
        }
    }
    // Same-directory disk scan, skipping already-open docs (canonical-path
    // keyed, matching `collect_references_across_files`).
    let open_paths: std::collections::HashSet<std::path::PathBuf> = docs
        .keys()
        .filter_map(|u| u.to_file_path().ok())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    if let Ok(file_path) = uri.to_file_path() {
        if let Some(dir) = file_path.parent() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if !path.extension().map_or(false, |e| e == "ws") {
                    continue;
                }
                let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if open_paths.contains(&canonical) {
                    continue;
                }
                let Ok(entry_uri) = Url::from_file_path(&path) else {
                    continue;
                };
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let file = path.to_string_lossy().to_string();
                for r in wirescript::analysis::atom_references(&src, &file, name) {
                    out.push(Location { uri: entry_uri.clone(), range: range_to_lsp(&src, &r) });
                }
            }
        }
    }
    out
}

fn uri_to_file_string(uri: &Url) -> String {
    uri.to_file_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| uri.path().to_string())
}

/// Candidate prefab-reference strings for `$./…` completion: every `.brz`
/// archive and `.ws` source file under the document's directory, as
/// `./relative/path.ext` (forward slashes, the wirescript reference form).
/// Bounded depth so large trees don't stall completion.
fn scan_prefab_paths(uri: &Url) -> Vec<String> {
    let Ok(file_path) = uri.to_file_path() else {
        return Vec::new();
    };
    let Some(base) = file_path.parent() else {
        return Vec::new();
    };
    fn walk(dir: &std::path::Path, base: &std::path::Path, depth: usize, out: &mut Vec<String>) {
        if depth > 6 || out.len() > 500 {
            return;
        }
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, depth + 1, out);
            } else if path.extension().is_some_and(|e| e == "brz" || e == "ws") {
                if let Ok(rel) = path.strip_prefix(base) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    out.push(format!("./{rel}"));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(base, base, 0, &mut out);
    out.sort();
    out
}

/// Resolve a prefab file reference path (the part after `$`) to a filesystem
/// path, the same way `disk_prefab_resolver` does: `./rel` and bare `rel`
/// resolve against the referencing file's directory; a leading `/` is absolute.
fn resolve_prefab_path(entry_file: &str, path: &str) -> std::path::PathBuf {
    use std::path::{Path, PathBuf};
    let base = Path::new(entry_file).parent();
    if let Some(rel) = path.strip_prefix("./") {
        base.map_or_else(|| PathBuf::from(rel), |b| b.join(rel))
    } else if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        base.map_or_else(|| PathBuf::from(path), |b| b.join(path))
    }
}

/// LSP diagnostics for prefab file references that don't resolve: a missing
/// file on disk, or a ref without the required `.brz`/`.ws` extension.
fn prefab_ref_diagnostics(source: &str, file: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for r in find_asset_refs(source).into_iter().filter(AssetRef::is_file) {
        let range = Range {
            start: Position { line: r.line as u32, character: char_col_to_lsp(source, r.line, r.start_col) },
            end: Position { line: r.line as u32, character: char_col_to_lsp(source, r.line, r.end_col) },
        };
        if !r.path.ends_with(".brz") && !r.path.ends_with(".ws") {
            out.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("prefab-ext".into())),
                source: Some("wirescript".into()),
                message: format!(
                    "prefab reference `${}` must end in `.brz` (a prebuilt archive) or `.ws` (a source file)",
                    r.path
                ),
                ..Default::default()
            });
            continue;
        }
        let resolved = resolve_prefab_path(file, &r.path);
        if !resolved.is_file() {
            out.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("prefab-missing".into())),
                source: Some("wirescript".into()),
                message: format!("prefab file not found: {}", resolved.display()),
                ..Default::default()
            });
        }
    }
    out
}

struct DocState {
    source: String,
    symbols: Vec<SymbolDef>,
    doc_comments: wirescript::parser::DocComments,
    type_map: TypeMap,
    if_contexts: wirescript::analysis::IfContextMap,
    var_read_contexts: VarReadContextMap,
    /// Ranges of `if`/`else` blocks a const-evaluable condition dropped before
    /// type-checking (never verified), with a human-readable reason for each —
    /// surfaced by `hover_dropped_range` so hovering inside one says so.
    dropped_ranges: Vec<(wirescript::diagnostic::SourceRange, String)>,
    resource_estimates: wirescript::collections::HashMap<String, ResourceEstimate>,
    pre_resolve_ast: Script,
    /// Canonical paths this doc imports, transitively — used to decide whether
    /// a change to another file can affect it.
    imported_files: Vec<String>,
}

/// One analysis' diagnostics, split by the file each actually names.
///
/// `resolve` inlines every import into the entry file's AST, so analysing
/// `main.ws` routinely raises diagnostics whose range names `util.ws`. A
/// publish is per-URI, so those have to be sent to the file they came from.
/// Keeping only the entry's share strands the rest in no buffer at all, and
/// the editor shows a clean file for a program the compiler rejects.
#[derive(Default)]
struct SplitDiagnostics {
    /// The analysed document's own share, plus range-less diagnostics.
    own: Vec<Diagnostic>,
    /// An imported file's share, keyed by that file's URI.
    foreign: HashMap<Url, Vec<Diagnostic>>,
}

impl SplitDiagnostics {
    /// Append `other`, skipping anything already reported for the same file.
    /// `compile` re-runs parse and typecheck, so the on-save lowering set
    /// overlaps the live one.
    fn extend_deduped(&mut self, other: SplitDiagnostics) {
        fn push_new(into: &mut Vec<Diagnostic>, from: Vec<Diagnostic>) {
            for d in from {
                if !into.iter().any(|e| e.range == d.range && e.message == d.message) {
                    into.push(d);
                }
            }
        }
        push_new(&mut self.own, other.own);
        for (uri, diags) in other.foreign {
            push_new(self.foreign.entry(uri).or_default(), diags);
        }
    }
}

fn to_lsp_diagnostic(source: &str, d: &wirescript::diagnostic::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: range_to_lsp(source, &d.range),
        severity: Some(match d.severity {
            wirescript::diagnostic::Severity::Error => DiagnosticSeverity::ERROR,
            wirescript::diagnostic::Severity::Warning => DiagnosticSeverity::WARNING,
            _ => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(d.code.clone())),
        source: Some("wirescript".into()),
        message: d.message.clone(),
        ..Default::default()
    }
}

/// Split compiler diagnostics by the file each names, converting every range
/// against the text of THAT file. `Pos::col` is a byte column into the file
/// the range names, so the entry document's text is the wrong ruler for an
/// imported one.
///
/// A diagnostic for a file that is currently `open` is dropped: that document
/// analyses itself and publishes its own set, and a second copy from its
/// importer would double every marker.
fn split_by_file<'a>(
    diags: impl Iterator<Item = &'a wirescript::diagnostic::Diagnostic>,
    file: &str,
    source: &str,
    open: &HashMap<Url, DocState>,
) -> SplitDiagnostics {
    // Clients respell file URIs (VS Code sends `file:///c%3A/...`), so an open
    // document is recognized by its canonical PATH and not by `Url` equality,
    // the same reason `collect_references_across_files` dedups that way.
    let open_paths: std::collections::HashSet<String> = open
        .keys()
        .map(|u| FsLoader.canonical_path(&uri_to_file_string(u), "."))
        .collect();
    let mut split = SplitDiagnostics::default();
    let mut foreign_src: HashMap<Url, Option<String>> = HashMap::new();
    for d in diags {
        if &*d.range.file == file || d.range.file.is_empty() {
            split.own.push(to_lsp_diagnostic(source, d));
            continue;
        }
        let Ok(other) = Url::from_file_path(&*d.range.file) else {
            continue;
        };
        if open_paths.contains(&FsLoader.canonical_path(&d.range.file, ".")) {
            continue;
        }
        let text = foreign_src
            .entry(other.clone())
            .or_insert_with(|| std::fs::read_to_string(&*d.range.file).ok());
        let Some(text) = text.as_deref() else { continue };
        split
            .foreign
            .entry(other)
            .or_default()
            .push(to_lsp_diagnostic(text, d));
    }
    split
}

struct Backend {
    client: Client,
    docs: Mutex<HashMap<Url, DocState>>,
    /// Which imported files each open document has published diagnostics to,
    /// so they can be cleared when that document stops reporting them (or is
    /// closed). Diagnostics belong to the server until it says otherwise, and
    /// nothing else would ever retract a marker in a file nobody has open.
    foreign_diags: Mutex<HashMap<Url, std::collections::HashSet<Url>>>,
    /// Whether the client accepts a dynamic `workspace/didChangeWatchedFiles`
    /// registration, read from its initialize capabilities. Set once in
    /// `initialize`, read once in `initialized`.
    watch_files: std::sync::atomic::AtomicBool,
    /// Bumped by every `did_change`. A debounced handler analyses only if it
    /// still holds the newest value when its wait ends, see [`DEBOUNCE`].
    change_gen: std::sync::atomic::AtomicU64,
}

/// How long `did_change` waits before analysing.
///
/// One analysis is tens of milliseconds of synchronous front end on a large
/// program, and typing outruns it, so without a window every intermediate
/// buffer state is analysed in full and thrown away while hover and completion
/// queue behind it. Long enough to swallow a burst of keystrokes, short enough
/// that a pause reads as instant.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// A loader that serves imports from the OPEN EDITOR BUFFERS first, falling back
/// to disk, so an unsaved edit in an imported file is visible to the files that
/// import it instead of their diagnostics describing the last saved version. Also
/// skips a disk read per import per keystroke for files already in memory.
///
/// Holds a snapshot (canonical path -> source) taken before `resolve` rather
/// than the live `docs` mutex, since `analyze` re-locks that mutex afterwards.
struct OpenDocLoader {
    open: HashMap<String, String>,
}

impl FileLoader for OpenDocLoader {
    // `Result` in this file is tower-lsp's single-parameter alias, so spell the
    // std one out.
    fn load(&self, path: &str, relative_to: &str) -> std::result::Result<String, String> {
        let canon = self.canonical_path(path, relative_to);
        if let Some(src) = self.open.get(&canon) {
            return Ok(src.clone());
        }
        FsLoader.load(path, relative_to)
    }

    fn canonical_path(&self, path: &str, relative_to: &str) -> String {
        FsLoader.canonical_path(path, relative_to)
    }
}

impl Backend {
    /// Typecheck-only analysis for one document, publishing its diagnostics.
    ///
    /// Resource estimates (the gate counts hover shows) are recomputed here on
    /// every analysis, including per keystroke, rather than cached across
    /// `did_change` calls: `collect_estimates` lowers every chip/handler body,
    /// but [`lookup_estimate`] keys by NAME, so a carried-forward map would have
    /// no entry for a mod added or renamed since the last save — it would render
    /// with no gate count while its neighbours had one, reading as the estimate
    /// being broken rather than stale.
    ///
    /// Recomputing costs ~11% of one analysis (measured on the largest program
    /// to hand: ~4ms of a ~39ms analyze, and well under 1ms on smaller files),
    /// which the template cache keeps flat. That is worth paying for a hover
    /// that matches the buffer.
    fn analyze(&self, uri: &Url, source: &str) -> SplitDiagnostics {
        let file = uri_to_file_string(uri);

        // Parse ONCE and hand the result to resolve, avoiding a second parse of
        // the same buffer per keystroke. The pre-resolve AST (kept for local
        // analysis: references, semantic tokens, rename) is cloned off before
        // resolve consumes the parse.
        // On the big stack: this runs on a ~2 MiB tokio worker, and an
        // overflow there aborts the process, so the server would vanish on
        // `didOpen`. The spawn is ~100 us against tens of ms of work.
        let pre_resolve = wirescript::on_big_stack(|| wirescript::parse(source, &file));
        let pre_resolve_ast = pre_resolve.ast.clone();
        // Snapshot the other open buffers so imports resolve against unsaved
        // edits (and skip a disk read); the lock is released before resolve.
        let loader = OpenDocLoader {
            open: self
                .docs
                .lock()
                .map(|docs| {
                    docs.iter()
                        .filter(|(u, _)| *u != uri)
                        .map(|(u, d)| {
                            (
                                FsLoader.canonical_path(&uri_to_file_string(u), "."),
                                d.source.clone(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        let (resolved, tc, symbols, resource_estimates) = wirescript::on_big_stack(|| {
            let resolved = resolve_parsed(pre_resolve, &file, &loader);
            let tc = typecheck_with_inference(&resolved.ast, &file).0;
            let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some(&file));
            let resource_estimates = collect_estimates(&resolved.ast, &tc, &file);
            (resolved, tc, symbols, resource_estimates)
        });

        if let Ok(mut docs) = self.docs.lock() {
            docs.insert(
                uri.clone(),
                DocState {
                    source: source.to_string(),
                    symbols,
                    doc_comments: resolved.doc_comments,
                    type_map: tc.type_of_expr,
                    if_contexts: tc.if_contexts,
                    var_read_contexts: tc.var_read_contexts,
                    dropped_ranges: tc.dropped_ranges,
                    resource_estimates,
                    pre_resolve_ast,
                    imported_files: resolved.imported_files.clone(),
                },
            );
        }

        let mut split = match self.docs.lock() {
            Ok(docs) => split_by_file(
                resolved.diagnostics.iter().chain(tc.diagnostics.iter()),
                &file,
                source,
                &docs,
            ),
            Err(_) => SplitDiagnostics::default(),
        };
        split.own.extend(prefab_ref_diagnostics(source, &file));
        split
    }

    /// Lowering and emit diagnostics for one document.
    ///
    /// `analyze()` stops at typecheck — lowering on every keystroke is the
    /// blowup this server is built to avoid — so a whole class of problem
    /// (a destructured field that binds nothing, a wire to a port the gate
    /// does not have) never reached the editor at all. Running the full
    /// pipeline on save is cheap enough and catches those where the author
    /// will see them, instead of at the next explicit Compile.
    ///
    /// Runs on a blocking task: `compile` reserves its own big stack, and the
    /// server must stay responsive while it works.
    async fn lowering_diagnostics(&self, uri: &Url, source: &str) -> SplitDiagnostics {
        let file = uri_to_file_string(uri);
        let src_owned = source.to_string();
        let file_owned = file.clone();
        let result = tokio::task::spawn_blocking(move || {
            wirescript::compile(wirescript::CompileInput {
                source: &src_owned,
                file: &file_owned,
                module_name: None,
                fold_mode: FoldMode::Auto,
            })
        })
        .await;

        // A panic in the compile must not take diagnostics (or the server) down.
        let result = match result {
            Ok(r) => r,
            Err(_) => return SplitDiagnostics::default(),
        };

        let (diags, emit_error) = match result {
            Ok(r) => (r.diagnostics, None),
            // `compile` stops at the first stage that reports an error, so a
            // file with a type error never reaches lowering and its WSP001
            // "no lowering for this expression" warnings are never produced.
            // `wirescript-check` runs the same front end past typecheck for
            // exactly that reason (`bin/check.rs`), and the two disagreed on
            // every file with both. Re-run for the full set; this second pass
            // only happens on a save of a file that already has errors.
            Err(wirescript::CompileError::HasErrors(_)) => {
                let src_owned = source.to_string();
                let file_owned = file.clone();
                let full = tokio::task::spawn_blocking(move || {
                    wirescript::diagnostics_only(wirescript::CompileInput {
                        source: &src_owned,
                        file: &file_owned,
                        module_name: None,
                        fold_mode: FoldMode::Auto,
                    })
                })
                .await;
                (full.unwrap_or_default(), None)
            }
            Err(wirescript::CompileError::Emit(e)) => (Vec::new(), Some(format!("{e:?}"))),
        };

        let mut out = match self.docs.lock() {
            Ok(docs) => split_by_file(diags.iter(), &file, source, &docs),
            Err(_) => SplitDiagnostics::default(),
        };

        // Emit failures carry no source range (they name a wire and a brick, not
        // a line). Surface one at the top of the file rather than dropping it —
        // it is the difference between a build that fails and a build that fails
        // for no visible reason.
        if let Some(msg) = emit_error {
            out.own.push(Diagnostic {
                range: tower_lsp::lsp_types::Range::default(),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String("WS-EMIT".into())),
                source: Some("wirescript".into()),
                message: format!("emit failed: {msg}"),
                ..Default::default()
            });
        }
        out
    }

    /// Re-analyze the other open documents that the changed file can actually
    /// affect — i.e. those importing it (transitively) — rather than every open
    /// document, so a keystroke in one file doesn't cost (open tabs + 1) full
    /// analyses for every other open file.
    async fn reanalyze_other_docs(&self, changed_uri: &Url) {
        let changed = FsLoader.canonical_path(&uri_to_file_string(changed_uri), ".");
        let others: Vec<(Url, String)> = {
            let docs = match self.docs.lock() {
                Ok(d) => d,
                Err(_) => return,
            };
            docs.iter()
                .filter(|(uri, _)| *uri != changed_uri)
                .filter(|(_, doc)| doc.imported_files.iter().any(|p| p == &changed))
                .map(|(uri, doc)| (uri.clone(), doc.source.clone()))
                .collect()
        };
        for (uri, source) in others {
            let split = self.analyze(&uri, &source);
            self.publish_split(&uri, split).await;
        }
    }

    /// Publish one analysis: the document's own diagnostics to `uri`, and each
    /// imported file's to that file. A file this document published to last
    /// time and no longer does is cleared, so a fixed error in an imported
    /// file doesn't leave a marker in a buffer nobody has open.
    async fn publish_split(&self, uri: &Url, split: SplitDiagnostics) {
        let SplitDiagnostics { own, foreign } = split;
        let stale: Vec<Url> = match self.foreign_diags.lock() {
            Ok(mut owned) => {
                let previous = owned
                    .insert(uri.clone(), foreign.keys().cloned().collect())
                    .unwrap_or_default();
                // The new set is already in `owned`, so this one test covers
                // both "this document still reports it" and "another open
                // document does".
                previous
                    .into_iter()
                    .filter(|u| !owned.values().any(|set| set.contains(u)))
                    .collect()
            }
            Err(_) => Vec::new(),
        };
        for (u, diags) in foreign {
            self.client.publish_diagnostics(u, diags, None).await;
        }
        for u in stale {
            self.client.publish_diagnostics(u, Vec::new(), None).await;
        }
        self.client.publish_diagnostics(uri.clone(), own, None).await;
    }

    /// Drop `uri`'s claim on the imported files it published to, clearing any
    /// no other open document still reports.
    async fn release_foreign_diags(&self, uri: &Url) {
        let stale: Vec<Url> = match self.foreign_diags.lock() {
            Ok(mut owned) => owned
                .remove(uri)
                .unwrap_or_default()
                .into_iter()
                .filter(|u| !owned.values().any(|set| set.contains(u)))
                .collect(),
            Err(_) => Vec::new(),
        };
        for u in stale {
            self.client.publish_diagnostics(u, Vec::new(), None).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // An importer's diagnostics are only refreshed by an edit to a file the
        // editor has OPEN (`reanalyze_other_docs` walks open documents). A
        // module changed or CREATED on disk while closed would leave every file
        // importing it showing diagnostics for the version that is gone - the
        // shape that reads as "the LSP is wrong about my import". Watching the
        // workspace's `.ws` files closes that gap; registered below, in
        // `initialized`, only when the client offers it.
        self.watch_files.store(
            params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|w| w.did_change_watched_files.as_ref())
                .and_then(|f| f.dynamic_registration)
                .unwrap_or(false),
            std::sync::atomic::Ordering::Relaxed,
        );
        // A client that brings its own formatter (the VS Code extension uses
        // its prettier plugin) can opt out of server-side formatting so the
        // editor doesn't list two identical providers.
        let provide_formatting = params
            .initialization_options
            .as_ref()
            .and_then(|o| o.get("provideFormatting"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Declared rather than negotiated: every column this server
                // hands out is converted to UTF-16 at the protocol boundary
                // (see `char_col_to_lsp`).
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..Default::default()
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into(), "$".into(), "/".into()]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                references_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(provide_formatting)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["wirescript.compile".into()],
                    ..Default::default()
                }),
                inlay_hint_provider: Some(OneOf::Left(true)),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: Default::default(),
                }),
                semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
                    SemanticTokensOptions {
                        legend: SemanticTokensLegend {
                            token_types: vec![
                                SemanticTokenType::TYPE,
                                SemanticTokenType::FUNCTION,
                                SemanticTokenType::PARAMETER,
                                SemanticTokenType::VARIABLE,
                                SemanticTokenType::NAMESPACE,
                            ],
                            token_modifiers: vec![],
                        },
                        full: Some(SemanticTokensFullOptions::Bool(true)),
                        range: Some(false),
                        work_done_progress_options: Default::default(),
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let docs = match self.docs.lock() {
            Ok(d) => d,
            Err(_) => return Ok(None),
        };
        let Some(doc) = docs.get(uri) else {
            return Ok(None);
        };
        let file = uri_to_file_string(uri);
        // Clickable links for prefab file references that exist on disk.
        let links: Vec<DocumentLink> = find_asset_refs(&doc.source)
            .into_iter()
            .filter(AssetRef::is_file)
            .filter_map(|r| {
                let target = resolve_prefab_path(&file, &r.path);
                if !target.is_file() {
                    return None;
                }
                let target_uri = Url::from_file_path(&target).ok()?;
                Some(DocumentLink {
                    range: Range {
                        start: Position { line: r.line as u32, character: char_col_to_lsp(&doc.source, r.line, r.start_col) },
                        end: Position { line: r.line as u32, character: char_col_to_lsp(&doc.source, r.line, r.end_col) },
                    },
                    target: Some(target_uri),
                    tooltip: Some("Open prefab file".into()),
                    data: None,
                })
            })
            .collect();
        Ok(Some(links))
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.watch_files.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = self
                .client
                .register_capability(vec![Registration {
                    id: "wirescript-watch-ws".into(),
                    method: "workspace/didChangeWatchedFiles".into(),
                    register_options: serde_json::to_value(
                        DidChangeWatchedFilesRegistrationOptions {
                            watchers: vec![FileSystemWatcher {
                                glob_pattern: GlobPattern::String("**/*.ws".into()),
                                kind: None,
                            }],
                        },
                    )
                    .ok(),
                }])
                .await;
        }
        self.client
            .log_message(MessageType::INFO, "wirescript LSP initialized")
            .await;
    }

    /// A `.ws` file changed on disk. Only files the editor has open are
    /// analyzed, so the changed one may not be among them - what matters is
    /// refreshing the open files that IMPORT it, which is exactly what
    /// `reanalyze_other_docs` does.
    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in &params.changes {
            self.reanalyze_other_docs(&change.uri).await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let split = self.analyze(&params.text_document.uri, &params.text_document.text);
        self.publish_split(&params.text_document.uri, split).await;
        // The file just opened may be one an already-open importer had been
        // publishing diagnostics INTO; that importer must drop them now the
        // file reports for itself, or every marker in it shows twice.
        self.reanalyze_other_docs(&params.text_document.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let Some(change) = params.content_changes.into_iter().next() else {
            return;
        };
        // Coalesce a burst of keystrokes: wait, then analyse only if no newer
        // change arrived meanwhile. A superseded handler returns without
        // touching the front end at all.
        let generation = self
            .change_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        tokio::time::sleep(DEBOUNCE).await;
        if self.change_gen.load(std::sync::atomic::Ordering::SeqCst) != generation {
            return;
        }
        let split = self.analyze(&uri, &change.text);
        self.publish_split(&uri, split).await;
        self.reanalyze_other_docs(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.reanalyze_other_docs(&params.text_document.uri).await;

        let uri = params.text_document.uri.clone();
        let source = match self.docs.lock() {
            Ok(docs) => match docs.get(&uri) {
                Some(doc) => doc.source.clone(),
                None => return,
            },
            Err(_) => return,
        };

        // Republish the typecheck set together with the lowering set, so the
        // save-only diagnostics do not wipe the live ones (a publish replaces
        // everything for the file).
        let mut split = self.analyze(&uri, &source);
        split.extend_deduped(self.lowering_diagnostics(&uri, &source).await);
        self.publish_split(&uri, split).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Ok(mut docs) = self.docs.lock() {
            docs.remove(&uri);
        }
        // Clear what this file was showing. Diagnostics belong to the server
        // until it says otherwise, so dropping the document without publishing
        // an empty set left every marker on screen for the rest of the session.
        self.client
            .publish_diagnostics(uri.clone(), Vec::new(), None)
            .await;
        self.release_foreign_diags(&uri).await;
        // It may itself be imported by a document still open, which stopped
        // reporting for it while it was open; that importer now owns its
        // diagnostics again.
        self.reanalyze_other_docs(&uri).await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let pos = params.text_document_position.position;
        let line = pos.line as usize;
        // `pos.character` is a UTF-16 column; everything below wants a char one.
        let raw_col = pos.character as usize;
        let uri = &params.text_document_position.text_document.uri;

        let prefab_paths = scan_prefab_paths(uri);
        let items = match self.docs.lock() {
            Ok(docs) => match docs.get(uri) {
                Some(doc) => {
                    let col = lsp_col_to_char(&doc.source, line, pos.character);
                    // Inside a `$```…``` ` nested-prefab block, complete against
                    // the INNER program so the outer file's context (SpawnPrefab
                    // params, outer symbols) doesn't leak into the isolated block.
                    if let Some((inner, il, ic)) =
                        nested_block_at(&doc.source, &uri_to_file_string(uri), line, col)
                    {
                        wirescript::on_big_stack(|| {
                            let resolved = resolve(&inner, "nested", &FsLoader);
                            let tc = typecheck_with_inference(&resolved.ast, "nested").0;
                            let syms = collect_symbols_for_file(
                                &resolved.ast,
                                &tc.type_of_expr,
                                Some("nested"),
                            );
                            build_completions(&inner, &syms, il, ic, &[])
                        })
                    } else {
                        build_completions(&doc.source, &doc.symbols, line, col, &prefab_paths)
                    }
                }
                None => build_completions("", &[], line, raw_col, &prefab_paths),
            },
            Err(_) => build_completions("", &[], line, raw_col, &prefab_paths),
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        // "Fill record fields": inside a record literal whose expected type is a
        // record, offer to insert the missing fields with type-appropriate
        // defaults (recursing into nested records). Reuses the server's resolved
        // symbols, so nested / aliased / imported record types work.
        //
        // "Fill missing match arms" (Task 22): on/inside a `match` whose written
        // arms don't cover its scrutinee enum, offer to insert the missing arms
        // as witness patterns (`typecheck::patterns::analyze`, Task 11, the same
        // witness engine the compiler's own WS054 exhaustiveness diagnostic
        // uses), so the arms it offers are exactly the ones WS054 would otherwise
        // complain about. Each arm gets a plain `todo` placeholder body baked
        // into `new_text`. This server advertises no snippet capability and the
        // lsp-types version in use has no SnippetTextEdit, so LSP snippet syntax
        // (`${N:todo}` tab-stops) would land in the buffer as literal characters
        // that fail to parse; plain `todo` parses (an undefined identifier the
        // author replaces) and is the correct behavior until snippet edits are
        // supported.
        let uri = &params.text_document.uri;
        let pos = params.range.start;
        let line = pos.line as usize;

        // The source comes out with the fills: their insertion columns are char
        // columns, and converting one back for the editor needs the line text.
        let (source, record_fill, match_fill) = match self.docs.lock() {
            Ok(docs) => match docs.get(uri) {
                Some(doc) => {
                    let col = lsp_col_to_char(&doc.source, line, pos.character);
                    (
                        doc.source.clone(),
                        fill_record_at(&doc.source, &doc.symbols, line, col),
                        fill_match_arms_at(&doc.source, &doc.symbols, &doc.type_map, &doc.pre_resolve_ast, line, col),
                    )
                }
                None => (String::new(), None, None),
            },
            Err(_) => (String::new(), None, None),
        };

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        if let Some(fill) = record_fill {
            let at = Position {
                line: fill.line as u32,
                character: char_col_to_lsp(&source, fill.line, fill.col),
            };
            let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range { start: at, end: at },
                    new_text: fill.text,
                }],
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Fill record fields".into(),
                kind: Some(CodeActionKind::QUICKFIX),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }
        if let Some(fill) = match_fill {
            let at = Position {
                line: fill.line as u32,
                character: char_col_to_lsp(&source, fill.line, fill.col),
            };
            let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range { start: at, end: at },
                    new_text: fill.text,
                }],
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Fill missing match arms".into(),
                kind: Some(CodeActionKind::QUICKFIX),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }

        if actions.is_empty() {
            return Ok(None);
        }
        Ok(Some(actions))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                // `hover_at` walks the AST, so it gets the big stack like
                // every other AST walk here.
                if let Some(value) = wirescript::on_big_stack(|| {
                    hover_at(
                        &doc.source,
                        &uri_to_file_string(uri),
                        &doc.pre_resolve_ast,
                        &doc.symbols,
                        &doc.type_map,
                        &doc.doc_comments,
                        &doc.if_contexts,
                        &doc.var_read_contexts,
                        &doc.dropped_ranges,
                        &doc.resource_estimates,
                        pos.line as usize,
                        lsp_col_to_char(&doc.source, pos.line as usize, pos.character),
                    )
                }) {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value,
                        }),
                        range: None,
                    }));
                }
            }
        }
        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let line = pos.line as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let col = lsp_col_to_char(&doc.source, line, pos.character);
                // `$./file.brz` prefab reference → jump to the referenced file.
                if let Some(r) = asset_ref_at(&doc.source, line, col) {
                    if r.is_file() {
                        let target = resolve_prefab_path(&uri_to_file_string(uri), &r.path);
                        if let Ok(target_uri) = Url::from_file_path(&target) {
                            if target.is_file() {
                                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                                    uri: target_uri,
                                    range: Range::default(),
                                })));
                            }
                        }
                    }
                    // Asset ref or missing file: nothing to navigate to.
                    return Ok(None);
                }

                // Type field → show all references. A cursor on a record FIELD
                // NAME is deliberately excluded: it must fall through to the
                // `definition_at` field-resolution path below, not resolve to
                // the enclosing `type X = {…}` binding (whose coarse range spans
                // the whole decl and would otherwise surface the type's own
                // references instead of the clicked field's definition).
                let in_type_def = doc.symbols.iter().any(|s| {
                    s.kind == "type"
                        && s.range.start.line.saturating_sub(1) as usize <= line
                        && s.range.end.line.saturating_sub(1) as usize >= line
                });
                if in_type_def && !field_name_at(&doc.pre_resolve_ast, &doc.source, line, col) {
                    let file = uri_to_file_string(uri);
                    if let Some((target, current_sites)) =
                        references_at(&doc.pre_resolve_ast, &doc.source, &file, line, col)
                    {
                        let refs = collect_references_across_files(&docs, uri, &target, &current_sites);
                        if !refs.is_empty() {
                            let locations: Vec<Location> = refs
                                .iter()
                                .map(|(u, r)| Location {
                                    uri: u.clone(),
                                    range: text_range_to_lsp(&source_for(&docs, u), r),
                                })
                                .collect();
                            return Ok(Some(GotoDefinitionResponse::Array(locations)));
                        }
                    }
                }

                if let Some(loc) = definition_at(
                    &doc.source,
                    &doc.pre_resolve_ast,
                    &doc.symbols,
                    &uri_to_file_string(uri),
                    &FsLoader,
                    line,
                    col,
                ) {
                    let target_uri = loc
                        .file
                        .as_ref()
                        .and_then(|f| Url::from_file_path(f).ok())
                        .unwrap_or_else(|| uri.clone());
                    let target_src = source_for(&docs, &target_uri);
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: target_uri.clone(),
                        range: Range {
                            // `Location`'s columns come from a `SourceRange`,
                            // so they are byte columns too.
                            start: Position {
                                line: loc.start_line as u32,
                                character: byte_off_to_lsp(&target_src, loc.start_line, loc.start_col),
                            },
                            end: Position {
                                line: loc.end_line as u32,
                                character: byte_off_to_lsp(&target_src, loc.end_line, loc.end_col),
                            },
                        },
                    })));
                }
            }
        }
        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let line = pos.line as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let col = lsp_col_to_char(&doc.source, line, pos.character);
                let file = uri_to_file_string(uri);
                // Atom `:name` find-references — atoms are global (one xxHash64
                // value per name, no scope), so gather every `:name` occurrence
                // across the workspace.
                if let Some(a) = wirescript::analysis::atom_at(&doc.source, &file, line, col) {
                    return Ok(Some(collect_atom_references(&docs, uri, &a.name)));
                }
                if is_field_or_keyword(&doc.pre_resolve_ast, &doc.source, line, col) {
                    return Ok(None);
                }
                if let Some((target, current_sites)) =
                    references_at(&doc.pre_resolve_ast, &doc.source, &file, line, col)
                {
                    let refs = collect_references_across_files(&docs, uri, &target, &current_sites);
                    let locations: Vec<Location> = refs
                        .iter()
                        .map(|(u, r)| Location {
                            uri: u.clone(),
                            range: text_range_to_lsp(&source_for(&docs, u), r),
                        })
                        .collect();
                    return Ok(Some(locations));
                }
            }
        }
        Ok(None)
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let pos = params.position;
        let line = pos.line as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let col = lsp_col_to_char(&doc.source, line, pos.character);
                let file = uri_to_file_string(uri);
                if let Some((range, placeholder)) =
                    prepare_rename_at(&doc.pre_resolve_ast, &doc.source, &file, line, col)
                {
                    return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                        range: range_to_lsp(&doc.source, &range),
                        placeholder,
                    }));
                }
            }
        }
        Ok(None)
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let new_name = &params.new_name;
        let line = pos.line as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let col = lsp_col_to_char(&doc.source, line, pos.character);
                let file = uri_to_file_string(uri);
                if is_field_or_keyword(&doc.pre_resolve_ast, &doc.source, line, col) {
                    return Ok(None);
                }
                if let Some((target, current_sites)) =
                    references_at(&doc.pre_resolve_ast, &doc.source, &file, line, col)
                {
                    let refs = collect_references_across_files(&docs, uri, &target, &current_sites);

                    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
                    for (file_uri, r) in &refs {
                        changes.entry(file_uri.clone()).or_default().push(TextEdit {
                            range: text_range_to_lsp(&source_for(&docs, file_uri), r),
                            new_text: rename_edit_text(r, &target.name, new_name),
                        });
                    }

                    let doc_changes: Vec<DocumentChangeOperation> = changes
                        .into_iter()
                        .map(|(file_uri, edits)| {
                            DocumentChangeOperation::Edit(TextDocumentEdit {
                                text_document: OptionalVersionedTextDocumentIdentifier {
                                    uri: file_uri,
                                    version: None,
                                },
                                edits: edits.into_iter().map(OneOf::Left).collect(),
                            })
                        })
                        .collect();
                    return Ok(Some(WorkspaceEdit {
                        document_changes: Some(DocumentChanges::Operations(doc_changes)),
                        ..Default::default()
                    }));
                }
            }
        }
        Ok(None)
    }

    /// Corrective semantic-token overrides layered atop the TextMate
    /// grammar's position-blind `support.type` coloring (see
    /// `wirescript::analysis::semantic_tokens`'s doc comment) — a name in
    /// type position highlights as a type (including a user `type` alias the
    /// grammar's fixed builtin list can't know about), while a value binding
    /// that merely shares a type's spelling (a `character` capture, a
    /// capitalized `var`) highlights as its own kind instead.
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;

        let docs = match self.docs.lock() {
            Ok(d) => d,
            Err(_) => return Ok(None),
        };
        let Some(doc) = docs.get(uri) else {
            return Ok(None);
        };

        struct Tok {
            line: u32,
            start_char: u32,
            length: u32,
            token_type: u32,
        }

        let spans = semantic_tokens(&doc.pre_resolve_ast);
        let mut toks: Vec<Tok> = Vec::with_capacity(spans.len());
        for span in &spans {
            // A coarse span (a whole-declaration/whole-type-expr range) must
            // be narrowed to its precise name token first — an un-narrowable
            // one is skipped rather than tokenizing its whole container.
            let range = if span.coarse {
                match find_name_range(&doc.source, &span.range, &span.name) {
                    Some(r) => r,
                    None => continue,
                }
            } else {
                span.range.clone()
            };
            let token_type = match span.kind {
                SemTokenKind::Type => 0,
                SemTokenKind::Function => 1,
                SemTokenKind::Parameter => 2,
                SemTokenKind::Variable => 3,
                SemTokenKind::Namespace => 4,
            };
            // Both `col`s are 1-based byte columns; the protocol counts UTF-16
            // units, so the length has to be measured after converting, not
            // before. A span crossing lines has no single line to measure in;
            // no name token does, so the byte width stands in.
            let line = range.start.line.saturating_sub(1);
            let start_char =
                byte_off_to_lsp(&doc.source, line as usize, range.start.col.saturating_sub(1) as usize);
            let length = if range.start.line == range.end.line {
                byte_off_to_lsp(&doc.source, line as usize, range.end.col.saturating_sub(1) as usize)
                    .saturating_sub(start_char)
            } else {
                range.end.col.saturating_sub(range.start.col)
            };
            toks.push(Tok {
                line,
                start_char,
                length,
                token_type,
            });
        }
        toks.sort_by_key(|t| (t.line, t.start_char));

        let mut data = Vec::with_capacity(toks.len());
        let mut prev_line = 0u32;
        let mut prev_start = 0u32;
        for t in &toks {
            let delta_line = t.line - prev_line;
            let delta_start = if delta_line == 0 { t.start_char - prev_start } else { t.start_char };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: t.length,
                token_type: t.token_type,
                token_modifiers_bitset: 0,
            });
            prev_line = t.line;
            prev_start = t.start_char;
        }

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        if params.command != "wirescript.compile" {
            return Ok(None);
        }
        let uri_str = params
            .arguments
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let out_path = params
            .arguments
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if uri_str.is_empty() || out_path.is_empty() {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(
                "expected [uri, outputPath]",
            ));
        }

        let uri = Url::parse(uri_str)
            .map_err(|_| tower_lsp::jsonrpc::Error::invalid_params("invalid URI"))?;
        let file = uri_to_file_string(&uri);
        let src = std::fs::read_to_string(&file).map_err(|e| {
            tower_lsp::jsonrpc::Error::invalid_params(format!("cannot read {file}: {e}"))
        })?;

        // Everything below narrates the compile into the LSP output channel
        // ("Wirescript Language Server" in VS Code) via window/logMessage. A
        // compile is the one thing here that can take real time or fail deep in
        // the pipeline, and until now it was entirely opaque from the editor.
        self.client
            .log_message(
                MessageType::INFO,
                format!("[compile] start {file} -> {out_path}"),
            )
            .await;
        let started = std::time::Instant::now();

        let client = self.client.clone();
        let src_owned = src.clone();
        let file_owned = file.clone();
        // The compile runs on threads with no ambient tokio context (a
        // blocking-pool thread, and inside that the library's big-stack
        // compile worker) — capture an explicit runtime handle for the
        // progress callback; a bare `tokio::spawn` there panics with "no
        // reactor running" and takes the whole server down.
        let rt = tokio::runtime::Handle::current();
        let compile_result = tokio::task::spawn_blocking(move || {
            let progress_cb: wirescript::ProgressCallback =
                std::sync::Arc::new(move |p: wirescript::CompileProgress| {
                    let client = client.clone();
                    // Stamped here, on the compile thread, so the number is the
                    // phase's real start time even if the spawned tasks (which
                    // are independent, hence not strictly ordered) interleave.
                    let ms = started.elapsed().as_millis();
                    rt.spawn(async move {
                        client.send_notification::<CompileProgressNotification>(
                        serde_json::json!({ "step": p.step, "total": p.total, "done": p.done, "label": p.label })
                    ).await;
                        if !p.done {
                            client
                                .log_message(
                                    MessageType::INFO,
                                    format!("[compile] {}/{} {} ({ms}ms)", p.step, p.total, p.label),
                                )
                                .await;
                        }
                    });
                });
            wirescript::compile_with_progress(
                wirescript::CompileInput {
                    source: &src_owned,
                    file: &file_owned,
                    module_name: None,
                    fold_mode: FoldMode::Auto,
                },
                wirescript::EmitOptions::default(),
                progress_cb,
            )
        })
        .await
        // A panic inside the compile must fail THIS request, not the server.
        .map_err(|e| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InternalError,
            message: format!("compile task failed: {e}").into(),
            data: None,
        })?;

        self.client
            .send_notification::<CompileProgressNotification>(
                serde_json::json!({ "step": 0, "total": 0, "done": true }),
            )
            .await;

        let result = match compile_result {
            Ok(r) => r,
            // Build errors (e.g. an unbarriered wire-graph cycle, WS005) carry a
            // source range each. Hand them back to the editor as structured, located
            // diagnostics so they render in the Problems panel / as squiggles instead
            // of a stringified popup. This only runs on the on-demand Compile command,
            // never in live analyze(), so it can't reintroduce the lowering-on-every-
            // keystroke blowup that keeps analyze() typecheck-only.
            Err(wirescript::CompileError::HasErrors(diags)) => {
                let sources: HashMap<String, String> = {
                    let docs = self.docs.lock().ok();
                    diags
                        .iter()
                        .map(|d| d.range.file.to_string())
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .map(|f| {
                            let text = Url::from_file_path(&f)
                                .ok()
                                .and_then(|u| {
                                    docs.as_ref().and_then(|docs| {
                                        docs.get(&u).map(|s| s.source.clone())
                                    })
                                })
                                .or_else(|| std::fs::read_to_string(&f).ok())
                                .unwrap_or_default();
                            (f, text)
                        })
                        .collect()
                };
                let items = compile_diagnostic_items(&diags, &sources);
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!(
                            "[compile] failed: {} problem{} in {}ms",
                            diags.len(),
                            if diags.len() == 1 { "" } else { "s" },
                            started.elapsed().as_millis()
                        ),
                    )
                    .await;
                for d in &diags {
                    self.client
                        .log_message(
                            MessageType::ERROR,
                            format!(
                                "[compile]   {}:{}:{} {} {}",
                                d.range.file, d.range.start.line, d.range.start.col, d.code, d.message
                            ),
                        )
                        .await;
                }
                return Ok(Some(serde_json::json!({ "ok": false, "diagnostics": items })));
            }
            // Emit / IO failures have no per-source location — keep them as a plain
            // error the extension can pop up.
            Err(e) => {
                self.client
                    .log_message(
                        MessageType::ERROR,
                        format!("[compile] error: {e} ({}ms)", started.elapsed().as_millis()),
                    )
                    .await;
                return Err(tower_lsp::jsonrpc::Error {
                    code: tower_lsp::jsonrpc::ErrorCode::InvalidRequest,
                    message: e.to_string().into(),
                    data: None,
                });
            }
        };

        if let Err(e) = std::fs::write(out_path, &result.brz) {
            self.client
                .log_message(MessageType::ERROR, format!("[compile] write failed: {e}"))
                .await;
            return Err(tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::InternalError,
                message: format!("write failed: {e}").into(),
                data: None,
            });
        }

        let warnings = result
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, wirescript::diagnostic::Severity::Warning))
            .count();
        for d in result
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, wirescript::diagnostic::Severity::Warning))
        {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!(
                        "[compile]   {}:{}:{} {} {}",
                        d.range.file, d.range.start.line, d.range.start.col, d.code, d.message
                    ),
                )
                .await;
        }
        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "[compile] ok -> {out_path} ({} bytes, {} warning{}, {}ms)",
                    result.brz.len(),
                    warnings,
                    if warnings == 1 { "" } else { "s" },
                    started.elapsed().as_millis()
                ),
            )
            .await;

        Ok(Some(serde_json::json!({ "ok": true, "path": out_path })))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let hints = collect_inlay_hints(
                    &doc.source,
                    &doc.pre_resolve_ast,
                    &doc.type_map,
                    &uri_to_file_string(uri),
                );
                let lsp_hints: Vec<InlayHint> = hints
                    .into_iter()
                    .map(|h| InlayHint {
                        position: Position {
                            line: h.line as u32,
                            character: char_col_to_lsp(&doc.source, h.line, h.col),
                        },
                        label: InlayHintLabel::String(h.label),
                        kind: Some(match h.kind {
                            InlayHintKind::Type => tower_lsp::lsp_types::InlayHintKind::TYPE,
                            InlayHintKind::Parameter => {
                                tower_lsp::lsp_types::InlayHintKind::PARAMETER
                            }
                        }),
                        padding_left: None,
                        padding_right: None,
                        text_edits: None,
                        tooltip: None,
                        data: None,
                    })
                    .collect();
                return Ok(Some(lsp_hints));
            }
        }
        Ok(None)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let tab = if params.options.insert_spaces {
            " ".repeat(params.options.tab_size as usize)
        } else {
            "\t".to_string()
        };

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let formatted = format_wirescript(&doc.source, &tab);
                if formatted == doc.source {
                    return Ok(None);
                }
                // The document's real end. `str::lines` drops a trailing
                // newline, so pairing its count with the last line's length
                // named a position one past the end of the document; clients
                // clamp it, but a full-document edit should not need them to.
                let newlines = doc.source.matches('\n').count() as u32;
                let end = if doc.source.ends_with('\n') {
                    Position { line: newlines, character: 0 }
                } else {
                    let last_line = doc.source.lines().last().unwrap_or("");
                    Position {
                        line: newlines,
                        character: last_line.chars().map(char::len_utf16).sum::<usize>() as u32,
                    }
                };
                return Ok(Some(vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end,
                    },
                    new_text: formatted,
                }]));
            }
        }
        Ok(None)
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(|client| Backend {
        client,
        docs: Mutex::new(HashMap::new()),
        foreign_diags: Mutex::new(HashMap::new()),
        watch_files: std::sync::atomic::AtomicBool::new(false),
        change_gen: std::sync::atomic::AtomicU64::new(0),
    });
    // Above tower-lsp's default of 4. A debounced `did_change` holds its slot
    // while it waits (see `DEBOUNCE`), so with the default a burst of five
    // keystrokes would leave a hover queued behind them, the opposite of what
    // the debounce is for. These slots are cheap: a waiting handler is idle.
    Server::new(stdin, stdout, socket)
        .concurrency_level(32)
        .serve(service)
        .await;
}

/// Completions for `receiver.` — array methods, var fields, record fields, or
/// the receiver methods valid for a typed value. Returns only the members of the
/// receiver (possibly empty); it never falls through to the global
/// keyword/function list, so e.g. a `string` receiver shows only string methods.
fn member_completions(
    var_name: &str,
    symbols: &[SymbolDef],
    source: &str,
    line: usize,
    col: usize,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    let enum_registry = enum_registry_from_source(source);

    // `arr[i].` — an indexed read, not the array itself. Its members are the
    // array-get gate's outputs (the element `Value` and the `OutOfBounds`
    // flag), never the array's methods.
    if let Some(base) = var_name.strip_suffix("[]") {
        let sym = resolve_symbol(symbols, source, base, line, col);
        let elem = sym
            .and_then(|s| s.ty.as_deref())
            .and_then(|t| t.strip_suffix("[]"))
            .unwrap_or("");
        for (name, ty) in [("Value", elem), ("OutOfBounds", "bool")] {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(if ty.is_empty() {
                    "field".to_string()
                } else {
                    format!("{name}: {ty}")
                }),
                insert_text: Some(name.to_string()),
                ..Default::default()
            });
        }
        return items;
    }

    let sym = resolve_symbol(symbols, source, var_name, line, col);

    // Field name (record field / swizzle component) completion item. Declared
    // ahead of the bare enum-type receiver check below so that check's bare-
    // variant fallback can also reach `push_type_members` (which closes over
    // `field_item`).
    let field_item = |name: String| CompletionItem {
        label: name.clone(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some("field".to_string()),
        insert_text: Some(name),
        ..Default::default()
    };
    // Method + swizzle members valid for a typed value: an enum-typed value's
    // `.Discriminant`, swizzle fields, builtin receiver-methods, then
    // in-scope user `self`-mods whose receiver matches.
    let push_type_members = |ty: &str, items: &mut Vec<CompletionItem>| {
        if enum_registry.contains_key(ty) {
            items.push(CompletionItem {
                label: "Discriminant".to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(format!("{ty} discriminant (int)")),
                insert_text: Some("Discriminant".to_string()),
                ..Default::default()
            });
        }
        for f in swizzle_fields(ty) {
            items.push(field_item(f.to_string()));
        }
        for (name, sig) in receiver_methods(ty) {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(sig),
                ..Default::default()
            });
        }
        for (name, sig) in user_receiver_methods(ty, symbols) {
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(format!("{name}{sig}")),
                insert_text: Some(name),
                ..Default::default()
            });
        }
    };

    // Bare enum-type receiver (`Shape.<here>`): a variant PATH ready for
    // construction or `.Discriminant`, not a value. Only when the name is NOT
    // shadowed by a value binding: `collect_symbols_for_file` emits no symbol
    // for an `enum` decl itself, so an unshadowed enum name resolves to `None`
    // (or, defensively, a `type` symbol), whereas a `var`/`let`/param/mod of
    // the same name resolves to that value here and must win. This mirrors the
    // compiler's own shadow guard (`infer.rs`'s
    // `resolve_variant_for_construction`: a value symbol whose name equals the
    // enum's shadows the type, so the name is NOT a construction site).
    if sym.is_none_or(|s| s.kind == "type") {
        if let Some(def) = enum_registry.get(var_name) {
            push_variant_completions(&mut items, def);
            return items;
        }
        // A bare variant name used as a value receiver (`EasingFunction.
        // Bounce.`, or any `Enum.Variant.` chain) arrives here as just
        // `Variant`, because `member_receiver_at` only reports the identifier
        // directly before the dot, discarding the `Enum.` qualifier. Resolve
        // it the same way the compiler resolves a bare variant
        // (`resolve_bare_variant_enum`): on a UNIQUE owning enum, treat the
        // receiver as a value of that enum type, offering `.Discriminant`
        // (via `push_type_members`) exactly as a directly-typed value would.
        // An ambiguous or unknown bare name yields no owner and falls through
        // unchanged.
        if let Some(owner) = wirescript::typecheck::enums::resolve_bare_variant_enum(
            &enum_registry,
            var_name,
            |n| enum_registry.contains_key(n),
        ) {
            push_type_members(owner, &mut items);
            if !items.is_empty() {
                return items;
            }
        }
    }

    // Collection methods come from the receiver's declared type (resolved through
    // type aliases): a `Map<K, V>` gets the map table, `T[]` or an `array` decl
    // the array table. The tables are distinct — a map's `length`/`clear`/
    // `copyFrom` are its own, and `get`/`set`/`has`/`keys`/`values` exist on no
    // array — so this dispatches on type, never on the bare method name.
    let collection = sym.and_then(|s| {
        s.ty.as_deref()
            .and_then(|ty| collection_kind(ty, symbols))
            .or_else(|| (s.kind == "array").then_some(CollectionKind::Array))
    });
    if let Some(kind) = collection {
        let method_item = |name: &str, signature: &str, doc: &str| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!("{name}{signature}")),
            documentation: Some(Documentation::String(doc.to_string())),
            ..Default::default()
        };
        match kind {
            CollectionKind::Array => {
                for m in ARRAY_METHODS {
                    items.push(method_item(m.name, m.signature, m.doc));
                }
            }
            CollectionKind::Map => {
                for m in MAP_METHODS {
                    items.push(method_item(m.name, m.signature, m.doc));
                }
            }
        }
        return items;
    }

    // Namespace alias (`import * as u`): offer its qualified `u.member` symbols.
    if sym.is_some_and(|s| s.kind == "namespace") {
        let prefix = format!("{var_name}.");
        for m in symbols {
            if let Some(member) = m.name.strip_prefix(&prefix) {
                if member.contains('.') {
                    continue; // deeper nesting isn't a direct member
                }
                items.push(CompletionItem {
                    label: member.to_string(),
                    kind: Some(namespace_member_kind(m.kind)),
                    insert_text: Some(member.to_string()),
                    ..Default::default()
                });
            }
        }
        return items;
    }

    // Vars (mutable `var` / `static var`) expose `.Value`/`.prev`, plus any
    // method/swizzle valid for the var's element type (`pos.Normalize()`,
    // `pos.x`).
    if sym.is_some_and(|s| matches!(s.kind, "var" | "static var")) {
        for (name, detail) in &[
            ("Value", "Read current value (pure)"),
            ("prev", "Read previous tick's value"),
        ] {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(detail.to_string()),
                insert_text: Some(name.to_string()),
                ..Default::default()
            });
        }
        if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
            push_type_members(ty, &mut items);
        }
        return items;
    }

    // Record-typed value (e.g. `let split = pl.InputReader()` → {Forward, Right,
    // Jump}, or a multi-output mod result): offer the record's field names, so
    // `split.<here>` / `on split.<here>` completes `Forward`/`Right`/`Jump`. The
    // type may be an inline `{…}` string or a named `type` alias resolved here.
    if let Some(fields) = sym
        .and_then(|s| s.ty.as_deref())
        .and_then(|ty| resolve_record_fields(ty, symbols))
    {
        for f in fields {
            items.push(field_item(f));
        }
        return items;
    }

    // Any other typed value: methods (e.g. string methods on a string) + swizzle.
    if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
        push_type_members(ty, &mut items);
    }

    items
}

/// Record field names for a receiver type string: an inline `{…}` record, or a
/// named `type` alias resolved through the `type` symbol it points at.
fn resolve_record_fields(ty: &str, symbols: &[SymbolDef]) -> Option<Vec<String>> {
    if let Some(fields) = record_field_names(ty) {
        return Some(fields);
    }
    let alias = symbols.iter().find(|s| s.name == ty && s.kind == "type")?;
    record_field_names(alias.ty.as_deref()?)
}

/// The completion-item kind for a namespace member of the given symbol kind.
fn namespace_member_kind(kind: &str) -> CompletionItemKind {
    match kind {
        "mod" | "chip" | "fn" => CompletionItemKind::FUNCTION,
        "let" => CompletionItemKind::CONSTANT,
        "type" => CompletionItemKind::CLASS,
        "event" => CompletionItemKind::EVENT,
        _ => CompletionItemKind::FIELD,
    }
}

/// The Compile command's diagnostic payload. `sources` maps a diagnostic's
/// `range.file` to that file's text.
///
/// `Pos::col` is a BYTE column and the extension feeds `startChar`/`endChar`
/// straight to a `vscode.Range`, whose characters are UTF-16 code units, so
/// every column is converted here, against the text of the file the range
/// names, since a build error can name an imported file. Handing the editor a
/// raw byte column puts the squiggle right of the token on any line holding
/// non-ASCII text, and disagreeing with the live diagnostics for that same
/// file is how it shows up.
fn compile_diagnostic_items(
    diags: &[wirescript::diagnostic::Diagnostic],
    sources: &HashMap<String, String>,
) -> Vec<serde_json::Value> {
    diags
        .iter()
        .map(|d| {
            let severity = match d.severity {
                wirescript::diagnostic::Severity::Error => "error",
                wirescript::diagnostic::Severity::Warning => "warning",
                _ => "info",
            };
            let text = sources.get(&*d.range.file).map_or("", String::as_str);
            let start_line = d.range.start.line.saturating_sub(1) as usize;
            let end_line = d.range.end.line.saturating_sub(1) as usize;
            serde_json::json!({
                "file": &*d.range.file,
                "startLine": start_line,
                "startChar": byte_off_to_lsp(
                    text,
                    start_line,
                    d.range.start.col.saturating_sub(1) as usize,
                ),
                "endLine": end_line,
                "endChar": byte_off_to_lsp(
                    text,
                    end_line,
                    d.range.end.col.saturating_sub(1) as usize,
                ),
                "severity": severity,
                "code": d.code,
                "message": d.message,
            })
        })
        .collect()
}

/// Inverse of [`cursor_byte_offset`]: a byte `offset` back to a zero-based
/// `(line, CHAR col)`, the coordinates the completion entry points take.
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, c) in source.char_indices() {
        if i >= offset {
            return (line, col);
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// If `(line, col)` sits inside a `$```…``` ` nested-prefab block, return the
/// block's inner source and the cursor's position remapped into it. LSP features
/// can then analyze the inner block as its own isolated program instead of
/// leaking the outer file's context into it. Returns `None` outside any block.
fn nested_block_at(
    source: &str,
    file: &str,
    line: usize,
    col: usize,
) -> Option<(String, usize, usize)> {
    // Fast path: nested-prefab blocks open with a `$``` ` fence. The vast
    // majority of documents have none, so skip re-lexing the whole file on
    // every completion when the fence marker is absent entirely.
    if !source.contains("$```") {
        return None;
    }
    let cursor = cursor_byte_offset(source, line, col);
    let lexed = wirescript::lex(source, file);
    for t in &lexed.tokens {
        if t.kind != wirescript::TokenKind::NestedPrefab {
            continue;
        }
        let Some(wirescript::lexer::TokenValue::Str(inner)) = &t.value else {
            continue;
        };
        // Inner text begins just past the opening `$``` fence.
        let content_start = t.start.offset + 4;
        let content_end = content_start + inner.len();
        if cursor >= content_start && cursor <= content_end {
            let (il, ic) = offset_to_line_col(inner, cursor - content_start);
            return Some((inner.clone(), il, ic));
        }
    }
    None
}

/// The `enum` declarations visible in `source`, keyed by name, plus the
/// built-in `Option`/`Result` prelude - reparsed on demand so enum-aware
/// completions don't require threading a cached AST/type-checked document
/// through every completion path. Gated by a fast substring check: a file
/// with no `enum ` keyword at all cannot contain a `TopDecl::Enum`, so
/// skipping the reparse in that case returns the exact same registry
/// (`build_registry` only reads `TopDecl::Enum` entries out of `decls`) that
/// a full parse would, at zero cost for the common no-enum file.
fn enum_registry_from_source(
    source: &str,
) -> wirescript::collections::HashMap<String, wirescript::typecheck::enums::EnumDef> {
    if !source.contains("enum ") {
        return wirescript::typecheck::enums::build_registry(&[]);
    }
    let ast = wirescript::on_big_stack(|| wirescript::parse(source, "completion")).ast;
    wirescript::typecheck::enums::build_registry(&ast.decls)
}

/// Push a completion item for each variant of `def` - the user-enum analog of
/// `push_enum_member_completions` below, used both for `match`-arm-head
/// pattern completion and for a bare `Enum.<here>` variant-path receiver.
fn push_variant_completions(items: &mut Vec<CompletionItem>, def: &wirescript::typecheck::enums::EnumDef) {
    for v in &def.variants {
        items.push(CompletionItem {
            label: v.name.clone(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            detail: Some(format!("{} variant", def.name)),
            insert_text: Some(v.name.clone()),
            ..Default::default()
        });
    }
}

/// If the cursor sits in a `match <scrutinee> { <here> }` arm-head position -
/// directly inside the match's own arm-list braces, at or past the last arm
/// separator (a `,`, or a comma-less block arm's closing `}`, or the opening
/// brace) and before any `=>` of an in-progress arm - return the scrutinee's
/// raw source text (trimmed). `None` inside a pattern's own parens
/// (`Circle(<here>)`), inside an arm's body (`Empty => { <here> }` or past
/// its `=>`), or outside any match at all.
///
/// Scans the whole prefix up to the cursor once, like `find_enclosing_call`
/// above, skipping strings/comments and tracking brace/paren/bracket nesting
/// with a small frame stack. Only the innermost `{ }` opened directly by a
/// `match <expr>` is tagged `MatchArms`; every other brace/paren/bracket is
/// an opaque `Other` frame, except the block body of an arm (a `{` whose arm
/// is `AwaitingBody`, the state right after `=>`), which is tagged `ArmBlock`
/// so that popping it returns the enclosing arm to `Head`. That mirrors the
/// parser: a block-bodied arm may omit the trailing comma
/// (`parser/expr.rs`), so the cursor after a comma-less `Empty => { .. }` is
/// still an arm head. A scrutinee expression that itself contains a raw `{`
/// (a record literal, e.g. `match f(){x:1} { ... }`) is not handled - the
/// `{` would be mistaken for the arms brace - but that shape is rare enough
/// not to be worth the extra bookkeeping here.
fn match_arm_head_scrutinee_at(source: &str, line: usize, col: usize) -> Option<String> {
    let offset = cursor_byte_offset(source, line, col);
    let prefix = &source[..offset];
    let bytes = prefix.as_bytes();

    // Per-arm position within a `MatchArms` frame: `Head` is pattern position
    // (the only state that offers variant completions); `AwaitingBody` is the
    // gap right after `=>` before the body's first token (which decides block
    // vs expression body); `InBody` is anywhere in an expression body.
    #[derive(PartialEq)]
    enum ArmState {
        Head,
        AwaitingBody,
        InBody,
    }

    enum Frame {
        MatchArms { scrutinee: String, state: ArmState },
        Other,
        // The `{ }` block body of a match arm; popping it returns the
        // enclosing arm to `Head` (a block arm may drop its trailing comma).
        ArmBlock,
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut pending_match_start: Option<usize> = None;
    let mut in_string: Option<u8> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(q) = in_string {
            if c == b'\\' {
                i += 2; // skip the escaped char
            } else {
                if c == q {
                    in_string = None;
                }
                i += 1;
            }
            continue;
        }
        // The first non-trivia token after `=>` fixes the body shape: a `{`
        // opens a block body (handled below), anything else is an expression
        // body, so a later `{` in it (a record literal) is not the arm block.
        if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'{') {
            if let Some(Frame::MatchArms { state, .. }) = stack.last_mut() {
                if *state == ArmState::AwaitingBody {
                    *state = ArmState::InBody;
                }
            }
        }
        match c {
            b'"' | b'\'' => {
                in_string = Some(c);
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                in_line_comment = true;
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                in_block_comment = true;
                i += 2;
            }
            b'{' => {
                if let Some(start) = pending_match_start.take() {
                    let scrutinee = prefix[start..i].trim().to_string();
                    stack.push(Frame::MatchArms { scrutinee, state: ArmState::Head });
                } else if matches!(
                    stack.last(),
                    Some(Frame::MatchArms { state: ArmState::AwaitingBody, .. })
                ) {
                    stack.push(Frame::ArmBlock);
                } else {
                    stack.push(Frame::Other);
                }
                i += 1;
            }
            b'}' => {
                let popped = stack.pop();
                if matches!(popped, Some(Frame::ArmBlock)) {
                    if let Some(Frame::MatchArms { state, .. }) = stack.last_mut() {
                        *state = ArmState::Head;
                    }
                }
                i += 1;
            }
            b'(' | b'[' => {
                stack.push(Frame::Other);
                i += 1;
            }
            b')' | b']' => {
                stack.pop();
                i += 1;
            }
            b',' => {
                if let Some(Frame::MatchArms { state, .. }) = stack.last_mut() {
                    *state = ArmState::Head;
                }
                i += 1;
            }
            b'=' if bytes.get(i + 1) == Some(&b'>') => {
                if let Some(Frame::MatchArms { state, .. }) = stack.last_mut() {
                    *state = ArmState::AwaitingBody;
                }
                i += 2;
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if &prefix[start..i] == "match" {
                    pending_match_start = Some(i);
                }
            }
            _ => i += 1,
        }
    }

    match stack.last() {
        Some(Frame::MatchArms { scrutinee, state: ArmState::Head }) if !scrutinee.is_empty() => {
            Some(scrutinee.clone())
        }
        _ => None,
    }
}

/// Push a completion item for each member of enum `et`. `filter_text` is set to
/// whatever is already typed (`value_so_far`) so VS Code keeps showing every
/// sibling even when the cursor sits at the end of a complete member. Shared by
/// the `CallSpec` and `EventSpec` named-arg value paths.
fn push_enum_member_completions(items: &mut Vec<CompletionItem>, et: &str, value_so_far: &str) {
    let filter = value_so_far.trim().to_string();
    for v in wirescript::catalog::enum_member_names(et) {
        items.push(CompletionItem {
            label: v.clone(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            detail: Some(format!("{et} member")),
            insert_text: Some(v),
            filter_text: Some(filter.clone()),
            ..Default::default()
        });
    }
}

/// Push a QUALIFIED completion item (`EasingFunction.Bounce`) for each variant
/// of the built-in game enum backed by schema enum `et`, if any, alongside
/// [`push_enum_member_completions`]'s bare-name items, so a config value slot
/// resolves to a schema enum offers both the bare member and the qualified
/// form an author would use to construct the value directly. `et` is the raw
/// schema type (`config_enum_for_named_arg`'s return); at most one built-in
/// game enum maps to it, found via [`wirescript::catalog::game_enum_schema_type`].
/// A no-op for a schema enum with no built-in game-enum counterpart.
fn push_qualified_builtin_game_enum_variant_completions(
    items: &mut Vec<CompletionItem>,
    et: &str,
    value_so_far: &str,
) {
    let filter = value_so_far.trim().to_string();
    let Some(def) = wirescript::typecheck::enums::game_enum_defs()
        .into_iter()
        .find(|def| wirescript::catalog::game_enum_schema_type(&def.name) == Some(et))
    else {
        return;
    };
    for v in &def.variants {
        let label = format!("{}.{}", def.name, v.name);
        items.push(CompletionItem {
            label: label.clone(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            detail: Some(format!("{} variant", def.name)),
            insert_text: Some(label),
            filter_text: Some(filter.clone()),
            ..Default::default()
        });
    }
}

/// Build completion items for a position. Pure (no document lock / async) so it
/// can be unit-tested.
fn build_completions(
    source: &str,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
    prefab_paths: &[String],
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // Prefab file reference `$./file.brz` / `$/abs/file.brz`: complete from the
    // candidate paths the frontend supplied (disk scan / drag registry). A
    // text edit over the whole `$…` fragment keeps `.`/`/` filtering robust.
    if let Some(l) = source.lines().nth(line) {
        let col_idx = char_col_to_byte(l, col);
        let before = &l[..col_idx];
        if let Some(dollar) = before.rfind('$') {
            let frag = &before[dollar + 1..];
            let is_prefab_frag = (frag.starts_with('.') || frag.starts_with('/'))
                && frag
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | '.' | '-'));
            if is_prefab_frag {
                let range = Range {
                    start: Position {
                        line: line as u32,
                        character: byte_off_to_lsp(source, line, dollar + 1),
                    },
                    end: Position {
                        line: line as u32,
                        character: char_col_to_lsp(source, line, col),
                    },
                };
                for path in prefab_paths {
                    if path.starts_with(frag) {
                        items.push(CompletionItem {
                            label: path.clone(),
                            kind: Some(CompletionItemKind::FILE),
                            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                                range,
                                new_text: path.clone(),
                            })),
                            ..Default::default()
                        });
                    }
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // Asset reference `$AssetType/AssetName`: complete types after `$`, names
    // after `$Type/`.
    if let Some(l) = source.lines().nth(line) {
        let col_idx = char_col_to_byte(l, col);
        let before = &l[..col_idx];
        if let Some(dollar) = before.rfind('$') {
            let frag = &before[dollar + 1..];
            if frag
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '/')
            {
                if let Some(slash) = frag.find('/') {
                    // filter_text = the name already typed, so ctrl-space at the
                    // end of a complete name still lists the other assets.
                    let typed = frag[slash + 1..].to_string();
                    for name in wirescript::analysis::asset_names(&frag[..slash]) {
                        items.push(CompletionItem {
                            label: name.to_string(),
                            kind: Some(CompletionItemKind::CONSTANT),
                            filter_text: Some(typed.clone()),
                            ..Default::default()
                        });
                    }
                } else {
                    for ty in wirescript::analysis::asset_types() {
                        items.push(CompletionItem {
                            label: ty.to_string(),
                            kind: Some(CompletionItemKind::CLASS),
                            insert_text: Some(format!("{ty}/")),
                            ..Default::default()
                        });
                    }
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // Member access `receiver.partial` — return only the receiver's members.
    // Checked before call-param completion so `Call(arg = recv.<here>` shows
    // recv's methods, not the enclosing call's params. A plain `Call(` (the dot
    // belongs to the callee, cursor at an arg boundary) yields no receiver here
    // and falls through to param completion below.
    if let Some(var_name) = member_receiver_at(source, line, col) {
        return member_completions(&var_name, symbols, source, line, col);
    }

    // Match-arm-head position (`match <scrutinee> { <here> }`): if the
    // scrutinee resolves to a registered enum, offer its variant names as
    // patterns (`Circle`, `Empty`, ...). Checked before call-param
    // completion for the same reason as member access above - a `match`
    // isn't itself a call, but its arm head can sit inside one lexically
    // (`foo(x = match s { <here> })`), and the arm-head reading must win.
    if let Some(scrutinee) = match_arm_head_scrutinee_at(source, line, col) {
        if let Some(ty) = resolve_symbol(symbols, source, &scrutinee, line, col).and_then(|s| s.ty.as_deref()) {
            let registry = enum_registry_from_source(source);
            if let Some(def) = registry.get(ty) {
                push_variant_completions(&mut items, def);
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // Named params inside a function call: `Call(<here>)`.
    if let Some(call_name) = find_enclosing_call(source, line, col) {
        if let Some(spec) = calls().get(call_name.as_str()) {
            // Enum-valued named arg (e.g. `justify = Center`): complete the
            // enum's member names when the cursor is in the value slot. Members
            // insert bare (the idiomatic form; a quoted string also works).
            let value_ctx = named_arg_value(source, line, col);
            if let Some((param_name, value_so_far)) = value_ctx.as_ref() {
                // Enum config value — works for both a hand-coded param
                // (`justify = …`) and a raw config field (`Justification = …`),
                // resolved through the unified call+event helper.
                if let Some(et) = wirescript::catalog::config_enum_for_named_arg(
                    call_name.as_str(),
                    param_name,
                ) {
                    push_enum_member_completions(&mut items, et, value_so_far);
                    push_qualified_builtin_game_enum_variant_completions(&mut items, et, value_so_far);
                    if !items.is_empty() {
                        return items;
                    }
                }
                if let Some(param) = spec.params.iter().find(|p| &p.name == param_name) {
                    // Asset-ref config param (`font = <here>`, `weapon = <here>`):
                    // offer full `$Type/Name` refs for the param's asset type, so
                    // the author needn't know the type name. (Once they type `$`,
                    // the `$Type/` block above takes over.) Constant-only params
                    // only — a wire-input Entity port takes a live value.
                    if !wirescript::catalog::is_wire_input(spec.gate_class, param.port.as_str()) {
                        if let Some(asset_ty) =
                            wirescript::analysis::asset_type_for_port(param.port.as_str())
                        {
                            for name in wirescript::analysis::asset_names(asset_ty) {
                                let full = format!("${asset_ty}/{name}");
                                items.push(CompletionItem {
                                    label: full.clone(),
                                    kind: Some(CompletionItemKind::CONSTANT),
                                    detail: Some(format!("{asset_ty} asset")),
                                    insert_text: Some(full),
                                    ..Default::default()
                                });
                            }
                            if !items.is_empty() {
                                return items;
                            }
                        }
                    }
                }
            }
            // Argument-NAME completions: only when NOT completing a value. In a
            // `name = <here>` value slot that wasn't an enum/asset value, skip the
            // arg names and fall through to the in-scope identifier list below.
            if value_ctx.is_none() {
                for (i, p) in spec.params.iter().enumerate() {
                    // The receiver param (index 0 on a method call) is already
                    // supplied via the `x.Method(` syntax — don't offer it.
                    if i == 0 && spec.receiver.is_some() {
                        continue;
                    }
                    // A config param surfaced as a plain int but backed by a schema
                    // enum shows the enum's name instead of `int`.
                    let ty_label = wirescript::field_enum_type(spec.gate_class, p.port.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| type_str(&p.ty));
                    if p.optional {
                        items.push(CompletionItem {
                            label: format!("{} = ", p.name),
                            kind: Some(CompletionItemKind::FIELD),
                            detail: Some(format!("{ty_label} (optional)")),
                            insert_text: Some(format!("{} = ", p.name)),
                            ..Default::default()
                        });
                    } else {
                        items.push(CompletionItem {
                            label: p.name.to_string(),
                            kind: Some(CompletionItemKind::FIELD),
                            detail: Some(format!("{ty_label} (required)")),
                            ..Default::default()
                        });
                    }
                }
                // Data-driven config attributes: the gate's raw settings-menu field
                // names (those without a hand-coded alias param).
                for cfg in wirescript::catalog::scalar_config_fields(spec.gate_class) {
                    if spec.params.iter().any(|p| p.name == cfg.name) {
                        continue;
                    }
                    let ty_label =
                        wirescript::catalog::config_field_enum_type(spec.gate_class, &cfg.name)
                            .map(str::to_string)
                            .unwrap_or_else(|| cfg.ty.clone());
                    items.push(CompletionItem {
                        label: format!("{} = ", cfg.name),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(format!("{ty_label} (config)")),
                        insert_text: Some(format!("{} = ", cfg.name)),
                        ..Default::default()
                    });
                }
                if !items.is_empty() {
                    return items;
                }
            }
        } else if let Some(evt) = wirescript::catalog::events::find_event(call_name.as_str()) {
            // Event trigger config (`on Clock(<here>)`): the call-param path has
            // no CallSpec, so resolve names/enum values from the EventSpec.
            let value_ctx = named_arg_value(source, line, col);
            if let Some((param_name, value_so_far)) = value_ctx.as_ref() {
                if let Some(et) =
                    wirescript::catalog::config_enum_for_named_arg(call_name.as_str(), param_name)
                {
                    push_enum_member_completions(&mut items, et, value_so_far);
                    push_qualified_builtin_game_enum_variant_completions(&mut items, et, value_so_far);
                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            // Argument-NAME completions only when NOT completing a value; in a
            // value slot, fall through to the in-scope identifier list below.
            if value_ctx.is_none() {
                for (surf, _, _) in &evt.input_named {
                    items.push(CompletionItem {
                        label: format!("{surf} = "),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some("wired input".to_string()),
                        insert_text: Some(format!("{surf} = ")),
                        ..Default::default()
                    });
                }
                for (surf, field) in &evt.config_named {
                    let ty_label =
                        wirescript::catalog::config_field_enum_type(evt.gate_class, field)
                            .map(str::to_string)
                            .unwrap_or_else(|| "config".to_string());
                    items.push(CompletionItem {
                        label: format!("{surf} = "),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(format!("{ty_label} (config)")),
                        insert_text: Some(format!("{surf} = ")),
                        ..Default::default()
                    });
                }
                if !items.is_empty() {
                    return items;
                }
            }
        } else if let Some(sig) = symbols
            .iter()
            .find(|s| s.name == call_name.as_str() && matches!(s.kind, "mod" | "chip" | "fn"))
            .and_then(|s| s.ty.as_deref())
        {
            // User-defined mod/chip/fn call: complete its parameter names,
            // parsed from the signature string in the symbol's type.
            if let Some(names) = param_names(sig) {
                for name in names {
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::FIELD),
                        insert_text: Some(name),
                        ..Default::default()
                    });
                }
                if !items.is_empty() {
                    return items;
                }
            }
        }
    }

    // User symbols. Qualified namespace members (`u.member`) are addressed only
    // through `u.` member completion, so keep them out of the bare-identifier list.
    // Dedupe by name: when a name is declared in several scopes, offer only the
    // one in scope at the cursor (nearest enclosing/preceding declaration), so a
    // handler-local `players: character[]` doesn't leak into file scope where a
    // different `players` is visible. Forward references to top-level symbols
    // still appear (`resolve_symbol` falls back to the first declaration).
    // Precompute the nearest-preceding (and first-seen) declaration per name in
    // ONE pass over the symbol table, then resolve each unique name by O(1)
    // lookup below. `resolve_symbol` re-scans every symbol per call, so calling
    // it once per unique name made this loop O(n²). This mirrors its rule: the
    // nearest declaration at/before the cursor wins, else the first declaration.
    let (cl, cc) = ((line + 1) as u32, (col + 1) as u32);
    let mut first: wirescript::collections::HashMap<&str, &SymbolDef> =
        wirescript::collections::HashMap::default();
    let mut best: wirescript::collections::HashMap<&str, (&SymbolDef, (u32, u32))> =
        wirescript::collections::HashMap::default();
    for s in symbols {
        if s.name.contains('.') {
            continue;
        }
        first.entry(s.name.as_str()).or_insert(s);
        let p = (s.range.start.line, s.range.start.col);
        let precedes = p.0 < cl || (p.0 == cl && p.1 <= cc);
        if precedes && best.get(s.name.as_str()).is_none_or(|(_, bp)| p > *bp) {
            best.insert(s.name.as_str(), (s, p));
        }
    }
    let mut seen: wirescript::collections::HashSet<&str> = wirescript::collections::HashSet::default();
    for sym in symbols {
        if sym.name.contains('.') {
            continue;
        }
        if !seen.insert(sym.name.as_str()) {
            continue;
        }
        let chosen = best
            .get(sym.name.as_str())
            .map(|(s, _)| *s)
            .or_else(|| first.get(sym.name.as_str()).copied())
            .unwrap_or(sym);
        let kind = match chosen.kind {
            "var" | "static var" | "buffer" | "array" => CompletionItemKind::VARIABLE,
            "fn" | "mod" | "chip" => CompletionItemKind::FUNCTION,
            "in" => CompletionItemKind::FIELD,
            "let" => CompletionItemKind::CONSTANT,
            "event" => CompletionItemKind::EVENT,
            "namespace" => CompletionItemKind::MODULE,
            "type" => CompletionItemKind::CLASS,
            _ => CompletionItemKind::TEXT,
        };
        items.push(CompletionItem {
            label: chosen.name.clone(),
            kind: Some(kind),
            detail: chosen.ty.clone(),
            ..Default::default()
        });
    }

    for kw in KEYWORDS {
        items.push(CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }

    // Annotations: `@side` port pins, the chip annotations, and the
    // module-level run that opens a file.
    for (ann, detail) in [
        ("@left", "outer rerouter pin"),
        ("@right", "outer rerouter pin"),
        ("@top", "outer rerouter pin"),
        ("@bottom", "outer rerouter pin"),
        ("@label", "display-text override"),
        ("@closed", "compile chip collapsed"),
        ("@fold", "module-level: fold constant expressions"),
        ("@nofold", "module-level: never fold"),
        ("@layout", "module-level: placement engine (code/cube)"),
        ("@flat", "module-level: inline every chip onto one grid"),
    ] {
        items.push(CompletionItem {
            label: ann.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..Default::default()
        });
    }

    // Built-in events (RoundStart, ChatCommand, CharacterSpawned, ...).
    for (name, evt) in events().iter() {
        let params: Vec<&str> = evt.data.iter().map(|d| d.name).collect();
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::EVENT),
            detail: if params.is_empty() {
                None
            } else {
                Some(format!("({})", params.join(", ")))
            },
            ..Default::default()
        });
    }

    for (name, spec) in calls().iter() {
        let params_str: Vec<String> = spec
            .params
            .iter()
            .map(|p| {
                if p.optional {
                    format!("{}?", p.name)
                } else {
                    p.name.to_string()
                }
            })
            .collect();
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("({})", params_str.join(", "))),
            ..Default::default()
        });
    }

    // Callable gate builtins (GetMapElement, PushToArray, SetVariable, …) — they
    // desugar to the method/assignment forms, so they aren't in `calls()`.
    for name in wirescript::catalog::gate_builtins::ALL {
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some("gate builtin".to_string()),
            ..Default::default()
        });
    }

    for ty in &[
        "int", "float", "bool", "string", "entity", "controller", "character", "vector",
        "rotator", "color", "exec",
    ] {
        items.push(CompletionItem {
            label: ty.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            ..Default::default()
        });
    }

    // Built-in game enum type names (`EasingFunction` and friends). Unlike a
    // user-declared `enum`, these have no `TopDecl::Enum` and so never appear
    // in `symbols`; offer them here so `var e: <here>` completes them the
    // same as any hardcoded scalar type above.
    for def in wirescript::typecheck::enums::game_enum_defs() {
        items.push(CompletionItem {
            label: def.name.clone(),
            kind: Some(CompletionItemKind::CLASS),
            detail: Some("game enum".to_string()),
            insert_text: Some(def.name),
            ..Default::default()
        });
    }

    items
}

#[cfg(test)]
mod tests;
