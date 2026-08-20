use std::sync::Arc;

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
    /// The ENTRY file's source map. Imported files contribute declarations
    /// but no line geometry — same rule as `@layout("code")` itself, which
    /// is only read off the entry file. Shared behind an `Arc` because the
    /// layout options that carry it are cloned per chip.
    pub source_map: Arc<SourceMap>,
    /// Canonical paths of every file this resolve loaded, TRANSITIVELY (an
    /// import's own imports included) — the entry file is not listed. Lets a
    /// caller tell whether a given file feeds this one, e.g. so the LSP
    /// re-analyzes only the open documents a changed file actually reaches
    /// instead of every open document on every keystroke.
    pub imported_files: Vec<String>,
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
            // A top-level `on` handler in an imported file runs as part of the
            // importing program (a library that installs behaviour). Without
            // this the handler was dropped, and an `on <expr>` handler's
            // desugared `let _on_expr_N = <expr>` (a Let, which IS importable)
            // still leaked in — a dangling trigger gate with no body.
            | TopDecl::Handler(_)
    )
}

/// Every top-level name a declaration introduces.
///
/// [`decl_name`] answers "what is this declaration CALLED", which is a
/// different question for the one form that binds several names at once: a
/// destructuring `let`/`const` (`const { x, y } = p`) has no single name, so
/// `decl_name` returns `None` for it — and every import path keyed on
/// `decl_name` therefore behaved as though the declaration introduced NOTHING.
/// A named import of `x` reported WS012 "not found", a duplicate check could
/// not see `x`, and the declaration-order restore sorted the binding to the
/// end. (`import "lib"` was unaffected: it pushes every importable
/// declaration and only consults `decl_name` to skip duplicates.)
///
/// Uses `const_eval::bound_names` — the same syntactic name list the constant
/// environment and both typecheck sites split a destructured value with — so
/// the import system cannot disagree with them about which names a binding
/// form introduces.
fn decl_names(d: &TopDecl) -> Vec<String> {
    match d {
        TopDecl::Let(l) => crate::const_eval::bound_names(&l.binding),
        _ => decl_name(d).map(|n| vec![n.to_string()]).unwrap_or_default(),
    }
}

/// Whether `d` introduces the top-level name `name`.
fn decl_binds(d: &TopDecl, name: &str) -> bool {
    decl_names(d).iter().any(|n| n == name)
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
        TopDecl::Namespace(n) => Some(&n.name),
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
    // The imported file's own `import * as Ns` bindings. These are not
    // importable by name, but a declaration we pull in may call through one
    // (`Ns.helper()`), so they travel with it — see the closure pass below.
    let source_namespaces: Vec<TopDecl> = parsed
        .ast
        .decls
        .iter()
        .filter(|d| matches!(d, TopDecl::Namespace(_)))
        .cloned()
        .collect();

    // Merge doc comments from imported file
    for (k, v) in &parsed.doc_comments {
        target_doc_comments.insert(*k, v.clone());
    }

    let already_has =
        |decls: &[TopDecl], name: &str| -> bool { decls.iter().any(|e| decl_binds(e, name)) };

    match &imp.kind {
        ImportKind::All => {
            for d in importable {
                if decl_names(&d)
                    .iter()
                    .any(|n| already_has(target_decls, n))
                {
                    continue;
                }
                target_decls.push(d);
            }
        }
        ImportKind::Named(bindings) => {
            let binding_names: HashSet<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
            // Everything this import contributes goes after here; the closure
            // pass below pulls dependencies in discovery order, which is not
            // declaration order, so the block is re-sorted at the end.
            let import_start = target_decls.len();
            for b in bindings {
                let effective_name = b.alias.as_deref().unwrap_or(&b.name);
                if already_has(target_decls, effective_name) {
                    continue;
                }
                let found = importable.iter().find(|d| decl_binds(d, &b.name));
                match found {
                    Some(d) => {
                        if let Some(alias) = &b.alias {
                            let mut d = d.clone();
                            // A declaration binding SEVERAL names needs to know
                            // WHICH one the alias renames; `rename_decl` only
                            // knows how to rename a single-name declaration.
                            if !rename_bound_name(&mut d, &b.name, alias) {
                                diagnostics.push(Diagnostic::error(
                                    "WS012",
                                    format!(
                                        "'{}' cannot be imported under an alias: it is one of \
                                         several names bound by a destructuring declaration in \
                                         '{}' — import it without `as`, or give the destructure \
                                         its own alias at the declaration",
                                        b.name, imp.path
                                    ),
                                    imp.range.clone(),
                                ));
                                continue;
                            }
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
                    let names = decl_names(d);
                    if !names.is_empty()
                        && names.iter().any(|n| used.contains(n.as_str()))
                        && !names.iter().any(|n| binding_names.contains(n.as_str()))
                        && !names
                            .iter()
                            .any(|n| target_decls.iter().any(|e| decl_binds(e, n)))
                    {
                        target_decls.push(d.clone());
                        added = true;
                    }
                }
                if !added {
                    break;
                }
            }
            // Restore the provider's own declaration order across everything
            // this import contributed. The closure pass appends dependencies
            // AFTER the declaration that needed them, but a top-level `let` is
            // declared as it is checked — so a constant discovered via an array
            // initializer would land after the array and read as undeclared.
            // The source file already orders constants before their users.
            let mut order: HashMap<String, usize> = HashMap::default();
            for (i, d) in importable.iter().enumerate() {
                for n in decl_names(d) {
                    order.entry(n).or_insert(i);
                }
            }
            target_decls[import_start..].sort_by_key(|d| {
                decl_names(d)
                    .iter()
                    .filter_map(|n| order.get(n).copied())
                    .min()
                    .unwrap_or(usize::MAX)
            });

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

            // Inline the module's own type aliases into its declarations, as a
            // named import already does. A namespaced `mod f() -> MyType` is
            // used from the importing module, where `MyType` is not in scope —
            // leaving the name unexpanded fails there with "unknown type".
            let mut decls = importable;
            let type_aliases: HashMap<String, TypeExpr> = decls
                .iter()
                .filter_map(|d| match d {
                    TopDecl::TypeAlias(t) => Some((t.name.clone(), t.typ.clone())),
                    _ => None,
                })
                .collect();
            if !type_aliases.is_empty() {
                for d in decls.iter_mut() {
                    expand_type_aliases_in_decl(d, &type_aliases);
                }
            }

            target_decls.push(TopDecl::Namespace(NamespaceDecl {
                name: ns_name.clone(),
                decls,
                source_path: imp.path.clone(),
                module_doc,
                range: imp.range.clone(),
            }));
        }
    }

    // Carry along any namespace the pulled-in declarations call through.
    // Without this an imported `mod` whose body says `Ns.helper()` loses `Ns`
    // entirely: the call resolves to nothing and silently lowers to an
    // `_Unsupported` placeholder that does nothing at runtime. Only referenced
    // namespaces travel, so importing a file does not leak its every import.
    if !source_namespaces.is_empty() {
        loop {
            let used = collect_runtime_idents_in_decls(target_decls);
            let mut added = false;
            for ns in &source_namespaces {
                if let Some(name) = decl_name(ns)
                    && used.contains(name)
                    && !already_has(target_decls, name)
                {
                    // Prepend: lowering registers declarations in source order,
                    // so a namespace appended after its caller would read as a
                    // use before declaration and resolve to nothing.
                    target_decls.insert(0, ns.clone());
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
    }
}

/// Rename the single bound name `old` to `new_name` inside `d`, for
/// `import { old as new_name }`. Returns `false` when the declaration binds
/// `old` but cannot express the rename — the caller turns that into a
/// diagnostic rather than pushing a declaration that silently still binds the
/// original name.
///
/// A `RecordDestruct` field renames through its own `alias` slot, which is
/// exactly what that slot means. A `Record` (`const { x, y } = p`) cannot:
/// its names ARE the field names it reads, so renaming one would change which
/// field it binds, and a `Tuple`'s `rest` has no positional identity to keep.
fn rename_bound_name(d: &mut TopDecl, old: &str, new_name: &str) -> bool {
    let TopDecl::Let(l) = d else {
        rename_decl(d, new_name);
        return true;
    };
    match &mut l.binding {
        LetBinding::Ident { name, .. } => {
            *name = new_name.to_string();
            true
        }
        LetBinding::Tuple { names, .. } => {
            for n in names.iter_mut().filter(|n| *n == old) {
                *n = new_name.to_string();
                return true;
            }
            false
        }
        LetBinding::RecordDestruct { fields, .. } => {
            for f in fields.iter_mut() {
                if let RecordDestructField::Named { name, alias, .. } = f
                    && alias.as_deref().unwrap_or(name) == old
                {
                    *alias = Some(new_name.to_string());
                    return true;
                }
            }
            false
        }
        LetBinding::Record { .. } => false,
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
    resolve_parsed(parse(source, file), file, loader)
}

/// `resolve` for a caller that ALREADY parsed the entry file. The LSP keeps the
/// pre-resolve AST for local analysis and used to parse the same buffer a second
/// time in here on every keystroke — measured at ~21% of a keystroke on a
/// 2.5k-line file. Consumes the `ParseResult`; clone it first if you need it.
pub fn resolve_parsed(parsed: ParseResult, file: &str, loader: &dyn FileLoader) -> ResolveResult {
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
            // Module-level @nofold/@fold apply to the ENTRY file's
            // compilation (imported decls lower as part of it and are
            // covered too) — an @fold/@nofold in an IMPORTED file is never
            // consulted here, so it's inert.
            no_fold: parsed.ast.no_fold,
            fold: parsed.ast.fold,
            // Same rule as @fold/@nofold above: only the entry file's
            // @layout is consulted; an imported file's is inert.
            layout: parsed.ast.layout,
            // Same rule again — the entry file decides whether the whole
            // program flattens; an imported file's @flat is inert.
            flat: parsed.ast.flat,
            // Same rule again — only the entry file's @invisible hides the
            // emitted shell; an imported file's is inert.
            invisible: parsed.ast.invisible,
            // Only the entry file's module `@label` labels the root chip; an
            // imported file's is inert. Cloned so the entry expr survives.
            module_label: parsed.ast.module_label.clone(),
        },
        diagnostics,
        doc_comments,
        source_map: Arc::new(parsed.source_map),
        // `cache` is keyed by canonical path and filled by `resolve_import` as
        // it walks imports depth-first, so its keys are exactly the transitive
        // import set.
        imported_files: cache.into_keys().collect(),
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
        // An array's initializer may name top-level constants
        // (`var teams: int[] = [T_RED, T_BLUE]`). Those constants have to
        // travel with the array through a named import, or the importing file
        // sees the array but not the values it is built from.
        TopDecl::Array(a) => {
            for el in &a.init {
                collect_idents_in_expr(el.expr(), idents);
            }
        }
        TopDecl::Out(o) => {
            if let Some(e) = &o.value { collect_idents_in_expr(e, idents); }
        }
        // A namespace's own declarations can call through a second namespace,
        // so its body counts as a use site for the closure pass.
        TopDecl::Namespace(n) => {
            for d in &n.decls {
                collect_runtime_idents_in_decl(d, idents);
            }
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
        TopDecl::Handler(h) => {
            // The event's CONFIG args count as uses: `on Clock(interval = TICK)`
            // reads TICK to configure the gate, not in the body. Missing these
            // reported the import unused (WS014) — and Organize Imports would
            // then delete it, breaking the handler it configures.
            for arg in &h.config {
                match arg {
                    HandlerConfigArg::Positional(e) => collect_idents_in_expr(e, idents),
                    HandlerConfigArg::Named { value, .. } => collect_idents_in_expr(value, idents),
                }
            }
            // A capture param may annotate its type with an imported alias
            // (`on CustomEvent("x") -> (v: SomeType)`).
            for p in &h.params {
                if let Some(t) = &p.ty {
                    collect_idents_in_type_expr(t, idents);
                }
            }
            collect_idents_in_block(&h.body, idents);
        }
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
            // The initializer counts too: an element may name a top-level `let`
            // constant (`var mask: int[] = [1 << C_FLAG]`). Missing these
            // would report the import unused — and Organize Imports would then
            // delete it, silently breaking the table it feeds.
            for el in &a.init {
                collect_idents_in_expr(el.expr(), idents);
            }
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
        TypeExpr::Generic { args, .. } => {
            for a in args { collect_idents_in_type_expr(a, idents); }
        }
    }
}

fn expand_type_aliases_in_decl(d: &mut TopDecl, aliases: &HashMap<String, TypeExpr>) {
    match d {
        // An alias body may name another alias (`type Rect = { a: Point, b: Point }`).
        // Expanding it here makes the alias self-contained, which is what a
        // namespace import needs: the module's aliases are declared to the
        // importer only under their qualified name (`Ns.Rect`), so a bare
        // `Point` surviving inside the body has nothing to resolve against.
        // Seeded with its own name so a self-referential alias stops after one
        // level instead of expanding forever.
        TopDecl::TypeAlias(t) => {
            let mut active = vec![t.name.clone()];
            expand_aliases(&mut t.typ, aliases, &mut active);
        }
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
    expand_aliases(t, aliases, &mut Vec::new());
}

/// Substitute alias names by their bodies, *including inside a body just
/// substituted in* — an alias whose body names another alias (`type Rect = {
/// a: Point, b: Point }`) otherwise leaves the inner name behind, unresolvable
/// in the importing module, and the field silently types as `any`.
///
/// `active` is the chain of aliases being substituted on the current path. A
/// name already on it is left alone, so a self-referential or mutually
/// recursive alias expands one level and stops rather than looping forever.
fn expand_aliases(t: &mut TypeExpr, aliases: &HashMap<String, TypeExpr>, active: &mut Vec<String>) {
    match t {
        TypeExpr::Name { name, .. } => {
            if active.iter().any(|a| a == name) {
                return;
            }
            let Some(mut body) = aliases.get(name.as_str()).cloned() else {
                return;
            };
            active.push(name.clone());
            expand_aliases(&mut body, aliases, active);
            active.pop();
            *t = body;
        }
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            expand_aliases(inner, aliases, active);
        }
        TypeExpr::Tuple { fields, .. } => {
            for f in fields { expand_aliases(f, aliases, active); }
        }
        TypeExpr::Record { fields, .. } => {
            for f in fields { expand_aliases(&mut f.typ, aliases, active); }
        }
        TypeExpr::Union { options, .. } => {
            for o in options { expand_aliases(o, aliases, active); }
        }
        TypeExpr::Generic { args, .. } => {
            for a in args { expand_aliases(a, aliases, active); }
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
            // Same treatment as a top-level handler: the CONFIG args and the
            // capture param types count as uses, not just the body. A handler
            // nested in a chip body reaches this arm instead of the TopDecl
            // one, so omitting them here reported `on CustomEvent(CH)` inside a
            // chip as an unused import of CH — and Organize Imports would then
            // delete it, breaking the handler it names.
            Stmt::Handler(h) => {
                for arg in &h.config {
                    match arg {
                        HandlerConfigArg::Positional(e) => collect_idents_in_expr(e, idents),
                        HandlerConfigArg::Named { value, .. } => {
                            collect_idents_in_expr(value, idents)
                        }
                    }
                }
                for p in &h.params {
                    if let Some(t) = &p.ty {
                        collect_idents_in_type_expr(t, idents);
                    }
                }
                collect_idents_in_block(&h.body, idents);
            }
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
        Expr::MapLit { entries, .. } => {
            for e in entries {
                collect_idents_in_expr(&e.key, idents);
                collect_idents_in_expr(&e.value, idents);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod dep_pull_tests;
