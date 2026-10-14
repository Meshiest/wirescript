use super::TypeMap;
use super::types::type_str;
use crate::ast::*;
use crate::ast::walk;
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
    let source = &Text { source, index: super::text::LineIndex::new(source) };

    for decl in &ast.decls {
        let node = walk::Node::Decl(decl);
        hint_node(source, node, type_map, &f, &mut hints);
        walk::walk(node, walk::Ctx::default(), &mut |n, _| {
            hint_node(source, n, type_map, &f, &mut hints);
            true
        });
    }

    hints
}

/// The hint a `let`, or a top-level `buffer`, gets when it names no type.
fn hint_node(source: &Text, node: walk::Node, type_map: &TypeMap, file: &Arc<str>, hints: &mut Vec<InlayHintInfo>) {
    match node {
        walk::Node::Decl(TopDecl::Let(l)) | walk::Node::Stmt(Stmt::Let(l)) => hint_let(source, l, type_map, file, hints),
        walk::Node::Decl(TopDecl::Buffer(b)) => hint_buffer(source, b, type_map, file, hints),
        _ => {}
    }
}

fn hint_let(
    source: &Text,
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
                    let (line, col) = source.line_col(range.end.offset);
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
                let (line, col) = source.line_col(range.end.offset);
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
    source: &Text,
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
            let (line, col) = source.line_col(name_end);
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

/// A file's text with its line index, which hints place by.
struct Text<'a> {
    source: &'a str,
    index: super::text::LineIndex,
}

impl Text<'_> {
    /// The 0-based line and char column of byte `offset`.
    fn line_col(&self, offset: usize) -> (usize, usize) {
        self.index.line_col(self.source, offset)
    }
}

#[cfg(test)]
mod tests;
