//! The typechecker: a two-pass walk over the AST (decl registration, then
//! per-decl checking), with the scope stack and exec/pure context tracking
//! fused inline.
//!
//! Walks the AST producing a side `typeOfExpr` map (keyed by each
//! expression's source-range start offset) so we don't need to rebuild
//! the AST as a typed parallel. `opResolutions` records the catalog
//! `OpRule` chosen for every BinOp/UnOp; the lower phase consumes it.
//!
//! Identifier semantics for `var`:
//! - In exec context: `n` auto-derefs (lowered to `Exec_Var_Get`); type = inner T.
//! - In a pure sink expecting `ref T`: `n` is the VarRef port; type = ref T.
//! - In a pure sink expecting `T`: error (WS006); author writes `n.Value`.
//! - `*n`: explicit deref; requires exec context.
//! - `n.Value`: delayed-read form; always yields T.

use crate::collections::HashMap;
use std::sync::Arc;

use crate::scope::Scope as ScopeStack;

use crate::ast::*;
use crate::catalog::calls::find_call;
use crate::catalog::events::{events, find_event};
use crate::catalog::operators::{OpRule, resolve_op};
use crate::diagnostic::{Diagnostic, Severity, SourceRange};
use crate::ir::Type;
use crate::types::classes::mask_contains;
use crate::types::coerce::{CoerceRule, coerce, widening_join_all};

pub(crate) mod infer;
mod sig;
pub use sig::{CallSignature, Param, ParamKind, check_args};

// ---------- scope + symbol info ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Var,
    Buffer,
    Array,
    Map,
    Param,
    EventParam,
    LetBinding,
    Fn,
    Chip,
    Event,
    ChipInstance,
    In,
    Out,
    Namespace,
    Type,
}

#[derive(Clone, Debug)]
pub struct EventDataField {
    pub name: String,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct FnOrChipSig {
    pub params: Vec<EventDataField>,
    pub outputs: Vec<EventDataField>,
    /// Generic type params declared on a `mod`/`chip<T, ...>` decl, each
    /// paired with its resolved mask (the concrete types it may bind to).
    /// Empty for non-generic decls and for `fn` (no generics parse there) —
    /// the call-site inference path is gated on this being non-empty so
    /// non-generic calls are completely unaffected.
    pub type_params: Vec<(String, Vec<Type>)>,
}

#[derive(Clone, Debug)]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub name: String,
    pub ty: Type,
    pub decl_range: SourceRange,
    pub signature: Option<FnOrChipSig>,
    pub event_data: Option<Vec<EventDataField>>,
}

/// Thin wrapper around the shared `Scope<V>` stack, preserving the
/// typecheck-specific API (`declare`, `lookup`, `set_type`).
pub struct Scope {
    inner: ScopeStack<SymbolInfo>,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    pub fn new() -> Self {
        Self {
            inner: ScopeStack::new(),
        }
    }
    pub fn push(&mut self) {
        self.inner.push(crate::scope::ScopeTag::BLOCK);
    }
    pub fn pop(&mut self) {
        self.inner.pop();
    }
    /// Declare in the top-most frame. Returns the prior info if any.
    pub fn declare(&mut self, name: &str, info: SymbolInfo) -> Option<SymbolInfo> {
        self.inner.insert(name, info)
    }
    /// Mutate an already-declared symbol's type (used to refine buffer
    /// types after their RHS infers).
    pub fn set_type(&mut self, name: &str, ty: Type) {
        if let Some(info) = self.inner.get_mut(name) {
            info.ty = ty;
        }
    }
    pub fn lookup(&self, name: &str) -> Option<&SymbolInfo> {
        self.inner.get(name)
    }
    /// Snapshot of every `SymbolKind::Type` alias currently visible
    /// (innermost frame wins on a name collision, matching `lookup`'s
    /// shadowing), for `resolve_type_expr`'s delegation to the shared
    /// `types::resolve::resolve_type`.
    pub fn type_aliases(&self) -> HashMap<String, Type> {
        let mut out = HashMap::default();
        for (name, info) in self.inner.iter() {
            if info.kind == SymbolKind::Type {
                out.entry(name.to_string()).or_insert_with(|| info.ty.clone());
            }
        }
        out
    }
}

// ---------- exec/pure context ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecMode {
    Exec,
    Pure,
}

// ---------- typecheck context ----------

/// Pre-indexed info about a namespace member for O(1) lookup.
#[derive(Clone, Debug)]
pub struct NsDeclInfo {
    pub kind: SymbolKind,
    pub return_type: Option<TypeExpr>,
    /// The member's declared params, carried so a namespaced call
    /// (`ns.f(args)`) can be arity/type-checked via `check_args` — before
    /// this field existed those calls did NO argument checking at all. A
    /// generic member's params may still carry `Type::Param` (there's no
    /// call-site `subst` here to resolve them against); `check_args`'s own
    /// `type_has_param` guard skips those, same as it does elsewhere.
    pub params: Vec<EventDataField>,
}

pub struct TypeCheckCtx<'a> {
    pub diagnostics: Vec<Diagnostic>,
    pub scope: Scope,
    exec_stack: Vec<ExecMode>,
    pub file: String,
    pub namespaces: HashMap<String, HashMap<String, NsDeclInfo>>,
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
    /// Typed every visited expression; key is (file, start_offset, end_offset).
    pub type_of_expr: HashMap<(Arc<str>, usize, usize), Type>,
    /// Operator rule chosen for every BinOp/UnOp; same key scheme.
    pub op_resolutions: HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Ferried payload type per local exec signal, recorded from
    /// `emit sig = <value>` so `let { a, b } = await sig` can type its fields.
    pub signal_payload_types: HashMap<String, Type>,
    /// Generic type aliases (`type Pair<T> = { a: T, b: T }`) in scope, keyed
    /// by name (namespaced ones by their qualified `Ns.Name`). Populated by a
    /// pre-pass over `script.decls` — BEFORE the two-pass decl
    /// registration/checking below — so a generic alias resolves regardless
    /// of where it's declared relative to its uses (including
    /// self-reference, guarded separately by the resolver's depth cap).
    /// Threaded into every `resolve_type_expr` call via `ResolveCtx`. Uses the
    /// crate's Fx `HashMap` (like every other compiler table here), matching
    /// `ResolveCtx::generic_aliases` and `Scope::type_aliases()`.
    pub generic_type_aliases: HashMap<String, crate::types::resolve::GenericAlias>,
    /// Top-level `let` constants, so a `var` initializer may name one
    /// (`1 << C_FLAG`) rather than restating its value. Populated before decl
    /// checking; must stay in step with lowering's own environment so both
    /// agree on exactly which initializers are constant.
    pub const_env: crate::lower::ConstEnv,
    /// Product of the per-mask-member combo counts on the CURRENT nesting path
    /// of generic decls (1 at the top level). A nested generic `mod`/`chip`
    /// re-runs its whole body per outer combo, so without this the per-decl
    /// combo cap multiplies with depth (`13^d`). The Chip body-check divides
    /// its remaining budget by this so total work stays bounded regardless of
    /// nesting depth; a nested decl whose masks exceed the remaining budget
    /// falls back to a single first-member check (like the too-many-params cap).
    pub active_combos: usize,
    /// Scoped constant `let` bindings, one frame per currently-open
    /// `ctx.scope` frame (a FRAME STACK mirroring `scope` 1:1 — every
    /// `push_scope`/`pop_scope` pair pushes/pops both together). A body-local
    /// `let name = <constant>` (top-level OR inside a handler/mod/if/block)
    /// records `name -> Literal` in the top frame here, so a constant-only
    /// config arg (see `const_lookup`) can resolve it the same way a
    /// top-level `let` resolves through `const_env`.
    pub scoped_consts: Vec<crate::collections::HashMap<String, crate::ir::Literal>>,
    /// Inferred custom-event receiver slot types (Task 2's `infer_custom_event_slots`
    /// output), keyed by handler source range. Empty on pass 1 (nothing inferred
    /// yet); populated on pass 2 by `typecheck_with_inference`. Consulted by
    /// `bind_handler_trigger_params` for unannotated custom-event params and by
    /// `check_custom_event_types`/`ce_receiver_of` for pass-2 WS030 checks.
    pub ce_slots: &'a CeSlotMap,
}

impl<'a> TypeCheckCtx<'a> {
    pub fn new(file: &str, ce_slots: &'a CeSlotMap) -> Self {
        Self {
            diagnostics: Vec::new(),
            scope: Scope::new(),
            exec_stack: vec![ExecMode::Pure],
            file: file.to_string(),
            namespaces: HashMap::default(),
            if_contexts: HashMap::default(),
            var_read_contexts: HashMap::default(),
            type_of_expr: HashMap::default(),
            op_resolutions: HashMap::default(),
            signal_payload_types: HashMap::default(),
            generic_type_aliases: HashMap::default(),
            const_env: crate::lower::ConstEnv::default(),
            active_combos: 1,
            scoped_consts: Vec::new(),
            ce_slots,
        }
    }
    /// Push a new `scope` frame together with a matching empty `scoped_consts`
    /// frame. Use this (paired with `pop_scope`) everywhere a bare
    /// `ctx.scope.push()`/`ctx.scope.pop()` pair would otherwise appear, so
    /// the two stacks can never drift out of lockstep.
    pub fn push_scope(&mut self) {
        self.scope.push();
        self.scoped_consts.push(crate::collections::HashMap::default());
    }
    /// Pop the frame pushed by `push_scope`.
    pub fn pop_scope(&mut self) {
        self.scope.pop();
        self.scoped_consts.pop();
    }
    /// The constant environment visible at the current point: the top-level
    /// `const_env` overlaid by every currently-open `scoped_consts` frame,
    /// applied outer-to-inner so an inner scope's `let` shadows an outer
    /// scope's (and both shadow a same-named top-level constant). `const_env`
    /// is small, so cloning per lookup is cheap.
    pub fn const_lookup(&self) -> crate::lower::ConstEnv {
        let mut env = self.const_env.clone();
        for frame in &self.scoped_consts {
            for (name, lit) in frame {
                env.insert(name.clone(), lit.clone());
            }
        }
        env
    }
    pub fn exec_mode(&self) -> ExecMode {
        *self.exec_stack.last().unwrap_or(&ExecMode::Pure)
    }
    pub fn in_exec<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.exec_stack.push(ExecMode::Exec);
        let r = f(self);
        self.exec_stack.pop();
        r
    }
    pub fn in_pure<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.exec_stack.push(ExecMode::Pure);
        let r = f(self);
        self.exec_stack.pop();
        r
    }
    pub fn emit(&mut self, code: &str, message: impl Into<String>, range: SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
            range,
        });
    }
}

// ---------- result ----------

pub struct TypeCheckResult {
    /// Typed every visited expression; key is (file, start_offset, end_offset).
    pub type_of_expr: HashMap<(Arc<str>, usize, usize), Type>,
    /// Operator rule chosen for every BinOp/UnOp; same key scheme.
    pub op_resolutions: HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Exec/pure context for each `if` node; key is (file, start_offset). true = exec.
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    /// Exec/pure context for each var identifier read; key is (file, start_offset). true = exec.
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Which custom-event namespace a receiver belongs to. Personal (`CustomEvent` /
/// `SendCustomEvent`, same-owner delivery) and Global (`GlobalCustomEvent` /
/// `SendGlobalCustomEvent`, ownership-agnostic) are DISTINCT channels: a send in
/// one namespace never resolves a receiver in the other. It is carried in the
/// key so the namespace is explicit in the map, not inferred from context.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CeNamespace {
    Personal,
    Global,
}

impl CeNamespace {
    /// The namespace for an event surface name, or `None` if it is not a custom
    /// event (`"CustomEvent"` → Personal, `"GlobalCustomEvent"` → Global).
    pub fn from_event_name(name: &str) -> Option<Self> {
        match name {
            "CustomEvent" => Some(CeNamespace::Personal),
            "GlobalCustomEvent" => Some(CeNamespace::Global),
            _ => None,
        }
    }
}

/// Key for a custom-event receiver's data slot: `(namespace, file, start.offset,
/// end.offset)` — the namespace plus the handler's source range. The range shape
/// mirrors `tmap`/`type_of_expr` because `SourceRange`/`Pos` do not derive `Hash`;
/// the namespace keeps personal and global receivers in disjoint key spaces.
pub type CeSlotKey = (CeNamespace, Arc<str>, usize, usize);
/// Resolved types for custom-event receivers' data slots, keyed by `CeSlotKey`.
/// `None` slot = declared (nothing to override); `Some(t)` = unannotated, resolve
/// binding to `t`; `Some(Float)` = inference fallback.
pub type CeSlotMap = HashMap<CeSlotKey, Vec<Option<Type>>>;

/// Build the `CeSlotKey` for a custom-event receiver handler `h` in namespace `ns`.
/// Public so lowering builds the identical key (namespace + range) when it reads
/// resolved slot types back out of the `CeSlotMap`.
pub fn ce_slot_key(ns: CeNamespace, h: &Handler) -> CeSlotKey {
    (ns, h.range.file.clone(), h.range.start.offset, h.range.end.offset)
}

pub fn typecheck(script: &Script, file: &str, ce_slots: &CeSlotMap) -> TypeCheckResult {
    let mut ctx = TypeCheckCtx::new(file, ce_slots);
    register_builtin_events(&mut ctx);

    // Pre-pass: collect every generic type alias (`type Pair<T> = …`) before
    // any decl registration/resolution runs, so a use resolves regardless of
    // whether its alias is declared earlier or later in the file.
    collect_generic_aliases(&mut ctx, &script.decls);

    // Two-pass: register all top-level decls first so forward refs resolve.
    for d in &script.decls {
        register_decl(&mut ctx, d);
    }
    // Constant `let`s, resolved before any decl is checked so a `var` / `array`
    // initializer may name one. Built from the same function lowering uses, so
    // the two can't disagree about what counts as a compile-time constant.
    ctx.const_env = crate::lower::build_const_env(&script.decls);
    // `@label(expr)` on a port/chip/nested-var must fold to a compile-time
    // constant (the folded text is baked as the label) — a runtime value there
    // has nowhere to host a wire, so it stays a WS040 error. A TOP-LEVEL `var`
    // is the exception: it carries a wireable text component, so a runtime label
    // there is a valid dynamic label (checked separately in
    // `check_dynamic_var_labels`, after all top-level symbols are declared). A
    // single dedicated walk, since these decls are checked from several
    // different call paths below (top-level, nested in chip/anon-chip bodies,
    // statement-level).
    check_label_exprs(&mut ctx, &script.decls);
    let mut saw_handler = false;
    // Named chip/mod bodies are checked AFTER everything else: top-level
    // `let` types are only inferred (and thus declared) during this pass, so
    // an eagerly-checked body could not see lets declared later — which is
    // exactly where imported mods land relative to the constants their
    // bodies reference. Signatures were already registered in pass 1, so
    // nothing else depends on a body being checked early.
    let mut deferred_chips: Vec<(bool, &TopDecl)> = Vec::new();
    for d in &script.decls {
        // Statements after `on` handlers run in the combined exec context
        // of all preceding handler exits (exec union).
        let is_handler = matches!(d, TopDecl::Handler(_))
            || matches!(d, TopDecl::AnonChip(ac) if ac.body.stmts.iter().any(|s| matches!(s, Stmt::Handler(_))));
        let exec_wrap = saw_handler && !is_handler;
        if is_handler {
            saw_handler = true;
        }
        if matches!(d, TopDecl::Chip(_)) {
            deferred_chips.push((exec_wrap, d));
            continue;
        }
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d);
        }
    }
    for (exec_wrap, d) in deferred_chips {
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d);
        }
    }

    // Runtime `@label(expr)` on a top-level `var`: type-check the expression
    // now that every top-level symbol is declared (an undefined ref or a bad
    // type surfaces here). A constant label is skipped — it bakes statically.
    check_dynamic_var_labels(&mut ctx, script);

    // Whole-program pass: `SendCustomEvent("name", …)` data whose wire types
    // disagree with the `on CustomEvent("name") -> (…)` receiver's declared
    // params.
    // Runs last so every arg has an inferred type in `type_of_expr`.
    // The custom-event pass reads already-inferred arg types while emitting
    // diagnostics; move the map out to satisfy the borrow checker, then restore.
    let type_of_expr = std::mem::take(&mut ctx.type_of_expr);
    // `ce_slots` is `Copy` (a shared reference), so this doesn't hold `ctx`
    // borrowed across the call below. On pass 1, `typecheck`'s `ce_slots` arg
    // is empty, so unannotated receiver slots stay unresolved; pass 2 (driven
    // by `typecheck_with_inference`) passes the real inferred map, so this
    // WS030 check sees inferred receiver types too.
    let ce_slots = ctx.ce_slots;
    check_custom_event_types(&mut ctx, script, &type_of_expr, ce_slots);
    ctx.type_of_expr = type_of_expr;

    TypeCheckResult {
        type_of_expr: ctx.type_of_expr,
        op_resolutions: ctx.op_resolutions,
        if_contexts: ctx.if_contexts,
        var_read_contexts: ctx.var_read_contexts,
        diagnostics: ctx.diagnostics,
    }
}

/// Two-pass typecheck: pass 1 typechecks with no custom-event slot inference
/// (matching plain `typecheck`'s historical behavior), then infers unannotated
/// custom-event receiver slot types from in-unit senders
/// (`infer_custom_event_slots`, Task 2) using pass 1's `type_of_expr`. If any
/// slot was inferred, a pass 2 re-typechecks the whole script with the
/// inferred map wired in, so bodies see the inferred types (not `any`) and
/// WS030 compares against them too. Pass 2's diagnostics include the
/// inference pass's own (WS042 for uninferable slots).
///
/// Returns the map alongside the result — Task 4 threads it into `lower` so
/// emit can pick the right wire-port variant for each custom-event slot.
pub fn typecheck_with_inference(script: &Script, file: &str) -> (TypeCheckResult, CeSlotMap) {
    let empty = CeSlotMap::default();
    let pass1 = typecheck(script, file, &empty);
    let (map, infer_diags) = infer_custom_event_slots(script, &pass1.type_of_expr);
    if map.is_empty() {
        return (pass1, map); // no unannotated custom-event slots → single pass
    }
    let mut pass2 = typecheck(script, file, &map);
    pass2.diagnostics.extend(infer_diags);
    (pass2, map)
}

// ---------- `@label(expr)` constant-folding check (WS040) ----------

/// Walk every top-level decl, recursing into chip/anon-chip bodies (and the
/// blocks nested inside them — `if`/`on`), and flag any `@label(expr)` whose
/// expression doesn't fold to a compile-time constant.
fn check_label_exprs(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    for d in decls {
        check_label_expr_decl(ctx, d);
    }
}

fn check_label_expr_decl(ctx: &mut TypeCheckCtx, d: &TopDecl) {
    match d {
        // A TOP-LEVEL `var` accepts a runtime `@label(expr)` — it becomes a
        // dynamic label wired into the text component's `Text` port at emit
        // (`lower::resolve_dynamic_var_labels`). So no WS040 here; the
        // expression is instead type-checked in `check_dynamic_var_labels`
        // after every top-level symbol is in scope. (A CONSTANT label still
        // bakes statically — that path folds it and never reaches the wire.)
        TopDecl::Var(_) => {}
        TopDecl::In(i) => check_one_label_expr(ctx, &i.label_expr),
        TopDecl::Out(o) => check_one_label_expr(ctx, &o.label_expr),
        TopDecl::Chip(c) => {
            check_one_label_expr(ctx, &c.label_expr);
            check_label_exprs_in_block(ctx, &c.body);
        }
        TopDecl::AnonChip(ac) => {
            check_one_label_expr(ctx, &ac.label_expr);
            check_label_exprs_in_block(ctx, &ac.body);
        }
        // A decl can also live inside a top-level `on Event { ... }` handler
        // (the standard Wirescript pattern) or a top-level `if` — recurse into
        // those blocks too, or a non-constant `@label` there would silently
        // fall back to the name instead of erroring.
        TopDecl::Handler(h) => check_label_exprs_in_block(ctx, &h.body),
        TopDecl::If(i) => {
            check_label_exprs_in_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_label_exprs_in_block(ctx, else_b);
            }
        }
        _ => {}
    }
}

/// Visit every statement in `block`, recursing into nested chip/handler/if
/// bodies. Shared by the decl-level and statement-level walks so the two
/// stay in step.
fn check_label_exprs_in_block(ctx: &mut TypeCheckCtx, block: &Block) {
    for s in &block.stmts {
        check_label_expr_stmt(ctx, s);
    }
}

fn check_label_expr_stmt(ctx: &mut TypeCheckCtx, s: &Stmt) {
    match s {
        Stmt::Var(v) => check_one_label_expr(ctx, &v.label_expr),
        Stmt::In(i) => check_one_label_expr(ctx, &i.label_expr),
        Stmt::OutBinding(o) => check_one_label_expr(ctx, &o.label_expr),
        Stmt::ChipDecl(c) => {
            check_one_label_expr(ctx, &c.label_expr);
            check_label_exprs_in_block(ctx, &c.body);
        }
        Stmt::AnonChip(ac) => {
            check_one_label_expr(ctx, &ac.label_expr);
            check_label_exprs_in_block(ctx, &ac.body);
        }
        Stmt::If(i) => {
            check_label_exprs_in_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_label_exprs_in_block(ctx, else_b);
            }
        }
        Stmt::Handler(h) => check_label_exprs_in_block(ctx, &h.body),
        _ => {}
    }
}

fn check_one_label_expr(ctx: &mut TypeCheckCtx, label_expr: &Option<Expr>) {
    let Some(expr) = label_expr else { return };
    if crate::lower::expr_to_literal_in(expr, &ctx.const_env).is_some() {
        return;
    }
    ctx.emit(
        "WS040",
        "`@label` expression must be a compile-time constant (a literal or a constant `let`); \
         a runtime value cannot be baked as a label",
        expr.range().clone(),
    );
}

/// Type-check the runtime `@label(expr)` on each top-level `var` so an undefined
/// symbol or a bad type surfaces (the lowering pass then wires the value into
/// the label component's `Text` port). Only the non-constant labels reach here —
/// a constant one bakes its text statically and needs no wire. Runs after the
/// main decl loop so every top-level symbol is already declared.
fn check_dynamic_var_labels(
    ctx: &mut TypeCheckCtx,
    script: &Script,
) {
    for d in &script.decls {
        if let TopDecl::Var(v) = d {
            if let Some(le) = &v.label_expr {
                if crate::lower::expr_to_literal_in(le, &ctx.const_env).is_none() {
                    infer::infer(ctx, le);
                }
            }
        }
    }
}

/// Populate `ctx.generic_type_aliases` from every `type Name<T, …> = …`
/// declaration in `decls` (top-level and namespaced). Deliberately does NOT
/// resolve the alias body here — it's still parametric (references its own
/// free `type_params`, unbound until a use site supplies concrete args), so
/// resolving it now would spuriously flag those params as unknown types (the
/// pre-generics behavior the brief calls out). Instantiation happens lazily,
/// per use, in `types::resolve::resolve_type`.
fn collect_generic_aliases(ctx: &mut TypeCheckCtx, decls: &[TopDecl]) {
    for d in decls {
        match d {
            TopDecl::TypeAlias(t) if !t.type_params.is_empty() => {
                ctx.generic_type_aliases.insert(
                    t.name.clone(),
                    crate::types::resolve::GenericAlias {
                        params: t.type_params.iter().map(|tp| tp.name.clone()).collect(),
                        body: t.typ.clone(),
                    },
                );
            }
            TopDecl::Namespace(ns) => {
                for nd in &ns.decls {
                    if let TopDecl::TypeAlias(t) = nd
                        && !t.type_params.is_empty()
                    {
                        let qualified = format!("{}.{}", ns.name, t.name);
                        ctx.generic_type_aliases.insert(
                            qualified,
                            crate::types::resolve::GenericAlias {
                                params: t.type_params.iter().map(|tp| tp.name.clone()).collect(),
                                body: t.typ.clone(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn register_builtin_events(ctx: &mut TypeCheckCtx) {
    let evts = events();
    let mut keys: Vec<&&str> = evts.keys().collect();
    keys.sort();
    for k in keys {
        let spec = &evts[*k];
        ctx.scope.declare(
            spec.surface_name,
            SymbolInfo {
                kind: SymbolKind::Event,
                name: spec.surface_name.to_string(),
                ty: Type::Exec,
                decl_range: SourceRange::default(),
                signature: None,
                event_data: Some(
                    spec.data
                        .iter()
                        .map(|d| EventDataField {
                            name: d.name.to_string(),
                            ty: d.ty.clone(),
                        })
                        .collect(),
                ),
            },
        );
    }
}

// ---------- type expression resolution ----------
//
// `any` (in annotation position) maps onto the *wildcard* type
// (`Type::Opaque`, the same type `Opaque(...)` produces) rather than the
// internal `Type::Any` error-fallback — see `reject_any_storage` below for
// why: `any` can flow through ports/lets/params but can't back a
// var/array/buffer's storage gate. That mapping lives in the crate's single
// primitive-name table, `types::resolve::primitive`.

/// Resolve a type annotation against the current scope. Delegates to the
/// crate's single canonical resolver (`types::resolve::resolve_type`): the
/// scope's currently-visible `SymbolKind::Type` aliases are snapshotted (scope
/// doesn't mutate during a type expression's resolution, so this is
/// equivalent to the old per-`Name`-node live `ctx.scope.lookup`) and handed
/// in as `type_aliases`, and any WS002 the resolver raises is forwarded to
/// `ctx.diagnostics` — preserving both the alias-resolution and diagnostic
/// behavior of the resolver this replaced.
fn resolve_type_expr(ctx: &mut TypeCheckCtx, t: &TypeExpr) -> Type {
    // Fast path: a primitive name needs no alias snapshot. Primitives always win
    // over aliases in `resolve_type` (it checks `primitive` before the alias
    // map), so this is behaviour-identical — but it skips building the full
    // in-scope alias map (an O(D) frame scan + deep `Type` clones) on the
    // overwhelmingly common case, where annotations are `int`/`float`/`bool`/…
    // Without it that snapshot was rebuilt for every annotation, incl. per
    // cartesian combo inside a generic body — an accidental O(D²).
    if let TypeExpr::Name { name, .. } = t
        && let Some(prim) = crate::types::resolve::primitive(name)
    {
        return prim;
    }
    let aliases = ctx.scope.type_aliases();
    let mut diags = Vec::new();
    let result = {
        let cx = crate::types::resolve::ResolveCtx {
            params: &[],
            type_aliases: &aliases,
            generic_aliases: &ctx.generic_type_aliases,
        };
        crate::types::resolve::resolve_type(t, &cx, &mut diags)
    };
    ctx.diagnostics.extend(diags);
    result
}

/// A type param's mask — the set of concrete types it may be inferred to.
/// Unbound (`T`, no `: Bound`) is the full `Variant` mask; a bound narrows it
/// (see `types::mono::mask_for_param`).
fn type_param_mask(ctx: &TypeCheckCtx, tp: &TypeParam) -> Vec<Type> {
    // The bound → mask resolution lives in the shared `types::mono` module so
    // the lowering-side monomorphizer can rebuild the same masks.
    crate::types::mono::mask_for_param(tp.bound.as_ref(), &ctx.scope.type_aliases())
}

/// Cartesian product of per-type-param masks: each inner
/// `Vec<Type>` in `masks` is one type param's candidate concrete members;
/// the result has one `Vec<Type>` per combination, index-aligned with
/// `masks` (and so with the decl's `type_params`). Materializes the full
/// result — callers must cap the product size (e.g. against
/// `MAX_BODY_CHECK_COMBOS`) before calling this.
fn cartesian_product(masks: &[Vec<Type>]) -> Vec<Vec<Type>> {
    let mut result: Vec<Vec<Type>> = vec![Vec::new()];
    for mask in masks {
        let mut next = Vec::with_capacity(result.len() * mask.len());
        for combo in &result {
            for ty in mask {
                let mut extended = combo.clone();
                extended.push(ty.clone());
                next.push(extended);
            }
        }
        result = next;
    }
    result
}

// ---------- decl registration (1st pass) ----------

/// The type of a constant-literal expression, used to infer an unannotated
/// var's (or array var's element) type at registration, before the full type
/// map exists. Returns `None` for anything that isn't a compile-time literal.
fn literal_expr_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::IntLit { .. } => Some(Type::Int),
        Expr::AtomLit { .. } => Some(Type::Int),
        Expr::FloatLit { .. } => Some(Type::Float),
        Expr::BoolLit { .. } => Some(Type::Bool),
        Expr::StringLit { .. } | Expr::InterpLit { .. } => Some(Type::String),
        Expr::UnOp { op, operand, .. } if op == "-" => literal_expr_type(operand),
        // Constructor calls type by name alone — the value folds to a constant
        // later only if the args are constant, but the type holds regardless.
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident { name, .. } => match name.as_str() {
                "Vec" => Some(Type::Vector),
                "Rotation" => Some(Type::Rotator),
                "Color" | "ColorSRGB" | "ColorHex" => Some(Type::Color),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Types a Variable gate can hold as a wire variant — the safe targets when
/// refining an unannotated `var`'s placeholder `any` from its initializer.
fn var_storable(t: &Type) -> bool {
    matches!(
        t,
        Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Vector
            | Type::Rotator
            | Type::Quat
            | Type::Color
            | Type::Entity
            | Type::Character
            | Type::Controller
           
           
    )
}

fn type_expr_range(t: &TypeExpr) -> SourceRange {
    t.range().clone()
}

/// `any` (`Type::Opaque`) is fine anywhere a value just flows through a wire
/// (ports, `let`s, mod/chip params) — but a var/array/buffer's storage gate
/// needs a concrete wire variant to hold, so an explicit `any` annotation
/// there is rejected (WS025). The same holds for the reference types
/// (`zone`/`teleport`/`prefab`): like a var ref, they can only be wired or
/// rerouted, never held in a storage gate. Only fires on an *explicit*
/// annotation: an unannotated declaration's inferred placeholder is
/// `Type::Any`, never `Type::Opaque`, so it never reaches this check.
fn reject_any_storage(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange, what: &str) {
    if matches!(resolved, Type::Zone | Type::Teleport | Type::PrefabRef) {
        let n = crate::analysis::types::type_str(resolved);
        ctx.emit(
            "WS025",
            format!(
                "'{n}' is a reference and cannot be stored: {what} needs a concrete \
                 wire type — a '{n}' can only be wired or rerouted (like a var ref)"
            ),
            range,
        );
    } else if matches!(resolved, Type::Opaque) {
        ctx.emit(
            "WS025",
            format!(
                "'any' cannot be stored: {what} needs a concrete wire type \
                 (int/float/bool/string/vector/...)"
            ),
            range,
        );
    }
}

/// True if `t` is, or structurally contains, the wildcard `any` type
/// (`Type::Opaque`). A `*any`/`any[]`/`Map<_, any>`/`{ a: any }` annotation
/// wraps the `Opaque` under a `Ref`/`Array`/`Map`/`Record`/…, so a shallow
/// `matches!(t, Type::Opaque)` would miss it — recurse through every compound
/// so `warn_any_annotation` fires on the whole annotation regardless of nesting.
fn contains_opaque(t: &Type) -> bool {
    match t {
        Type::Opaque => true,
        Type::Ref(inner) | Type::Array(inner) => contains_opaque(inner),
        Type::Map(k, v) => contains_opaque(k) || contains_opaque(v),
        Type::Tuple(fields) | Type::Union(fields) => fields.iter().any(contains_opaque),
        Type::Record(fields) => fields.iter().any(|(_, ft)| contains_opaque(ft)),
        _ => false,
    }
}

/// True if `t` is, or structurally contains, a `Type::Param` — i.e. an
/// unresolved generic type parameter. A call whose param still carries one
/// (its `T` couldn't be inferred) is left to the WS033 inference diagnostics,
/// not the concrete arg-vs-param coercion check.
pub(crate) fn type_has_param(t: &Type) -> bool {
    match t {
        Type::Param(_) => true,
        Type::Ref(inner) | Type::Array(inner) => type_has_param(inner),
        Type::Map(k, v) => type_has_param(k) || type_has_param(v),
        Type::Tuple(fields) | Type::Union(fields) => fields.iter().any(type_has_param),
        Type::Record(fields) => fields.iter().any(|(_, ft)| type_has_param(ft)),
        _ => false,
    }
}

/// Warns (does not error) when a user's *non-storage* type annotation — an
/// `in`/`out` port, a mod/chip param or output, a handler's typed destructure
/// param, a `let`, or a `type X = ...` alias body — resolves to `any`
/// (`Type::Opaque`), including when the `any` is wrapped in a compound
/// (`*any`, `any[]`, `Map<_, any>`, `{ a: any }`, …). `any` still works there
/// (the value just flows through as a wildcard), but a generic type parameter
/// would let the type flow instead of erasing it. Storage positions
/// (var/array/buffer/map — see `reject_any_storage` above) are deliberately NOT
/// routed through this: an explicit `any` there is already a hard error
/// (WS025), and warning on top would double-fire the same annotation. Fires
/// once per annotation (at the top-level annotation range), not once per nested
/// `Opaque`.
fn warn_any_annotation(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange) {
    if contains_opaque(resolved) {
        ctx.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: "WS032".into(),
            message: "'any' works, but prefer a generic type parameter here — it lets the type \
                       flow instead of erasing it"
                .into(),
            range,
        });
    }
}

fn register_decl(ctx: &mut TypeCheckCtx, d: &TopDecl) {
    match d {
        TopDecl::Var(v) => {
            let inner = v
                .typ
                .as_ref()
                .map(|t| resolve_type_expr(ctx, t))
                // No annotation: infer an array type from a `[..]` initializer so
                // `var foo = [1, 2]` indexes/iterates as an array, not `Any`, and
                // a scalar type from a literal initializer so `var foo = ""` is a
                // string var (`var n = 0` an int var, …).
                .or_else(|| match &v.init {
                    Some(Expr::Array { elements, .. }) => {
                        let elem = elements
                            .iter()
                            .find_map(|el| literal_expr_type(el.expr()))
                            .unwrap_or(Type::Any);
                        Some(Type::Array(Box::new(elem)))
                    }
                    Some(init) => literal_expr_type(init),
                    None => None,
                })
                .unwrap_or(Type::Any);
            if let Some(t) = &v.typ {
                // A `var` can be scalar, array (`var x: T[]`), or map
                // (`var m: Map<K,V>`) storage — each needs a concrete element/
                // value wire type. Reject `any` in the STORED position: the
                // element for an array, the value for a map, else the type
                // itself. (Matches the `array`/`map` decl checks below, so the
                // `var` form has full parity now that those keywords are gone.)
                let (stored, what) = match &inner {
                    Type::Array(elem) => (elem.as_ref(), "an array's element type"),
                    Type::Map(_, val) => (val.as_ref(), "a map's value type"),
                    other => (other, "a variable gate"),
                };
                reject_any_storage(ctx, stored, type_expr_range(t), what);
            }
            declare_or_dup(
                ctx,
                &v.name,
                SymbolInfo {
                    kind: SymbolKind::Var,
                    name: v.name.clone(),
                    ty: Type::Ref(Box::new(inner)),
                    decl_range: v.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Array(a) => {
            let inner = resolve_type_expr(ctx, &a.element_type);
            reject_any_storage(
                ctx,
                &inner,
                type_expr_range(&a.element_type),
                "an array's element type",
            );
            declare_or_dup(
                ctx,
                &a.name,
                SymbolInfo {
                    kind: SymbolKind::Array,
                    name: a.name.clone(),
                    ty: Type::Array(Box::new(inner)),
                    decl_range: a.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Map(m) => {
            let key = resolve_type_expr(ctx, &m.key_type);
            let value = resolve_type_expr(ctx, &m.value_type);
            reject_any_storage(
                ctx,
                &value,
                type_expr_range(&m.value_type),
                "a map's value type",
            );
            declare_or_dup(
                ctx,
                &m.name,
                SymbolInfo {
                    kind: SymbolKind::Map,
                    name: m.name.clone(),
                    ty: Type::Map(Box::new(key), Box::new(value)),
                    decl_range: m.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Buffer(b) => {
            // Type refined in pass 2 from RHS unless an annotation exists.
            let placeholder = b
                .typ
                .as_ref()
                .map(|t| resolve_type_expr(ctx, t))
                .unwrap_or(Type::Any);
            if let Some(t) = &b.typ {
                reject_any_storage(ctx, &placeholder, type_expr_range(t), "a buffer");
            }
            declare_or_dup(
                ctx,
                &b.name,
                SymbolInfo {
                    kind: SymbolKind::Buffer,
                    name: b.name.clone(),
                    ty: placeholder,
                    decl_range: b.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::In(d) => {
            let t = resolve_type_expr(ctx, &d.typ);
            warn_any_annotation(ctx, &t, type_expr_range(&d.typ));
            declare_or_dup(
                ctx,
                &d.name,
                SymbolInfo {
                    kind: SymbolKind::In,
                    name: d.name.clone(),
                    ty: t,
                    decl_range: d.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::Fn(f) => {
            let params: Vec<EventDataField> = f
                .params
                .iter()
                .map(|p| {
                    let ty = resolve_type_expr(ctx, &p.typ);
                    warn_any_annotation(ctx, &ty, type_expr_range(&p.typ));
                    EventDataField {
                        name: p.name.clone(),
                        ty,
                    }
                })
                .collect();
            let ret = f
                .return_type
                .as_ref()
                .map(|t| {
                    let ty = resolve_type_expr(ctx, t);
                    warn_any_annotation(ctx, &ty, type_expr_range(t));
                    ty
                })
                .unwrap_or(Type::Any);
            declare_or_dup(
                ctx,
                &f.name,
                SymbolInfo {
                    kind: SymbolKind::Fn,
                    name: f.name.clone(),
                    ty: Type::Any,
                    decl_range: f.range.clone(),
                    signature: Some(FnOrChipSig {
                        params,
                        outputs: vec![EventDataField {
                            name: "_".into(),
                            ty: ret,
                        }],
                        type_params: Vec::new(),
                    }),
                    event_data: None,
                },
            );
        }
        TopDecl::Chip(c) => {
            // Generic *chips* (physical microchips, `inline == false`) are
            // monomorphized per distinct type instantiation at lowering time:
            // `lower_chip_call_instance` builds one microchip template per
            // `(name, concrete subst)` and keys the emitted grid on it, so
            // `Box<int>` and `Box<vector>` never share a body. (Generic `mod`s
            // still inline per call site.) Nothing to reject here.
            // Generic decl: register each type param as a scope `Type`
            // symbol resolving to `Type::Param(name)` so the sig's own
            // `TypeExpr`s (below) see `T` as a known type instead of
            // WS002. Scoped to this signature's resolution only — popped
            // before `declare_or_dup` so it never leaks to sibling decls.
            let has_type_params = !c.type_params.is_empty();
            // Name + mask for each type param, computed once at registration
            // (mirrors the scope-declare loop just below) so the call-typing
            // path can later solve `Constraint`s against it without having to
            // re-walk the decl. Stored on the signature (see `FnOrChipSig`).
            let mut type_param_masks: Vec<(String, Vec<Type>)> = Vec::new();
            if has_type_params {
                ctx.push_scope();
                for tp in &c.type_params {
                    ctx.scope.declare(
                        &tp.name,
                        SymbolInfo {
                            kind: SymbolKind::Type,
                            name: tp.name.clone(),
                            ty: Type::Param(tp.name.clone()),
                            decl_range: tp.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                type_param_masks = c
                    .type_params
                    .iter()
                    .map(|tp| (tp.name.clone(), type_param_mask(ctx, tp)))
                    .collect();
            }
            let params: Vec<EventDataField> = c
                .inputs
                .iter()
                .map(|p| {
                    let ty = resolve_type_expr(ctx, &p.typ);
                    warn_any_annotation(ctx, &ty, type_expr_range(&p.typ));
                    EventDataField {
                        name: p.name.clone(),
                        ty,
                    }
                })
                .collect();
            let outputs: Vec<EventDataField> = c
                .outputs
                .iter()
                .map(|o| {
                    let ty = resolve_type_expr(ctx, &o.typ);
                    warn_any_annotation(ctx, &ty, type_expr_range(&o.typ));
                    EventDataField {
                        name: o.name.clone(),
                        ty,
                    }
                })
                .collect();
            if has_type_params {
                ctx.pop_scope();
            }
            // A `self`-mod whose name AND receiver type collide with a builtin
            // receiver-method (e.g. `mod Dot(self: vector, …)` vs the builtin
            // `Dot` on vector) would be silently shadowed at every call site —
            // the builtin always wins the `.method()` resolution — so reject it
            // at the declaration. Only flags an *overlapping* receiver type
            // (same coercion rule the completion uses): a same-named mod on an
            // unrelated receiver type is reachable and left alone.
            if c.is_self_receiver()
                && let Some(recv) = params.first().map(|p| p.ty.clone())
                && let Some(builtin_recv) = find_call(&c.name).and_then(|b| b.receiver.clone())
                && matches!(coerce(&recv, &builtin_recv), CoerceRule::Same | CoerceRule::Coerce)
            {
                ctx.emit(
                    "WS035",
                    format!(
                        "`{name}` shadows the builtin receiver-method `{name}` on `{recv}` — \
                         a call like `x.{name}(…)` always resolves to the builtin, so this \
                         mod could never be reached as a method (rename it)",
                        name = c.name,
                        recv = crate::analysis::types::type_str(&recv),
                    ),
                    c.inputs
                        .first()
                        .map(|p| p.range.clone())
                        .unwrap_or_else(|| c.range.clone()),
                );
            }
            declare_or_dup(
                ctx,
                &c.name,
                SymbolInfo {
                    kind: SymbolKind::Chip,
                    name: c.name.clone(),
                    ty: Type::Any,
                    decl_range: c.range.clone(),
                    signature: Some(FnOrChipSig {
                        params,
                        outputs,
                        type_params: type_param_masks,
                    }),
                    event_data: None,
                },
            );
        }
        TopDecl::Event(e) => {
            declare_or_dup(
                ctx,
                &e.name,
                SymbolInfo {
                    kind: SymbolKind::Event,
                    name: e.name.clone(),
                    ty: Type::Exec,
                    decl_range: e.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        TopDecl::AnonChip(ac) => {
            // Anon chip shares parent scope — register its inner decls.
            for s in &ac.body.stmts {
                match s {
                    Stmt::Var(v) => register_decl(ctx, &TopDecl::Var(v.clone())),
                    Stmt::Buffer(b) => register_decl(ctx, &TopDecl::Buffer(b.clone())),
                    Stmt::Array(a) => register_decl(ctx, &TopDecl::Array(a.clone())),
                    Stmt::In(i) => register_decl(ctx, &TopDecl::In(i.clone())),
                    _ => {}
                }
            }
        }
        TopDecl::TypeAlias(t) => {
            if t.type_params.is_empty() {
                let resolved = resolve_type_expr(ctx, &t.typ);
                warn_any_annotation(ctx, &resolved, type_expr_range(&t.typ));
                declare_or_dup(
                    ctx,
                    &t.name,
                    SymbolInfo {
                        kind: SymbolKind::Type,
                        name: t.name.clone(),
                        ty: resolved,
                        decl_range: t.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            } else {
                // Generic alias: already collected into `ctx.generic_type_aliases`
                // by the `collect_generic_aliases` pre-pass (its body is
                // resolved lazily, per-use, by `resolve_type` — NOT here,
                // since it still references its own free `type_params`).
                // Still declare a placeholder `SymbolKind::Type` symbol so
                // `declare_or_dup` catches a duplicate name; a bare use of
                // this name (no `<Args>`) is caught by `resolve_type`'s
                // `generic_aliases` check before it would ever reach this
                // placeholder via the plain alias map.
                declare_or_dup(
                    ctx,
                    &t.name,
                    SymbolInfo {
                        kind: SymbolKind::Type,
                        name: t.name.clone(),
                        ty: Type::Any,
                        decl_range: t.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        TopDecl::Out(_)
        | TopDecl::Let(_)
        | TopDecl::Handler(_)
        | TopDecl::Assign(_)
        | TopDecl::If(_)
        | TopDecl::ExprStmt(_)
        | TopDecl::Import(_)
        | TopDecl::Await(_) => {
            // Resolved before typecheck.
        }
        TopDecl::Namespace(ns) => {
            declare_or_dup(
                ctx,
                &ns.name,
                SymbolInfo {
                    kind: SymbolKind::Namespace,
                    name: ns.name.clone(),
                    ty: Type::Any,
                    decl_range: ns.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
            let mut ns_map = HashMap::default();
            for d in &ns.decls {
                match d {
                    TopDecl::Chip(c) => {
                        // Carry the outputs across as the member's result type.
                        // Without them a namespaced `mod f() -> int` types as
                        // `Any`, so `Ns.f(x) + 1` finds no operator overload and
                        // the whole expression drops to an unsupported gate.
                        let return_type = match c.outputs.len() {
                            0 => None,
                            1 => Some(c.outputs[0].typ.clone()),
                            _ => Some(TypeExpr::Record {
                                fields: c
                                    .outputs
                                    .iter()
                                    .map(|o| RecordTypeField {
                                        name: o.name.clone(),
                                        typ: o.typ.clone(),
                                        range: o.range.clone(),
                                    })
                                    .collect(),
                                range: c.range.clone(),
                            }),
                        };
                        // Mirror the chip param build (register_decl's
                        // `TopDecl::Chip` arm above) so a namespaced call can
                        // be arg-checked the same way a local call is. The
                        // `warn_any_annotation` call is skipped here — it
                        // already fires once when the member's own decl is
                        // checked (this loop only indexes it for lookup).
                        //
                        // A GENERIC member's param types reference its own
                        // type params (`mod maxT<T>(a: T, …)`), so — exactly
                        // like the normal registration above — push those type
                        // params into scope as `Type::Param` symbols BEFORE
                        // resolving, then pop. Without this a generic member's
                        // `T` hits `resolve_type_expr`'s unknown-type arm and
                        // wrongly emits WS002 at mere import time (the member
                        // need not even be called).
                        let has_type_params = !c.type_params.is_empty();
                        if has_type_params {
                            ctx.push_scope();
                            for tp in &c.type_params {
                                ctx.scope.declare(
                                    &tp.name,
                                    SymbolInfo {
                                        kind: SymbolKind::Type,
                                        name: tp.name.clone(),
                                        ty: Type::Param(tp.name.clone()),
                                        decl_range: tp.range.clone(),
                                        signature: None,
                                        event_data: None,
                                    },
                                );
                            }
                        }
                        let params: Vec<EventDataField> = c
                            .inputs
                            .iter()
                            .map(|p| EventDataField {
                                name: p.name.clone(),
                                ty: resolve_type_expr(ctx, &p.typ),
                            })
                            .collect();
                        if has_type_params {
                            ctx.pop_scope();
                        }
                        ns_map.insert(
                            c.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Chip,
                                return_type,
                                params,
                            },
                        );
                    }
                    TopDecl::Fn(f) => {
                        let params: Vec<EventDataField> = f
                            .params
                            .iter()
                            .map(|p| EventDataField {
                                name: p.name.clone(),
                                ty: resolve_type_expr(ctx, &p.typ),
                            })
                            .collect();
                        ns_map.insert(
                            f.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Fn,
                                return_type: f.return_type.clone(),
                                params,
                            },
                        );
                    }
                    // Declare the module's type aliases under their qualified
                    // name so `let p: Ns.Point` resolves through the ordinary
                    // type lookup. The bare name stays private to the module.
                    TopDecl::TypeAlias(t) => {
                        let qualified = format!("{}.{}", ns.name, t.name);
                        if t.type_params.is_empty() {
                            let resolved = resolve_type_expr(ctx, &t.typ);
                            warn_any_annotation(ctx, &resolved, type_expr_range(&t.typ));
                            ctx.scope.declare(
                                &qualified,
                                SymbolInfo {
                                    kind: SymbolKind::Type,
                                    name: qualified.clone(),
                                    ty: resolved,
                                    decl_range: t.range.clone(),
                                    signature: None,
                                    event_data: None,
                                },
                            );
                        } else {
                            // Generic alias, already collected under its
                            // qualified name by `collect_generic_aliases`;
                            // see the top-level `TopDecl::TypeAlias` arm.
                            ctx.scope.declare(
                                &qualified,
                                SymbolInfo {
                                    kind: SymbolKind::Type,
                                    name: qualified.clone(),
                                    ty: Type::Any,
                                    decl_range: t.range.clone(),
                                    signature: None,
                                    event_data: None,
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
            ctx.namespaces.insert(ns.name.clone(), ns_map);
        }
    }
}

fn declare_or_dup(ctx: &mut TypeCheckCtx, name: &str, info: SymbolInfo) {
    let range = info.decl_range.clone();
    if ctx.scope.declare(name, info).is_some() {
        ctx.emit("WS013", format!("duplicate declaration of '{name}'"), range);
    }
}

// ---------- decl checking (2nd pass) ----------

/// Validate a top-level (non-exec) var initializer: every element must be a
/// constant literal whose type coerces to the array's element type. Spreads and
/// non-literal values are only meaningful when the array is built at runtime, so
/// they're rejected here with a pointer to assigning the array in an exec
/// handler.
fn check_top_level_array_init(
    ctx: &mut TypeCheckCtx,
    elements: &[ArrayElem],
    elem_ty: &Type,
) {
    for el in elements {
        let e = el.expr();
        let t = ctx.in_pure(|ctx| infer::infer(ctx, e));
        if matches!(el, ArrayElem::Spread(_)) {
            ctx.emit(
                "WS003",
                "spread `...` in an array initializer is only allowed when building the array inside an exec handler",
                e.range().clone(),
            );
        } else if let Some(lit) = crate::lower::expr_to_literal_in(e, &ctx.const_env) {
            // Asset / prefab references are object references — they lower to
            // their own reference gate (e.g. AudioReference) whose output must be
            // WIRED into the array, so they can't be baked into the initializer's
            // constant value list. Inlined here they'd be silently dropped.
            if matches!(
                lit,
                crate::ir::Literal::Asset { .. }
                    | crate::ir::Literal::PrefabRef { .. }
                    | crate::ir::Literal::NestedPrefab { .. }
            ) {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS024".into(),
                    message: "asset / prefab references can't be inlined into an array initializer — \
                              they're object references wired in from their own brick. Build the array \
                              with `.push(...)` inside an exec handler instead."
                        .into(),
                    range: e.range().clone(),
                });
            } else if !matches!(elem_ty, Type::Any) && coerce(&t, elem_ty) == CoerceRule::Mismatch {
                ctx.emit(
                    "WS003",
                    format!(
                        "var element: expected {}, got {}",
                        crate::analysis::type_str(elem_ty),
                        crate::analysis::type_str(&t)
                    ),
                    e.range().clone(),
                );
            }
        } else {
            ctx.emit(
                "WS003",
                "array initializer elements must be constant literals — assign the array inside an exec handler to build it from runtime values",
                e.range().clone(),
            );
        }
    }
}

/// Check a map literal's entries against `Map<K, V>`: each key must coerce to
/// `k`, each value to `v`. This is deliberately STRICTER than the `coerce_or_emit`
/// sink assignment checking uses — a map *literal* entry additionally rejects
/// `ViaString` (and any coercion that isn't `Same`/`Coerce`), because the literal
/// entry has no gate to run a string-format coercion through (an assignment does).
/// Call this from the VALID slots — a `var` initializer or an assignment
/// RHS to a map var — so the literal never reaches the generic `infer`
/// `MapLit` arm, which is the position-guard for every other use.
///
/// The entries of a map initializer for a `Map<K, V>` slot: a real `MapLit`, or
/// an empty `{}`. The parser emits `{}` as an empty `RecordLit` (with no keys it
/// can't tell an empty map from an empty record), but in a `Map`-typed slot it
/// is the natural spelling of an empty-map initializer — equivalent to no
/// initializer, since lowering's `bake_map_init` starts the map empty for any
/// non-`MapLit` init. Returns `None` for anything else (a non-empty record, a
/// scalar, …) so it still hits the generic coerce / position guard.
fn map_init_entries(init: &Expr) -> Option<&[MapLitEntry]> {
    match init {
        Expr::MapLit { entries, .. } => Some(entries),
        Expr::RecordLit { fields, .. } if fields.is_empty() => Some(&[]),
        _ => None,
    }
}

fn check_map_literal(
    ctx: &mut TypeCheckCtx,
    entries: &[MapLitEntry],
    k: &Type,
    v: &Type,
) {
    // A map literal entry has no gate to run a coercion through (e.g.
    // `ViaString`'s `FormatText` gate) — only coercions that hold at the
    // literal itself (`Same`/`Coerce`) are valid; anything else (notably
    // `ViaString`) would silently corrupt every non-matching entry at emit.
    for MapLitEntry { key, value, .. } in entries {
        let kt = infer::infer(ctx, key);
        let key_rule = coerce(&kt, k);
        if !matches!(key_rule, CoerceRule::Same | CoerceRule::Coerce) {
            ctx.emit(
                "WS003",
                format!(
                    "map literal key: expected {}, got {} — a map literal entry \
                     can't be string-formatted; write a matching-type literal",
                    crate::analysis::types::type_str(k),
                    crate::analysis::types::type_str(&kt),
                ),
                key.range().clone(),
            );
        }
        let vt = infer::infer(ctx, value);
        let value_rule = coerce(&vt, v);
        if !matches!(value_rule, CoerceRule::Same | CoerceRule::Coerce) {
            ctx.emit(
                "WS003",
                format!(
                    "map literal value: expected {}, got {} — a map literal entry \
                     can't be string-formatted; write a matching-type literal",
                    crate::analysis::types::type_str(v),
                    crate::analysis::types::type_str(&vt),
                ),
                value.range().clone(),
            );
        }
    }
}

fn check_decl(
    ctx: &mut TypeCheckCtx,
    d: &TopDecl,
) {
    match d {
        TopDecl::Var(v) => {
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                // An array-valued `var` declared at top level (no exec context)
                // is baked into the gate just like an `array` decl — its elements
                // must be constant literals.
                if let Expr::Array { elements, .. } = init {
                    // Type the whole array expr so its element type lands in the
                    // map — lowering reads it to infer an unannotated var's type.
                    ctx.in_pure(|ctx| {
                        infer::infer(ctx, init);
                    });
                    let elem_ty = match &inner {
                        Type::Array(e) => e.as_ref().clone(),
                        _ => Type::Any,
                    };
                    check_top_level_array_init(ctx, elements, &elem_ty);
                } else if let Some(entries) = map_init_entries(init)
                    && let Type::Map(k, v_ty) = &inner
                {
                    // A map-valued `var` initializer: validate entries against
                    // the declared `Map<K, V>` directly, bypassing the generic
                    // `infer` `MapLit` arm (the position guard) — this IS
                    // the valid initializer slot. An empty `{}` is an empty-map init.
                    ctx.in_pure(|ctx| {
                        check_map_literal(ctx, entries, k, v_ty);
                    });
                } else {
                    ctx.in_pure(|ctx| {
                        let t = infer::check(ctx, init, &inner);
                        // Unannotated var with a non-literal init (`var v =
                        // Vec(…)`): refine the placeholder `any` from the RHS,
                        // like buffers do.
                        if v.typ.is_none() && matches!(inner, Type::Any) {
                            let u = unwrap_ref(&t);
                            if var_storable(&u) {
                                ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                            }
                        }
                    });
                }
            }
        }
        TopDecl::Buffer(b) => {
            ctx.in_pure(|ctx| {
                let t = infer::infer(ctx, &b.init);
                if b.typ.is_none() {
                    let unwrapped = unwrap_ref(&t);
                    ctx.scope.set_type(&b.name, unwrapped);
                }
            });
        }
        TopDecl::Array(a) => {
            // A top-level initializer is baked into the gate, so its elements
            // must be constant literals matching the element type.
            if !a.init.is_empty() {
                let inner = resolve_type_expr(ctx, &a.element_type);
                check_top_level_array_init(ctx, &a.init, &inner);
            }
        }
        TopDecl::Map(m) => {
            // Optional literal initializer: `var m: Map<K, V> = { k => v, ... }`.
            // Key/value types were already resolved in registration — reuse the
            // registered symbol instead of re-resolving (avoids double-emitting
            // an "unknown type" diagnostic if `key_type`/`value_type` is bad).
            if let Some(init) = &m.init {
                let (key, value) = match ctx.scope.lookup(&m.name) {
                    Some(SymbolInfo {
                        ty: Type::Map(k, v),
                        ..
                    }) => (k.as_ref().clone(), v.as_ref().clone()),
                    _ => (Type::Any, Type::Any),
                };
                if let Some(entries) = map_init_entries(init) {
                    // The valid initializer slot: validate entries directly,
                    // bypassing the generic `infer` `MapLit` arm (the
                    // position guard). An empty `{}` is an empty-map init.
                    ctx.in_pure(|ctx| {
                        check_map_literal(ctx, entries, &key, &value);
                    });
                } else {
                    ctx.in_pure(|ctx| {
                        infer::check(
                            ctx,
                            init,
                            &Type::Map(Box::new(key.clone()), Box::new(value.clone())),
                        );
                    });
                }
            }
        }
        TopDecl::In(_) => {
            // Already handled in registration.
        }
        TopDecl::Out(b) => {
            if let Some(value) = &b.value {
                let value_ty = ctx.in_pure(|ctx| infer::infer(ctx, value));
                // When out has ref type and value is a var, override to show "ref" in hover
                if let Some(ref te) = b.typ {
                    let resolved = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &resolved, type_expr_range(te));
                    if matches!(resolved, Type::Ref(_))
                        && let Expr::Ident { range, .. } = value
                    {
                        ctx.var_read_contexts
                            .remove(&(range.file.clone(), range.start.offset));
                    }
                    // An annotated out must accept its value (WS003 on a
                    // genuine mismatch; coercions — including string → bool,
                    // which lowers to an inserted `!= ""` compare at the
                    // port — pass). Both sides unwrap refs so `out y: *int
                    // = x` compares int against int, the ref-ness being the
                    // exposure mode rather than a value type.
                    infer::coerce_or_emit(
                        ctx,
                        &unwrap_ref(&value_ty),
                        &unwrap_ref(&resolved),
                        value.range(),
                    );
                }
                if b.typ.is_none()
                    && let Expr::Ident { name, .. } = value
                    && let Some(sym) = ctx.scope.lookup(name)
                    && sym.kind == SymbolKind::Var
                {
                    ctx.diagnostics.push(Diagnostic {
                                    severity: Severity::Warning,
                                    code: "WS017".into(),
                                    message: format!(
                                        "out '{}' infers type from var '{}' — add explicit type: \
                                         `out {}: {} = {}` for value, or `out {}: *{} = {}` for ref",
                                        b.name, name,
                                        b.name, crate::analysis::types::type_str(&unwrap_ref(&sym.ty)), name,
                                        b.name, crate::analysis::types::type_str(&unwrap_ref(&sym.ty)), name,
                                    ),
                                    range: b.range.clone(),
                                });
                }
            }
        }
        TopDecl::Let(l) => {
            let t = ctx.in_pure(|ctx| infer::infer(ctx, &l.value));
            check_let_type_annotation(ctx, l, &t);
            bind_let(ctx, &l.binding, &t);
        }
        TopDecl::Fn(f) => {
            ctx.push_scope();
            for p in &f.params {
                let pt = resolve_type_expr(ctx, &p.typ);
                ctx.scope.declare(
                    &p.name,
                    SymbolInfo {
                        kind: SymbolKind::Param,
                        name: p.name.clone(),
                        ty: pt,
                        decl_range: p.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            ctx.in_pure(|ctx| {
                infer::infer(ctx, &f.body);
            });
            ctx.pop_scope();
        }
        TopDecl::Chip(c) => {
            // Bounded-polymorphism body checking. Body-check the
            // decl once per concrete member of the cartesian product of its
            // type params' masks — an operation on a type param (`a + 1`)
            // can't resolve against the signature's `Type::Param`
            // placeholder (registration, untouched by this
            // arm), and even where it somehow could, the body must be valid
            // for EVERY concrete type the mask allows, not just checked once
            // symbolically. A non-generic decl (`type_params` empty) gets
            // the single trivial "no bindings" combo below, so its check —
            // and its diagnostics — are unchanged from before this task.
            const MAX_BODY_CHECK_COMBOS: usize = 64;
            // Budget is a whole-PATH cap, not per-decl: a nested generic decl is
            // re-checked once per OUTER combo, so divide the cap by the combos
            // already committed on this path (`active_combos`). At the top level
            // `active_combos == 1`, so a single generic decl is unchanged (cap
            // 64); deeper nesting shrinks the effective cap toward 1, bounding
            // total body-check work at ~64 regardless of nesting depth.
            let per_path_cap = (MAX_BODY_CHECK_COMBOS / ctx.active_combos.max(1)).max(1);
            let combos: Vec<Vec<Type>> = if c.type_params.is_empty() {
                vec![Vec::new()]
            } else {
                let masks: Vec<Vec<Type>> = c
                    .type_params
                    .iter()
                    .map(|tp| {
                        let m = type_param_mask(ctx, tp);
                        if m.is_empty() { vec![Type::Any] } else { m }
                    })
                    .collect();
                let mut total: usize = 1;
                let mut capped = false;
                for m in &masks {
                    match total.checked_mul(m.len()) {
                        Some(t) if t <= per_path_cap => total = t,
                        _ => {
                            capped = true;
                            break;
                        }
                    }
                }
                if capped {
                    // Too many combinations for the full cartesian product.
                    // A single all-first-member combo is unsound: a param's
                    // first mask member is the MOST permissive for arithmetic
                    // (an unbounded param's is `bool`, which coerces into every
                    // numeric op), so a body op invalid for other members would
                    // slip through unchecked (a silent miscompile at any call
                    // site that picks those members). Instead vary each param
                    // across its WHOLE mask while the others hold a fixed
                    // representative (mask[0]). This checks every operation
                    // whose validity depends on a single param's type — the
                    // realistic case — at O(sum of mask sizes) rather than
                    // O(product), then truncates to the path budget so nested
                    // generics stay bounded. Residual: an op valid for every
                    // single-param variation yet invalid only for a specific
                    // multi-param COMBINATION is not caught (needs ≥2 unbounded
                    // params AND a cross-param-only type dependency).
                    let rep: Vec<Type> = masks.iter().map(|m| m[0].clone()).collect();
                    let mut combos = vec![rep.clone()];
                    for (i, m) in masks.iter().enumerate() {
                        for member in m.iter().skip(1) {
                            let mut combo = rep.clone();
                            combo[i] = member.clone();
                            combos.push(combo);
                        }
                    }
                    combos.truncate(per_path_cap.max(1));
                    combos
                } else {
                    cartesian_product(&masks)
                }
            };
            // Diagnostics are only batched + deduped for an actual
            // multi-combo generic check; the non-generic single combo
            // writes straight into `ctx.diagnostics` exactly as before.
            let multi_pass = combos.len() > 1;
            let mut seen = std::collections::HashSet::new();
            let mut deduped: Vec<Diagnostic> = Vec::new();
            // Commit this decl's combos to the path budget so any nested generic
            // decl checked inside the loop divides the REMAINING cap (restored
            // after). Non-generic decls (`combos.len() == 1`) don't move it.
            let saved_active_combos = ctx.active_combos;
            ctx.active_combos = saved_active_combos.saturating_mul(combos.len().max(1));
            for combo in &combos {
                ctx.push_scope();
                // Same registration as the signature pass (register_decl),
                // but in the body scope: covers annotations inside the body
                // (e.g. `let x: T = a`). For a generic decl each type param
                // is bound to THIS combo's concrete mask member (not
                // `Type::Param` — an operation like `a + 1` must resolve
                // against a real type); `combo` is empty for a non-generic
                // decl, so this loop is a no-op there. Scope is popped at
                // the end of each pass, so params never leak to sibling
                // decls.
                for (tp, ty) in c.type_params.iter().zip(combo.iter()) {
                    ctx.scope.declare(
                        &tp.name,
                        SymbolInfo {
                            kind: SymbolKind::Type,
                            name: tp.name.clone(),
                            ty: ty.clone(),
                            decl_range: tp.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                let before = ctx.diagnostics.len();
                for p in &c.inputs {
                    let pt = resolve_type_expr(ctx, &p.typ);
                    let kind = if matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
                        SymbolKind::Var
                    } else {
                        SymbolKind::Param
                    };
                    // If the param has a destructuring pattern, register the
                    // synthetic name with the full type, then also register each
                    // destructured field with its resolved field type.
                    if let Some(pattern) = &p.pattern {
                        ctx.scope.declare(
                            &p.name,
                            SymbolInfo {
                                kind: SymbolKind::Param,
                                name: p.name.clone(),
                                ty: pt.clone(),
                                decl_range: p.range.clone(),
                                signature: None,
                                event_data: None,
                            },
                        );
                        match pattern {
                            crate::ast::ParamPattern::Record { fields, .. } => {
                                for field in fields {
                                    match field {
                                        crate::ast::RecordDestructField::Named {
                                            name, alias, ..
                                        } => {
                                            let bind_name = alias.as_ref().unwrap_or(name);
                                            let field_ty = if let Type::Record(rec_fields) = &pt {
                                                rec_fields
                                                    .iter()
                                                    .find(|(k, _)| k == name)
                                                    .map(|(_, t)| t.clone())
                                                    .unwrap_or(Type::Any)
                                            } else {
                                                Type::Any
                                            };
                                            ctx.scope.declare(
                                                bind_name,
                                                SymbolInfo {
                                                    kind: SymbolKind::Param,
                                                    name: bind_name.clone(),
                                                    ty: field_ty,
                                                    decl_range: p.range.clone(),
                                                    signature: None,
                                                    event_data: None,
                                                },
                                            );
                                        }
                                        crate::ast::RecordDestructField::Rest { name, .. } => {
                                            ctx.scope.declare(
                                                name,
                                                SymbolInfo {
                                                    kind: SymbolKind::Param,
                                                    name: name.clone(),
                                                    ty: Type::Any,
                                                    decl_range: p.range.clone(),
                                                    signature: None,
                                                    event_data: None,
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                            crate::ast::ParamPattern::Tuple { names, .. } => {
                                let field_types = if let Type::Tuple(fs) = &pt {
                                    fs.clone()
                                } else {
                                    vec![]
                                };
                                for (i, name) in names.iter().enumerate() {
                                    let field_ty = field_types.get(i).cloned().unwrap_or(Type::Any);
                                    ctx.scope.declare(
                                        name,
                                        SymbolInfo {
                                            kind: SymbolKind::Param,
                                            name: name.clone(),
                                            ty: field_ty,
                                            decl_range: p.range.clone(),
                                            signature: None,
                                            event_data: None,
                                        },
                                    );
                                }
                            }
                        }
                    } else {
                        ctx.scope.declare(
                            &p.name,
                            SymbolInfo {
                                kind,
                                name: p.name.clone(),
                                ty: pt,
                                decl_range: p.range.clone(),
                                signature: None,
                                event_data: None,
                            },
                        );
                    }
                }
                ctx.in_exec(|ctx| check_block(ctx, &c.body));
                // Warn if outputs are declared but never assigned. Assignments
                // count anywhere in the body, including nested if blocks and
                // `on` handlers: `out x = expr`, `emit x (= expr)`, or a plain
                // `x = expr` assignment.
                if !c.outputs.is_empty() && !block_has_return_value(&c.body) {
                    let mut assigned = std::collections::HashSet::default();
                    collect_output_assignments(&c.body, &mut assigned);
                    for out in &c.outputs {
                        if !assigned.contains(&out.name) {
                            ctx.emit(
                                "WS013",
                                format!("output '{}' is never assigned — use `out {} = expr`, `emit {}`, or `return expr`", out.name, out.name, out.name),
                                out.range.clone(),
                            );
                        }
                    }
                }
                if multi_pass {
                    // Dedup by (code, start offset): the same body error
                    // surfaces once per mask member it's invalid for, but
                    // should be reported once, at the definition.
                    for d in ctx.diagnostics.split_off(before) {
                        if seen.insert((d.code.clone(), d.range.start.offset)) {
                            deduped.push(d);
                        }
                    }
                }
                ctx.pop_scope();
            }
            ctx.active_combos = saved_active_combos;
            if multi_pass {
                ctx.diagnostics.extend(deduped);
            }
        }
        TopDecl::AnonChip(ac) => {
            // Anon chip shares parent scope — NO scope push/pop.
            // Vars already pre-registered in pass 1; use check_decl (not
            // check_stmt) for them to avoid duplicate-declaration errors.
            check_anon_chip_stmts(ctx, &ac.body.stmts, true);
        }
        TopDecl::Event(e) => {
            if let Some(body) = &e.captured_body {
                ctx.in_exec(|ctx| check_block(ctx, body));
            } else {
                ctx.in_pure(|ctx| {
                    infer::infer(ctx, &e.source);
                });
            }
        }
        TopDecl::Handler(h) => {
            ctx.push_scope();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body));
            ctx.pop_scope();
        }
        TopDecl::ExprStmt(s) => {
            ctx.in_pure(|ctx| {
                infer::infer(ctx, &s.expr);
            });
        }
        TopDecl::Assign(a) => {
            check_stmt(ctx, &Stmt::Assign(a.clone()));
        }
        TopDecl::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "top-level 'if' outside an exec context",
                    i.range.clone(),
                );
            }
            check_stmt(ctx, &Stmt::If(i.clone()));
        }
        TopDecl::Namespace(ns) => {
            // A namespaced (`import * as ns`) mod body references its sibling
            // constants and mods by BARE name, and those mods are inlined at
            // call sites in the importing module. Typecheck the bodies here in
            // an isolated scope (siblings registered as bare names) so operator
            // resolutions and expression types get recorded — otherwise the
            // inlined body's arithmetic and sibling calls lower to _Unsupported.
            ctx.push_scope();
            for d in &ns.decls {
                register_decl(ctx, d);
            }
            for d in &ns.decls {
                if matches!(
                    d,
                    TopDecl::Let(_) | TopDecl::Var(_) | TopDecl::Array(_) | TopDecl::Buffer(_)
                ) {
                    check_decl(ctx, d);
                }
            }
            for d in &ns.decls {
                if matches!(d, TopDecl::Chip(_) | TopDecl::Fn(_)) {
                    check_decl(ctx, d);
                }
            }
            ctx.pop_scope();
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) => {}
    }
}

fn bind_handler_trigger_params(ctx: &mut TypeCheckCtx, h: &Handler) {
    let (name, range) = match &h.trigger {
        Trigger::Ident { name, range } => (name, range),
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, range } => (name, range),
            _ => return,
        },
        _ => return,
    };
    {
        let evt = find_event(name);
        let sym = ctx.scope.lookup(name).cloned();
        let known_event = evt.is_some();
        let known_capture = matches!(&sym, Some(s) if s.kind == SymbolKind::Event);
        let known_input_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::In && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
        );
        let known_buffer_trigger = matches!(
            &sym,
            Some(s)
                if s.kind == SymbolKind::Buffer
                    && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Any)
        );
        let known_let_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::LetBinding
        );
        // A mod/chip param (`Param`) or an event data output bound by the
        // enclosing handler (`EventParam`, e.g. `on CustomEvent("x") -> (p:
        // character)`) can trigger a nested handler on its value/edge — `on p`
        // / `on !p`.
        let known_param_trigger = matches!(
            &sym,
            Some(s) if matches!(s.kind, SymbolKind::Param | SymbolKind::EventParam)
                && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Character | Type::Controller | Type::Entity)
        );
        // A `var` can trigger a handler on its value change — `on x` / `on !x`.
        let known_var_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::Var
                && matches!(s.ty, Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
        );
        if !known_event
            && !known_capture
            && !known_input_trigger
            && !known_buffer_trigger
            && !known_let_trigger
            && !known_param_trigger
            && !known_var_trigger
        {
            ctx.emit(
                "WS001",
                format!("unknown event or trigger '{name}'"),
                range.clone(),
            );
        }
        // Event config args (`on Clock(enabled = ...)`) must be compile-time
        // constants: they bake into the event gate's data and have no wire pin.
        // Validated here — before the no-params early-return below, since a
        // config-only handler has no destructure params.
        if let Some(e) = evt {
            validate_handler_config(ctx, e, &h.config);
        }
        if h.params.is_empty() {
            return;
        }
        let Some(evt) = evt else {
            // Non-event trigger with `-> (…)`/`-> {…}` capture params — a
            // general mod/chip CALL trigger (`on pair(5, exec = go) -> (a, b)`).
            // Type each capture from the trigger call's OUTPUT RECORD instead of
            // `Any`, or the captured names read as `Any` and arithmetic on them
            // fails WS004. The record's single `Type::Exec` field is consumed by
            // `on` (it drives the body) and is EXCLUDED — the pattern binds the
            // remaining DATA fields.
            //
            // Record source: prefer the trigger binding's own type — the
            // synthesized `_on_expr_N` let binds the call's `Type::Record`
            // (declared before the handler, so it's in scope here). Only if that
            // isn't a record (auto-unwrapped to exec, or a folded `.field`
            // trigger) fall back to the trigger expr's recorded type; capture
            // params only ever come from `-> ` on a plain call, so the binding
            // source is the reliable one and the `type_of_expr` fallback avoids
            // the `.field`-trigger key overlap seen in lowering.
            let record_fields: Option<Vec<(String, Type)>> = match sym.as_ref().map(|s| &s.ty) {
                Some(Type::Record(fs)) => Some(fs.clone()),
                _ => {
                    let key = (range.file.clone(), range.start.offset, range.end.offset);
                    match ctx.type_of_expr.get(&key) {
                        Some(Type::Record(fs)) => Some(fs.clone()),
                        _ => None,
                    }
                }
            };
            let data_fields: Vec<(String, Type)> = record_fields
                .map(|fs| {
                    fs.into_iter()
                        .filter(|(_, t)| !matches!(t, Type::Exec))
                        .collect()
                })
                .unwrap_or_default();
            for (i, pname) in h.params.iter().enumerate() {
                // An explicit annotation wins; a record capture (`-> { f: a }`)
                // resolves by the original field name; a tuple capture
                // (`-> (a, b)`) is positional over the data fields; `Any` when
                // nothing resolves (a genuinely-untyped trigger — unchanged).
                let ty = if let Some(te) = &pname.ty {
                    let t = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &t, type_expr_range(te));
                    t
                } else if let Some(field) = &pname.source_field {
                    data_fields
                        .iter()
                        .find(|(n, _)| n == field)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                } else {
                    data_fields
                        .get(i)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                };
                ctx.scope.declare(
                    &pname.name,
                    SymbolInfo {
                        kind: SymbolKind::EventParam,
                        name: pname.name.clone(),
                        ty,
                        decl_range: h.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            return;
        };
        if evt.data.len() < h.params.len() {
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: expected {} param(s), got {}",
                    evt.data.len(),
                    h.params.len()
                ),
                h.range.clone(),
            );
        }
        for (i, pname) in h.params.iter().enumerate() {
            // A handler type annotation (`a: int` on Custom Event) overrides the
            // event's declared data type (which is `any` for such events).
            let ty = match &pname.ty {
                Some(te) => {
                    let t = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &t, type_expr_range(te));
                    t
                }
                None => {
                    // Custom Event's data outputs are untyped in the catalog, so an
                    // unannotated param has no declared type to fall back on. Look
                    // it up in `ce_slots` (Task 2's `infer_custom_event_slots`,
                    // keyed by this handler's source range): pass 2 has it resolved
                    // from an in-unit sender (or defaulted to float with a WS042
                    // warning already emitted by the inference pass); pass 1 (and
                    // any non-custom-event handler) has nothing there yet, so fall
                    // back to the event's declared data type (Any for untyped events).
                    let inferred = CeNamespace::from_event_name(evt.surface_name)
                        .and_then(|ns| ctx.ce_slots.get(&ce_slot_key(ns, h)))
                        .and_then(|v| v.get(i).cloned().flatten());
                    match inferred {
                        Some(t) => t,
                        None => evt.data.get(i).map(|d| d.ty.clone()).unwrap_or(Type::Any),
                    }
                }
            };
            ctx.scope.declare(
                &pname.name,
                SymbolInfo {
                    kind: SymbolKind::EventParam,
                    name: pname.name.clone(),
                    ty,
                    decl_range: h.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        return;
    }

    // TrigField: TODO, treat params as Any if any.
    #[allow(unreachable_code)]
    for pname in &h.params {
        ctx.scope.declare(
            &pname.name,
            SymbolInfo {
                kind: SymbolKind::EventParam,
                name: pname.name.clone(),
                ty: Type::Any,
                decl_range: h.range.clone(),
                signature: None,
                event_data: None,
            },
        );
    }
}

/// Collect names assigned as outputs anywhere in a block: `out name = expr`
/// bindings, `emit name (= expr)`, and bare `name = expr` assignments (an
/// over-approximation — variable assigns land here too — but this set only
/// suppresses the WS013 unassigned-output warning). Recurses into if blocks,
/// `on` handlers, and anonymous chip blocks; nested named chips own their
/// outputs and are skipped.
fn collect_output_assignments(block: &Block, assigned: &mut std::collections::HashSet<String>) {
    for s in &block.stmts {
        match s {
            Stmt::OutBinding(o) => {
                assigned.insert(o.name.clone());
            }
            Stmt::Emit(e) => {
                assigned.insert(e.name.clone());
            }
            Stmt::Assign(a) => {
                if let Expr::Ident { name, .. } = &a.target {
                    assigned.insert(name.clone());
                }
            }
            Stmt::If(i) => {
                collect_output_assignments(&i.then_block, assigned);
                if let Some(eb) = &i.else_block {
                    collect_output_assignments(eb, assigned);
                }
            }
            Stmt::Handler(h) => collect_output_assignments(&h.body, assigned),
            Stmt::AnonChip(ac) => collect_output_assignments(&ac.body, assigned),
            _ => {}
        }
    }
}

fn block_has_return_value(block: &Block) -> bool {
    for s in &block.stmts {
        match s {
            Stmt::Return { value: Some(_), .. } => return true,
            Stmt::If(i) => {
                if block_has_return_value(&i.then_block) {
                    return true;
                }
                if let Some(eb) = &i.else_block
                    && block_has_return_value(eb)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn check_block(
    ctx: &mut TypeCheckCtx,
    block: &Block,
) {
    ctx.push_scope();
    for s in &block.stmts {
        check_stmt(ctx, s);
    }
    ctx.pop_scope();
}

fn check_anon_chip_stmts(
    ctx: &mut TypeCheckCtx,
    stmts: &[Stmt],
    pre_registered: bool,
) {
    if !pre_registered {
        for s in stmts {
            match s {
                Stmt::Var(v) => register_decl(ctx, &TopDecl::Var(v.clone())),
                Stmt::Buffer(b) => register_decl(ctx, &TopDecl::Buffer(b.clone())),
                Stmt::Array(a) => register_decl(ctx, &TopDecl::Array(a.clone())),
                Stmt::In(i) => register_decl(ctx, &TopDecl::In(i.clone())),
                _ => {}
            }
        }
    }
    for s in stmts {
        // A chip's PURE statements (`let`/`var`/`out`/`buffer`/declarations) are
        // reactive signal flow — their var reads are continuous, so check them in
        // PURE context so `var_read_contexts` records them as pure (correct hover;
        // mirrors the lowering's per-statement purity in `lower_chip_body`). This
        // holds even when the chip as a whole is exec-wrapped (a top-level chip
        // after a handler), which keeps its EXEC statements (`if`, …) valid.
        if matches!(
            s,
            Stmt::Let(_)
                | Stmt::Var(_)
                | Stmt::Buffer(_)
                | Stmt::OutBinding(_)
                | Stmt::Array(_)
                | Stmt::In(_)
        ) {
            ctx.in_pure(|ctx| check_one_chip_stmt(ctx, s));
        } else {
            check_one_chip_stmt(ctx, s);
        }
    }
}

fn check_one_chip_stmt(ctx: &mut TypeCheckCtx, s: &Stmt) {
    match s {
        Stmt::Var(v) => check_decl(ctx, &TopDecl::Var(v.clone())),
        Stmt::Buffer(b) => check_decl(ctx, &TopDecl::Buffer(b.clone())),
        Stmt::Array(a) => check_decl(ctx, &TopDecl::Array(a.clone())),
        Stmt::In(i) => check_decl(ctx, &TopDecl::In(i.clone())),
        other => check_stmt(ctx, other),
    }
}

fn check_stmt(
    ctx: &mut TypeCheckCtx,
    s: &Stmt,
) {
    match s {
        Stmt::Var(v) => {
            register_decl(ctx, &TopDecl::Var(v.clone()));
            // Statement-level vars inherit the current exec context (not forced pure)
            // so that `var x: int = arr[i]` works inside handlers.
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                if let Some(entries) = map_init_entries(init)
                    && let Type::Map(k, v_ty) = &inner
                {
                    // A map-valued `var` initializer inside a handler: validate
                    // entries directly (the valid initializer slot) instead of
                    // routing through the generic `infer` position guard.
                    // An empty `{}` is an empty-map init.
                    check_map_literal(ctx, entries, k, v_ty);
                } else {
                    let t = infer::check(ctx, init, &inner);
                    // Refine an unannotated var's placeholder `any` from a
                    // non-literal init, same as the top-level decl path.
                    if v.typ.is_none() && matches!(inner, Type::Any) {
                        let u = unwrap_ref(&t);
                        if var_storable(&u) {
                            ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                        }
                    }
                }
            }
        }
        Stmt::Buffer(b) => {
            register_decl(ctx, &TopDecl::Buffer(b.clone()));
            check_decl(ctx, &TopDecl::Buffer(b.clone()));
        }
        Stmt::Array(a) => {
            register_decl(ctx, &TopDecl::Array(a.clone()));
            check_decl(ctx, &TopDecl::Array(a.clone()));
        }
        Stmt::Map(m) => {
            register_decl(ctx, &TopDecl::Map(m.clone()));
            check_decl(ctx, &TopDecl::Map(m.clone()));
        }
        Stmt::Let(l) => {
            let t = infer::infer(ctx, &l.value);
            check_let_type_annotation(ctx, l, &t);
            bind_let(ctx, &l.binding, &t);
            // A body-local `let name = <constant>` is recorded in the
            // innermost `scoped_consts` frame (mirroring how a top-level
            // `let` lands in `const_env`), so a constant-only config arg
            // elsewhere in this scope (or a nested one) can resolve `name`
            // via `ctx.const_lookup()`. Only the simple `Ident` binding form
            // can name a single constant — a destructured `let` can't.
            if let LetBinding::Ident { name, .. } = &l.binding
                && let Some(lit) = crate::lower::expr_to_literal_in(&l.value, &ctx.const_lookup())
                && let Some(frame) = ctx.scoped_consts.last_mut()
            {
                frame.insert(name.clone(), lit);
            }
        }
        Stmt::Assign(a) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    format!(
                        "var write '{}' outside an exec context",
                        target_name(&a.target).unwrap_or("<expr>".into())
                    ),
                    a.range.clone(),
                );
            }
            let target_ty = infer_assign_target(ctx, &a.target);
            if let Expr::MapLit { entries, .. } = &a.value
                && let Type::Map(k, v) = &target_ty
            {
                // Assigning a map literal to a map var: the other valid slot —
                // validate entries directly instead of routing through the
                // generic `infer` position guard.
                check_map_literal(ctx, entries, k, v);
            } else {
                infer::check(ctx, &a.value, &target_ty);
            }
        }
        Stmt::OutBinding(b) => {
            // An anon-chip's statement-level `out` carries a type annotation
            // too (`chip { @bottom out done: any = 5 }`); mirror the top-level
            // `TopDecl::Out` any-warn so `any` there is flagged like anywhere
            // else. (This path predates — and still lacks — the top-level
            // decl's WS003 value/annotation check; that gap is out of scope.)
            if let Some(te) = &b.typ {
                let resolved = resolve_type_expr(ctx, te);
                warn_any_annotation(ctx, &resolved, type_expr_range(te));
            }
            if let Some(value) = &b.value {
                infer::infer(ctx, value);
            }
        }
        Stmt::Emit(e) => {
            if e.value.is_none() && ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    format!("emit '{}' outside an exec context", e.name),
                    e.range.clone(),
                );
            }
            if let Some(ref val) = e.value {
                let t = infer::infer(ctx, val);
                // Remember the ferried payload type so a later
                // `let { .. } = await sig` can type its destructured fields.
                ctx.signal_payload_types.insert(e.name.clone(), t);
            }
        }
        Stmt::Await(a) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit("WS007", "await outside an exec context", a.range.clone());
            }
            // Push scope with `_` as Bool (the armed flag) for exec expression
            ctx.push_scope();
            ctx.scope.declare(
                "_",
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: "_".into(),
                    ty: Type::Bool,
                    decl_range: a.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
            let exec_ty = infer::infer(ctx, &a.exec_expr);
            ctx.pop_scope();
            let val_ty = if let Some(ref val) = a.value_expr {
                infer::infer(ctx, val)
            } else {
                exec_ty
            };
            if let Some(ref binding) = a.binding {
                ctx.scope.declare(
                    binding,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: binding.clone(),
                        ty: val_ty,
                        decl_range: a.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            // `let { a, b } = await sig`: type each destructured local from the
            // signal's recorded payload record (Any when unknown).
            if let Some(ref fields) = a.destructure {
                let payload_ty = match &a.exec_expr {
                    Expr::Ident { name, .. } => ctx.signal_payload_types.get(name).cloned(),
                    _ => None,
                };
                for (field, local) in fields {
                    let fty = match &payload_ty {
                        Some(Type::Record(fs)) => fs
                            .iter()
                            .find(|(n, _)| n == field)
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Type::Any),
                        _ => Type::Any,
                    };
                    ctx.scope.declare(
                        local,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: local.clone(),
                            ty: fty,
                            decl_range: a.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
            }
        }
        Stmt::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "'if' statement outside an exec context",
                    i.range.clone(),
                );
            }
            ctx.if_contexts.insert(
                (i.range.file.clone(), i.range.start.offset),
                ctx.exec_mode() == ExecMode::Exec,
            );
            infer::infer(ctx, &i.cond);
            check_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_block(ctx, else_b);
            }
        }
        Stmt::ExprStmt(es) => {
            infer::infer(ctx, &es.expr);
        }
        Stmt::In(i) => {
            register_decl(ctx, &TopDecl::In(i.clone()));
        }
        Stmt::Handler(h) => {
            ctx.push_scope();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body));
            ctx.pop_scope();
        }
        Stmt::AnonChip(ac) => {
            // Anon chip shares parent scope — register + check inline.
            check_anon_chip_stmts(ctx, &ac.body.stmts, false);
        }
        Stmt::ChipDecl(c) => {
            register_decl(ctx, &TopDecl::Chip(c.clone()));
            check_decl(ctx, &TopDecl::Chip(c.clone()));
        }
        Stmt::Return { value, range } => {
            if ctx.exec_mode() != ExecMode::Exec && value.is_none() {
                ctx.emit(
                    "WS007",
                    "'return' (without value) outside an exec context",
                    range.clone(),
                );
            }
            if let Some(expr) = value {
                infer::infer(ctx, expr);
            }
        }
    }
}

fn target_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// The element types of `t` viewed as a tuple. A tuple literal desugars to a
/// record keyed by element index, so `Record([("0", T0), ("1", T1)])` describes
/// the same shape as `Tuple([T0, T1])` and destructures the same way.
/// The result type of a call whose declared output is a union.
///
/// The math-variant gates (`Blend`/`lerp`/`Easing`) carry whichever variant
/// their inputs do, so a union output resolves to the **widening join**
/// (`crate::types::coerce::widening_join`) of every union-typed param's
/// argument type — the least upper bound, not just "first arg wins". Left as
/// the union, the result would satisfy no operator overload and every use of
/// it would fail. When only one operand is concrete, the join is just that
/// type (unchanged from the old behavior). When two operands have no common
/// widening (e.g. `Blend(vector, 1, t)`), that's a genuine incompatibility —
/// emit `WS033` (the same code the generic-mod inference solver uses for an
/// unwidenable conflict; this is the same kind of failure, just for a
/// builtin's dynamically-typed param instead of a user type parameter) and
/// fall back to the declared union. Any other output type is returned
/// unchanged.
fn union_output_type(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    out_index: usize,
    range: &SourceRange,
) -> Type {
    let declared = c.outputs[out_index].ty.clone();
    // The output rides the input variant when it is a union directly (Blend /
    // lerp / Easing) OR contains one as a Record FIELD (a stateful gate like
    // `Tween`, whose `{ Value: <variant>, Arrived: exec }` should give a float
    // `Value` for a float target, not the full `float|int|vector|…` union). Grab
    // that union's mask; nothing to resolve if there is no union.
    let mask: Vec<Type> = match &declared {
        Type::Union(m) => m.clone(),
        Type::Record(fs) => match fs.iter().find_map(|(_, t)| match t {
            Type::Union(m) => Some(m.clone()),
            _ => None,
        }) {
            Some(m) => m,
            None => return declared,
        },
        _ => return declared,
    };
    let mut joined: Option<Type> = None;
    for (i, p) in c.params.iter().enumerate() {
        if matches!(p.ty, Type::Union(_))
            && let Some(CallArg::Positional(e)) = args.get(i)
        {
            let t = unwrap_ref(&infer::infer(ctx, e));
            if matches!(t, Type::Any) {
                continue;
            }
            joined = Some(match joined {
                None => t,
                Some(prev) => match widening_join_all([prev.clone(), t.clone()]) {
                    Some(j) => j,
                    None => {
                        ctx.emit(
                            "WS033",
                            format!(
                                "'{}': incompatible operand types {} and {} — no common \
                                 widening (the math-variant params must all agree, up to \
                                 numeric/rotation widening)",
                                c.name,
                                crate::analysis::types::type_str(&prev),
                                crate::analysis::types::type_str(&t),
                            ),
                            range.clone(),
                        );
                        return declared;
                    }
                },
            });
        }
    }
    let resolved = match joined {
        // Bool never appears as a math-variant on its own (the mask is
        // Float/Int/Vector/Rotator/Quat/Color) — an all-bool fold widens one
        // step further to Int, the mask's narrowest numeric member.
        Some(Type::Bool) if !mask_contains(&mask, &Type::Bool) => Type::Int,
        Some(t) => t,
        None => return declared,
    };
    // Apply the resolved variant: replace the bare union, or each union-typed
    // field of the Record output (leaving `Arrived: exec` and the like intact).
    match declared {
        Type::Union(_) => resolved,
        Type::Record(fields) => Type::Record(
            fields
                .into_iter()
                .map(|(k, ft)| {
                    let nt = if matches!(ft, Type::Union(_)) { resolved.clone() } else { ft };
                    (k, nt)
                })
                .collect(),
        ),
        other => other,
    }
}

/// The result type of a call with at least one declared output — shared by
/// the builtin and receiver call arms. A single output widens directly via
/// `union_output_type`; multiple outputs each widen independently (per their
/// own `out_index`) and assemble into a field-keyed record, so a multi-output
/// gate whose field rides the math variant (like a single-output `Blend`)
/// still resolves to the argument type instead of the declared union. Callers
/// with a zero-output `CallSpec` handle that fallback themselves (the
/// builtin and receiver arms differ there) and never call this helper.
fn output_record_type(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    range: &SourceRange,
) -> Type {
    if c.outputs.len() == 1 {
        return union_output_type(ctx, c, args, 0, range);
    }
    Type::Record(
        c.outputs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                (
                    o.field.unwrap_or(o.port.as_str()).to_string(),
                    union_output_type(ctx, c, args, i, range),
                )
            })
            .collect(),
    )
}

fn as_tuple_fields(t: &Type) -> Option<Vec<Type>> {
    match t {
        Type::Tuple(fields) => Some(fields.clone()),
        Type::Record(fields) => fields
            .iter()
            .enumerate()
            .map(|(i, (key, ft))| (*key == i.to_string()).then(|| ft.clone()))
            .collect(),
        _ => None,
    }
}

fn bind_let(ctx: &mut TypeCheckCtx, b: &LetBinding, t: &Type) {
    match b {
        LetBinding::Ident { name, range } => {
            ctx.scope.declare(
                name,
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: name.clone(),
                    ty: t.clone(),
                    decl_range: range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        LetBinding::Tuple { names, range, .. } => {
            if let Some(fields) = as_tuple_fields(t)
                && fields.len() == names.len()
            {
                for (n, ft) in names.iter().zip(fields.iter()) {
                    ctx.scope.declare(
                        n,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: n.clone(),
                            ty: ft.clone(),
                            decl_range: range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                return;
            }
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: expected tuple[{}], got {:?}",
                    names.len(),
                    t
                ),
                range.clone(),
            );
            for n in names {
                ctx.scope.declare(
                    n,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: n.clone(),
                        ty: Type::Any,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        LetBinding::Record { names, range } => {
            for n in names {
                let ty = if let Type::Record(fields) = t {
                    fields
                        .iter()
                        .find(|(k, _)| k == n)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                } else {
                    Type::Any
                };
                ctx.scope.declare(
                    n,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: n.clone(),
                        ty,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
        LetBinding::RecordDestruct { fields, range } => {
            for field in fields {
                let (name, ty) = match field {
                    crate::ast::RecordDestructField::Named { name, alias, .. } => {
                        let bind_name = alias.as_ref().unwrap_or(name);
                        let field_ty = if let Type::Record(rec_fields) = t {
                            rec_fields
                                .iter()
                                .find(|(k, _)| k == name)
                                .map(|(_, t)| t.clone())
                                .unwrap_or(Type::Any)
                        } else {
                            Type::Any
                        };
                        (bind_name.clone(), field_ty)
                    }
                    crate::ast::RecordDestructField::Rest { name, .. } => {
                        // Rest collects remaining fields into a new record
                        (name.clone(), Type::Any)
                    }
                };
                ctx.scope.declare(
                    &name,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: name.clone(),
                        ty,
                        decl_range: range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
        }
    }
}

/// Non-emitting predicate mirroring `infer_assign_target`'s accepted
/// writable-target shapes, used to validate `&`/`ref` operands (`WS008`)
/// without recording a diagnostic itself — the caller decides what to emit.
pub(crate) fn is_ref_able(ctx: &TypeCheckCtx, e: &Expr) -> bool {
    match e {
        Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
            Some(s) => matches!(
                s.kind,
                SymbolKind::Var | SymbolKind::Array | SymbolKind::Map | SymbolKind::LetBinding
            ) || (s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)))
                || (s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_))),
            None => false,
        },
        Expr::IndexAccess { obj, .. } => is_ref_able(ctx, obj),
        _ => false,
    }
}

fn infer_assign_target(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
) -> Type {
    if let Expr::Ident { name, range } = e {
        if name == "_" {
            return Type::Any;
        }
        let sym = ctx.scope.lookup(name).cloned();
        match sym {
            None => {
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::Var => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Array => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Map => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::LetBinding => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)) => {
                unwrap_ref(&s.ty)
            }
            Some(s) if s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_)) => {
                unwrap_ref(&s.ty)
            }
            _ => {
                ctx.emit(
                    "WS007",
                    format!("'{name}' isn't a writable target"),
                    range.clone(),
                );
                Type::Any
            }
        }
    } else if let Expr::IndexAccess { obj, index, .. } = e {
        let obj_ty = infer_assign_target(ctx, obj);
        infer::infer(ctx, index);
        match obj_ty {
            Type::Array(inner) => *inner,
            _ => Type::Any,
        }
    } else {
        Type::Any
    }
}

/// Type a resolved user `mod`/`chip` call from its already-inferred positional
/// argument types. Shared by the plain-identifier call path and the
/// `self`-receiver method path (which prepends the receiver as positional
/// arg 0). Emits WS021 (use-before-declaration), WS022 (argument count) and
/// WS033 (generic inference), then returns the call's result type — the
/// (possibly monomorphized) single output, a record of the outputs, or `any`.
///
/// `positional_count` and `positional_arg_types` must already include the
/// receiver for a method call. `name_range` anchors the count / decl-order
/// diagnostics; `call_range` anchors the generic-inference one. `args` feeds
/// the `sig::check_args` arg-coercion pass — it must be the FULL `CallArg`
/// list, receiver included as its own leading `CallArg::Positional` for a
/// method call (mirroring `positional_arg_types`).
#[allow(clippy::too_many_arguments)]
fn type_user_symbol_call(
    ctx: &mut TypeCheckCtx,
    name: &str,
    sym: &SymbolInfo,
    positional_arg_types: &[Type],
    args: &[CallArg],
    type_args: &[TypeExpr],
    positional_count: usize,
    has_spread: bool,
    has_exec_arg: bool,
    call_range: &SourceRange,
    name_range: &SourceRange,
) -> Type {
    // Use-before-declaration. Chips/mods are registered in source order during
    // lowering, so a call whose declaration lexically follows the call site
    // cannot resolve — it would synthesise an `_Unsupported` gate that silently
    // reads 0 at runtime. Only applies to same-file chip/mod decls (imports
    // live elsewhere and are always available).
    if sym.kind == SymbolKind::Chip
        && sym.signature.is_some()
        && sym.decl_range.file == name_range.file
        && (name_range.start.line, name_range.start.col)
            < (sym.decl_range.start.line, sym.decl_range.start.col)
    {
        ctx.emit(
            "WS021",
            format!(
                "call to `{name}` before its declaration — chips and \
                 mods must be declared before the point where they \
                 are used (move the declaration above its first caller)"
            ),
            name_range.clone(),
        );
    }
    // Argument-count check. User chips/mods/fns have no default parameters, so
    // the positional-argument count must equal the parameter count — each param
    // (including a whole-record or destructured one, and the `self` receiver)
    // takes exactly one positional arg. A spread makes the count dynamic, so
    // skip the check then. A mismatch would otherwise leave a param unbound,
    // silently reading 0 / an empty value.
    if let Some(sig) = &sym.signature
        && !has_spread
    {
        let expected = sig.params.len();
        if positional_count != expected {
            ctx.emit(
                "WS022",
                format!(
                    "`{name}` expects {expected} argument{} but {positional_count} {} given",
                    if expected == 1 { "" } else { "s" },
                    if positional_count == 1 { "was" } else { "were" },
                ),
                name_range.clone(),
            );
        }
    }
    let Some(sig) = &sym.signature else {
        // The callee resolved to a non-callable symbol — a var / let / array /
        // buffer / input / param, not a mod, chip, or function. Without this the
        // call typed as `any` with no diagnostic and lowering emitted an
        // `_Unsupported` gate that reads 0 — a silent miscompile. Common causes:
        // an index typo (`xs(i)` for `xs[i]`) and a chained comparison the
        // parser reads as an explicit-type-argument call (`a < b > (c)`).
        ctx.emit(
            "WS038",
            format!("`{name}` is not callable — only mods, chips, and functions can be called"),
            name_range.clone(),
        );
        return Type::Any;
    };
    // Arg-driven inference for a generic mod/chip call: collect an equality
    // constraint from every (declared param type, inferred arg type) pair, solve
    // for each type param, and substitute the result into the output types
    // below. Guarded on `type_params` being non-empty so a non-generic call
    // takes exactly the pre-generics path — `subst` stays `None` and `out_ty`
    // is a plain clone.
    let subst: Option<crate::types::infer::Subst> = if sig.type_params.is_empty() {
        if !type_args.is_empty() {
            ctx.emit(
                "WS033",
                format!("`{name}` is not generic — it takes no type arguments"),
                call_range.clone(),
            );
        }
        None
    } else if !type_args.is_empty() {
        // Explicit type arguments `f<int>(...)`: the caller pinned each type
        // param. Bind them directly (skipping arg-driven inference), validating
        // arity and each arg against its param's bound mask. This is the ONLY
        // way to pin a `T` that appears only in the return type (`make<int>()`),
        // which inference can't derive from the arguments.
        if type_args.len() != sig.type_params.len() {
            ctx.emit(
                "WS033",
                format!(
                    "`{name}` expects {} type argument{}, but {} {} given",
                    sig.type_params.len(),
                    if sig.type_params.len() == 1 { "" } else { "s" },
                    type_args.len(),
                    if type_args.len() == 1 { "was" } else { "were" },
                ),
                call_range.clone(),
            );
            None
        } else {
            let mut s = crate::types::infer::Subst::new();
            for ((pname, mask), te) in sig.type_params.iter().zip(type_args.iter()) {
                let ty = resolve_type_expr(ctx, te);
                if !crate::types::classes::mask_contains(mask, &ty) {
                    ctx.emit(
                        "WS033",
                        format!(
                            "`{pname}` = {}, which isn't allowed by its bound",
                            crate::analysis::types::type_str(&ty),
                        ),
                        te.range().clone(),
                    );
                }
                s.insert(pname.clone(), ty);
            }
            Some(s)
        }
    } else {
        // Ref-align param and arg before collecting. A `*T` param resolves to
        // `Ref(Param(T))`, but a var passed to it infers to its already-auto-
        // derefed inner type (`Int`, not `Ref(Int)`), so the Ref layers are
        // asymmetric: strip a leading Ref off BOTH so `*T` vs `int` collects
        // as `Param(T)` vs `int` → `Eq(T, int)`, exactly like a value `T`.
        let param_types: Vec<Type> = sig.params.iter().map(|p| p.ty.clone()).collect();
        let constraints = crate::types::mono::call_constraints(&param_types, positional_arg_types);
        match crate::types::infer::solve(&constraints, &sig.type_params) {
            Ok(s) => Some(s),
            Err(e) => {
                let msg = match &e {
                    crate::types::infer::InferError::Conflict { var, a, b } => format!(
                        "cannot infer '{var}': it's {} from one argument but {} from another — all '{var}' arguments must be the same type",
                        crate::analysis::types::type_str(a),
                        crate::analysis::types::type_str(b),
                    ),
                    crate::types::infer::InferError::Unpinnable(var) => format!(
                        "cannot infer type parameter '{var}' — annotate the argument(s)"
                    ),
                    crate::types::infer::InferError::OutOfMask { var, ty, .. } => format!(
                        "'{var}' = {}, which isn't allowed by its bound",
                        crate::analysis::types::type_str(ty),
                    ),
                };
                ctx.emit("WS033", msg, call_range.clone());
                None
            }
        }
    };
    // Validate each argument against its (substituted) parameter type — the
    // same coercion the wire layer applies (`PortsAreCompatible`), now routed
    // through the shared `sig::check_args` (arity already checked above as
    // WS022, so `check_arity = false` here). User mod/chip calls previously
    // skipped this entirely, so `f(int)` on a `vector` param — and a receiver
    // call `x.m()` whose `x`'s type doesn't match `self` — passed clean and
    // then miscompiled at the wire level. Skipped only for a spread (variable
    // positional count, nothing to line up positionally); `check_args`'s own
    // `Wire`-arm coerce already treats `Any`/`Opaque` args as always-`Same`
    // and skips a still-generic (`Type::Param`-carrying) param — the latter
    // left to the WS033 inference diagnostics above.
    if !has_spread {
        check_args(
            ctx,
            &sig_of_fnchip(name, sig, subst.as_ref()),
            args,
            0,
            /* check_arity */ false,
            call_range,
        );
    }
    let out_ty = |t: &Type| match &subst {
        Some(s) => substitute(t, s),
        None => t.clone(),
    };
    // A call with an `exec =` trigger also returns the chip's completion exec as
    // an `exec` field (unless the chip declares its own `exec` output).
    if sig.outputs.len() == 1 && !has_exec_arg {
        return out_ty(&sig.outputs[0].ty);
    }
    if !sig.outputs.is_empty() {
        let mut fields: Vec<(String, Type)> = sig
            .outputs
            .iter()
            .map(|o| (o.name.clone(), out_ty(&o.ty)))
            .collect();
        if has_exec_arg && !fields.iter().any(|(n, _)| n == "exec") {
            fields.push(("exec".into(), Type::Exec));
        }
        return Type::Record(fields);
    }
    Type::Any
}

/// Reference-only types: like a variable ref, these wire and reroute but are not
/// values — they can't be selected, stored, or operated on. Covers the explicit
/// `ref T` var ref plus the opaque `zone`/`teleport` component references and
/// the compile-time-constant `prefab` reference.
fn is_reference_type(t: &Type) -> bool {
    matches!(t, Type::Ref(_) | Type::Zone | Type::Teleport | Type::PrefabRef)
}

// ---------- generic mod/chip call-site inference ----------
//
// Arg-driven inference at a call to a generic `mod`/`chip`: `mono::call_constraints`
// walks each (declared param type, inferred arg type) pair in lockstep and
// records an equality `Constraint` everywhere the param side names a
// `Type::Param`. `types::infer::solve` turns the collected constraints into a
// `Subst`; `mono::substitute` then replaces every `Type::Param` in the
// signature's output type with its solved binding. The
// `call_constraints`/`substitute`/`mask_for_param` helpers live in the shared
// `types::mono` module so the lowering-side monomorphizer reuses the
// exact same inference at each generic-mod inline site.
use crate::types::mono::{substitute, unwrap_ref};

// ---------- custom-event sender/receiver type consistency (WS030) ----------

/// The wire-variant class a custom-event value takes on the wire. Two types in
/// the same class transfer identically (a `character` and an `entity` are both
/// the `Object` variant), so only a cross-class disagreement is a real
/// mismatch. Mirrors `emit::var_type_to_wire_variant`'s grouping. `None` means
/// "unclassifiable" (`any`/`exec`/records/…) — never linted, either side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WireClass {
    Bool,
    Int,
    Number,
    Str,
    Object,
    Vector,
    Rotator,
    Quat,
    Color,
}

fn wire_class(t: &Type) -> Option<WireClass> {
    Some(match t {
        Type::Bool => WireClass::Bool,
        Type::Int => WireClass::Int,
        Type::Float => WireClass::Number,
        Type::String => WireClass::Str,
        Type::Vector => WireClass::Vector,
        Type::Rotator => WireClass::Rotator,
        Type::Quat => WireClass::Quat,
        Type::Color => WireClass::Color,
        Type::Controller | Type::Character | Type::Entity => {
            WireClass::Object
        }
        _ => return None,
    })
}

/// Resolve a Custom Event param annotation to its type without emitting — the
/// handler-binding pass already reported any bad annotation, so this must not
/// double-report. Custom-event data is always a wire variant (a primitive), so
/// anything more exotic resolves to `any` and simply isn't linted. Delegates
/// to the crate's single canonical resolver (`types::resolve::resolve_type`);
/// any diagnostic it would emit is discarded per the no-double-report rule
/// above.
fn ce_param_type(te: &TypeExpr) -> Type {
    let cx = crate::types::resolve::ResolveCtx {
        params: &[],
        type_aliases: &HashMap::default(),
        generic_aliases: &HashMap::default(),
    };
    crate::types::resolve::resolve_type(te, &cx, &mut Vec::new())
}

/// `(channel name, per-slot declared type)` for a handler that is a custom-event
/// receiver of kind `event_name` (`"CustomEvent"` or `"GlobalCustomEvent"`) with
/// a literal channel name. `None` for any other handler (or a dynamic channel
/// name — nothing to key receivers by). An unannotated slot falls back to
/// `ce_slots`' inferred type for this handler (keyed by its source range) when
/// present — pass 1 has no inference available yet, so it passes an empty map.
fn ce_receiver_of(
    h: &Handler,
    event_name: &str,
    ce_slots: &CeSlotMap,
) -> Option<(String, Vec<Option<Type>>)> {
    if !matches!(&h.trigger, Trigger::Ident { name, .. } if name == event_name) {
        return None;
    }
    let name = h.config.iter().find_map(|c| match c {
        HandlerConfigArg::Positional(Expr::StringLit { value, .. }) => Some(value.clone()),
        _ => None,
    })?;
    let ns = CeNamespace::from_event_name(event_name)?;
    let inferred = ce_slots.get(&ce_slot_key(ns, h));
    let params = h
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| match p.ty.as_ref() {
            Some(te) => Some(ce_param_type(te)),
            // Fill from inference when available (pass 2); else unresolved.
            None => inferred.and_then(|v| v.get(i).cloned().flatten()),
        })
        .collect();
    Some((name, params))
}

/// Fold a receiver's declared params into the per-channel signature, keeping the
/// first classifiable type seen at each slot. Multiple receivers for one channel
/// are allowed; they define the same wire, so the first concrete type wins.
fn ce_merge_receiver(
    map: &mut HashMap<String, Vec<Option<Type>>>,
    name: String,
    params: Vec<Option<Type>>,
) {
    let slots = map.entry(name).or_default();
    for (i, ty) in params.into_iter().enumerate() {
        if slots.len() <= i {
            slots.resize(i + 1, None);
        }
        if slots[i].is_none()
            && let Some(t) = ty
            && wire_class(&t).is_some()
        {
            slots[i] = Some(t);
        }
    }
}

/// The literal channel name a `SendCustomEvent` call targets, or `None` when it
/// is passed dynamically (a variable/interpolation) — dynamic sends can reach
/// any receiver, so they are never linted.
fn ce_send_event_name(args: &[CallArg]) -> Option<String> {
    // A named `eventName = …` is the channel if present; otherwise the first
    // positional arg is.
    for a in args {
        if let CallArg::Named { name, value, .. } = a
            && name == "eventName"
        {
            return match value {
                Expr::StringLit { value, .. } => Some(value.clone()),
                _ => None,
            };
        }
    }
    match args.iter().find(|a| matches!(a, CallArg::Positional(_))) {
        Some(CallArg::Positional(Expr::StringLit { value, .. })) => Some(value.clone()),
        _ => None,
    }
}

/// Map a `SendCustomEvent` call's data args to `(0-based slot, value expr)`.
/// Positional arg 0 is the channel name (unless it was named), so positional
/// data starts at slot 0 from the *second* positional; `dataN` names slot N-1.
fn ce_send_data_args(args: &[CallArg]) -> Vec<(usize, &Expr)> {
    let name_is_named = args
        .iter()
        .any(|a| matches!(a, CallArg::Named { name, .. } if name == "eventName"));
    let mut out = Vec::new();
    let mut pos = 0usize;
    for a in args {
        match a {
            CallArg::Positional(e) => {
                let slot = if name_is_named {
                    Some(pos)
                } else if pos == 0 {
                    None // the channel name
                } else {
                    Some(pos - 1)
                };
                if let Some(s) = slot {
                    out.push((s, e));
                }
                pos += 1;
            }
            CallArg::Named { name, value, .. } => {
                if let Some(n) = name
                    .strip_prefix("data")
                    .and_then(|d| d.parse::<usize>().ok())
                    && n >= 1
                {
                    out.push((n - 1, value));
                }
            }
            CallArg::Spread(_) => {}
        }
    }
    out
}

fn check_custom_event_types(
    ctx: &mut TypeCheckCtx,
    script: &Script,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
    ce_slots: &CeSlotMap,
) {
    // Personal and Global custom events are SEPARATE channel namespaces: a
    // `SendCustomEvent("x")` only reaches `on CustomEvent("x")`, and a
    // `SendGlobalCustomEvent("x")` only reaches `on GlobalCustomEvent("x")`. Gather
    // each namespace's receivers and senders in one AST walk, then type-check the
    // send/receive pairs within each namespace independently.
    let mut personal_recv: HashMap<String, Vec<Option<Type>>> = HashMap::default();
    let mut global_recv: HashMap<String, Vec<Option<Type>>> = HashMap::default();
    let mut personal_send: Vec<&Expr> = Vec::new();
    let mut global_send: Vec<&Expr> = Vec::new();
    crate::analysis::visit_program(
        script,
        &mut |h| {
            if let Some((name, params)) = ce_receiver_of(h, "CustomEvent", ce_slots) {
                ce_merge_receiver(&mut personal_recv, name, params);
            } else if let Some((name, params)) = ce_receiver_of(h, "GlobalCustomEvent", ce_slots) {
                ce_merge_receiver(&mut global_recv, name, params);
            }
        },
        &mut |call| {
            if let Expr::Call { callee, .. } = call {
                // The channel name + data args live in `call`'s positional args in
                // both the plain `SendCustomEvent("x", …)` and the receiver form
                // `entity.SendCustomEvent("x", …)` (the receiver is separate), so
                // both forms are collected the same way.
                let callee_name = match callee.as_ref() {
                    Expr::Ident { name, .. } => Some(name.as_str()),
                    Expr::FieldAccess { field, .. } => Some(field.as_str()),
                    _ => None,
                };
                match callee_name {
                    Some("SendCustomEvent") => personal_send.push(call),
                    Some("SendGlobalCustomEvent") => global_send.push(call),
                    _ => {}
                }
            }
        },
    );
    check_ce_namespace(ctx, &personal_send, &personal_recv, tmap, "SendCustomEvent", "CustomEvent");
    check_ce_namespace(
        ctx,
        &global_send,
        &global_recv,
        tmap,
        "SendGlobalCustomEvent",
        "GlobalCustomEvent",
    );
}

/// Within ONE custom-event namespace, warn (WS030) when a send's data value type
/// disagrees with the receiver's declared param type for the same channel — the
/// game keys the wire variant off the sender, so a mismatch is a real bug.
fn check_ce_namespace(
    ctx: &mut TypeCheckCtx,
    senders: &[&Expr],
    receivers: &HashMap<String, Vec<Option<Type>>>,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
    send_name: &str,
    event_name: &str,
) {
    for call in senders {
        let Expr::Call { args, .. } = call else {
            continue;
        };
        let Some(name) = ce_send_event_name(args) else {
            continue; // dynamic channel name — not linted
        };
        let Some(slots) = receivers.get(&name) else {
            continue; // no in-unit receiver to compare against
        };
        for (slot, expr) in ce_send_data_args(args) {
            let Some(recv_ty) = slots.get(slot).and_then(|o| o.as_ref()) else {
                continue;
            };
            let Some(recv_class) = wire_class(recv_ty) else {
                continue;
            };
            let r = expr.range();
            let Some(send_ty) = tmap.get(&(r.file.clone(), r.start.offset, r.end.offset)) else {
                continue;
            };
            let send_ty = unwrap_ref(send_ty);
            let Some(send_class) = wire_class(&send_ty) else {
                continue;
            };
            if send_class != recv_class {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS030".into(),
                    message: format!(
                        "custom event '{name}' data #{} is {}, but the `on {event_name}(\"{name}\", …)` \
                         receiver declares {} — {send_name} values must match the receiver's param types",
                        slot + 1,
                        send_ty,
                        recv_ty,
                    ),
                    range: r.clone(),
                });
            }
        }
    }
}

/// Resolve every UNANNOTATED custom-event receiver slot to a concrete type from
/// the matching in-unit sender, or `float` (with a WS042 warning) when none is
/// inferable. Returns the map (keyed by handler range) plus the WS042
/// diagnostics. Reads sender arg types from `tmap` (the pass-1 `type_of_expr`).
///
/// Handles both namespaces (`CustomEvent`/`SendCustomEvent` and
/// `GlobalCustomEvent`/`SendGlobalCustomEvent`), keeping them separate exactly
/// as `check_custom_event_types` does — a personal send must never resolve a
/// global receiver's slot, or vice versa.
fn infer_custom_event_slots(
    ast: &Script,
    tmap: &HashMap<(Arc<str>, usize, usize), Type>,
) -> (CeSlotMap, Vec<Diagnostic>) {
    // 1. Collect senders → channel → slot → first concrete wire-typed value.
    //    (personal vs global namespaces, mirroring check_custom_event_types.)
    let mut personal: HashMap<String, HashMap<usize, Type>> = HashMap::default();
    let mut global: HashMap<String, HashMap<usize, Type>> = HashMap::default();
    let mut receivers: Vec<(&Handler, &'static str)> = Vec::new();
    crate::analysis::visit_program(
        ast,
        &mut |h| {
            if matches!(&h.trigger, Trigger::Ident { name, .. } if name == "CustomEvent") {
                receivers.push((h, "CustomEvent"));
            } else if matches!(&h.trigger, Trigger::Ident { name, .. } if name == "GlobalCustomEvent")
            {
                receivers.push((h, "GlobalCustomEvent"));
            }
        },
        &mut |call| {
            if let Expr::Call { callee, args, .. } = call {
                let callee_name = match callee.as_ref() {
                    Expr::Ident { name, .. } => Some(name.as_str()),
                    Expr::FieldAccess { field, .. } => Some(field.as_str()),
                    _ => None,
                };
                let bucket = match callee_name {
                    Some("SendCustomEvent") => Some(&mut personal),
                    Some("SendGlobalCustomEvent") => Some(&mut global),
                    _ => None,
                };
                if let Some(bucket) = bucket
                    && let Some(chan) = ce_send_event_name(args)
                {
                    let slots = bucket.entry(chan).or_default();
                    for (slot, expr) in ce_send_data_args(args) {
                        let r = expr.range();
                        if let Some(t) = tmap.get(&(r.file.clone(), r.start.offset, r.end.offset))
                        {
                            let t = unwrap_ref(t);
                            if wire_class(&t).is_some() {
                                slots.entry(slot).or_insert(t); // first sender wins
                            }
                        }
                    }
                }
            }
        },
    );

    // 2. Resolve each receiver's UNANNOTATED slots.
    let mut map = CeSlotMap::default();
    let mut diags = Vec::new();
    for (h, event_name) in receivers {
        let Some(ns) = CeNamespace::from_event_name(event_name) else {
            continue;
        };
        let bucket = match ns {
            CeNamespace::Personal => &personal,
            CeNamespace::Global => &global,
        };
        // Only constant-channel receivers can key against senders.
        let chan = h.config.iter().find_map(|c| match c {
            HandlerConfigArg::Positional(Expr::StringLit { value, .. }) => Some(value.clone()),
            _ => None,
        });
        let has_unannotated = h.params.iter().any(|p| p.ty.is_none());
        if !has_unannotated {
            continue;
        }
        let key = ce_slot_key(ns, h);
        let slots_out = map.entry(key).or_insert_with(|| vec![None; h.params.len()]);
        for (i, p) in h.params.iter().enumerate() {
            if p.ty.is_some() {
                continue; // annotated: nothing to infer
            }
            let inferred = chan.as_ref().and_then(|c| bucket.get(c)).and_then(|m| m.get(&i)).cloned();
            match inferred {
                Some(t) => slots_out[i] = Some(t),
                None => {
                    slots_out[i] = Some(Type::Float);
                    // No WS042 for a dynamic (non-constant) channel — nothing to key on.
                    if chan.is_some() {
                        diags.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS042".into(),
                            message: format!(
                                "custom event '{}' data param '{}' (#{}): no in-unit sender \
                                 to infer its type from; defaulting to float — annotate \
                                 `{}: <type>` to silence",
                                chan.as_deref().unwrap_or(""),
                                p.name,
                                i + 1,
                                p.name,
                            ),
                            range: p.range.clone(),
                        });
                    }
                }
            }
        }
    }
    (map, diags)
}

/// Operand type for operator-overload resolution: unwrap a `ref`, then collapse
/// a multi-output gate result (`Record`) to its PRIMARY (first) field. A gate
/// like `ParseInt`/`GetDamage` exposes `{ Value, Success }` / `{ Damage,
/// DamageLimit }`, whose first field is the value the call "is" — so `ParseInt(s)
/// == n` compares against that value, mirroring the record auto-unwrap the
/// coercion layer already does for assignments and call arguments.
fn op_operand_type(t: &Type) -> Type {
    match unwrap_ref(t) {
        Type::Record(fields) if !fields.is_empty() => op_operand_type(&fields[0].1),
        other => other,
    }
}

/// The schema enum type of a config (non-wire, data-only) param, if any.
/// A param is data-only when its port is not a wire input on the gate; such a
/// param backed by a schema enum field takes a bare member name or an int.
fn call_param_config_enum(
    spec: &crate::catalog::calls::CallSpec,
    param: &crate::catalog::calls::CallParam,
) -> Option<&'static str> {
    let port = param.port.as_str();
    if crate::catalog::is_wire_input(spec.gate_class, port) {
        return None;
    }
    crate::catalog::config_field_enum_type(spec.gate_class, port)
}

/// Validate an argument bound to an enum-typed config param against the
/// schema's member list. A bare identifier is an enum member (not a variable);
/// an int is range-checked; anything else is rejected (config is constant-only).
fn validate_enum_config_arg(ctx: &mut TypeCheckCtx, enum_type: &str, e: &Expr) {
    match e {
        Expr::Ident { name, range } => {
            if crate::catalog::enum_member_value(enum_type, name).is_none() {
                let members = crate::catalog::enum_member_names(enum_type).join(", ");
                ctx.emit(
                    "WS028",
                    format!(
                        "unknown enum member '{name}' for {enum_type}; expected one of: {members}"
                    ),
                    range.clone(),
                );
            }
        }
        Expr::IntLit { value, range, .. } => {
            if !crate::catalog::enum_has_value(enum_type, *value) {
                let members = crate::catalog::enum_member_names(enum_type).join(", ");
                ctx.emit(
                    "WS028",
                    format!("{value} is not a valid {enum_type} value; expected one of: {members}"),
                    range.clone(),
                );
            }
        }
        // The quoted-name form (`function = "Bounce"`) — validate the same way.
        Expr::StringLit { value, range } => {
            if crate::catalog::enum_member_value(enum_type, value).is_none() {
                let members = crate::catalog::enum_member_names(enum_type).join(", ");
                ctx.emit(
                    "WS028",
                    format!("unknown enum member \"{value}\" for {enum_type}; expected one of: {members}"),
                    range.clone(),
                );
            }
        }
        other => {
            ctx.emit(
                "WS028",
                format!("{enum_type} config must be a constant enum member name or int"),
                other.range().clone(),
            );
        }
    }
}

/// Composite constant-only config params (`meshColors: Color[]`,
/// `ammoOverride`: the `WeaponAmmoOverride` nested struct) that fold into gate
/// data rather than wiring. `true` when this param is one and targets a
/// non-wire data field on its gate.
fn is_composite_config_param(
    spec: &crate::catalog::calls::CallSpec,
    param: &crate::catalog::calls::CallParam,
) -> bool {
    let port = param.port.as_str();
    matches!(port, "MeshColors" | "WeaponAmmoOverride")
        && !crate::catalog::is_wire_input(spec.gate_class, port)
}

/// Validate a composite constant-only config argument. It must fold to a
/// constant of the expected shape (the same fold the lowering uses); a
/// non-constant or malformed value is rejected here rather than becoming a
/// silent broken gate.
///
/// Takes `gate_class` + `display_name` + `port_name` (rather than a
/// `&CallParam`) so the `sig::check_args` port can call it from a bare `Param`
/// (which doesn't carry a `CallParam::port`). `port_name` — the gate PORT name
/// — keys the composite-shape lookup ("MeshColors"/"WeaponAmmoOverride");
/// `display_name` — the source-level surface name — is what the WS028 message
/// shows the author (these differ, e.g. surface `meshColors` binds the
/// `MeshColors` port). `gate_class` isn't used by the composite check itself
/// (kept for signature symmetry with `validate_data_driven_config`).
fn validate_composite_config_arg(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    display_name: &str,
    port_name: &str,
    e: &Expr,
) {
    let _ = gate_class;
    let (ok, hint) = match port_name {
        "MeshColors" => (
            crate::lower::fold_mesh_colors(e).is_some(),
            "a constant array of ColorSRGB(r, g, b, a) values",
        ),
        "WeaponAmmoOverride" => (
            crate::lower::fold_ammo_override(e).is_some(),
            "a constant record { overrideStartingAmmo: bool, resources: [{ loaded: int, reserve: int }] }",
        ),
        _ => (true, ""),
    };
    if !ok {
        ctx.emit(
            "WS028",
            format!("'{display_name}' config must be {hint}"),
            e.range().clone(),
        );
    }
}

/// A plain scalar/asset config param — a non-wire settings-menu field that is
/// neither enum-typed nor a composite (meshColors/ammoOverride). Its value
/// bakes into the gate's data and cannot be wired, so it must be a constant.
fn is_scalar_config_param(
    spec: &crate::catalog::calls::CallSpec,
    param: &crate::catalog::calls::CallParam,
) -> bool {
    !crate::catalog::is_wire_input(spec.gate_class, param.port.as_str())
        && call_param_config_enum(spec, param).is_none()
        && !is_composite_config_param(spec, param)
}

/// Adapt a builtin/receiver `CallSpec` into the generic `CallSignature` shape
/// `sig::check_args` validates against — the bridge that lets both call forms
/// route through the one arg checker. Each `CallParam` maps to a `Param` whose
/// `ParamKind` mirrors exactly the per-param branch the arg checker takes (enum
/// config / composite config / scalar config / ordinary wire), keyed off the
/// gate PORT name so the config validators' schema lookups are unchanged.
fn sig_of_callspec(spec: &crate::catalog::calls::CallSpec) -> CallSignature {
    let params = spec
        .params
        .iter()
        .map(|p| {
            let kind = if let Some(et) = call_param_config_enum(spec, p) {
                ParamKind::ConfigEnum(et)
            } else if is_composite_config_param(spec, p) {
                ParamKind::ConfigComposite(p.port.as_str())
            } else if is_scalar_config_param(spec, p) {
                ParamKind::ConfigScalar(p.port.as_str())
            } else {
                ParamKind::Wire
            };
            Param {
                name: p.name.to_string(),
                ty: p.ty.clone(),
                optional: p.optional,
                kind,
            }
        })
        .collect();
    CallSignature {
        name: spec.name.to_string(),
        params,
        config_gate: Some(spec.gate_class),
    }
}

/// Adapt a user mod/chip's `FnOrChipSig` into a `CallSignature` for
/// `sig::check_args` — the `type_user_symbol_call` analog of
/// `sig_of_callspec`. Every param is a plain `ParamKind::Wire` (user mods
/// have no config-menu params) and non-optional (user mods have no default
/// params either). `subst` — the generic-inference result computed by the
/// caller, `None` for a non-generic call — is applied to each param's type
/// first, exactly like the pre-`check_args` inline coerce loop did; a param
/// whose (possibly-substituted) type still carries a `Type::Param` is left
/// alone here too — `check_args`'s own `type_has_param` guard skips it.
/// `config_gate` is `None`: a named arg that matches no declared param has
/// no data-driven fallback to dispatch to for a user call.
fn sig_of_fnchip(
    name: &str,
    sig: &FnOrChipSig,
    subst: Option<&crate::types::infer::Subst>,
) -> CallSignature {
    let params = sig
        .params
        .iter()
        .map(|p| {
            let ty = match subst {
                Some(s) => substitute(&p.ty, s),
                None => p.ty.clone(),
            };
            Param {
                name: p.name.clone(),
                ty,
                optional: false,
                kind: ParamKind::Wire,
            }
        })
        .collect();
    CallSignature {
        name: name.to_string(),
        params,
        config_gate: None,
    }
}

/// Reject a non-constant value for a scalar/asset config param — it has no wire
/// pin, so a variable or computed value would otherwise lower to a broken wire
/// (a silent "Failed to connect wire" at load) with the config never applied.
/// Uses the same fold check (`expr_to_literal_in`, against the scoped-or-top-level
/// constant environment) the config lowering path uses — so a `let`-bound
/// constant (body-local or top-level) resolves here too, while a genuine `var`
/// or computed value still folds to `None` and stays WS028.
///
/// Takes `gate_class` + `display_name` + `port_name` (rather than a
/// `&CallParam`) so the `sig::check_args` port can call it from a bare `Param`.
/// `display_name` — the source-level surface name — is what the WS028 message
/// shows; `gate_class`/`port_name` are unused by the constant check itself
/// (kept for signature symmetry with `validate_composite_config_arg` /
/// `validate_data_driven_config`, and for any future per-port scalar rule).
fn validate_scalar_config_arg(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    display_name: &str,
    port_name: &str,
    e: &Expr,
) {
    let _ = (gate_class, port_name);
    if crate::lower::expr_to_literal_in(e, &ctx.const_lookup()).is_none() {
        ctx.emit(
            "WS028",
            format!(
                "'{display_name}' is constant-only gate config and cannot take a variable or computed value"
            ),
            e.range().clone(),
        );
    }
}

/// Reject non-constant values in an event handler's constant-only config slots
/// (`on Clock(enabled = flag)`, `on ChatCommand(description = s)`). These
/// settings-menu fields bake into the event gate's data and have no wire pin, so
/// a variable/computed value is silently dropped at lowering (the pre-existing
/// `event_config_props` gap). Named args that WIRE into a gate input port
/// (`input_named`, e.g. Clock's `interval`) may be non-constant and are skipped.
fn validate_handler_config(
    ctx: &mut TypeCheckCtx,
    evt: &crate::catalog::events::EventSpec,
    config: &[crate::ast::HandlerConfigArg],
) {
    use crate::ast::HandlerConfigArg;
    let mut positional = 0usize;
    for arg in config {
        match arg {
            HandlerConfigArg::Positional(value) => {
                let field = evt.config_positional.get(positional).copied();
                positional += 1;
                if let Some(field) = field
                    && crate::lower::expr_to_literal(value).is_none()
                {
                    emit_event_config_const_error(ctx, field, value);
                }
            }
            HandlerConfigArg::Named { name, value } => {
                // A named arg that wires into a gate input port may be dynamic.
                if evt
                    .input_named
                    .iter()
                    .any(|(surf, _, _)| surf.eq_ignore_ascii_case(name))
                {
                    continue;
                }
                if evt.config_named.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
                    && crate::lower::expr_to_literal(value).is_none()
                {
                    emit_event_config_const_error(ctx, name, value);
                }
            }
        }
    }
}

/// Type-check the values wired into an event's `input_named` ports
/// (`on ZoneEntered(zone = z) -> (character)` — `z` must be a `zone`; Clock's
/// `interval`/`enabled` must be float/bool). The value flows on a pure wire, so
/// it is inferred in pure context.
fn check_handler_input_wires(
    ctx: &mut TypeCheckCtx,
    h: &Handler,
) {
    let name = match &h.trigger {
        Trigger::Ident { name, .. } => name,
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, .. } => name,
            _ => return,
        },
        _ => return,
    };
    let Some(evt) = find_event(name) else { return };
    if evt.input_named.is_empty() {
        return;
    }
    for arg in &h.config {
        let HandlerConfigArg::Named { name: argname, value } = arg else {
            continue;
        };
        let Some((_, _, port_ty)) = evt
            .input_named
            .iter()
            .find(|(surf, _, _)| surf.eq_ignore_ascii_case(argname))
        else {
            continue;
        };
        let vty = unwrap_ref(&ctx.in_pure(|ctx| infer::infer(ctx, value)));
        if coerce(&vty, port_ty) == CoerceRule::Mismatch {
            ctx.emit(
                "WS003",
                format!(
                    "event input '{argname}': expected {}, got {}",
                    crate::analysis::types::type_str(port_ty),
                    crate::analysis::types::type_str(&vty),
                ),
                value.range().clone(),
            );
        }
    }
}

fn emit_event_config_const_error(ctx: &mut TypeCheckCtx, field: &str, e: &Expr) {
    ctx.emit(
        "WS028",
        format!(
            "'{field}' is constant-only event config and cannot take a variable or computed value"
        ),
        e.range().clone(),
    );
}

/// Validate a data-driven config attribute (a settings-menu config field set via
/// `<FieldName> = value`, resolved from the inventory `config` array). Enum
/// fields validate their member against the schema; other scalars must be
/// compile-time constants.
fn validate_data_driven_config(
    ctx: &mut TypeCheckCtx,
    gate_class: &str,
    cfg: &crate::catalog::ConfigProperty,
    e: &Expr,
) {
    if let Some(enum_type) = crate::catalog::config_field_enum_type(gate_class, &cfg.name) {
        validate_enum_config_arg(ctx, enum_type, e);
    } else if crate::lower::expr_to_literal_in(e, &ctx.const_lookup()).is_none() {
        ctx.emit(
            "WS028",
            format!(
                "'{}' is constant-only gate config and cannot take a variable or computed value",
                cfg.name
            ),
            e.range().clone(),
        );
    }
}

fn check_let_type_annotation(
    ctx: &mut TypeCheckCtx,
    l: &crate::ast::LetDecl,
    inferred: &Type,
) {
    if let Some(ref te) = l.typ {
        // Record literals: validate field names against the expected record type.
        // Point errors at the specific field/spread that introduced the mismatch.
        if let Expr::RecordLit { fields, .. } = &l.value {
            let expected = resolve_type_expr(ctx, te);
            warn_any_annotation(ctx, &expected, type_expr_range(te));
            if let Type::Record(expected_fields) = &expected {
                let type_name = crate::analysis::types::type_expr_str(te);
                // Check each field/spread for extra fields
                for f in fields {
                    match f {
                        RecordLitField::Named { name, range, .. } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{}' not in type {}", name, type_name),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Shorthand { name, range } => {
                            if !expected_fields.iter().any(|(n, _)| n == name) {
                                ctx.emit(
                                    "WS003",
                                    format!("field '{}' not in type {}", name, type_name),
                                    range.clone(),
                                );
                            }
                        }
                        RecordLitField::Spread { value, range } => {
                            let spread_ty = infer::infer(ctx, value);
                            if let Type::Record(spread_fields) = &spread_ty {
                                let extras: Vec<&str> = spread_fields
                                    .iter()
                                    .filter(|(n, _)| !expected_fields.iter().any(|(en, _)| en == n))
                                    .map(|(n, _)| n.as_str())
                                    .collect();
                                if !extras.is_empty() {
                                    ctx.emit(
                                        "WS003",
                                        format!(
                                            "spread introduces fields not in {}: {}",
                                            type_name,
                                            extras.join(", ")
                                        ),
                                        range.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
                // Check for missing fields (use the whole literal range)
                if let Type::Record(inferred_fields) = inferred {
                    for (fname, _) in expected_fields {
                        if !inferred_fields.iter().any(|(n, _)| n == fname) {
                            ctx.emit(
                                "WS003",
                                format!("missing field '{}' for type {}", fname, type_name),
                                l.range.clone(),
                            );
                        }
                    }
                }
            }
            return;
        }
        let expected = resolve_type_expr(ctx, te);
        warn_any_annotation(ctx, &expected, type_expr_range(te));
        let rule = coerce(inferred, &expected);
        // `ViaString` is fine: anything primitive casts to string, so
        // `let s: string = 5` is an intentional format, not a type lie.
        if rule == CoerceRule::Mismatch {
            let name = match &l.binding {
                crate::ast::LetBinding::Ident { name, .. } => name.clone(),
                _ => "<binding>".into(),
            };
            ctx.diagnostics.push(crate::Diagnostic {
                severity: crate::diagnostic::Severity::Warning,
                code: "WS016".into(),
                message: format!(
                    "let '{}' annotated as {}, but expression has type {}",
                    name,
                    crate::analysis::types::type_expr_str(te),
                    crate::analysis::types::type_str(inferred),
                ),
                range: l.range.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests;
