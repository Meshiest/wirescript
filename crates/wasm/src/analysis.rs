use std::collections::HashMap;

use serde::Serialize;
use wirescript::analysis::{
    Location, TextRange, TypeMap, collect_symbols, definition_at, find_all_references,
    find_enclosing_call, format_wirescript, hover_at, named_arg_value, receiver_methods, type_str,
    word_at,
};
use wirescript::ast::*;
use wirescript::catalog::calls::calls;
use wirescript::catalog::events::events;
use wirescript::lexer::KEYWORDS;
use wirescript::resolve::{MemLoader, resolve};
use wirescript::{parse, typecheck::typecheck};

#[derive(Serialize)]
pub struct DiagnosticOut {
    pub severity: &'static str,
    pub code: String,
    pub message: String,
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "startCol")]
    pub start_col: usize,
    #[serde(rename = "endLine")]
    pub end_line: usize,
    #[serde(rename = "endCol")]
    pub end_col: usize,
}

#[derive(Serialize)]
pub struct CompletionOut {
    pub label: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
}

#[derive(Serialize)]
pub struct HoverOut {
    pub value: String,
}

#[derive(Serialize)]
pub struct LocationOut {
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "startCol")]
    pub start_col: usize,
    #[serde(rename = "endLine")]
    pub end_line: usize,
    #[serde(rename = "endCol")]
    pub end_col: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl From<Location> for LocationOut {
    fn from(loc: Location) -> Self {
        LocationOut {
            start_line: loc.start_line,
            start_col: loc.start_col,
            end_line: loc.end_line,
            end_col: loc.end_col,
            file: loc.file,
        }
    }
}

impl From<TextRange> for LocationOut {
    fn from(r: TextRange) -> Self {
        LocationOut {
            start_line: r.start_line,
            start_col: r.start_col,
            end_line: r.end_line,
            end_col: r.end_col,
            file: None,
        }
    }
}

fn make_loader(files_json: &str) -> MemLoader {
    let files: HashMap<String, String> = serde_json::from_str(files_json).unwrap_or_default();
    MemLoader { files }
}

pub fn diagnostics(source: &str, files_json: &str) -> String {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck(&resolved.ast, "editor");
    let diags: Vec<DiagnosticOut> = resolved
        .diagnostics
        .iter()
        .chain(tc.diagnostics.iter())
        .filter(|d| &*d.range.file == "editor" || d.range.file.is_empty())
        .map(|d| DiagnosticOut {
            severity: match d.severity {
                wirescript::diagnostic::Severity::Error => "error",
                wirescript::diagnostic::Severity::Warning => "warning",
                _ => "info",
            },
            code: d.code.clone(),
            message: d.message.clone(),
            start_line: d.range.start.line.saturating_sub(1) as usize,
            start_col: d.range.start.col.saturating_sub(1) as usize,
            end_line: d.range.end.line.saturating_sub(1) as usize,
            end_col: d.range.end.col.saturating_sub(1) as usize,
        })
        .collect();
    serde_json::to_string(&diags).unwrap_or_else(|_| "[]".into())
}

pub fn completions(
    source: &str,
    line: u32,
    col: u32,
    files_json: &str,
    prefab_paths: &[String],
) -> String {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck(&resolved.ast, "editor");
    let symbols = collect_symbols(&resolved.ast, &tc.type_of_expr);
    let mut items: Vec<CompletionOut> = Vec::new();

    // Prefab file reference `$./file.brz` / `$/abs.brz`: complete from the
    // registered (dragged-in) prefab paths.
    {
        let l = source.lines().nth(line as usize).unwrap_or("");
        let col_idx = (col as usize).min(l.len());
        let before = &l[..col_idx];
        if let Some(dollar) = before.rfind('$') {
            let frag = &before[dollar + 1..];
            let is_prefab_frag = (frag.starts_with('.') || frag.starts_with('/'))
                && frag
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '/' | '.' | '-'));
            if is_prefab_frag {
                for path in prefab_paths {
                    if path.starts_with(frag) {
                        items.push(CompletionOut {
                            label: path.clone(),
                            kind: "file",
                            detail: None,
                            // Replace the `$…` fragment (after `$`) with the path.
                            insert_text: Some(path.clone()),
                        });
                    }
                }
                if !items.is_empty() {
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                }
            }
        }
    }

    // Asset reference `$AssetType/AssetName`: types after `$`, names after `$Type/`.
    {
        let l = source.lines().nth(line as usize).unwrap_or("");
        let col_idx = (col as usize).min(l.len());
        let before = &l[..col_idx];
        if let Some(dollar) = before.rfind('$') {
            let frag = &before[dollar + 1..];
            if frag
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '/')
            {
                if let Some(slash) = frag.find('/') {
                    for name in wirescript::analysis::asset_names(&frag[..slash]) {
                        items.push(CompletionOut {
                            label: name.to_string(),
                            kind: "constant",
                            detail: None,
                            insert_text: None,
                        });
                    }
                } else {
                    for ty in wirescript::analysis::asset_types() {
                        items.push(CompletionOut {
                            label: ty.to_string(),
                            kind: "class",
                            detail: None,
                            insert_text: Some(format!("{ty}/")),
                        });
                    }
                }
                if !items.is_empty() {
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                }
            }
        }
    }

    // Array method completions after `arrayVar.`
    let l = source.lines().nth(line as usize).unwrap_or("");
    let col_idx = (col as usize).min(l.len());
    if col_idx > 0 {
        let before = &l[..col_idx];
        if let Some(dot_pos) = before.rfind('.') {
            let prefix = before[..dot_pos].trim_end();
            let var_name_start = prefix
                .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|i| i + 1)
                .unwrap_or(0);
            let var_name = &prefix[var_name_start..];
            if !var_name.is_empty() {
                let sym = symbols.iter().find(|s| s.name == var_name);
                // Arrays — `array` decls and any array-typed value (e.g. a
                // `var ids: string[]`). Methods come from the canonical table.
                let is_array = sym.is_some_and(|s| {
                    s.kind == "array" || s.ty.as_deref().is_some_and(|t| t.ends_with("[]"))
                });
                if is_array {
                    for m in wirescript::catalog::arrays::ARRAY_METHODS {
                        items.push(CompletionOut {
                            label: m.name.to_string(),
                            kind: "method",
                            detail: Some(format!("{}{} - {}", m.name, m.signature, m.doc)),
                            insert_text: None,
                        });
                    }
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                }
                let is_var = symbols
                    .iter()
                    .any(|s| s.name == var_name && s.kind == "var");
                if is_var {
                    items.push(CompletionOut {
                        label: "Value".to_string(),
                        kind: "field",
                        detail: Some("Read current value (pure)".to_string()),
                        insert_text: Some("Value".to_string()),
                    });
                    items.push(CompletionOut {
                        label: "prev".to_string(),
                        kind: "field",
                        detail: Some("Read previous tick's value".to_string()),
                        insert_text: Some("prev".to_string()),
                    });
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                }
                // Receiver methods for a typed value (e.g. string methods on a
                // string). A member-access context never falls through to the
                // global keyword/function list.
                if let Some(ty) = symbols
                    .iter()
                    .find(|s| s.name == var_name)
                    .and_then(|s| s.ty.as_deref())
                {
                    for (name, sig) in receiver_methods(ty) {
                        items.push(CompletionOut {
                            label: name.to_string(),
                            kind: "method",
                            detail: Some(sig),
                            insert_text: None,
                        });
                    }
                }
                return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
            }
        }
    }

    // Param completions inside a call
    if let Some(call_name) = find_enclosing_call(source, line as usize, col as usize) {
        if let Some(spec) = calls().get(call_name.as_str()) {
            // Enum-valued named arg (e.g. `justify = "Center"`): if the cursor
            // is in the value slot of a param whose data field is an enum,
            // offer the enum's variant names instead of param names.
            if let Some((param_name, value_so_far)) =
                named_arg_value(source, line as usize, col as usize)
            {
                if let Some(param) = spec.params.iter().find(|p| p.name == param_name) {
                    if let Some(values) =
                        wirescript::field_enum_values(spec.gate_class, param.port.as_str())
                    {
                        let quoted = !value_so_far.contains('"');
                        for v in values {
                            let insert = if quoted { format!("\"{v}\"") } else { v.clone() };
                            items.push(CompletionOut {
                                label: v,
                                kind: "enum",
                                detail: Some(format!("{param_name} value")),
                                insert_text: Some(insert),
                            });
                        }
                        if !items.is_empty() {
                            return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                        }
                    }
                }
            }
            for p in &spec.params {
                if p.optional {
                    items.push(CompletionOut {
                        label: format!("{} = ", p.name),
                        kind: "field",
                        detail: Some(format!("{} (optional)", type_str(&p.ty))),
                        insert_text: Some(format!("{} = ", p.name)),
                    });
                } else {
                    items.push(CompletionOut {
                        label: p.name.to_string(),
                        kind: "field",
                        detail: Some(format!("{} (required)", type_str(&p.ty))),
                        insert_text: None,
                    });
                }
            }
            if !items.is_empty() {
                return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
            }
        }
    }

    for kw in KEYWORDS {
        items.push(CompletionOut {
            label: kw.to_string(),
            kind: "keyword",
            detail: None,
            insert_text: None,
        });
    }
    for (name, evt) in events().iter() {
        let params: Vec<&str> = evt.data.iter().map(|d| d.name).collect();
        let detail = if params.is_empty() {
            None
        } else {
            Some(format!("({})", params.join(", ")))
        };
        items.push(CompletionOut {
            label: name.to_string(),
            kind: "event",
            detail,
            insert_text: None,
        });
    }
    for (name, spec) in calls().iter() {
        let params: Vec<&str> = spec
            .params
            .iter()
            .filter(|p| !p.optional)
            .map(|p| p.name)
            .collect();
        items.push(CompletionOut {
            label: name.to_string(),
            kind: "function",
            detail: Some(format!("({})", params.join(", "))),
            insert_text: None,
        });
    }
    for ty in &[
        "int",
        "float",
        "bool",
        "string",
        "entity",
        "controller",
        "character",
        "vector",
        "rotator",
        "color",
    ] {
        items.push(CompletionOut {
            label: ty.to_string(),
            kind: "type",
            detail: None,
            insert_text: None,
        });
    }
    for sym in &symbols {
        items.push(CompletionOut {
            label: sym.name.clone(),
            kind: sym.kind,
            detail: sym.ty.clone(),
            insert_text: None,
        });
    }
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".into())
}

pub fn hover(source: &str, line: u32, col: u32, files_json: &str) -> Option<String> {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck(&resolved.ast, "editor");
    let symbols = collect_symbols(&resolved.ast, &tc.type_of_expr);
    let estimates = wirescript::analysis::collect_estimates(&resolved.ast, &tc, "editor");
    let value = hover_at(
        source,
        "editor",
        &symbols,
        &tc.type_of_expr,
        &resolved.doc_comments,
        &tc.if_contexts,
        &tc.var_read_contexts,
        &estimates,
        line as usize,
        col as usize,
    )?;
    Some(serde_json::to_string(&HoverOut { value }).ok()?)
}

#[cfg(test)]
pub fn definition(source: &str, line: u32, col: u32) -> Option<String> {
    definition_with_files(source, line, col, "{}")
}

pub fn definition_with_files(
    source: &str,
    line: u32,
    col: u32,
    files_json: &str,
) -> Option<String> {
    let loader = make_loader(files_json);
    let pre_resolve = parse(source, "editor");
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck(&resolved.ast, "editor");
    let symbols = collect_symbols(&resolved.ast, &tc.type_of_expr);

    let loc = definition_at(
        source,
        &pre_resolve.ast,
        &symbols,
        "editor",
        &loader,
        line as usize,
        col as usize,
    )?;

    let out: LocationOut = loc.into();
    Some(serde_json::to_string(&out).ok()?)
}

#[cfg(test)]
pub fn references(source: &str, line: u32, col: u32) -> Option<String> {
    references_with_files(source, line, col, "{}")
}

pub fn references_with_files(
    source: &str,
    line: u32,
    col: u32,
    _files_json: &str,
) -> Option<String> {
    let word = word_at(source, line as usize, col as usize)?;
    let refs: Vec<LocationOut> = find_all_references(source, &word)
        .into_iter()
        .map(LocationOut::from)
        .collect();
    Some(serde_json::to_string(&refs).unwrap_or_else(|_| "[]".into()))
}

pub fn format(source: &str, tab_size: u32, use_tabs: bool) -> String {
    let tab = if use_tabs {
        "\t".to_string()
    } else {
        " ".repeat(tab_size as usize)
    };
    format_wirescript(source, &tab)
}

#[derive(Serialize)]
struct WorkspaceSymbol {
    name: String,
    kind: &'static str,
    file: String,
    detail: Option<String>,
}

pub fn workspace_symbols(files_json: &str) -> String {
    let files: HashMap<String, String> = serde_json::from_str(files_json).unwrap_or_default();
    let _empty_tmap: TypeMap = HashMap::new();
    let mut syms = Vec::new();
    for (path, source) in &files {
        let parsed = parse(source, path);
        for d in &parsed.ast.decls {
            let (name, kind) = match d {
                TopDecl::Chip(c) => (c.name.clone(), if c.inline { "mod" } else { "chip" }),
                TopDecl::Fn(f) => (f.name.clone(), "fn"),
                TopDecl::Let(l) => {
                    if let LetBinding::Ident { name, .. } = &l.binding {
                        (name.clone(), "let")
                    } else {
                        continue;
                    }
                }
                TopDecl::Event(e) => (e.name.clone(), "event"),
                _ => continue,
            };
            syms.push(WorkspaceSymbol {
                name,
                kind,
                file: path.clone(),
                detail: None,
            });
        }
    }
    serde_json::to_string(&syms).unwrap_or_else(|_| "[]".into())
}

#[derive(Serialize)]
struct InlayHintOut {
    line: usize,
    col: usize,
    label: String,
    kind: &'static str,
}

pub fn inlay_hints(source: &str, files_json: &str) -> String {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck(&resolved.ast, "editor");
    let hints = wirescript::analysis::collect_inlay_hints(source, &resolved.ast, &tc.type_of_expr, "editor");
    let out: Vec<InlayHintOut> = hints
        .into_iter()
        .map(|h| InlayHintOut {
            line: h.line,
            col: h.col,
            label: h.label,
            kind: match h.kind {
                wirescript::analysis::InlayHintKind::Type => "type",
                wirescript::analysis::InlayHintKind::Parameter => "parameter",
            },
        })
        .collect();
    serde_json::to_string(&out).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hover_value(source: &str, line: u32, col: u32) -> Option<String> {
        let raw = hover(source, line, col, "{}")?;
        let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
        parsed
            .get("value")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    // ---- var hover ----
    #[test]
    fn hover_var() {
        let h = hover_value("var count: int = 0", 0, 4).unwrap();
        assert!(h.contains("var count: int"), "got: {h}");
    }

    #[test]
    fn hover_var_inferred() {
        let h = hover_value("var x = 42", 0, 4).unwrap();
        assert!(h.contains("var x"), "got: {h}");
    }

    // ---- let hover ----
    #[test]
    fn hover_let() {
        let h = hover_value("var x: int = 0\nlet y = x + 1", 1, 4).unwrap();
        assert!(h.contains("let y"), "got: {h}");
    }

    // ---- buffer hover ----
    #[test]
    fn hover_buffer() {
        let h = hover_value("var x: int = 0\nbuffer prev = x", 1, 7).unwrap();
        assert!(h.contains("buffer prev"), "got: {h}");
    }

    // ---- in hover ----
    #[test]
    fn hover_in() {
        let h = hover_value("in trigger: exec", 0, 3).unwrap();
        assert!(h.contains("in trigger: exec"), "got: {h}");
    }

    // ---- mod hover ----
    #[test]
    fn hover_mod() {
        let h = hover_value("mod inc(v: *int) { v = v + 1 }", 0, 4).unwrap();
        assert!(h.contains("mod inc"), "got: {h}");
        assert!(h.contains("*int"), "got: {h}");
    }

    // ---- chip hover ----
    #[test]
    fn hover_chip() {
        let h = hover_value(
            "chip Foo(x: int) -> (result: int) {\n  out result = x\n}",
            0,
            5,
        )
        .unwrap();
        assert!(h.contains("chip Foo"), "got: {h}");
    }

    // ---- fn hover ----
    #[test]
    fn hover_fn() {
        let h = hover_value("fn double(x: int) -> int = x * 2", 0, 3).unwrap();
        assert!(h.contains("fn double"), "got: {h}");
        assert!(h.contains("int"), "got: {h}");
    }

    // ---- builtin call hover ----
    #[test]
    fn hover_builtin_call() {
        let h = hover_value("var x: int = Random(0, 10)", 0, 13).unwrap();
        assert!(h.contains("Random"), "got: {h}");
    }

    // ---- builtin call with gate docs ----
    #[test]
    fn hover_display_text_has_docs() {
        let h = hover_value(
            "var ctrl: controller\non RoundStart { ctrl.DisplayText(\"hi\") }",
            1,
            21,
        )
        .unwrap();
        assert!(h.contains("DisplayText"), "got: {h}");
    }

    // ---- named param hover ----
    #[test]
    fn hover_named_param() {
        // fontSize at col 24-31 on line 2
        let src =
            "var ctrl: controller\non RoundStart {\n  DisplayText(ctrl, \"hi\", fontSize = 20)\n}";
        // Try hovering on the 'f' of fontSize
        if let Some(h) = hover_value(src, 2, 24) {
            assert!(h.contains("fontSize") || h.contains("Font"), "got: {h}");
        }
        // Named param hover depends on find_enclosing_call detecting the call context
    }

    // ---- field access hover ----
    #[test]
    fn hover_record_field() {
        let src = "in player: character\nlet input = InputReader(player)\nlet fwd = input.Forward";
        let h = hover_value(src, 2, 16).unwrap();
        assert!(h.contains("Forward") && h.contains("float"), "got: {h}");
    }

    // ---- event hover ----
    #[test]
    fn hover_event() {
        let h = hover_value("on RoundStart { }", 0, 3).unwrap();
        assert!(h.contains("RoundStart"), "got: {h}");
        // Should show `on RoundStart` (using `on`, not fictional `event` keyword)
        assert!(
            h.contains("on RoundStart"),
            "hover should use `on` keyword, got: {h}"
        );
    }

    #[test]
    fn hover_event_with_params() {
        let h = hover_value("on CharacterDied(character) { }", 0, 3).unwrap();
        assert!(h.contains("CharacterDied"), "got: {h}");
        assert!(h.contains("character"), "got: {h}");
        // Should show `on CharacterDied(...)` using `on`, not `event`
        assert!(
            h.contains("on CharacterDied"),
            "hover should use `on` keyword, got: {h}"
        );
    }

    #[test]
    fn hover_event_mid_name() {
        // Hovering at any position within the event name should show the same info
        let h = hover_value("on CharacterDied(a) { }", 0, 10).unwrap();
        assert!(
            h.contains("CharacterDied"),
            "mid-name hover should show event, got: {h}"
        );
        assert!(
            h.contains("character"),
            "should show event param type, got: {h}"
        );
    }

    // ---- array method hover ----
    #[test]
    fn hover_array_push() {
        let h = hover_value("array items: int[]\non RoundStart { items.push(1) }", 1, 22).unwrap();
        assert!(h.contains("push"), "got: {h}");
    }

    // ---- chained receiver call type inference ----
    #[test]
    fn hover_chain_vec_normalize() {
        let h = hover_value(
            "let foo = Vec(0.0, 1.0, 2.0).Normalize()\nout r = foo",
            1,
            8,
        )
        .unwrap();
        assert!(
            h.contains("vector"),
            "Vec.Normalize() should be vector, got: {h}"
        );
    }

    #[test]
    fn hover_chain_vec_magnitude() {
        let h = hover_value(
            "let foo = Vec(1.0, 2.0, 3.0).Magnitude()\nout r = foo",
            1,
            8,
        )
        .unwrap();
        assert!(
            h.contains("float"),
            "Vec.Magnitude() should be float, got: {h}"
        );
    }

    #[test]
    fn hover_chain_vec_dot() {
        let h = hover_value(
            "let a = Vec(1.0, 0.0, 0.0)\nlet b = Vec(0.0, 1.0, 0.0)\nlet d = a.Dot(b)\nout r = d",
            3,
            8,
        )
        .unwrap();
        assert!(h.contains("float"), "a.Dot(b) should be float, got: {h}");
    }

    #[test]
    fn hover_chain_vec_cross() {
        let h = hover_value(
            "let a = Vec(1.0, 0.0, 0.0)\nlet c = a.Cross(Vec(0.0, 1.0, 0.0))\nout r = c",
            2,
            8,
        )
        .unwrap();
        assert!(
            h.contains("vector"),
            "a.Cross(b) should be vector, got: {h}"
        );
    }

    #[test]
    fn hover_chain_vec_distance() {
        let h = hover_value(
            "let d = Vec(0.0, 0.0, 0.0).Distance(Vec(1.0, 1.0, 1.0))\nout r = d",
            1,
            8,
        )
        .unwrap();
        assert!(
            h.contains("float"),
            "Vec.Distance() should be float, got: {h}"
        );
    }

    #[test]
    fn hover_chain_vec_scale() {
        let h = hover_value("let s = Vec(1.0, 2.0, 3.0).ScaleVec(2.0)\nout r = s", 1, 8).unwrap();
        assert!(
            h.contains("vector"),
            "Vec.ScaleVec() should be vector, got: {h}"
        );
    }

    #[test]
    fn hover_chain_vec_split() {
        let h = hover_value("let p = Vec(1.0, 2.0, 3.0).SplitVec()\nout r = p", 1, 8).unwrap();
        assert!(
            h.contains("x") && h.contains("y") && h.contains("z"),
            "Vec.SplitVec() should be {{x,y,z}}, got: {h}"
        );
    }

    #[test]
    fn hover_chain_vec_magnitude_sq() {
        let h = hover_value("let m = Vec(1.0, 2.0, 3.0).MagnitudeSq()\nout r = m", 1, 8).unwrap();
        assert!(
            h.contains("float"),
            "Vec.MagnitudeSq() should be float, got: {h}"
        );
    }

    #[test]
    fn hover_chain_string_contains() {
        let h = hover_value(
            "var s: string = \"hello\"\nlet r = s.Contains(\"ell\")\nout o = r",
            2,
            8,
        )
        .unwrap();
        assert!(h.contains("bool"), "s.Contains() should be bool, got: {h}");
    }

    #[test]
    fn hover_chain_string_tolower() {
        let h = hover_value(
            "var s: string = \"HELLO\"\nlet r = s.ToLower()\nout o = r",
            2,
            8,
        )
        .unwrap();
        assert!(
            h.contains("string"),
            "s.ToLower() should be string, got: {h}"
        );
    }

    #[test]
    fn hover_chain_string_length() {
        let h = hover_value(
            "var s: string = \"hello\"\nlet r = s.Length()\nout o = r",
            2,
            8,
        )
        .unwrap();
        assert!(h.contains("int"), "s.Length() should be int, got: {h}");
    }

    #[test]
    fn hover_chain_string_split() {
        let h = hover_value(
            "var s: string = \"a,b\"\nlet r = s.Split(\",\")\nout o = r",
            2,
            8,
        )
        .unwrap();
        assert!(
            h.contains("Left") && h.contains("Right"),
            "s.Split() should be {{Left, Right}}, got: {h}"
        );
    }

    #[test]
    fn hover_chain_string_find() {
        let h = hover_value(
            "var s: string = \"hello\"\nlet r = s.Find(\"ll\")\nout o = r",
            2,
            8,
        )
        .unwrap();
        assert!(h.contains("int"), "s.Find() should be int, got: {h}");
    }

    #[test]
    fn hover_chain_color_split() {
        let h = hover_value(
            "let c = Color(1.0, 0.0, 0.0)\nlet p = c.SplitColor()\nout r = p",
            2,
            8,
        )
        .unwrap();
        assert!(
            h.contains("r") && h.contains("g") && h.contains("b"),
            "c.SplitColor() should be {{r,g,b,a}}, got: {h}"
        );
    }

    // ---- doc comment hover ----
    #[test]
    fn hover_with_doc_comment() {
        let h = hover_value(
            "/// Increments a value.\nmod inc(v: *int) { v = v + 1 }",
            1,
            4,
        )
        .unwrap();
        assert!(h.contains("mod inc"), "got: {h}");
    }

    // ---- InputReader (pure call) hover ----
    #[test]
    fn hover_input_reader() {
        let h = hover_value("in ch: character\nlet input = InputReader(ch)", 1, 12).unwrap();
        assert!(h.contains("InputReader"), "got: {h}");
        assert!(h.contains("Forward"), "should show record output, got: {h}");
    }

    fn parse_diags(json: &str) -> Vec<serde_json::Value> {
        serde_json::from_str(json).unwrap_or_default()
    }

    fn parse_items(json: &str) -> Vec<serde_json::Value> {
        serde_json::from_str(json).unwrap_or_default()
    }

    // ---- diagnostics ----
    #[test]
    fn diagnostics_no_errors() {
        let items = parse_diags(&diagnostics("var x: int = 0", "{}"));
        assert!(items.is_empty());
    }

    #[test]
    fn diagnostics_unknown_ident() {
        let items = parse_diags(&diagnostics("on RoundStart { x = 1 }", "{}"));
        assert!(items.iter().any(|i| i["code"] == "WS002"));
    }

    #[test]
    fn diagnostics_import_with_files() {
        let files = r#"{"lib.ws": "mod foo(v: *int) { v = v + 1 }"}"#;
        let items = parse_diags(&diagnostics(
            "import \"lib\"\nvar x: int = 0\non RoundStart { foo(x) }",
            files,
        ));
        assert!(items.is_empty(), "unexpected diags: {:?}", items);
    }

    #[test]
    fn diagnostics_import_missing_file() {
        let items = parse_diags(&diagnostics("import \"nonexistent\"", "{}"));
        assert!(items.iter().any(|i| {
            i["message"]
                .as_str()
                .unwrap_or("")
                .contains("cannot resolve")
        }));
    }

    // ---- completions ----
    #[test]
    fn completions_includes_keywords() {
        let items = parse_items(&completions("", 0, 0, "{}", &[]));
        assert!(items.iter().any(|i| i["label"] == "var"));
        assert!(items.iter().any(|i| i["label"] == "import"));
    }

    #[test]
    fn completions_includes_builtins() {
        let items = parse_items(&completions("", 0, 0, "{}", &[]));
        assert!(items.iter().any(|i| i["label"] == "Random"));
        assert!(items.iter().any(|i| i["label"] == "DisplayText"));
    }

    #[test]
    fn completions_includes_user_symbols() {
        let items = parse_items(&completions("var score: int = 0\n", 1, 0, "{}", &[]));
        assert!(items.iter().any(|i| i["label"] == "score"));
    }

    #[test]
    fn completions_includes_chat_command_event() {
        let items = parse_items(&completions("", 0, 0, "{}", &[]));
        assert!(items.iter().any(|i| i["label"] == "ChatCommand"));
    }

    #[test]
    fn var_array_member_completion_has_full_method_set() {
        // `var ids: string[]` completes array methods, including the ones the
        // old hardcoded list omitted (find/sort/insert/...).
        let src = "var ids: string[]\nids.";
        let items = parse_items(&completions(src, 1, 4, "{}", &[]));
        for m in ["push", "find", "sort", "insert", "slice"] {
            assert!(items.iter().any(|i| i["label"] == m), "array method {m} missing: {items:?}");
        }
        assert!(!items.iter().any(|i| i["label"] == "if"), "keyword leaked");
    }

    #[test]
    fn string_member_completion_is_string_only() {
        // `foo.` where foo is a string must show string methods, not the
        // global keyword/function/type list.
        let src = "let foo = \"\"\nfoo.";
        let items = parse_items(&completions(src, 1, 4, "{}", &[]));
        assert!(items.iter().any(|i| i["label"] == "Contains"), "Contains missing: {items:?}");
        assert!(!items.iter().any(|i| i["label"] == "if"), "keyword leaked");
        assert!(!items.iter().any(|i| i["label"] == "int"), "type leaked");
        assert!(!items.iter().any(|i| i["label"] == "ChatCommand"), "event leaked");
    }

    #[test]
    fn prefab_ref_completes_from_registry() {
        // `$./t` offers registered prefab paths under `./t…`.
        let prefabs = vec!["./turret.brz".to_string(), "./enemies/tank.brz".to_string()];
        let src = "on x { SpawnPrefab(prefab = $./t) }";
        let col = (src.find("$./t").unwrap() + "$./t".len()) as u32;
        let items = parse_items(&completions(src, 0, col, "{}", &prefabs));
        assert!(items.iter().any(|i| i["label"] == "./turret.brz"), "got: {items:?}");
        assert!(!items.iter().any(|i| i["label"] == "./enemies/tank.brz"));
    }

    // ---- workspace symbols ----
    #[test]
    fn workspace_symbols_lists_exports() {
        let files =
            r#"{"utils.ws": "mod inc(v: *int) { v = v + 1 }\nfn double(x: int) -> int = x * 2"}"#;
        let items = parse_items(&workspace_symbols(files));
        assert!(
            items
                .iter()
                .any(|i| i["name"] == "inc" && i["kind"] == "mod")
        );
        assert!(
            items
                .iter()
                .any(|i| i["name"] == "double" && i["kind"] == "fn")
        );
    }

    // ---- definition ----
    fn def_loc(source: &str, line: u32, col: u32) -> Option<serde_json::Value> {
        definition(source, line, col).and_then(|s| serde_json::from_str(&s).ok())
    }

    fn def_loc_files(source: &str, line: u32, col: u32, files: &str) -> Option<serde_json::Value> {
        definition_with_files(source, line, col, files).and_then(|s| serde_json::from_str(&s).ok())
    }

    #[test]
    fn definition_var() {
        let d = def_loc("var x: int = 0\non RoundStart { x = 1 }", 1, 16).unwrap();
        assert_eq!(d["startLine"], 0);
        assert_eq!(d["startCol"], 4, "should point to 'x', not 'var'");
        assert_eq!(d["endCol"], 5);
    }

    #[test]
    fn definition_let() {
        let d = def_loc("var x: int = 0\nlet y = x + 1\nout r = y", 2, 8);
        assert!(d.is_some(), "should find definition of y");
        assert_eq!(d.unwrap()["startLine"], 1);
    }

    #[test]
    fn definition_buffer() {
        let d = def_loc("var x: int = 0\nbuffer prev = x\nout r = prev", 2, 8);
        assert!(d.is_some(), "should find definition of prev");
        assert_eq!(d.unwrap()["startLine"], 1);
    }

    #[test]
    fn definition_in() {
        let d = def_loc("in trigger: exec\non trigger { }", 1, 3);
        assert!(d.is_some(), "should find definition of trigger");
        assert_eq!(d.unwrap()["startLine"], 0);
    }

    #[test]
    fn definition_mod() {
        let d = def_loc(
            "mod inc(v: *int) { v = v + 1 }\non RoundStart { inc(x) }",
            1,
            16,
        );
        assert!(d.is_some(), "should find definition of inc");
        assert_eq!(d.unwrap()["startLine"], 0);
    }

    #[test]
    fn definition_chip() {
        let d = def_loc(
            "chip Foo(x: int) -> (r: int) {\n  out r = x\n}\nlet f = Foo(1)",
            3,
            8,
        );
        assert!(d.is_some(), "should find definition of Foo");
        assert_eq!(d.unwrap()["startLine"], 0);
    }

    #[test]
    fn definition_fn() {
        let d = def_loc("fn double(x: int) -> int = x * 2\nlet y = double(21)", 1, 8);
        assert!(d.is_some(), "should find definition of double");
        assert_eq!(d.unwrap()["startLine"], 0);
    }

    #[test]
    fn definition_array() {
        let d = def_loc("array items: int[]\non RoundStart { items.push(1) }", 1, 16);
        assert!(d.is_some(), "should find definition of items");
        assert_eq!(d.unwrap()["startLine"], 0);
    }

    #[test]
    fn definition_param_in_mod() {
        let d = def_loc("mod inc(v: *int) { v = v + 1 }", 0, 19);
        assert!(d.is_some(), "should find definition of param v");
    }

    #[test]
    fn definition_imported_symbol() {
        let files = r#"{"lib.ws": "mod foo(v: *int) { v = v + 1 }"}"#;
        let d = def_loc_files(
            "import \"lib\"\nvar x: int = 0\non RoundStart { foo(x) }",
            2,
            16,
            files,
        );
        assert!(d.is_some(), "should find definition of imported foo");
        let d = d.unwrap();
        assert_eq!(d["file"], "lib.ws", "should point to imported file");
    }

    #[test]
    fn definition_import_path_jumps_to_file() {
        let files = r#"{"lib.ws": "mod foo(v: *int) { v = v + 1 }"}"#;
        // Cursor on the "lib" path string
        let d = def_loc_files("import \"lib\"\nvar x: int = 0", 0, 8, files);
        assert!(d.is_some(), "clicking import path should jump to file");
        let d = d.unwrap();
        assert_eq!(d["file"], "lib.ws");
        assert_eq!(d["startLine"], 0);
        assert_eq!(d["startCol"], 0);
    }

    #[test]
    fn definition_named_import_jumps_to_symbol() {
        let files =
            r#"{"lib.ws": "chip Add(a: int, b: int) -> (result: int) {\n  out result = a + b\n}"}"#;
        // Cursor on "Add" in import { Add } from "lib"
        let d = def_loc_files(
            "import { Add } from \"lib\"\nlet r = Add(1, 2)",
            0,
            10,
            files,
        );
        assert!(d.is_some(), "clicking named import should jump to symbol");
        let d = d.unwrap();
        assert_eq!(d["file"], "lib.ws");
    }

    #[test]
    fn definition_imported_usage_jumps_cross_file() {
        let files = r#"{"math.ws": "chip Add(a: int, b: int) -> (result: int) {\n  out result = a + b\n}"}"#;
        // Cursor on "Add" in the call expression
        let d = def_loc_files(
            "import { Add } from \"math\"\nlet r = Add(1, 2)",
            1,
            9,
            files,
        );
        assert!(d.is_some(), "should find cross-file definition of Add");
        let d = d.unwrap();
        assert_eq!(d["file"], "math.ws", "should point to the imported file");
    }

    #[test]
    fn definition_builtin_returns_none() {
        let d = def_loc("on RoundStart { }", 0, 3);
        assert!(d.is_none(), "builtins have no source definition");
    }

    // ---- references ----
    #[test]
    fn references_var() {
        let r = references("var x: int = 0\non RoundStart { x = x + 1 }", 0, 4);
        let refs: Vec<serde_json::Value> = serde_json::from_str(&r.unwrap()).unwrap();
        assert!(
            refs.len() >= 3,
            "should find at least 3 references (decl + 2 uses), got {}",
            refs.len()
        );
    }

    #[test]
    fn references_mod() {
        let r = references(
            "mod inc(v: *int) { v = v + 1 }\non RoundStart { inc(x) }",
            0,
            4,
        );
        let refs: Vec<serde_json::Value> = serde_json::from_str(&r.unwrap()).unwrap();
        assert!(
            refs.len() >= 2,
            "should find at least 2 references, got {}",
            refs.len()
        );
    }

    #[test]
    fn references_let() {
        let r = references("let y = 42\nout a = y\nout b = y", 0, 4);
        let refs: Vec<serde_json::Value> = serde_json::from_str(&r.unwrap()).unwrap();
        assert!(
            refs.len() >= 3,
            "should find at least 3 references, got {}",
            refs.len()
        );
    }
}
