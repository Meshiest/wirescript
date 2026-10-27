//! Scope-aware name resolution behind find-references and rename.
//!
//! An AST walk tracks a lexical scope stack with two namespaces, values and
//! types. Every binding site (`var`, `let`, parameters, handler captures, chip,
//! mod, type and event declarations, and so on) registers a [`Binding`] with
//! its scope extent, and every identifier use resolves to the nearest enclosing
//! binding in its namespace. Rename therefore touches only sites that resolve
//! to the same declaration: never comments, strings, type-position names,
//! other scopes or unrelated files.
//!
//! - [`build_scope_model`] registers every binding and every value- and
//!   type-position use; [`resolve_uses`] resolves them, matching namespaces.
//! - [`references_at`] returns every same-file site bound to the binding under
//!   a cursor. [`prepare_rename_at`] adds the refusals it does not cover: a
//!   lexer keyword and a record field name.
//! - A named import specifier registers a binding in both namespaces under its
//!   local name, tagged with the exported name (`Binding::import_export`), so a
//!   use of it classifies as `CrossFile::Imported`. [`references_to_export`]
//!   goes the other way: every site in another file's AST that traces back to
//!   an exported name. Namespace members (`u.foo`) are not renamed.

use super::text::{char_col_to_byte, line_text, word_at};
use crate::ast::walk;
use crate::ast::*;
use crate::collections::{HashMap, HashSet};
use crate::diagnostic::{Pos, SourceRange};
use crate::ir::plane::{ForSlot, LoopKind, for_slots, plane_spec};
use crate::scope::top::{Home, ReadAt, TopNames, within};
use std::sync::OnceLock;

#[cfg(test)]
mod tests;

/// Which lexical namespace a name lives in. Value names (`var`, `let`,
/// parameters) and type names (`type`, type parameters) are separate, so a value
/// named `character` is invisible to a `TypeExpr::Name` `character` and the
/// other way round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// True when `range` is a whole-declaration span rather than a name token: the
    /// declaration sites of container-ranged kinds (`var`, `chip`, `mod`, `type`,
    /// tuple or record `let`, `emit NAME = ...`) and uses with only a coarse range
    /// (`Emit.name`'s statement, a `TypeExpr::Generic` head). The LSP narrows these
    /// to the name with `references::find_name_range`.
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

/// A name-binding site: a declaration that introduces `name` into scope. A
/// `Binding` never itself refers to another binding (that is a [`Use`]).
#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub(crate) id: usize,
    pub(crate) name: String,
    pub(crate) ns: RefNs,
    /// The declaration's name span: the name token where the AST has a range for
    /// it (`LetBinding::Ident`, `HandlerParam`, `RecordDestructField::Named`),
    /// else the whole declaration (`VarDecl`, `ChipDecl`, `InDecl`, and so on).
    pub(crate) name_range: SourceRange,
    /// `None` = visible for the whole file (file-scope decl); `Some(r)` =
    /// visible only within lexical extent `r` (the owning block/handler/
    /// chip-or-mod body's range).
    pub(crate) scope: Option<SourceRange>,
    /// A tag for the binding's source construct ("var", "let", "param",
    /// "capture", "chip", "mod", "event", "type", "typeparam", and so on) for
    /// diagnostics and tests; code outside this module does not match on it.
    pub(crate) kind: &'static str,
    /// True only for file-scope chip, mod, let, type and event bindings, the names
    /// another file can import. Every other binding is `false`, even with `scope`
    /// `None`.
    pub(crate) importable: bool,
    /// True when `name_range` is a whole-declaration span (the LSP narrows it with
    /// `find_name_range`); false only for handler captures and `LetBinding::Ident`.
    /// Several coarse bindings can share one range (a tuple or record `let`, a tuple
    /// parameter, an `await` destructure), which is why cursor dispatch breaks ties
    /// on the identifier under the cursor. `RecordDestructField` ranges are coarse
    /// too (they span `src: bound`).
    pub(crate) coarse: bool,
    /// True for a record-destructure shorthand site (`let { x } = p`,
    /// `mod f({ x }: P)`): renaming its local expands to `{ x: new }`, as record
    /// literal shorthand does. An aliased `{ src: bound }` is plain-replaced.
    pub(crate) is_shorthand: bool,
    /// The exported name for a binding from a named import specifier
    /// (`import { name }`, `import { orig as name }`), looked up among the source
    /// file's exports. `None` for every other binding, a namespace alias included.
    pub(crate) import_export: Option<String>,
    /// For a statement's `var` or `let`: the byte offset its name is visible
    /// from, the end of the declaration. A read before it, the initializer's
    /// own included, reads what the name meant there.
    pub(crate) visible_from: Option<usize>,
    /// For a declaration of an anonymous chip at the top level (storage, a
    /// `let` or `const`): the chip. Inside it the declaration is what its
    /// name means; outside it, a declaration of the name made elsewhere at
    /// the top level is (see `scope::top`).
    pub(crate) home: Option<SourceRange>,
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
    /// True for a use whose `range` is a whole statement or type expression rather
    /// than the identifier: `Emit.name` (`emit NAME = expr`) and a
    /// `TypeExpr::Generic` head (its range covers `Name<Args>`).
    pub(crate) coarse: bool,
}

/// The whole-program binding + use inventory a cursor query resolves
/// against.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScopeModel {
    pub(crate) bindings: Vec<Binding>,
    pub(crate) uses: Vec<Use>,
    /// Spans of record field names, which are never renamed: the `field` of
    /// `obj.field`, the key of `name: value` in a record literal, and a record
    /// type field's name. None of those nodes has a name-only range, so [`Walker`]
    /// computes each one ([`field_suffix_span`], [`name_prefix_span`]) as it visits.
    /// [`prepare_rename_at`] checks these first, since a coarse declaration range
    /// (`var x: int = p.field`) can contain a field cursor.
    pub(crate) field_name_spans: Vec<SourceRange>,
    /// What each free name means at the top level (see `scope::top`).
    pub(crate) top: TopNames,
    /// Each top-level declaration, as the top level is read from it: a
    /// `mod` or `chip` as a body, anything else as code.
    pub(crate) top_reads: Vec<ReadAt>,
}

/// The span of the `field`-length identifier that ends exactly at `range.end`,
/// for `Expr::FieldAccess`, whose range ends at the field token. Identifiers are
/// ASCII (`lexer::is_ident_start`), so byte length equals column width.
fn field_suffix_span(field: &str, range: &SourceRange) -> SourceRange {
    let len = field.len() as u32;
    SourceRange {
        file: std::sync::Arc::clone(&range.file),
        start: Pos {
            offset: range.end.offset.saturating_sub(field.len()),
            line: range.end.line,
            col: range.end.col.saturating_sub(len),
        },
        end: range.end,
    }
}

/// The span of the `name`-length identifier that starts exactly at
/// `range.start`, for `RecordLitField::Named` (`name: value`) and
/// `RecordTypeField` (`name: Type`), both of which start at the name token.
fn name_prefix_span(name: &str, range: &SourceRange) -> SourceRange {
    let len = name.len() as u32;
    SourceRange {
        file: std::sync::Arc::clone(&range.file),
        start: range.start,
        end: Pos {
            offset: range.start.offset + name.len(),
            line: range.start.line,
            col: range.start.col + len,
        },
    }
}

/// The type-namespace globals: builtin type names and the generic heads the
/// parser desugars (`Array<V>` and `Ref<V>` become `TypeExpr::Array`/`Ref`;
/// `Map` stays a `Generic`). They never resolve to a user [`Binding`], so a
/// `TypeExpr::Name` naming one is refused for rename.
pub(crate) fn builtin_type_names() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        crate::types::resolve::PRIMITIVES
            .iter()
            .map(|(n, _)| *n)
            .chain(["Map", "Ref", "Array"])
            .collect()
    })
}

/// Walk `script`, registering every value and type binding and every value- and
/// type-position use, then resolve each use to the binding that owns it in its
/// own namespace. Resolution runs after the walk so a use can bind to a
/// declaration later in the source (see [`resolve_uses`]).
pub(crate) fn build_scope_model(script: &Script) -> ScopeModel {
    let top = TopNames::of(&script.decls);
    let top_reads = script
        .decls
        .iter()
        .map(|d| match d {
            TopDecl::Chip(_) => ReadAt::body(d.range()),
            // A `var` initializer reads the declaration before the `var`.
            TopDecl::Var(v) => top
                .initializer_read(&v.name, &v.range)
                .unwrap_or_else(|| ReadAt::code(d.range())),
            _ => ReadAt::code(d.range()),
        })
        .collect();
    let mut model = ScopeModel {
        top,
        top_reads,
        ..Default::default()
    };
    let mut walker = Walker { model: &mut model };
    walker.walk_script(script);
    widen_hoisted_out_bindings(script, &mut model);
    resolve_uses(&mut model);
    model
}

/// An `out` written in a top-level handler body, a captured event body or an
/// anonymous chip body is a boundary port, not a block-local binding. The walk
/// registers every `Stmt::OutBinding` with its block as `scope` (an `out` inside
/// a named chip or mod stays local, WS073), so this widens the hoisted ones to
/// the frame the port lands in, using the same sites typecheck
/// (`register_handler_outputs`, `register_anon_chip_outputs`) and lowering
/// (`pre_declare_handler_outputs`, `pre_declare_anon_chip`) claim.
///
/// The frame is `None` (file-wide) for a top-level handler, a captured event and
/// a file-scope anonymous chip; a statement-level `chip { }` widens to the block
/// holding it, and a nested one to its parent's frame. Matched by `name_range`,
/// which is unique per occurrence.
fn widen_hoisted_out_bindings(script: &Script, model: &mut ScopeModel) {
    let mut hoisted: Vec<(SourceRange, Option<SourceRange>)> = Vec::new();
    for d in &script.decls {
        match d {
            TopDecl::Handler(h) => {
                collect_hoisted_handler_outs(&h.body, &mut hoisted);
                collect_anon_chip_outs(&h.body.stmts, Some(&h.body.range), &mut hoisted);
            }
            TopDecl::Event(e) => {
                if let Some(b) = e.captured_body() {
                    collect_hoisted_handler_outs(b, &mut hoisted);
                    collect_anon_chip_outs(&b.stmts, Some(&b.range), &mut hoisted);
                }
            }
            TopDecl::AnonChip(ac) => {
                collect_anon_chip_body_outs(&ac.body.stmts, None, &mut hoisted);
            }
            TopDecl::Chip(c) => {
                collect_anon_chip_outs(&c.body.stmts, Some(&c.body.range), &mut hoisted);
            }
            _ => {}
        }
    }
    if hoisted.is_empty() {
        return;
    }
    for b in &mut model.bindings {
        if b.kind == "out"
            && let Some((_, frame)) = hoisted.iter().find(|(r, _)| *r == b.name_range)
        {
            b.scope = frame.clone();
        }
    }
}

/// Widen the ports an anon chip body declares DIRECTLY - every non-`@side`
/// `Stmt::OutBinding` among `stmts`, the shallow scan
/// `register_anon_chip_outputs` performs - to `frame`, then keep walking for
/// anon chips nested deeper. A conditional or nested-handler `out` inside an
/// anon chip body is not a port of it (WS023 for `@side`, WS073 for the
/// nested-handler case), so only a direct one counts.
fn collect_anon_chip_body_outs(
    stmts: &[Stmt],
    frame: Option<&SourceRange>,
    out: &mut Vec<(SourceRange, Option<SourceRange>)>,
) {
    for o in crate::ast::anon_chip_port_sites(stmts) {
        out.push((o.range.clone(), frame.cloned()));
    }
    collect_anon_chip_outs(stmts, frame, out);
}

/// Find every statement-level `chip { }` reachable from `stmts` and widen its
/// ports to the frame `check_anon_chip_stmts` registers them in. `frame` is the
/// scope of the block holding these statements (`None` at file scope); each
/// block that opens a typecheck frame passes its own range down, while an anon
/// chip - which shares its parent's frame - passes `frame` through unchanged.
fn collect_anon_chip_outs(
    stmts: &[Stmt],
    frame: Option<&SourceRange>,
    out: &mut Vec<(SourceRange, Option<SourceRange>)>,
) {
    for s in stmts {
        match s {
            Stmt::AnonChip(ac) => collect_anon_chip_body_outs(&ac.body.stmts, frame, out),
            Stmt::If(i) => {
                collect_anon_chip_outs(&i.then_block.stmts, Some(&i.then_block.range), out);
                if let Some(eb) = &i.else_block {
                    collect_anon_chip_outs(&eb.stmts, Some(&eb.range), out);
                }
            }
            Stmt::IfLet(i) => {
                collect_anon_chip_outs(&i.then_block.stmts, Some(&i.then_block.range), out);
                if let Some(eb) = &i.else_block {
                    collect_anon_chip_outs(&eb.stmts, Some(&eb.range), out);
                }
            }
            Stmt::LetElse(l) => {
                collect_anon_chip_outs(&l.else_block.stmts, Some(&l.else_block.range), out);
            }
            Stmt::Handler(h) => collect_anon_chip_outs(&h.body.stmts, Some(&h.body.range), out),
            Stmt::ChipDecl(c) => collect_anon_chip_outs(&c.body.stmts, Some(&c.body.range), out),
            _ => {}
        }
    }
}

/// Collect every site `ast::handler_port_sites` names - the set lowering's
/// `pre_declare_handler_outputs` and typecheck's `register_handler_outputs`
/// claim, shared with them so this third system agrees by construction. Every
/// one of them is file-wide: `register_handler_outputs` runs from the top-level
/// dispatch, with no frame pushed for the handler body.
fn collect_hoisted_handler_outs(block: &Block, out: &mut Vec<(SourceRange, Option<SourceRange>)>) {
    for o in crate::ast::handler_port_sites(block) {
        out.push((o.range.clone(), None));
    }
}

struct Walker<'a> {
    model: &'a mut ScopeModel,
}

impl Walker<'_> {
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is one field of the recorded binding"
    )]
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
            visible_from: None,
            home: None,
        });
    }

    /// Make the bindings pushed from index `from` on visible only after the
    /// declaration at `range`.
    fn visible_after(&mut self, from: usize, range: &SourceRange) {
        for b in &mut self.model.bindings[from..] {
            b.visible_from = Some(range.end.offset);
        }
    }

    /// Mark the binding `push` just appended as a record-destructure shorthand
    /// site. Only the two `RecordDestructField::Named` paths (let and parameter)
    /// set it.
    fn mark_last_shorthand(&mut self, is_shorthand: bool) {
        if let Some(b) = self.model.bindings.last_mut() {
            b.is_shorthand = is_shorthand;
        }
    }

    /// Mark the binding `push` just appended as a named-import local, recording
    /// the exported name it was imported under for `references_at` and
    /// `references_to_export`.
    fn mark_last_import_export(&mut self, export_name: String) {
        if let Some(b) = self.model.bindings.last_mut() {
            b.import_export = Some(export_name);
        }
    }

    /// Record a value-position identifier (`Expr::Ident`, a call's callee, a
    /// record-literal shorthand field). It is resolved after the walk (see
    /// [`resolve_uses`]), since it may name a declaration not walked yet.
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

    /// Record a type-position identifier (`TypeExpr::Name`, or a `Generic`'s head),
    /// resolved against type bindings after the walk. A builtin name (`character`,
    /// `int`, `Map`) never matches a user [`Binding`], so it stays unresolved and is
    /// not a rename target.
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

    /// Descend a type annotation, recording a type-namespace [`Use`] for every
    /// named type in it: a `TypeExpr::Name` precisely, a `Generic` head coarsely.
    /// `Ref`, `Array`, `Tuple`, `Union` and `Record` carry no name and just recurse.
    fn walk_type_expr(&mut self, t: &TypeExpr) {
        match t {
            TypeExpr::Name { name, range } => {
                self.push_type_use(name.clone(), range.clone(), false);
            }
            TypeExpr::Ref { inner, .. } | TypeExpr::Array { inner, .. } => {
                self.walk_type_expr(inner);
            }
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
                    self.model
                        .field_name_spans
                        .push(name_prefix_span(&f.name, &f.range));
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
        for e in [
            &script.module_label,
            &script.module_color,
            &script.module_material,
        ]
        .into_iter()
        .flatten()
        {
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
                self.push(
                    v.name.clone(),
                    RefNs::Value,
                    v.range.clone(),
                    None,
                    "var",
                    false,
                    true,
                );
                if let Some(t) = &v.typ {
                    self.walk_type_expr(t);
                }
                if let Some(e) = &v.init {
                    self.walk_expr(e);
                }
                for e in [&v.label_expr, &v.color_expr, &v.material_expr]
                    .into_iter()
                    .flatten()
                {
                    self.walk_expr(e);
                }
            }
            TopDecl::Array(a) => {
                self.push(
                    a.name.clone(),
                    RefNs::Value,
                    a.range.clone(),
                    None,
                    "array",
                    false,
                    true,
                );
                self.walk_type_expr(&a.element_type);
                for el in &a.init {
                    self.walk_expr(el.expr());
                }
            }
            TopDecl::Map(m) => {
                self.push(
                    m.name.clone(),
                    RefNs::Value,
                    m.range.clone(),
                    None,
                    "map",
                    false,
                    true,
                );
                self.walk_type_expr(&m.key_type);
                self.walk_type_expr(&m.value_type);
                if let Some(e) = &m.init {
                    self.walk_expr(e);
                }
            }
            TopDecl::Buffer(b) => {
                self.push(
                    b.name.clone(),
                    RefNs::Value,
                    b.range.clone(),
                    None,
                    "buffer",
                    false,
                    true,
                );
                if let Some(t) = &b.typ {
                    self.walk_type_expr(t);
                }
                self.walk_expr(&b.init);
            }
            TopDecl::Chip(c) => self.register_chip(c, None, true),
            TopDecl::AnonChip(ac) => self.walk_anon_chip(ac, None, None),
            TopDecl::Event(e) => {
                self.push(
                    e.name.clone(),
                    RefNs::Value,
                    e.range.clone(),
                    None,
                    "event",
                    true,
                    true,
                );
                match &e.captured {
                    Some(h) => self.register_handler(h),
                    None => self.walk_expr(&e.source),
                }
            }
            TopDecl::In(d) => {
                self.push(
                    d.name.clone(),
                    RefNs::Value,
                    d.range.clone(),
                    None,
                    "in",
                    false,
                    true,
                );
                self.walk_type_expr(&d.typ);
                if let Some(e) = &d.label_expr {
                    self.walk_expr(e);
                }
            }
            TopDecl::Out(o) => {
                self.push(
                    o.name.clone(),
                    RefNs::Value,
                    o.range.clone(),
                    None,
                    "out",
                    false,
                    true,
                );
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
            // A top-level `let` is named from its declaration on, as the
            // compiler reads it; a top-level `var` is declared up front.
            TopDecl::Let(l) => {
                let from = self.model.bindings.len();
                self.register_let(l, None, true);
                self.visible_after(from, &l.range);
                self.walk_expr(&l.value);
            }
            // Task 18: parser-only stub, same minimal treatment as
            // `Expr::MatchExpr`'s arms - walk the scrutinee, register the
            // pattern's captures, walk the else block. File-scope (`None`),
            // matching `TopDecl::Let` above.
            TopDecl::LetElse(l) => {
                self.walk_expr(&l.scrutinee);
                self.walk_pattern_bindings(&l.pattern, None);
                self.walk_block(&l.else_block);
            }
            TopDecl::Await(a) => self.register_await(a, None),
            TopDecl::Assign(a) => {
                self.walk_expr(&a.target);
                self.walk_expr(&a.value);
            }
            TopDecl::If(i) => self.walk_if(&i.cond, &i.then_block, i.else_block.as_ref()),
            TopDecl::IfLet(i) => {
                self.walk_if_let(
                    &i.pattern,
                    &i.scrutinee,
                    &i.then_block,
                    i.else_block.as_ref(),
                );
            }
            TopDecl::ExprStmt(es) => self.walk_expr(&es.expr),
            TopDecl::TypeAlias(t) => self.register_type_alias(t),
            TopDecl::Import(imp) => self.register_import(imp),
            TopDecl::Enum(_) => {}
        }
    }

    /// `import { name }` and `import { orig as name }` bindings: each registers a
    /// file-scope binding under its local name at the specifier's precise span
    /// (`ImportBinding.range`). The source file is not read here, so whether the
    /// name is a value or a type is unknown: it is registered in both namespaces.
    /// It is not importable itself. `ImportKind::All` and `Namespace` bring in no
    /// named bindings (a resolved namespace alias is a `TopDecl::Namespace`).
    fn register_import(&mut self, imp: &ImportDecl) {
        let ImportKind::Named(bindings) = &imp.kind else {
            return;
        };
        for b in bindings {
            let local = b.alias.clone().unwrap_or_else(|| b.name.clone());
            self.push(
                local.clone(),
                RefNs::Value,
                b.range.clone(),
                None,
                "import",
                false,
                false,
            );
            self.mark_last_import_export(b.name.clone());
            self.push(
                local,
                RefNs::Type,
                b.range.clone(),
                None,
                "import",
                false,
                false,
            );
            self.mark_last_import_export(b.name.clone());
        }
    }

    /// Shared by a file-scope `chip`/`mod` (`TopDecl::Chip`, `scope = None`,
    /// `importable = true`) and a nested `Stmt::ChipDecl` (`scope =
    /// Some(enclosing block range)`, `importable = false`). `type_params` are
    /// scoped to the WHOLE decl range (`c.range`, not just the body) since
    /// they're visible from the input/output type annotations too, both of
    /// which precede the body in source order.
    fn register_chip(&mut self, c: &ChipDecl, scope: Option<SourceRange>, importable: bool) {
        let kind = c.kind_label();
        self.push(
            c.name.clone(),
            RefNs::Value,
            c.range.clone(),
            scope,
            kind,
            importable,
            true,
        );
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
        for e in [&c.label_expr, &c.color_expr, &c.material_expr]
            .into_iter()
            .flatten()
        {
            self.walk_expr(e);
        }
        self.walk_block(&c.body);
    }

    /// `type Name<T, U: Bound> = ...` (file scope, importable, `kind = "type"`)
    /// plus each of its type params, scoped to the whole alias `range` (RHS
    /// type positions and a later param's bound can both reference an
    /// earlier one).
    fn register_type_alias(&mut self, t: &TypeAliasDecl) {
        // `TypeAliasDecl.range` spans the whole `type X = ...` decl (no
        // name-only span) -> coarse.
        self.push(
            t.name.clone(),
            RefNs::Type,
            t.range.clone(),
            None,
            "type",
            true,
            true,
        );
        for tp in &t.type_params {
            self.register_type_param(tp, t.range.clone());
        }
        self.walk_type_expr(&t.typ);
    }

    /// A generic type param (`T` or `T: Bound`) on a `chip`/`mod`/`type`
    /// decl. `TypeParam.range` spans the bound too when present (`parser.rs`:
    /// `make_range(name_tok.start, bound.range().end)`), so it's not reliably
    /// name-only -> always coarse. The bound itself is a type-position
    /// reference (e.g. `Numeric`), not a binding, so it's walked as a use,
    /// not registered.
    fn register_type_param(&mut self, tp: &TypeParam, scope: SourceRange) {
        self.push(
            tp.name.clone(),
            RefNs::Type,
            tp.range.clone(),
            Some(scope),
            "typeparam",
            false,
            true,
        );
        if let Some(bound) = &tp.bound {
            self.walk_type_expr(bound);
        }
    }

    /// A plain parameter binds its name; a destructured one binds each field's name
    /// (or alias) or tuple name, plus an optional `...rest`. All of them share the
    /// parameter's `scope` (the owning body). `p.typ` types the whole value and is
    /// walked once.
    fn register_param(&mut self, p: &Param, scope: SourceRange) {
        self.walk_type_expr(&p.typ);
        match &p.pattern {
            // Plain param: `p.range` is the whole param span -> coarse.
            None => self.push(
                p.name.clone(),
                RefNs::Value,
                p.range.clone(),
                Some(scope),
                "param",
                false,
                true,
            ),
            Some(ParamPattern::Record { fields, rest }) => {
                for f in fields {
                    match f {
                        // `RecordDestructField::Named.range` spans `src: bound`
                        // / `x ` (trailing), not the bound name alone -> coarse
                        // (narrowed to the bound name downstream). A non-alias
                        // `{ x }` is shorthand: rename expands to `{ x: new }`.
                        RecordDestructField::Named { name, alias, range } => {
                            let bound = alias.clone().unwrap_or_else(|| name.clone());
                            self.push(
                                bound,
                                RefNs::Value,
                                range.clone(),
                                Some(scope.clone()),
                                "param",
                                false,
                                true,
                            );
                            self.mark_last_shorthand(alias.is_none());
                        }
                        // `...rest` spans the `...` too -> coarse, not shorthand.
                        RecordDestructField::Rest { name, range } => {
                            self.push(
                                name.clone(),
                                RefNs::Value,
                                range.clone(),
                                Some(scope.clone()),
                                "param",
                                false,
                                true,
                            );
                        }
                    }
                }
                // The pattern-level `...rest` binder has no name span of its
                // own here, so it borrows the whole-param range -> coarse.
                if let Some(r) = rest {
                    self.push(
                        r.clone(),
                        RefNs::Value,
                        p.range.clone(),
                        Some(scope),
                        "param",
                        false,
                        true,
                    );
                }
            }
            // Tuple param names all share the one whole-param range -> coarse.
            Some(ParamPattern::Tuple { names, rest }) => {
                for n in names {
                    self.push(
                        n.clone(),
                        RefNs::Value,
                        p.range.clone(),
                        Some(scope.clone()),
                        "param",
                        false,
                        true,
                    );
                }
                if let Some(r) = rest {
                    self.push(
                        r.clone(),
                        RefNs::Value,
                        p.range.clone(),
                        Some(scope),
                        "param",
                        false,
                        true,
                    );
                }
            }
        }
    }

    /// Descend a handler's trigger (`on go`, `on MyEvent()`, `on split.Forward`,
    /// `on !a`, `on a | b`) so the trigger name is a value use: renaming an `in`
    /// exec port or a user `event` must also rename the handlers it triggers. A
    /// builtin event name or a synthesized `_on_expr_N` never matches a user
    /// [`Binding`] and stays unresolved.
    fn walk_trigger(&mut self, t: &Trigger) {
        match t {
            // `on go` or `on MyEvent`: the trigger is the name token (`parse_trigger_atom`
            // gives it a name-only span), so this is a precise use.
            Trigger::Ident { name, range } => {
                self.push_use(name.clone(), range.clone(), false, false);
            }
            // `on split.Forward`: `range` spans `obj.field` as `Expr::FieldAccess` does,
            // so `field_suffix_span` isolates the field token. `obj` has no name-only span
            // here, so its use is coarse; `field` is a member reference, never renamed,
            // so it goes into `field_name_spans`.
            Trigger::Field { obj, field, range } => {
                self.push_use(obj.clone(), range.clone(), false, true);
                self.model
                    .field_name_spans
                    .push(field_suffix_span(field, range));
            }
            Trigger::Not { inner, .. } => self.walk_trigger(inner),
            Trigger::Union { parts, .. } => {
                for p in parts {
                    self.walk_trigger(p);
                }
            }
        }
    }

    /// Shared by a file-scope `on ...` handler and a nested `Stmt::Handler`:
    /// captures are scoped to `handler.body.range` either way.
    fn register_handler(&mut self, h: &Handler) {
        let scope = h.body.range.clone();
        match &h.alias {
            Some((name, range)) => self.push_use(name.clone(), range.clone(), false, false),
            None => self.walk_trigger(&h.trigger),
        }
        for hp in &h.params {
            // `HandlerParam.range` is the capture name token -> precise.
            self.push(
                hp.name.clone(),
                RefNs::Value,
                hp.range.clone(),
                Some(scope.clone()),
                "capture",
                false,
                false,
            );
            // Optional explicit type annotation (`-> (a: int, b: float)`,
            // custom-event data outputs); `None` for events with fixed
            // built-in data types.
            if let Some(t) = &hp.ty {
                self.walk_type_expr(t);
            }
        }
        // Trigger config arguments (`on ChatCommand("greet", Description = expr)`)
        // can hold a `BlockExpr` with locals, so walk them for bindings.
        for cfg in &h.config {
            match cfg {
                HandlerConfigArg::Positional(e) | HandlerConfigArg::Named { value: e, .. } => {
                    self.walk_expr(e);
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
    fn register_await(&mut self, a: &AwaitStmt, scope: Option<&SourceRange>) {
        if let Some(name) = &a.binding {
            self.push(
                name.clone(),
                RefNs::Value,
                a.range.clone(),
                scope.cloned(),
                "await",
                false,
                true,
            );
        }
        if let Some(fields) = &a.destructure {
            // Each destructured local borrows the whole `await` stmt range
            // (no per-name span), so several can share it -> coarse.
            for (_field, local) in fields {
                self.push(
                    local.clone(),
                    RefNs::Value,
                    a.range.clone(),
                    scope.cloned(),
                    "await",
                    false,
                    true,
                );
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
            // `LetBinding::Ident.range` is the name token only -> precise.
            LetBinding::Ident { name, range } => self.push(
                name.clone(),
                RefNs::Value,
                range.clone(),
                scope,
                "let",
                importable,
                false,
            ),
            // Tuple/record shorthand names all share the one container range
            // -> coarse (this is exactly the tie the cursor dispatch breaks).
            LetBinding::Tuple { names, rest, range } => {
                for n in names {
                    self.push(
                        n.clone(),
                        RefNs::Value,
                        range.clone(),
                        scope.clone(),
                        "let",
                        importable,
                        true,
                    );
                }
                if let Some(r) = rest {
                    self.push(
                        r.clone(),
                        RefNs::Value,
                        range.clone(),
                        scope,
                        "let",
                        importable,
                        true,
                    );
                }
            }
            // `RecordDestructField::Named.range` spans `src: bound` / `x `
            // (trailing), not the bound name alone -> coarse (narrowed to the
            // bound name downstream). A non-alias `{ x }` is shorthand: rename
            // expands to `{ x: new }`. `...rest` spans `...` too -> coarse.
            LetBinding::RecordDestruct { fields, .. } => {
                for f in fields {
                    match f {
                        RecordDestructField::Named { name, alias, range } => {
                            let bound = alias.clone().unwrap_or_else(|| name.clone());
                            self.push(
                                bound,
                                RefNs::Value,
                                range.clone(),
                                scope.clone(),
                                "let",
                                importable,
                                true,
                            );
                            self.mark_last_shorthand(alias.is_none());
                        }
                        RecordDestructField::Rest { name, range } => {
                            self.push(
                                name.clone(),
                                RefNs::Value,
                                range.clone(),
                                scope.clone(),
                                "let",
                                importable,
                                true,
                            );
                        }
                    }
                }
            }
        }
    }

    fn walk_block(&mut self, block: &Block) {
        self.walk_stmts(&block.stmts, &block.range);
    }

    /// An anonymous `chip { }` body: its storage is declared in the scope the
    /// chip shares (`scope`), visible from `visible_from` on (the chip's start
    /// for a chip in a body, everywhere for one at the top level), whichever
    /// of its statements reads it.
    fn walk_anon_chip(
        &mut self,
        ac: &AnonChipDecl,
        scope: Option<&SourceRange>,
        visible_from: Option<usize>,
    ) {
        for e in [&ac.label_expr, &ac.color_expr, &ac.material_expr]
            .into_iter()
            .flatten()
        {
            self.walk_expr(e);
        }
        for s in &ac.body.stmts {
            // A chip nested in a top-level one shares the top level too: its
            // storage is named from the nested chip on, past the outer chip.
            if let (None, Stmt::AnonChip(nested)) = (scope, s) {
                self.walk_anon_chip(nested, None, Some(nested.range.start.offset));
                continue;
            }
            let from = self.model.bindings.len();
            self.walk_stmt(s, &ac.body.range);
            if matches!(
                s,
                Stmt::Var(_) | Stmt::Buffer(_) | Stmt::Array(_) | Stmt::Map(_)
            ) && let Some(b) = self.model.bindings.get_mut(from)
            {
                b.home = scope.is_none().then(|| ac.range.clone());
                b.scope = scope.cloned();
                b.visible_from = visible_from;
            }
            // A `let`/`const` of a top-level chip is its declaration too,
            // named from where it is written.
            if let (None, Stmt::Let(l)) = (scope, s) {
                let names = crate::const_eval::bound_names(&l.binding);
                for b in &mut self.model.bindings[from..] {
                    if b.ns == RefNs::Value
                        && b.scope.as_ref() == Some(&ac.body.range)
                        && names.contains(&b.name)
                    {
                        b.home = Some(ac.range.clone());
                        b.scope = None;
                    }
                }
            }
        }
    }

    /// Walk each direct child of `node`; a statement child belongs to
    /// `scope`.
    fn walk_children(&mut self, node: walk::Node<'_>, scope: &SourceRange) {
        walk::children(node, &mut |child, _| match child {
            walk::Node::Expr(e) => self.walk_expr(e),
            walk::Node::Block(b) => self.walk_block(b),
            walk::Node::Stmt(s) => self.walk_stmt(s, scope),
            walk::Node::Decl(d) => self.walk_top_decl(d),
        });
    }

    fn walk_stmts(&mut self, stmts: &[Stmt], scope: &SourceRange) {
        for s in stmts {
            self.walk_stmt(s, scope);
        }
    }

    fn walk_stmt(&mut self, s: &Stmt, scope: &SourceRange) {
        match s {
            Stmt::Var(v) => {
                self.push(
                    v.name.clone(),
                    RefNs::Value,
                    v.range.clone(),
                    Some(scope.clone()),
                    "var",
                    false,
                    true,
                );
                self.visible_after(self.model.bindings.len() - 1, &v.range);
                if let Some(t) = &v.typ {
                    self.walk_type_expr(t);
                }
                if let Some(e) = &v.init {
                    self.walk_expr(e);
                }
                for e in [&v.label_expr, &v.color_expr, &v.material_expr]
                    .into_iter()
                    .flatten()
                {
                    self.walk_expr(e);
                }
            }
            Stmt::Buffer(b) => {
                self.push(
                    b.name.clone(),
                    RefNs::Value,
                    b.range.clone(),
                    Some(scope.clone()),
                    "buffer",
                    false,
                    true,
                );
                if let Some(t) = &b.typ {
                    self.walk_type_expr(t);
                }
                self.walk_expr(&b.init);
            }
            Stmt::Array(a) => {
                self.push(
                    a.name.clone(),
                    RefNs::Value,
                    a.range.clone(),
                    Some(scope.clone()),
                    "array",
                    false,
                    true,
                );
                self.walk_type_expr(&a.element_type);
                for el in &a.init {
                    self.walk_expr(el.expr());
                }
            }
            Stmt::Map(m) => {
                self.push(
                    m.name.clone(),
                    RefNs::Value,
                    m.range.clone(),
                    Some(scope.clone()),
                    "map",
                    false,
                    true,
                );
                self.walk_type_expr(&m.key_type);
                self.walk_type_expr(&m.value_type);
                if let Some(e) = &m.init {
                    self.walk_expr(e);
                }
            }
            Stmt::Let(l) => {
                let from = self.model.bindings.len();
                self.register_let(l, Some(scope.clone()), false);
                self.visible_after(from, &l.range);
                self.walk_expr(&l.value);
            }
            // Task 18: parser-only stub, block-scoped like `Stmt::Let` above.
            Stmt::LetElse(l) => {
                self.walk_expr(&l.scrutinee);
                self.walk_pattern_bindings(&l.pattern, Some(scope.clone()));
                self.walk_block(&l.else_block);
            }
            Stmt::In(d) => {
                self.push(
                    d.name.clone(),
                    RefNs::Value,
                    d.range.clone(),
                    Some(scope.clone()),
                    "in",
                    false,
                    true,
                );
                self.walk_type_expr(&d.typ);
                if let Some(e) = &d.label_expr {
                    self.walk_expr(e);
                }
            }
            Stmt::OutBinding(ob) => {
                self.push(
                    ob.name.clone(),
                    RefNs::Value,
                    ob.range.clone(),
                    Some(scope.clone()),
                    "out",
                    false,
                    true,
                );
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
                self.walk_anon_chip(ac, Some(scope), Some(ac.range.start.offset));
            }
            Stmt::ChipDecl(c) => self.register_chip(c, Some(scope.clone()), false),
            Stmt::IfLet(i) => {
                self.walk_if_let(
                    &i.pattern,
                    &i.scrutinee,
                    &i.then_block,
                    i.else_block.as_ref(),
                );
            }
            Stmt::Emit(e) => {
                // `emit NAME = expr` writes the `out` or `var` named NAME, a value use. `Emit`
                // has no name-only span, so the statement range stands in (coarse); the LSP
                // narrows it to NAME with `find_name_range`.
                self.push_use(e.name.clone(), e.range.clone(), false, true);
                self.walk_children(walk::Node::Stmt(s), scope);
            }
            Stmt::Await(a) => self.register_await(a, Some(scope)),
            Stmt::For(f) => {
                match &f.source {
                    ForSource::Range(r) => {
                        match &r.extent {
                            RangeExtent::Bounds { start, end } => {
                                self.walk_expr(start);
                                self.walk_expr(end);
                            }
                            RangeExtent::Count { count, from } => {
                                self.walk_expr(count);
                                if let Some(from) = from {
                                    self.walk_expr(from);
                                }
                            }
                        }
                        if let Some(step) = &r.step {
                            self.walk_expr(step);
                        }
                    }
                    ForSource::Collection { expr, .. } => self.walk_expr(expr),
                }
                self.register_for(f, Some(scope.clone()));
            }
            Stmt::If(_)
            | Stmt::Assign(_)
            | Stmt::ExprStmt(_)
            | Stmt::Return { .. }
            | Stmt::Break { .. }
            | Stmt::Continue { .. } => self.walk_children(walk::Node::Stmt(s), scope),
        }
    }

    fn walk_if(&mut self, cond: &Expr, then_block: &Block, else_block: Option<&Block>) {
        self.walk_expr(cond);
        self.walk_block(then_block);
        if let Some(eb) = else_block {
            self.walk_block(eb);
        }
    }

    /// Registers a `for` loop's slot bindings scoped to its body, then
    /// registers `let run = for` scoped to the enclosing block AFTER the loop,
    /// never inside it.
    fn register_for(&mut self, f: &ForStmt, outer_scope: Option<SourceRange>) {
        // This pass has no types, so a collection asks with the wider of the
        // two collection tables: the kind only decides the `{ }` reading and
        // the slot count, and both collection kinds destructure `{ }` alike.
        let kind = match f.source {
            ForSource::Range(_) => LoopKind::CountFor,
            ForSource::Collection { .. } => LoopKind::ForEachMap,
        };
        let body_scope = Some(f.body.range.clone());

        for (pat, slot) in f.bindings.iter().zip(for_slots(kind, &f.bindings)) {
            match slot {
                // A `{ }` binding on a range names the loop's own slots (no
                // element to destructure). Names come from `ir::plane`'s table;
                // one not in it binds nothing here, matching typecheck's WS076 refusal.
                ForSlot::RangeSlotNames => {
                    let ForPat::Record { fields, .. } = pat else {
                        continue;
                    };
                    let slot_labels = plane_spec(LoopKind::CountFor).slot_labels;
                    for field in fields {
                        let RecordDestructField::Named { name, alias, range } = field else {
                            // `...rest` has no slots to collect on a range (WS076); nothing to bind.
                            continue;
                        };
                        if !slot_labels.contains(&name.as_str()) {
                            continue;
                        }
                        let bound = alias.clone().unwrap_or_else(|| name.clone());
                        self.push(
                            bound,
                            RefNs::Value,
                            range.clone(),
                            body_scope.clone(),
                            "for",
                            false,
                            true,
                        );
                        self.mark_last_shorthand(alias.is_none());
                    }
                }
                ForSlot::Slot(_) => self.bind_for_pat(pat, body_scope.clone()),
                // Over-bound, so typecheck refused it (WS076); binding the
                // name here would resolve a reference the program cannot have.
                ForSlot::OverBound => {}
            }
        }

        self.walk_block(&f.body);

        if let Some((name, name_range)) = &f.result {
            // Scope containment is a pure range test with no ordering, so
            // scoping `run` to the whole enclosing block would let a body
            // reference resolve to it. Start the scope at the loop's own closing brace instead.
            let after_loop = outer_scope.map(|s| SourceRange {
                file: std::sync::Arc::clone(&s.file),
                start: f.range.end,
                end: s.end,
            });
            self.push(
                name.clone(),
                RefNs::Value,
                name_range.clone(),
                after_loop,
                "for",
                false,
                false,
            );
        }
    }

    /// Binds one `for` slot pattern: a name, `_` (nothing), or a `{ }`
    /// destructure of the slot's value. Never called for a range's own `{ }`
    /// slot names - `register_for` handles those separately.
    fn bind_for_pat(&mut self, pat: &ForPat, scope: Option<SourceRange>) {
        match pat {
            ForPat::Ignore(_) => {}
            // `ForPat::Name`'s range is the name token only -> precise.
            ForPat::Name(name, range) => self.push(
                name.clone(),
                RefNs::Value,
                range.clone(),
                scope,
                "for",
                false,
                false,
            ),
            ForPat::Record { fields, .. } => {
                for field in fields {
                    match field {
                        RecordDestructField::Named { name, alias, range } => {
                            let bound = alias.clone().unwrap_or_else(|| name.clone());
                            self.push(
                                bound,
                                RefNs::Value,
                                range.clone(),
                                scope.clone(),
                                "for",
                                false,
                                true,
                            );
                            self.mark_last_shorthand(alias.is_none());
                        }
                        RecordDestructField::Rest { name, range } => {
                            self.push(
                                name.clone(),
                                RefNs::Value,
                                range.clone(),
                                scope.clone(),
                                "for",
                                false,
                                true,
                            );
                        }
                    }
                }
            }
        }
    }

    /// `if let <pattern> = <scrutinee> { .. } else { .. }` - the pattern's
    /// captures are scoped to the then-block, same as a `match` arm's
    /// captures are scoped to its body.
    fn walk_if_let(
        &mut self,
        pattern: &Pattern,
        scrutinee: &Expr,
        then_block: &Block,
        else_block: Option<&Block>,
    ) {
        self.walk_expr(scrutinee);
        self.walk_pattern_bindings(pattern, Some(then_block.range.clone()));
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
            Expr::Call {
                callee,
                args,
                type_args,
                ..
            } => {
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
            Expr::FieldAccess { obj, field, range } => {
                // The field half of `obj.field` is never a rename target
                // (record fields are a deliberate deferral).
                self.model
                    .field_name_spans
                    .push(field_suffix_span(field, range));
                self.walk_expr(obj);
            }
            Expr::As { value, ty, .. } => {
                self.walk_expr(value);
                self.walk_type_expr(ty);
            }
            Expr::BlockExpr {
                stmts,
                value,
                range,
            } => {
                self.walk_stmts(stmts, range);
                self.walk_expr(value);
            }
            Expr::RecordLit { fields, .. } => {
                for f in fields {
                    match f {
                        // The KEY half of `name: value` is never a rename
                        // target (only the value side is a use).
                        RecordLitField::Named { name, value, range } => {
                            self.model
                                .field_name_spans
                                .push(name_prefix_span(name, range));
                            self.walk_expr(value);
                        }
                        RecordLitField::Spread { value, .. } => self.walk_expr(value),
                        // `{ name }` is shorthand for `{ name: name }`: the bare field name is the
                        // value use.
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
                    // Every `Pattern::Binding` in the arm's pattern is a
                    // captured payload name scoped to this arm's body. There's
                    // no name-token span on the arm itself, so use the arm
                    // body's range as the extent.
                    let mut arm_scope = match &arm.body {
                        MatchBody::Expr(x) => x.range().clone(),
                        MatchBody::Block(b) => b.range.clone(),
                    };
                    // A guard reads the captures too.
                    if let Some(g) = &arm.guard {
                        arm_scope.start = g.range().start;
                    }
                    self.walk_pattern_bindings(&arm.pattern, Some(arm_scope));
                    if let Some(g) = &arm.guard {
                        self.walk_expr(g);
                    }
                    match &arm.body {
                        MatchBody::Expr(x) => self.walk_expr(x),
                        MatchBody::Block(b) => self.walk_block(b),
                    }
                }
            }
            // `path` is the `Enum.Variant` `FieldAccess`, walked generically
            // (its own arm above pushes the field-name span); the field list
            // mirrors `RecordLit`'s handling - the key is never a rename
            // target, only the value side is a use.
            Expr::VariantCtor { path, fields, .. } => {
                self.walk_expr(path);
                for f in fields {
                    match f {
                        RecordLitField::Named { name, value, range } => {
                            self.model
                                .field_name_spans
                                .push(name_prefix_span(name, range));
                            self.walk_expr(value);
                        }
                        RecordLitField::Spread { value, .. } => self.walk_expr(value),
                        RecordLitField::Shorthand { name, range } => {
                            self.push_use(name.clone(), range.clone(), true, false);
                        }
                    }
                }
            }
            // A call's callee is walked like any value position (`Call`'s arm walks
            // `callee`), so `helper(1)` reaches this arm for `helper`.
            Expr::Ident { name, range } => self.push_use(name.clone(), range.clone(), false, false),
            _ => self.walk_children(walk::Node::Expr(e), e.range()),
        }
    }

    /// Registers every `Pattern::Binding` in a match arm's, `if let`'s or
    /// `let else`'s pattern as a captured value binding under `scope` (`None` at
    /// file scope, as for `TopDecl::Let`; the block for a local capture).
    fn walk_pattern_bindings(&mut self, pattern: &Pattern, scope: Option<SourceRange>) {
        match pattern {
            Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range { .. } => {}
            Pattern::Binding { name, range } => {
                self.push(
                    name.clone(),
                    RefNs::Value,
                    range.clone(),
                    scope,
                    "capture",
                    false,
                    false,
                );
            }
            Pattern::Concat { parts: pats, .. } | Pattern::Or { alts: pats, .. } => {
                for p in pats {
                    self.walk_pattern_bindings(p, scope.clone());
                }
            }
            Pattern::Variant { sub, .. } => match sub {
                VariantPattern::Unit => {}
                VariantPattern::Positional(elems) => {
                    for p in elems {
                        self.walk_pattern_bindings(p, scope.clone());
                    }
                }
                VariantPattern::Named { fields, .. } => {
                    for (_, p) in fields {
                        self.walk_pattern_bindings(p, scope.clone());
                    }
                }
            },
        }
    }
}

/// `(line, col)` as a comparison key. The lexer fills both as it advances
/// through a file, so tuple order is source order. Cursor queries carry
/// `line`/`col` and no byte offset, so containment in this module compares these.
fn pos_key(p: &Pos) -> (u32, u32) {
    (p.line, p.col)
}

/// True if lexical `scope` fully encloses `r` (inclusive at both ends).
fn scope_contains(scope: &SourceRange, r: &SourceRange) -> bool {
    pos_key(&scope.start) <= pos_key(&r.start) && pos_key(&r.end) <= pos_key(&scope.end)
}

/// True if the 1-based `(line, col)` cursor lies in `[r.start, r.end)`, half-open
/// like the lexer's token ranges (`end` is one past the last char).
fn range_contains_cursor(r: &SourceRange, line: u32, col: u32) -> bool {
    let p = (line, col);
    pos_key(&r.start) <= p && p < pos_key(&r.end)
}

/// A 0-based LSP cursor as the 1-based BYTE `(line, col)` every `SourceRange`
/// in this module carries. `col` arrives as a char column (the convention all
/// of `analysis` takes) and `Pos::col` counts bytes, so the two agree only
/// while the line is pure ASCII. Without the conversion one accented letter
/// earlier on the line slides the cursor left onto a different identifier,
/// and rename rewrites that one instead.
fn cursor_key(source: &str, line: usize, col: usize) -> (u32, u32) {
    let byte_col = char_col_to_byte(line_text(source, line), col);
    ((line + 1) as u32, (byte_col + 1) as u32)
}

/// A range's length in bytes, used only to compare the sizes of two AST ranges
/// (a cursor has no byte offset, so containment uses `(line, col)`).
fn range_len(r: &SourceRange) -> usize {
    r.end.offset.saturating_sub(r.start.offset)
}

/// Resolve every [`Use`] in `model` to the [`Binding`] that owns it: among
/// same-name bindings in the same namespace (`b.ns == u.ns`), keep those whose
/// `scope` is `None` (file scope) or contains the use, and pick the innermost.
/// A use matching nothing (a builtin type, an undefined global) stays
/// unresolved, so it is not a rename target.
fn resolve_uses(model: &mut ScopeModel) {
    let ScopeModel {
        bindings,
        uses,
        top,
        top_reads,
        ..
    } = model;
    // Each name's bindings in each namespace, in binding order.
    let mut named: HashMap<(RefNs, &str), Vec<usize>> = HashMap::default();
    for (i, b) in bindings.iter().enumerate() {
        named.entry((b.ns, b.name.as_str())).or_default().push(i);
    }
    // The top-level declaration a use is written in. Declarations do not
    // nest and each read starts at most a byte before its declaration, so of
    // the reads starting at or before the use only the last two can hold it;
    // the earlier one is checked first, as a search in order would.
    let in_order = top_reads
        .windows(2)
        .all(|w| w[0].range.start.offset <= w[1].range.start.offset);
    let read_around = |r: &SourceRange| {
        let linear = || top_reads.iter().find(|t| within(&t.range, r));
        if !in_order {
            return linear();
        }
        let i = top_reads.partition_point(|t| t.range.start.offset <= r.start.offset);
        let found = [i.checked_sub(2), i.checked_sub(1)]
            .into_iter()
            .flatten()
            .map(|k| &top_reads[k])
            .find(|t| within(&t.range, r));
        debug_assert_eq!(
            found.map(|t| &t.range),
            linear().map(|t| &t.range),
            "top-level read for {r:?}"
        );
        found
    };
    for u in uses.iter_mut() {
        // A type use naming a builtin (`character`, `int`, `Map`) never resolves to a
        // user binding, even one of the same name (`type character = ...` parses,
        // since these are identifiers, not keywords): renaming that alias must not
        // rename the builtin's uses.
        if u.ns == RefNs::Type && builtin_type_names().contains(u.name.as_str()) {
            u.resolved = None;
            continue;
        }
        // `current` tracks the winning binding id plus its scope's byte span
        // (`None` = file scope, which loses to any local match).
        let mut current: Option<(usize, Option<usize>)> = None;
        // Between two file-scope bindings, the one the compiler's rule names
        // (`scope::top`): the declaration of the chip around the use, else
        // the top level's, else a chip's.
        let home = top.resolve_name(&u.name, &u.range);
        let home_rank = |b: &Binding| match (&b.home, home) {
            (Some(h), Home::Chip(c)) if *h == c.range => 0,
            (None, Home::Top) => 0,
            (None, _) => 1,
            (Some(_), _) => 2,
        };
        // The top-level declaration the use is written in, as the top level is
        // read from it: a `mod` or `chip` as a body, which names a top-level
        // `let` declared below it, anything else as code.
        let read = read_around(&u.range)
            .cloned()
            .unwrap_or_else(|| ReadAt::code(&u.range));
        let same_name = named
            .get(&(u.ns, u.name.as_str()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        for b in same_name.iter().map(|&i| &bindings[i]) {
            let hoisted = read.body && b.scope.is_none() && b.home.is_none();
            if !hoisted
                && b.visible_from.is_some_and(|at| {
                    b.name_range.file == u.range.file && u.range.start.offset < at
                })
            {
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
                (Some((cur, None)), None) => home_rank(b) < home_rank(&bindings[*cur]),
            };
            if better {
                current = Some((b.id, scope_len));
            }
        }
        // A name the top level of the file declares more than once names the
        // declaration visible where the compiler reads it.
        if let Some((id, None)) = current
            && bindings[id].home.is_none()
            && u.ns == RefNs::Value
            && let Some(Some(seen)) = top.visible_decl(&u.name, &read)
            && let Some(b) = same_name.iter().map(|&i| &bindings[i]).find(|b| {
                b.scope.is_none() && b.home.is_none() && within(&seen.range, &b.name_range)
            })
        {
            current = Some((b.id, None));
        }
        u.resolved = current.map(|(id, _)| id);
    }
}

/// Resolve the binding under the cursor and return every same-file site bound to
/// it, the declaration included. `line`/`col` are 0-based with `col` a char
/// column; `source` is the file's text, for the byte-column conversion and the
/// tie-break below. `_file` is unused: `script` is one file's AST.
///
/// Among every [`Use`] and [`Binding::name_range`] containing the cursor, the
/// smallest wins, so a precise use beats a coarse declaration range. When
/// several share that range (a tuple or record `let`, a tuple parameter, an
/// `await` destructure), the identifier under the cursor (`word_at`) decides. An
/// unresolved use gives `None`, which [`prepare_rename_at`] refuses.
pub fn references_at(
    script: &Script,
    source: &str,
    _file: &str,
    line: usize,
    col: usize,
) -> Option<(RefTarget, Vec<RefSite>)> {
    let model = build_scope_model(script);
    let (cursor_line, cursor_col) = cursor_key(source, line, col);

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
        let word = word_at(source, line, col);
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
            // An aliased import (`import { orig as local }`): the alias is file-local, so
            // renaming it edits only this file's specifier token and uses, never the
            // defining file (which spells `orig`) or another importer. As a cross-file
            // target, narrowing `mod orig(){...}` against the alias would replace the
            // whole declaration.
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

/// The renameable name span under the cursor and the current name (the rename
/// placeholder), or `None` for a position that is not renameable: an unresolved
/// use (a builtin call or event name), a lexer keyword, or a record field name
/// (`Expr::FieldAccess.field`, `RecordLitField::Named.name`, a record type
/// field).
///
/// The keyword and field refusals run before [`references_at`]'s dispatch: a
/// coarse declaration range (`var`, `buffer`, `array`, `map`, `in`, `out`,
/// `chip`, `mod`, `type`) runs from its keyword through its body, so a cursor on
/// `var`, or on `field` in `var x: int = p.field`, would otherwise resolve to `x`.
pub fn prepare_rename_at(
    script: &Script,
    source: &str,
    file: &str,
    line: usize,
    col: usize,
) -> Option<(SourceRange, String)> {
    if let Some(word) = word_at(source, line, col)
        && crate::lexer::KEYWORDS.contains(&word.as_str())
    {
        return None;
    }

    let (cursor_line, cursor_col) = cursor_key(source, line, col);
    let model = build_scope_model(script);
    if model
        .field_name_spans
        .iter()
        .any(|r| range_contains_cursor(r, cursor_line, cursor_col))
    {
        return None;
    }

    let (target, sites) = references_at(script, source, file, line, col)?;

    // The rename span is the target's site (declaration or use) containing the
    // cursor, the smallest one winning as in `references_at`, so a use inside its
    // own coarse declaration (`var x: int = x + 1`) resolves to itself.
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

/// True if the 0-based `(line, char col)` cursor is on a record field name (a
/// `FieldAccess.field`, a `RecordLitField::Named` key, a record type field), the
/// positions [`prepare_rename_at`] refuses. The LSP routes such a click to its
/// type-directed goto-definition, since an enclosing coarse `type X = {...}`
/// range would otherwise surface the whole type's references.
pub fn field_name_at(script: &Script, source: &str, line: usize, col: usize) -> bool {
    let (cursor_line, cursor_col) = cursor_key(source, line, col);
    let model = build_scope_model(script);
    model
        .field_name_spans
        .iter()
        .any(|r| range_contains_cursor(r, cursor_line, cursor_col))
}

/// A token class for [`semantic_tokens`], which `crates/lsp` maps onto its
/// `SemanticTokensLegend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemTokenKind {
    Type,
    Function,
    Parameter,
    Variable,
    Namespace,
}

/// One semantic-token override. The `TextMate` grammar
/// (`editors/vscode/syntaxes/wirescript.tmLanguage.json`) colors every builtin
/// type keyword and every capitalized identifier as `support.type` regardless
/// of position, so a capitalized value (a `character` capture, a capitalized
/// `var`) shows as a type. [`semantic_tokens`] emits a span only where it
/// disagrees with the grammar, and VS Code layers these over it.
#[derive(Clone, Debug)]
pub struct SemSpan {
    pub range: SourceRange,
    pub name: String,
    pub kind: SemTokenKind,
    /// As [`Use::coarse`] and [`Binding::coarse`]: the LSP narrows a coarse span to
    /// its name token with [`super::references::find_name_range`] first and skips
    /// it when that fails.
    pub coarse: bool,
}

/// Record one [`SemSpan`], deduplicated by `(start line, start col)`: several
/// sites (a use and its declaration, coarse bindings sharing a container) can
/// share a span, and the LSP wants one token per position. The first wins.
fn record_sem_span(
    spans: &mut Vec<SemSpan>,
    seen: &mut HashSet<(u32, u32)>,
    range: SourceRange,
    name: String,
    kind: SemTokenKind,
    coarse: bool,
) {
    if seen.insert((range.start.line, range.start.col)) {
        spans.push(SemSpan {
            range,
            name,
            kind,
            coarse,
        });
    }
}

/// A compiler-synthesized identifier standing in for user source. The handler
/// trigger `on <call>()` desugars to `let _on_expr_N = <call>` and
/// `on _on_expr_N`, both spanning the whole `<call>`, so semantic tokens skip
/// them and the call keeps its own coloring.
fn is_synthetic_ident(name: &str) -> bool {
    name.starts_with("_on_expr_")
}

/// Semantic-token overrides for the grammar's position-blind type coloring (see
/// [`SemSpan`]): every use and declaration the scope resolver classifies
/// differently from the grammar's guess.
///
/// - A type-namespace [`Use`] is a `Type` token, which gives a user `type`
///   alias the type color.
/// - A value [`Use`] the grammar would color as a type ([`grammar_would_type`])
///   gets its binding's category ([`kind_token`]) when it resolves to a user
///   [`Binding`]; an unresolved one (a builtin call) keeps the grammar's color.
/// - A [`Binding`] gets the same treatment. Import bindings are skipped: they
///   are registered in both namespaces, so their kind is unknown here.
///
/// The result is unsorted with no duplicate positions (see [`record_sem_span`]);
/// the LSP sorts it before delta-encoding.
pub fn semantic_tokens(script: &Script) -> Vec<SemSpan> {
    let model = build_scope_model(script);
    let mut spans = Vec::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::default();

    // Type-position names become Type tokens (user `type` aliases included); every
    // resolved user value becomes a Variable token, so a value spelled like a type
    // keyword (`character`), a builtin (`round`) or any capitalized word reads as
    // an identifier. An unresolved value use is a real builtin (`round(...)`,
    // `DisplayText`) and keeps the grammar's function color.
    for u in &model.uses {
        if is_synthetic_ident(&u.name) {
            continue;
        }
        match u.ns {
            RefNs::Type => record_sem_span(
                &mut spans,
                &mut seen,
                u.range.clone(),
                u.name.clone(),
                SemTokenKind::Type,
                u.coarse,
            ),
            RefNs::Value => {
                if u.resolved.is_some() {
                    record_sem_span(
                        &mut spans,
                        &mut seen,
                        u.range.clone(),
                        u.name.clone(),
                        SemTokenKind::Variable,
                        u.coarse,
                    );
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
        record_sem_span(
            &mut spans,
            &mut seen,
            b.name_range.clone(),
            b.name.clone(),
            kind,
            b.coarse,
        );
    }

    spans
}

/// Every site in `script` that renaming the export `export_name` (namespace
/// `ns`, defined in another file) must edit, whether this file imports it
/// plainly or under an alias:
///
/// - a plain specifier (`import { orig }`): `ImportBinding.range` is the name
///   token, a precise site;
/// - an aliased one (`import { orig as x }`): `ImportBinding.range` covers the
///   alias `x`, so the whole `ImportDecl.range` is a coarse site, which the LSP
///   narrows to `orig` with `find_name_range`, leaving `x` alone;
/// - the uses, only for a plain import: an aliased import's uses spell the
///   alias, which an export rename keeps.
///
/// A local binding shadowing the import resolves to itself (innermost scope
/// wins), so it never appears among the import's uses. Empty when `script` has
/// no specifier for `export_name`.
pub fn references_to_export(script: &Script, export_name: &str, ns: RefNs) -> Vec<RefSite> {
    let model = build_scope_model(script);
    let mut sites = Vec::new();

    for d in &script.decls {
        let TopDecl::Import(imp) = d else { continue };
        let ImportKind::Named(bindings) = &imp.kind else {
            continue;
        };
        for b in bindings {
            if b.name != export_name {
                continue;
            }
            // The specifier: a NON-aliased binding's own `range` IS the
            // precise name token (no narrowing needed); an ALIASED binding's
            // `range` covers the ALIAS instead, so fall back to the coarse
            // whole-`import ...` span narrowed to `orig` downstream.
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
            // The imported name's uses, only for a plain import: an aliased import's uses
            // spell the alias, which an export rename leaves alone.
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
