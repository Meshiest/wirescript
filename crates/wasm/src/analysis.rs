use serde::Serialize;
use wirescript::analysis::text::{Ruler, byte_col_to_editor, editor_col_to_char};
use wirescript::analysis::{
    Location, TypeMap, collect_symbols, definition_at, find_name_range, format_wirescript,
    hover_at, references_at,
};

// Three column conventions meet in this file. The editor (Monaco, and the VS Code SDK shim) counts columns in UTF-16 code
// units; `analysis::` takes and returns 0-based CHAR columns; the compiler's
// `Pos::col` is a 1-based BYTE column. They agree only while a line is pure
// ASCII, and slicing a line with the wrong one lands mid-character and traps
// the wasm module until the page is reloaded. Convert at this boundary and
// nowhere else.

use crate::make_loader;
use wirescript::ast::*;
use wirescript::resolve::resolve;
use wirescript::{parse, typecheck_with_inference};

#[derive(Serialize)]
pub(crate) struct DiagnosticOut {
    pub severity: &'static str,
    pub code: String,
    /// The docs section for `code`, when the docs cover it.
    #[serde(rename = "docUrl", skip_serializing_if = "Option::is_none")]
    pub doc_url: Option<String>,
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
pub(crate) struct CompletionOut {
    pub label: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(rename = "insertText", skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct HoverOut {
    pub value: String,
}

#[derive(Serialize)]
pub(crate) struct LocationOut {
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

/// A `Location` carries 0-based BYTE columns (it is built straight from a
/// `SourceRange`), so converting one for the editor needs the source it was
/// measured against, which is why this is not a `From` impl.
/// A cross-file `Location` is left alone: its columns index a file this
/// frontend does not hold, and the playground is single-file anyway.
fn location_out(source: &str, loc: Location) -> LocationOut {
    let same_file = loc.file.is_none();
    let conv = |line: usize, col: usize| {
        if same_file {
            byte_col_to_editor(source, line, col)
        } else {
            col
        }
    };
    LocationOut {
        start_col: conv(loc.start_line, loc.start_col),
        end_col: conv(loc.end_line, loc.end_col),
        start_line: loc.start_line,
        end_line: loc.end_line,
        file: loc.file,
    }
}

pub(crate) fn diagnostics(source: &str, files_json: &str) -> String {
    let loader = make_loader(files_json);
    // Lowering runs so the editor sees `WSP001`: a program that type-checks
    // clean and lowers to an `_Unsupported` placeholder is a dead circuit, and
    // stopping at typecheck hid every one of them while `wirescript_compile`
    // went on to succeed and hand the user the `.brz`.
    let input = wirescript::CompileInput::new(source, "editor");
    let ruler = Ruler::new(source);
    let diags: Vec<DiagnosticOut> = wirescript::diagnostics_only(input, &loader)
        .iter()
        .filter(|d| &*d.range.file == "editor" || d.range.file.is_empty())
        .map(|d| DiagnosticOut {
            severity: match d.severity {
                wirescript::diagnostic::Severity::Error => "error",
                wirescript::diagnostic::Severity::Warning => "warning",
                wirescript::diagnostic::Severity::Info => "info",
            },
            code: d.code.clone(),
            doc_url: wirescript::analysis::doc_url(wirescript::analysis::Doc::Diagnostic(&d.code)),
            message: d.message.clone(),
            start_line: d.range.start.line.saturating_sub(1) as usize,
            start_col: ruler.byte_col_to_editor(
                d.range.start.line.saturating_sub(1) as usize,
                d.range.start.col.saturating_sub(1) as usize,
            ),
            end_line: d.range.end.line.saturating_sub(1) as usize,
            end_col: ruler.byte_col_to_editor(
                d.range.end.line.saturating_sub(1) as usize,
                d.range.end.col.saturating_sub(1) as usize,
            ),
        })
        .collect();
    serde_json::to_string(&diags).unwrap_or_else(|_| "[]".into())
}

/// The kind name the playground editor reads for a completion.
fn kind_name(kind: wirescript::analysis::CompletionKind) -> &'static str {
    use wirescript::analysis::CompletionKind as K;
    match kind {
        K::Text => "text",
        K::Method => "method",
        K::Function => "function",
        K::Field => "field",
        K::Variable => "variable",
        K::Class => "type",
        K::Module => "module",
        K::Property => "property",
        K::Keyword => "keyword",
        K::Constant => "constant",
        K::EnumMember => "enum",
        K::Event => "event",
        K::File => "file",
    }
}

pub(crate) fn completions(
    source: &str,
    line: u32,
    col: u32,
    files_json: &str,
    prefab_paths: &[String],
) -> String {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck_with_inference(&resolved.ast).0;
    let symbols = collect_symbols(&resolved.ast, &tc.type_of_expr);
    let col = editor_col_to_char(source, line as usize, col as usize);
    let items: Vec<CompletionOut> = wirescript::analysis::completions(
        source,
        &symbols,
        line as usize,
        col,
        prefab_paths,
        &wirescript::EnumRegistry::editor_lookup,
    )
    .into_iter()
    .map(|c| CompletionOut {
        kind: kind_name(c.kind),
        // A completion that replaces the text before the cursor inserts
        // its whole label.
        insert_text: c
            .insert_text
            .or_else(|| c.replace_from.map(|_| c.label.clone())),
        label: c.label,
        detail: c.detail,
    })
    .collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".into())
}

pub(crate) fn hover(source: &str, line: u32, col: u32, files_json: &str) -> Option<String> {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck_with_inference(&resolved.ast).0;
    let symbols = collect_symbols(&resolved.ast, &tc.type_of_expr);
    let estimates = wirescript::analysis::collect_estimates(&resolved.ast, &tc, "editor");
    let pre_resolve = parse(source, "editor");
    let value = hover_at(
        source,
        "editor",
        &pre_resolve.ast,
        &symbols,
        &tc.type_of_expr,
        &resolved.doc_comments,
        &tc.if_contexts,
        &tc.var_read_contexts,
        &tc.narrowed_reads,
        &tc.dropped_ranges,
        &estimates,
        line as usize,
        editor_col_to_char(source, line as usize, col as usize),
    )?;
    serde_json::to_string(&HoverOut { value }).ok()
}

#[cfg(test)]
pub(crate) fn definition(source: &str, line: u32, col: u32) -> Option<String> {
    definition_with_files(source, line, col, "{}")
}

pub(crate) fn definition_with_files(
    source: &str,
    line: u32,
    col: u32,
    files_json: &str,
) -> Option<String> {
    let loader = make_loader(files_json);
    let pre_resolve = parse(source, "editor");
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck_with_inference(&resolved.ast).0;
    let symbols = collect_symbols(&resolved.ast, &tc.type_of_expr);

    let loc = definition_at(
        source,
        &pre_resolve.ast,
        &symbols,
        "editor",
        &loader,
        line as usize,
        editor_col_to_char(source, line as usize, col as usize),
    )?;

    serde_json::to_string(&location_out(source, loc)).ok()
}

#[cfg(test)]
pub(crate) fn references(source: &str, line: u32, col: u32) -> Option<String> {
    references_with_files(source, line, col, "{}")
}

pub(crate) fn references_with_files(
    source: &str,
    line: u32,
    col: u32,
    _files_json: &str,
) -> Option<String> {
    // The playground is single-file, so `_files_json` is unused: `references_at`
    // returns same-file sites only (cross-file rename is the LSP's).
    let parsed = parse(source, "editor");
    let (target, sites) = references_at(
        &parsed.ast,
        source,
        "editor",
        line as usize,
        editor_col_to_char(source, line as usize, col as usize),
    )?;
    let ruler = Ruler::new(source);
    let refs: Vec<LocationOut> = sites
        .into_iter()
        .map(|site| {
            // A coarse site spans its whole declaration or statement: narrow it to the
            // name token as the LSP does, keeping the coarse range when that fails.
            let range = if site.coarse {
                find_name_range(source, &site.range, &target.name).unwrap_or(site.range)
            } else {
                site.range
            };
            let (sl, el) = (
                range.start.line.saturating_sub(1) as usize,
                range.end.line.saturating_sub(1) as usize,
            );
            LocationOut {
                start_line: sl,
                start_col: ruler.byte_col_to_editor(sl, range.start.col.saturating_sub(1) as usize),
                end_line: el,
                end_col: ruler.byte_col_to_editor(el, range.end.col.saturating_sub(1) as usize),
                file: None,
            }
        })
        .collect();
    Some(serde_json::to_string(&refs).unwrap_or_else(|_| "[]".into()))
}

pub(crate) fn format(source: &str, tab_size: u32, use_tabs: bool) -> String {
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

pub(crate) fn workspace_symbols(files_json: &str) -> String {
    let files: wirescript::collections::HashMap<String, String> =
        serde_json::from_str(files_json).unwrap_or_default();
    let _empty_tmap: TypeMap = TypeMap::default();
    let mut syms = Vec::new();
    for (path, source) in &files {
        let parsed = parse(source, path);
        for d in &parsed.ast.decls {
            let (name, kind) = match d {
                TopDecl::Chip(c) => (c.name.clone(), c.kind_label()),
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

pub(crate) fn inlay_hints(source: &str, files_json: &str) -> String {
    let loader = make_loader(files_json);
    let resolved = resolve(source, "editor", &loader);
    let tc = typecheck_with_inference(&resolved.ast).0;
    let hints = wirescript::analysis::collect_inlay_hints(
        source,
        &resolved.ast,
        &tc.type_of_expr,
        "editor",
    );
    let ruler = Ruler::new(source);
    let out: Vec<InlayHintOut> = hints
        .into_iter()
        .map(|h| InlayHintOut {
            line: h.line,
            col: ruler.char_col_to_editor(h.line, h.col),
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
