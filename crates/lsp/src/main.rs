use std::collections::HashMap;
use std::sync::Mutex;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use wirescript::analysis::{
    asset_ref_at, collect_estimates, collect_inlay_hints, collect_symbols_for_file, definition_at,
    collection_kind, find_all_references, find_asset_refs, find_enclosing_call, find_name_range,
    format_wirescript, hover_at, member_receiver_at, named_arg_value, param_names, receiver_methods,
    record_field_names, rename_edit_text, resolve_symbol, swizzle_fields, type_str,
    user_receiver_methods, word_at, AssetRef, CollectionKind, InlayHintKind, ResourceEstimate,
    SymbolDef, TextRange, TypeMap, VarReadContextMap,
};
use wirescript::ast::Script;
use wirescript::catalog::arrays::ARRAY_METHODS;
use wirescript::catalog::maps::MAP_METHODS;
use wirescript::catalog::calls::calls;
use wirescript::catalog::events::events;
use wirescript::lexer::KEYWORDS;
use wirescript::resolve::{resolve, FsLoader};
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

fn collect_references_across_files(
    docs: &HashMap<Url, DocState>,
    uri: &Url,
    word: &str,
) -> Vec<(Url, TextRange)> {
    let mut results = Vec::new();

    for (doc_uri, doc_state) in docs.iter() {
        for r in find_all_references(&doc_state.source, word) {
            results.push((doc_uri.clone(), r));
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
                let entry_uri = match Url::from_file_path(&path) {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                let src = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                for r in find_all_references(&src, word) {
                    results.push((entry_uri.clone(), r));
                }
            }
        }
    }

    // Deterministic order + belt-and-braces dedup: rename must never hand the
    // client two edits for the same site.
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

fn uri_to_file_string(uri: &Url) -> String {
    uri.to_file_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| uri.path().to_string())
}

/// Candidate prefab-reference strings for `$./…brz` completion: every `.brz`
/// file under the document's directory, as `./relative/path.brz` (forward
/// slashes, the wirescript reference form). Bounded depth so large trees don't
/// stall completion.
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
            } else if path.extension().is_some_and(|e| e == "brz") {
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
/// `.brz` on disk, or a ref without the required `.brz` extension.
fn prefab_ref_diagnostics(source: &str, file: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for r in find_asset_refs(source).into_iter().filter(AssetRef::is_file) {
        let range = Range {
            start: Position { line: r.line as u32, character: r.start_col as u32 },
            end: Position { line: r.line as u32, character: r.end_col as u32 },
        };
        if !r.path.ends_with(".brz") {
            out.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String("prefab-ext".into())),
                source: Some("wirescript".into()),
                message: format!("prefab reference `${}` must end in `.brz`", r.path),
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
}

struct Backend {
    client: Client,
    docs: Mutex<HashMap<Url, DocState>>,
}

impl Backend {
    fn analyze(&self, uri: &Url, source: &str) -> Vec<Diagnostic> {
        let file = uri_to_file_string(uri);

        let pre_resolve = wirescript::parse(source, &file);
        let resolved = resolve(source, &file, &FsLoader);
        let tc = typecheck_with_inference(&resolved.ast, &file).0;
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some(&file));
        let resource_estimates = collect_estimates(&resolved.ast, &tc, &file);

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
                    pre_resolve_ast: pre_resolve.ast,
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

    async fn reanalyze_other_docs(&self, changed_uri: &Url) {
        let others: Vec<(Url, String)> = {
            let docs = match self.docs.lock() {
                Ok(d) => d,
                Err(_) => return,
            };
            docs.iter()
                .filter(|(uri, _)| *uri != changed_uri)
                .map(|(uri, doc)| (uri.clone(), doc.source.clone()))
                .collect()
        };
        for (uri, source) in others {
            let diags = self.analyze(&uri, &source);
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
        let diags = self.analyze(&params.text_document.uri, &params.text_document.text);
        self.client
            .publish_diagnostics(params.text_document.uri, diags, None)
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.first() {
            let diags = self.analyze(&uri, &change.text);
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
        let mut diags = self.analyze(&uri, &source);
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

                // Type field → show all references
                let in_type_def = doc.symbols.iter().any(|s| {
                    s.kind == "type"
                        && s.range.start.line.saturating_sub(1) as usize <= line
                        && s.range.end.line.saturating_sub(1) as usize >= line
                });
                if in_type_def {
                    if let Some(word) = word_at(&doc.source, line, col) {
                        let refs = collect_references_across_files(&docs, uri, &word);
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

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                if let Some(word) = word_at(&doc.source, pos.line as usize, pos.character as usize)
                {
                    let refs = collect_references_across_files(&docs, uri, &word);
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
                if let Some(word) = word_at(&doc.source, line, col) {
                    if calls().contains_key(word.as_str())
                        || KEYWORDS.contains(&word.as_str())
                        || events().contains_key(word.as_str())
                    {
                        return Ok(None);
                    }
                    // The word's own range on this line (text-based).
                    let word_range = || {
                        let l = doc.source.lines().nth(line).unwrap_or("");
                        let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
                        let ws = l[..c]
                            .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        let we = l[c..]
                            .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
                            .map(|i| c + i)
                            .unwrap_or(l.len());
                        Range {
                            start: Position {
                                line: line as u32,
                                character: ws as u32,
                            },
                            end: Position {
                                line: line as u32,
                                character: we as u32,
                            },
                        }
                    };
                    // Type fields: use text-based range
                    let in_type = doc.symbols.iter().any(|s| {
                        s.kind == "type"
                            && s.range.start.line.saturating_sub(1) as usize <= line
                            && s.range.end.line.saturating_sub(1) as usize >= line
                    });
                    if in_type {
                        return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                            range: word_range(),
                            placeholder: word,
                        }));
                    }
                    // Symbols: use name range within declaration
                    for sym in &doc.symbols {
                        if sym.name == word {
                            let name_range = find_name_range(&doc.source, &sym.range, &sym.name)
                                .map(|r| range_to_lsp(&r))
                                .unwrap_or_else(|| range_to_lsp(&sym.range));
                            return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                                range: name_range,
                                placeholder: word,
                            }));
                        }
                    }
                    // Anything else find-references can see — a namespace-
                    // qualified member (`u.foo`), a record field in a literal,
                    // a shorthand binding — is renameable too. Refusing here
                    // blocked rename from exactly the sites references finds.
                    return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                        range: word_range(),
                        placeholder: word,
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

        if let Ok(docs) = self.docs.lock() {
            if let Some(doc) = docs.get(uri) {
                if let Some(word) = word_at(&doc.source, pos.line as usize, pos.character as usize)
                {
                    let refs = collect_references_across_files(&docs, uri, &word);

                    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
                    for (file_uri, r) in &refs {
                        changes.entry(file_uri.clone()).or_default().push(TextEdit {
                            range: text_range_to_lsp(r),
                            new_text: rename_edit_text(r, &word, new_name),
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
