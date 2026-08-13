
use serde::Serialize;
use wirescript::analysis::{
    Location, TextRange, TypeMap, collect_symbols, definition_at, find_enclosing_call,
    find_name_range, format_wirescript, hover_at, named_arg_value, receiver_methods,
    references_at, type_str,
};
use wirescript::ast::*;
use wirescript::catalog::calls::calls;
use wirescript::catalog::events::events;
use wirescript::lexer::KEYWORDS;
use wirescript::resolve::{MemLoader, resolve};
use wirescript::{parse, typecheck::typecheck_with_inference};

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
    let files: wirescript::collections::HashMap<String, String> = serde_json::from_str(files_json).unwrap_or_default();
    MemLoader { files }
}

pub fn diagnostics(source: &str, files_json: &str) -> String {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck_with_inference(&resolved.ast, "editor").0;
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

/// Swizzle components (`vector`/`color`) + receiver methods (builtin and user
/// `self`-mods) valid for a typed value.
fn push_type_members(
    ty: &str,
    symbols: &[wirescript::analysis::SymbolDef],
    items: &mut Vec<CompletionOut>,
) {
    for f in wirescript::analysis::swizzle_fields(ty) {
        items.push(CompletionOut {
            label: f.to_string(),
            kind: "field",
            detail: Some("field".to_string()),
            insert_text: Some(f.to_string()),
        });
    }
    for (name, sig) in receiver_methods(ty) {
        items.push(CompletionOut {
            label: name.to_string(),
            kind: "method",
            detail: Some(sig),
            insert_text: None,
        });
    }
    for (name, sig) in wirescript::analysis::user_receiver_methods(ty, symbols) {
        items.push(CompletionOut {
            label: name.clone(),
            kind: "method",
            detail: Some(format!("{name}{sig}")),
            insert_text: Some(name),
        });
    }
}

/// Record fields for a receiver type: an inline `{…}` record, or a named `type`
/// alias resolved through its `type` symbol.
fn resolve_record_fields(
    ty: &str,
    symbols: &[wirescript::analysis::SymbolDef],
) -> Option<Vec<String>> {
    if let Some(fields) = wirescript::analysis::record_field_names(ty) {
        return Some(fields);
    }
    let alias = symbols.iter().find(|s| s.name == ty && s.kind == "type")?;
    wirescript::analysis::record_field_names(alias.ty.as_deref()?)
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
    let tc = typecheck_with_inference(&resolved.ast, "editor").0;
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

    // Member access `receiver.partial` — precise: the receiver identifier must
    // directly precede the dot, and only identifier chars may sit between the
    // dot and the cursor. A plain `Call(` boundary yields no receiver and falls
    // through to the param completion below.
    if let Some(var_name) =
        wirescript::analysis::member_receiver_at(source, line as usize, col as usize)
    {
        let sym = symbols.iter().find(|s| s.name == var_name);
        // Collection methods dispatch on the receiver's declared type (resolved
        // through type aliases): `Map<K, V>` -> map table, `T[]` / an `array` decl
        // -> array table. The tables are distinct (map-only `get`/`set`/`has`/
        // `keys`/`values` exist on no array), so this keys on type, never on name.
        let collection = sym.and_then(|s| {
            s.ty.as_deref()
                .and_then(|ty| wirescript::analysis::collection_kind(ty, &symbols))
                .or_else(|| {
                    (s.kind == "array").then_some(wirescript::analysis::CollectionKind::Array)
                })
        });
        if let Some(kind) = collection {
            let push_method = |items: &mut Vec<CompletionOut>, name: &str, sig: &str, doc: &str| {
                items.push(CompletionOut {
                    label: name.to_string(),
                    kind: "method",
                    detail: Some(format!("{name}{sig} - {doc}")),
                    insert_text: None,
                });
            };
            match kind {
                wirescript::analysis::CollectionKind::Array => {
                    for m in wirescript::catalog::arrays::ARRAY_METHODS {
                        push_method(&mut items, m.name, m.signature, m.doc);
                    }
                }
                wirescript::analysis::CollectionKind::Map => {
                    for m in wirescript::catalog::maps::MAP_METHODS {
                        push_method(&mut items, m.name, m.signature, m.doc);
                    }
                }
            }
            return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        }
        // Namespace alias (`import * as u`): offer its qualified `u.member` symbols.
        if sym.is_some_and(|s| s.kind == "namespace") {
            let prefix = format!("{var_name}.");
            for m in &symbols {
                if let Some(member) = m.name.strip_prefix(&prefix) {
                    if member.contains('.') {
                        continue;
                    }
                    items.push(CompletionOut {
                        label: member.to_string(),
                        kind: "method",
                        detail: None,
                        insert_text: Some(member.to_string()),
                    });
                }
            }
            return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        }
        // Vars (`var`/`static var`): `.Value`/`.prev` plus the element type's
        // methods/swizzle (`pos.Normalize()`, `pos.x`).
        if sym.is_some_and(|s| matches!(s.kind, "var" | "static var")) {
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
            if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
                push_type_members(ty, &symbols, &mut items);
            }
            return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        }
        // Record-typed value (e.g. `let split = pl.InputReader()` → {Forward,
        // Right, Jump}, or a named `type` alias): offer the record's field names.
        if let Some(fields) = sym
            .and_then(|s| s.ty.as_deref())
            .and_then(|ty| resolve_record_fields(ty, &symbols))
        {
            for f in fields {
                items.push(CompletionOut {
                    label: f.clone(),
                    kind: "field",
                    detail: Some("field".to_string()),
                    insert_text: Some(f),
                });
            }
            return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
        }
        // Any other typed value: methods (e.g. string methods) + swizzle. A
        // member-access context never falls through to the global list.
        if let Some(ty) = sym.and_then(|s| s.ty.as_deref()) {
            push_type_members(ty, &symbols, &mut items);
        }
        return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
    }

    // Param completions inside a call
    if let Some(call_name) = find_enclosing_call(source, line as usize, col as usize) {
        if let Some(spec) = calls().get(call_name.as_str()) {
            // Enum-valued named arg (e.g. `justify = "Center"`): if the cursor
            // is in the value slot of a param whose data field is an enum,
            // offer the enum's variant names instead of param names.
            let value_ctx = named_arg_value(source, line as usize, col as usize);
            if let Some((param_name, _value_so_far)) = value_ctx.as_ref() {
                // Enum config value — works for a hand-coded param or a raw
                // config field, via the unified call+event helper.
                if let Some(et) =
                    wirescript::catalog::config_enum_for_named_arg(call_name.as_str(), param_name)
                {
                    for v in wirescript::catalog::enum_member_names(et) {
                        items.push(CompletionOut {
                            label: v.clone(),
                            kind: "enum",
                            detail: Some(format!("{et} member")),
                            insert_text: Some(v),
                        });
                    }
                    if !items.is_empty() {
                        return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                    }
                }
                if let Some(param) = spec.params.iter().find(|p| &p.name == param_name) {
                    // Asset-ref config param (`font = <here>`): offer full
                    // `$Type/Name` refs for the param's asset type. Constant-only
                    // params only (a wire-input Entity port takes a live value).
                    if !wirescript::catalog::is_wire_input(spec.gate_class, param.port.as_str())
                        && let Some(asset_ty) =
                            wirescript::analysis::asset_type_for_port(param.port.as_str())
                    {
                        for name in wirescript::analysis::asset_names(asset_ty) {
                            let full = format!("${asset_ty}/{name}");
                            items.push(CompletionOut {
                                label: full.clone(),
                                kind: "constant",
                                detail: Some(format!("{asset_ty} asset")),
                                insert_text: Some(full),
                            });
                        }
                        if !items.is_empty() {
                            return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
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
                        items.push(CompletionOut {
                            label: format!("{} = ", p.name),
                            kind: "field",
                            detail: Some(format!("{ty_label} (optional)")),
                            insert_text: Some(format!("{} = ", p.name)),
                        });
                    } else {
                        items.push(CompletionOut {
                            label: p.name.to_string(),
                            kind: "field",
                            detail: Some(format!("{ty_label} (required)")),
                            insert_text: None,
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
                    items.push(CompletionOut {
                        label: format!("{} = ", cfg.name),
                        kind: "field",
                        detail: Some(format!("{ty_label} (config)")),
                        insert_text: Some(format!("{} = ", cfg.name)),
                    });
                }
                if !items.is_empty() {
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                }
            }
        } else if let Some(evt) =
            wirescript::catalog::events::find_event(call_name.as_str())
        {
            // Event trigger config (`on Clock(<here>)`): resolve names / enum
            // values from the EventSpec (no CallSpec exists for an event).
            if let Some((param_name, _)) = named_arg_value(source, line as usize, col as usize) {
                if let Some(et) =
                    wirescript::catalog::config_enum_for_named_arg(call_name.as_str(), &param_name)
                {
                    for v in wirescript::catalog::enum_member_names(et) {
                        items.push(CompletionOut {
                            label: v.clone(),
                            kind: "enum",
                            detail: Some(format!("{et} member")),
                            insert_text: Some(v),
                        });
                    }
                    if !items.is_empty() {
                        return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                    }
                }
            }
            for (surf, _, _) in &evt.input_named {
                items.push(CompletionOut {
                    label: format!("{surf} = "),
                    kind: "field",
                    detail: Some("wired input".to_string()),
                    insert_text: Some(format!("{surf} = ")),
                });
            }
            for (surf, field) in &evt.config_named {
                let ty_label = wirescript::catalog::config_field_enum_type(evt.gate_class, field)
                    .map(str::to_string)
                    .unwrap_or_else(|| "config".to_string());
                items.push(CompletionOut {
                    label: format!("{surf} = "),
                    kind: "field",
                    detail: Some(format!("{ty_label} (config)")),
                    insert_text: Some(format!("{surf} = ")),
                });
            }
            if !items.is_empty() {
                return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
            }
        } else if let Some(sig) = symbols
            .iter()
            .find(|s| s.name == call_name.as_str() && matches!(s.kind, "mod" | "chip" | "fn"))
            .and_then(|s| s.ty.as_deref())
        {
            // User-defined mod/chip/fn call: complete its parameter names.
            if let Some(names) = wirescript::analysis::param_names(sig) {
                for name in names {
                    items.push(CompletionOut {
                        label: name.clone(),
                        kind: "field",
                        detail: None,
                        insert_text: Some(name),
                    });
                }
                if !items.is_empty() {
                    return serde_json::to_string(&items).unwrap_or_else(|_| "[]".into());
                }
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
        "int", "float", "bool", "string", "entity", "controller", "character", "vector", "rotator",
        "color", "exec",
    ] {
        items.push(CompletionOut {
            label: ty.to_string(),
            kind: "type",
            detail: None,
            insert_text: None,
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
        items.push(CompletionOut {
            label: ann.to_string(),
            kind: "keyword",
            detail: Some(detail.to_string()),
            insert_text: None,
        });
    }
    // Qualified namespace members (`u.member`) are reached only through `u.`
    // member completion, so keep them out of the bare-identifier list.
    for sym in symbols.iter().filter(|s| !s.name.contains('.')) {
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
    let tc = typecheck_with_inference(&resolved.ast, "editor").0;
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
    let tc = typecheck_with_inference(&resolved.ast, "editor").0;
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
    // The playground is single-file, so `_files_json` is unused — `references_at`
    // only ever returns same-file sites (cross-file rename lives in the LSP).
    let parsed = parse(source, "editor");
    let (target, sites) =
        references_at(&parsed.ast, source, "editor", line as usize, col as usize)?;
    let refs: Vec<LocationOut> = sites
        .into_iter()
        .map(|site| {
            // A coarse site's range spans the whole declaration/statement, not
            // just the name — narrow it to the name token, same as the LSP
            // wiring layer (falls back to the coarse range if narrowing fails).
            let range = if site.coarse {
                find_name_range(source, &site.range, &target.name).unwrap_or(site.range)
            } else {
                site.range
            };
            LocationOut {
                start_line: range.start.line.saturating_sub(1) as usize,
                start_col: range.start.col.saturating_sub(1) as usize,
                end_line: range.end.line.saturating_sub(1) as usize,
                end_col: range.end.col.saturating_sub(1) as usize,
                file: None,
            }
        })
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
    let files: wirescript::collections::HashMap<String, String> = serde_json::from_str(files_json).unwrap_or_default();
    let _empty_tmap: TypeMap = TypeMap::default();
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
    let tc = typecheck_with_inference(&resolved.ast, "editor").0;
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
mod tests;
