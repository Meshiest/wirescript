//! `context.ts` and `scope.ts` fused inline).
//!
//! Walks the AST producing a side `typeOfExpr` map (keyed by each
//! expression's source-range start offset) so we don't need to rebuild
//! the AST as a typed parallel. `opResolutions` records the catalog
//! `OpRule` chosen for every BinOp/UnOp; the lower phase consumes it.
//!
//! Identifier semantics for `var` (the design plan's core rule):
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
use crate::types::coerce::{CoerceRule, coerce, widening_join, widening_join_all};

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
}

pub struct TypeCheckCtx {
    pub diagnostics: Vec<Diagnostic>,
    pub scope: Scope,
    exec_stack: Vec<ExecMode>,
    pub file: String,
    pub namespaces: HashMap<String, HashMap<String, NsDeclInfo>>,
    pub if_contexts: HashMap<(Arc<str>, usize), bool>,
    pub var_read_contexts: HashMap<(Arc<str>, usize), bool>,
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
    /// Top-level `let` constants, so a `var` / `array` initializer may name one
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
}

impl TypeCheckCtx {
    pub fn new(file: &str) -> Self {
        Self {
            diagnostics: Vec::new(),
            scope: Scope::new(),
            exec_stack: vec![ExecMode::Pure],
            file: file.to_string(),
            namespaces: HashMap::default(),
            if_contexts: HashMap::default(),
            var_read_contexts: HashMap::default(),
            signal_payload_types: HashMap::default(),
            generic_type_aliases: HashMap::default(),
            const_env: crate::lower::ConstEnv::default(),
            active_combos: 1,
        }
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

pub fn typecheck(script: &Script, file: &str) -> TypeCheckResult {
    let mut ctx = TypeCheckCtx::new(file);
    register_builtin_events(&mut ctx);

    let mut tmap: HashMap<(Arc<str>, usize, usize), Type> = HashMap::default();
    let mut omap: HashMap<(Arc<str>, usize, usize), OpRule> = HashMap::default();

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
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
        }
    }
    for (exec_wrap, d) in deferred_chips {
        if exec_wrap {
            ctx.exec_stack.push(ExecMode::Exec);
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
            ctx.exec_stack.pop();
        } else {
            check_decl(&mut ctx, d, &mut tmap, &mut omap);
        }
    }

    // Whole-program pass: `SendCustomEvent("name", …)` data whose wire types
    // disagree with the `on CustomEvent("name", …)` receiver's declared params.
    // Runs last so every arg has an inferred type in `tmap`.
    check_custom_event_types(&mut ctx, script, &tmap);

    TypeCheckResult {
        type_of_expr: tmap,
        op_resolutions: omap,
        if_contexts: ctx.if_contexts,
        var_read_contexts: ctx.var_read_contexts,
        diagnostics: ctx.diagnostics,
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
    // the lowering-side monomorphizer (P2.5) can rebuild the same masks.
    crate::types::mono::mask_for_param(tp.bound.as_ref(), &ctx.scope.type_aliases())
}

/// Cartesian product of per-type-param masks (Task 2.4): each inner
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
/// (`zone`/`teleport`): like a var ref, they can only be wired or rerouted,
/// never held in a storage gate. Only fires on an *explicit* annotation: an
/// unannotated declaration's inferred placeholder is `Type::Any`, never
/// `Type::Opaque`, so it never reaches this check.
fn reject_any_storage(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange, what: &str) {
    if matches!(resolved, Type::Zone | Type::Teleport) {
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
/// (`Type::Opaque`). A `*any`/`any[]`/`Dict<_, any>`/`{ a: any }` annotation
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
fn type_has_param(t: &Type) -> bool {
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
/// (`*any`, `any[]`, `Dict<_, any>`, `{ a: any }`, …). `any` still works there
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
                // (`var m: Dict<K,V>`) storage — each needs a concrete element/
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
                "a dict's value type",
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
                ctx.scope.push();
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
                ctx.scope.pop();
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
                        ns_map.insert(
                            c.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Chip,
                                return_type,
                            },
                        );
                    }
                    TopDecl::Fn(f) => {
                        ns_map.insert(
                            f.name.clone(),
                            NsDeclInfo {
                                kind: SymbolKind::Fn,
                                return_type: f.return_type.clone(),
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
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    for el in elements {
        let e = el.expr();
        let t = ctx.in_pure(|ctx| infer_expr(ctx, e, tmap, omap));
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
                crate::ir::Literal::Asset { .. } | crate::ir::Literal::PrefabRef { .. }
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

/// Check a map literal's entries against `Dict<K, V>`: each key must coerce to
/// `k`, each value to `v`. This is deliberately STRICTER than the `expect_coerce`
/// helper assignment checking uses — a map *literal* entry additionally rejects
/// `ViaString` (and any coercion that isn't `Same`/`Coerce`), because the literal
/// entry has no gate to run a string-format coercion through (an assignment does).
/// Call this from the VALID slots — a `map`/`var` initializer or an assignment
/// RHS to a map var — so the literal never reaches the generic `infer_expr`
/// `MapLit` arm, which is the position-guard for every other use.
///
/// The entries of a map initializer for a `Dict<K, V>` slot: a real `MapLit`, or
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
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    // A map literal entry has no gate to run a coercion through (e.g.
    // `ViaString`'s `FormatText` gate) — only coercions that hold at the
    // literal itself (`Same`/`Coerce`) are valid; anything else (notably
    // `ViaString`) would silently corrupt every non-matching entry at emit.
    for MapLitEntry { key, value, .. } in entries {
        let kt = infer_expr(ctx, key, tmap, omap);
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
        let vt = infer_expr(ctx, value, tmap, omap);
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
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
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
                        infer_expr(ctx, init, tmap, omap);
                    });
                    let elem_ty = match &inner {
                        Type::Array(e) => e.as_ref().clone(),
                        _ => Type::Any,
                    };
                    check_top_level_array_init(ctx, elements, &elem_ty, tmap, omap);
                } else if let Some(entries) = map_init_entries(init)
                    && let Type::Map(k, v_ty) = &inner
                {
                    // A map-valued `var` initializer: validate entries against
                    // the declared `Dict<K, V>` directly, bypassing the generic
                    // `infer_expr` `MapLit` arm (the position guard) — this IS
                    // the valid initializer slot. An empty `{}` is an empty-map init.
                    ctx.in_pure(|ctx| {
                        check_map_literal(ctx, entries, k, v_ty, tmap, omap);
                    });
                } else {
                    ctx.in_pure(|ctx| {
                        let t = infer_expr(ctx, init, tmap, omap);
                        expect_coerce(ctx, &t, &inner, init.range());
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
                let t = infer_expr(ctx, &b.init, tmap, omap);
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
                check_top_level_array_init(ctx, &a.init, &inner, tmap, omap);
            }
        }
        TopDecl::Map(m) => {
            // Optional literal initializer: `var m: Dict<K, V> = { k => v, ... }`.
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
                    // bypassing the generic `infer_expr` `MapLit` arm (the
                    // position guard). An empty `{}` is an empty-map init.
                    ctx.in_pure(|ctx| {
                        check_map_literal(ctx, entries, &key, &value, tmap, omap);
                    });
                } else {
                    ctx.in_pure(|ctx| {
                        let t = infer_expr(ctx, init, tmap, omap);
                        expect_coerce(
                            ctx,
                            &t,
                            &Type::Map(Box::new(key.clone()), Box::new(value.clone())),
                            init.range(),
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
                let value_ty = ctx.in_pure(|ctx| infer_expr(ctx, value, tmap, omap));
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
                    expect_coerce(
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
            let t = ctx.in_pure(|ctx| infer_expr(ctx, &l.value, tmap, omap));
            check_let_type_annotation(ctx, l, &t, tmap, omap);
            bind_let(ctx, &l.binding, &t);
        }
        TopDecl::Fn(f) => {
            ctx.scope.push();
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
                infer_expr(ctx, &f.body, tmap, omap);
            });
            ctx.scope.pop();
        }
        TopDecl::Chip(c) => {
            // Task 2.4: bounded-polymorphism body checking. Body-check the
            // decl once per concrete member of the cartesian product of its
            // type params' masks — an operation on a type param (`a + 1`)
            // can't resolve against the signature's `Type::Param`
            // placeholder (P2.2/P2.3's registration, untouched by this
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
                ctx.scope.push();
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
                ctx.in_exec(|ctx| check_block(ctx, &c.body, tmap, omap));
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
                ctx.scope.pop();
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
            check_anon_chip_stmts(ctx, &ac.body.stmts, true, tmap, omap);
        }
        TopDecl::Event(e) => {
            if let Some(body) = &e.captured_body {
                ctx.in_exec(|ctx| check_block(ctx, body, tmap, omap));
            } else {
                ctx.in_pure(|ctx| {
                    infer_expr(ctx, &e.source, tmap, omap);
                });
            }
        }
        TopDecl::Handler(h) => {
            ctx.scope.push();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h, tmap, omap);
            ctx.in_exec(|ctx| check_block(ctx, &h.body, tmap, omap));
            ctx.scope.pop();
        }
        TopDecl::ExprStmt(s) => {
            ctx.in_pure(|ctx| {
                infer_expr(ctx, &s.expr, tmap, omap);
            });
        }
        TopDecl::Assign(a) => {
            check_stmt(ctx, &Stmt::Assign(a.clone()), tmap, omap);
        }
        TopDecl::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "top-level 'if' outside an exec context",
                    i.range.clone(),
                );
            }
            check_stmt(ctx, &Stmt::If(i.clone()), tmap, omap);
        }
        TopDecl::Namespace(ns) => {
            // A namespaced (`import * as ns`) mod body references its sibling
            // constants and mods by BARE name, and those mods are inlined at
            // call sites in the importing module. Typecheck the bodies here in
            // an isolated scope (siblings registered as bare names) so operator
            // resolutions and expression types get recorded — otherwise the
            // inlined body's arithmetic and sibling calls lower to _Unsupported.
            ctx.scope.push();
            for d in &ns.decls {
                register_decl(ctx, d);
            }
            for d in &ns.decls {
                if matches!(
                    d,
                    TopDecl::Let(_) | TopDecl::Var(_) | TopDecl::Array(_) | TopDecl::Buffer(_)
                ) {
                    check_decl(ctx, d, tmap, omap);
                }
            }
            for d in &ns.decls {
                if matches!(d, TopDecl::Chip(_) | TopDecl::Fn(_)) {
                    check_decl(ctx, d, tmap, omap);
                }
            }
            ctx.scope.pop();
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
        let known_param_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::Param && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Character | Type::Controller | Type::Entity)
        );
        if !known_event
            && !known_capture
            && !known_input_trigger
            && !known_buffer_trigger
            && !known_let_trigger
            && !known_param_trigger
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
            // Unknown event: bind params as Any so they don't trip downstream lookups.
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
                    let evt_ty = evt.data.get(i).map(|d| d.ty.clone()).unwrap_or(Type::Any);
                    if matches!(evt_ty, Type::Any) {
                        // Custom Event's data outputs are untyped in the catalog:
                        // the receiver must declare each one's type, or the value
                        // has no wire type and defaults to float on emit.
                        ctx.diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS029".into(),
                            message: format!(
                                "custom event param '{0}' should have a type annotation \
                                 (e.g. `{0}: int`) — untyped data has no wire type and \
                                 defaults to float",
                                pname.name
                            ),
                            range: pname.range.clone(),
                        });
                    }
                    evt_ty
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
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    ctx.scope.push();
    for s in &block.stmts {
        check_stmt(ctx, s, tmap, omap);
    }
    ctx.scope.pop();
}

fn check_anon_chip_stmts(
    ctx: &mut TypeCheckCtx,
    stmts: &[Stmt],
    pre_registered: bool,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
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
        match s {
            Stmt::Var(v) => check_decl(ctx, &TopDecl::Var(v.clone()), tmap, omap),
            Stmt::Buffer(b) => check_decl(ctx, &TopDecl::Buffer(b.clone()), tmap, omap),
            Stmt::Array(a) => check_decl(ctx, &TopDecl::Array(a.clone()), tmap, omap),
            Stmt::In(i) => check_decl(ctx, &TopDecl::In(i.clone()), tmap, omap),
            other => check_stmt(ctx, other, tmap, omap),
        }
    }
}

fn check_stmt(
    ctx: &mut TypeCheckCtx,
    s: &Stmt,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
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
                    // routing through the generic `infer_expr` position guard.
                    // An empty `{}` is an empty-map init.
                    check_map_literal(ctx, entries, k, v_ty, tmap, omap);
                } else {
                    let t = infer_expr(ctx, init, tmap, omap);
                    expect_coerce(ctx, &t, &inner, init.range());
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
            check_decl(ctx, &TopDecl::Buffer(b.clone()), tmap, omap);
        }
        Stmt::Array(a) => {
            register_decl(ctx, &TopDecl::Array(a.clone()));
            check_decl(ctx, &TopDecl::Array(a.clone()), tmap, omap);
        }
        Stmt::Map(m) => {
            register_decl(ctx, &TopDecl::Map(m.clone()));
            check_decl(ctx, &TopDecl::Map(m.clone()), tmap, omap);
        }
        Stmt::Let(l) => {
            let t = infer_expr(ctx, &l.value, tmap, omap);
            check_let_type_annotation(ctx, l, &t, tmap, omap);
            bind_let(ctx, &l.binding, &t);
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
            let target_ty = infer_assign_target(ctx, &a.target, tmap, omap);
            if let Expr::MapLit { entries, .. } = &a.value
                && let Type::Map(k, v) = &target_ty
            {
                // Assigning a map literal to a map var: the other valid slot —
                // validate entries directly instead of routing through the
                // generic `infer_expr` position guard.
                check_map_literal(ctx, entries, k, v, tmap, omap);
            } else {
                let value_ty = infer_expr(ctx, &a.value, tmap, omap);
                expect_coerce(ctx, &value_ty, &target_ty, a.value.range());
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
                infer_expr(ctx, value, tmap, omap);
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
                let t = infer_expr(ctx, val, tmap, omap);
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
            ctx.scope.push();
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
            let exec_ty = infer_expr(ctx, &a.exec_expr, tmap, omap);
            ctx.scope.pop();
            let val_ty = if let Some(ref val) = a.value_expr {
                infer_expr(ctx, val, tmap, omap)
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
            infer_expr(ctx, &i.cond, tmap, omap);
            check_block(ctx, &i.then_block, tmap, omap);
            if let Some(else_b) = &i.else_block {
                check_block(ctx, else_b, tmap, omap);
            }
        }
        Stmt::ExprStmt(es) => {
            infer_expr(ctx, &es.expr, tmap, omap);
        }
        Stmt::In(i) => {
            register_decl(ctx, &TopDecl::In(i.clone()));
        }
        Stmt::Handler(h) => {
            ctx.scope.push();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h, tmap, omap);
            ctx.in_exec(|ctx| check_block(ctx, &h.body, tmap, omap));
            ctx.scope.pop();
        }
        Stmt::AnonChip(ac) => {
            // Anon chip shares parent scope — register + check inline.
            check_anon_chip_stmts(ctx, &ac.body.stmts, false, tmap, omap);
        }
        Stmt::ChipDecl(c) => {
            register_decl(ctx, &TopDecl::Chip(c.clone()));
            check_decl(ctx, &TopDecl::Chip(c.clone()), tmap, omap);
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
                infer_expr(ctx, expr, tmap, omap);
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
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    let declared = c.outputs[out_index].ty.clone();
    let Type::Union(mask) = &declared else {
        return declared;
    };
    let mut joined: Option<Type> = None;
    for (i, p) in c.params.iter().enumerate() {
        if matches!(p.ty, Type::Union(_))
            && let Some(CallArg::Positional(e)) = args.get(i)
        {
            let t = unwrap_ref(&infer_expr(ctx, e, tmap, omap));
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
    match joined {
        // Bool never appears as a math-variant on its own (the mask is
        // Float/Int/Vector/Rotator/Quat/Color) — an all-bool fold widens one
        // step further to Int, the mask's narrowest numeric member.
        Some(Type::Bool) if !mask_contains(mask, &Type::Bool) => Type::Int,
        Some(t) => t,
        None => declared,
    }
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

fn infer_assign_target(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
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
        let obj_ty = infer_assign_target(ctx, obj, tmap, omap);
        infer_expr(ctx, index, tmap, omap);
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
/// diagnostics; `call_range` anchors the generic-inference one.
#[allow(clippy::too_many_arguments)]
fn type_user_symbol_call(
    ctx: &mut TypeCheckCtx,
    name: &str,
    sym: &SymbolInfo,
    positional_arg_types: &[Type],
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
    // Validate each positional argument against its (substituted) parameter
    // type — the same coercion the wire layer applies (`PortsAreCompatible`).
    // User mod/chip calls previously skipped this entirely, so `f(int)` on a
    // `vector` param — and a receiver call `x.m()` whose `x`'s type doesn't
    // match `self` — passed clean and then miscompiled at the wire level.
    // Skip: a spread (variable positional count), an argument whose type is the
    // unknown/wildcard `Any`/`Opaque` (already erroring, or deliberately `any`),
    // and a parameter still carrying a `Type::Param` (an uninferable generic,
    // left to the WS033 inference diagnostics above).
    if !has_spread {
        for (i, arg_ty) in positional_arg_types.iter().enumerate() {
            let Some(param) = sig.params.get(i) else { break };
            if matches!(unwrap_ref(arg_ty), Type::Any | Type::Opaque) {
                continue;
            }
            let param_ty = match &subst {
                Some(s) => substitute(&param.ty, s),
                None => param.ty.clone(),
            };
            if type_has_param(&param_ty) {
                continue;
            }
            if coerce(&unwrap_ref(arg_ty), &unwrap_ref(&param_ty)) == CoerceRule::Mismatch {
                ctx.emit(
                    "WS003",
                    format!(
                        "argument '{}': expected {}, got {}",
                        param.name,
                        crate::analysis::types::type_str(&param_ty),
                        crate::analysis::types::type_str(arg_ty),
                    ),
                    call_range.clone(),
                );
            }
        }
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

// ---------- expression inference ----------

fn infer_expr(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    let t = infer_expr_inner(ctx, e, tmap, omap);
    let r = e.range();
    tmap.insert((r.file.clone(), r.start.offset, r.end.offset), t.clone());
    t
}

fn infer_expr_inner(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) -> Type {
    match e {
        Expr::IntLit { .. } => Type::Int,
        Expr::AtomLit { .. } => Type::Int,
        Expr::FloatLit { .. } => Type::Float,
        Expr::StringLit { .. } => Type::String,
        Expr::BoolLit { .. } => Type::Bool,
        Expr::Array { elements, .. } => {
            // Type each element so it lands in the type map; the array's element
            // type is taken from the first element. A spread contributes its
            // source array's element type, a plain item its value type. Whether
            // the elements must be constant literals is enforced at the
            // declaration site (top level) — not here, since the same literal is
            // valid with runtime elements in an exec-context assignment.
            let mut elem = Type::Any;
            for (i, el) in elements.iter().enumerate() {
                let t = unwrap_ref(&infer_expr_inner(ctx, el.expr(), tmap, omap));
                let et = match el {
                    ArrayElem::Spread(_) => match t {
                        Type::Array(inner) => *inner,
                        other => other,
                    },
                    ArrayElem::Item(_) => t,
                };
                if i == 0 {
                    elem = et;
                }
            }
            Type::Array(Box::new(elem))
        }
        // Reached only when a map literal is used somewhere OTHER than a
        // `map`/`var` initializer or an assignment RHS to a map var — those
        // valid slots call `check_map_literal` directly (see `check_decl` /
        // `check_stmt`) and never reach this arm. Still infer each entry (so
        // key/value expressions get typed and any inner errors surface, e.g. a
        // key referencing an unknown identifier) and infer the literal's real
        // `Dict<K, V>` shape from the first entry — mirroring the array
        // literal's element-type inference above — but the position itself is
        // always an error: a map literal must initialize or assign a Map.
        Expr::MapLit { entries, range } => {
            let mut kt = Type::Any;
            let mut vt = Type::Any;
            for (i, entry) in entries.iter().enumerate() {
                let k = infer_expr(ctx, &entry.key, tmap, omap);
                let v = infer_expr(ctx, &entry.value, tmap, omap);
                if i == 0 {
                    kt = k;
                    vt = v;
                }
            }
            ctx.emit(
                "WS026",
                "a dict literal must initialize or assign a Dict variable",
                range.clone(),
            );
            Type::Map(Box::new(kt), Box::new(vt))
        }
        Expr::AssetRef { .. } => {
            // An external asset reference (`$Type/Name`) is an object/class
            // reference — typed `entity` so it can be compared against entity
            // values (e.g. `weapon == $BRItemBase/Weapon_Pickaxe`) and passed
            // into object/class gate ports (which accept `any`/entity anyway).
            // Validation against the asset catalog happens in analysis.
            Type::Entity
        }
        Expr::PrefabRef { path, range } => {
            // A prefab file reference flows into a `bundle_path_ref` gate
            // property; typed `any` so it's accepted there. The `.brz`
            // extension is required (the file resolution + embedding happens at
            // emit); flag it early so the error points at the reference.
            if !path.ends_with(".brz") {
                ctx.emit(
                    "WS019",
                    format!("prefab reference `${path}` must end in `.brz`"),
                    range.clone(),
                );
            }
            Type::Any
        }
        Expr::InterpLit { parts, .. } => {
            for p in parts {
                if let InterpPart::Expr(expr) = p {
                    let t = unwrap_ref(&infer_expr(ctx, expr, tmap, omap));
                    if coerce(&t, &Type::String) == CoerceRule::Mismatch {
                        ctx.emit(
                            "WS003",
                            format!("expected string, got {:?}", t),
                            expr.range().clone(),
                        );
                    }
                }
            }
            Type::String
        }
        Expr::Ident { name, range } => {
            let Some(sym) = ctx.scope.lookup(name).cloned() else {
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                return Type::Any;
            };
            match sym.kind {
                SymbolKind::Event => Type::Exec,
                SymbolKind::Fn | SymbolKind::Chip => Type::Any,
                SymbolKind::Var => {
                    let is_exec = ctx.exec_mode() == ExecMode::Exec;
                    ctx.var_read_contexts
                        .insert((range.file.clone(), range.start.offset), is_exec);
                    unwrap_ref(&sym.ty)
                }
                SymbolKind::Array => sym.ty.clone(),
                _ => sym.ty.clone(),
            }
        }
        Expr::Deref { operand, range } => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS006",
                    format!(
                        "'*{}' deref requires exec context — use .Value for pure reads",
                        target_name(operand).unwrap_or("<expr>".into())
                    ),
                    range.clone(),
                );
            }
            let t = infer_expr(ctx, operand, tmap, omap);
            match t {
                Type::Ref(inner) => *inner,
                Type::Any => Type::Any,
                other => {
                    ctx.emit(
                        "WS003",
                        format!("expected ref T, got {:?}", other),
                        range.clone(),
                    );
                    Type::Any
                }
            }
        }
        Expr::RefOf { operand, .. } => {
            let t = infer_expr(ctx, operand, tmap, omap);
            if matches!(t, Type::Ref(_)) {
                t
            } else {
                Type::Ref(Box::new(t))
            }
        }
        Expr::UnOp { op, operand, range } => {
            let operand_t = infer_expr(ctx, operand, tmap, omap);
            let op_key = if op == "-" { "-u" } else { op.as_str() };
            let unwrapped = op_operand_type(&operand_t);
            let rule = resolve_op(op_key, &[unwrapped]);
            if let Some(r) = rule {
                let result = r.result.clone();
                omap.insert(
                    (range.file.clone(), range.start.offset, range.end.offset),
                    r.clone(),
                );
                result
            } else {
                let code = if matches!(op.as_str(), "&" | "|" | "^" | "~" | "<<" | ">>") {
                    "WS011"
                } else {
                    "WS004"
                };
                ctx.emit(
                    code,
                    format!("no overload for '{op}' on {:?}", operand_t),
                    range.clone(),
                );
                Type::Any
            }
        }
        Expr::BinOp {
            op,
            left,
            right,
            range,
        } => {
            let lt = infer_expr(ctx, left, tmap, omap);
            let rt = infer_expr(ctx, right, tmap, omap);
            let lt_u = op_operand_type(&lt);
            let rt_u = op_operand_type(&rt);
            let rule = resolve_op(op, &[lt_u, rt_u]);
            if let Some(r) = rule {
                let result = r.result.clone();
                omap.insert(
                    (range.file.clone(), range.start.offset, range.end.offset),
                    r.clone(),
                );
                result
            } else {
                let code = if matches!(op.as_str(), "&" | "|" | "^" | "<<" | ">>") {
                    "WS011"
                } else {
                    "WS004"
                };
                ctx.emit(
                    code,
                    format!("no overload for '{op}' on {:?}, {:?}", lt, rt),
                    range.clone(),
                );
                Type::Any
            }
        }
        Expr::FieldAccess { obj, field, range } => {
            let ot = infer_expr(ctx, obj, tmap, omap);
            if let Type::Ref(inner) = &ot {
                if field == "Value" || field == "prev" {
                    return inner.as_ref().clone();
                }
                if field == "VarRef" {
                    return ot.clone();
                }
            }
            // `cN.prev` where `cN` was auto-dereffed in exec context:
            // the obj's type is the inner T, not Ref(T). Look up the
            // declared var type directly.
            if (field == "Value" || field == "prev")
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && let Type::Ref(inner) = &sym.ty
            {
                return inner.as_ref().clone();
            }
            // `x.Value` on a var/ref reads through to the inner type. A record
            // with its own `Value` field (a multi-output gate result such as
            // `a.pop()` → `{ Value, IsEmpty }`) must project that field instead,
            // or `.Value` yields the whole record and every use of it mistypes.
            if (field == "value" || field == "Value") && !matches!(ot, Type::Record(_)) {
                return unwrap_ref(&ot);
            }
            if let Type::Record(fields) = &ot {
                if let Some((_, t)) = fields.iter().find(|(k, _)| k == field) {
                    return t.clone();
                }
                // One concise line naming the valid fields. The full,
                // syntax-COLOURED record type is rendered on hover
                // (analysis::hover) — the only place VS Code can show colour;
                // dumping the multi-line block into the diagnostic too just
                // repeats it verbatim under the hover (VS Code stacks the
                // hover-provider content and the diagnostic message together).
                let names: String = fields
                    .iter()
                    .map(|(n, _)| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                ctx.emit(
                    "WS010",
                    format!("no field `{field}` on record (has: {names})"),
                    range.clone(),
                );
                return Type::Any;
            }
            match (&ot, field.as_str()) {
                (Type::Vector, "x" | "X" | "y" | "Y" | "z" | "Z") => Type::Float,
                (Type::Color, "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A") => Type::Float,
                (Type::Rotator, "pitch" | "yaw" | "roll") => Type::Float,
                // An array read yields the element plus a bounds flag, but it is
                // typed as the bare element (see IndexAccess), so by the time the
                // flag is projected - directly or through a `let` - the object is
                // the element type and this would fall through to Any. Lowering
                // already maps these names to the gate's bOutOfBounds port.
                (_, "OutOfBounds" | "bOutOfBounds") => Type::Bool,
                _ => Type::Any,
            }
        }
        Expr::IndexAccess { obj, index, range } => {
            let ot = unwrap_ref(&infer_expr(ctx, obj, tmap, omap));
            infer_expr(ctx, index, tmap, omap);
            match &ot {
                Type::Array(inner) => {
                    if ctx.exec_mode() != ExecMode::Exec {
                        ctx.emit(
                            "WS007",
                            format!(
                                "array index read '{}[...]' outside an exec context",
                                target_name(obj).unwrap_or("<expr>".into())
                            ),
                            range.clone(),
                        );
                    }
                    inner.as_ref().clone()
                }
                _ => Type::Any,
            }
        }
        Expr::TuplePick { obj, index, range } => {
            let ot = infer_expr(ctx, obj, tmap, omap);
            match &ot {
                Type::Tuple(fields) => fields.get(*index).cloned().unwrap_or_else(|| {
                    ctx.emit(
                        "WS010",
                        format!("tuple index .{index} out of range"),
                        range.clone(),
                    );
                    Type::Any
                }),
                Type::Record(fields) => fields
                    .get(*index)
                    .map(|(_, t)| t.clone())
                    .unwrap_or(Type::Any),
                _ => Type::Any,
            }
        }
        Expr::Call {
            callee,
            args,
            type_args,
            range: call_range,
        } => {
            // Resolve the target call spec (if any) so enum-typed config args
            // can be skipped below — a bare member name (`function = Bounce`)
            // is not a variable and must not be inferred as one.
            let arg_spec: Option<&'static crate::catalog::calls::CallSpec> = match callee.as_ref() {
                Expr::Ident { name, .. } => find_call(name),
                Expr::FieldAccess { field, .. } => {
                    find_call(field).filter(|c| c.receiver.is_some())
                }
                _ => None,
            };
            // A receiver method call binds the object as param 0, so positional
            // args in `args` start at param index 1.
            let pos_base = usize::from(
                matches!(callee.as_ref(), Expr::FieldAccess { .. })
                    && arg_spec.is_some_and(|c| c.receiver.is_some()),
            );
            // Side-effect: typecheck every arg, except enum-typed config args.
            // `positional_arg_types` mirrors the positional args in order —
            // used below to infer a generic user mod/chip call's type params
            // from its arguments (builtin `find_call`s ignore it).
            let mut pos = 0usize;
            let mut positional_arg_types: Vec<Type> = Vec::new();
            for a in args {
                let is_config_enum = arg_spec.is_some_and(|c| match a {
                    CallArg::Named { name, .. } => match c.params.iter().find(|p| p.name == name) {
                        Some(p) => call_param_config_enum(c, p).is_some(),
                        // Not a declared param: a data-driven config attribute.
                        // Skip inference when it names an enum config field (the
                        // value is a bare member name, not a variable).
                        None => crate::catalog::config_field_enum_type(c.gate_class, name).is_some(),
                    },
                    CallArg::Positional(_) => c
                        .params
                        .get(pos_base + pos)
                        .is_some_and(|p| call_param_config_enum(c, p).is_some()),
                    CallArg::Spread(_) => false,
                });
                if matches!(a, CallArg::Positional(_)) {
                    pos += 1;
                }
                if is_config_enum {
                    continue;
                }
                match a {
                    CallArg::Positional(v) => {
                        let t = infer_expr(ctx, v, tmap, omap);
                        positional_arg_types.push(t);
                    }
                    CallArg::Named { value, .. } => {
                        infer_expr(ctx, value, tmap, omap);
                    }
                    CallArg::Spread(v) => {
                        infer_expr(ctx, v, tmap, omap);
                    }
                }
            }
            // Resolve callee if it's a plain identifier.
            if let Expr::Ident { name, range } = callee.as_ref() {
                if let Some(c) = find_call(name) {
                    // Explicit type arguments on a builtin are ignored: a
                    // builtin's result type is derived from its arguments (the
                    // masked-generic engine), not pinned by an explicit `<...>`.
                    // Warn rather than silently dropping them.
                    if !type_args.is_empty() {
                        ctx.diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS037".into(),
                            message: format!(
                                "explicit type arguments are ignored on builtin `{name}` — its \
                                 result type is derived from the arguments"
                            ),
                            range: range.clone(),
                        });
                    }
                    if c.exec && ctx.exec_mode() != ExecMode::Exec {
                        let has_exec_arg = args
                            .iter()
                            .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                        if !has_exec_arg {
                            ctx.emit("WS007", format!("exec call '{name}' outside an exec context (pass exec = ... to override)"), range.clone());
                        }
                    }
                    // Random rides the PrimMath variant like the math operators:
                    // its min/max may be a vector/rotator/quat/color (a
                    // per-component random on the same gate), and the result then
                    // matches that type rather than the int-typed CallSpec.
                    if name == "Random" {
                        let arg_tys: Vec<Type> = args
                            .iter()
                            .filter_map(|a| match a {
                                CallArg::Positional(e) => {
                                    Some(unwrap_ref(&infer_expr(ctx, e, tmap, omap)))
                                }
                                _ => None,
                            })
                            .collect();
                        if let Some(t) = arg_tys.into_iter().find(|t| {
                            matches!(t, Type::Vector | Type::Color | Type::Rotator | Type::Quat)
                        }) {
                            return t;
                        }
                    }
                    check_call_args(ctx, c, args, range, tmap, omap);
                    if c.outputs.len() == 1 {
                        return union_output_type(ctx, c, args, 0, range, tmap, omap);
                    }
                    if c.outputs.len() > 1 {
                        return Type::Record(
                            c.outputs
                                .iter()
                                .map(|o| (o.port.as_str().into(), o.ty.clone()))
                                .collect(),
                        );
                    }
                    if c.exec {
                        return Type::Any;
                    }
                    return c.params.first().map(|p| p.ty.clone()).unwrap_or(Type::Any);
                }
                let Some(sym) = ctx.scope.lookup(name).cloned() else {
                    ctx.emit(
                        "WS002",
                        format!("unknown identifier '{name}'"),
                        range.clone(),
                    );
                    return Type::Any;
                };
                let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                let has_exec_arg = args
                    .iter()
                    .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                let positional_count = args
                    .iter()
                    .filter(|a| matches!(a, CallArg::Positional(_)))
                    .count();
                return type_user_symbol_call(
                    ctx,
                    name,
                    &sym,
                    &positional_arg_types,
                    type_args,
                    positional_count,
                    has_spread,
                    has_exec_arg,
                    call_range,
                    range,
                );
            }
            // Namespace call: ns.foo(args)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Expr::Ident { name: ns_name, .. } = obj.as_ref()
                && ctx.scope.lookup(ns_name).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                let ns_lookup = ctx
                    .namespaces
                    .get(ns_name.as_str())
                    .and_then(|ns_map| ns_map.get(field.as_str()))
                    .map(|info| (info.kind, info.return_type.clone()));
                match ns_lookup {
                    Some((_, Some(ret))) => return resolve_type_expr(ctx, &ret),
                    Some((_, None)) => return Type::Any,
                    None => {
                        ctx.emit(
                            "WS002",
                            format!("'{}' not found in namespace '{}'", field, ns_name),
                            fa_range.clone(),
                        );
                        return Type::Any;
                    }
                }
            }
            // Array method call: arr.push(val), arr.length(), arr.pop(), etc.
            // Any array-typed value works (an `array` decl or a `var ids: T[]`),
            // gated on the field actually being an array method.
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && (sym.kind == SymbolKind::Array || matches!(unwrap_ref(&sym.ty), Type::Array(_)))
                && crate::catalog::arrays::is_array_method(field)
            {
                let elem = match unwrap_ref(&sym.ty) {
                    Type::Array(inner) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                // Return type is derived from the method's gate
                // output ports (see catalog::arrays). Multi-output
                // gates (e.g. find) yield a record that auto-unwraps
                // to whichever field matches the use.
                return crate::catalog::arrays::array_return_type(field, &elem)
                    .unwrap_or(Type::Any);
            }
            // Map method call: m.get(k), m.set(k, v), m.has(k), etc.
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident { name, .. } = obj.as_ref()
                && let Some(sym) = ctx.scope.lookup(name)
                && matches!(unwrap_ref(&sym.ty), Type::Map(_, _))
                && crate::catalog::maps::is_map_method(field)
            {
                let (key, value) = match unwrap_ref(&sym.ty) {
                    Type::Map(k, v) => (k.as_ref().clone(), v.as_ref().clone()),
                    _ => (Type::Any, Type::Any),
                };
                return crate::catalog::maps::map_return_type(field, &key, &value)
                    .unwrap_or(Type::Any);
            }
            // Receiver method call: entity.SetLocation(pos)
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(c) = find_call(field)
                && c.receiver.is_some()
            {
                let mut recv_args = vec![CallArg::Positional(obj.as_ref().clone())];
                recv_args.extend(args.iter().cloned());
                check_call_args(ctx, c, &recv_args, fa_range, tmap, omap);
                if c.outputs.len() == 1 {
                    return union_output_type(ctx, c, &recv_args, 0, fa_range, tmap, omap);
                }
                if c.outputs.len() > 1 {
                    return Type::Record(
                        c.outputs
                            .iter()
                            .map(|o| (o.port.as_str().into(), o.ty.clone()))
                            .collect(),
                    );
                }
                return Type::Any;
            }
            // User `self`-receiver method call: `v.dist(o)` where `dist` is a
            // user mod/chip whose first parameter is named `self`. Desugars to
            // `dist(v, o)` — bind the receiver as positional arg 0 and type it
            // exactly like the plain `dist(v, o)` call would (generics inferred
            // through the receiver too). Placed AFTER the builtin-receiver case
            // so a builtin receiver-method of the same name wins (a user
            // self-mod shadowing one is rejected as WS035 at its declaration).
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && let Some(sym) = ctx.scope.lookup(field).cloned()
                && sym.kind == SymbolKind::Chip
                && sym
                    .signature
                    .as_ref()
                    .is_some_and(|s| s.params.first().is_some_and(|p| p.name == "self"))
            {
                let recv_ty = infer_expr(ctx, obj, tmap, omap);
                let mut recv_pos_types = Vec::with_capacity(positional_arg_types.len() + 1);
                recv_pos_types.push(recv_ty);
                recv_pos_types.extend(positional_arg_types.iter().cloned());
                let has_spread = args.iter().any(|a| matches!(a, CallArg::Spread(_)));
                let has_exec_arg = args
                    .iter()
                    .any(|a| matches!(a, CallArg::Named { name, .. } if name == "exec"));
                let positional_count = 1 + args
                    .iter()
                    .filter(|a| matches!(a, CallArg::Positional(_)))
                    .count();
                return type_user_symbol_call(
                    ctx,
                    field,
                    &sym,
                    &recv_pos_types,
                    type_args,
                    positional_count,
                    has_spread,
                    has_exec_arg,
                    call_range,
                    fa_range,
                );
            }
            // A method/namespace call whose base identifier resolves to nothing:
            // e.g. `card.drawLobby(...)` after an `import * as card` was removed.
            // None of the branches above matched and `card` is not a namespace,
            // variable, or value in scope. Left alone this silently lowers to an
            // `_Unsupported` gate that reads a default (does nothing) at runtime —
            // flag the dangling base, mirroring the bare-identifier WS002 above.
            // Returns so it stays the primary diagnostic for an unknown base
            // (the non-self-mod check below never double-reports over it).
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && let Expr::Ident {
                    name,
                    range: base_range,
                } = obj.as_ref()
                && ctx.scope.lookup(name).is_none()
                && find_call(name).is_none()
            {
                ctx.emit(
                    "WS002",
                    format!(
                        "unknown identifier '{name}' in call `{name}.{field}(...)` — \
                         no namespace, variable, or value named '{name}' is in scope \
                         (is an import missing?)"
                    ),
                    base_range.clone(),
                );
                return Type::Any;
            }
            // A method call `obj.field(...)` on a valid receiver whose `field`
            // names a KNOWN user mod/chip that is NOT a `self`-receiver (the
            // self-mod case above would have returned; builtin / array / map
            // receiver methods were handled earlier). Only a `self`-mod is
            // method-callable — without this, the call would silently type as
            // `any` and lower to an `_Unsupported` no-op (typecheck/lowering
            // divergence). Flag it instead of letting it disappear.
            if let Expr::FieldAccess {
                obj,
                field,
                range: fa_range,
            } = callee.as_ref()
                && ctx
                    .scope
                    .lookup(field)
                    .is_some_and(|s| s.kind == SymbolKind::Chip)
            {
                // Keep the receiver's type in the map (hover/goto) even though
                // the call itself doesn't resolve to a value.
                infer_expr(ctx, obj, tmap, omap);
                ctx.emit(
                    "WS036",
                    format!(
                        "`{field}` is not a receiver method — its first parameter is not \
                         named `self`, so it can't be called as `x.{field}(…)`. Call \
                         `{field}(<receiver>, …)` directly, or rename its first parameter \
                         to `self` to allow method syntax."
                    ),
                    fa_range.clone(),
                );
                return Type::Any;
            }
            Type::Any
        }
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range,
            ..
        } => {
            ctx.if_contexts
                .insert((range.file.clone(), range.start.offset), false);
            infer_expr(ctx, cond, tmap, omap);
            let tt = infer_expr(ctx, then_branch, tmap, omap);
            let et = infer_expr(ctx, else_branch, tmap, omap);
            // A reference (a var ref, or a `zone`/`teleport` component ref) can't
            // flow through the Select gate an if-expr compiles to — Select routes
            // a value, not a reference. Flag whichever branch is a reference.
            for (br, bty) in [(then_branch, &tt), (else_branch, &et)] {
                if is_reference_type(bty) {
                    ctx.emit(
                        "WS031",
                        format!(
                            "'{}' is a reference and can't be used in an if-then-else — a Select \
                             routes a value, not a reference; wire or reroute it instead",
                            crate::analysis::types::type_str(bty),
                        ),
                        br.range().clone(),
                    );
                }
            }
            match widening_join(&tt, &et) {
                Some(j) => j,
                None => {
                    ctx.emit(
                        "WS003",
                        format!(
                            "if-then-else branch type mismatch: then is {}, else is {} (no common widening)",
                            crate::analysis::types::type_str(&tt),
                            crate::analysis::types::type_str(&et),
                        ),
                        range.clone(),
                    );
                    et
                }
            }
        }
        Expr::BlockExpr { stmts, value, .. } => {
            ctx.scope.push();
            for s in stmts {
                check_stmt(ctx, s, tmap, omap);
            }
            let t = infer_expr(ctx, value, tmap, omap);
            ctx.scope.pop();
            t
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            infer_expr(ctx, scrutinee, tmap, omap);
            let mut tys: Vec<Type> = Vec::new();
            for arm in arms {
                if let MatchBody::Expr(expr) = &arm.body {
                    tys.push(infer_expr(ctx, expr, tmap, omap));
                }
            }
            if tys.is_empty() {
                Type::Any
            } else if tys
                .iter()
                .all(|t| std::mem::discriminant(t) == std::mem::discriminant(&tys[0]))
            {
                tys[0].clone()
            } else {
                Type::Union(tys)
            }
        }
        Expr::RecordLit { fields, .. } => {
            let mut rec_fields: Vec<(String, Type)> = Vec::new();
            for f in fields {
                match f {
                    RecordLitField::Named { name, value, .. } => {
                        let ty = infer_expr(ctx, value, tmap, omap);
                        // Override if field already exists (from spread)
                        if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                            existing.1 = ty;
                        } else {
                            rec_fields.push((name.clone(), ty));
                        }
                    }
                    RecordLitField::Shorthand { name, .. } => {
                        let ty = ctx
                            .scope
                            .lookup(name)
                            .map(|s| s.ty.clone())
                            .unwrap_or(Type::Any);
                        if let Some(existing) = rec_fields.iter_mut().find(|(n, _)| n == name) {
                            existing.1 = ty;
                        } else {
                            rec_fields.push((name.clone(), ty));
                        }
                    }
                    RecordLitField::Spread { value, .. } => {
                        let spread_ty = infer_expr(ctx, value, tmap, omap);
                        if let Type::Record(spread_fields) = spread_ty {
                            for (fname, fty) in spread_fields {
                                if let Some(existing) =
                                    rec_fields.iter_mut().find(|(n, _)| *n == fname)
                                {
                                    existing.1 = fty;
                                } else {
                                    rec_fields.push((fname, fty));
                                }
                            }
                        }
                    }
                }
            }
            Type::Record(rec_fields)
        }
    }
}

/// Reference-only types: like a variable ref, these wire and reroute but are not
/// values — they can't be selected, stored, or operated on. Covers the explicit
/// `ref T` var ref plus the opaque `zone`/`teleport` component references.
fn is_reference_type(t: &Type) -> bool {
    matches!(t, Type::Ref(_) | Type::Zone | Type::Teleport)
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
// `types::mono` module so the lowering-side monomorphizer (P2.5) reuses the
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
/// name — nothing to key receivers by).
fn ce_receiver_of(h: &Handler, event_name: &str) -> Option<(String, Vec<Option<Type>>)> {
    if !matches!(&h.trigger, Trigger::Ident { name, .. } if name == event_name) {
        return None;
    }
    let name = h.config.iter().find_map(|c| match c {
        HandlerConfigArg::Positional(Expr::StringLit { value, .. }) => Some(value.clone()),
        _ => None,
    })?;
    let params = h
        .params
        .iter()
        .map(|p| p.ty.as_ref().map(|te| ce_param_type(te)))
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
        if let CallArg::Named { name, value } = a
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
            CallArg::Named { name, value } => {
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
            if let Some((name, params)) = ce_receiver_of(h, "CustomEvent") {
                ce_merge_receiver(&mut personal_recv, name, params);
            } else if let Some((name, params)) = ce_receiver_of(h, "GlobalCustomEvent") {
                ce_merge_receiver(&mut global_recv, name, params);
            }
        },
        &mut |call| {
            if let Expr::Call { callee, .. } = call {
                match callee.as_ref() {
                    Expr::Ident { name, .. } if name == "SendCustomEvent" => {
                        personal_send.push(call)
                    }
                    Expr::Ident { name, .. } if name == "SendGlobalCustomEvent" => {
                        global_send.push(call)
                    }
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
fn validate_composite_config_arg(
    ctx: &mut TypeCheckCtx,
    param: &crate::catalog::calls::CallParam,
    e: &Expr,
) {
    let (ok, hint) = match param.port.as_str() {
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
            format!("'{}' config must be {hint}", param.name),
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

/// Reject a non-constant value for a scalar/asset config param — it has no wire
/// pin, so a variable or computed value would otherwise lower to a broken wire
/// (a silent "Failed to connect wire" at load) with the config never applied.
/// Uses the same fold check (`expr_to_literal`) the config lowering path uses.
fn validate_scalar_config_arg(
    ctx: &mut TypeCheckCtx,
    param: &crate::catalog::calls::CallParam,
    e: &Expr,
) {
    if crate::lower::expr_to_literal(e).is_none() {
        ctx.emit(
            "WS028",
            format!(
                "'{}' is constant-only gate config and cannot take a variable or computed value",
                param.name
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
                let key = name.to_ascii_lowercase();
                if evt.config_named.iter().any(|(k, _)| *k == key)
                    && crate::lower::expr_to_literal(value).is_none()
                {
                    emit_event_config_const_error(ctx, name, value);
                }
            }
        }
    }
}

/// Type-check the values wired into an event's `input_named` ports
/// (`on ZoneEntered(character, zone = z)` — `z` must be a `zone`; Clock's
/// `interval`/`enabled` must be float/bool). The value flows on a pure wire, so
/// it is inferred in pure context.
fn check_handler_input_wires(
    ctx: &mut TypeCheckCtx,
    h: &Handler,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
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
        let vty = unwrap_ref(&ctx.in_pure(|ctx| infer_expr(ctx, value, tmap, omap)));
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

fn check_call_args(
    ctx: &mut TypeCheckCtx,
    spec: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    range: &SourceRange,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
) {
    let positional: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            _ => None,
        })
        .collect();
    let required_count = spec.params.iter().filter(|p| !p.optional).count();
    if positional.len() > spec.params.len() {
        ctx.emit(
            "WS011",
            format!(
                "'{}' expects at most {} positional arg{}, got {}",
                spec.name,
                spec.params.len(),
                if spec.params.len() == 1 { "" } else { "s" },
                positional.len(),
            ),
            range.clone(),
        );
    } else if positional.len() < required_count {
        ctx.emit(
            "WS011",
            format!(
                "'{}' requires {} arg{}, got {}",
                spec.name,
                required_count,
                if required_count == 1 { "" } else { "s" },
                positional.len(),
            ),
            range.clone(),
        );
    }
    for (i, arg_expr) in positional.iter().enumerate() {
        if i >= spec.params.len() {
            break;
        }
        let param = &spec.params[i];
        // An enum-typed config arg is a bare member name or int — validate it
        // against the schema here instead of inferring it as a value (a bare
        // member would otherwise read as an unknown identifier).
        if let Some(enum_type) = call_param_config_enum(spec, param) {
            validate_enum_config_arg(ctx, enum_type, arg_expr);
            continue;
        }
        // Composite constant-only config (meshColors/ammoOverride): validate the
        // constant shape here instead of the generic value coerce below.
        if is_composite_config_param(spec, param) {
            validate_composite_config_arg(ctx, param, arg_expr);
            continue;
        }
        // Plain scalar/asset config (bool/int/float/string/asset settings-menu
        // fields): must be a constant — it bakes into the gate's data.
        if is_scalar_config_param(spec, param) {
            validate_scalar_config_arg(ctx, param, arg_expr);
            continue;
        }
        let arg_ty = unwrap_ref(&infer_expr(ctx, arg_expr, tmap, omap));
        if coerce(&arg_ty, &param.ty) == CoerceRule::Mismatch {
            ctx.emit(
                "WS003",
                format!(
                    "argument '{}': expected {}, got {}",
                    param.name,
                    crate::analysis::types::type_str(&param.ty),
                    crate::analysis::types::type_str(&arg_ty),
                ),
                arg_expr.range().clone(),
            );
        }
    }
    // Named args: config params validate against the schema (enum member,
    // composite shape, or constant scalar); a named arg bound to a real
    // wire-input param is type-checked against the param type with the same
    // coerce the positional path applies — otherwise a wrong-typed named wire
    // arg (`target = 5` on an entity port) would wire an incompatible value in
    // with no diagnostic.
    for a in args {
        if let CallArg::Named { name, value } = a {
            if let Some(param) = spec.params.iter().find(|p| p.name == name) {
                if let Some(enum_type) = call_param_config_enum(spec, param) {
                    validate_enum_config_arg(ctx, enum_type, value);
                } else if is_composite_config_param(spec, param) {
                    validate_composite_config_arg(ctx, param, value);
                } else if is_scalar_config_param(spec, param) {
                    validate_scalar_config_arg(ctx, param, value);
                } else {
                    let arg_ty = unwrap_ref(&infer_expr(ctx, value, tmap, omap));
                    if coerce(&arg_ty, &param.ty) == CoerceRule::Mismatch {
                        ctx.emit(
                            "WS003",
                            format!(
                                "argument '{}': expected {}, got {}",
                                param.name,
                                crate::analysis::types::type_str(&param.ty),
                                crate::analysis::types::type_str(&arg_ty),
                            ),
                            value.range().clone(),
                        );
                    }
                }
            } else if let Some(cfg) =
                crate::catalog::scalar_config_field(spec.gate_class, name)
            {
                // Data-driven config attribute: a settings-menu field set by its
                // raw name (`bOnlyHitPlayerBodyParts = true`), not a declared
                // param. Enum fields validate their member; other scalars must be
                // compile-time constants (they bake into the gate's data).
                validate_data_driven_config(ctx, spec.gate_class, cfg, value);
            }
        }
    }
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
    } else if crate::lower::expr_to_literal(e).is_none() {
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

fn expect_coerce(ctx: &mut TypeCheckCtx, from: &Type, to: &Type, range: &SourceRange) {
    if coerce(from, to) == CoerceRule::Mismatch {
        ctx.emit(
            "WS003",
            format!(
                "expected {}, got {}",
                crate::analysis::types::type_str(to),
                crate::analysis::types::type_str(from),
            ),
            range.clone(),
        );
    }
}

fn check_let_type_annotation(
    ctx: &mut TypeCheckCtx,
    l: &crate::ast::LetDecl,
    inferred: &Type,
    tmap: &mut HashMap<(Arc<str>, usize, usize), Type>,
    omap: &mut HashMap<(Arc<str>, usize, usize), OpRule>,
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
                            let spread_ty = infer_expr(ctx, value, tmap, omap);
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
mod tests {
    use super::*;
    use crate::parser::parse;

    fn tc(src: &str) -> TypeCheckResult {
        let p = parse(src, "test");
        assert!(
            p.diagnostics.is_empty(),
            "parse diagnostics: {:?}",
            p.diagnostics
        );
        typecheck(&p.ast, "test")
    }

    fn assert_no_diags(r: &TypeCheckResult) {
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    }

    #[test]
    fn generic_array_and_ref_syntax_desugar() {
        // `Array<V>` is an alternate spelling of `V[]`, and `Ref<V>` of `*V`.
        assert_no_diags(&tc("var a: Array<int> = [1, 2]"));
        assert_no_diags(&tc("var a: int[] = [1, 2]"));
        assert_no_diags(&tc("mod inc(v: Ref<int>) { v = v + 1 }"));
        assert_no_diags(&tc("mod inc(v: *int) { v = v + 1 }"));
    }

    #[test]
    fn generic_map_type_resolves() {
        // `Dict<K, V>` resolves to a map type usable in an annotation.
        assert_no_diags(&tc("mod f(m: Dict<string, int>) { }"));
    }

    #[test]
    fn unknown_generic_errors() {
        let r = tc("mod f(x: Bogus<int>) { }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"),
            "unknown generic must be WS002: {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn use_before_declaration_is_ws021() {
        // A chip/mod call whose declaration lexically follows the call site
        // cannot resolve during lowering (decls register in source order), so
        // typecheck flags it so the editor surfaces it before compiling.
        let r = tc("mod caller() { let x = target(1) }\nmod target(n: int) -> int { return n }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS021"),
            "use-before-declaration must emit WS021; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn declaration_before_use_no_ws021() {
        let r = tc("mod target(n: int) -> int { return n }\nmod caller() { let x = target(1) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS021"),
            "declaration-before-use must NOT emit WS021; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn random_is_polymorphic_on_prim_math_variant() {
        // Random rides the PrimMath variant like the math operators: min/max may
        // be a vector/rotator/quat/color and the result matches, so assigning it
        // to a same-typed var is clean (no WS003 int-mismatch).
        let r = tc(
            "in a: vector\nin b: vector\nin c1: color\nin c2: color\nvar rv: vector = Vec(0.0, 0.0, 0.0)\nvar rc: color = ColorHex(\"#000000\")\nin go: exec\non go {\n  rv = Random(a, b)\n  rc = Random(c1, c2)\n}",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn random_int_stays_int() {
        // The scalar path is unchanged: Random(int, int) is an int, so it does
        // NOT assign into a vector var.
        let ok = tc("var n: int = 0\nin go: exec\non go { n = Random(1, 10) }");
        assert_no_diags(&ok);
        let bad =
            tc("var v: vector = Vec(0.0, 0.0, 0.0)\nin go: exec\non go { v = Random(1, 10) }");
        assert!(
            bad.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error),
            "Random(int, int) is int and must not assign into a vector var; got {:?}",
            bad.diagnostics
        );
    }

    #[test]
    fn asset_in_array_initializer_warns_ws024() {
        // Asset/prefab references are object references wired in from their own
        // brick; they can't bake into a constant array initializer (they'd be
        // silently dropped), so warn.
        let r =
            tc("var songs: entity[] = [$BrickAudioDescriptor/BA_MUS_Component_Basil_CoffeeShop]");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS024" && d.severity == Severity::Warning),
            "asset in array initializer should warn WS024; got {:?}",
            r.diagnostics
        );
        // A constant array initializer must NOT warn.
        let ok = tc("var nums: int[] = [1, 2, 3]");
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS024"),
            "constant array initializer must not warn; got {:?}",
            ok.diagnostics
        );
    }

    #[test]
    fn wrong_arg_count_is_ws022() {
        // User chips/mods have no default params, so too few (or too many)
        // positional args leaves a param unbound / an arg dropped.
        let too_few =
            tc("mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1) }");
        assert!(
            too_few.diagnostics.iter().any(|d| d.code == "WS022"),
            "too-few args must emit WS022; got {:?}",
            too_few.diagnostics
        );
        let too_many =
            tc("mod g(a: int) -> int { return a }\nin z: exec\non z { let x = g(1, 2) }");
        assert!(
            too_many.diagnostics.iter().any(|d| d.code == "WS022"),
            "too-many args must emit WS022; got {:?}",
            too_many.diagnostics
        );
    }

    #[test]
    fn correct_arg_count_no_ws022() {
        // Matching arity, and an extra `exec =` trigger (not a parameter), are
        // both fine.
        let ok = tc(
            "mod f(a: int, b: int) -> int { return a + b }\nin z: exec\non z { let x = f(1, 2) }",
        );
        assert!(
            !ok.diagnostics.iter().any(|d| d.code == "WS022"),
            "matching arity must NOT emit WS022; got {:?}",
            ok.diagnostics
        );
    }

    #[test]
    fn empty_script() {
        let r = tc("");
        assert_no_diags(&r);
    }

    #[test]
    fn var_int_init() {
        assert_no_diags(&tc("var x: int = 0"));
    }

    #[test]
    fn var_float_int_mismatch_coerces() {
        assert_no_diags(&tc("var x: float = 1"));
    }

    #[test]
    fn var_string_annotation_ok() {
        // Strings can now be stored in vars (WireGraphVariant supports `str`).
        assert_no_diags(&tc("var x: string = \"hi\""));
    }

    #[test]
    fn var_string_inferred_ok() {
        assert_no_diags(&tc("var x = \"hello\""));
    }

    #[test]
    fn var_string_inferred_usable_as_string() {
        // The inferred type must actually be `string`, not `any` — an `any`
        // operand has no `==` overload and would emit WS004.
        assert_no_diags(&tc("var s = \"\"\nout r = s == \"ready\""));
    }

    #[test]
    fn var_int_inferred_usable_in_math() {
        assert_no_diags(&tc("var n = 0\nout d = n + 1"));
    }

    #[test]
    fn var_float_inferred_usable_in_math() {
        assert_no_diags(&tc("var f = 1.5\nout d = f * 2.0"));
    }

    #[test]
    fn var_bool_inferred_usable_in_logic() {
        assert_no_diags(&tc("var b = true\nout d = b && false"));
    }

    #[test]
    fn var_negative_literal_inferred() {
        assert_no_diags(&tc("var n = -5\nout d = n + 1"));
    }

    #[test]
    fn var_nonliteral_init_refines_type() {
        // `var v = Vec(…)` has no literal init; the type refines from the
        // RHS in pass 2 (buffer-style), so vector math resolves.
        assert_no_diags(&tc(
            "var v = Vec(1.0, 2.0, 3.0)\nout d = v + Vec(0.0, 0.0, 1.0)",
        ));
    }

    #[test]
    fn handler_local_var_inferred() {
        assert_no_diags(&tc(
            "on RoundStart { var v = Vec(1.0, 2.0, 3.0)\n let w = v + v }",
        ));
    }

    #[test]
    fn var_inferred_type_catches_mismatch() {
        // Inference makes the var `int`, so assigning a vector is a real
        // WS003 — under the old `any` placeholder this passed silently.
        let r = tc("var n = 0\non RoundStart { n = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into inferred int var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn let_string_annotation_accepts_numeric() {
        // Everything primitive casts to string, so a string annotation on a
        // numeric expression is a format, not a WS016 type lie.
        assert_no_diags(&tc("let s: string = 5"));
    }

    #[test]
    fn let_string_annotation_accepts_entity_family() {
        assert_no_diags(&tc("in c: controller\nlet msg: string = c"));
    }

    #[test]
    fn concat_casts_character_to_string() {
        assert_no_diags(&tc("in p: character\nout s = \"hi \" .. p"));
    }

    #[test]
    fn vector_array_init_elements_are_constants() {
        // Constant Vec(…) folds to a literal, so it's a legal top-level
        // array initializer element (previously WS003).
        assert_no_diags(&tc(
            "var pts: vector[] = [Vec(0.0, 0.0, 0.0), Vec(1.0, 2.0, 3.0)]",
        ));
    }

    #[test]
    fn var_array_of_vectors_infers_element_type() {
        // literal_expr_type knows constructor calls, so an unannotated
        // `var foo = [Vec(…)]` infers vector[] instead of any[].
        assert_no_diags(&tc("var pts = [Vec(1.0, 1.0, 1.0)]"));
    }

    #[test]
    fn color_var_inferred_and_reassignable() {
        // Color() now returns `color` (was `any`), so the var refines and a
        // later color assignment typechecks.
        assert_no_diags(&tc(
            "var tint = Color(1.0, 0.0, 0.0)\non RoundStart { tint = Color(0.0, 1.0, 0.0) }",
        ));
    }

    #[test]
    fn color_var_rejects_vector_assignment() {
        let r = tc("var tint = Color(1.0, 0.0, 0.0)\non RoundStart { tint = Vec(1.0, 1.0, 1.0) }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS003"),
            "vector into color var should be WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_string_in_handler_ok() {
        assert_no_diags(&tc("on RoundStart { var x: string = \"hi\" }"));
    }

    #[test]
    fn let_string_is_fine() {
        let r = tc("let x = \"hello\"");
        assert_no_diags(&r);
    }

    #[test]
    fn unknown_event_diag() {
        let r = tc("on Bogus { }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS001"));
    }

    #[test]
    fn known_event_no_diag() {
        let r = tc("on RoundStart { }");
        assert_no_diags(&r);
    }

    #[test]
    fn expr_trigger_bool_and_compiles() {
        // `on a && b { x = 1 }` is desugared by the parser to
        //   let _on_expr_0 = a && b
        //   on _on_expr_0 { x = 1 }
        // Both steps should typecheck without errors.
        let src = "in a: bool\nin b: bool\nvar x: int = 0\non a && b { x = 1 }";
        assert_no_diags(&tc(src));
    }

    #[test]
    fn handler_event_param_typed() {
        let r = tc("on CharacterDied(c) { }");
        assert_no_diags(&r);
    }

    #[test]
    fn assignment_in_handler_ok() {
        let r = tc("var n: int = 0\non RoundStart { n = n + 1 }");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
    }

    #[test]
    fn assignment_outside_exec_diag() {
        // Top-level assigns trip WS007 because there's no enclosing exec chain.
        let r = tc("var n: int = 0\nn = 1");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS007"));
    }

    #[test]
    fn binop_resolution_recorded() {
        let r = tc("var x: int = 1\nvar y = x + 2");
        // We don't care about the *contents* of opResolutions deeply here;
        // just that something was recorded.
        assert!(!r.op_resolutions.is_empty());
    }

    #[test]
    fn unknown_var_emits_diag() {
        let r = tc("on RoundStart { x = 1 }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS002"));
    }

    #[test]
    fn namespace_call_with_undefined_base_is_ws002() {
        // A namespace-qualified call whose base identifier isn't in scope — e.g.
        // an `import * as card` was removed but `card.drawLobby(...)` calls
        // remain. None of the namespace/array/receiver branches match, so
        // without an explicit check the call silently lowers to an
        // `_Unsupported` gate that does nothing at runtime.
        let r = tc("mod drawLobby(n: int) { }\non RoundStart { card.drawLobby(1) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS002" && d.message.contains("card")),
            "undefined namespace base must emit WS002; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn return_in_handler_no_error() {
        let r = tc("var x: int = 0\non RoundStart { x = 1\nreturn\nx = 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn return_in_exec_no_error() {
        let r = tc("var x: int = 0\non RoundStart { if x > 5 { return } }");
        assert_no_diags(&r);
    }

    #[test]
    fn not_on_int_no_error() {
        let r = tc("var x: int = 0\nlet y = !x");
        assert_no_diags(&r);
    }

    #[test]
    fn interp_ref_var_no_error() {
        let r = tc("var x: int = 0\nlet s = \"value: ${x}\"");
        assert_no_diags(&r);
    }

    // ---- chip single-output auto-unwrap ----
    #[test]
    fn chip_single_output_pure() {
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f == 42",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn chip_single_output_exec() {
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nvar err: int = 0\non RoundStart {\n  if f != 42 { err = 1 }\n}",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn chip_single_output_field_access_compat() {
        // f.result should still work for backwards compatibility
        let r = tc(
            "chip Foo(x: int) -> (result: int) {\n  out result = x * 2\n}\nlet f = Foo(21)\nlet ok = f.result",
        );
        assert_no_diags(&r);
    }

    // ---- buffer ----
    #[test]
    fn buffer_decl() {
        let r = tc("var x: int = 0\nbuffer prev: int = x");
        assert_no_diags(&r);
    }

    #[test]
    fn buffer_inferred_type() {
        let r = tc("var x: int = 0\nbuffer prev = x + 1");
        assert_no_diags(&r);
    }

    // ---- mod / inline chip ----
    #[test]
    fn mod_decl_no_error() {
        let r = tc("mod inc(v: *int) { v = v + 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn mod_call_in_exec() {
        let r = tc("var x: int = 0\nmod inc(v: *int) { v = v + 1 }\non RoundStart { inc(x) }");
        assert_no_diags(&r);
    }

    // ---- anonymous chip ----
    #[test]
    fn anon_chip_shares_scope() {
        let r = tc("var x: int = 0\nchip { var y: int = 0 }\non RoundStart { x = 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn chip_on_handler() {
        let r = tc("var x: int = 0\nchip on RoundStart { x = 1 }");
        assert_no_diags(&r);
    }

    // ---- emit ----
    #[test]
    fn emit_in_exec() {
        let r = tc("var x: int = 0\nout result = x\non RoundStart { emit result }");
        assert_no_diags(&r);
    }

    // ---- bool literal ----
    #[test]
    fn bool_literal() {
        let r = tc("var x: bool = true\nvar y: bool = false");
        assert_no_diags(&r);
    }

    // ---- chip exec param as trigger ----
    #[test]
    fn chip_exec_param_trigger() {
        let r = tc(
            "chip Counter(bump: exec, reset: exec) -> (value: int) {\n  var n: int = 0\n  on bump { n = n + 1 }\n  on reset { n = 0 }\n  out value = n.Value\n}",
        );
        assert_no_diags(&r);
    }

    // ---- character to entity coercion ----
    #[test]
    fn character_coerces_to_entity() {
        let r = tc("in ch: character\non RoundStart { ch.SetLocation(Vec(0.0, 0.0, 0.0)) }");
        assert_no_diags(&r);
    }

    // ---- call arg validation ----
    #[test]
    fn call_too_many_args() {
        let r = tc("on RoundStart { Random(1, 2, 3, 4, 5) }");
        assert!(r.diagnostics.iter().any(|d| d.code == "WS011"));
    }

    #[test]
    fn call_wrong_arg_type() {
        let r = tc("on RoundStart { SetLocation(42, Vec(0.0, 0.0, 0.0)) }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("argument"))
        );
    }

    // ---- namespace import ----
    #[test]
    fn namespace_symbol_registered() {
        use crate::resolve::{MemLoader, resolve};
        let loader = MemLoader {
            files: [("lib.ws".into(), "mod foo(v: *int) { v = v + 1 }".into())]
                .into_iter()
                .collect(),
        };
        let resolved = resolve("import * as lib from \"lib\"", "main.ws", &loader);
        let r = typecheck(&resolved.ast, "main.ws");
        assert_no_diags(&r);
    }

    // ---- chip let ----
    #[test]
    fn chip_let_pure_context() {
        let r = tc("var x: int = 0\nchip let doubled = x * 2");
        assert_no_diags(&r);
    }

    // ---- receiver call ----
    #[test]
    fn receiver_call_method() {
        let r = tc("var ctrl: controller\non RoundStart { ctrl.DisplayText(\"hi\") }");
        assert_no_diags(&r);
    }

    #[test]
    fn entity_receiver_accepts_character_controller_methods() {
        // An entity wire (e.g. Sweep's HitEntity) can be a player, so
        // character/controller receiver methods and params accept it.
        let r = tc("in e: entity\nin t: exec\non t { e.ShowStatusMessage(\"hi\") }");
        assert_no_diags(&r);
        let r2 = tc("in e: entity\nin t: exec\non t { ShowStatusMessage(e, \"hi\") }");
        assert_no_diags(&r2);
    }

    // ---- array index ----
    #[test]
    fn array_index_returns_element_type() {
        // Array reads require exec context (compile to Exec_ArrayVar_Get).
        let r = tc(
            "var items: int[]\nin trigger: exec\non trigger { let x = items[0]\nlet ok = x + 1 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn array_index_outside_exec_is_ws007() {
        // Array index read in pure context should emit WS007.
        let r = tc("var items: int[]\nlet x = items[0]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS007"),
            "expected WS007 for array index read outside exec context"
        );
    }

    #[test]
    fn array_param_index() {
        // Array params put the mod in exec context, so arr[idx] is fine.
        let r = tc("mod process(arr: int[], idx: int) {\n  let old = arr[idx]\n  out r = old\n}");
        assert_no_diags(&r);
    }

    #[test]
    fn array_param_index_dot_value() {
        // arr[i].value works fine — array params put the mod in exec context.
        let r =
            tc("mod process(arr: int[], idx: int) {\n  let old = arr[idx].value\n  out r = old\n}");
        assert_no_diags(&r);
    }

    // ---- array methods ----
    #[test]
    fn array_push_pop() {
        let r =
            tc("var items: int[]\nin trigger: exec\non trigger { items.push(1)\nitems.pop() }");
        assert_no_diags(&r);
    }

    #[test]
    fn array_length_returns_int() {
        let r = tc(
            "var items: int[]\nin trigger: exec\non trigger { let len = items.length()\nlet ok = len + 1 }",
        );
        assert_no_diags(&r);
        // len should be Int, so len + 1 should resolve without error.
        // If length() returned Any, the + would still work (Any coerces),
        // so also check the inferred type directly.
        let len_type = r.type_of_expr.values().find(|t| **t == Type::Int);
        assert!(len_type.is_some(), "length() should infer as Int");
    }

    // ---- if expression (ternary) ----
    #[test]
    fn if_expr_ternary() {
        let r = tc("var x: int = 0\nlet y = if x > 0 then 1 else 0");
        assert_no_diags(&r);
    }

    // ---- string interpolation ----
    #[test]
    fn string_interp_multiple() {
        let r = tc("var a: int = 1\nvar b: float = 2.0\nlet s = \"a=${a} b=${b}\"");
        assert_no_diags(&r);
    }

    // ---- octal/hex/binary literals ----
    #[test]
    fn numeric_literal_bases() {
        let r = tc("var a: int = 0xFF\nvar b: int = 0b1010\nvar c: int = 0o77");
        assert_no_diags(&r);
    }

    // ---- records & type aliases ----
    #[test]
    fn type_alias_record() {
        let r = tc("type Point = { x: int, y: int }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_literal_typed() {
        let r = tc("type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_field_access() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet sum = p.x + p.y",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_shorthand() {
        let r =
            tc("type Point = { x: int, y: int }\nlet x = 1\nlet y = 2\nlet p: Point = { x, y }");
        assert_no_diags(&r);
    }

    #[test]
    fn record_spread() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet a: Point = { x: 1, y: 2 }\nlet b: Point = { ...a, y: 99 }",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_destructure() {
        let r = tc(
            "type Point = { x: int, y: int }\nlet p: Point = { x: 1, y: 2 }\nlet { x, y } = p\nlet sum = x + y",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn record_as_mod_param() {
        let r = tc(
            "type Point = { x: int, y: int }\nmod sum(p: Point) -> (r: int) { return p.x + p.y }\nlet p: Point = { x: 3, y: 4 }\nlet s = sum(p)",
        );
        assert_no_diags(&r);
    }

    #[test]
    fn mod_param_record_destruct() {
        let r = tc(
            "type Point = { x: int, y: int }\nmod add({ x, y }: Point) -> int { return x + y }\nlet p: Point = { x: 3, y: 4 }\nlet sum = add(p)",
        );
        assert_no_diags(&r);
    }

    // ---- `any` type annotation ----

    #[test]
    fn any_in_port_wildcard_operators_typecheck() {
        // `any` on a port must resolve real operator overloads (the wildcard
        // behavior `Type::Opaque` gives it), not fall back to the generic
        // `Type::Any` error type that WS004 rejects for operators.
        let r = tc("in t: any\nlet a = t & 1\nlet b = t + 1\nlet c = t == \"x\"");
        assert_no_diags(&r);
    }

    #[test]
    fn any_let_annotation_no_error() {
        let r = tc("let x: any = 5\nlet y = x + 1");
        assert_no_diags(&r);
    }

    #[test]
    fn any_mod_param_no_error() {
        let r = tc("mod f(v: any) { let inner = v + 1 }");
        assert_no_diags(&r);
    }

    #[test]
    fn any_chip_param_and_output_no_error() {
        let r = tc("chip C(v: any) -> (z: any) { out z = v }\nlet c = C(1)\nlet y = c + 1");
        assert_no_diags(&r);
    }

    #[test]
    fn any_out_annotation_no_error() {
        let r = tc("in t: any\nout y: any = t");
        assert_no_diags(&r);
    }

    #[test]
    fn var_any_is_ws025() {
        let r = tc("var foo: any = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`var foo: any` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn static_var_any_is_ws025() {
        let r = tc("static var foo: any = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`static var foo: any` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn array_any_element_is_ws025() {
        let r = tc("var arr: any[]");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`var arr: any[]` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn buffer_any_is_ws025() {
        let r = tc("buffer buf: any = 0");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "`buffer buf: any = 0` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_any_inside_handler_is_ws025() {
        // Same rejection for a statement-level `var` declared inside a
        // handler body (a separate code path from the top-level decl).
        let r = tc("in t: exec\non t { var foo: any = 0 }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS025"),
            "statement-level `var foo: any` must be rejected as WS025, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_unannotated_is_not_ws025() {
        // An unannotated var's placeholder type is `Type::Any` (the
        // generic fallback), never `Type::Opaque` — it must not trip the
        // `any`-storage rejection.
        let r = tc("var foo = 0");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS025"),
            "unannotated var must not emit WS025, got {:?}",
            r.diagnostics
        );
    }

    // ---- string → bool coercion (lowers to an inserted `!= ""` compare) ----

    #[test]
    fn if_string_condition_compiles() {
        // No dedicated bool-condition check exists for `if` — a string
        // condition typechecks cleanly, and lowering inserts the
        // `CompareNotEqual(s, "")` coercion gate in front of the Branch.
        let r = tc("in s: string\nin t: exec\nvar a: int = 0\non t { if s { a = 1 } }");
        assert_no_diags(&r);
    }

    #[test]
    fn let_bool_annotation_from_string_no_warning() {
        // Before the `String -> Bool` coercion rule this hit the generic
        // "let annotated as X, but expression has type Y" WS016 warning
        // (a mismatch is a legitimate re-annotation warning elsewhere, but
        // here it's a certified native coercion, not a type lie).
        let r = tc("in s: string\nlet b: bool = s");
        assert!(
            r.diagnostics.is_empty(),
            "`let b: bool = s` must not warn or error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn var_bool_assign_from_string_no_error() {
        // Assigning a string-typed value into a declared-bool var previously
        // hit a hard WS003 "expected Bool, got String" mismatch.
        let r = tc("in s: string\nin t: exec\nvar v: bool = false\non t { v = s }");
        assert_no_diags(&r);
    }

    // ---- string -> bool must NOT chain transitively into numerics ----
    //
    // Every consumer of `coerce()` (expect_coerce, check_call_args,
    // check_let_type_annotation, unify_glb) applies exactly ONE rule between
    // a source and a destination type — nothing composes String -> Bool with
    // Bool -> Int — and operator resolution (`resolve_op`) never consults
    // coercions at all. These pins keep it that way.

    #[test]
    fn string_does_not_coerce_to_int_destination() {
        // `let n: int = s` stays flagged (WS016 — the annotated-let mismatch
        // warning, the same diagnostic any non-coercing annotation gets)...
        let r = tc("in s: string\nlet n: int = s");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS016"),
            "`let n: int = s` must still warn WS016, got {:?}",
            r.diagnostics
        );
        // ...and assigning a string into a declared-int var stays a hard
        // WS003 error (the exec-assign path).
        let r = tc("in s: string\nin t: exec\nvar n: int = 0\non t { n = s }");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.severity == Severity::Error),
            "`n = s` into an int var must stay a WS003 error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn string_math_operand_still_ws004() {
        // Operator resolution matches explicit rule lists only (no coercion
        // consult), and the math gates have no string operand rules — a
        // string on either side of `+` must keep erroring, not sneak in as
        // String -> Bool -> Int. (`..` is the string concat operator.)
        let r = tc("in s: string\nlet a = s + 1");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS004"),
            "`s + 1` must stay WS004, got {:?}",
            r.diagnostics
        );
        let r = tc("let a = \"a\" + 1");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS004"),
            "`\"a\" + 1` must stay WS004, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn string_into_int_builtin_param_still_errors() {
        // A string argument into an int-typed builtin port stays WS003 —
        // check_call_args does one coerce(String, Int) = Mismatch, with no
        // bool hop available.
        let r = tc("in s: string\nlet c = ColorSRGB(s, 0, 0, 255)");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.message.contains("expected int")),
            "string into ColorSRGB's int param must stay WS003, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn bool_to_int_coercion_still_works() {
        // Regression pin: the neighboring bool -> int coercion (and the
        // annotated-let form) must be unaffected by the string-truthiness
        // rule sitting next to it in coerce().
        let r = tc("in f: bool\nlet n: int = f\nin t: exec\nvar m: int = 0\non t { m = f }");
        assert_no_diags(&r);
    }

    // ---- annotated `out` value/annotation agreement ----

    #[test]
    fn annotated_out_bool_from_string_coerces() {
        // `out y: bool = s` is the string → bool coercion (the `!= ""`
        // compare inserts at lowering) — no diagnostic.
        let r = tc("in s: string\nout y: bool = s");
        assert_no_diags(&r);
    }

    #[test]
    fn annotated_out_int_from_string_is_ws003() {
        // Pre-existing hole: annotated outs never checked their value
        // against the annotation, so `out y: int = s` passed silently and
        // emitted a mistyped pin. Now WS003.
        let r = tc("in s: string\nout y: int = s");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.code == "WS003" && d.severity == Severity::Error),
            "`out y: int = s` must be a WS003 error, got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn annotated_out_string_from_int_formats() {
        // Per the coercion table, int → string is `ViaString` (format
        // gate), not a mismatch — an annotated string out accepts a
        // numeric value without diagnostics.
        let r = tc("in n: int\nout label: string = n");
        assert_no_diags(&r);
    }

    #[test]
    fn annotated_ref_out_still_accepts_var() {
        // `out y: *int = x` — the ref annotation unwraps for the check
        // (int against int), so the ref-exposure pattern stays clean.
        let r = tc("var x: int = 0\nout y: *int = x");
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "ref out must not error: {:?}", errors);
    }

    // ---------- `self`-receiver (UFCS) ----------

    #[test]
    fn self_mod_shadowing_builtin_receiver_is_ws035() {
        // A user `self`-mod named exactly like a builtin receiver-method on the
        // same receiver type (`Dot` on vector) would be silently shadowed by the
        // builtin at every call site — a footgun, so it is rejected at the
        // declaration.
        let r = tc("mod Dot(self: vector, o: vector) -> float { return 0.0 }");
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS035"),
            "a self-mod shadowing a builtin receiver-method must emit WS035; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn self_mod_distinct_name_no_shadow() {
        // A distinctly-named self-mod does not collide with any builtin.
        let r = tc("mod dist(self: vector, o: vector) -> float { return self.Dot(o) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS035"),
            "a distinct-named self-mod must not be flagged as shadowing; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn self_mod_same_name_different_receiver_no_shadow() {
        // `Dot` is a builtin receiver-method on `vector`; a self-mod named `Dot`
        // whose receiver is a DIFFERENT type (int) does not overlap it, so the
        // builtin never shadows it and there is no error.
        let r = tc("mod Dot(self: int) -> int { return self }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS035"),
            "the same name on a different receiver type must not shadow; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn non_self_mod_receiver_type_still_not_shadow() {
        // A normal (non-`self`) mod is never a receiver method, so it can never
        // shadow a builtin receiver-method even if it shares the name.
        let r = tc("mod Dot(a: vector, o: vector) -> float { return a.Dot(o) }");
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS035"),
            "a non-self mod must never be flagged as shadowing; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn self_mod_method_call_wrong_arg_count_is_ws022() {
        // `a.dist()` is missing the `o` argument: with the receiver bound as
        // arg 0 the call still has too few args for `dist(self, o)`. Proves the
        // method call is resolved to the user mod (before the feature it typed
        // as `any` and no arg-count check fired).
        let r = tc(
            "mod dist(self: vector, o: vector) -> float { return self.Dot(o) }\n\
             in a: vector\nin go: exec\non go { let d = a.dist() }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS022"),
            "a receiver method call with too few args must emit WS022; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn non_self_mod_method_call_is_ws036() {
        // `f`'s first param is not named `self`, so `v.f(w)` is NOT a valid
        // method call — only `self` opts in. Rather than silently typing as
        // `any` and lowering to an `_Unsupported` no-op, it is a hard error.
        let r = tc(
            "mod f(a: vector, o: vector) -> float { return a.Dot(o) }\n\
             in v: vector\nin w: vector\nin go: exec\non go { let d = v.f(w) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS036"),
            "a non-self mod called with method syntax must emit WS036; got {:?}",
            r.diagnostics
        );
    }

    #[test]
    fn unknown_base_method_call_stays_ws002_not_ws036() {
        // An unknown receiver base is the primary problem — it stays WS002 and
        // must NOT be masked by the non-self-mod WS036 check, even when the
        // method name happens to be a known non-self mod.
        let r = tc(
            "mod scale(v: vector, k: float) -> vector { return v * k }\n\
             in go: exec\non go { let s = missing.scale(2.0) }",
        );
        assert!(
            r.diagnostics.iter().any(|d| d.code == "WS002"),
            "an unknown base must emit WS002; got {:?}",
            r.diagnostics
        );
        assert!(
            !r.diagnostics.iter().any(|d| d.code == "WS036"),
            "WS036 must not double-report over an unknown base; got {:?}",
            r.diagnostics
        );
    }
}
