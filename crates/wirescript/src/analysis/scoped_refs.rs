//! Pure, scope-aware name resolver behind find-references / rename.
//!
//! Replaces the old textual find-and-replace with an AST walk that tracks a
//! two-namespace (value / type) lexical scope stack: every binding site
//! (`var`, `let`, params, handler captures, chip/mod/fn/type/event decls, …)
//! registers a [`Binding`] with a scope extent, and every identifier use
//! resolves to the nearest enclosing binding in its namespace. That makes
//! rename precise — it only ever touches sites that resolve to the same
//! declaration, never comments, strings, type-position names, other-scope
//! identifiers, or unrelated files.
//!
//! Structure:
//! - The binding inventory — [`build_scope_model`] walks the whole AST and
//!   registers every VALUE binding with its scope extent.
//! - VALUE-position use collection (`Expr::Ident`, a call's callee,
//!   record-shorthand, every sub-expression) plus [`references_at`] —
//!   resolve the binding under a cursor and return every same-file site
//!   bound to it. `cross_file` only distinguishes `Local` / `Exported` here;
//!   `Imported` and type-namespace targets are handled separately below.
//! - The TYPE namespace — `type` alias bindings and per-decl `TypeParam`
//!   bindings, plus TYPE-position use collection over every `TypeExpr`
//!   reachable from an annotation (`var`/`in`/`out`/`buffer`/`param`/`let`/
//!   handler-capture/fn-return/chip-output types, `type X = …`'s RHS, a
//!   `TypeParam`'s bound, and `Call.type_args`). Value and type uses share
//!   one resolution pass ([`resolve_uses`]) that already matches
//!   `Binding.ns == Use.ns`, so the two namespaces stay separate for
//!   free — a value binding named `character` is invisible to a
//!   `TypeExpr::Name { name: "character", .. }` use and vice versa.
//! - [`prepare_rename_at`] — the rename entry point. Finalizes cursor
//!   dispatch with the refusals `references_at` alone doesn't cover: a
//!   lexer keyword, and a record FIELD name (`Expr::FieldAccess.field`,
//!   `RecordLitField::Named.name`, a record TYPE field's name). The
//!   builtin-type/unresolved-global refusal is already implicit in
//!   `references_at` (an unresolved `Use` makes it return `None`), so no
//!   extra check is needed for that case.
//! - Cross-file resolution. `register_import` fills in `TopDecl::Import(_)`
//!   (otherwise left a no-op by the binding walk): a NAMED import specifier
//!   (`import { name }` / `import { orig as name }`) registers a binding in
//!   BOTH namespaces under its local name, tagged with the ORIGINAL exported
//!   name via `Binding::import_export`. `references_at` checks that tag
//!   first, so a use of an imported name classifies as `CrossFile::Imported`
//!   rather than `Local`/`Exported`. [`references_to_export`] is the reverse
//!   direction — given another file's AST plus an exported name, return
//!   every site (import specifier + uses) in THAT file that traces back to
//!   it; a local shadow excludes itself for free via the existing
//!   innermost-scope resolution rule. Namespace-member rename (`u.foo`)
//!   stays deferred — `foo` is already refused via `field_name_spans`; only
//!   the alias `u` itself is renameable, and only once `ImportKind::Namespace`
//!   grows its own binding.
//!
//! See `docs/superpowers/plans/2026-08-12-scoped-rename-name-resolution.md`
//! for the LSP/WASM wiring layered on top of this module.

use crate::ast::*;
use crate::collections::HashSet;
use crate::diagnostic::{Pos, SourceRange};
use std::sync::OnceLock;

#[cfg(test)]
mod tests;

/// Which lexical namespace a name lives in. Value names (`var`, `let`,
/// params, …) and type names (`type`, type params, …) are separate — a
/// value binding named `character` is invisible to a
/// `TypeExpr::Name { name: "character", .. }` and vice versa (this
/// separation is what makes a capture named after a type safe to rename).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefNs {
    Value,
    Type,
}

/// One renameable occurrence of a resolved binding: a precise identifier
/// span plus whether it's a record-literal shorthand value site (`{ name }`,
/// which renames to `{ name: new_name }` via [`super::rename_edit_text`]
/// rather than a straight text swap).
#[derive(Clone, Debug)]
pub struct RefSite {
    pub range: SourceRange,
    pub is_shorthand: bool,
    /// True when `range` is a whole-declaration span rather than a precise
    /// name token — the decl sites of container-ranged kinds (`var`, `chip`,
    /// `mod`, `type`, tuple/record `let`, `emit NAME = …`, …) and any use
    /// that could only be given a coarse range (`Emit.name`'s statement span;
    /// a `TypeExpr::Generic` head, whose range covers the whole `Name<Args>`
    /// with no name-only sub-span). The wiring layer (Tasks 6/7) narrows
    /// these to the name span via `references::find_name_range`; precise
    /// sites (`Expr::Ident`, record shorthand, `HandlerParam`/
    /// `RecordDestructField` decls, `TypeExpr::Name`) are left alone.
    pub coarse: bool,
}

/// Whether a resolved binding's uses can appear outside the current file.
/// `Local` covers every non-importable file-scope binding, plus every
/// local/param/capture. `Exported` covers an importable file-scope
/// chip/mod/fn/let/type/event, named for `import { name }` by sibling
/// files. `Imported` is a name brought into *this* file from another one.
#[derive(Clone, Debug)]
pub enum CrossFile {
    Local,
    Exported { export_name: String },
    Imported { export_name: String },
}

/// The binding a cursor query resolved to: enough to classify it (namespace,
/// source-construct `kind`) and locate/rename its declaration.
#[derive(Clone, Debug)]
pub struct RefTarget {
    pub name: String,
    pub ns: RefNs,
    pub kind: &'static str,
    pub decl_name_range: SourceRange,
    pub cross_file: CrossFile,
}

/// A single name-binding site: a declaration that introduces `name` into
/// scope. Namespace-separated from [`Use`] — a `Binding` never itself
/// represents a reference to another binding.
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub(crate) id: usize,
    pub(crate) name: String,
    pub(crate) ns: RefNs,
    /// The declaration's own name span. Precise (name-token-only) where the
    /// AST carries a dedicated range for it (`LetBinding::Ident`,
    /// `HandlerParam`, `RecordDestructField::Named`, …); falls back to the
    /// whole declaration's range for constructs the parser doesn't give a
    /// narrower span (`VarDecl`, `ChipDecl`, `InDecl`, …). Narrowing those
    /// is follow-up work.
    pub(crate) name_range: SourceRange,
    /// `None` = visible for the whole file (file-scope decl); `Some(r)` =
    /// visible only within lexical extent `r` (the owning block/handler/
    /// chip-or-mod body's range).
    pub(crate) scope: Option<SourceRange>,
    /// A short tag identifying the binding's source construct — "var",
    /// "let", "param", "capture", "chip", "mod", "fn", "event", "in", "out",
    /// "buffer", "array", "map", "namespace", "type", "typeparam", … — for
    /// diagnostics/tests; callers outside this module should not match on it.
    pub(crate) kind: &'static str,
    /// True only for file-scope chip/mod/fn/let/type/event bindings — the
    /// only names importable from another file (Global Constraints). Every
    /// other binding (var/buffer/array/map/in/out at file scope, and every
    /// local/param/capture) is `false`, even when `scope` is `None`.
    pub(crate) importable: bool,
    /// True when `name_range` is a whole-declaration span, not a precise name
    /// token — the wiring layer narrows those via `find_name_range`. False
    /// only for the genuinely name-token-precise kinds: handler captures
    /// (`HandlerParam.range`) and `LetBinding::Ident`. Two coarse bindings
    /// can share ONE container range (tuple/record `let`, tuple param,
    /// `await` destructure), which is why cursor dispatch needs the
    /// identifier-under-cursor tie-break. NB `RecordDestructField` ranges are
    /// coarse too — they span `src: bound` / `x ` (trailing), not the bound
    /// name alone (verified in `parser.rs`).
    pub(crate) coarse: bool,
    /// True for a record-destructure shorthand decl site (`let { x } = p` /
    /// `mod f({ x }: P)`) — renaming its local must expand to `{ x: new }`
    /// (the field name `x` stays), the same shorthand rule record literals
    /// use, so the decl `RefSite` carries this onto `rename_edit_text`. False
    /// everywhere else, including an aliased `{ src: bound }` (that narrows to
    /// the `bound` token and plain-replaces → `{ src: new }`).
    pub(crate) is_shorthand: bool,
    /// `Some(original_export_name)` for a binding registered from a NAMED
    /// import specifier (`import { name }` / `import { orig as name }`) — the
    /// name to look up among the SOURCE file's exports. `None` for every
    /// other binding, including a `TopDecl::Namespace` alias (a different,
    /// already-resolved construct — see `walk_top_decl`).
    pub(crate) import_export: Option<String>,
}

/// One value- or type-position identifier *use*, populated by
/// [`Walker::push_use`] (value) and [`Walker::push_type_use`] (type).
#[derive(Clone, Debug)]
pub(crate) struct Use {
    pub(crate) range: SourceRange,
    pub(crate) name: String,
    pub(crate) ns: RefNs,
    pub(crate) resolved: Option<usize>,
    pub(crate) is_shorthand: bool,
    /// True for a use whose `range` is a whole-statement/whole-type-expr span
    /// rather than a precise identifier token — `Emit.name` (`emit NAME =
    /// expr`, where the AST carries no name-only span) and a
    /// `TypeExpr::Generic` head (`Name<Args>`'s `range` covers the whole
    /// generic application, not just `Name`). Ordinary `Expr::Ident` / record
    /// shorthand / `TypeExpr::Name` uses are precise (`false`).
    pub(crate) coarse: bool,
}

/// The whole-program binding + use inventory a cursor query resolves
/// against.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScopeModel {
    pub(crate) bindings: Vec<Binding>,
    pub(crate) uses: Vec<Use>,
    /// Spans of record FIELD names — never a rename target (deferred per
    /// the plan's Global Constraints). Covers `Expr::FieldAccess.field` (the
    /// part of `obj.field` after the last `.`), the KEY half of
    /// `RecordLitField::Named` (`name: value`), and a `RecordTypeField`'s
    /// `name` (`name: Type` in a record type). None of these AST nodes carry
    /// a name-only sub-range of their own (`field` is a bare `String`; the
    /// containing node's own `range` spans the name AND the value/type), so
    /// [`Walker`] computes each span by position arithmetic —
    /// [`field_suffix_span`]/[`name_prefix_span`] — as it visits these nodes
    /// during the AST walk, rather than re-walking the tree.
    /// [`prepare_rename_at`] checks this FIRST, before dispatching through
    /// [`references_at`]'s own cursor match: a coarser declaration's
    /// `name_range` can otherwise swallow a field cursor that falls inside
    /// its own initializer/body (e.g. `var x: int = p.field`, whose `var`
    /// binding's coarse range spans the whole statement, `field` included).
    pub(crate) field_name_spans: Vec<SourceRange>,
}

/// The [`SourceRange`] of the `field`-length identifier token that ends
/// exactly at `range.end` — for `Expr::FieldAccess`, whose own `range`
/// covers the whole `obj.field` expression but whose `end` the parser sets
/// to the field token's own end (the postfix-access arm in `parser.rs`).
/// Wirescript identifiers are ASCII-only (`lexer::is_ident_start`), so byte
/// length == char count == column width and no source text is needed to
/// locate the field name within `range`.
fn field_suffix_span(field: &str, range: &SourceRange) -> SourceRange {
    let len = field.len() as u32;
    SourceRange {
        file: range.file.clone(),
        start: Pos {
            offset: range.end.offset.saturating_sub(field.len()),
            line: range.end.line,
            col: range.end.col.saturating_sub(len),
        },
        end: range.end.clone(),
    }
}

/// The [`SourceRange`] of the `name`-length identifier token that starts
/// exactly at `range.start` — for `RecordLitField::Named` (`range` covers
/// `name: value`) and `RecordTypeField` (`range` covers `name: Type`), both
/// anchored by the parser at the name token's own start (`parser.rs`).
fn name_prefix_span(name: &str, range: &SourceRange) -> SourceRange {
    let len = name.len() as u32;
    SourceRange {
        file: range.file.clone(),
        start: range.start.clone(),
        end: Pos {
            offset: range.start.offset + name.len(),
            line: range.start.line,
            col: range.start.col + len,
        },
    }
}

/// The type-namespace globals — primitive/builtin type names plus the
/// generic heads the parser desugars specially (`Array<V>`/`Ref<V>` fold
/// into `TypeExpr::Array`/`Ref` at parse time; `Map` stays a `Generic`).
/// None of these ever resolve to a user [`Binding`], so a `TypeExpr::Name`
/// matching one is refused for rename rather than left dangling.
pub(crate) const BUILTIN_TYPE_NAMES: &[&str] = &[
    "int", "float", "bool", "string", "entity", "controller", "character", "vector", "rotator",
    "color", "exec", "zone", "teleport", "Map", "Ref", "Array",
];

pub(crate) fn builtin_type_names() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| BUILTIN_TYPE_NAMES.iter().copied().collect())
}

/// Walk `script`'s whole AST, registering every value AND type binding
/// (Tasks 1/3) and every value- and type-position use (Tasks 2/3), then
/// resolve each use to the binding that (lexically) owns it IN ITS OWN
/// NAMESPACE. Uses are resolved in a pass AFTER the walk completes so a use
/// can bind to a file-scope decl or a block-local that appears later in the
/// source (forward references) — see [`resolve_uses`].
pub(crate) fn build_scope_model(script: &Script) -> ScopeModel {
    let mut model = ScopeModel::default();
    let mut walker = Walker { model: &mut model };
    walker.walk_script(script);
    resolve_uses(&mut model);
    model
}

struct Walker<'a> {
    model: &'a mut ScopeModel,
}

impl<'a> Walker<'a> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        name: String,
        ns: RefNs,
        name_range: SourceRange,
        scope: Option<SourceRange>,
        kind: &'static str,
        importable: bool,
        coarse: bool,
    ) {
        let id = self.model.bindings.len();
        self.model.bindings.push(Binding {
            id,
            name,
            ns,
            name_range,
            scope,
            kind,
            importable,
            coarse,
            is_shorthand: false,
            import_export: None,
        });
    }

    /// Mark the just-`push`ed binding as a record-destructure shorthand decl
    /// site. Safe because `push` appends exactly one binding immediately
    /// before this is called; only the two `RecordDestructField::Named`
    /// registration paths (let + param) ever need a non-default value, so it
    /// isn't worth threading through every `push` call.
    fn mark_last_shorthand(&mut self, is_shorthand: bool) {
        if let Some(b) = self.model.bindings.last_mut() {
            b.is_shorthand = is_shorthand;
        }
    }

    /// Mark the just-`push`ed binding as a named-import local, recording the
    /// ORIGINAL exported name it was imported under so `references_at` /
    /// `references_to_export` can classify and find it. Safe for the same
    /// reason as `mark_last_shorthand`: `push` appends exactly one binding
    /// immediately before this is called.
    fn mark_last_import_export(&mut self, export_name: String) {
        if let Some(b) = self.model.bindings.last_mut() {
            b.import_export = Some(export_name);
        }
    }

    /// Record a value-position identifier occurrence (`Expr::Ident`, a call
    /// callee, a record-literal shorthand field). Resolved to a binding in a
    /// pass over the whole model after the walk completes (see
    /// [`resolve_uses`]) — not here, since a use may point at a file-scope
    /// decl or block-local that hasn't been walked yet.
    fn push_use(&mut self, name: String, range: SourceRange, is_shorthand: bool, coarse: bool) {
        self.model.uses.push(Use {
            range,
            name,
            ns: RefNs::Value,
            resolved: None,
            is_shorthand,
            coarse,
        });
    }

    /// Record a type-position identifier occurrence (`TypeExpr::Name`, or a
    /// `TypeExpr::Generic`'s head name). Resolved against TYPE-namespace
    /// bindings only in the same post-walk pass as value uses ([`resolve_uses`]) —
    /// a builtin name (`character`, `int`, `Map`, …) simply never matches a
    /// user [`Binding`] and stays `resolved: None`, which is exactly the
    /// "not a rename target" outcome cursor dispatch needs.
    fn push_type_use(&mut self, name: String, range: SourceRange, coarse: bool) {
        self.model.uses.push(Use {
            range,
            name,
            ns: RefNs::Type,
            resolved: None,
            is_shorthand: false,
            coarse,
        });
    }

    /// Descend a type annotation, pushing a TYPE-namespace [`Use`] for every
    /// named type reachable from it — `TypeExpr::Name` precisely (the AST
    /// gives a name-only span), a `Generic`'s head coarsely (`range` covers
    /// the whole `Name<Args>`, no name-only sub-span exists to narrow to).
    /// `Ref`/`Array`/`Tuple`/`Union`/`Record` carry no name of their own —
    /// only their nested `TypeExpr`s do — so they just recurse.
    fn walk_type_expr(&mut self, t: &TypeExpr) {
        match t {
            TypeExpr::Name { name, range } => self.push_type_use(name.clone(), range.clone(), false),
            TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => self.walk_type_expr(inner),
            TypeExpr::Tuple { fields, .. } => {
                for f in fields {
                    self.walk_type_expr(f);
                }
            }
            TypeExpr::Union { options, .. } => {
                for o in options {
                    self.walk_type_expr(o);
                }
            }
            TypeExpr::Record { fields, .. } => {
                for f in fields {
                    // The field's own name is never a rename target.
                    self.model.field_name_spans.push(name_prefix_span(&f.name, &f.range));
                    self.walk_type_expr(&f.typ);
                }
            }
            TypeExpr::Generic { name, args, range } => {
                self.push_type_use(name.clone(), range.clone(), true);
                for a in args {
                    self.walk_type_expr(a);
                }
            }
        }
    }

    fn walk_script(&mut self, script: &Script) {
        if let Some(e) = &script.module_label {
            self.walk_expr(e);
        }
        for d in &script.decls {
            self.walk_top_decl(d);
        }
    }

    fn walk_top_decl(&mut self, d: &TopDecl) {
        match d {
            // Cross-file: the namespace alias itself is a file-scope value
            // binding; its members are handled by cross-file resolution,
            // not walked here.
            TopDecl::Namespace(ns) => {
                self.push(
                    ns.name.clone(),
                    RefNs::Value,
                    ns.range.clone(),
                    None,
                    "namespace",
                    false,
                    true,
                );
            }
            TopDecl::Var(v) => {
                self.push(v.name.clone(), RefNs::Value, v.range.clone(), None, "var", false, true);
                if let Some(t) = &v.typ {
                    self.walk_type_expr(t);
                }
                if let Some(e) = &v.init {
                    self.walk_expr(e);
                }
                if let Some(e) = &v.label_expr {
                    self.walk_expr(e);
                }
            }
            TopDecl::Array(a) => {
                self.push(a.name.clone(), RefNs::Value, a.range.clone(), None, "array", false, true);
                self.walk_type_expr(&a.element_type);
                for el in &a.init {
                    self.walk_expr(el.expr());
                }
            }
            TopDecl::Map(m) => {
                self.push(m.name.clone(), RefNs::Value, m.range.clone(), None, "map", false, true);
                self.walk_type_expr(&m.key_type);
                self.walk_type_expr(&m.value_type);
                if let Some(e) = &m.init {
                    self.walk_expr(e);
                }
            }
            TopDecl::Buffer(b) => {
                self.push(b.name.clone(), RefNs::Value, b.range.clone(), None, "buffer", false, true);
                if let Some(t) = &b.typ {
                    self.walk_type_expr(t);
                }
                self.walk_expr(&b.init);
            }
            TopDecl::Fn(f) => self.register_fn(f),
            TopDecl::Chip(c) => self.register_chip(c, None, true),
            TopDecl::AnonChip(ac) => {
                if let Some(e) = &ac.label_expr {
                    self.walk_expr(e);
                }
                self.walk_block(&ac.body);
            }
            TopDecl::Event(e) => {
                self.push(e.name.clone(), RefNs::Value, e.range.clone(), None, "event", true, true);
                self.walk_expr(&e.source);
                if let Some(b) = &e.captured_body {
                    self.walk_block(b);
                }
            }
            TopDecl::In(d) => {
                self.push(d.name.clone(), RefNs::Value, d.range.clone(), None, "in", false, true);
                self.walk_type_expr(&d.typ);
                if let Some(e) = &d.label_expr {
                    self.walk_expr(e);
                }
            }
            TopDecl::Out(o) => {
                self.push(o.name.clone(), RefNs::Value, o.range.clone(), None, "out", false, true);
                if let Some(t) = &o.typ {
                    self.walk_type_expr(t);
                }
                if let Some(e) = &o.value {
                    self.walk_expr(e);
                }
                if let Some(e) = &o.label_expr {
                    self.walk_expr(e);
                }
            }
            TopDecl::Handler(h) => self.register_handler(h),
            TopDecl::Let(l) => {
                self.register_let(l, None, true);
                self.walk_expr(&l.value);
            }
            TopDecl::Await(a) => self.register_await(a, None),
            TopDecl::Assign(a) => {
                self.walk_expr(&a.target);
                self.walk_expr(&a.value);
            }
            TopDecl::If(i) => self.walk_if(&i.cond, &i.then_block, &i.else_block),
            TopDecl::ExprStmt(es) => self.walk_expr(&es.expr),
            TopDecl::TypeAlias(t) => self.register_type_alias(t),
            TopDecl::Import(imp) => self.register_import(imp),
        }
    }

    /// `import { name }` / `import { orig as name }` bindings. Each named
    /// binding registers a file-scope binding under its LOCAL name (`alias`
    /// if present, else `name`) at the import specifier's own span —
    /// `ImportBinding.range` is the effective-identifier span per `ast.rs`,
    /// so this is precise (`coarse: false`), not a whole-decl span. This
    /// module never resolves the source file (no filesystem — see the module
    /// doc), so whether the imported name denotes a value or a type isn't
    /// knowable locally: register it in BOTH namespaces (two bindings, same
    /// local name/range/export), so a value use binds the value copy and a
    /// type use binds the type copy. Not importable itself (bringing a name
    /// IN doesn't re-export it back out). `ImportKind::All`/`Namespace`
    /// bring in no individually-named bindings and are handled separately —
    /// the namespace alias itself, once resolved, is a value binding via
    /// `TopDecl::Namespace` above.
    fn register_import(&mut self, imp: &ImportDecl) {
        let ImportKind::Named(bindings) = &imp.kind else {
            return;
        };
        for b in bindings {
            let local = b.alias.clone().unwrap_or_else(|| b.name.clone());
            self.push(local.clone(), RefNs::Value, b.range.clone(), None, "import", false, false);
            self.mark_last_import_export(b.name.clone());
            self.push(local, RefNs::Type, b.range.clone(), None, "import", false, false);
            self.mark_last_import_export(b.name.clone());
        }
    }

    fn register_fn(&mut self, f: &FnDecl) {
        self.push(f.name.clone(), RefNs::Value, f.range.clone(), None, "fn", true, true);
        let scope = f.body.range().clone();
        for p in &f.params {
            self.register_param(p, scope.clone());
        }
        if let Some(rt) = &f.return_type {
            self.walk_type_expr(rt);
        }
        self.walk_expr(&f.body);
    }

    /// Shared by a file-scope `chip`/`mod` (`TopDecl::Chip`, `scope = None`,
    /// `importable = true`) and a nested `Stmt::ChipDecl` (`scope =
    /// Some(enclosing block range)`, `importable = false`). `type_params` are
    /// scoped to the WHOLE decl range (`c.range`, not just the body) since
    /// they're visible from the input/output type annotations too, both of
    /// which precede the body in source order.
    fn register_chip(&mut self, c: &ChipDecl, scope: Option<SourceRange>, importable: bool) {
        let kind = if c.inline { "mod" } else { "chip" };
        self.push(c.name.clone(), RefNs::Value, c.range.clone(), scope, kind, importable, true);
        for tp in &c.type_params {
            self.register_type_param(tp, c.range.clone());
        }
        let body_scope = c.body.range.clone();
        for p in &c.inputs {
            self.register_param(p, body_scope.clone());
        }
        for o in &c.outputs {
            self.walk_type_expr(&o.typ);
        }
        if let Some(e) = &c.label_expr {
            self.walk_expr(e);
        }
        self.walk_block(&c.body);
    }

    /// `type Name<T, U: Bound> = …` (file scope, importable, `kind = "type"`)
    /// plus each of its type params, scoped to the whole alias `range` (RHS
    /// type positions and a later param's bound can both reference an
    /// earlier one).
    fn register_type_alias(&mut self, t: &TypeAliasDecl) {
        // `TypeAliasDecl.range` spans the whole `type X = …` decl (no
        // name-only span) → coarse.
        self.push(t.name.clone(), RefNs::Type, t.range.clone(), None, "type", true, true);
        for tp in &t.type_params {
            self.register_type_param(tp, t.range.clone());
        }
        self.walk_type_expr(&t.typ);
    }

    /// A generic type param (`T` or `T: Bound`) on a `chip`/`mod`/`type`
    /// decl. `TypeParam.range` spans the bound too when present (`parser.rs`:
    /// `make_range(name_tok.start, bound.range().end)`), so it's not reliably
    /// name-only → always coarse. The bound itself is a type-position
    /// reference (e.g. `Numeric`), not a binding, so it's walked as a use,
    /// not registered.
    fn register_type_param(&mut self, tp: &TypeParam, scope: SourceRange) {
        self.push(tp.name.clone(), RefNs::Type, tp.range.clone(), Some(scope), "typeparam", false, true);
        if let Some(bound) = &tp.bound {
            self.walk_type_expr(bound);
        }
    }

    /// A plain param binds its own name; a destructured param binds each
    /// field's name (alias if present) or tuple-pattern name, plus an
    /// optional `...rest` binder. All bound names share the param's own
    /// `scope` (the owning fn/chip body's range). `p.typ` is always present
    /// (even for a destructured pattern — it types the whole destructured
    /// value) and is walked once regardless of pattern shape.
    fn register_param(&mut self, p: &Param, scope: SourceRange) {
        self.walk_type_expr(&p.typ);
        match &p.pattern {
            // Plain param: `p.range` is the whole param span → coarse.
            None => self.push(p.name.clone(), RefNs::Value, p.range.clone(), Some(scope), "param", false, true),
            Some(ParamPattern::Record { fields, rest }) => {
                for f in fields {
                    match f {
                        // `RecordDestructField::Named.range` spans `src: bound`
                        // / `x ` (trailing), not the bound name alone → coarse
                        // (narrowed to the bound name downstream). A non-alias
                        // `{ x }` is shorthand: rename expands to `{ x: new }`.
                        RecordDestructField::Named { name, alias, range } => {
                            let bound = alias.clone().unwrap_or_else(|| name.clone());
                            self.push(bound, RefNs::Value, range.clone(), Some(scope.clone()), "param", false, true);
                            self.mark_last_shorthand(alias.is_none());
                        }
                        // `...rest` spans the `...` too → coarse, not shorthand.
                        RecordDestructField::Rest { name, range } => {
                            self.push(name.clone(), RefNs::Value, range.clone(), Some(scope.clone()), "param", false, true);
                        }
                    }
                }
                // The pattern-level `...rest` binder has no name span of its
                // own here, so it borrows the whole-param range → coarse.
                if let Some(r) = rest {
                    self.push(r.clone(), RefNs::Value, p.range.clone(), Some(scope), "param", false, true);
                }
            }
            // Tuple param names all share the one whole-param range → coarse.
            Some(ParamPattern::Tuple { names, rest }) => {
                for n in names {
                    self.push(n.clone(), RefNs::Value, p.range.clone(), Some(scope.clone()), "param", false, true);
                }
                if let Some(r) = rest {
                    self.push(r.clone(), RefNs::Value, p.range.clone(), Some(scope), "param", false, true);
                }
            }
        }
    }

    /// Descend a handler's trigger (`on go`, `on MyEvent()`, `on
    /// split.Forward`, `on !a`, `on a | b`) so the trigger token itself is a
    /// value-position use — without this, renaming an `in` exec port or a
    /// user `event` would leave every handler that triggers on it dangling,
    /// and the trigger token itself wouldn't be a valid rename start point.
    /// A builtin event name (`CharacterSpawned`, `Clock`, `ChatCommand`, …)
    /// or a synthesized expr-trigger name (`_on_expr_N`) simply never
    /// matches a user [`Binding`], so it resolves to `None` — the same safe
    /// no-op as an unresolved builtin call.
    fn walk_trigger(&mut self, t: &Trigger) {
        match t {
            // `on go` / `on MyEvent` — the whole trigger IS the name token
            // (`parser.rs`'s `parse_trigger_atom` gives it a name-only span),
            // so this is a precise use, exactly like `Expr::Ident`.
            Trigger::Ident { name, range } => self.push_use(name.clone(), range.clone(), false, false),
            // `on split.Forward` — `range` spans `obj.field` exactly like
            // `Expr::FieldAccess` (`parse_trigger_atom` builds it the same
            // way: `name_tok.start` to `field_tok.end`), so `field_suffix_span`
            // isolates the field token the same way `Expr::FieldAccess`'s
            // handling does. `obj` has no name-only sub-span of its own on
            // `Trigger::Field`, so its use is coarse (the wiring layer narrows
            // it to the object token downstream); `field` is a member
            // reference — never a rename target — so it goes into
            // `field_name_spans`, not a `Use`.
            Trigger::Field { obj, field, range } => {
                self.push_use(obj.clone(), range.clone(), false, true);
                self.model.field_name_spans.push(field_suffix_span(field, range));
            }
            Trigger::Not { inner, .. } => self.walk_trigger(inner),
            Trigger::Union { parts, .. } => {
                for p in parts {
                    self.walk_trigger(p);
                }
            }
        }
    }

    /// Shared by a file-scope `on ...` handler and a nested `Stmt::Handler`
    /// — captures are always scoped to `handler.body.range` regardless of
    /// nesting.
    fn register_handler(&mut self, h: &Handler) {
        let scope = h.body.range.clone();
        self.walk_trigger(&h.trigger);
        for hp in &h.params {
            // `HandlerParam.range` is the capture name token → precise.
            self.push(hp.name.clone(), RefNs::Value, hp.range.clone(), Some(scope.clone()), "capture", false, false);
            // Optional explicit type annotation (`-> (a: int, b: float)`,
            // custom-event data outputs); `None` for events with fixed
            // built-in data types.
            if let Some(t) = &hp.ty {
                self.walk_type_expr(t);
            }
        }
        // Trigger config args (`on ChatCommand("greet", Description = expr)`)
        // can host a `BlockExpr` with locals — walk them for nested bindings.
        for cfg in &h.config {
            match cfg {
                HandlerConfigArg::Positional(e) | HandlerConfigArg::Named { value: e, .. } => {
                    self.walk_expr(e)
                }
            }
        }
        self.walk_block(&h.body);
    }

    /// Register an `await`'s bound name(s) as value bindings and descend its
    /// value/exec sub-expressions for nested bindings. Shared by the
    /// file-scope (`scope = None`) and block-local (`scope = Some(block)`)
    /// forms. `binding` gets the coarse stmt `range` as its `name_range`
    /// (there's no name-token span on `AwaitStmt`); each `destructure`
    /// `(field, local)` pair binds the LOCAL name, same coarse range.
    fn register_await(&mut self, a: &AwaitStmt, scope: Option<SourceRange>) {
        if let Some(name) = &a.binding {
            self.push(name.clone(), RefNs::Value, a.range.clone(), scope.clone(), "await", false, true);
        }
        if let Some(fields) = &a.destructure {
            // Each destructured local borrows the whole `await` stmt range
            // (no per-name span), so several can share it → coarse.
            for (_field, local) in fields {
                self.push(local.clone(), RefNs::Value, a.range.clone(), scope.clone(), "await", false, true);
            }
        }
        if let Some(e) = &a.value_expr {
            self.walk_expr(e);
        }
        self.walk_expr(&a.exec_expr);
    }

    /// Shared by a file-scope `let` (`scope = None`, `importable = true`)
    /// and a block-local `Stmt::Let` (`scope = Some(enclosing block)`,
    /// `importable = false`). Each bound name uses its alias when present.
    fn register_let(&mut self, l: &LetDecl, scope: Option<SourceRange>, importable: bool) {
        if let Some(t) = &l.typ {
            self.walk_type_expr(t);
        }
        match &l.binding {
            // `LetBinding::Ident.range` is the name token only → precise.
            LetBinding::Ident { name, range } => {
                self.push(name.clone(), RefNs::Value, range.clone(), scope, "let", importable, false)
            }
            // Tuple/record shorthand names all share the one container range
            // → coarse (this is exactly the tie the cursor dispatch breaks).
            LetBinding::Tuple { names, rest, range } => {
                for n in names {
                    self.push(n.clone(), RefNs::Value, range.clone(), scope.clone(), "let", importable, true);
                }
                if let Some(r) = rest {
                    self.push(r.clone(), RefNs::Value, range.clone(), scope, "let", importable, true);
                }
            }
            LetBinding::Record { names, range } => {
                for n in names {
                    self.push(n.clone(), RefNs::Value, range.clone(), scope.clone(), "let", importable, true);
                }
            }
            // `RecordDestructField::Named.range` spans `src: bound` / `x `
            // (trailing), not the bound name alone → coarse (narrowed to the
            // bound name downstream). A non-alias `{ x }` is shorthand: rename
            // expands to `{ x: new }`. `...rest` spans `...` too → coarse.
            LetBinding::RecordDestruct { fields, .. } => {
                for f in fields {
                    match f {
                        RecordDestructField::Named { name, alias, range } => {
                            let bound = alias.clone().unwrap_or_else(|| name.clone());
                            self.push(bound, RefNs::Value, range.clone(), scope.clone(), "let", importable, true);
                            self.mark_last_shorthand(alias.is_none());
                        }
                        RecordDestructField::Rest { name, range } => {
                            self.push(name.clone(), RefNs::Value, range.clone(), scope.clone(), "let", importable, true);
                        }
                    }
                }
            }
        }
    }

    fn walk_block(&mut self, block: &Block) {
        self.walk_stmts(&block.stmts, block.range.clone());
    }

    fn walk_stmts(&mut self, stmts: &[Stmt], scope: SourceRange) {
        for s in stmts {
            self.walk_stmt(s, &scope);
        }
    }

    fn walk_stmt(&mut self, s: &Stmt, scope: &SourceRange) {
        match s {
            Stmt::Var(v) => {
                self.push(v.name.clone(), RefNs::Value, v.range.clone(), Some(scope.clone()), "var", false, true);
                if let Some(t) = &v.typ {
                    self.walk_type_expr(t);
                }
                if let Some(e) = &v.init {
                    self.walk_expr(e);
                }
                if let Some(e) = &v.label_expr {
                    self.walk_expr(e);
                }
            }
            Stmt::Buffer(b) => {
                self.push(b.name.clone(), RefNs::Value, b.range.clone(), Some(scope.clone()), "buffer", false, true);
                if let Some(t) = &b.typ {
                    self.walk_type_expr(t);
                }
                self.walk_expr(&b.init);
            }
            Stmt::Array(a) => {
                self.push(a.name.clone(), RefNs::Value, a.range.clone(), Some(scope.clone()), "array", false, true);
                self.walk_type_expr(&a.element_type);
                for el in &a.init {
                    self.walk_expr(el.expr());
                }
            }
            Stmt::Map(m) => {
                self.push(m.name.clone(), RefNs::Value, m.range.clone(), Some(scope.clone()), "map", false, true);
                self.walk_type_expr(&m.key_type);
                self.walk_type_expr(&m.value_type);
                if let Some(e) = &m.init {
                    self.walk_expr(e);
                }
            }
            Stmt::Let(l) => {
                self.register_let(l, Some(scope.clone()), false);
                self.walk_expr(&l.value);
            }
            Stmt::In(d) => {
                self.push(d.name.clone(), RefNs::Value, d.range.clone(), Some(scope.clone()), "in", false, true);
                self.walk_type_expr(&d.typ);
                if let Some(e) = &d.label_expr {
                    self.walk_expr(e);
                }
            }
            Stmt::OutBinding(ob) => {
                self.push(ob.name.clone(), RefNs::Value, ob.range.clone(), Some(scope.clone()), "out", false, true);
                if let Some(t) = &ob.typ {
                    self.walk_type_expr(t);
                }
                if let Some(e) = &ob.value {
                    self.walk_expr(e);
                }
                if let Some(e) = &ob.label_expr {
                    self.walk_expr(e);
                }
            }
            Stmt::Handler(h) => self.register_handler(h),
            Stmt::AnonChip(ac) => {
                if let Some(e) = &ac.label_expr {
                    self.walk_expr(e);
                }
                self.walk_block(&ac.body);
            }
            Stmt::ChipDecl(c) => self.register_chip(c, Some(scope.clone()), false),
            Stmt::If(i) => self.walk_if(&i.cond, &i.then_block, &i.else_block),
            Stmt::Assign(a) => {
                self.walk_expr(&a.target);
                self.walk_expr(&a.value);
            }
            Stmt::Emit(e) => {
                // `emit NAME = expr` writes the `out`/`var` named NAME — a
                // value use of that binding. The AST has no name-only span on
                // `Emit`, so the whole-stmt range stands in (coarse: true);
                // the wiring layer narrows it to NAME via `find_name_range`.
                self.push_use(e.name.clone(), e.range.clone(), false, true);
                if let Some(v) = &e.value {
                    self.walk_expr(v);
                }
                if let Some(b) = &e.buffer {
                    if let Some(d) = &b.delay {
                        self.walk_expr(d);
                    }
                    if let Some(h) = &b.hold {
                        self.walk_expr(h);
                    }
                }
            }
            Stmt::Await(a) => self.register_await(a, Some(scope.clone())),
            Stmt::ExprStmt(es) => self.walk_expr(&es.expr),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.walk_expr(e);
                }
            }
        }
    }

    fn walk_if(&mut self, cond: &Expr, then_block: &Block, else_block: &Option<Block>) {
        self.walk_expr(cond);
        self.walk_block(then_block);
        if let Some(eb) = else_block {
            self.walk_block(eb);
        }
    }

    /// Descends every `Expr` subtree so nested statement-bearing sub-blocks
    /// (`BlockExpr`, a `MatchExpr` arm's `MatchBody::Block`) get their
    /// locals registered with the right scope extent. Plain value-position
    /// identifiers and calls register a USE (via `push_use`), never a
    /// binding.
    fn walk_expr(&mut self, e: &Expr) {
        match e {
            Expr::Call { callee, args, type_args, .. } => {
                self.walk_expr(callee);
                for a in args {
                    match a {
                        CallArg::Positional(x) | CallArg::Spread(x) => self.walk_expr(x),
                        CallArg::Named { value, .. } => self.walk_expr(value),
                    }
                }
                // Explicit type arguments: `pick<int>(...)`.
                for t in type_args {
                    self.walk_type_expr(t);
                }
            }
            Expr::BinOp { left, right, .. } => {
                self.walk_expr(left);
                self.walk_expr(right);
            }
            Expr::UnOp { operand, .. } | Expr::Deref { operand, .. } | Expr::RefOf { operand, .. } => {
                self.walk_expr(operand)
            }
            Expr::FieldAccess { obj, field, range } => {
                // The field half of `obj.field` is never a rename target
                // (record fields are a deliberate deferral).
                self.model.field_name_spans.push(field_suffix_span(field, range));
                self.walk_expr(obj);
            }
            Expr::TuplePick { obj, .. } => self.walk_expr(obj),
            Expr::IndexAccess { obj, index, .. } => {
                self.walk_expr(obj);
                self.walk_expr(index);
            }
            Expr::IfExpr {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.walk_expr(cond);
                self.walk_expr(then_branch);
                self.walk_expr(else_branch);
            }
            Expr::BlockExpr { stmts, value, range } => {
                self.walk_stmts(stmts, range.clone());
                self.walk_expr(value);
            }
            Expr::InterpLit { parts, .. } => {
                for p in parts {
                    if let InterpPart::Expr(x) = p {
                        self.walk_expr(x);
                    }
                }
            }
            Expr::RecordLit { fields, .. } => {
                for f in fields {
                    match f {
                        // The KEY half of `name: value` is never a rename
                        // target (only the value side is a use).
                        RecordLitField::Named { name, value, range } => {
                            self.model.field_name_spans.push(name_prefix_span(name, range));
                            self.walk_expr(value);
                        }
                        RecordLitField::Spread { value, .. } => self.walk_expr(value),
                        // `{ name }` is shorthand for `{ name: name }` — the
                        // bare field name IS the value use.
                        RecordLitField::Shorthand { name, range } => {
                            self.push_use(name.clone(), range.clone(), true, false);
                        }
                    }
                }
            }
            Expr::MatchExpr {
                scrutinee, arms, ..
            } => {
                self.walk_expr(scrutinee);
                for arm in arms {
                    // `MatchArm.binding` is a captured payload name scoped to
                    // this arm's body. There's no name-token span on the arm,
                    // so use the arm body's range as the extent and the coarse
                    // arm `range` as `name_range` (narrowed later by wiring).
                    let arm_scope = match &arm.body {
                        MatchBody::Expr(x) => x.range().clone(),
                        MatchBody::Block(b) => b.range.clone(),
                    };
                    if let Some(name) = &arm.binding {
                        self.push(name.clone(), RefNs::Value, arm.range.clone(), Some(arm_scope), "capture", false, true);
                    }
                    match &arm.body {
                        MatchBody::Expr(x) => self.walk_expr(x),
                        MatchBody::Block(b) => self.walk_block(b),
                    }
                }
            }
            Expr::Array { elements, .. } => {
                for el in elements {
                    self.walk_expr(el.expr());
                }
            }
            Expr::MapLit { entries, .. } => {
                for en in entries {
                    self.walk_expr(&en.key);
                    self.walk_expr(&en.value);
                }
            }
            // The callee of a call is walked generically above (`Call`'s
            // arm calls `self.walk_expr(callee)`), so a call like `helper(1)`
            // already reaches this arm for `helper` — no separate handling
            // needed for "callee position" vs. any other value position.
            Expr::Ident { name, range } => self.push_use(name.clone(), range.clone(), false, false),
            Expr::IntLit { .. }
            | Expr::AtomLit { .. }
            | Expr::FloatLit { .. }
            | Expr::StringLit { .. }
            | Expr::BoolLit { .. }
            | Expr::NullLit { .. }
            | Expr::AssetRef { .. }
            | Expr::PrefabRef { .. }
            | Expr::NestedPrefab { .. } => {}
        }
    }
}

/// `(line, col)` as a comparison key. Both fields are populated by the
/// lexer as it advances byte-by-byte through the file (see
/// `lexer::Lexer::advance`), so they increase monotonically through a
/// single file and a tuple comparison is a valid "earlier/later in the
/// source" ordering — the same trick `definition.rs` uses for import-range
/// containment. Cursor queries arrive as bare `line`/`col` (no byte offset
/// to compare against), so this — not `Pos::offset` — is what containment
/// checks use throughout this module.
fn pos_key(p: &Pos) -> (u32, u32) {
    (p.line, p.col)
}

/// True if lexical `scope` fully encloses `r` (inclusive at both ends).
fn scope_contains(scope: &SourceRange, r: &SourceRange) -> bool {
    pos_key(&scope.start) <= pos_key(&r.start) && pos_key(&r.end) <= pos_key(&scope.end)
}

/// True if the 1-based `(line, col)` cursor falls within `[r.start, r.end)`
/// — half-open, matching the lexer's snapshot-before/snapshot-after token
/// convention (`end` is one past the last consumed char).
fn range_contains_cursor(r: &SourceRange, line: u32, col: u32) -> bool {
    let p = (line, col);
    pos_key(&r.start) <= p && p < pos_key(&r.end)
}

/// A range's span in source bytes — used only to compare two AST-derived
/// ranges' sizes (never a cursor, which has no byte offset), so
/// `Pos::offset` is fine here even though containment checks above use
/// `(line, col)`.
fn range_len(r: &SourceRange) -> usize {
    r.end.offset.saturating_sub(r.start.offset)
}

/// Resolve every [`Use`] (value AND type) in `model` to the [`Binding`] that
/// (lexically) owns it: among same-name, SAME-NAMESPACE bindings (`b.ns ==
/// u.ns` below is what keeps the value and type namespaces separate — a
/// value binding never matches a type use or vice versa), keep those whose
/// `scope` is `None` (file scope) or contains the use, and pick the one with
/// the smallest (innermost) containing scope — that's the shadowing rule. A
/// `None`-scope binding only wins when no local scope contains the use. A
/// use whose name matches nothing in its namespace (a builtin type name, an
/// undefined global) is left `resolved: None` — not a rename target.
fn resolve_uses(model: &mut ScopeModel) {
    let ScopeModel { bindings, uses, .. } = model;
    for u in uses.iter_mut() {
        // A type use naming a builtin (`character`, `int`, `Map`, …) never
        // resolves to a user binding, even if one happens to share the name
        // (these are plain identifiers, not lexer keywords, so `type
        // character = …` parses — but it must not let a builtin type
        // position get renamed as a side effect of that shadowing). Skip the
        // match entirely rather than relying on it just not finding one.
        if u.ns == RefNs::Type && builtin_type_names().contains(u.name.as_str()) {
            u.resolved = None;
            continue;
        }
        // `current` tracks the winning binding id plus its scope's byte span
        // (`None` = file scope, which loses to any local match).
        let mut current: Option<(usize, Option<usize>)> = None;
        for b in bindings.iter() {
            if b.ns != u.ns || b.name != u.name {
                continue;
            }
            let scope_len = match &b.scope {
                None => None,
                Some(s) => {
                    if !scope_contains(s, &u.range) {
                        continue;
                    }
                    Some(range_len(s))
                }
            };
            let better = match (&current, scope_len) {
                (None, _) => true,
                (Some((_, None)), Some(_)) => true,
                (Some((_, Some(_))), None) => false,
                (Some((_, Some(cur_len))), Some(new_len)) => new_len < *cur_len,
                (Some((_, None)), None) => false,
            };
            if better {
                current = Some((b.id, scope_len));
            }
        }
        u.resolved = current.map(|(id, _)| id);
    }
}

/// Resolve the binding under the cursor and return every same-file site
/// bound to it (including the declaration name). `line`/`col` are 0-based
/// (LSP); converted to the 1-based coordinates AST ranges carry. `source` is
/// the same file's text, used only to break a same-range tie (below).
///
/// Cursor dispatch: among every [`Use`] and every [`Binding::name_range`]
/// whose range contains the cursor, the innermost (smallest) one wins — so a
/// precise use inside a coarser whole-declaration `name_range` takes
/// priority. When several candidates share that same smallest range — the
/// tuple/record `let`, tuple param, and `await` destructure bindings all
/// carry ONE container span (e.g. both `aa` and `bb` in `let (aa, bb) = …`)
/// — the tie is broken by the identifier actually under the cursor
/// (`word_at`), so clicking `bb` resolves to `bb`, not the first-registered
/// `aa`. If that node is a use that didn't resolve to any binding
/// (builtin/global), this returns `None` — [`prepare_rename_at`] turns that
/// into an explicit refusal path.
pub fn references_at(
    script: &Script,
    source: &str,
    file: &str,
    line: usize,
    col: usize,
) -> Option<(RefTarget, Vec<RefSite>)> {
    // `script`/`source` are always a single parsed file (this module does no
    // filesystem I/O — see the module doc), so `file` isn't needed to
    // disambiguate anything. It's kept in the signature per the plan, for
    // parity with `references_to_export`'s cross-file callers.
    let _ = file;
    let model = build_scope_model(script);
    let cursor_line = (line + 1) as u32;
    let cursor_col = (col + 1) as u32;

    enum Cand {
        Use(usize),
        Binding(usize),
    }

    // Collect every use/binding node whose range covers the cursor, then keep
    // only those sharing the smallest (innermost) range.
    let mut cands: Vec<(usize, &str, Cand)> = Vec::new();
    for (i, u) in model.uses.iter().enumerate() {
        if range_contains_cursor(&u.range, cursor_line, cursor_col) {
            cands.push((range_len(&u.range), u.name.as_str(), Cand::Use(i)));
        }
    }
    for (i, b) in model.bindings.iter().enumerate() {
        if range_contains_cursor(&b.name_range, cursor_line, cursor_col) {
            cands.push((range_len(&b.name_range), b.name.as_str(), Cand::Binding(i)));
        }
    }
    let min_len = cands.iter().map(|(len, _, _)| *len).min()?;
    cands.retain(|(len, _, _)| *len == min_len);

    // Tie-break: if more than one node shares the smallest range (container-
    // ranged sibling bindings), prefer the one whose name matches the word
    // literally under the cursor; else fall back to the first registered.
    let chosen = if cands.len() > 1 {
        let word = super::text::word_at(source, line, col);
        cands
            .iter()
            .find(|(_, name, _)| word.as_deref() == Some(name))
            .or_else(|| cands.first())
    } else {
        cands.first()
    };
    let (_, _, cand) = chosen?;

    let binding_id = match cand {
        Cand::Use(i) => model.uses[*i].resolved?,
        Cand::Binding(i) => model.bindings[*i].id,
    };
    let binding = model.bindings.get(binding_id)?;

    let mut sites: Vec<RefSite> = model
        .uses
        .iter()
        .filter(|u| u.resolved == Some(binding_id))
        .map(|u| RefSite {
            range: u.range.clone(),
            is_shorthand: u.is_shorthand,
            coarse: u.coarse,
        })
        .collect();
    sites.push(RefSite {
        range: binding.name_range.clone(),
        is_shorthand: binding.is_shorthand,
        coarse: binding.coarse,
    });

    let cross_file = if let Some(export_name) = &binding.import_export {
        if &binding.name != export_name {
            // ALIASED import (`import { orig as local }`): the alias is a
            // purely file-local name. Renaming it must touch only THIS file's
            // alias specifier token + its uses — never the defining file,
            // whose declaration still spells `orig`, nor a sibling importer
            // that chose a different alias. Classifying it `Local` keeps the
            // whole rename in-file (no cross-file scan, no defining-file
            // resolution — treating it as cross-file would corrupt
            // `mod orig(){…}` by narrowing its decl range against the local
            // alias name).
            CrossFile::Local
        } else {
            // NON-aliased import: the local name IS the export name, so
            // renaming it is genuinely an export rename that must reach the
            // defining file and every other importer.
            CrossFile::Imported {
                export_name: export_name.clone(),
            }
        }
    } else if binding.importable {
        CrossFile::Exported {
            export_name: binding.name.clone(),
        }
    } else {
        CrossFile::Local
    };

    let target = RefTarget {
        name: binding.name.clone(),
        ns: binding.ns,
        kind: binding.kind,
        decl_name_range: binding.name_range.clone(),
        cross_file,
    };
    Some((target, sites))
}

/// The precise renameable NAME span under the cursor plus the current name
/// as a rename placeholder, or `None` when the position is not a renameable
/// binding/use: a builtin type / unresolved global (a `Use` [`references_at`]
/// couldn't resolve to any binding — a builtin call or event name), a lexer
/// keyword, or a record field name (`Expr::FieldAccess.field`,
/// `RecordLitField::Named.name`, or a record TYPE field's name — deferred
/// per the plan's Global Constraints, not renamed by this plan).
///
/// Builds on [`references_at`]'s own cursor dispatch, which already refuses
/// the builtin/unresolved-global case for free (an unresolved `Use` makes it
/// return `None`). The keyword and field-name refusals run FIRST and
/// independently of that dispatch, because a coarser declaration's
/// `name_range` can span far enough to swallow either: `var`/`buffer`/
/// `array`/`map`/`in`/`out`/`chip`/`mod`/`type` bindings all carry a coarse
/// `name_range` running from their leading KEYWORD through their whole
/// initializer/body (see `parser.rs`'s `parse_var_decl` etc.), so e.g. the
/// cursor sitting on the `var` keyword itself, or on `field` inside
/// `var x: int = p.field`, would otherwise resolve as if it were on `x`.
pub fn prepare_rename_at(
    script: &Script,
    source: &str,
    file: &str,
    line: usize,
    col: usize,
) -> Option<(SourceRange, String)> {
    if let Some(word) = super::text::word_at(source, line, col) {
        if crate::lexer::KEYWORDS.contains(&word.as_str()) {
            return None;
        }
    }

    let cursor_line = (line + 1) as u32;
    let cursor_col = (col + 1) as u32;
    let model = build_scope_model(script);
    if model
        .field_name_spans
        .iter()
        .any(|r| range_contains_cursor(r, cursor_line, cursor_col))
    {
        return None;
    }

    let (target, sites) = references_at(script, source, file, line, col)?;

    // Among this target's own sites (decl + every resolved use), the one
    // whose range actually contains the cursor is the exact span to rename —
    // `decl_name_range` when the cursor sat on the decl, a specific use's
    // range otherwise. Smallest containing range wins (mirrors
    // `references_at`'s own dispatch) so a precise self-referential use
    // inside a coarse decl's own range (e.g. `var x: int = x + 1`) still
    // resolves to itself rather than the whole declaration.
    let chosen = sites
        .iter()
        .filter(|s| range_contains_cursor(&s.range, cursor_line, cursor_col))
        .min_by_key(|s| range_len(&s.range))?;

    let span = if chosen.coarse {
        super::references::find_name_range(source, &chosen.range, &target.name)
            .unwrap_or_else(|| chosen.range.clone())
    } else {
        chosen.range.clone()
    };
    Some((span, target.name))
}

/// True if the 0-based `(line, col)` cursor sits on a record FIELD name — a
/// `FieldAccess.field` (`obj.field`), a `RecordLitField::Named` KEY
/// (`{ name: value }`), or a record-TYPE field's own name — exactly the
/// positions [`prepare_rename_at`] refuses (field rename is deferred per the
/// plan's Global Constraints). The LSP uses this to route a field click to
/// its type-directed goto-definition path instead of the scope-aware
/// reference set (a coarse enclosing `type X = {…}` binding would otherwise
/// swallow the field cursor and surface the whole type's references).
pub fn field_name_at(script: &Script, line: usize, col: usize) -> bool {
    let cursor_line = (line + 1) as u32;
    let cursor_col = (col + 1) as u32;
    let model = build_scope_model(script);
    model
        .field_name_spans
        .iter()
        .any(|r| range_contains_cursor(r, cursor_line, cursor_col))
}

/// A token classification for [`semantic_tokens`] — the LSP-facing category
/// `crates/lsp` maps onto its `SemanticTokensLegend` (index assignment is the
/// LSP wiring's job, not this module's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemTokenKind {
    Type,
    Function,
    Parameter,
    Variable,
    Namespace,
}

/// One semantic-token override. The TextMate grammar
/// (`editors/vscode/syntaxes/wirescript.tmLanguage.json`) colors as
/// `support.type` every occurrence of a builtin type keyword AND every
/// capitalized identifier (`[A-Z][A-Za-z0-9_]*`) — position-blind, so a
/// capitalized VALUE binding (a `character` handler capture, a capitalized
/// `var`) shows as a type even though it isn't one. [`semantic_tokens`] only
/// emits a span where it disagrees with what the grammar would already
/// color, so a client that layers these atop the grammar (VS Code does) sees
/// a correction only where one is needed.
#[derive(Clone, Debug)]
pub struct SemSpan {
    pub range: SourceRange,
    pub name: String,
    pub kind: SemTokenKind,
    /// Same meaning as [`Use::coarse`]/[`Binding::coarse`] — the LSP wiring
    /// narrows a coarse span to the precise name token via
    /// [`super::references::find_name_range`] before emitting a token, and
    /// skips the span entirely when narrowing fails (never tokenizes a whole
    /// coarse container).
    pub coarse: bool,
}


/// Record one [`SemSpan`], deduped by `(start line, start col)` — several
/// candidate sites (a use and its own declaration, or several coarse
/// container-sharing bindings) can legitimately share one span, and the LSP
/// wiring only wants one token per position. First write wins.
fn record_sem_span(
    spans: &mut Vec<SemSpan>,
    seen: &mut HashSet<(u32, u32)>,
    range: SourceRange,
    name: String,
    kind: SemTokenKind,
    coarse: bool,
) {
    if seen.insert((range.start.line, range.start.col)) {
        spans.push(SemSpan { range, name, kind, coarse });
    }
}

/// Semantic-token overrides for the TextMate grammar's position-blind type
/// coloring (see [`SemSpan`]): every use/decl [`build_scope_model`]'s scope
/// resolver classifies differently from what the grammar would guess from
/// spelling alone.
///
/// - A TYPE-namespace [`Use`] always emits a `Type` token — this is what
///   gives a user `type` ALIAS the type color the grammar's fixed builtin
///   list doesn't know about (the grammar only recognizes the builtin type
///   keywords by name, not a `type Foo = …` alias).
/// - A VALUE-namespace `Use` whose name the grammar would mis-color as a
///   type ([`grammar_would_type`]) emits a token in the binding's own
///   category ([`kind_token`]) ONLY when it resolved to a user [`Binding`] —
///   an unresolved value use (a builtin/global call like `DisplayText`) is
///   left alone, so the grammar's own (correct, for a builtin) coloring
///   stands.
/// - Every [`Binding`] gets the matching treatment: a TYPE binding emits
///   `Type`; a VALUE binding whose name the grammar would mis-color emits
///   its [`kind_token`]. Import bindings (`b.kind == "import"`) are skipped
///   entirely — they're dual value/type-registered, so classifying them
///   here would be a guess, not a fact the resolver actually knows; the
///   grammar's own coloring stands instead.
///
/// A compiler-synthesized identifier that stands in for real user source (its
/// range overlaps user code, so coloring it would mis-paint that code). The
/// general-expression handler trigger `on <call>()` desugars to a synthetic
/// `let _on_expr_N = <call>` + `on _on_expr_N`, whose binding + trigger use
/// both span the whole `<call>` — semantic tokens must skip it so the call
/// keeps its own coloring.
fn is_synthetic_ident(name: &str) -> bool {
    name.starts_with("_on_expr_")
}

/// The result is unsorted and safe to contain no duplicate-position spans
/// (deduped by [`record_sem_span`]) — the LSP wiring sorts before
/// delta-encoding into the protocol's token stream.
pub fn semantic_tokens(script: &Script) -> Vec<SemSpan> {
    let model = build_scope_model(script);
    let mut spans = Vec::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::default();

    // Type-position names → Type (incl. user `type` aliases the grammar's
    // builtin list misses). Every USER value identifier → one uniform Variable
    // token, so a value binding whose spelling collides with a type keyword
    // (`character`), a builtin function (`round`), or the grammar's "any
    // capitalized word is a type" rule reads as a plain, consistent identifier
    // rather than being mis-colored. An UNRESOLVED value use is a genuine
    // builtin/global (a real `round(...)` call, `DisplayText`, …) — left to the
    // grammar's function coloring.
    for u in &model.uses {
        if is_synthetic_ident(&u.name) {
            continue;
        }
        match u.ns {
            RefNs::Type => {
                record_sem_span(&mut spans, &mut seen, u.range.clone(), u.name.clone(), SemTokenKind::Type, u.coarse)
            }
            RefNs::Value => {
                if u.resolved.is_some() {
                    record_sem_span(&mut spans, &mut seen, u.range.clone(), u.name.clone(), SemTokenKind::Variable, u.coarse);
                }
            }
        }
    }

    for b in &model.bindings {
        if b.kind == "import" || is_synthetic_ident(&b.name) {
            continue;
        }
        let kind = match b.ns {
            RefNs::Type => SemTokenKind::Type,
            RefNs::Value => SemTokenKind::Variable,
        };
        record_sem_span(&mut spans, &mut seen, b.name_range.clone(), b.name.clone(), kind, b.coarse);
    }

    spans
}

/// Every site in `script` that an EXPORT rename of `export_name` (namespace
/// `ns`, defined in another file) must edit here — safe whether this file
/// imports the name plainly or under an alias:
///
/// - A NON-aliased specifier (`import { orig }`) uses `ImportBinding.range`
///   directly — per `ast.rs` that range IS the name token (there's no alias
///   to prefer), so it's already precise (`coarse: false`) and needs no
///   downstream narrowing.
/// - An ALIASED specifier (`import { orig as x }`) has no precise range for
///   `orig` on its own — `ImportBinding.range` covers the ALIAS token `x`
///   instead (`ast.rs`: "Range of the effective identifier (alias if
///   present, otherwise name)") — so this falls back to a COARSE site
///   spanning the whole `import { … } from "…"` declaration (`ImportDecl.range`).
///   The wiring layer narrows it via `references::find_name_range` against
///   `export_name`, which lands on the `orig` token in the specifier list,
///   leaving the alias `x` untouched.
/// - The import's resolved USE sites are returned ONLY for a NON-aliased
///   import (`alias.is_none()`), because only then do the uses spell the
///   export name. An aliased import's uses spell the local alias, which an
///   export rename must NOT change — so those sites are deliberately omitted.
///
/// A same-named LOCAL binding that shadows the import resolves to its own
/// binding via `resolve_uses`'s innermost-scope rule, so it never appears in
/// an import binding's use set — no extra filtering needed. Like the rest of
/// this module, this only ever walks AST sites, never comments/string text.
///
/// `script`/`file` are always a single parsed file (this module does no
/// filesystem I/O — see the module doc); `file` isn't needed to disambiguate
/// anything since callers already picked which file's AST to pass in. Kept
/// in the signature per the plan, matching `references_at`'s same unused
/// `file` parameter.
///
/// Returns an empty `Vec` when `script` has no `import { export_name }` (or
/// `import { export_name as … }`) specifier — e.g. when called speculatively
/// against a sibling file that doesn't import the renamed symbol at all.
pub fn references_to_export(script: &Script, file: &str, export_name: &str, ns: RefNs) -> Vec<RefSite> {
    let _ = file;
    let model = build_scope_model(script);
    let mut sites = Vec::new();

    for d in &script.decls {
        let TopDecl::Import(imp) = d else { continue };
        let ImportKind::Named(bindings) = &imp.kind else { continue };
        for b in bindings {
            if b.name != export_name {
                continue;
            }
            // The specifier: a NON-aliased binding's own `range` IS the
            // precise name token (no narrowing needed); an ALIASED binding's
            // `range` covers the ALIAS instead, so fall back to the coarse
            // whole-`import …` span narrowed to `orig` downstream.
            if b.alias.is_none() {
                sites.push(RefSite {
                    range: b.range.clone(),
                    is_shorthand: false,
                    coarse: false,
                });
            } else {
                sites.push(RefSite {
                    range: imp.range.clone(),
                    is_shorthand: false,
                    coarse: true,
                });
            }
            // Uses of the imported name — only when NON-aliased, since an
            // aliased import's uses spell the alias, which an export rename
            // must leave alone.
            if b.alias.is_none() {
                // The model binding for this non-aliased specifier (its local
                // name equals the export name) carries the resolved uses.
                if let Some(binding) = model.bindings.iter().find(|mb| {
                    mb.ns == ns
                        && mb.import_export.as_deref() == Some(export_name)
                        && mb.name.as_str() == export_name
                }) {
                    let id = binding.id;
                    for u in model.uses.iter().filter(|u| u.resolved == Some(id)) {
                        sites.push(RefSite {
                            range: u.range.clone(),
                            is_shorthand: u.is_shorthand,
                            coarse: u.coarse,
                        });
                    }
                }
            }
        }
    }
    sites
}
