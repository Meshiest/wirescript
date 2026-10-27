use super::*;

fn doc_state(source: &str) -> DocState {
    let pre_resolve = wirescript::parse(source, "test");
    let resolved = resolve(source, "test", &FsLoader);
    let tc = typecheck_with_inference(&resolved.ast).0;
    DocState {
        source: source.to_string(),
        symbols: Vec::new(),
        doc_comments: HashMap::default(),
        type_map: tc.type_of_expr,
        if_contexts: tc.if_contexts,
        var_read_contexts: tc.var_read_contexts,
        narrowed_reads: tc.narrowed_reads,
        dropped_ranges: tc.dropped_ranges,
        resource_estimates: HashMap::default(),
        pre_resolve_ast: pre_resolve.ast,
        imported_files: resolved.imported_files,
    }
}

#[test]
fn cross_file_references_skip_open_docs_even_with_respelled_uris() {
    // Clients spell file URIs differently from `Url::from_file_path`
    // (VS Code sends `file:///c%3A/...`); the same-directory disk scan must
    // still recognize an open doc as the same file and not re-collect it,
    // or rename gets two identical edits per site and the client refuses
    // to apply them.
    let dir = std::env::temp_dir().join(format!("ws-lsp-refs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.ws");
    let source = "mod foo() {\n}\nin go: exec\non go { foo() }\n";
    std::fs::write(&file, source).unwrap();

    let canonical_uri = Url::from_file_path(&file).unwrap();
    // Same file, percent-encoded spelling -> a different Url value.
    let respelled = canonical_uri.as_str().replacen("main.ws", "mai%6E.ws", 1);
    let client_uri = Url::parse(&respelled).unwrap();
    assert_ne!(client_uri, canonical_uri, "respelling must differ as a Url");
    assert_eq!(
        client_uri.to_file_path().unwrap(),
        canonical_uri.to_file_path().unwrap(),
        "…but point at the same file"
    );

    let mut docs = HashMap::new();
    docs.insert(client_uri.clone(), doc_state(source));

    // Cursor on the `foo` call site (line 3, `on go { foo() }`).
    let ds = &docs[&client_uri];
    let (target, current_sites) =
        references_at(&ds.pre_resolve_ast, &ds.source, "test", 3, 9).expect("target");
    let refs = collect_references_across_files(&docs, &client_uri, &target, &current_sites);
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        refs.len(),
        2,
        "each site once (def + call), not doubled by the disk scan: {refs:?}"
    );
    assert!(
        refs.iter().all(|(u, _)| u == &client_uri),
        "sites must be reported under the open doc's URI: {refs:?}"
    );
}

fn symbols_for(source: &str) -> Vec<SymbolDef> {
    let resolved = resolve(source, "test", &FsLoader);
    let tc = typecheck_with_inference(&resolved.ast).0;
    collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("test"))
}

#[test]
fn completion_inside_nested_block_isolates_from_outer_context() {
    // Cursor inside a `$```...``` ` block: the outer `SpawnPrefab` parameter
    // names (`velocity`, `offset`, `lifetime`) stay out, since the block is
    // completed as its own program.
    let src = "on go { let e = SpawnPrefab(lifetime = 0, offset = 1, $```in a: exec\non a { v }```) }\nin go: exec";
    let cursor = src.find("on a { v").unwrap() + "on a { ".len();
    let (line, col) = offset_to_line_col(src, cursor);
    let block = nested_block_at(src, "t", line, col);
    assert!(
        block.is_some(),
        "cursor inside the block should be detected"
    );
    let (inner, il, ic) = block.unwrap();
    assert!(
        inner.contains("in a: exec"),
        "inner source captured: {inner:?}"
    );
    let syms = symbols_for(&inner);
    let got: Vec<String> = build_completions(&inner, &syms, il, ic, &[])
        .into_iter()
        .map(|i| i.label)
        .collect();
    assert!(
        !got.iter()
            .any(|l| l == "velocity" || l == "offset" || l == "lifetime"),
        "SpawnPrefab params must not leak into the nested block: {got:?}"
    );
}

#[test]
fn completion_outside_nested_block_is_not_isolated() {
    // A cursor that is NOT inside a nested block returns None (the normal
    // outer-file completion path runs).
    let src = "on go { SpawnPrefab(prefab = $./a.brz) }\nin go: exec";
    let col = src.find("SpawnPrefab").unwrap() + 3;
    assert!(nested_block_at(src, "t", 0, col).is_none());
}

/// The labels completed at the `‸` in `src`.
fn labels_at_cursor(src: &str) -> Vec<String> {
    let (before, _) = src.split_once('‸').expect("the source marks the cursor");
    let line = before.matches('\n').count();
    let col = before.rsplit('\n').next().unwrap_or("").chars().count();
    let source = src.replacen('‸', "", 1);
    build_completions(&source, &symbols_for(&source), line, col, &[])
        .into_iter()
        .map(|i| i.label)
        .collect()
}

/// The completions that read an enum by the completed text go through the
/// editor's enum lookup, which only the server's build passes. Each row:
/// every `has` label is offered and no `lacks` one.
#[test]
fn the_editor_enum_lookup_completes_variants_and_discriminants() {
    let rows: &[(&str, &str, &[&str], &[&str])] = &[
        // The arm-head position inside `match s {  }` (s: Shape) offers
        // the enum's variant names as candidate patterns.
        (
            "a match arm head offers the variants",
            "enum Shape { Empty, Circle(float), Rect(float, float) }\nin s: Shape\nout x = match s { ‸ }\n",
            &["Circle", "Empty", "Rect"],
            &[],
        ),
        // A block-bodied arm may omit its trailing comma, so the line
        // after `Empty => { 1 }` is still an arm head.
        (
            "a match arm head after a block arm offers the variants",
            "enum Shape { Empty, Circle(float) }\nin s: Shape\nout x = match s {\n  Empty => { 1.0 }\n  ‸\n}\n",
            &["Circle", "Empty"],
            &[],
        ),
        (
            "an enum-typed value offers .Discriminant",
            "enum S { A, B }\nin s: S\nout d = s.‸\n",
            &["Discriminant"],
            &[],
        ),
        // `Shape.` (the enum TYPE name, not a value) offers its variant
        // names, matching the compiler's own `Enum.Variant` construction
        // syntax rather than a value's `.Discriminant`.
        (
            "a bare enum type receiver offers its variants",
            "enum Shape { Empty, Circle(float) }\nout d = Shape.‸\n",
            &["Circle", "Empty"],
            &["Discriminant"],
        ),
        // A value binding (`Shape: int` param) whose name equals the
        // enum's SHADOWS the type, so `Shape.` here is an (invalid) field
        // access on an int, never variant construction. Mirrors the
        // compiler's own shadow guard (`resolve_variant_for_construction`).
        (
            "a shadowed enum name offers no variants",
            "enum Shape { Empty, Circle(float) }\nmod useShape(Shape: int) -> int { return Shape.‸ }\n",
            &[],
            &["Empty", "Circle"],
        ),
        // `EasingFunction.` (no `enum` decl anywhere in the file) offers
        // its variant names, same as a user-declared enum.
        (
            "a bare built-in game enum receiver offers its variants",
            "out d = EasingFunction.‸\n",
            &["Bounce"],
            &[],
        ),
        // `EasingFunction.Bounce.` offers `.Discriminant`. `member_receiver_at`
        // only reports the identifier directly before the dot (`Bounce`),
        // so this only works if a bare variant name resolves to its
        // uniquely owning enum.
        (
            "a qualified built-in game enum variant offers .Discriminant",
            "out d = EasingFunction.Bounce.‸\n",
            &["Discriminant"],
            &[],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, has, lacks) in rows {
        let got = labels_at_cursor(src);
        let missing: Vec<&&str> = has
            .iter()
            .filter(|h| !got.iter().any(|l| l == *h))
            .collect();
        let leaked: Vec<&&str> = lacks
            .iter()
            .filter(|h| got.iter().any(|l| l == *h))
            .collect();
        if !missing.is_empty() || !leaked.is_empty() {
            failures.push(format!(
                "{label}: missing {missing:?}, leaked {leaked:?}, got {got:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}

// ---------- end-to-end LSP rename / references (scoped resolver) ----------

/// A rename's edits as document-change operations. The server always answers
/// in the `Operations` form, which carries a per-file version so an editor can
/// refuse a stale multi-file rename instead of applying half of it.
fn rename_operations(edit: WorkspaceEdit) -> Vec<DocumentChangeOperation> {
    match edit.document_changes.expect("document_changes present") {
        DocumentChanges::Operations(ops) => ops,
        DocumentChanges::Edits(_) => panic!("expected Operations, got flat Edits"),
    }
}

/// A fresh `Backend` behind the real `LanguageServer` trait, so its rename,
/// references and prepare-rename handlers run as the editor calls them, with no
/// documents open. `.inner()` on the returned service gives `&Backend`; the
/// handlers are driven in-process with no transport.
fn build_backend() -> LspService<Backend> {
    LspService::new(Backend::new).0
}

/// The real `rename` handler's edit for renaming at `(line, col)` to
/// `new_name`.
async fn rename_at(
    service: &LspService<Backend>,
    uri: &Url,
    line: u32,
    col: u32,
    new_name: &str,
) -> WorkspaceEdit {
    service
        .inner()
        .rename(RenameParams {
            text_document_position: text_document_position(uri, line, col),
            new_name: new_name.to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .unwrap()
        .expect("rename should produce edits")
}

/// The real `references` handler's answer at `(line, col)`, declaration
/// included.
async fn references_req(
    service: &LspService<Backend>,
    uri: &Url,
    line: u32,
    col: u32,
) -> Option<Vec<Location>> {
    service
        .inner()
        .references(ReferenceParams {
            text_document_position: text_document_position(uri, line, col),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: true,
            },
        })
        .await
        .unwrap()
}

/// Insert a document into a running `Backend` the same shape `did_open`
/// would produce (built via the module's own `doc_state` helper above).
fn open_doc(service: &LspService<Backend>, uri: &Url, source: &str) {
    service
        .inner()
        .docs
        .lock()
        .unwrap()
        .insert(uri.clone(), doc_state(source));
}

/// [`doc_state`] with the symbol table filled in: `goto_definition` reads
/// `doc.symbols` (its `in_type_def` check and `definition_at` both do).
fn doc_state_with_symbols(source: &str) -> DocState {
    let mut ds = doc_state(source);
    ds.symbols = symbols_for(source);
    ds
}

fn text_document_position(uri: &Url, line: u32, character: u32) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        position: Position { line, character },
    }
}

/// Run the real `code_action` handler for a zero-width range at
/// `(line, col)` and return only the `CodeAction` variants (none of this
/// server's actions are bare `Command`s).
async fn code_action_at(
    service: &LspService<Backend>,
    uri: &Url,
    line: u32,
    col: u32,
) -> Vec<CodeAction> {
    let pos = Position {
        line,
        character: col,
    };
    let resp = service
        .inner()
        .code_action(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: pos,
                end: pos,
            },
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .unwrap()
        .unwrap_or_default();
    resp.into_iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) => Some(ca),
            CodeActionOrCommand::Command(_) => None,
        })
        .collect()
}

/// `code_action_at`, but with the diagnostics the editor would attach to
/// the request, since the "ignore this warning" actions are driven by them.
async fn code_action_with_diagnostic(
    service: &LspService<Backend>,
    uri: &Url,
    line: u32,
    code: &str,
    severity: DiagnosticSeverity,
) -> Vec<CodeAction> {
    let pos = Position { line, character: 0 };
    let resp = service
        .inner()
        .code_action(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: pos,
                end: pos,
            },
            context: CodeActionContext {
                diagnostics: vec![Diagnostic {
                    range: Range {
                        start: pos,
                        end: pos,
                    },
                    severity: Some(severity),
                    code: Some(NumberOrString::String(code.into())),
                    source: Some("wirescript".into()),
                    message: "test".into(),
                    ..Default::default()
                }],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .unwrap()
        .unwrap_or_default();
    resp.into_iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) => Some(ca),
            CodeActionOrCommand::Command(_) => None,
        })
        .collect()
}

/// The `new_text` of a code action's first `TextEdit`.
fn first_edit_text(action: &CodeAction) -> String {
    action
        .edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.values().next())
        .and_then(|edits| edits.first())
        .map(|e| e.new_text.clone())
        .expect("code action has a text edit")
}

/// A tempdir under a name unique to this test + the process, so parallel
/// test runs never collide.
fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ws-lsp-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The `$`/`/` that triggers a completion is still in `did_change`'s
/// debounce when the request arrives; completion must read that text, not
/// the last analysed one.
#[tokio::test]
async fn completion_reads_the_undebounced_keystroke() {
    let uri = Url::parse("file:///c:/t/main.ws").unwrap();
    let service = build_backend();
    let complete = |typed: &'static str| {
        let service = &service;
        let uri = uri.clone();
        async move {
            let analysed = "in b: brick\non go {\n  b.GetComponent()\n}\n";
            let newest = format!("in b: brick\non go {{\n  b.GetComponent({typed})\n}}\n");
            open_doc(service, &uri, analysed);
            service
                .inner()
                .changes
                .lock()
                .unwrap()
                .insert(uri.clone(), (1, newest));
            let col = format!("  b.GetComponent({typed}").len() as u32;
            let resp = service
                .inner()
                .completion(CompletionParams {
                    text_document_position: text_document_position(&uri, 2, col),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                    context: None,
                })
                .await
                .unwrap();
            match resp {
                Some(CompletionResponse::Array(items)) => items,
                other => panic!("unexpected response {other:?}"),
            }
        }
    };
    let names = complete("$Component/").await;
    assert!(
        names.iter().any(|i| i.label == "PointLight"),
        "{:?}",
        names.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
    assert!(
        names.iter().all(|i| i.filter_text.is_none()),
        "a fixed filter text hides every name once one more letter is typed"
    );
    let types = complete("$compon").await;
    assert!(
        types.iter().any(|i| i.label == "Component"),
        "{:?}",
        types.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn rename_capture_does_not_touch_comments_types_or_other_files() {
    // main.ws: a `character` capture (local to its handler body), a
    // `: character` type annotation and a `// character` comment; other.ws: an
    // unrelated `character`-typed `in`. Renaming the capture touches none of the
    // three: types are a separate namespace, comments are never AST sites, and
    // captures never cross files.
    let dir = scratch_dir("rename-capture");
    let main_path = dir.join("main.ws");
    let main_source = "in go: exec\n\
in target: character\n\
// character comment\n\
on CharacterSpawned() -> (character) {\n\
  character.DisplayText(\"hi\")\n\
}\n";
    std::fs::write(&main_path, main_source).unwrap();
    let other_path = dir.join("other.ws");
    std::fs::write(&other_path, "in character: character\n").unwrap();

    let main_uri = Url::from_file_path(&main_path).unwrap();
    let service = build_backend();
    open_doc(&service, &main_uri, main_source);

    // Cursor on the capture's BODY use (`  character.DisplayText(...)`).
    let line = 4u32;
    let col = main_source
        .lines()
        .nth(4)
        .unwrap()
        .find("character")
        .unwrap() as u32;

    let edit = rename_at(&service, &main_uri, line, col, "spawned").await;

    std::fs::remove_dir_all(&dir).ok();

    let changes = rename_operations(edit);
    assert_eq!(
        changes.len(),
        1,
        "only main.ws should be edited: {changes:?}"
    );
    let DocumentChangeOperation::Edit(te) = &changes[0] else {
        panic!("expected a TextDocumentEdit operation: {:?}", changes[0]);
    };
    assert_eq!(
        te.text_document.uri, main_uri,
        "edit landed in the wrong file"
    );
    assert_eq!(
        te.edits.len(),
        2,
        "exactly the capture's decl + its one body use, nothing from the \
             type annotation, comment, or other.ws: {:?}",
        te.edits
    );
    for e in &te.edits {
        let OneOf::Left(edit) = e else {
            panic!("expected a plain TextEdit, got an annotated one: {e:?}")
        };
        assert!(
            edit.range.start.line == 3 || edit.range.start.line == 4,
            "edit escaped the capture's own handler body: {edit:?}"
        );
    }
}

#[tokio::test]
async fn rename_exported_mod_updates_import_specifier_and_call_across_files() {
    // lib.ws defines an exported `mod`; main.ws imports and calls it. Renaming
    // from the call site (an `Imported` target) edits both files: lib.ws's
    // declaration, and main.ws's import specifier and call, although lib.ws is
    // not open (it is read from disk through the import).
    let dir = scratch_dir("rename-import");
    let lib_path = dir.join("lib.ws");
    std::fs::write(&lib_path, "mod helper() {\n}\n").unwrap();
    let main_path = dir.join("main.ws");
    let main_source = "import { helper } from \"lib\"\nin go: exec\non go { helper() }\n";
    std::fs::write(&main_path, main_source).unwrap();

    let main_uri = Url::from_file_path(&main_path).unwrap();
    let lib_uri = Url::from_file_path(&lib_path).unwrap();
    let service = build_backend();
    open_doc(&service, &main_uri, main_source);

    // Cursor on the `helper()` call in `on go { helper() }` (line 2).
    let line = 2u32;
    let col = main_source.lines().nth(2).unwrap().find("helper").unwrap() as u32;

    let edit = rename_at(&service, &main_uri, line, col, "assist").await;

    std::fs::remove_dir_all(&dir).ok();

    let changes = rename_operations(edit);
    let mut by_file: HashMap<Url, usize> = HashMap::new();
    for op in &changes {
        let DocumentChangeOperation::Edit(te) = op else {
            panic!("expected a TextDocumentEdit operation: {op:?}")
        };
        by_file.insert(te.text_document.uri.clone(), te.edits.len());
    }
    assert_eq!(
        by_file.get(&lib_uri).copied(),
        Some(1),
        "lib.ws's declaration must be renamed even though it was never opened: {by_file:?}"
    );
    assert_eq!(
        by_file.get(&main_uri).copied(),
        Some(2),
        "main.ws needs both the import specifier AND the call site renamed: {by_file:?}"
    );
}

#[tokio::test]
async fn find_references_is_scoped_not_a_name_string_search() {
    // The capture-rename fixture again: references on the `character` capture
    // return only its handler-body occurrences (the declaration and one use),
    // never the `: character` annotation, the comment or other.ws. `references()`
    // and `rename()` share `references_at`, so this guards both against a
    // same-name text search.
    let dir = scratch_dir("find-refs-scoped");
    let main_path = dir.join("main.ws");
    let main_source = "in go: exec\n\
in target: character\n\
// character comment\n\
on CharacterSpawned() -> (character) {\n\
  character.DisplayText(\"hi\")\n\
}\n";
    std::fs::write(&main_path, main_source).unwrap();
    let other_path = dir.join("other.ws");
    std::fs::write(&other_path, "in character: character\n").unwrap();

    let main_uri = Url::from_file_path(&main_path).unwrap();
    let service = build_backend();
    open_doc(&service, &main_uri, main_source);

    let line = 4u32;
    let col = main_source
        .lines()
        .nth(4)
        .unwrap()
        .find("character")
        .unwrap() as u32;

    let locations = references_req(&service, &main_uri, line, col)
        .await
        .expect("references should find sites");

    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        locations.len(),
        2,
        "exactly the capture's decl + its one body use: {locations:?}"
    );
    for loc in &locations {
        assert_eq!(
            loc.uri, main_uri,
            "reference escaped into other.ws: {loc:?}"
        );
        assert!(
            loc.range.start.line == 3 || loc.range.start.line == 4,
            "reference escaped the capture's own handler body: {loc:?}"
        );
    }
}

#[tokio::test]
async fn find_references_on_field_name_is_refused() {
    // A field-name cursor inside a coarse enclosing binding's decl range
    // (`var x`'s initializer `p.field`) is refused by `references`, not
    // resolved to that binding: the scoped_refs table
    // `non_binding_positions_refuse_rename`, through the LSP handler.
    let source = "type P = { field: int }\nin p: P\nvar x: int = p.field\n";
    let uri = Url::from_file_path(std::env::temp_dir().join("ws-lsp-find-refs-field.ws")).unwrap();
    let service = build_backend();
    open_doc(&service, &uri, source);

    // Cursor inside `field` of `p.field` (line 2).
    let col = source.lines().nth(2).unwrap().find("field").unwrap() as u32 + 1;

    let locations = references_req(&service, &uri, 2, col).await;

    assert!(
        locations.is_none() || locations.as_ref().unwrap().is_empty(),
        "field click must be refused, not resolve to the enclosing var's references: {locations:?}"
    );
}

#[tokio::test]
async fn rename_export_updates_multiline_aliased_import_specifier_not_whole_decl() {
    // A multi-line `import { ... }` specifier (an aliased one stays coarse) is
    // narrowed by a word-boundary search over every line of the site, not just
    // its first: a miss would replace the whole `import { ... } from "..."` span.
    // The decoy `helperX` on an earlier line starts with the target's name.
    let dir = scratch_dir("rename-multiline-import");
    let lib_path = dir.join("lib.ws");
    let lib_source = "mod helper() {\n}\n";
    std::fs::write(&lib_path, lib_source).unwrap();
    let main_path = dir.join("main.ws");
    let main_source =
        "import {\n  helperX,\n  helper as x\n} from \"lib\"\nin go: exec\non go { x() }\n";
    std::fs::write(&main_path, main_source).unwrap();

    let lib_uri = Url::from_file_path(&lib_path).unwrap();
    let main_uri = Url::from_file_path(&main_path).unwrap();
    let service = build_backend();
    open_doc(&service, &lib_uri, lib_source);
    open_doc(&service, &main_uri, main_source);

    // Cursor on `helper` in `mod helper` (lib.ws, line 0).
    let line = 0u32;
    let col = lib_source.lines().next().unwrap().find("helper").unwrap() as u32;

    let edit = rename_at(&service, &lib_uri, line, col, "assist").await;

    std::fs::remove_dir_all(&dir).ok();

    let changes = rename_operations(edit);
    let main_te = changes
        .iter()
        .find_map(|op| {
            let DocumentChangeOperation::Edit(te) = op else {
                panic!("expected a TextDocumentEdit operation: {op:?}")
            };
            (te.text_document.uri == main_uri).then_some(te)
        })
        .expect("main.ws must be edited");

    assert_eq!(
        main_te.edits.len(),
        1,
        "only the specifier — the `helper` half — should be edited: {:?}",
        main_te.edits
    );
    let OneOf::Left(edit) = &main_te.edits[0] else {
        panic!("expected a plain TextEdit: {:?}", main_te.edits[0])
    };
    assert_eq!(edit.new_text, "assist");
    assert_eq!(
        edit.range.start.line, 2,
        "the edit must land on line 2 (`  helper as x`), not the decoy or the decl line: {edit:?}"
    );
    assert_eq!(
        edit.range.start.character, 2,
        "must land on `helper`, not swallow the whole decl: {edit:?}"
    );
    assert_eq!(
        edit.range.end.character, 8,
        "must cover exactly the `helper` token: {edit:?}"
    );
}

#[tokio::test]
async fn rename_aliased_import_alias_is_file_local() {
    // `import { helper as assist } from "lib"` and `assist()`: the alias is a
    // file-local name, so renaming `assist` edits only main.ws (the alias token
    // and the call), never lib.ws's `helper` or the `helper` half of the specifier.
    let dir = scratch_dir("rename-alias-local");
    let lib_path = dir.join("lib.ws");
    std::fs::write(&lib_path, "mod helper() {\n}\n").unwrap();
    let main_path = dir.join("main.ws");
    let main_source = "import { helper as assist } from \"lib\"\nin go: exec\non go { assist() }\n";
    std::fs::write(&main_path, main_source).unwrap();

    let main_uri = Url::from_file_path(&main_path).unwrap();
    let lib_uri = Url::from_file_path(&lib_path).unwrap();
    let service = build_backend();
    open_doc(&service, &main_uri, main_source);

    // Cursor on the `assist()` call (line 2).
    let line = 2u32;
    let col = main_source.lines().nth(2).unwrap().find("assist").unwrap() as u32;

    let edit = rename_at(&service, &main_uri, line, col, "aid").await;

    std::fs::remove_dir_all(&dir).ok();

    let changes = rename_operations(edit);
    assert_eq!(changes.len(), 1, "only main.ws may be edited: {changes:?}");
    let DocumentChangeOperation::Edit(te) = &changes[0] else {
        panic!("expected a TextDocumentEdit operation: {:?}", changes[0]);
    };
    assert_eq!(
        te.text_document.uri, main_uri,
        "an aliased-import rename must stay in-file"
    );
    assert_ne!(te.text_document.uri, lib_uri, "lib.ws must be untouched");
    assert_eq!(
        te.edits.len(),
        2,
        "exactly the alias specifier token + the call site: {:?}",
        te.edits
    );
    for e in &te.edits {
        let OneOf::Left(edit) = e else {
            panic!("expected a plain TextEdit: {e:?}")
        };
        assert_eq!(edit.new_text, "aid", "each site renames to the new alias");
        // The specifier edit must land on the alias token, NOT the `helper`
        // half: on line 0 it starts at/after `import { helper as ` (col > 17).
        if edit.range.start.line == 0 {
            assert!(
                edit.range.start.character > 17,
                "specifier edit hit the `helper` half, not the alias: {edit:?}"
            );
        }
    }
}

#[tokio::test]
async fn rename_export_updates_aliased_importer_original_name() {
    // lib.ws `mod helper(){}`; other.ws `import { helper as x }` and `x()`.
    // Renaming `helper` from lib.ws renames its declaration and the `helper` half
    // of other.ws's specifier, and leaves the alias `x` and `x()` alone.
    let dir = scratch_dir("rename-export-aliased-importer");
    let lib_path = dir.join("lib.ws");
    let lib_source = "mod helper() {\n}\n";
    std::fs::write(&lib_path, lib_source).unwrap();
    let other_path = dir.join("other.ws");
    let other_source = "import { helper as x } from \"lib\"\nin go: exec\non go { x() }\n";
    std::fs::write(&other_path, other_source).unwrap();

    let lib_uri = Url::from_file_path(&lib_path).unwrap();
    let other_uri = Url::from_file_path(&other_path).unwrap();
    let service = build_backend();
    open_doc(&service, &lib_uri, lib_source);
    open_doc(&service, &other_uri, other_source);

    // Cursor on `helper` in `mod helper` (lib.ws, line 0).
    let line = 0u32;
    let col = lib_source.lines().next().unwrap().find("helper").unwrap() as u32;

    let edit = rename_at(&service, &lib_uri, line, col, "assist").await;

    std::fs::remove_dir_all(&dir).ok();

    let changes = rename_operations(edit);
    let mut by_file: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for op in &changes {
        let DocumentChangeOperation::Edit(te) = op else {
            panic!("expected a TextDocumentEdit operation: {op:?}")
        };
        let plain: Vec<TextEdit> = te
            .edits
            .iter()
            .map(|e| match e {
                OneOf::Left(t) => t.clone(),
                OneOf::Right(_) => panic!("expected a plain TextEdit"),
            })
            .collect();
        by_file.insert(te.text_document.uri.clone(), plain);
    }

    let lib_edits = by_file.get(&lib_uri).expect("lib.ws must be edited");
    assert_eq!(lib_edits.len(), 1, "lib.ws: just the decl: {lib_edits:?}");
    assert_eq!(lib_edits[0].new_text, "assist");

    let other_edits = by_file.get(&other_uri).expect("other.ws must be edited");
    assert_eq!(
        other_edits.len(),
        1,
        "other.ws: only the `helper` half of the specifier, never the alias or its call: {other_edits:?}"
    );
    let e = &other_edits[0];
    assert_eq!(e.new_text, "assist");
    assert_eq!(
        e.range.start.line, 0,
        "the specifier edit is on the import line"
    );
    // The `helper` token sits at cols 9..15 of `import { helper as x }`; the
    // alias `x` is at col 19. The edit must cover `helper`, not `x`.
    assert_eq!(
        e.range.start.character, 9,
        "edit must land on the `helper` token: {e:?}"
    );
    assert_eq!(
        e.range.end.character, 15,
        "edit must end at the `helper` token: {e:?}"
    );
}

#[tokio::test]
async fn goto_definition_on_record_type_field_routes_through_definition_at() {
    // A cursor on a record field name inside a `type` declaration does not
    // resolve to the type's reference set; it falls through to `definition_at`,
    // which has no target for a field. The assertion that matters is that the
    // response is not the type's references.
    let source = "type P = { field: int }\n";
    let uri = Url::from_file_path(std::env::temp_dir().join("ws-lsp-goto-field.ws")).unwrap();
    let service = build_backend();
    service
        .inner()
        .docs
        .lock()
        .unwrap()
        .insert(uri.clone(), doc_state_with_symbols(source));

    // Cursor on the record-type field name `field` (line 0).
    let col = source.find("field").unwrap() as u32;
    let resp = service
        .inner()
        .goto_definition(GotoDefinitionParams {
            text_document_position_params: text_document_position(&uri, 0, col),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .unwrap();

    assert!(
        resp.is_none(),
        "field click must route to definition_at, not the type's reference set: {resp:?}"
    );
}

#[tokio::test]
async fn semantic_tokens_tag_capture_as_variable_and_annotation_as_type() {
    // Semantic tokens over the position-blind grammar: a `character` handler
    // capture and its use are plain Variable tokens, while the `: character`
    // annotation stays a Type.
    let source = "\
in go: exec
in foo: character
on CharacterSpawned() -> (character) {
  character.DisplayText(\"hi\")
}";
    let uri = Url::from_file_path(std::env::temp_dir().join("ws-lsp-semtok.ws")).unwrap();
    let service = build_backend();
    open_doc(&service, &uri, source);

    let resp = service
        .inner()
        .semantic_tokens_full(SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .await
        .unwrap()
        .expect("semantic tokens should be produced");

    let SemanticTokensResult::Tokens(tokens) = resp else {
        panic!("expected a full SemanticTokens result, got a delta/partial variant");
    };

    // Decode the delta-encoded stream back to absolute (line, start_char,
    // length, token_type) tuples per the LSP spec's own encoding rule.
    let mut abs = Vec::new();
    let mut line = 0u32;
    let mut start = 0u32;
    for t in &tokens.data {
        line += t.delta_line;
        start = if t.delta_line == 0 {
            start + t.delta_start
        } else {
            t.delta_start
        };
        abs.push((line, start, t.length, t.token_type));
    }

    // Legend order (see the capability registration): Type=0,
    // Function=1, Parameter=2, Variable=3, Namespace=4.
    let decl_col = source.lines().nth(2).unwrap().find("character").unwrap() as u32;
    assert!(
        abs.iter()
            .any(|&(l, c, _, ty)| l == 2 && c == decl_col && ty == 3),
        "capture decl should tokenize as Variable: {abs:?}"
    );

    let type_col = source.lines().nth(1).unwrap().find("character").unwrap() as u32;
    assert!(
        abs.iter()
            .any(|&(l, c, _, ty)| l == 1 && c == type_col && ty == 0),
        ": character annotation should tokenize as Type: {abs:?}"
    );
}

/// An import resolves against the open editor buffer, not the file saved on
/// disk, so an importer's diagnostics follow unsaved edits.
#[test]
fn open_buffer_shadows_disk_for_imports() {
    let dir = std::env::temp_dir().join(format!("ws-lsp-openimp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lib = dir.join("lib.ws");
    // On DISK the export is named `OLD`.
    std::fs::write(&lib, "let OLD: int = 1\n").unwrap();

    // The unsaved editor buffer for that same file renames it to `FRESH`.
    let mut open = HashMap::new();
    open.insert(
        FsLoader.canonical_path(&lib.to_string_lossy(), "."),
        "let FRESH: int = 2\n".to_string(),
    );
    let loader = OpenDocLoader { open };

    let main_path = dir.join("main.ws");
    let r = resolve(
        "import { FRESH } from \"lib\"\nout o = FRESH",
        &main_path.to_string_lossy(),
        &loader,
    );
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        r.diagnostics.is_empty(),
        "the unsaved buffer's export must be visible, got {:?}",
        r.diagnostics
    );
}

/// The fallback still reaches disk for a file that is not open.
#[test]
fn unopened_import_still_loads_from_disk() {
    let dir = std::env::temp_dir().join(format!("ws-lsp-diskimp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.ws"), "let ONDISK: int = 1\n").unwrap();
    let loader = OpenDocLoader {
        open: HashMap::new(),
    };
    let main_path = dir.join("main.ws");
    let r = resolve(
        "import { ONDISK } from \"lib\"\nout o = ONDISK",
        &main_path.to_string_lossy(),
        &loader,
    );
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        r.diagnostics.is_empty(),
        "a closed import must still load from disk, got {:?}",
        r.diagnostics
    );
}

/// A mod added since the last save gets a gate estimate immediately:
/// estimates are recomputed on every change, since they are looked up by name.
#[tokio::test]
async fn a_mod_added_since_the_last_save_still_gets_a_gate_estimate() {
    let dir = scratch_dir("estimate-freshness");
    let file = dir.join("main.ws");
    let opened = "mod alpha(v: *int) { v = v + 1 }\n";
    std::fs::write(&file, opened).unwrap();
    let uri = Url::from_file_path(&file).unwrap();

    let service = build_backend();
    service
        .inner()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "wirescript".into(),
                version: 1,
                text: opened.into(),
            },
        })
        .await;

    // Type a second mod, as a change only, never saved to disk.
    let edited = "mod alpha(v: *int) { v = v + 1 }\nmod beta(v: *int) { v = v * 3 + 2 }\n";
    service
        .inner()
        .did_change(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: edited.into(),
            }],
        })
        .await;

    let docs = service.inner().docs.lock().unwrap();
    let est = &docs.get(&uri).expect("doc state").resource_estimates;
    assert!(
        est.contains_key("beta"),
        "a mod added by an unsaved edit must have an estimate; keys: {:?}",
        est.keys().collect::<Vec<_>>()
    );
    assert!(
        est.contains_key("alpha"),
        "the pre-existing mod must keep its estimate"
    );
}

/// An edit to one document must not cancel the pending analysis of
/// another. The debounce counter was shared by every document, so an edit
/// to `b.ws` within the wait after one to `a.ws` made `a.ws`'s handler
/// return without analysing, and its newest text got no diagnostics until
/// it was edited again.
#[tokio::test]
async fn an_edit_to_one_document_does_not_cancel_another_documents_analysis() {
    let dir = scratch_dir("debounce-per-document");
    let (a_file, b_file) = (dir.join("a.ws"), dir.join("b.ws"));
    let a_opened = "import \"b\"\nmod alpha(v: *int) { v = v + 1 }\n";
    let b_opened = "mod helper(v: *int) { v = v + 2 }\n";
    std::fs::write(&a_file, a_opened).unwrap();
    std::fs::write(&b_file, b_opened).unwrap();
    let (a, b) = (
        Url::from_file_path(&a_file).unwrap(),
        Url::from_file_path(&b_file).unwrap(),
    );

    let service = build_backend();
    for (uri, text) in [(&a, a_opened), (&b, b_opened)] {
        service
            .inner()
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "wirescript".into(),
                    version: 1,
                    text: text.into(),
                },
            })
            .await;
    }
    let change = |uri: &Url, text: &str| DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.into(),
        }],
    };
    let a_edited =
        "import \"b\"\nmod alpha(v: *int) { v = v + 1 }\nmod gamma(v: *int) { v = v * 3 + 2 }\n";
    let b_edited = "mod helper(v: *int) { v = v + 5 }\n";
    tokio::join!(service.inner().did_change(change(&a, a_edited)), async {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        service.inner().did_change(change(&b, b_edited)).await;
    });

    let docs = service.inner().docs.lock().unwrap();
    let a_doc = docs.get(&a).expect("a.ws is open");
    assert_eq!(
        a_doc.source, a_edited,
        "a.ws must be analysed at its last text"
    );
    assert!(
        a_doc.resource_estimates.contains_key("gamma"),
        "the mod a.ws's last edit added must be analysed"
    );
    assert_eq!(docs.get(&b).expect("b.ws is open").source, b_edited);
}

/// `if constexpr` semantics mean an untaken branch is never checked, so it
/// can silently rot. Making it visible in the editor is the compensation.
#[tokio::test]
async fn hovering_dropped_code_says_it_was_removed_at_compile_time() {
    let dir = scratch_dir("const-dropped");
    let file = dir.join("main.ws");
    let src = "const MODE = 1\nvar x: int = 0\nin go: exec\non go {\n  if MODE == 1 { x = 1 } else { x = 2 }\n}\n";
    std::fs::write(&file, src).unwrap();
    let uri = Url::from_file_path(&file).unwrap();
    let service = build_backend();
    service
        .inner()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "wirescript".into(),
                version: 1,
                text: src.into(),
            },
        })
        .await;

    // column inside the `x = 2` else-block body
    let col = src.lines().nth(4).unwrap().find("x = 2").unwrap() as u32 + 1;
    let h = service
        .inner()
        .hover(HoverParams {
            text_document_position_params: text_document_position(&uri, 4, col),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .unwrap()
        .expect("hover");
    let HoverContents::Markup(m) = h.contents else {
        panic!("expected markup")
    };
    assert!(
        m.value.contains("Removed at compile time"),
        "got {:?}",
        m.value
    );
}

// ---------- fill missing match arms (Task 22) ----------

/// A `match` covering only `Circle` is missing `Empty` and `Rect`; the
/// code action must offer both as witness arms, reusing the same
/// witness engine (`typecheck::patterns::analyze`) the compiler's own
/// WS054 exhaustiveness diagnostic runs. The placeholder body is a plain
/// `todo`, never LSP snippet syntax (`${...}`), which this client would
/// insert verbatim and fail to parse.
#[tokio::test]
async fn fill_missing_match_arms_offers_the_witnesses() {
    let src = "enum Shape { Empty, Circle(float), Rect(float, float) }\nin s: Shape\nout x = match s {\n  Circle(r) => 1.0,\n}\n";
    let uri = Url::parse("file:///t.ws").unwrap();
    let service = build_backend();
    open_doc(&service, &uri, src);

    // cursor on the `match` line
    let actions = code_action_at(&service, &uri, 2, 10).await;
    let fill = actions
        .iter()
        .find(|a| a.title.contains("Fill missing match arms"))
        .expect("action offered");
    let text = first_edit_text(fill);
    assert!(text.contains("Empty"), "missing Empty witness: {text:?}");
    assert!(text.contains("Rect("), "missing Rect witness: {text:?}");
    assert!(
        text.contains("todo"),
        "missing plain placeholder body: {text:?}"
    );
    assert!(
        !text.contains("${"),
        "must not emit LSP snippet syntax: {text:?}"
    );
}

/// A `match` already covering every variant offers no fill action; the
/// action must not appear when there's nothing to fill.
#[tokio::test]
async fn exhaustive_match_offers_no_fill_action() {
    let src = "enum Shape { Empty, Circle(float) }\nin s: Shape\nout x = match s {\n  Circle(r) => 1.0,\n  Empty => 0.0,\n}\n";
    let uri = Url::parse("file:///t2.ws").unwrap();
    let service = build_backend();
    open_doc(&service, &uri, src);

    let actions = code_action_at(&service, &uri, 2, 10).await;
    assert!(
        !actions
            .iter()
            .any(|a| a.title.contains("Fill missing match arms")),
        "an exhaustive match must not offer a fill action: {actions:?}"
    );
}

/// A diagnostic raised inside an imported file is published against that
/// file, not dropped from the importer's analysis.
#[tokio::test]
async fn imported_file_diagnostics_go_to_the_imported_file() {
    let dir = scratch_dir("import-diags");
    let util = dir.join("util.ws");
    // The non-ASCII string puts the error's byte column and its UTF-16
    // column one apart, so the assertion below also pins that the range
    // was converted against util.ws's own text and not the importer's.
    std::fs::write(
        &util,
        "mod helper() {\n  let x = \"h\u{e9}llo\" .. undefinedThing\n}\n",
    )
    .unwrap();
    let main = dir.join("main.ws");
    let main_source = "import \"./util.ws\"\nin go: exec\non go { helper() }\n";
    std::fs::write(&main, main_source).unwrap();

    let service = build_backend();
    let split = service
        .inner()
        .analyze(&Url::from_file_path(&main).unwrap(), main_source);
    let util_uri = Url::from_file_path(&util).unwrap();
    let for_util = split.foreign.get(&util_uri).cloned().unwrap_or_default();
    let own = split.own;
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        for_util
            .iter()
            .any(|d| d.message.contains("undefinedThing")),
        "the unknown-identifier error must be published against util.ws, got {for_util:?} / own {own:?}"
    );
    assert!(
        own.iter().all(|d| !d.message.contains("undefinedThing")),
        "and must not also be reported against main.ws: {own:?}"
    );
    let d = for_util
        .iter()
        .find(|d| d.message.contains("undefinedThing"))
        .unwrap();
    assert_eq!(
        (d.range.start.line, d.range.start.character),
        (1, 21),
        "line 2 of util.ws, at the UTF-16 column of `undefinedThing` in ITS text"
    );
}

/// The on-save lowering pass must reach lowering even when the entry file
/// has a type error, and its diagnostics must land on the imported file
/// they name. `compile` stops at the first stage that reports an error, so
/// a file with one never lowered at all and the whole WSP001 class went
/// missing from the editor while `wirescript-check` still reported it.
#[tokio::test]
async fn lowering_diagnostics_survive_a_type_error_and_reach_the_import() {
    let dir = scratch_dir("lowering-import-diags");
    let util = dir.join("util.ws");
    // A non-constant `var` initializer is dropped by lowering with a
    // WSP001 warning; nothing before lowering says a word about it.
    std::fs::write(&util, "in a: int\nvar x: int = a + 1\nout r = x\n").unwrap();
    let main = dir.join("main.ws");
    let main_source = "import \"./util.ws\"\nin go: exec\non go { let y = undefinedThing }\n";
    std::fs::write(&main, main_source).unwrap();
    let uri = Url::from_file_path(&main).unwrap();

    let service = build_backend();
    let mut split = service.inner().analyze(&uri, main_source);
    split.extend_deduped(
        service
            .inner()
            .lowering_diagnostics(&uri, main_source)
            .await,
    );
    let for_util = split
        .foreign
        .get(&Url::from_file_path(&util).unwrap())
        .cloned()
        .unwrap_or_default();
    let own = split.own.clone();
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        own.iter().any(|d| d.message.contains("undefinedThing")),
        "the entry file keeps its own error: {own:?}"
    );
    assert!(
        for_util
            .iter()
            .any(|d| d.code == Some(NumberOrString::String("WSP001".into()))),
        "util.ws's lowering warning must reach util.ws: {for_util:?}"
    );
}

/// The Compile command converts `Pos::col` (a byte column) for the editor, so
/// its squiggle on a line with non-ASCII text lands on the token and matches
/// the live diagnostics for the same file.
#[test]
fn compile_command_diagnostics_carry_utf16_columns() {
    let source = "let s = \"h\u{e9}llo\" .. undefined\n";
    // `undefined` starts at byte 20 of that line and UTF-16 column 19.
    let byte_col = source.find("undefined").unwrap();
    assert_eq!(byte_col, 20);
    let range = wirescript::diagnostic::SourceRange {
        file: "main.ws".into(),
        start: wirescript::diagnostic::Pos {
            line: 1,
            col: byte_col as u32 + 1,
            offset: byte_col,
        },
        end: wirescript::diagnostic::Pos {
            line: 1,
            col: byte_col as u32 + 10,
            offset: byte_col + 9,
        },
    };
    let diags = vec![wirescript::diagnostic::Diagnostic {
        code: "WS002".into(),
        message: "unknown identifier 'undefined'".into(),
        severity: wirescript::diagnostic::Severity::Error,
        range,
    }];
    let mut sources = HashMap::new();
    sources.insert("main.ws".to_string(), source.to_string());

    let items = compile_diagnostic_items(&diags, &sources);
    assert_eq!(items[0]["startChar"], 19);
    assert_eq!(items[0]["endChar"], 28);
}

/// A whole-document format edit must name a position the document actually
/// has. `str::lines` drops a trailing newline, so pairing its count with
/// the last line's length pointed one line past the end.
#[tokio::test]
async fn formatting_edit_ends_at_the_document_end() {
    let uri = Url::parse("file:///fmt.ws").unwrap();
    let service = build_backend();
    // Badly indented so the formatter actually returns an edit.
    open_doc(&service, &uri, "in go: exec\non go {\nlet v: int = 1\n}\n");

    let edits = service
        .inner()
        .formatting(DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: FormattingOptions {
                tab_size: 2,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .await
        .unwrap()
        .expect("the buffer is not formatted, so there is an edit");
    assert_eq!(
        (edits[0].range.end.line, edits[0].range.end.character),
        (4, 0),
        "four newlines, so the document ends at the start of line 4"
    );
}

#[tokio::test]
async fn code_action_offers_the_ws_ignore_directives_for_a_warning() {
    // The suppression comment has an exact spelling the compiler reads; the
    // quick fix is what saves the author from remembering it.
    let dir = scratch_dir("ws-ignore-action");
    let path = dir.join("main.ws");
    let source = "in go: exec
out r: int = 1
";
    std::fs::write(&path, source).unwrap();
    let uri = Url::from_file_path(&path).unwrap();
    let service = build_backend();
    open_doc(&service, &uri, source);

    let actions =
        code_action_with_diagnostic(&service, &uri, 0, "WS014", DiagnosticSeverity::WARNING).await;
    std::fs::remove_dir_all(&dir).ok();

    let line_fix = actions
        .iter()
        .find(|a| a.title == "Ignore WS014 on this line")
        .unwrap_or_else(|| panic!("expected a line-ignore action: {actions:?}"));
    assert_eq!(first_edit_text(line_fix), " // ws-ignore-line:WS014");

    let file_fix = actions
        .iter()
        .find(|a| a.title == "Ignore WS014 in this file")
        .unwrap_or_else(|| panic!("expected a file-ignore action: {actions:?}"));
    assert_eq!(
        first_edit_text(file_fix),
        "// ws-ignore-file:WS014
"
    );
}

#[tokio::test]
async fn code_action_does_not_offer_to_ignore_an_error() {
    // The compiler never suppresses an error, so offering the action would
    // hand the author an edit that silently does nothing.
    let dir = scratch_dir("ws-ignore-action-error");
    let path = dir.join("main.ws");
    let source = "in go: exec
out r: int = 1
";
    std::fs::write(&path, source).unwrap();
    let uri = Url::from_file_path(&path).unwrap();
    let service = build_backend();
    open_doc(&service, &uri, source);

    let actions =
        code_action_with_diagnostic(&service, &uri, 0, "WS002", DiagnosticSeverity::ERROR).await;
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        !actions.iter().any(|a| a.title.starts_with("Ignore ")),
        "an error must not offer a ws-ignore quick fix: {actions:?}"
    );
}

#[test]
fn base64_matches_rfc_4648_vectors() {
    for (raw, encoded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(raw.as_bytes()), encoded, "{raw:?}");
    }
    assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
}

/// A diagnostic whose code the docs cover links to that section; one they
/// don't has no link.
#[test]
fn diagnostics_link_to_their_docs() {
    let ruler = Ruler::new("x\n");
    let range = wirescript::diagnostic::SourceRange::default();
    let d = to_lsp_diagnostic(
        &ruler,
        &wirescript::diagnostic::Diagnostic::error("WS012", "m", range.clone()),
    );
    assert_eq!(
        d.code_description.map(|c| c.href.to_string()).as_deref(),
        Some("https://wirescript.brickadia.dev/docs/diagnostics.html#names-declarations--imports"),
    );
    let d = to_lsp_diagnostic(
        &ruler,
        &wirescript::diagnostic::Diagnostic::error("WS-EMIT", "m", range),
    );
    assert!(d.code_description.is_none());
}

/// Typing a program into an empty file, one character at a time, must not
/// panic any request the editor sends as you type. An empty document used
/// to take the server down from `code_action` (the record-fill scan
/// indexed line 0 of no lines).
#[tokio::test]
async fn typing_into_an_empty_file_never_panics_a_request() {
    let program = "type P = { x: int, y: int }\nin go: exec\nvar n: int = 0\n\
            mod bump(v: *int) { v += 1 }\nlet p: P = {\n  x: 1,\n}\non go {\n  bump(n)\n  \
            match n {\n    0 => { n = 1 }\n    _ => { n = 2 }\n  }\n  \
            BroadcastChatMessage(\"n ${n}\")\n}\nlet pf = $./a.brz\n";
    let uri = Url::parse("file:///c:/t/main.ws").unwrap();
    let service = build_backend();
    let backend = service.inner();
    let ends: Vec<usize> = std::iter::once(0)
        .chain(program.char_indices().map(|(i, c)| i + c.len_utf8()))
        .collect();
    for end in ends {
        let typed = &program[..end];
        backend
            .docs
            .lock()
            .unwrap()
            .insert(uri.clone(), doc_state_with_symbols(typed));
        let line = typed.matches('\n').count() as u32;
        let col = typed.rsplit('\n').next().unwrap_or("").chars().count() as u32;
        let at = || text_document_position(&uri, line, col);
        let doc = || TextDocumentIdentifier { uri: uri.clone() };
        let ctx = format!("after typing {typed:?}");
        backend
            .completion(CompletionParams {
                text_document_position: at(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: None,
            })
            .await
            .expect(&ctx);
        code_action_at(&service, &uri, line, col).await;
        backend
            .hover(HoverParams {
                text_document_position_params: at(),
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .expect(&ctx);
        backend
            .goto_definition(GotoDefinitionParams {
                text_document_position_params: at(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .expect(&ctx);
        backend
            .references(ReferenceParams {
                text_document_position: at(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: ReferenceContext {
                    include_declaration: true,
                },
            })
            .await
            .expect(&ctx);
        backend.prepare_rename(at()).await.expect(&ctx);
        backend
            .semantic_tokens_full(SemanticTokensParams {
                text_document: doc(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .expect(&ctx);
        backend
            .inlay_hint(InlayHintParams {
                text_document: doc(),
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line,
                        character: col,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .expect(&ctx);
        backend
            .formatting(DocumentFormattingParams {
                text_document: doc(),
                options: FormattingOptions {
                    tab_size: 2,
                    insert_spaces: true,
                    ..Default::default()
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .await
            .expect(&ctx);
        backend
            .document_link(DocumentLinkParams {
                text_document: doc(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .await
            .expect(&ctx);
    }
}

#[test]
fn every_completion_kind_maps_to_its_lsp_kind() {
    use wirescript::analysis::{Completion, CompletionKind as K};
    let want = [
        (K::Text, CompletionItemKind::TEXT),
        (K::Method, CompletionItemKind::METHOD),
        (K::Function, CompletionItemKind::FUNCTION),
        (K::Field, CompletionItemKind::FIELD),
        (K::Variable, CompletionItemKind::VARIABLE),
        (K::Class, CompletionItemKind::CLASS),
        (K::Module, CompletionItemKind::MODULE),
        (K::Property, CompletionItemKind::PROPERTY),
        (K::Keyword, CompletionItemKind::KEYWORD),
        (K::Constant, CompletionItemKind::CONSTANT),
        (K::EnumMember, CompletionItemKind::ENUM_MEMBER),
        (K::Event, CompletionItemKind::EVENT),
        (K::File, CompletionItemKind::FILE),
    ];
    let ruler = Ruler::new("x");
    for (kind, lsp) in want {
        let c = Completion {
            label: "x".into(),
            kind,
            ..Default::default()
        };
        assert_eq!(completion_item(&ruler, 0, 1, c).kind, Some(lsp), "{kind:?}");
    }
}

/// A completion that replaces text before the cursor becomes a text edit
/// spanning, in UTF-16 columns, from its byte start to the cursor.
#[test]
fn a_replacing_completion_is_a_text_edit_in_utf16_columns() {
    use wirescript::analysis::Completion;
    let ruler = Ruler::new("\u{1F389} $./a");
    let c = Completion {
        label: "$./abc.brz".into(),
        replace_from: Some(5),
        ..Default::default()
    };
    let Some(CompletionTextEdit::Edit(edit)) = completion_item(&ruler, 0, 6, c).text_edit else {
        panic!("expected a text edit");
    };
    assert_eq!(
        (edit.range.start.character, edit.range.end.character),
        (3, 7)
    );
    assert_eq!(edit.new_text, "$./abc.brz");
}
