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

    // If cursor is on a field access (e.g. cpu.cpsr), resolve the field
    // within the object's record type rather than matching standalone symbols.
    if is_field_access(source, line, col) {
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
                        let decl_name = match td {
                            TopDecl::Chip(c) => &c.name,
                            TopDecl::Fn(f) => &f.name,
                            TopDecl::Let(l) => match &l.binding {
                                LetBinding::Ident { name, .. } => name,
                                _ => continue,
                            },
                            TopDecl::Event(e) => &e.name,
                            _ => continue,
                        };
                        if decl_name == &b.name {
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
