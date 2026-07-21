use std::collections::HashMap;
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use wirescript::{
    lower::{lower, LowerInput},
    resolve::{resolve, MemLoader},
    template_cache::TemplateCache,
    typecheck::typecheck,
    emit::{emit_brz, EmitOptions, PrefabResolver},
};

mod analysis;

fn make_loader(files_json: &str) -> MemLoader {
    let files: wirescript::collections::HashMap<String, String> =
        serde_json::from_str(files_json).unwrap_or_default();
    MemLoader { files }
}

/// Parse the dragged-in prefab registry: a JSON object mapping a prefab
/// reference path (`./turret.brz`) to the file's raw bytes as a JSON number
/// array. The sandbox builds this from files the user drags in.
fn parse_prefab_registry(prefabs_json: &str) -> HashMap<String, Vec<u8>> {
    serde_json::from_str(prefabs_json).unwrap_or_default()
}

/// A [`PrefabResolver`] backed by the in-memory drag registry.
fn registry_prefab_resolver(registry: HashMap<String, Vec<u8>>) -> PrefabResolver {
    PrefabResolver::new(move |path: &str| {
        registry
            .get(path)
            .cloned()
            .ok_or_else(|| format!("no dragged-in prefab registered for `{path}`"))
    })
}

// ---------- wirescript analysis (LSP-like, for browser IDE) ----------

#[wasm_bindgen]
pub fn wirescript_diagnostics(source: String, files_json: Option<String>) -> String {
    analysis::diagnostics(&source, files_json.as_deref().unwrap_or("{}"))
}

#[wasm_bindgen]
pub fn wirescript_completions(source: String, line: u32, col: u32, files_json: Option<String>, prefabs_json: Option<String>) -> String {
    let registry = parse_prefab_registry(prefabs_json.as_deref().unwrap_or("{}"));
    let prefab_paths: Vec<String> = {
        let mut p: Vec<String> = registry.into_keys().collect();
        p.sort();
        p
    };
    analysis::completions(&source, line, col, files_json.as_deref().unwrap_or("{}"), &prefab_paths)
}

#[wasm_bindgen]
pub fn wirescript_hover(source: String, line: u32, col: u32, files_json: Option<String>) -> String {
    analysis::hover(&source, line, col, files_json.as_deref().unwrap_or("{}")).unwrap_or_default()
}

#[wasm_bindgen]
pub fn wirescript_definition(source: String, line: u32, col: u32, files_json: Option<String>) -> String {
    analysis::definition_with_files(&source, line, col, files_json.as_deref().unwrap_or("{}")).unwrap_or_default()
}

#[wasm_bindgen]
pub fn wirescript_references(source: String, line: u32, col: u32, files_json: Option<String>) -> String {
    analysis::references_with_files(&source, line, col, files_json.as_deref().unwrap_or("{}")).unwrap_or_else(|| "[]".into())
}

#[wasm_bindgen]
pub fn wirescript_format(source: String, tab_size: u32, use_tabs: bool) -> String {
    analysis::format(&source, tab_size, use_tabs)
}

#[wasm_bindgen]
pub fn wirescript_workspace_symbols(files_json: String) -> String {
    analysis::workspace_symbols(&files_json)
}

#[wasm_bindgen]
pub fn wirescript_inlay_hints(source: String, files_json: Option<String>) -> String {
    analysis::inlay_hints(&source, files_json.as_deref().unwrap_or("{}"))
}

// ---------- wirescript compile ----------

#[wasm_bindgen]
pub fn wirescript_compile(source: String, module_name: Option<String>, files_json: Option<String>, prefabs_json: Option<String>) -> Result<Vec<u8>, JsValue> {
    let file = module_name.as_deref().unwrap_or("inline");
    let loader = make_loader(files_json.as_deref().unwrap_or("{}"));
    let resolved = resolve(&source, file, &loader);
    let tc = typecheck(&resolved.ast, file);
    let template_cache = Arc::new(TemplateCache::new());
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name: module_name.as_deref(),
        template_cache: template_cache.clone(),
        doc_comments: &resolved.doc_comments,
        fold_mode: wirescript::FoldMode::Auto,
    });

    let errors: Vec<String> = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .chain(lowered.diagnostics.iter())
        .filter(|d| matches!(d.severity, wirescript::diagnostic::Severity::Error))
        .map(|d| format!("[{}] {} ({}:{}:{})", d.code, d.message, d.range.file, d.range.start.line, d.range.start.col))
        .collect();

    if !errors.is_empty() {
        return Err(JsValue::from_str(&errors.join("\n")));
    }

    let lr = wirescript::layout::layout(&lowered.module);
    let registry = parse_prefab_registry(prefabs_json.as_deref().unwrap_or("{}"));
    let opts = EmitOptions {
        prefab_resolver: Some(registry_prefab_resolver(registry)),
        ..Default::default()
    };
    emit_brz(&lowered.module, &lr, &opts, &template_cache)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
