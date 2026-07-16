use super::references::find_name_range;
use super::symbols::SymbolDef;
use super::text::word_at;
use crate::ast::*;
use crate::catalog::calls::calls;
use crate::catalog::events::find_event;
use crate::diagnostic::SourceRange;
use crate::resolve::FileLoader;

#[derive(Clone, Debug)]
pub struct Location {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub file: Option<String>,
}

fn source_range_to_location(r: &SourceRange, file: Option<String>) -> Location {
    Location {
        start_line: r.start.line.saturating_sub(1) as usize,
        start_col: r.start.col.saturating_sub(1) as usize,
        end_line: r.end.line.saturating_sub(1) as usize,
        end_col: r.end.col.saturating_sub(1) as usize,
        file,
    }
}

fn cross_file_path(sym: &SymbolDef, current_file: &str) -> Option<String> {
    if !sym.range.file.is_empty() && &*sym.range.file != current_file {
        Some(sym.range.file.to_string())
    } else {
        None
    }
}

pub fn definition_at(
    source: &str,
    pre_resolve_ast: &Script,
    symbols: &[SymbolDef],
    current_file: &str,
    loader: &dyn FileLoader,
    line: usize,
    col: usize,
) -> Option<Location> {
    // Check if cursor is on an import path or binding
    if let Some(loc) = find_import_definition(pre_resolve_ast, current_file, loader, line, col) {
        return Some(loc);
    }

    let word = word_at(source, line, col)?;

    if is_field_access(source, line, col) {
        // Namespace-qualified name (`card.drawCard` with `import * as card`):
        // resolve in the imported file. Checked before the symbol loop so a
        // same-named local decl can't shadow the qualified reference.
        if let Some(loc) = resolve_namespace_definition(
            source, pre_resolve_ast, current_file, loader, &word, line, col,
        ) {
            return Some(loc);
        }
        // Field access on a record value (e.g. cpu.cpsr): resolve the field
        // within the object's record type rather than matching standalone symbols.
        if let Some(loc) = resolve_field_definition(source, symbols, current_file, loader, &word, line, col) {
            return Some(loc);
        }
    }

    for sym in symbols {
        if sym.name == word {
            let file = cross_file_path(sym, current_file);
            let file_source = file.as_ref().and_then(|_| loader.load(&sym.range.file, current_file).ok());
            let search_source = file_source.as_deref().unwrap_or(source);
            let r = find_name_range(search_source, &sym.range, &sym.name)
                .unwrap_or_else(|| sym.range.clone());
            return Some(source_range_to_location(&r, file));
        }
    }

    if find_event(&word).is_some() || calls().get(word.as_str()).is_some() {
        return None;
    }

    None
}

fn find_import_definition(
    ast: &Script,
    current_file: &str,
    loader: &dyn FileLoader,
    line: usize,
    _col: usize,
) -> Option<Location> {
    let cursor_line = (line + 1) as u32;

    for d in &ast.decls {
        let TopDecl::Import(imp) = d else { continue };
        if cursor_line < imp.range.start.line || cursor_line > imp.range.end.line {
            continue;
        }

        let resolved_path = loader.canonical_path(&imp.path, current_file);
        let import_path = if resolved_path.ends_with(".ws") {
            resolved_path
        } else {
            format!("{}.ws", imp.path)
        };

        if let ImportKind::Named(bindings) = &imp.kind {
            if let Ok(file_src) = loader.load(&imp.path, current_file) {
                let target_ast = crate::parse(&file_src, &import_path);
                for b in bindings {
                    for td in &target_ast.ast.decls {
                        if top_decl_name(td) == Some(&b.name) {
                            let r = find_name_range(&file_src, td.range(), &b.name)
                                .unwrap_or_else(|| td.range().clone());
                            return Some(source_range_to_location(&r, Some(import_path.clone())));
                        }
                    }
                }
            }
        }

        return Some(Location {
            start_line: 0, start_col: 0, end_line: 0, end_col: 0,
            file: Some(import_path),
        });
    }
    None
}

fn is_field_access(source: &str, line: usize, col: usize) -> bool {
    let Some(l) = source.lines().nth(line) else { return false };
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    start > 0 && l.as_bytes().get(start - 1) == Some(&b'.')
}

/// The referenceable name a top-level declaration binds, if any.
fn top_decl_name(td: &TopDecl) -> Option<&str> {
    match td {
        TopDecl::Chip(c) => Some(&c.name),
        TopDecl::Fn(f) => Some(&f.name),
        TopDecl::Let(l) => match &l.binding {
            LetBinding::Ident { name, .. } => Some(name),
            _ => None,
        },
        TopDecl::Event(e) => Some(&e.name),
        _ => None,
    }
}

/// Definition of `ns.name` where `ns` is a star-import alias
/// (`import * as ns from "file"`): the decl named `name` in that file.
fn resolve_namespace_definition(
    source: &str,
    ast: &Script,
    current_file: &str,
    loader: &dyn FileLoader,
    name: &str,
    line: usize,
    col: usize,
) -> Option<Location> {
    // Identifier immediately before the `.` the cursor's word follows.
    let l = source.lines().nth(line)?;
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let field_start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    if field_start == 0 || l.as_bytes().get(field_start - 1) != Some(&b'.') {
        return None;
    }
    let dot = field_start - 1;
    let obj_start = l[..dot]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ns = &l[obj_start..dot];
    if ns.is_empty() {
        return None;
    }

    for d in &ast.decls {
        let TopDecl::Import(imp) = d else { continue };
        let ImportKind::Namespace(alias) = &imp.kind else {
            continue;
        };
        if alias != ns {
            continue;
        }
        let file_src = loader.load(&imp.path, current_file).ok()?;
        let resolved_path = loader.canonical_path(&imp.path, current_file);
        let import_path = if resolved_path.ends_with(".ws") {
            resolved_path
        } else {
            format!("{}.ws", imp.path)
        };
        let target_ast = crate::parse(&file_src, &import_path);
        for td in &target_ast.ast.decls {
            if top_decl_name(td) == Some(name) {
                let r = find_name_range(&file_src, td.range(), name)
                    .unwrap_or_else(|| td.range().clone());
                return Some(source_range_to_location(&r, Some(import_path)));
            }
        }
        // The alias matched but the member doesn't exist in that file:
        // report nothing rather than letting a same-named local decl
        // swallow the jump.
        return None;
    }
    None
}

fn resolve_field_definition(
    source: &str,
    symbols: &[SymbolDef],
    current_file: &str,
    loader: &dyn FileLoader,
    field: &str,
    line: usize,
    col: usize,
) -> Option<Location> {
    let l = source.lines().nth(line)?;
    let c = l.char_indices().nth(col).map(|(i, _)| i).unwrap_or(l.len());
    let field_start = l[..c]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    if field_start == 0 || l.as_bytes().get(field_start - 1) != Some(&b'.') {
        return None;
    }
    let dot = field_start - 1;
    let obj_start = l[..dot]
        .rfind(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let obj_name = &l[obj_start..dot];
    if obj_name.is_empty() {
        return None;
    }

    // Find the object's symbol and its type, then locate the type declaration
    let obj_sym = symbols.iter().find(|s| s.name == obj_name)?;
    let ty_name = obj_sym.ty.as_deref()?;
    let type_sym = symbols.iter().find(|s| s.kind == "type" && s.name == ty_name)?;
    let file = cross_file_path(type_sym, current_file);
    let type_source = file.as_ref().and_then(|_| loader.load(&type_sym.range.file, current_file).ok());
    let search_src = type_source.as_deref().unwrap_or(source);

    // Search within the type definition's source lines for the field name
    let start_line = type_sym.range.start.line.saturating_sub(1) as usize;
    let end_line = (type_sym.range.end.line as usize).min(search_src.lines().count());
    for line_idx in start_line..end_line {
        if let Some(line_str) = search_src.lines().nth(line_idx) {
            if let Some(pos) = line_str.find(field) {
                let before = if pos > 0 { line_str.as_bytes()[pos - 1] } else { b' ' };
                let after = line_str.as_bytes().get(pos + field.len()).copied().unwrap_or(b' ');
                if !before.is_ascii_alphanumeric() && before != b'_'
                    && !after.is_ascii_alphanumeric() && after != b'_'
                {
                    return Some(Location {
                        start_line: line_idx, start_col: pos,
                        end_line: line_idx, end_col: pos + field.len(),
                        file,
                    });
                }
            }
        }
    }

    // Fallback: jump to the type declaration itself
    Some(source_range_to_location(&type_sym.range, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::symbols::collect_symbols_for_file;
    use crate::resolve::{MemLoader, resolve};

    fn goto(main: &str, display: &str, line: usize, col: usize) -> Option<Location> {
        let mut files = crate::collections::HashMap::default();
        files.insert("display.ws".to_string(), display.to_string());
        let loader = MemLoader { files };
        let pre = crate::parse(main, "main.ws");
        let resolved = resolve(main, "main.ws", &loader);
        let tc = crate::typecheck::typecheck(&resolved.ast, "main.ws");
        let symbols = collect_symbols_for_file(&resolved.ast, &tc.type_of_expr, Some("main.ws"));
        definition_at(main, &pre.ast, &symbols, "main.ws", &loader, line, col)
    }

    #[test]
    fn namespaced_call_goes_to_imported_decl_not_local_shadow() {
        // `card.drawCard` must jump to display.ws's drawCard even though a
        // local `mod drawCard` shares the name.
        let display = "mod drawCard(x: int) {\n  let unused = x\n}\n";
        let main = "import * as card from \"display\"\n\nmod drawCard(y: int) {\n  card.drawCard(y)\n}\n";
        let call_line = 3;
        let col = main.lines().nth(call_line).unwrap().find(".drawCard").unwrap() + 2;
        let loc = goto(main, display, call_line, col).expect("definition should resolve");
        assert_eq!(
            loc.file.as_deref(),
            Some("display.ws"),
            "qualified name must resolve in the imported file, got {loc:?}"
        );
        assert_eq!(loc.start_line, 0, "display.ws drawCard is on its line 0");
    }

    #[test]
    fn unqualified_call_still_goes_to_local_decl() {
        // Bare `drawCard(...)` keeps resolving to the local mod.
        let display = "mod drawCard(x: int) {\n  let unused = x\n}\n";
        let main = "import * as card from \"display\"\n\nmod drawCard(y: int) {\n  let z = y\n}\n\nmod use1(w: int) {\n  drawCard(w)\n}\n";
        let call_line = 7;
        let col = main.lines().nth(call_line).unwrap().find("drawCard").unwrap() + 1;
        let loc = goto(main, display, call_line, col).expect("definition should resolve");
        assert_eq!(loc.file, None, "bare name resolves to the local decl, got {loc:?}");
        assert_eq!(loc.start_line, 2, "local drawCard is on line 2");
    }
}
