use std::collections::HashMap;
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use wirescript::{
    lower::{lower, LowerInput},
    resolve::{resolve, MemLoader},
    template_cache::TemplateCache,
    typecheck::typecheck_with_inference,
    emit::{emit_brz, EmitOptions, NestedCompiler, PrefabResolver},
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

/// Maximum `$```…``` nesting depth before compilation refuses (runaway guard);
/// mirrors the native CLI's limit in `compile.rs`.
const MAX_NESTED_DEPTH: usize = 8;

#[wasm_bindgen]
pub fn wirescript_compile(source: String, module_name: Option<String>, files_json: Option<String>, prefabs_json: Option<String>) -> Result<Vec<u8>, JsValue> {
    let file = module_name.as_deref().unwrap_or("inline");
    let files_json = files_json.unwrap_or_else(|| "{}".into());
    let registry = parse_prefab_registry(prefabs_json.as_deref().unwrap_or("{}"));
    compile_source_to_brz(&source, file, module_name.as_deref(), &files_json, registry, 0)
        .map_err(|e| JsValue::from_str(&e))
}

/// Compile a single Wirescript source to `.brz` bytes against the in-memory
/// loader (`files_json`) and dragged-in prefab registry, wiring a nested
/// compiler so inline `$```…``` blocks compile recursively — the browser
/// analog of the CLI's `default_nested_compiler`. `depth` is the current
/// nesting level; the top-level compile starts at 0.
fn compile_source_to_brz(
    source: &str,
    file: &str,
    module_name: Option<&str>,
    files_json: &str,
    registry: HashMap<String, Vec<u8>>,
    depth: usize,
) -> Result<Vec<u8>, String> {
    let loader = make_loader(files_json);
    let resolved = resolve(source, file, &loader);
    let (tc, ce_slots) = typecheck_with_inference(&resolved.ast, file);
    let template_cache = Arc::new(TemplateCache::new());
    let lowered = lower(LowerInput {
        ast: &resolved.ast,
        type_of_expr: &tc.type_of_expr,
        op_resolutions: &tc.op_resolutions,
        file,
        module_name,
        template_cache: template_cache.clone(),
        doc_comments: &resolved.doc_comments,
        fold_mode: wirescript::FoldMode::Auto,
        ce_slots: &ce_slots,
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
        return Err(errors.join("\n"));
    }

    let lopts = wirescript::layout_options_for(&resolved.ast, Some(resolved.source_map.clone()));
    let lr = wirescript::layout::layout_with_opts(&lowered.module, &lopts);
    let opts = EmitOptions {
        prefab_resolver: Some(registry_prefab_resolver(registry.clone())),
        nested_compiler: Some(wasm_nested_compiler(
            file.to_string(),
            files_json.to_string(),
            registry,
            depth + 1,
        )),
        ..Default::default()
    };
    emit_brz(&lowered.module, &lr, &opts, &template_cache).map_err(|e| e.to_string())
}

/// A [`NestedCompiler`] that recompiles an inline `$```…``` block's source at
/// `depth`, reusing the same in-memory loader + prefab registry so the inner
/// program can `import` and reference `$./…` prefabs exactly like the outer
/// one. Mirrors the CLI's `default_nested_compiler`: the emit layer's depth
/// argument is ignored (it always passes 1) in favor of the captured `depth`,
/// which is what accumulates across nesting levels.
fn wasm_nested_compiler(
    file: String,
    files_json: String,
    registry: HashMap<String, Vec<u8>>,
    depth: usize,
) -> NestedCompiler {
    NestedCompiler::new(move |inner_src: &str, _embed_depth: usize| {
        if depth > MAX_NESTED_DEPTH {
            return Err(format!(
                "nested prefab blocks are nested too deeply (limit {MAX_NESTED_DEPTH})"
            ));
        }
        compile_source_to_brz(inner_src, &file, None, &files_json, registry.clone(), depth)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An inline nested-prefab block (`$```…```) must compile in the browser
    /// build: the inner source is compiled to its own `.brz` and embedded.
    /// Without a nested compiler configured, this fails with "no nested
    /// compiler configured for this compile".
    #[test]
    fn nested_prefab_block_compiles_and_embeds() {
        let src = "in start: exec\non start { SpawnPrefab($```in q: exec\non q { }```) }\n";
        let brz = compile_source_to_brz(src, "test.ws", None, "{}", HashMap::new(), 0)
            .expect("nested-prefab compile should succeed in the wasm build");
        assert!(!brz.is_empty(), "emitted .brz is non-empty");
        // The output parses as a valid `.brz` archive (the embedded inner-block
        // prefab and its wiring are verified structurally by the native
        // `prefab_ref.rs` tests that share this emit path).
        assert!(
            brdb::Brz::read_slice(&brz).is_ok(),
            "emitted bytes are a valid .brz archive"
        );
    }

    /// A compile error inside the nested block surfaces as a compile error,
    /// proving the inner source is actually compiled (not silently embedded).
    #[test]
    fn broken_inner_block_surfaces_error() {
        let src = "in start: exec\non start { SpawnPrefab($```out y = zzz```) }\n";
        let err = compile_source_to_brz(src, "test.ws", None, "{}", HashMap::new(), 0)
            .expect_err("a broken inner block must fail the whole compile");
        assert!(!err.is_empty(), "error message should be non-empty");
    }

    /// Blocks nested past the runaway cap fail with a clear error, not a hang
    /// or stack overflow — the same guard the native CLI enforces.
    #[test]
    fn nested_prefab_depth_guard_trips() {
        let mut inner = "in q: exec".to_string();
        for _ in 0..(MAX_NESTED_DEPTH + 2) {
            inner = format!("in go: exec\non go {{ SpawnPrefab($```{inner}```) }}");
        }
        let src = format!("in go: exec\non go {{ SpawnPrefab($```{inner}```) }}\n");
        let msg = compile_source_to_brz(&src, "t.ws", None, "{}", HashMap::new(), 0)
            .expect_err("deeply nested prefab should fail")
            .to_lowercase();
        assert!(
            msg.contains("nest") || msg.contains("deep"),
            "depth-guard message: {msg}"
        );
    }
}
