use std::sync::Arc;

use crate::collections::{HashMap, HashSet};

use crate::ast::walk;
use crate::ast::*;
use crate::diagnostic::{Diagnostic, Suppressions};
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
    pub doc_comments: crate::parser::DocComments,
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
    /// `// ws-ignore-line` / `// ws-ignore-file` directives from EVERY file
    /// this resolve read, the entry file included. `diagnostics` below is
    /// already filtered through them; the later stages (typecheck, lowering,
    /// cycle analysis) report against the same files, so their diagnostics go
    /// through [`Suppressions::apply`] at whichever point a caller aggregates
    /// them.
    pub suppressions: Suppressions,
}

/// Collect one file's `ws-ignore` directives off the comments its lex already
/// recorded.
fn collect_suppressions(into: &mut Suppressions, sm: &SourceMap) {
    for c in &sm.comments {
        into.add_comment(&sm.file, c.line, c.own_line, &c.text);
    }
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
            // An `enum` is a type declaration like a `type` alias, and a
            // library that hands back one of its variants is useless without
            // it.
            | TopDecl::Enum(_)
            // A top-level `on` handler in an imported file runs as part of the
            // importing program (a library that installs behaviour). Without
            // this the handler would be dropped, and an `on <expr>` handler's
            // desugared `let _on_expr_N = <expr>` (a Let, which IS importable)
            // would still leak in — a dangling trigger gate with no body.
            | TopDecl::Handler(_)
            // An anonymous `chip { … }` (including the `chip on t { … }`
            // desugar) in an imported library installs behaviour just like a
            // top-level handler. Without this it would be filtered out before
            // the merge and silently dropped, taking its writes with it.
            | TopDecl::AnonChip(_)
    )
}

/// A declaration that installs behaviour by existing rather than by being
/// named: a top-level `on` handler, or an anonymous `chip { ... }`. Importing a
/// library at all brings these in, there is no name to select them by, and
/// the whole point of such a library is that it runs.
fn installs_behaviour(d: &TopDecl) -> bool {
    matches!(d, TopDecl::Handler(_) | TopDecl::AnonChip(_))
}

/// Every top-level name a declaration introduces.
///
/// [`decl_name`] answers "what is this declaration CALLED", which is a
/// different question for the one form that binds several names at once: a
/// destructuring `let`/`const` (`const { x, y } = p`) has no single name, so
/// `decl_name` returns `None` for it — and every import path keyed on
/// `decl_name` alone would behave as though the declaration introduced
/// NOTHING: a named import of `x` would report WS012 "not found", a
/// duplicate check couldn't see `x`, and the declaration-order restore would
/// sort the binding to the end. (`import "lib"` is unaffected: it pushes
/// every importable declaration and only consults `decl_name` to skip
/// duplicates.)
///
/// Uses `const_eval::bound_names` — the same syntactic name list the constant
/// environment and both typecheck sites split a destructured value with — so
/// the import system cannot disagree with them about which names a binding
/// form introduces.
fn decl_names(d: &TopDecl) -> Vec<String> {
    match d {
        TopDecl::Let(l) => crate::const_eval::bound_names(&l.binding),
        _ => decl_name(d)
            .map(|n| vec![n.to_string()])
            .unwrap_or_default(),
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
        TopDecl::Enum(e) => Some(&e.name),
        TopDecl::Namespace(n) => Some(&n.name),
        _ => None,
    }
}

/// The accumulated import-merged declaration list, with the two indexes the
/// per-import passes query: `used` is the union of every runtime identifier
/// referenced by the decls so far (what [`collect_idents`] with
/// [`Uses::Code`] would recompute from scratch per fixpoint sweep), and `bound` maps each bound
/// top-level name to the source files that bind it (what the diamond-dedup
/// checks rescanned the whole list for). Declarations are only ever added, so
/// both indexes update monotonically on `push`/`insert_front`.
#[derive(Default)]
struct DeclAccum {
    decls: Vec<TopDecl>,
    used: HashSet<String>,
    bound: HashMap<String, Vec<Arc<str>>>,
    /// Source positions of the declarations already merged. A handler or
    /// anonymous chip binds no name, so the by-name diamond dedup cannot see
    /// it, and a module reached twice would install its behaviour twice; a
    /// file may declare a name twice, and its second declaration is not the
    /// first reached again.
    installed: HashSet<(Arc<str>, usize)>,
    /// Each declaration merged, with each name it was merged under: one
    /// imported under an alias is not merged under its own name.
    merged: HashSet<(Arc<str>, usize, String)>,
}

impl DeclAccum {
    fn index(&mut self, d: &TopDecl) {
        collect_idents(walk::Node::Decl(d), Uses::Code, &mut self.used);
        let file = d.range().file.clone();
        for n in decl_names(d) {
            self.bound.entry(n.clone()).or_default().push(file.clone());
            self.merged
                .insert((file.clone(), d.range().start.offset, n));
        }
        self.installed.insert((file, d.range().start.offset));
    }

    /// Whether the declaration `d` has already been merged (the same
    /// declaration of the same file, reached by another import edge).
    fn already_installed(&self, d: &TopDecl) -> bool {
        self.installed
            .contains(&(d.range().file.clone(), d.range().start.offset))
    }
    fn push(&mut self, d: TopDecl) {
        self.index(&d);
        self.decls.push(d);
    }
    fn insert_front(&mut self, d: TopDecl) {
        self.index(&d);
        self.decls.insert(0, d);
    }
    /// `decls.iter().any(|e| decl_binds(e, name) && e.range().file == file)`.
    fn binds_in_file(&self, name: &str, file: &Arc<str>) -> bool {
        self.bound
            .get(name)
            .is_some_and(|files| files.iter().any(|f| f == file))
    }

    /// Whether `d`, which binds `names`, has been merged under them already:
    /// the same declaration reached by another import path. A second
    /// declaration of a name in the same file is not, nor the declaration
    /// merged under an alias only.
    fn merged_as(&self, d: &TopDecl, names: &[String]) -> bool {
        let (file, at) = decl_id(d);
        names
            .iter()
            .any(|n| self.merged.contains(&(file.clone(), at, n.clone())))
    }
}

/// A declaration's identity in [`ImportScope`]: its file and start offset.
fn decl_id(d: &TopDecl) -> (Arc<str>, usize) {
    (d.range().file.clone(), d.range().start.offset)
}

/// The canonical module an `import * as` alias names.
///
/// Resolved against the file the alias was WRITTEN in, not the file being
/// imported into: a namespace that travelled here with a pulled-in declaration
/// carries its own module's spelling of the path, which says nothing about
/// where the importing file sits on disk.
fn namespace_target(n: &NamespaceDecl, loader: &dyn FileLoader) -> String {
    loader.canonical_path(&n.source_path, &n.range.file)
}

/// The namespace alias already merged under `name`, from any file.
fn bound_namespace<'a>(decls: &'a [TopDecl], name: &str) -> Option<&'a NamespaceDecl> {
    decls.iter().find_map(|d| match d {
        TopDecl::Namespace(n) if n.name == name => Some(n),
        _ => None,
    })
}

fn resolve_file(
    path: &str,
    relative_to: &str,
    loader: &dyn FileLoader,
    cache: &mut HashMap<String, ParseResult>,
    stack: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
    scope: &mut ImportScope,
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
    // Surface the imported file's OWN parse diagnostics. Without this a parse
    // error in an imported module would be silently swallowed — it would
    // compile clean while the identical source errored as an entry file. Each
    // file is parsed exactly once (the cache check above returns before
    // re-parsing), so these are collected once; the ranges point into the
    // imported file.
    diagnostics.extend(parsed.diagnostics.iter().cloned());

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
    let mut sub_decls = DeclAccum::default();
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
            scope,
        );
    }
    if !sub_decls.decls.is_empty() {
        let mut merged = sub_decls.decls;
        merged.append(&mut imported_ast.decls);
        imported_ast.decls = merged;
    }
    let own_aliases = imported_ast
        .decls
        .iter()
        .filter_map(|d| match d {
            TopDecl::TypeAlias(_) => Some(d.clone()),
            TopDecl::Namespace(ns) => Some(TopDecl::Namespace(NamespaceDecl {
                name: ns.name.clone(),
                decls: ns
                    .decls
                    .iter()
                    .filter(|m| matches!(m, TopDecl::TypeAlias(_)))
                    .cloned()
                    .collect(),
                source_path: ns.source_path.clone(),
                module_doc: None,
                range: ns.range.clone(),
            })),
            _ => None,
        })
        .collect();
    scope.aliases.insert(Arc::from(canon.as_str()), own_aliases);

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
    target: &mut DeclAccum,
    target_doc_comments: &mut crate::parser::DocComments,
    scope: &mut ImportScope,
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

    let canon = match resolve_file(
        &imp.path,
        relative_to,
        loader,
        cache,
        stack,
        diagnostics,
        scope,
    ) {
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
    // Borrowed, not cloned: a named import picks a handful of names out of a
    // module and would otherwise deep-clone every declaration in it first.
    // Each arm below clones only what it actually keeps.
    let importable: Vec<&TopDecl> = parsed
        .ast
        .decls
        .iter()
        .filter(|d| is_importable(d))
        .collect();
    // The imported file's own `import * as Ns` bindings. These are not
    // importable by name, but a declaration we pull in may call through one
    // (`Ns.helper()`), so they travel with it — see the closure pass below.
    let source_namespaces: Vec<&TopDecl> = parsed
        .ast
        .decls
        .iter()
        .filter(|d| matches!(d, TopDecl::Namespace(_)))
        .collect();

    for (k, v) in &parsed.doc_comments {
        target_doc_comments.insert(k.clone(), v.clone());
    }

    // What the module itself can name: its own declarations, and what its own
    // imports named. A private helper a named import pulled in beside one of
    // its declarations is present but not nameable, there or here.
    let module_file: Arc<str> = Arc::from(canon.as_str());
    let module_names = |d: &TopDecl| {
        d.range().file == module_file
            || scope
                .named
                .get(&module_file)
                .is_some_and(|s| s.contains(&decl_id(d)))
    };
    let named_here: Vec<(Arc<str>, usize)> = match &imp.kind {
        ImportKind::All => importable
            .iter()
            .filter(|d| module_names(d))
            .map(|d| decl_id(d))
            .collect(),
        ImportKind::Named(bindings) => bindings
            .iter()
            .filter_map(|b| {
                importable
                    .iter()
                    .rfind(|d| decl_binds(d, &b.name))
                    .map(|d| decl_id(d))
            })
            .collect(),
        ImportKind::Namespace(_) => Vec::new(),
    };
    scope
        .named
        .entry(imp.range.file.clone())
        .or_default()
        .extend(named_here);
    scope
        .files
        .entry(imp.range.file.clone())
        .or_default()
        .insert(module_file.clone());

    match &imp.kind {
        ImportKind::All => {
            for d in importable {
                // Diamond-import dedup: skip a decl already merged under its
                // names (the same declaration of a module reached via two
                // import paths). A same-named decl from a DIFFERENT file is a
                // genuine collision in the flat merged scope — keep it so the
                // duplicate-name check (WS013) fires, rather than silently
                // dropping one and aliasing the two onto a single storage gate.
                // A second declaration of the name in the SAME file is kept
                // too: the module reads its names by position (WS108).
                let is_diamond = if installs_behaviour(d) {
                    target.already_installed(d)
                } else {
                    target.merged_as(d, &decl_names(d))
                };
                if is_diamond {
                    continue;
                }
                target.push(d.clone());
            }
        }
        ImportKind::Named(bindings) => {
            // The aliases this import has ALREADY contributed, so the closure
            // pass below does not push a module declaration spelled like one
            // of them. Keyed on the effective (post-`as`) name, because that is
            // what was pushed: keyed on the pre-alias name, `import { addK, K
            // as KK }` would block the closure from restoring `klib`'s own `K`,
            // leaving the module's `x + K` to bind to whatever the IMPORTER
            // called `K`. A name imported as itself is the declaration pushed
            // (`merged_as`); another declaration of it in the module, which
            // the module reads by position, still travels.
            let binding_names: HashSet<&str> =
                bindings.iter().filter_map(|b| b.alias.as_deref()).collect();
            // Everything this import contributes goes after here; the closure
            // pass below pulls dependencies in discovery order, which is not
            // declaration order, so the block is re-sorted at the end.
            let import_start = target.decls.len();
            for b in bindings {
                let effective_name = b.alias.as_deref().unwrap_or(&b.name);
                // A name the module declares twice is its last declaration,
                // as a flat import reads it.
                let Some(d) = importable.iter().copied().rfind(|d| decl_binds(d, &b.name)) else {
                    diagnostics.push(Diagnostic::error(
                        "WS012",
                        format!("'{}' not found in '{}'", b.name, imp.path),
                        imp.range.clone(),
                    ));
                    continue;
                };
                // Diamond-import dedup vs genuine collision, same rule as
                // `ImportKind::All`: skip only when `effective_name` is already
                // bound FROM THE SAME source file (the same module reached twice).
                // A same name selected from a DIFFERENT file is a real collision —
                // keep it so WS013 fires instead of silently aliasing them.
                let d_file = d.range().file.clone();
                if target.binds_in_file(effective_name, &d_file) {
                    continue;
                }
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
                                 '{}'. Import it without `as`, or give the destructure \
                                 its own alias at the declaration",
                                b.name, imp.path
                            ),
                            imp.range.clone(),
                        ));
                        continue;
                    }
                    target.push(d);
                } else {
                    target.push(d.clone());
                }
            }
            // A named import brings ONLY what it names. A module's `on`
            // handlers and anonymous chips bind no name to select, so they
            // stay behind, which is the point of the spelling (a test harness
            // imports a module's pure helpers without installing its event
            // surface). It is warned about because a library written to
            // install behaviour looks identical at the import site.
            let skipped = importable.iter().filter(|d| installs_behaviour(d)).count();
            if skipped > 0 {
                diagnostics.push(Diagnostic::warning(
                    "WS014",
                    format!(
                        "`import {{ ... }} from '{}'` brings only the names it lists, so the {} \
                         `on` handler(s)/anonymous chip(s) '{}' installs are NOT included; \
                         write `import \"{}\"` to install them too",
                        imp.path, skipped, imp.path, imp.path
                    ),
                    imp.range.clone(),
                ));
            }
            // Pull in non-requested declarations that are referenced by
            // the imported ones. Covers both transitive imports (from other
            // files) and same-file helpers (e.g. timer_tick used by
            // timers_advance). Iterates to a fixed point so transitive
            // chains (A calls B calls C) are fully resolved.
            // TypeAlias declarations are NOT pulled — they are inlined below.
            loop {
                let mut added = false;
                for d in &importable {
                    if matches!(d, TopDecl::TypeAlias(_)) {
                        continue;
                    }
                    let names = decl_names(d);
                    // Same diamond-dedup rule as the `ImportKind::All` path, and
                    // for the same reason: a name already bound FROM ANOTHER
                    // FILE is a different declaration that happens to share a
                    // spelling, so skipping on it would wire this module's
                    // private `helper` to the other module's `helper` with no
                    // diagnostic at all. Comparing the declaration keeps the
                    // pull, and WS013 then reports the collision.
                    let called_as_method = matches!(d, TopDecl::Chip(c)
                        if c.is_self_receiver() && target.used.contains(&format!(".{}", c.name)));
                    if !names.is_empty()
                        && (names.iter().any(|n| target.used.contains(n.as_str()))
                            || called_as_method)
                        && !names.iter().any(|n| binding_names.contains(n.as_str()))
                        && !target.merged_as(d, &names)
                    {
                        target.push((*d).clone());
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
            // The source file already orders constants before their users,
            // and a name it declares twice in the order it reads them.
            let order: HashMap<(Arc<str>, usize), usize> = importable
                .iter()
                .enumerate()
                .map(|(i, d)| (decl_id(d), i))
                .collect();
            target.decls[import_start..]
                .sort_by_key(|d| order.get(&decl_id(d)).copied().unwrap_or(usize::MAX));

            // Inline-expand type aliases in imported declarations' params
            // so the TypeAlias doesn't need to be in the importing scope.
            let type_aliases: HashMap<String, Alias> = importable
                .iter()
                .filter_map(|d| match d {
                    TopDecl::TypeAlias(t) => Some((t.name.clone(), alias_of(t))),
                    _ => None,
                })
                .collect();
            if !type_aliases.is_empty() {
                for d in target.decls.iter_mut() {
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
                    .and_then(|d| parsed.doc_comments.get(&crate::parser::doc_key(d.range())))
                    .cloned()
            });

            // Inline the module's own type aliases into its declarations, as a
            // named import already does. A namespaced `mod f() -> MyType` is
            // used from the importing module, where `MyType` is not in scope —
            // leaving the name unexpanded fails there with "unknown type".
            let mut decls: Vec<TopDecl> = importable.into_iter().cloned().collect();
            let type_aliases: HashMap<String, Alias> = decls
                .iter()
                .filter_map(|d| match d {
                    TopDecl::TypeAlias(t) => Some((t.name.clone(), alias_of(t))),
                    _ => None,
                })
                .collect();
            if !type_aliases.is_empty() {
                for d in decls.iter_mut() {
                    expand_type_aliases_in_decl(d, &type_aliases);
                }
            }

            // An alias is WRITTEN per file, but the namespace it binds is
            // merged into one flat program-wide list keyed by name, and
            // lowering resolves by that name alone outside a namespaced mod's
            // own body. So a second alias under this name is reported wherever
            // it was written, not only when it was written here. Compare
            // canonical targets, not spellings: one module reached twice under
            // one alias (`"utils"` and `"./utils"`) is a redundant import, not
            // a collision.
            let prior = bound_namespace(&target.decls, ns_name).map(|n| {
                (
                    namespace_target(n, loader),
                    n.source_path.clone(),
                    n.range.file.clone(),
                )
            });
            if let Some((prior_canon, prior_path, prior_file)) = prior {
                let same_file = prior_file == imp.range.file;
                if prior_canon != canon {
                    let message = if same_file {
                        format!(
                            "namespace alias '{ns_name}' is already bound in this file to \
                             '{prior_path}', so this import would silently replace it and every \
                             reference through '{ns_name}' would read the wrong module. Rename \
                             one of the two aliases"
                        )
                    } else {
                        format!(
                            "namespace alias '{ns_name}' is already bound to '{prior_path}' by an \
                             `import * as` in '{prior_file}', which travelled here with an import \
                             of that module. Both merge into one flat scope, so a call through \
                             '{ns_name}' in either file reads whichever module lowering \
                             registered last. Rename one of the two aliases"
                        )
                    };
                    diagnostics.push(Diagnostic::error("WS012", message, imp.range.clone()));
                }
                if same_file {
                    // The alias is already bound in this file; a second
                    // namespace under it would shadow the first.
                    return;
                }
                // A prior from ANOTHER file is still pushed: typecheck resolves
                // a namespaced call against the alias declared in the caller's
                // own file, so dropping this one would bury the reported
                // collision under a WS002 for every use site in THIS file.
            }

            target.push(TopDecl::Namespace(NamespaceDecl {
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
        // One report per colliding alias, however many fixpoint sweeps the
        // loop below takes to settle.
        let mut reported: HashSet<&str> = HashSet::default();
        loop {
            let mut added = false;
            for ns in &source_namespaces {
                let TopDecl::Namespace(nsd) = ns else {
                    continue;
                };
                let name = nsd.name.as_str();
                if !target.used.contains(name) {
                    continue;
                }
                // Already travelled (the same module reached twice).
                if target.binds_in_file(name, &nsd.range.file) {
                    continue;
                }
                // Whether a same-named alias from ANOTHER file really coexists
                // depends on how this import spells itself. `import * as`
                // wraps what it pulls in a `NamespaceDecl`, and lowering
                // scopes a namespaced mod's body against its own module's
                // aliases first (`ns_mod_scopes` / `ns_by_file` in
                // `lower/decl.rs`), so those two are independent. A bare or
                // named import pushes the same declarations FLAT, where there
                // is no per-body scope: typecheck still resolves each `U.f` by
                // the file it was written in, but lowering has one binding per
                // name for the whole program, so two libraries that each
                // privately wrote `import * as U` type-check clean and then
                // both call whichever module registered last. Report that; the
                // namespace is still pushed, so the WS012 is the only thing
                // the user has to read.
                if !matches!(imp.kind, ImportKind::Namespace(_))
                    && let Some(prior) = bound_namespace(&target.decls, name)
                    && namespace_target(prior, loader) != namespace_target(nsd, loader)
                    && reported.insert(name)
                {
                    diagnostics.push(Diagnostic::error(
                        "WS012",
                        format!(
                            "namespace alias '{name}' is bound to '{}' in '{}' and to '{}' in \
                             '{}'. A bare or named import merges both into one flat scope, so a \
                             call through '{name}' in either file reads whichever module lowering \
                             registered last. Rename one of the two aliases",
                            prior.source_path, prior.range.file, nsd.source_path, nsd.range.file
                        ),
                        imp.range.clone(),
                    ));
                }
                // Prepend: lowering registers declarations in source order,
                // so a namespace appended after its caller would read as a
                // use before declaration and resolve to nothing.
                target.insert_front((*ns).clone());
                added = true;
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
        LetBinding::Tuple { names, .. } => match names.iter_mut().find(|n| *n == old) {
            Some(n) => {
                *n = new_name.to_string();
                true
            }
            None => false,
        },
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
/// pre-resolve AST for local analysis; re-parsing the same buffer a second
/// time in here costs ~21% of a keystroke on a 2.5k-line file, which this
/// avoids. Consumes the `ParseResult`; clone it first if you need it.
pub fn resolve_parsed(parsed: ParseResult, file: &str, loader: &dyn FileLoader) -> ResolveResult {
    let mut diagnostics = parsed.diagnostics.clone();
    let mut doc_comments = parsed.doc_comments.clone();

    let mut merged = DeclAccum::default();
    let mut main_decls: Vec<TopDecl> = Vec::new();
    let mut cache: HashMap<String, ParseResult> = HashMap::default();
    let mut stack: HashSet<String> = HashSet::default();
    let mut import_scope = ImportScope::default();

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
                &mut merged,
                &mut doc_comments,
                &mut import_scope,
            );
        } else {
            main_decls.push(d.clone());
        }
    }

    // Check for unused named imports. An import counts as used when the main
    // file OR another imported declaration references it — imported mods
    // reference their defining module's constants inside their bodies. A
    // `self` mod also counts when it is called with method syntax (`v.f()`).
    let mut used_idents = HashSet::default();
    for d in main_decls.iter().chain(&merged.decls) {
        collect_idents(walk::Node::Decl(d), Uses::All, &mut used_idents);
    }
    let called_as_method = |name: &str| {
        used_idents.contains(&format!(".{name}"))
            && merged.decls.iter().any(|d| match d {
                TopDecl::Chip(c) => c.name == name && c.is_self_receiver(),
                TopDecl::Fn(f) => {
                    f.name == name && f.params.first().is_some_and(|p| p.name == "self")
                }
                _ => false,
            })
    };
    for d in &parsed.ast.decls {
        if let TopDecl::Import(imp) = d
            && let ImportKind::Named(bindings) = &imp.kind
        {
            for b in bindings {
                let check_name = b.alias.as_deref().unwrap_or(&b.name);
                if !used_idents.contains(check_name) && !called_as_method(check_name) {
                    diagnostics.push(Diagnostic::warning(
                        "WS014",
                        format!("unused import '{}'", check_name),
                        b.range.clone(),
                    ));
                }
            }
        }
    }

    // `ws-ignore` directives from the entry file and every file it pulled in.
    // Applied here so `diagnostics` is already clean for any caller that reads
    // it directly; the map travels on so the stages after resolve can filter
    // their own diagnostics through the same directives.
    let mut suppressions = Suppressions::default();
    collect_suppressions(&mut suppressions, &parsed.source_map);
    for p in cache.values() {
        collect_suppressions(&mut suppressions, &p.source_map);
    }
    suppressions.apply(&mut diagnostics);

    // Imported declarations come first, then main file declarations
    let mut decls = merged.decls;
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
            // Same rule as the module `@label`: only the entry file's module
            // `@color` / `@material` reach the root chip.
            module_color: parsed.ast.module_color.clone(),
            module_material: parsed.ast.module_material.clone(),
            imports: Arc::new(import_scope),
        },
        diagnostics,
        doc_comments,
        source_map: Arc::new(parsed.source_map),
        // `cache` is keyed by canonical path and filled by `resolve_import` as
        // it walks imports depth-first, so its keys are exactly the transitive
        // import set.
        imported_files: cache.into_keys().collect(),
        suppressions,
    }
}

/// What [`collect_idents`] gathers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Uses {
    /// The names code reads: identifiers, a method call's name as `.name`,
    /// record shorthands and the target of an `as`. The closure pass pulls
    /// the imported declarations these name.
    Code,
    /// Those, plus the names handler triggers and type annotations mention.
    /// The unused-import check reads these; an `import * as` module's own
    /// declarations are left to that module.
    All,
}

/// Every name `node`, or anything below it, uses (see [`Uses`]).
pub(crate) fn collect_idents(node: walk::Node, uses: Uses, idents: &mut HashSet<String>) {
    let skipped =
        |n: walk::Node| uses == Uses::All && matches!(n, walk::Node::Decl(TopDecl::Namespace(_)));
    if skipped(node) {
        return;
    }
    leaf_idents(node, uses, idents);
    walk::walk(node, walk::Ctx::default(), &mut |n, _| {
        if skipped(n) {
            return false;
        }
        leaf_idents(n, uses, idents);
        true
    });
}

/// The names `node` itself uses, not counting its children.
fn leaf_idents(node: walk::Node, uses: Uses, idents: &mut HashSet<String>) {
    let mut ty = |t: &TypeExpr| collect_idents_in_type_expr(t, idents);
    match node {
        walk::Node::Expr(e) => match e {
            Expr::Ident { name, .. } => {
                idents.insert(name.clone());
            }
            Expr::Call { callee, .. } => {
                // A method call `v.dist(o)` names a `self` mod `dist`, recorded
                // as `.dist` so it pulls in only such a mod.
                if let Expr::FieldAccess { field, .. } = callee.as_ref() {
                    idents.insert(format!(".{field}"));
                }
            }
            Expr::RecordLit { fields, .. } | Expr::VariantCtor { fields, .. } => {
                for f in fields {
                    if let RecordLitField::Shorthand { name, .. } = f {
                        idents.insert(name.clone());
                    }
                }
            }
            Expr::As { ty: t, .. } => ty(t),
            _ => {}
        },
        _ if uses == Uses::Code => {}
        walk::Node::Decl(d) => match d {
            TopDecl::Handler(h)
            | TopDecl::Event(EventDecl {
                captured: Some(h), ..
            }) => handler_idents(h, idents),
            TopDecl::Chip(c) => chip_idents(c, idents),
            TopDecl::Fn(f) => {
                for p in &f.params {
                    ty(&p.typ);
                }
            }
            TopDecl::Var(v) => v.typ.iter().for_each(ty),
            TopDecl::Let(l) => l.typ.iter().for_each(ty),
            TopDecl::Out(o) => o.typ.iter().for_each(ty),
            TopDecl::Array(a) => ty(&a.element_type),
            TopDecl::Map(m) => {
                ty(&m.key_type);
                ty(&m.value_type);
            }
            TopDecl::Buffer(b) => b.typ.iter().for_each(ty),
            TopDecl::In(i) => ty(&i.typ),
            _ => {}
        },
        walk::Node::Stmt(s) => match s {
            Stmt::Handler(h) => handler_idents(h, idents),
            Stmt::ChipDecl(c) => chip_idents(c, idents),
            Stmt::Var(v) => v.typ.iter().for_each(ty),
            Stmt::Let(l) => l.typ.iter().for_each(ty),
            Stmt::OutBinding(o) => o.typ.iter().for_each(ty),
            Stmt::Array(a) => ty(&a.element_type),
            Stmt::Map(m) => {
                ty(&m.key_type);
                ty(&m.value_type);
            }
            Stmt::Buffer(b) => b.typ.iter().for_each(ty),
            Stmt::In(i) => ty(&i.typ),
            _ => {}
        },
        walk::Node::Block(_) => {}
    }
}

/// A handler's trigger and its capture params' types. `on trig { }` uses
/// `trig`, `on A.done` uses `A`; a capture param may annotate its type with
/// an imported alias (`on CustomEvent("x") -> (v: SomeType)`).
fn handler_idents(h: &Handler, idents: &mut HashSet<String>) {
    collect_idents_in_trigger(&h.trigger, idents);
    if let Some((name, _)) = &h.alias {
        idents.insert(name.clone());
    }
    for p in &h.params {
        if let Some(t) = &p.ty {
            collect_idents_in_type_expr(t, idents);
        }
    }
}

/// A chip's parameter types.
fn chip_idents(c: &ChipDecl, idents: &mut HashSet<String>) {
    for p in &c.inputs {
        collect_idents_in_type_expr(&p.typ, idents);
    }
}

/// A handler's trigger names a use of whatever it fires on: `on trig { }` uses
/// `trig`, `on A.done` uses `A`. Without counting these, an import used ONLY as
/// a trigger reports WS014 "unused import" and Organize Imports deletes it.
fn collect_idents_in_trigger(t: &Trigger, idents: &mut HashSet<String>) {
    match t {
        Trigger::Ident { name, .. } => {
            idents.insert(name.clone());
        }
        Trigger::Field { obj, .. } => {
            idents.insert(obj.clone());
        }
        Trigger::Not { inner, .. } => collect_idents_in_trigger(inner, idents),
        Trigger::Union { parts, .. } => {
            for p in parts {
                collect_idents_in_trigger(p, idents);
            }
        }
    }
}

fn collect_idents_in_type_expr(t: &TypeExpr, idents: &mut HashSet<String>) {
    match t {
        TypeExpr::Name { name, .. } => {
            idents.insert(name.clone());
        }
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            collect_idents_in_type_expr(inner, idents);
        }
        TypeExpr::Tuple { fields, .. } => {
            for f in fields {
                collect_idents_in_type_expr(f, idents);
            }
        }
        TypeExpr::Record { fields, .. } => {
            for f in fields {
                collect_idents_in_type_expr(&f.typ, idents);
            }
        }
        TypeExpr::Union { options, .. } => {
            for o in options {
                collect_idents_in_type_expr(o, idents);
            }
        }
        TypeExpr::Generic { args, .. } => {
            for a in args {
                collect_idents_in_type_expr(a, idents);
            }
        }
    }
}

/// A module's type alias: its type parameters and body.
type Alias = (Vec<String>, TypeExpr);

fn alias_of(t: &crate::ast::TypeAliasDecl) -> Alias {
    (
        t.type_params.iter().map(|p| p.name.clone()).collect(),
        t.typ.clone(),
    )
}

fn expand_type_aliases_in_decl(d: &mut TopDecl, aliases: &HashMap<String, Alias>) {
    match d {
        // An alias body may name another alias (`type Rect = { a: Point, b: Point }`).
        // Expanding it here makes the alias self-contained, which is what a
        // namespace import needs: the module's aliases are declared to the
        // importer only under their qualified name (`Ns.Rect`), so a bare
        // `Point` surviving inside the body has nothing to resolve against.
        // Seeded with its own name so a self-referential alias stops after one
        // level instead of expanding forever, and with its type parameters,
        // which shadow an alias of the same name.
        TopDecl::TypeAlias(t) => {
            let params: Vec<String> = t.type_params.iter().map(|p| p.name.clone()).collect();
            expand_aliases(&mut t.typ, aliases, &params, &mut vec![t.name.clone()]);
        }
        TopDecl::Chip(c) => {
            let params: Vec<String> = c.type_params.iter().map(|p| p.name.clone()).collect();
            for p in &mut c.inputs {
                expand_aliases(&mut p.typ, aliases, &params, &mut Vec::new());
            }
            for o in &mut c.outputs {
                expand_aliases(&mut o.typ, aliases, &params, &mut Vec::new());
            }
        }
        TopDecl::Fn(f) => {
            for p in &mut f.params {
                expand_type_aliases_in_type_expr(&mut p.typ, aliases);
            }
            if let Some(t) = &mut f.return_type {
                expand_type_aliases_in_type_expr(t, aliases);
            }
        }
        TopDecl::Let(l) => {
            if let Some(t) = &mut l.typ {
                expand_type_aliases_in_type_expr(t, aliases);
            }
        }
        TopDecl::Var(v) => {
            if let Some(t) = &mut v.typ {
                expand_type_aliases_in_type_expr(t, aliases);
            }
        }
        TopDecl::Out(o) => {
            if let Some(t) = &mut o.typ {
                expand_type_aliases_in_type_expr(t, aliases);
            }
        }
        TopDecl::Buffer(b) => {
            if let Some(t) = &mut b.typ {
                expand_type_aliases_in_type_expr(t, aliases);
            }
        }
        TopDecl::In(i) => {
            expand_type_aliases_in_type_expr(&mut i.typ, aliases);
        }
        _ => {}
    }
}

fn expand_type_aliases_in_type_expr(t: &mut TypeExpr, aliases: &HashMap<String, Alias>) {
    expand_aliases(t, aliases, &[], &mut Vec::new());
}

/// Substitute alias names by their bodies, *including inside a body just
/// substituted in* — an alias whose body names another alias (`type Rect = {
/// a: Point, b: Point }`) otherwise leaves the inner name behind, unresolvable
/// in the importing module, and the field silently types as `any`. A generic
/// alias applied to arguments (`Pair<int>`) is instantiated the same way, so
/// `type IP = Pair<int>` needs no `Pair` where it is read.
///
/// `params` are the type parameters of the declaration `t` is written in,
/// which hide an alias of the same name there. An alias body is written at the
/// top level, so none of them reach into it: its own names are expanded as its
/// declaration reads them, and a generic alias's arguments are substituted
/// only after its body is written out.
///
/// `active` is the chain of aliases being substituted on the current path. A
/// name already on it is left alone, so a self-referential or mutually
/// recursive alias expands one level and stops rather than looping forever.
fn expand_aliases(
    t: &mut TypeExpr,
    aliases: &HashMap<String, Alias>,
    params: &[String],
    active: &mut Vec<String>,
) {
    match t {
        TypeExpr::Name { name, .. } => {
            if params.iter().any(|p| p == name) || active.iter().any(|a| a == name) {
                return;
            }
            let Some((alias_params, body)) = aliases.get(name.as_str()) else {
                return;
            };
            if !alias_params.is_empty() {
                return;
            }
            let mut body = body.clone();
            active.push(name.clone());
            expand_aliases(&mut body, aliases, &[], active);
            active.pop();
            *t = body;
        }
        TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
            expand_aliases(inner, aliases, params, active);
        }
        TypeExpr::Tuple { fields, .. } => {
            for f in fields {
                expand_aliases(f, aliases, params, active);
            }
        }
        TypeExpr::Record { fields, .. } => {
            for f in fields {
                expand_aliases(&mut f.typ, aliases, params, active);
            }
        }
        TypeExpr::Union { options, .. } => {
            for o in options {
                expand_aliases(o, aliases, params, active);
            }
        }
        TypeExpr::Generic { name, args, .. } => {
            for a in args.iter_mut() {
                expand_aliases(a, aliases, params, active);
            }
            if params.iter().any(|p| p == name) || active.iter().any(|a| a == name) {
                return;
            }
            let Some((alias_params, body)) = aliases.get(name.as_str()) else {
                return;
            };
            if alias_params.is_empty() || alias_params.len() != args.len() {
                return;
            }
            let mut body = body.clone();
            active.push(name.clone());
            expand_aliases(&mut body, aliases, alias_params, active);
            active.pop();
            let subst: HashMap<String, TypeExpr> = alias_params
                .iter()
                .cloned()
                .zip(args.iter().cloned())
                .collect();
            *t = crate::types::resolve::substitute_type_expr(&body, &subst);
        }
    }
}

#[cfg(test)]
mod dep_pull_tests;
#[cfg(test)]
mod tests;
