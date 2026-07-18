use crate::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::diagnostic::{Diagnostic, SourceRange};
use crate::parser::{ParseResult, parse};

pub trait FileLoader {
    fn load(&self, path: &str, relative_to: &str) -> Result<String, String>;
    fn canonical_path(&self, path: &str, relative_to: &str) -> String;
}

pub struct FsLoader;

impl FileLoader for FsLoader {
    fn load(&self, path: &str, relative_to: &str) -> Result<String, String> {
        let base = std::path::Path::new(relative_to)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let mut full = base.join(path);
        if full.extension().is_none() {
            full.set_extension("ws");
        }
        std::fs::read_to_string(&full)
            .map_err(|e| format!("cannot read '{}': {}", full.display(), e))
    }

    fn canonical_path(&self, path: &str, relative_to: &str) -> String {
        let base = std::path::Path::new(relative_to)
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let mut full = base.join(path);
        if full.extension().is_none() {
            full.set_extension("ws");
        }
        full.canonicalize()
            .unwrap_or(full.clone())
            .to_string_lossy()
            .to_string()
    }
}

pub struct MemLoader {
    pub files: HashMap<String, String>,
}

impl FileLoader for MemLoader {
    fn load(&self, path: &str, _relative_to: &str) -> Result<String, String> {
        let key = if path.ends_with(".ws") {
            path.to_string()
        } else {
            format!("{path}.ws")
        };
        self.files
            .get(&key)
            .or_else(|| self.files.get(path))
            .cloned()
            .ok_or_else(|| format!("file not found: '{path}'"))
    }

    fn canonical_path(&self, path: &str, _relative_to: &str) -> String {
        if path.ends_with(".ws") {
            path.to_string()
        } else {
            format!("{path}.ws")
        }
    }
}

pub struct ResolveResult {
    pub ast: Script,
    pub diagnostics: Vec<Diagnostic>,
    pub doc_comments: HashMap<usize, String>,
}

fn is_importable(d: &TopDecl) -> bool {
    matches!(
        d,
        TopDecl::Chip(_)
            | TopDecl::Fn(_)
            | TopDecl::Let(_)
            | TopDecl::Event(_)
            | TopDecl::Var(_)
            | TopDecl::Array(_)
            | TopDecl::Buffer(_)
            | TopDecl::In(_)
            | TopDecl::Out(_)
            | TopDecl::TypeAlias(_)
    )
}

fn decl_name(d: &TopDecl) -> Option<&str> {
    match d {
        TopDecl::Chip(c) => Some(&c.name),
        TopDecl::Fn(f) => Some(&f.name),
        TopDecl::Let(l) => match &l.binding {
            LetBinding::Ident { name, .. } => Some(name),
            _ => None,
        },
        TopDecl::Event(e) => Some(&e.name),
        TopDecl::Var(v) => Some(&v.name),
        TopDecl::Array(a) => Some(&a.name),
        TopDecl::Buffer(b) => Some(&b.name),
        TopDecl::In(i) => Some(&i.name),
        TopDecl::Out(o) => Some(&o.name),
        TopDecl::TypeAlias(t) => Some(&t.name),
        _ => None,
    }
}

fn resolve_file(
    path: &str,
    relative_to: &str,
    loader: &dyn FileLoader,
    cache: &mut HashMap<String, ParseResult>,
    stack: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    let canon = loader.canonical_path(path, relative_to);
    if stack.contains(&canon) {
        return None; // cycle — caller emits diagnostic
    }
    if cache.contains_key(&canon) {
        return Some(canon);
    }
    let source = match loader.load(path, relative_to) {
        Ok(s) => s,
        Err(_) => return None,
    };
    stack.insert(canon.clone());
    let parsed = parse(&source, &canon);

    // Recursively resolve imports in the imported file
    let mut imported_ast = parsed.ast.clone();
    let mut sub_imports = Vec::new();
    imported_ast.decls.retain(|d| {
        if let TopDecl::Import(imp) = d {
            sub_imports.push(imp.clone());
            false
        } else {
            true
        }
    });
    // Collect into a separate list and prepend, so this file's own decls stay
    // after the ones it imports — chips/mods register in source order during
    // lowering, so appending would make every call into an imported module a
    // use-before-declaration.
    let mut sub_decls: Vec<TopDecl> = Vec::new();
    for imp in &sub_imports {
        resolve_import(
            imp,
            &canon,
            loader,
            cache,
            stack,
            diagnostics,
            &mut sub_decls,
            &mut HashMap::default(),
        );
    }
    if !sub_decls.is_empty() {
        sub_decls.append(&mut imported_ast.decls);
        imported_ast.decls = sub_decls;
    }

    stack.remove(&canon);
    let mut result = parsed;
    result.ast = imported_ast;
    cache.insert(canon.clone(), result);
    Some(canon)
}

#[allow(clippy::too_many_arguments)]
fn resolve_import(
    imp: &ImportDecl,
    relative_to: &str,
    loader: &dyn FileLoader,
    cache: &mut HashMap<String, ParseResult>,
    stack: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
    target_decls: &mut Vec<TopDecl>,
    target_doc_comments: &mut HashMap<usize, String>,
) {
    let canon = loader.canonical_path(&imp.path, relative_to);
    if stack.contains(&canon) {
        diagnostics.push(Diagnostic::error(
            "WS012",
            format!("circular import: '{}'", imp.path),
            imp.range.clone(),
        ));
        return;
    }

    let canon = match resolve_file(&imp.path, relative_to, loader, cache, stack, diagnostics) {
        Some(c) => c,
        None => {
            diagnostics.push(Diagnostic::error(
                "WS012",
                format!("cannot resolve import '{}'", imp.path),
                imp.range.clone(),
            ));
            return;
        }
    };

    let parsed = cache.get(&canon).unwrap();
    let importable: Vec<TopDecl> = parsed
        .ast
        .decls
        .iter()
        .filter(|d| is_importable(d))
        .cloned()
        .collect();

    // Merge doc comments from imported file
    for (k, v) in &parsed.doc_comments {
        target_doc_comments.insert(*k, v.clone());
    }

    let already_has = |decls: &[TopDecl], name: &str| -> bool {
        decls
            .iter()
            .any(|existing| decl_name(existing) == Some(name))
    };

    match &imp.kind {
        ImportKind::All => {
            for d in importable {
                if let Some(name) = decl_name(&d)
                    && already_has(target_decls, name) {
                        continue;
                    }
                target_decls.push(d);
            }
        }
        ImportKind::Named(bindings) => {
            let binding_names: HashSet<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
            for b in bindings {
                let effective_name = b.alias.as_deref().unwrap_or(&b.name);
                if already_has(target_decls, effective_name) {
                    continue;
                }
                let found = importable.iter().find(|d| decl_name(d) == Some(&b.name));
                match found {
                    Some(d) => {
                        if let Some(alias) = &b.alias {
                            let mut d = d.clone();
                            rename_decl(&mut d, alias);
                            target_decls.push(d);
                        } else {
                            target_decls.push(d.clone());
                        }
                    }
                    None => {
                        diagnostics.push(Diagnostic::error(
                            "WS012",
                            format!("'{}' not found in '{}'", b.name, imp.path),
                            imp.range.clone(),
                        ));
                    }
                }
            }
            // Pull in non-requested declarations that are referenced by
            // the imported ones. Covers both transitive imports (from other
            // files) and same-file helpers (e.g. timer_tick used by
            // timers_advance). Iterates to a fixed point so transitive
            // chains (A calls B calls C) are fully resolved.
            // TypeAlias declarations are NOT pulled — they are inlined below.
            loop {
                let used = collect_runtime_idents_in_decls(target_decls);
                let mut added = false;
                for d in &importable {
                    if matches!(d, TopDecl::TypeAlias(_)) { continue; }
                    if let Some(name) = decl_name(d)
                        && !binding_names.contains(name)
                            && used.contains(name)
                            && !target_decls
                                .iter()
                                .any(|existing| decl_name(existing) == Some(name))
                        {
                            target_decls.push(d.clone());
                            added = true;
                        }
                }
                if !added {
                    break;
                }
            }

            // Inline-expand type aliases in imported declarations' params
            // so the TypeAlias doesn't need to be in the importing scope.
            let type_aliases: HashMap<String, TypeExpr> = importable.iter()
                .filter_map(|d| match d {
                    TopDecl::TypeAlias(t) => Some((t.name.clone(), t.typ.clone())),
                    _ => None,
                })
                .collect();
            if !type_aliases.is_empty() {
                for d in target_decls.iter_mut() {
                    expand_type_aliases_in_decl(d, &type_aliases);
                }
            }
        }
        ImportKind::Namespace(ns_name) => {
            // Module doc: an explicit top-of-file `///` block, else the first
            // declaration's doc comment.
            let module_doc = parsed.ast.module_doc.clone().or_else(|| {
                parsed
                    .ast
                    .decls
                    .first()
                    .and_then(|d| parsed.doc_comments.get(&d.range().start.offset))
                    .cloned()
            });

            target_decls.push(TopDecl::Namespace(NamespaceDecl {
                name: ns_name.clone(),
                decls: importable,
                source_path: imp.path.clone(),
                module_doc,
                range: imp.range.clone(),
            }));
        }
    }
}

fn rename_decl(d: &mut TopDecl, new_name: &str) {
    match d {
        TopDecl::Chip(c) => c.name = new_name.to_string(),
        TopDecl::Fn(f) => f.name = new_name.to_string(),
        TopDecl::Let(l) => {
            if let LetBinding::Ident { name, .. } = &mut l.binding {
                *name = new_name.to_string();
            }
        }
        TopDecl::Event(e) => e.name = new_name.to_string(),
        TopDecl::Var(v) => v.name = new_name.to_string(),
        TopDecl::Array(a) => a.name = new_name.to_string(),
        TopDecl::Buffer(b) => b.name = new_name.to_string(),
        TopDecl::In(i) => i.name = new_name.to_string(),
        TopDecl::Out(o) => o.name = new_name.to_string(),
        TopDecl::TypeAlias(t) => t.name = new_name.to_string(),
        _ => {}
    }
}

pub fn resolve(source: &str, file: &str, loader: &dyn FileLoader) -> ResolveResult {
    let parsed = parse(source, file);
    let mut diagnostics = parsed.diagnostics.clone();
    let mut doc_comments = parsed.doc_comments.clone();

    let mut decls: Vec<TopDecl> = Vec::new();
    let mut main_decls: Vec<TopDecl> = Vec::new();
    let mut cache: HashMap<String, ParseResult> = HashMap::default();
    let mut stack: HashSet<String> = HashSet::default();

    let canon_self = loader.canonical_path(file, ".");
    stack.insert(canon_self.clone());

    for d in &parsed.ast.decls {
        if let TopDecl::Import(imp) = d {
            resolve_import(
                imp,
                file,
                loader,
                &mut cache,
                &mut stack,
                &mut diagnostics,
                &mut decls,
                &mut doc_comments,
            );
        } else {
            main_decls.push(d.clone());
        }
    }

    // Check for unused named imports. An import counts as used when the main
    // file OR another imported declaration references it — imported mods
    // reference their defining module's constants inside their bodies.
    let mut used_idents = collect_idents_in_decls(&main_decls);
    used_idents.extend(collect_idents_in_decls(&decls));
    for d in &parsed.ast.decls {
        if let TopDecl::Import(imp) = d
            && let ImportKind::Named(bindings) = &imp.kind {
                for b in bindings {
                    let check_name = b.alias.as_deref().unwrap_or(&b.name);
                    if !used_idents.contains(check_name) {
                        diagnostics.push(Diagnostic::warning(
                            "WS014",
                            format!("unused import '{}'", check_name),
                            b.range.clone(),
                        ));
                    }
                }
            }
    }

    // Imported declarations come first, then main file declarations
    decls.extend(main_decls);

    ResolveResult {
        ast: Script {
            decls,
            range: parsed.ast.range,
            module_doc: parsed.ast.module_doc.clone(),
            // Module-level @nofold applies to the ENTRY file's compilation
            // (imported decls lower as part of it and are covered too).
            no_fold: parsed.ast.no_fold,
        },
        diagnostics,
        doc_comments,
    }
}

fn collect_runtime_idents_in_decls(decls: &[TopDecl]) -> HashSet<String> {
    let mut idents = HashSet::default();
    for d in decls {
        collect_runtime_idents_in_decl(d, &mut idents);
    }
    idents
}

fn collect_runtime_idents_in_decl(d: &TopDecl, idents: &mut HashSet<String>) {
    match d {
        TopDecl::Handler(h) => collect_runtime_idents_in_block(&h.body, idents),
        TopDecl::AnonChip(ac) => collect_runtime_idents_in_block(&ac.body, idents),
        TopDecl::Chip(c) => collect_runtime_idents_in_block(&c.body, idents),
        TopDecl::Fn(f) => collect_idents_in_expr(&f.body, idents),
        TopDecl::Var(v) => {
            if let Some(e) = &v.init { collect_idents_in_expr(e, idents); }
        }
        TopDecl::Let(l) => collect_idents_in_expr(&l.value, idents),
        TopDecl::Out(o) => {
            if let Some(e) = &o.value { collect_idents_in_expr(e, idents); }
        }
        _ => {}
    }
}

fn collect_runtime_idents_in_block(block: &Block, idents: &mut HashSet<String>) {
    for s in &block.stmts {
        match s {
            Stmt::Assign(a) => {
                collect_idents_in_expr(&a.target, idents);
                collect_idents_in_expr(&a.value, idents);
            }
            Stmt::If(i) => {
                collect_idents_in_expr(&i.cond, idents);
                collect_runtime_idents_in_block(&i.then_block, idents);
                if let Some(eb) = &i.else_block {
                    collect_runtime_idents_in_block(eb, idents);
                }
            }
            Stmt::ExprStmt(es) => collect_idents_in_expr(&es.expr, idents),
            Stmt::Let(l) => collect_idents_in_expr(&l.value, idents),
            Stmt::OutBinding(o) => {
                if let Some(e) = &o.value { collect_idents_in_expr(e, idents); }
            }
            Stmt::Handler(h) => collect_runtime_idents_in_block(&h.body, idents),
            Stmt::Return { value: Some(e), .. } => collect_idents_in_expr(e, idents),
            Stmt::Var(v) => {
                if let Some(e) = &v.init { collect_idents_in_expr(e, idents); }
            }
            Stmt::Emit(e) => {
                if let Some(v) = &e.value { collect_idents_in_expr(v, idents); }
            }
            Stmt::Await(a) => {
                if let Some(v) = &a.value_expr { collect_idents_in_expr(v, idents); }
                collect_idents_in_expr(&a.exec_expr, idents);
            }
            Stmt::Buffer(b) => collect_idents_in_expr(&b.init, idents),
            Stmt::AnonChip(ac) => collect_runtime_idents_in_block(&ac.body, idents),
            Stmt::ChipDecl(c) => collect_runtime_idents_in_block(&c.body, idents),
            _ => {}
        }
    }
}

fn collect_idents_in_decls(decls: &[TopDecl]) -> HashSet<String> {
    let mut idents = HashSet::default();
    for d in decls {
        collect_idents_in_decl(d, &mut idents);
    }
    idents
}

fn collect_idents_in_decl(d: &TopDecl, idents: &mut HashSet<String>) {
    match d {
        TopDecl::Handler(h) => collect_idents_in_block(&h.body, idents),
        TopDecl::AnonChip(ac) => collect_idents_in_block(&ac.body, idents),
        TopDecl::Chip(c) => {
            for p in &c.inputs {
                collect_idents_in_type_expr(&p.typ, idents);
            }
            collect_idents_in_block(&c.body, idents);
        }
        TopDecl::Fn(f) => {
            for p in &f.params {
                collect_idents_in_type_expr(&p.typ, idents);
            }
            collect_idents_in_expr(&f.body, idents);
        }
        TopDecl::Var(v) => {
            if let Some(t) = &v.typ {
                collect_idents_in_type_expr(t, idents);
            }
            if let Some(e) = &v.init {
                collect_idents_in_expr(e, idents);
            }
        }
        TopDecl::Let(l) => {
            if let Some(t) = &l.typ {
                collect_idents_in_type_expr(t, idents);
            }
            collect_idents_in_expr(&l.value, idents);
        }
        TopDecl::Out(o) => {
            if let Some(t) = &o.typ {
                collect_idents_in_type_expr(t, idents);
            }
            if let Some(e) = &o.value {
                collect_idents_in_expr(e, idents);
            }
        }
        TopDecl::Array(a) => {
            collect_idents_in_type_expr(&a.element_type, idents);
        }
        TopDecl::Buffer(b) => {
            if let Some(t) = &b.typ {
                collect_idents_in_type_expr(t, idents);
            }
            collect_idents_in_expr(&b.init, idents);
        }
        TopDecl::In(i) => {
            collect_idents_in_type_expr(&i.typ, idents);
        }
        _ => {}
    }
}

fn collect_idents_in_type_expr(t: &TypeExpr, idents: &mut HashSet<String>) {
    match t {
        TypeExpr::Name { name, .. } => { idents.insert(name.clone()); }
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            collect_idents_in_type_expr(inner, idents);
        }
        TypeExpr::Tuple { fields, .. } => {
            for f in fields { collect_idents_in_type_expr(f, idents); }
        }
        TypeExpr::Record { fields, .. } => {
            for f in fields { collect_idents_in_type_expr(&f.typ, idents); }
        }
        TypeExpr::Union { options, .. } => {
            for o in options { collect_idents_in_type_expr(o, idents); }
        }
    }
}

fn expand_type_aliases_in_decl(d: &mut TopDecl, aliases: &HashMap<String, TypeExpr>) {
    match d {
        TopDecl::Chip(c) => {
            for p in &mut c.inputs { expand_type_aliases_in_type_expr(&mut p.typ, aliases); }
            for o in &mut c.outputs { expand_type_aliases_in_type_expr(&mut o.typ, aliases); }
        }
        TopDecl::Fn(f) => {
            for p in &mut f.params { expand_type_aliases_in_type_expr(&mut p.typ, aliases); }
            if let Some(t) = &mut f.return_type { expand_type_aliases_in_type_expr(t, aliases); }
        }
        TopDecl::Let(l) => {
            if let Some(t) = &mut l.typ { expand_type_aliases_in_type_expr(t, aliases); }
        }
        TopDecl::Var(v) => {
            if let Some(t) = &mut v.typ { expand_type_aliases_in_type_expr(t, aliases); }
        }
        TopDecl::Out(o) => {
            if let Some(t) = &mut o.typ { expand_type_aliases_in_type_expr(t, aliases); }
        }
        TopDecl::Buffer(b) => {
            if let Some(t) = &mut b.typ { expand_type_aliases_in_type_expr(t, aliases); }
        }
        TopDecl::In(i) => {
            expand_type_aliases_in_type_expr(&mut i.typ, aliases);
        }
        _ => {}
    }
}

fn expand_type_aliases_in_type_expr(t: &mut TypeExpr, aliases: &HashMap<String, TypeExpr>) {
    match t {
        TypeExpr::Name { name, .. } => {
            if let Some(expanded) = aliases.get(name.as_str()) {
                *t = expanded.clone();
            }
        }
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            expand_type_aliases_in_type_expr(inner, aliases);
        }
        TypeExpr::Tuple { fields, .. } => {
            for f in fields { expand_type_aliases_in_type_expr(f, aliases); }
        }
        TypeExpr::Record { fields, .. } => {
            for f in fields { expand_type_aliases_in_type_expr(&mut f.typ, aliases); }
        }
        TypeExpr::Union { options, .. } => {
            for o in options { expand_type_aliases_in_type_expr(o, aliases); }
        }
    }
}

fn collect_idents_in_block(block: &Block, idents: &mut HashSet<String>) {
    for s in &block.stmts {
        match s {
            Stmt::Assign(a) => {
                collect_idents_in_expr(&a.target, idents);
                collect_idents_in_expr(&a.value, idents);
            }
            Stmt::If(i) => {
                collect_idents_in_expr(&i.cond, idents);
                collect_idents_in_block(&i.then_block, idents);
                if let Some(eb) = &i.else_block {
                    collect_idents_in_block(eb, idents);
                }
            }
            Stmt::ExprStmt(es) => collect_idents_in_expr(&es.expr, idents),
            Stmt::Let(l) => {
                if let Some(t) = &l.typ {
                    collect_idents_in_type_expr(t, idents);
                }
                collect_idents_in_expr(&l.value, idents);
            }
            Stmt::OutBinding(o) => {
                if let Some(e) = &o.value {
                    collect_idents_in_expr(e, idents);
                }
            }
            Stmt::Handler(h) => collect_idents_in_block(&h.body, idents),
            Stmt::Return { value: Some(e), .. } => collect_idents_in_expr(e, idents),
            Stmt::Var(v) => {
                if let Some(t) = &v.typ {
                    collect_idents_in_type_expr(t, idents);
                }
                if let Some(e) = &v.init {
                    collect_idents_in_expr(e, idents);
                }
            }
            Stmt::AnonChip(ac) => collect_idents_in_block(&ac.body, idents),
            Stmt::ChipDecl(c) => {
                for p in &c.inputs {
                    collect_idents_in_type_expr(&p.typ, idents);
                }
                collect_idents_in_block(&c.body, idents);
            }
            _ => {}
        }
    }
}

fn collect_idents_in_expr(e: &Expr, idents: &mut HashSet<String>) {
    match e {
        Expr::Ident { name, .. } => {
            idents.insert(name.clone());
        }
        Expr::BinOp { left, right, .. } => {
            collect_idents_in_expr(left, idents);
            collect_idents_in_expr(right, idents);
        }
        Expr::UnOp { operand, .. } => collect_idents_in_expr(operand, idents),
        Expr::Call { callee, args, .. } => {
            collect_idents_in_expr(callee, idents);
            for a in args {
                match a {
                    CallArg::Positional(e) | CallArg::Named { value: e, .. } | CallArg::Spread(e) => {
                        collect_idents_in_expr(e, idents)
                    }
                }
            }
        }
        Expr::FieldAccess { obj, .. } => collect_idents_in_expr(obj, idents),
        Expr::IndexAccess { obj, index, .. } => {
            collect_idents_in_expr(obj, idents);
            collect_idents_in_expr(index, idents);
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_idents_in_expr(cond, idents);
            collect_idents_in_expr(then_branch, idents);
            collect_idents_in_expr(else_branch, idents);
        }
        Expr::InterpLit { parts, .. } => {
            for p in parts {
                if let InterpPart::Expr(e) = p {
                    collect_idents_in_expr(e, idents);
                }
            }
        }
        Expr::RecordLit { fields, .. } => {
            for f in fields {
                match f {
                    RecordLitField::Named { value, .. } => collect_idents_in_expr(value, idents),
                    // Shorthand `{ name }` references an identifier by that name.
                    RecordLitField::Shorthand { name, .. } => {
                        idents.insert(name.clone());
                    }
                    RecordLitField::Spread { value, .. } => collect_idents_in_expr(value, idents),
                }
            }
        }
        Expr::Array { elements, .. } => {
            for el in elements {
                match el {
                    ArrayElem::Item(e) | ArrayElem::Spread(e) => collect_idents_in_expr(e, idents),
                }
            }
        }
        Expr::BlockExpr { stmts, value, .. } => {
            let tmp_block = Block {
                stmts: stmts.clone(),
                range: SourceRange::default(),
            };
            collect_idents_in_block(&tmp_block, idents);
            collect_idents_in_expr(value, idents);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(files: &[(&str, &str)]) -> MemLoader {
        MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn import_all() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "foo"))
        );
    }

    #[test]
    fn import_named() {
        let loader = mem(&[(
            "lib.ws",
            "mod foo(x: *int) { x = x + 1 }\nmod bar(x: *int) { x = x - 1 }",
        )]);
        let r = resolve(r#"import { foo } from "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "foo"))
        );
        assert!(
            !r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "bar"))
        );
    }

    #[test]
    fn import_alias() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import { foo as inc } from "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "inc"))
        );
    }

    #[test]
    fn import_namespace() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import * as myLib from "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Namespace(n) if n.name == "myLib"))
        );
    }

    #[test]
    fn circular_import_error() {
        let loader = mem(&[("a.ws", r#"import "b""#), ("b.ws", r#"import "a""#)]);
        let r = resolve(r#"import "a""#, "main.ws", &loader);
        assert!(r.diagnostics.iter().any(|d| d.message.contains("circular")));
    }

    #[test]
    fn missing_file_error() {
        let loader = mem(&[]);
        let r = resolve(r#"import "nonexistent""#, "main.ws", &loader);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("cannot resolve"))
        );
    }

    #[test]
    fn missing_symbol_error() {
        let loader = mem(&[("lib.ws", "mod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import { bar } from "lib""#, "main.ws", &loader);
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("not found"))
        );
    }

    #[test]
    fn var_and_mod_both_importable() {
        let loader = mem(&[("lib.ws", "var x: int = 0\nmod foo(x: *int) { x = x + 1 }")]);
        let r = resolve(r#"import "lib""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(r.ast.decls.iter().any(|d| matches!(d, TopDecl::Var(v) if v.name == "x")));
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Chip(c) if c.name == "foo"))
        );
    }

    #[test]
    fn implicit_ws_extension() {
        let loader = mem(&[("utils.ws", "fn double(x: int) -> int = x * 2")]);
        let r = resolve(r#"import "utils""#, "main.ws", &loader);
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        assert!(
            r.ast
                .decls
                .iter()
                .any(|d| matches!(d, TopDecl::Fn(f) if f.name == "double"))
        );
    }

    #[test]
    fn type_import_used_in_param_not_unused() {
        let loader = mem(&[(
            "types.ws",
            "type Cpu = { regs: int[], cpsr: *int }",
        )]);
        let r = resolve(
            "import { Cpu } from \"types\"\nmod foo(cpu: Cpu) { cpu.regs.push(0) }",
            "main.ws",
            &loader,
        );
        let ws014: Vec<_> = r.diagnostics.iter().filter(|d| d.code == "WS014").collect();
        assert!(
            ws014.is_empty(),
            "type used in param annotation should not trigger unused import: {:?}",
            ws014
        );
    }

    #[test]
    fn import_var_alias_renames() {
        let loader = mem(&[("lib.ws", "var counter: int = 0")]);
        let r = resolve(
            "import { counter as cnt } from \"lib\"",
            "main.ws",
            &loader,
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}", r.diagnostics
        );
        assert!(
            r.ast.decls.iter().any(|d| matches!(d, TopDecl::Var(v) if v.name == "cnt")),
            "var should be renamed to 'cnt'"
        );
    }

    #[test]
    fn transitive_import_decls_precede_importing_files_own_decls() {
        // A file's own declarations must come AFTER the ones it imports, at
        // every level of the import graph — chips/mods register in source
        // order during lowering, so the reverse ordering makes a call to a
        // transitively imported helper a use-before-declaration (WS021).
        let loader = mem(&[
            ("util.ws", "mod helper(x: *int) { x = x + 1 }"),
            (
                "game.ws",
                "import \"util\"\nmod game_step(x: *int) { helper(x) }",
            ),
            ("container.ws", "import \"game\""),
        ]);
        let r = resolve(
            "import \"container\"\nvar n: int = 0\nin go: exec\non go { game_step(n) }",
            "main.ws",
            &loader,
        );
        assert!(
            !r.diagnostics
                .iter()
                .any(|d| d.severity == crate::diagnostic::Severity::Error),
            "errors: {:?}",
            r.diagnostics
        );
        let pos = |name: &str| {
            r.ast
                .decls
                .iter()
                .position(|d| decl_name(d) == Some(name))
                .unwrap_or_else(|| panic!("'{name}' missing from resolved decls"))
        };
        assert!(
            pos("helper") < pos("game_step"),
            "'helper' must precede its caller 'game_step'; decls: {:?}",
            r.ast.decls.iter().filter_map(decl_name).collect::<Vec<_>>()
        );
        let tc = crate::typecheck::typecheck(&r.ast, "main.ws");
        assert!(
            !tc.diagnostics.iter().any(|d| d.code == "WS021"),
            "transitive import must not produce use-before-declaration: {:?}",
            tc.diagnostics
        );
    }

    #[test]
    fn type_alias_not_leaked_transitively() {
        let loader = mem(&[
            ("types.ws", "type Cpu = { regs: int[], cpsr: *int }"),
            ("cpu.ws", "import { Cpu } from \"types\"\nmod cpu_init(cpu: Cpu) { cpu.regs.push(0) }"),
        ]);
        let r = resolve(
            "import { cpu_init } from \"cpu\"",
            "main.ws",
            &loader,
        );
        let has_type_alias = r.ast.decls.iter().any(|d| matches!(d, TopDecl::TypeAlias(t) if t.name == "Cpu"));
        assert!(
            !has_type_alias,
            "TypeAlias 'Cpu' should NOT be pulled transitively into the importing file's AST"
        );
    }
}

#[cfg(test)]
mod dep_pull_tests {
    use super::*;

    fn mem(files: &[(&str, &str)]) -> MemLoader {
        MemLoader {
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn record_let_pulls_array_deps_and_expands_let_annotation() {
        // Importing a record-of-arrays `let` must pull the arrays its
        // initializer references (named and shorthand fields) and inline the
        // alias in the let's annotation; emits inside the chip body must pull
        // the mods/constants they reference.
        let loader = mem(&[(
            "lib.ws",
            "let X = 7\n\
             type Tables = { vals: int[] }\n\
             array vals: int[]\n\
             let TB: Tables = { vals: vals }\n\
             mod bump(tables: Tables, v: int) {\n  tables.vals.push(v + X)\n}\n\
             chip Init(init: exec, tables: Tables) -> (code: int) {\n  on init {\n    bump(tables, X)\n    emit code = X\n  }\n}\n",
        )]);
        let src = "import { Init, TB } from \"lib\"\nin reset: exec\nlet R = Init(reset, TB)\nout v = R.code";
        let r = resolve(src, "main.ws", &loader);
        assert!(
            r.diagnostics.is_empty(),
            "resolve diags: {:?}",
            r.diagnostics
        );
        let tc = crate::typecheck::typecheck(&r.ast, "main.ws");
        let errors: Vec<_> = tc
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "typecheck errors: {errors:?}");
    }
}
