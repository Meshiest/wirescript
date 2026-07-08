use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use wirescript::analysis::{
    asset_ref_at, collect_estimates, collect_inlay_hints, collect_symbols_for_file, definition_at,
    find_all_references, find_asset_refs, find_enclosing_call, find_name_range, format_wirescript,
    hover_at, named_arg_value, receiver_methods, type_str, word_at, AssetRef, InlayHintKind,
    ResourceEstimate, SymbolDef, TextRange, TypeMap, VarReadContextMap,
};
use wirescript::ast::Script;
use wirescript::catalog::arrays::ARRAY_METHODS;
use wirescript::catalog::calls::calls;
use wirescript::catalog::events::events;
use wirescript::lexer::KEYWORDS;
use wirescript::resolve::{resolve, FsLoader};
use wirescript::typecheck::typecheck;

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

    if let Ok(file_path) = uri.to_file_path() {
        if let Some(dir) = file_path.parent() {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if !path.extension().map_or(false, |e| e == "ws") {
                    continue;
                }
                let entry_uri = match Url::from_file_path(&path) {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                if docs.contains_key(&entry_uri) {
                    continue;
                }
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
    doc_comments: HashMap<usize, String>,
    type_map: TypeMap,
    if_contexts: wirescript::analysis::IfContextMap,
    var_read_contexts: VarReadContextMap,
    resource_estimates: HashMap<String, ResourceEstimate>,
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
        let tc = typecheck(&resolved.ast, &file);
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
                    build_completions(&doc.source, &doc.symbols, line, col, &prefab_paths)
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
                    if calls().contains_key(word.as_str()) || KEYWORDS.contains(&word.as_str()) {
                        return Ok(None);
                    }
                    // Type fields: use text-based range
                    let in_type = doc.symbols.iter().any(|s| {
                        s.kind == "type"
                            && s.range.start.line.saturating_sub(1) as usize <= line
                            && s.range.end.line.saturating_sub(1) as usize >= line
                    });
                    if in_type {
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
                        return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                            range: Range {
                                start: Position {
                                    line: line as u32,
                                    character: ws as u32,
                                },
                                end: Position {
                                    line: line as u32,
                                    character: we as u32,
                                },
                            },
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
                        let new_text = if r.is_shorthand {
                            format!("{}: {}", new_name, word)
                        } else {
                            new_name.clone()
                        };
                        changes.entry(file_uri.clone()).or_default().push(TextEdit {
                            range: text_range_to_lsp(r),
                            new_text,
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
        let compile_result = tokio::task::spawn_blocking(move || {
            let progress_cb: wirescript::ProgressCallback =
                Box::new(move |p: wirescript::CompileProgress| {
                    let client = client.clone();
                    tokio::spawn(async move {
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
                },
                wirescript::EmitOptions::default(),
                progress_cb,
            )
        })
        .await
        .unwrap();

        self.client
            .send_notification::<CompileProgressNotification>(
                serde_json::json!({ "step": 0, "total": 0, "done": true }),
            )
            .await;

        let result = compile_result.map_err(|e| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InvalidRequest,
            message: e.to_string().into(),
            data: None,
        })?;

        std::fs::write(out_path, &result.brz).map_err(|e| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InternalError,
            message: format!("write failed: {e}").into(),
            data: None,
        })?;

        Ok(Some(serde_json::json!({ "path": out_path })))
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

/// Completions for `receiver.` — array methods, var fields, or the receiver
/// methods valid for a typed value. Returns only the members of the receiver
/// (possibly empty); it never falls through to the global keyword/function
/// list, so e.g. a `string` receiver shows only string methods.
fn member_completions(var_name: &str, symbols: &[SymbolDef]) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let sym = symbols.iter().find(|s| s.name == var_name);

    // Arrays — declared with `array` or any array-typed value (e.g. a
    // `var ids: string[]`). All methods come from the canonical table.
    let is_array = sym.is_some_and(|s| {
        s.kind == "array" || s.ty.as_deref().is_some_and(|t| t.ends_with("[]"))
    });
    if is_array {
        for m in ARRAY_METHODS {
            items.push(CompletionItem {
                label: m.name.to_string(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(format!("{}{}", m.name, m.signature)),
                documentation: Some(Documentation::String(m.doc.to_string())),
                ..Default::default()
            });
        }
        return items;
    }

    if sym.is_some_and(|s| s.kind == "var") {
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
        return items;
    }

    // Receiver methods for a typed value (e.g. string methods on a string),
    // via the shared analysis helper used by every editor frontend.
    if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
        for (name, sig) in receiver_methods(ty) {
            items.push(CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::METHOD),
                detail: Some(sig),
                ..Default::default()
            });
        }
    }

    items
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
                    for name in wirescript::analysis::asset_names(&frag[..slash]) {
                        items.push(CompletionItem {
                            label: name.to_string(),
                            kind: Some(CompletionItemKind::CONSTANT),
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

    // Named params inside a function call: `Call(<here>)`.
    if let Some(call_name) = find_enclosing_call(source, line, col) {
        if let Some(spec) = calls().get(call_name.as_str()) {
            // Enum-valued named arg (e.g. `justify = "Center"`): complete the
            // enum's variant names when the cursor is in the value slot.
            if let Some((param_name, value_so_far)) = named_arg_value(source, line, col) {
                if let Some(param) = spec.params.iter().find(|p| p.name == param_name) {
                    if let Some(values) =
                        wirescript::field_enum_values(spec.gate_class, param.port.as_str())
                    {
                        let quoted = !value_so_far.contains('"');
                        for v in values {
                            let insert = if quoted { format!("\"{v}\"") } else { v.clone() };
                            items.push(CompletionItem {
                                label: v,
                                kind: Some(CompletionItemKind::ENUM_MEMBER),
                                detail: Some(format!("{param_name} value")),
                                insert_text: Some(insert),
                                ..Default::default()
                            });
                        }
                        if !items.is_empty() {
                            return items;
                        }
                    }
                }
            }
            for p in &spec.params {
                if p.optional {
                    items.push(CompletionItem {
                        label: format!("{} = ", p.name),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(type_str(&p.ty)),
                        insert_text: Some(format!("{} = ", p.name)),
                        ..Default::default()
                    });
                }
            }
            if !items.is_empty() {
                return items;
            }
        }
    }

    // Member access `receiver.` — return only the receiver's members.
    if let Some(l) = source.lines().nth(line) {
        let col_idx = col.min(l.len());
        if col_idx > 0 {
            if let Some(dot_pos) = l[..col_idx].rfind('.') {
                let prefix = l[..dot_pos].trim_end();
                let var_start = prefix
                    .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let var_name = &prefix[var_start..];
                if !var_name.is_empty() {
                    return member_completions(var_name, symbols);
                }
            }
        }
    }

    // User symbols.
    for sym in symbols {
        let kind = match sym.kind {
            "var" | "buffer" | "array" => CompletionItemKind::VARIABLE,
            "fn" | "mod" | "chip" => CompletionItemKind::FUNCTION,
            "in" => CompletionItemKind::FIELD,
            "let" => CompletionItemKind::CONSTANT,
            "event" => CompletionItemKind::EVENT,
            _ => CompletionItemKind::TEXT,
        };
        items.push(CompletionItem {
            label: sym.name.clone(),
            kind: Some(kind),
            detail: sym.ty.clone(),
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
mod tests {
    use super::*;

    fn symbols_for(source: &str) -> Vec<SymbolDef> {
        let resolved = resolve(source, "test", &FsLoader);
        let tc = typecheck(&resolved.ast, "test");
        collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"))
    }

    fn labels(source: &str, line: usize, col: usize) -> Vec<String> {
        let syms = symbols_for(source);
        build_completions(source, &syms, line, col, &[])
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    fn labels_with_prefabs(
        source: &str,
        line: usize,
        col: usize,
        prefabs: &[String],
    ) -> Vec<String> {
        let syms = symbols_for(source);
        build_completions(source, &syms, line, col, prefabs)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn prefab_ref_completes_from_candidate_paths() {
        let prefabs = vec![
            "./turret.brz".to_string(),
            "./enemies/tank.brz".to_string(),
            "./notes.txt".to_string(), // not a candidate; excluded by the scan
        ];
        // `SpawnPrefab(prefab = $./t` → offers the `./t…` prefab paths.
        let src = "on x { SpawnPrefab(prefab = $./t) }";
        let col = src.find("$./t").unwrap() + "$./t".len();
        let got = labels_with_prefabs(src, 0, col, &prefabs);
        assert!(got.contains(&"./turret.brz".to_string()), "got: {got:?}");
        assert!(
            !got.contains(&"./enemies/tank.brz".to_string()),
            "`./t` shouldn't match `./enemies/…`: {got:?}"
        );
    }

    #[test]
    fn completion_includes_builtin_events() {
        let ls = labels("", 0, 0);
        assert!(ls.iter().any(|l| l == "ChatCommand"), "ChatCommand missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "RoundStart"), "RoundStart missing");
        assert!(ls.iter().any(|l| l == "CharacterSpawned"), "CharacterSpawned missing");
        // ...and regular functions are still there.
        assert!(ls.iter().any(|l| l == "GetAim"), "GetAim function missing");
    }

    #[test]
    fn asset_ref_completes_types_then_names() {
        // After `$` → asset types.
        let src = "let w = $";
        let ls = labels(src, 0, 9);
        assert!(ls.iter().any(|l| l == "BRItemBase"), "asset type missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "BrickAudioDescriptor"), "asset type missing");
        // It's an isolated context — keywords/functions must not leak in.
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked into $ completion");

        // After `$BRItemBase/` → asset names of that type.
        let src2 = "let w = $BRItemBase/";
        let ls2 = labels(src2, 0, 20);
        assert!(ls2.iter().any(|l| l == "Weapon_Pistol"), "asset name missing: {ls2:?}");
        assert!(!ls2.iter().any(|l| l == "BRItemBase"), "type leaked into name completion");
    }

    #[test]
    fn string_dot_shows_only_string_methods() {
        let src = "let foo = \"\"\nfoo.";
        let ls = labels(src, 1, 4); // cursor right after `foo.`
        // String methods are offered.
        assert!(ls.iter().any(|l| l == "Length"), "Length missing: {ls:?}");
        assert!(ls.iter().any(|l| l == "Contains"), "Contains missing: {ls:?}");
        // The global list must NOT leak into a member-access context.
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked into member completion");
        assert!(!ls.iter().any(|l| l == "int"), "type leaked into member completion");
        assert!(!ls.iter().any(|l| l == "GetAim"), "non-string fn leaked");
        assert!(!ls.iter().any(|l| l == "ChatCommand"), "event leaked into member completion");
    }

    #[test]
    fn array_dot_shows_full_method_set() {
        let src = "array xs: int[]\nxs.";
        let ls = labels(src, 1, 3);
        // The full method set is offered, not just push/pop/length.
        for m in ["push", "find", "sort", "insert", "append", "slice"] {
            assert!(ls.iter().any(|l| l == m), "array method {m} missing: {ls:?}");
        }
        assert!(!ls.iter().any(|l| l == "if"), "keyword leaked");
    }

    #[test]
    fn var_array_dot_shows_array_methods() {
        // `var ids: string[]` is an array-typed var — it must complete array
        // methods (`.push`, `.find`, ...), not the var Value/prev fields.
        let src = "var ids: string[]\nids.";
        let ls = labels(src, 1, 4);
        assert!(ls.iter().any(|l| l == "push"), "push missing on var array: {ls:?}");
        assert!(ls.iter().any(|l| l == "find"), "find missing on var array: {ls:?}");
    }
}
