use std::collections::HashMap;
use std::sync::Mutex;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use wirescript::analysis::{
    asset_ref_at, collect_estimates, collect_inlay_hints, collect_symbols_for_file, definition_at,
    collection_kind, field_name_at, fill_record_at, find_asset_refs, find_enclosing_call, find_name_range,
    format_wirescript, hover_at, member_receiver_at, named_arg_value, param_names,
    prepare_rename_at, receiver_methods, record_field_names, references_at, references_to_export,
    rename_edit_text, resolve_symbol, semantic_tokens, swizzle_fields, type_str,
    user_receiver_methods, word_at, AssetRef, CollectionKind, CrossFile, InlayHintKind, RefNs,
    RefSite, RefTarget, ResourceEstimate, SemTokenKind, SymbolDef, TextRange, TypeMap,
    VarReadContextMap,
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

fn pos_to_lsp(p: wirescript::diagnostic::Pos) -> Position {
    Position {
        line: p.line.saturating_sub(1) as u32,
        character: p.col.saturating_sub(1) as u32,
    }
}

fn range_to_lsp(r: &wirescript::diagnostic::SourceRange) -> Range {
    Range {
        start: pos_to_lsp(r.start),
        end: pos_to_lsp(r.end),
    }
}

fn text_range_to_lsp(r: &TextRange) -> Range {
    Range {
        start: Position {
            line: r.start_line as u32,
            character: r.start_col as u32,
        },
        end: Position {
            line: r.end_line as u32,
            character: r.end_col as u32,
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
    field_name_at(ast, line, col)
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
    let d_ast = wirescript::parse(&d_source, &d_file).ast;

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
        let ast = wirescript::parse(&doc_state.source, &file).ast;
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
                let ast = wirescript::parse(&src, &file).ast;
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
            out.push(Location { uri: doc_uri.clone(), range: range_to_lsp(&r) });
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
                    out.push(Location { uri: entry_uri.clone(), range: range_to_lsp(&r) });
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
            start: Position { line: r.line as u32, character: r.start_col as u32 },
            end: Position { line: r.line as u32, character: r.end_col as u32 },
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
    doc_comments: wirescript::collections::HashMap<usize, String>,
    type_map: TypeMap,
    if_contexts: wirescript::analysis::IfContextMap,
    var_read_contexts: VarReadContextMap,
    resource_estimates: wirescript::collections::HashMap<String, ResourceEstimate>,
    pre_resolve_ast: Script,
    /// Canonical paths this doc imports, transitively — used to decide whether
    /// a change to another file can affect it.
    imported_files: Vec<String>,
}

struct Backend {
    client: Client,
    docs: Mutex<HashMap<Url, DocState>>,
}

/// A loader that serves imports from the OPEN EDITOR BUFFERS first, falling back
/// to disk. `analyze` previously resolved every import straight off disk, so an
/// unsaved edit in an imported file was invisible to the files importing it —
/// their diagnostics described the last SAVED version until you hit save. It
/// also skips a disk read per import per keystroke for files already in memory.
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
    /// `with_estimates` controls the resource estimates that feed hover. Those
    /// require REAL lowering of every chip/handler body (`collect_estimates` ->
    /// `compile_chip_template`), which is the one thing this server is built not
    /// to do per keystroke, so `did_change` passes `false` and reuses the
    /// previous doc's estimates; `did_open`/`did_save` recompute them. Hover
    /// numbers can therefore trail the buffer by an edit — they are approximate
    /// by nature, and the alternative is paying a lowering pass per character.
    fn analyze(&self, uri: &Url, source: &str, with_estimates: bool) -> Vec<Diagnostic> {
        let file = uri_to_file_string(uri);

        // Parse ONCE and hand the result to resolve — it used to re-parse the
        // same buffer internally, paying the entry parse twice per keystroke.
        // The pre-resolve AST (kept for local analysis: references, semantic
        // tokens, rename) is cloned off before resolve consumes the parse.
        let pre_resolve = wirescript::parse(source, &file);
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
        let resolved = resolve_parsed(pre_resolve, &file, &loader);
        let tc = typecheck_with_inference(&resolved.ast, &file).0;
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some(&file));
        let resource_estimates = if with_estimates {
            collect_estimates(&resolved.ast, &tc, &file)
        } else {
            // Carry the previous estimates forward rather than re-lowering.
            self.docs
                .lock()
                .ok()
                .and_then(|d| d.get(uri).map(|s| s.resource_estimates.clone()))
                .unwrap_or_default()
        };

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
                    resource_estimates,
                    pre_resolve_ast,
                    imported_files: resolved.imported_files.clone(),
                },
            );
        }

        let mut diags: Vec<Diagnostic> = resolved
            .diagnostics
            .iter()
            .chain(tc.diagnostics.iter())
            .filter(|d| &*d.range.file == file || d.range.file.is_empty())
            .map(|d| {
                let severity = match d.severity {
                    wirescript::diagnostic::Severity::Error => DiagnosticSeverity::ERROR,
                    wirescript::diagnostic::Severity::Warning => DiagnosticSeverity::WARNING,
                    _ => DiagnosticSeverity::INFORMATION,
                };
                Diagnostic {
                    range: range_to_lsp(&d.range),
                    severity: Some(severity),
                    code: Some(NumberOrString::String(d.code.clone())),
                    source: Some("wirescript".into()),
                    message: d.message.clone(),
                    ..Default::default()
                }
            })
            .collect();
        diags.extend(prefab_ref_diagnostics(source, &file));
        diags
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
    async fn lowering_diagnostics(&self, uri: &Url, source: &str) -> Vec<Diagnostic> {
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
            Err(_) => return Vec::new(),
        };

        let (diags, emit_error) = match result {
            Ok(r) => (r.diagnostics, None),
            Err(wirescript::CompileError::HasErrors(d)) => (d, None),
            Err(wirescript::CompileError::Emit(e)) => (Vec::new(), Some(format!("{e:?}"))),
        };

        let mut out: Vec<Diagnostic> = diags
            .iter()
            .filter(|d| &*d.range.file == file.as_str() || d.range.file.is_empty())
            .map(|d| Diagnostic {
                range: range_to_lsp(&d.range),
                severity: Some(match d.severity {
                    wirescript::diagnostic::Severity::Error => DiagnosticSeverity::ERROR,
                    wirescript::diagnostic::Severity::Warning => DiagnosticSeverity::WARNING,
                    _ => DiagnosticSeverity::INFORMATION,
                }),
                code: Some(NumberOrString::String(d.code.clone())),
                source: Some("wirescript".into()),
                message: d.message.clone(),
                ..Default::default()
            })
            .collect();

        // Emit failures carry no source range (they name a wire and a brick, not
        // a line). Surface one at the top of the file rather than dropping it —
        // it is the difference between a build that fails and a build that fails
        // for no visible reason.
        if let Some(msg) = emit_error {
            out.push(Diagnostic {
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
    /// affect — i.e. those importing it (transitively). Previously EVERY open
    /// document was re-analyzed on EVERY keystroke, so a keystroke cost
    /// (open tabs + 1) full analyses; an unrelated open file paid the whole
    /// bill for every character typed elsewhere.
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
            let diags = self.analyze(&uri, &source, /*with_estimates=*/ false);
            self.client.publish_diagnostics(uri, diags, None).await;
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
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
                        start: Position { line: r.line as u32, character: r.start_col as u32 },
                        end: Position { line: r.line as u32, character: r.end_col as u32 },
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
        self.client
            .log_message(MessageType::INFO, "wirescript LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let diags =
            self.analyze(&params.text_document.uri, &params.text_document.text, true);
        self.client
            .publish_diagnostics(params.text_document.uri, diags, None)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.first() {
            let diags = self.analyze(&uri, &change.text, /*with_estimates=*/ false);
            self.client
                .publish_diagnostics(uri.clone(), diags, None)
                .await;
        }
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
        let mut diags = self.analyze(&uri, &source, /*with_estimates=*/ true);
        for d in self.lowering_diagnostics(&uri, &source).await {
            // compile re-runs parse/typecheck, so its output overlaps analyze's.
            let dup = diags
                .iter()
                .any(|e| e.range == d.range && e.message == d.message);
            if !dup {
                diags.push(d);
            }
        }
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Ok(mut docs) = self.docs.lock() {
            docs.remove(&params.text_document.uri);
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let pos = params.text_document_position.position;
        let line = pos.line as usize;
        let col = pos.character as usize;
        let uri = &params.text_document_position.text_document.uri;

        let prefab_paths = scan_prefab_paths(uri);
        let items = match self.docs.lock() {
            Ok(docs) => match docs.get(uri) {
                Some(doc) => {
                    // Inside a `$```…``` ` nested-prefab block, complete against
                    // the INNER program so the outer file's context (SpawnPrefab
                    // params, outer symbols) doesn't leak into the isolated block.
                    if let Some((inner, il, ic)) =
                        nested_block_at(&doc.source, &uri_to_file_string(uri), line, col)
                    {
                        let resolved = resolve(&inner, "nested", &FsLoader);
                        let tc = typecheck_with_inference(&resolved.ast, "nested").0;
                        let syms = collect_symbols_for_file(
                            &resolved.ast,
                            &tc.type_of_expr,
                            Some("nested"),
                        );
                        build_completions(&inner, &syms, il, ic, &[])
                    } else {
                        build_completions(&doc.source, &doc.symbols, line, col, &prefab_paths)
                    }
                }
                None => build_completions("", &[], line, col, &prefab_paths),
            },
            Err(_) => build_completions("", &[], line, col, &prefab_paths),
        };
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        // "Fill record fields": inside a record literal whose expected type is a
        // record, offer to insert the missing fields with type-appropriate
        // defaults (recursing into nested records). Reuses the server's resolved
        // symbols, so nested / aliased / imported record types work.
        let uri = &params.text_document.uri;
        let pos = params.range.start;
        let (line, col) = (pos.line as usize, pos.character as usize);
        let fill = match self.docs.lock() {
            Ok(docs) => docs
                .get(uri)
                .and_then(|doc| fill_record_at(&doc.source, &doc.symbols, line, col)),
            Err(_) => None,
        };
        let Some(fill) = fill else {
            return Ok(None);
        };
        let at = Position {
            line: fill.line as u32,
            character: fill.col as u32,
        };
        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range { start: at, end: at },
                new_text: fill.text,
            }],
        );
        let action = CodeAction {
            title: "Fill record fields".into(),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        };
        Ok(Some(vec![CodeActionOrCommand::CodeAction(action)]))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                if let Some(value) = hover_at(
                    &doc.source,
                    &uri_to_file_string(uri),
                    &doc.symbols,
                    &doc.type_map,
                    &doc.doc_comments,
                    &doc.if_contexts,
                    &doc.var_read_contexts,
                    &doc.resource_estimates,
                    pos.line as usize,
                    pos.character as usize,
                ) {
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
        let col = pos.character as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
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
                if in_type_def && !field_name_at(&doc.pre_resolve_ast, line, col) {
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
                                    range: text_range_to_lsp(r),
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
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: target_uri,
                        range: Range {
                            start: Position {
                                line: loc.start_line as u32,
                                character: loc.start_col as u32,
                            },
                            end: Position {
                                line: loc.end_line as u32,
                                character: loc.end_col as u32,
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
        let col = pos.character as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
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
                            range: text_range_to_lsp(r),
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
        let col = pos.character as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let file = uri_to_file_string(uri);
                if let Some((range, placeholder)) =
                    prepare_rename_at(&doc.pre_resolve_ast, &doc.source, &file, line, col)
                {
                    return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                        range: range_to_lsp(&range),
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
        let col = pos.character as usize;

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
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
                            range: text_range_to_lsp(r),
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
            toks.push(Tok {
                line: range.start.line.saturating_sub(1),
                start_char: range.start.col.saturating_sub(1),
                length: range.end.col.saturating_sub(range.start.col),
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
                Box::new(move |p: wirescript::CompileProgress| {
                    let client = client.clone();
                    rt.spawn(async move {
                        client.send_notification::<CompileProgressNotification>(
                        serde_json::json!({ "step": p.step, "total": p.total, "done": p.done })
                    ).await;
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
                let items: Vec<serde_json::Value> = diags
                    .iter()
                    .map(|d| {
                        let severity = match d.severity {
                            wirescript::diagnostic::Severity::Error => "error",
                            wirescript::diagnostic::Severity::Warning => "warning",
                            _ => "info",
                        };
                        serde_json::json!({
                            "file": &*d.range.file,
                            "startLine": d.range.start.line.saturating_sub(1),
                            "startChar": d.range.start.col.saturating_sub(1),
                            "endLine": d.range.end.line.saturating_sub(1),
                            "endChar": d.range.end.col.saturating_sub(1),
                            "severity": severity,
                            "code": d.code,
                            "message": d.message,
                        })
                    })
                    .collect();
                return Ok(Some(serde_json::json!({ "ok": false, "diagnostics": items })));
            }
            // Emit / IO failures have no per-source location — keep them as a plain
            // error the extension can pop up.
            Err(e) => {
                return Err(tower_lsp::jsonrpc::Error {
                    code: tower_lsp::jsonrpc::ErrorCode::InvalidRequest,
                    message: e.to_string().into(),
                    data: None,
                });
            }
        };

        std::fs::write(out_path, &result.brz).map_err(|e| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InternalError,
            message: format!("write failed: {e}").into(),
            data: None,
        })?;

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
                            character: h.col as u32,
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
                let lines = doc.source.lines().count();
                let last_line = doc.source.lines().last().unwrap_or("");
                return Ok(Some(vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: lines as u32,
                            character: last_line.len() as u32,
                        },
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
    });
    Server::new(stdin, stdout, socket).serve(service).await;
}

/// Completions for `receiver.` — array methods, var fields, record fields, or
/// the receiver methods valid for a typed value. Returns only the members of the
/// receiver (possibly empty); it never falls through to the global
/// keyword/function list, so e.g. a `string` receiver shows only string methods.
fn member_completions(
    var_name: &str,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // `arr[i].` — an indexed read, not the array itself. Its members are the
    // array-get gate's outputs (the element `Value` and the `OutOfBounds`
    // flag), never the array's methods.
    if let Some(base) = var_name.strip_suffix("[]") {
        let sym = resolve_symbol(symbols, base, line, col);
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

    let sym = resolve_symbol(symbols, var_name, line, col);

    // Field name (record field / swizzle component) completion item.
    let field_item = |name: String| CompletionItem {
        label: name.clone(),
        kind: Some(CompletionItemKind::FIELD),
        detail: Some("field".to_string()),
        insert_text: Some(name),
        ..Default::default()
    };
    // Method + swizzle members valid for a typed value: swizzle fields, builtin
    // receiver-methods, then in-scope user `self`-mods whose receiver matches.
    let push_type_members = |ty: &str, items: &mut Vec<CompletionItem>| {
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

/// Byte offset of a zero-based `(line, col)` position in `source`. `col` is a
/// byte offset within the line (the convention the rest of this file uses),
/// clamped to the line's length.
fn line_col_to_offset(source: &str, line: usize, col: usize) -> usize {
    let mut off = 0usize;
    for (i, l) in source.split_inclusive('\n').enumerate() {
        if i == line {
            let line_len = l.len() - usize::from(l.ends_with('\n'));
            return off + col.min(line_len);
        }
        off += l.len();
    }
    source.len()
}

/// Inverse of [`line_col_to_offset`]: a byte `offset` back to a zero-based
/// `(line, col)`.
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let mut line = 0usize;
    let mut line_start = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if i >= clamped {
            break;
        }
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    (line, clamped - line_start)
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
    let cursor = line_col_to_offset(source, line, col);
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
        let col_idx = col.min(l.len());
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
                        character: (dollar + 1) as u32,
                    },
                    end: Position {
                        line: line as u32,
                        character: col as u32,
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
        let col_idx = col.min(l.len());
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
        return member_completions(&var_name, symbols, line, col);
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

    // Keywords.
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

    // Built-in calls / functions.
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

    // Types.
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

    items
}

#[cfg(test)]
mod tests;
