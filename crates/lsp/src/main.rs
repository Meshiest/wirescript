use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(not(target_family = "wasm"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use wirescript::analysis::text::Ruler;
use wirescript::analysis::{
    asset_ref_at, collect_estimates, collect_inlay_hints, collect_symbols_for_file, definition_at,
    cursor_byte_offset, field_name_at, fill_match_arms_at, fill_record_at, find_asset_refs,
    find_name_range, format_with, hover_at, prepare_rename_at, references_at, FormatOptions,
    references_to_export, rename_edit_text, semantic_tokens, word_at, AssetRef, CrossFile,
    InlayHintKind, RefNs, RefSite, RefTarget, ResourceEstimate, SemTokenKind, SymbolDef, TextRange,
    TypeMap, VarReadContextMap,
};
use wirescript::ast::{ImportKind, LetBinding, Script, TopDecl};
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
    wirescript::analysis::text::editor_col_to_char(source, line, character as usize)
}

/// [`wirescript::analysis::text::char_col_to_editor`] as an LSP position's
/// `character`.
fn char_col_to_lsp(ruler: &Ruler, line: usize, col: usize) -> u32 {
    ruler.char_col_to_editor(line, col) as u32
}

/// [`wirescript::analysis::text::byte_col_to_editor`] as an LSP position's
/// `character`.
fn byte_off_to_lsp(ruler: &Ruler, line: usize, byte: usize) -> u32 {
    ruler.byte_col_to_editor(line, byte) as u32
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

fn pos_to_lsp(ruler: &Ruler, p: wirescript::diagnostic::Pos) -> Position {
    let line = p.line.saturating_sub(1);
    Position {
        line: line as u32,
        // `Pos::col` is a 1-based byte column.
        character: byte_off_to_lsp(ruler, line as usize, p.col.saturating_sub(1) as usize),
    }
}

fn range_to_lsp(ruler: &Ruler, r: &wirescript::diagnostic::SourceRange) -> Range {
    Range {
        start: pos_to_lsp(ruler, r.start),
        end: pos_to_lsp(ruler, r.end),
    }
}

/// A [`TextRange`] carries 0-based BYTE columns, `ref_site_to_text_range`
/// builds it straight from a `SourceRange`, not the char columns the rest of
/// `analysis::` returns.
fn text_range_to_lsp(ruler: &Ruler, r: &TextRange) -> Range {
    Range {
        start: Position {
            line: r.start_line as u32,
            character: byte_off_to_lsp(ruler, r.start_line, r.start_col),
        },
        end: Position {
            line: r.end_line as u32,
            character: byte_off_to_lsp(ruler, r.end_line, r.end_col),
        },
    }
}

/// The editor range of each result, in order, reading each file (from disk
/// when it is not open) and indexing its lines once however many results it
/// holds.
fn ranges_in_files(docs: &HashMap<Url, DocState>, refs: &[(Url, TextRange)]) -> Vec<Range> {
    let mut sources: HashMap<&Url, std::borrow::Cow<str>> = HashMap::new();
    for (u, _) in refs {
        sources.entry(u).or_insert_with(|| source_for(docs, u));
    }
    let rulers: HashMap<&Url, Ruler> = sources.iter().map(|(u, s)| (*u, Ruler::new(s))).collect();
    refs.iter().map(|(u, r)| text_range_to_lsp(&rulers[u], r)).collect()
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

    for_each_unopened_sibling(docs, uri, d_canonical.as_deref(), |entry_uri, file, src| {
        let ast = wirescript::on_big_stack(|| wirescript::parse(&src, &file)).ast;
        for s in references_to_export(&ast, &file, &export_name, target.ns) {
            results.push((entry_uri.clone(), ref_site_to_text_range(&src, &export_name, &s)));
        }
    });

    sort_and_dedup(results)
}

/// A QUICKFIX code action that inserts `text` at a 0-based `line`/`col` in the
/// document, converting the char column the analysis layer reports into the
/// UTF-16 one the editor expects.
fn insert_quickfix(
    uri: &Url,
    source: &str,
    title: &str,
    line: usize,
    col: usize,
    text: String,
) -> CodeActionOrCommand {
    let at = Position {
        line: line as u32,
        character: char_col_to_lsp(&Ruler::new(source), line, col),
    };
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range { start: at, end: at },
            new_text: text,
        }],
    );
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.into(),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// Visit every `.ws` file beside `uri` that is not already open in the editor
/// (nor `skip`), handing `f` its URL, its path as a `file` string, and its
/// source read from disk.
///
/// Skipping is keyed on the CANONICAL path, never the `Url`: the client's URI
/// spelling (`file:///c%3A/…`) differs from `Url::from_file_path`'s
/// (`file:///C:/…`), so a Url-keyed skip re-added every open doc from disk.
/// References then showed each site twice, and rename emitted two identical
/// TextEdits per site - overlapping edits the editor refuses to apply, silently
/// leaving those files un-renamed.
fn for_each_unopened_sibling(
    docs: &HashMap<Url, DocState>,
    uri: &Url,
    skip: Option<&std::path::Path>,
    mut f: impl FnMut(Url, String, String),
) {
    let open_paths: std::collections::HashSet<std::path::PathBuf> = docs
        .keys()
        .filter_map(|u| u.to_file_path().ok())
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    let Ok(file_path) = uri.to_file_path() else {
        return;
    };
    let Some(dir) = file_path.parent() else {
        return;
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.extension().map_or(false, |e| e == "ws") {
            continue;
        }
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if open_paths.contains(&canonical) || skip == Some(canonical.as_path()) {
            continue;
        }
        let Ok(entry_uri) = Url::from_file_path(&path) else {
            continue;
        };
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        f(entry_uri, path.to_string_lossy().to_string(), src);
    }
}

/// Find-references for an atom `:name`: every `:name` occurrence in every open
/// document plus every `.ws` file in the referencing file's directory. Atoms
/// are global (a name hashes to one xxHash64 value, with no scope), so this is
/// a plain name match across the workspace — no resolution needed.
fn collect_atom_references(docs: &HashMap<Url, DocState>, uri: &Url, name: &str) -> Vec<Location> {
    let mut out: Vec<Location> = Vec::new();
    for (doc_uri, doc_state) in docs.iter() {
        let file = uri_to_file_string(doc_uri);
        let ruler = Ruler::new(&doc_state.source);
        for r in wirescript::analysis::atom_references(&doc_state.source, &file, name) {
            out.push(Location { uri: doc_uri.clone(), range: range_to_lsp(&ruler, &r) });
        }
    }
    for_each_unopened_sibling(docs, uri, None, |entry_uri, file, src| {
        let ruler = Ruler::new(&src);
        for r in wirescript::analysis::atom_references(&src, &file, name) {
            out.push(Location { uri: entry_uri.clone(), range: range_to_lsp(&ruler, &r) });
        }
    });
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

/// LSP diagnostics for `.brz` prefab file references with no file on disk.
fn prefab_ref_diagnostics(source: &str, file: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let ruler = Ruler::new(source);
    for r in find_asset_refs(source).into_iter().filter(AssetRef::is_file) {
        let range = Range {
            start: Position { line: r.line as u32, character: char_col_to_lsp(&ruler, r.line, r.start_col) },
            end: Position { line: r.line as u32, character: char_col_to_lsp(&ruler, r.line, r.end_col) },
        };
        // Typecheck reports a bad extension and a `.ws` source it cannot read
        // (WS019); a `.brz` is only read at compile time.
        if !r.path.ends_with(".brz") {
            continue;
        }
        let resolved = wirescript::compile::resolve_prefab_path(file, &r.path);
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

fn to_lsp_diagnostic(ruler: &Ruler, d: &wirescript::diagnostic::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: range_to_lsp(ruler, &d.range),
        severity: Some(match d.severity {
            wirescript::diagnostic::Severity::Error => DiagnosticSeverity::ERROR,
            wirescript::diagnostic::Severity::Warning => DiagnosticSeverity::WARNING,
            _ => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(d.code.clone())),
        code_description: wirescript::analysis::doc_url(wirescript::analysis::Doc::Diagnostic(&d.code))
            .and_then(|url| Url::parse(&url).ok())
            .map(|href| CodeDescription { href }),
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
    let ruler = Ruler::new(source);
    // Each imported file is read once, and its lines indexed once, after the
    // diagnostics naming it are gathered.
    let mut foreign_src: HashMap<Url, Option<String>> = HashMap::new();
    let mut foreign: Vec<(Url, &wirescript::diagnostic::Diagnostic)> = Vec::new();
    for d in diags {
        if &*d.range.file == file || d.range.file.is_empty() {
            split.own.push(to_lsp_diagnostic(&ruler, d));
            continue;
        }
        let Ok(other) = Url::from_file_path(&*d.range.file) else {
            continue;
        };
        if open_paths.contains(&FsLoader.canonical_path(&d.range.file, ".")) {
            continue;
        }
        foreign_src
            .entry(other.clone())
            .or_insert_with(|| std::fs::read_to_string(&*d.range.file).ok());
        foreign.push((other, d));
    }
    let rulers: HashMap<&Url, Ruler> =
        foreign_src.iter().filter_map(|(u, text)| Some((u, Ruler::new(text.as_deref()?)))).collect();
    for (other, d) in foreign {
        let Some(ruler) = rulers.get(&other) else { continue };
        split.foreign.entry(other).or_default().push(to_lsp_diagnostic(ruler, d));
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
    /// Per document, a count of its `did_change`s and the text of the newest.
    /// A debounced handler analyses only if its document's count is still the
    /// one it took when its wait ends, see [`DEBOUNCE`]; meanwhile an analysis
    /// of an importer reads the newest text rather than the last analysed.
    changes: Mutex<HashMap<Url, (u64, String)>>,
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
                                self.newest_text(u).unwrap_or_else(|| d.source.clone()),
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

        // `resolved.diagnostics` arrives already filtered; typecheck's are not,
        // and a `// ws-ignore-line:WS0xx` on a type-level warning has to hold on
        // this path too or the editor would keep underlining what the compiler
        // has been told to drop.
        let mut tc_diagnostics = tc.diagnostics;
        resolved.suppressions.apply(&mut tc_diagnostics);
        let mut split = match self.docs.lock() {
            Ok(docs) => split_by_file(
                resolved.diagnostics.iter().chain(tc_diagnostics.iter()),
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
                    }, &wirescript::FsLoader)
                })
                .await;
                (full.unwrap_or_default(), None)
            }
            // An emit failure is reported the same way, and for the same reason:
            // it says nothing about the type errors that are also in the file,
            // and returning it alone dropped every real diagnostic the user
            // could act on. `EmitError` carries a written message, so render it
            // rather than its `Debug` shape.
            Err(wirescript::CompileError::Emit(e)) => {
                let src_owned = source.to_string();
                let file_owned = file.clone();
                let full = tokio::task::spawn_blocking(move || {
                    wirescript::diagnostics_only(wirescript::CompileInput {
                        source: &src_owned,
                        file: &file_owned,
                        module_name: None,
                        fold_mode: FoldMode::Auto,
                    }, &wirescript::FsLoader)
                })
                .await;
                (full.unwrap_or_default(), Some(e.to_string()))
            }
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
    /// The text of `uri`'s newest change, which its own debounced handler
    /// may not have analysed yet.
    fn newest_text(&self, uri: &Url) -> Option<String> {
        self.changes.lock().ok()?.get(uri).map(|(_, text)| text.clone())
    }

    /// Drop what `did_change` recorded for `uri`, when it is opened afresh or
    /// closed. A handler still waiting then finds no count and returns.
    fn forget_changes(&self, uri: &Url) {
        if let Ok(mut changes) = self.changes.lock() {
            changes.remove(uri);
        }
    }

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
                .map(|(uri, doc)| (uri.clone(), self.newest_text(uri).unwrap_or_else(|| doc.source.clone())))
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
        // A client that brings its own formatter can opt out of server-side
        // formatting so the editor doesn't list two providers.
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
        let ruler = Ruler::new(&doc.source);
        // Clickable links for prefab file references that exist on disk.
        let links: Vec<DocumentLink> = find_asset_refs(&doc.source)
            .into_iter()
            .filter(AssetRef::is_file)
            .filter_map(|r| {
                let target = wirescript::compile::resolve_prefab_path(&file, &r.path);
                if !target.is_file() {
                    return None;
                }
                let target_uri = Url::from_file_path(&target).ok()?;
                Some(DocumentLink {
                    range: Range {
                        start: Position { line: r.line as u32, character: char_col_to_lsp(&ruler, r.line, r.start_col) },
                        end: Position { line: r.line as u32, character: char_col_to_lsp(&ruler, r.line, r.end_col) },
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
        self.forget_changes(&params.text_document.uri);
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
        // change to this document arrived meanwhile. A superseded handler
        // returns without touching the front end at all.
        let generation = match self.changes.lock() {
            Ok(mut changes) => {
                let entry = changes.entry(uri.clone()).or_default();
                entry.0 += 1;
                entry.1 = change.text.clone();
                entry.0
            }
            Err(_) => return,
        };
        tokio::time::sleep(DEBOUNCE).await;
        if self.changes.lock().map_or(true, |changes| changes.get(&uri).map(|c| c.0) != Some(generation)) {
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
        self.forget_changes(&uri);
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
        // The keystroke that triggered this request (`$`, `/`) is usually still
        // waiting out `did_change`'s debounce, so `doc.source` lacks it.
        let newest = self.newest_text(uri);
        let items = match self.docs.lock() {
            Ok(docs) => match docs.get(uri) {
                Some(doc) => {
                    let source = newest.as_deref().unwrap_or(&doc.source);
                    let col = lsp_col_to_char(source, line, pos.character);
                    // Inside a `$```…``` ` nested-prefab block, complete against
                    // the INNER program so the outer file's context (SpawnPrefab
                    // params, outer symbols) doesn't leak into the isolated block.
                    if let Some((inner, il, ic)) =
                        nested_block_at(source, &uri_to_file_string(uri), line, col)
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
                        build_completions(source, &doc.symbols, line, col, &prefab_paths)
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
            actions.push(insert_quickfix(
                uri, &source, "Fill record fields", fill.line, fill.col, fill.text,
            ));
        }
        if let Some(fill) = match_fill {
            actions.push(insert_quickfix(
                uri, &source, "Fill missing match arms", fill.line, fill.col, fill.text,
            ));
        }

        // "Ignore this warning": write the `// ws-ignore-line:CODE` (or
        // `// ws-ignore-file:CODE`) directive the compiler reads, rather than
        // leaving the author to remember the spelling. Offered per distinct
        // code under the cursor, and only for warnings. The compiler never
        // suppresses an error, so an action that inserted one would sit in the
        // file doing nothing with no way to tell.
        if !source.is_empty() {
            let mut seen: Vec<&String> = Vec::new();
            for d in &params.context.diagnostics {
                if matches!(d.severity, Some(DiagnosticSeverity::ERROR)) {
                    continue;
                }
                let Some(NumberOrString::String(code)) = &d.code else {
                    continue;
                };
                if seen.contains(&code) {
                    continue;
                }
                seen.push(code);
                let dline = d.range.start.line as usize;
                let line_text = source.lines().nth(dline).unwrap_or("");
                if !line_text.contains(&format!("ws-ignore-line:{code}")) {
                    actions.push(insert_quickfix(
                        uri,
                        &source,
                        &format!("Ignore {code} on this line"),
                        dline,
                        line_text.chars().count(),
                        format!(" // ws-ignore-line:{code}"),
                    ));
                }
                if !source.contains(&format!("ws-ignore-file:{code}")) {
                    actions.push(insert_quickfix(
                        uri,
                        &source,
                        &format!("Ignore {code} in this file"),
                        0,
                        0,
                        format!("// ws-ignore-file:{code}
"),
                    ));
                }
            }
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
                // `$./file.brz` prefab reference -> jump to the referenced file.
                if let Some(r) = asset_ref_at(&doc.source, line, col) {
                    if r.is_file() {
                        let target = wirescript::compile::resolve_prefab_path(&uri_to_file_string(uri), &r.path);
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

                // Type field -> show all references. A cursor on a record FIELD
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
                                .zip(ranges_in_files(&docs, &refs))
                                .map(|((u, _), range)| Location { uri: u.clone(), range })
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
                    let target_src = Ruler::new(&target_src);
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
                        .zip(ranges_in_files(&docs, &refs))
                        .map(|((u, _), range)| Location { uri: u.clone(), range })
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
                        range: range_to_lsp(&Ruler::new(&doc.source), &range),
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
                    for ((file_uri, r), range) in refs.iter().zip(ranges_in_files(&docs, &refs)) {
                        changes.entry(file_uri.clone()).or_default().push(TextEdit {
                            range,
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
        // One index for the whole pass: this loop converts two columns per
        // token, and the scanning conversion is O(offset) each time.
        let ruler = Ruler::new(&doc.source);
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
            let start_char = byte_off_to_lsp(
                &ruler,
                line as usize,
                range.start.col.saturating_sub(1) as usize,
            );
            let length = if range.start.line == range.end.line {
                byte_off_to_lsp(
                    &ruler,
                    line as usize,
                    range.end.col.saturating_sub(1) as usize,
                )
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
        // Without an output path the save comes back in the response instead
        // (base64): a WASI server sees only the mounted workspace, not the
        // editor's temp folder.
        let out_path = params
            .arguments
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if uri_str.is_empty() {
            return Err(tower_lsp::jsonrpc::Error::invalid_params(
                "expected [uri, outputPath?]",
            ));
        }
        let destination = if out_path.is_empty() { "the editor" } else { out_path };

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
                format!("[compile] start {file} -> {destination}"),
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
                            format!("[compile]   {} {} {}", d.location(), d.code, d.message),
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

        if !out_path.is_empty() {
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
                    format!("[compile]   {} {} {}", d.location(), d.code, d.message),
                )
                .await;
        }
        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "[compile] ok -> {destination} ({} bytes, {} warning{}, {}ms)",
                    result.brz.len(),
                    warnings,
                    if warnings == 1 { "" } else { "s" },
                    started.elapsed().as_millis()
                ),
            )
            .await;

        if out_path.is_empty() {
            return Ok(Some(serde_json::json!({ "ok": true, "brz": base64(&result.brz) })));
        }
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
                let ruler = Ruler::new(&doc.source);
                let lsp_hints: Vec<InlayHint> = hints
                    .into_iter()
                    .map(|h| InlayHint {
                        position: Position {
                            line: h.line as u32,
                            character: char_col_to_lsp(&ruler, h.line, h.col),
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
        // The editor's indentation, then whatever `.editorconfig` sets for this
        // file (indentation, print width, line endings, final newline).
        let opts = FormatOptions::default()
            .indent_with(!params.options.insert_spaces, params.options.tab_size as usize)
            .with_editorconfig(std::path::Path::new(&uri_to_file_string(uri)));

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                let formatted = format_with(&doc.source, &opts);
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

/// Stack for the runtime's threads. Every request handler runs inside the
/// `serve` future and walks user ASTs recursively (clone, drop, hover,
/// references), so a deep program must not outgrow it. Native reserves the
/// address space and commits pages as touched. The WASI build's main thread
/// gets the same size from `build.rs`.
const HANDLER_STACK: usize = 64 * 1024 * 1024;

fn main() {
    #[cfg(not(target_family = "wasm"))]
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    #[cfg(target_family = "wasm")]
    let mut builder = tokio::runtime::Builder::new_current_thread();
    let rt = builder
        .enable_all()
        .thread_stack_size(HANDLER_STACK)
        .build()
        .expect("failed to build the tokio runtime");
    // `block_on` polls its future on the calling thread, and a native main
    // thread's stack is 1 MiB on Windows, so the server goes to a worker.
    #[cfg(not(target_family = "wasm"))]
    rt.block_on(async { tokio::spawn(serve()).await.expect("language server task panicked") });
    #[cfg(target_family = "wasm")]
    rt.block_on(serve());
    // `serve` returns on the client's `exit`. Dropping the runtime would wait
    // for the stdin reader, blocked in a read that never returns, so the
    // process ends here. A WASI host never kills the server itself.
    std::process::exit(0);
}

async fn serve() {
    #[cfg(not(target_family = "wasm"))]
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());
    #[cfg(target_family = "wasm")]
    let (stdin, stdout) = (wasi_stdio::stdin(), wasi_stdio::Stdout);

    let (service, socket) = LspService::new(|client| Backend {
        client,
        docs: Mutex::new(HashMap::new()),
        foreign_diags: Mutex::new(HashMap::new()),
        watch_files: std::sync::atomic::AtomicBool::new(false),
        changes: Mutex::new(HashMap::new()),
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
    let rulers: HashMap<&str, Ruler> = sources.iter().map(|(f, s)| (f.as_str(), Ruler::new(s))).collect();
    let none = Ruler::new("");
    diags
        .iter()
        .map(|d| {
            let severity = match d.severity {
                wirescript::diagnostic::Severity::Error => "error",
                wirescript::diagnostic::Severity::Warning => "warning",
                _ => "info",
            };
            let text = rulers.get(&*d.range.file).unwrap_or(&none);
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
                "docUrl": wirescript::analysis::doc_url(wirescript::analysis::Doc::Diagnostic(&d.code)),
                "message": d.message,
            })
        })
        .collect()
}

/// Standard base64 (RFC 4648, padded), for returning a save in a JSON response.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

/// The completions at a position, as LSP items. Pure (no document lock /
/// async) so it can be unit-tested.
fn build_completions(
    source: &str,
    symbols: &[SymbolDef],
    line: usize,
    col: usize,
    prefab_paths: &[String],
) -> Vec<CompletionItem> {
    let ruler = Ruler::new(source);
    wirescript::analysis::completions(source, symbols, line, col, prefab_paths, &wirescript::typecheck::enums::EnumRegistry::editor_lookup)
        .into_iter()
        .map(|c| completion_item(&ruler, line, col, c))
        .collect()
}

/// `c` as an LSP item, its replaced span in UTF-16 columns.
fn completion_item(ruler: &Ruler, line: usize, col: usize, c: wirescript::analysis::Completion) -> CompletionItem {
    use wirescript::analysis::CompletionKind as K;
    let kind = match c.kind {
        K::Text => CompletionItemKind::TEXT,
        K::Method => CompletionItemKind::METHOD,
        K::Function => CompletionItemKind::FUNCTION,
        K::Field => CompletionItemKind::FIELD,
        K::Variable => CompletionItemKind::VARIABLE,
        K::Class => CompletionItemKind::CLASS,
        K::Module => CompletionItemKind::MODULE,
        K::Property => CompletionItemKind::PROPERTY,
        K::Keyword => CompletionItemKind::KEYWORD,
        K::Constant => CompletionItemKind::CONSTANT,
        K::EnumMember => CompletionItemKind::ENUM_MEMBER,
        K::Event => CompletionItemKind::EVENT,
        K::File => CompletionItemKind::FILE,
    };
    let text_edit = c.replace_from.map(|from| {
        let at = |character| Position { line: line as u32, character };
        CompletionTextEdit::Edit(TextEdit {
            range: Range { start: at(byte_off_to_lsp(ruler, line, from)), end: at(char_col_to_lsp(ruler, line, col)) },
            new_text: c.label.clone(),
        })
    });
    CompletionItem {
        label: c.label,
        kind: Some(kind),
        detail: c.detail,
        documentation: c.documentation.map(Documentation::String),
        insert_text: c.insert_text,
        filter_text: c.filter_text,
        text_edit,
        ..Default::default()
    }
}

#[cfg(test)]
use wirescript::analysis::match_arm_head_scrutinee_at;

#[cfg(target_family = "wasm")]
mod wasi_stdio;

#[cfg(test)]
mod tests;
