use super::TypeMap;
use super::types::type_str;
use crate::ast::*;
use crate::ir::Type;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct InlayHintInfo {
    pub line: usize,
    pub col: usize,
    pub label: String,
    pub kind: InlayHintKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InlayHintKind {
    Type,
    Parameter,
}

pub fn collect_inlay_hints(
    source: &str,
    ast: &Script,
    type_map: &TypeMap,
    file: &str,
) -> Vec<InlayHintInfo> {
    let mut hints = Vec::new();
    let f: Arc<str> = file.into();

    for decl in &ast.decls {
        collect_from_decl(source, decl, type_map, &f, &mut hints);
    }

    hints
}

fn collect_from_decl(
    source: &str,
    decl: &TopDecl,
    type_map: &TypeMap,
    file: &Arc<str>,
    hints: &mut Vec<InlayHintInfo>,
) {
    match decl {
        TopDecl::Let(l) => hint_let(source, l, type_map, file, hints),
        TopDecl::Buffer(b) => hint_buffer(source, b, type_map, file, hints),
        TopDecl::Chip(c) => {
            for s in &c.body.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
        }
        TopDecl::AnonChip(ac) => {
            for s in &ac.body.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
        }
        TopDecl::Handler(h) => {
            for s in &h.body.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
        }
        TopDecl::Namespace(ns) => {
            for d in &ns.decls {
                collect_from_decl(source, d, type_map, file, hints);
            }
        }
        _ => {}
    }
}

fn collect_from_stmt(
    source: &str,
    stmt: &Stmt,
    type_map: &TypeMap,
    file: &Arc<str>,
    hints: &mut Vec<InlayHintInfo>,
) {
    match stmt {
        Stmt::Let(l) => hint_let(source, l, type_map, file, hints),
        Stmt::If(i) => {
            for s in &i.then_block.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
            if let Some(ref else_block) = i.else_block {
                for s in &else_block.stmts {
                    collect_from_stmt(source, s, type_map, file, hints);
                }
            }
        }
        Stmt::Handler(h) => {
            for s in &h.body.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
        }
        Stmt::AnonChip(ac) => {
            for s in &ac.body.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
        }
        Stmt::ChipDecl(c) => {
            for s in &c.body.stmts {
                collect_from_stmt(source, s, type_map, file, hints);
            }
        }
        Stmt::Await(_) => {}
        _ => {}
    }
}

fn hint_let(
    source: &str,
    l: &LetDecl,
    type_map: &TypeMap,
    file: &Arc<str>,
    hints: &mut Vec<InlayHintInfo>,
) {
    if l.typ.is_some() {
        return;
    }
    match &l.binding {
        LetBinding::Ident { range, .. } => {
            let ty = infer_from_map(type_map, file, &l.value);
            if let Some(ty) = ty {
                let ts = type_str(&ty);
                if ts != "any" && ts != "unknown" {
                    let (line, col) = offset_to_line_col(source, range.end.offset);
                    hints.push(InlayHintInfo {
                        line,
                        col,
                        label: format!(": {ts}"),
                        kind: InlayHintKind::Type,
                    });
                }
            }
        }
        LetBinding::Tuple { range, .. } => {
            let ty = infer_from_map(type_map, file, &l.value);
            if let Some(Type::Tuple(fields)) = ty {
                // Hint each element if we can find individual offsets
                // For now, hint the whole tuple binding
                let ts: Vec<String> = fields.iter().map(type_str).collect();
                let (line, col) = offset_to_line_col(source, range.end.offset);
                hints.push(InlayHintInfo {
                    line,
                    col,
                    label: format!(": ({})", ts.join(", ")),
                    kind: InlayHintKind::Type,
                });
            }
        }
        _ => {}
    }
}

fn hint_buffer(
    source: &str,
    b: &BufferDecl,
    type_map: &TypeMap,
    file: &Arc<str>,
    hints: &mut Vec<InlayHintInfo>,
) {
    if b.typ.is_some() {
        return;
    }
    let ty = infer_from_map(type_map, file, &b.init);
    if let Some(ty) = ty {
        let ts = type_str(&ty);
        if ts != "any" && ts != "unknown" {
            let name_end = b.range.start.offset + "buffer ".len() + b.name.len();
            let (line, col) = offset_to_line_col(source, name_end);
            hints.push(InlayHintInfo {
                line,
                col,
                label: format!(": {ts}"),
                kind: InlayHintKind::Type,
            });
        }
    }
}

fn infer_from_map(type_map: &TypeMap, file: &Arc<str>, expr: &Expr) -> Option<Type> {
    let range = expr.range();
    let key = (file.clone(), range.start.offset, range.end.offset);
    type_map.get(&key).cloned()
}

fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

#[cfg(test)]
mod tests;
