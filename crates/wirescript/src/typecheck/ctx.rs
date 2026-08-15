//! Type-checker state: symbols, scopes, the shared context, and its result.

use super::*;

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
    /// For a VALUE member (`let`/`var`/`array`/`map`/`buffer`), its resolved
    /// type — what `ns.member` evaluates to when read rather than called.
    /// `None` for callables, and for a value whose type can't be determined at
    /// registration time (an unannotated non-literal initializer), which reads
    /// as `any` exactly as before. Without this a namespaced value reference
    /// typed `any`, so passing it anywhere typed was a spurious mismatch.
    pub value_type: Option<Type>,
}

pub struct TypeCheckCtx<'a> {
    pub diagnostics: Vec<Diagnostic>,
    pub scope: Scope,
    pub(super) exec_stack: Vec<ExecMode>,
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
    /// Stack of the enclosing decl's declared outputs, one frame per
    /// currently-checked module/chip/mod body — `out_ctx.last()` is the
    /// CURRENT (innermost) frame. Consulted by `current_output_ty` so
    /// `return`/`emit`/an unannotated statement-level `out` can be checked
    /// against the declared output type they're implicitly targeting (see
    /// the module-level push in `typecheck` and the per-combo push in the
    /// `TopDecl::Chip` body-check loop).
    pub(super) out_ctx: Vec<Vec<EventDataField>>,
    /// `let f = Foo(…)` where `Foo` is a chip/mod with exactly ONE output:
    /// binding name -> that output's name. A single-output call types as the
    /// bare output value (the name is dropped), but `f.result` stays legal for
    /// readability — so this records the one field name a scalar binding may be
    /// projected by, letting `infer` tell that projection apart from a typo
    /// (`f.reslt`) instead of silently typing both as `any`. A `None` value
    /// means "known to be a single-output call result, but its output name
    /// isn't indexed here" — any field is accepted, never a false typo.
    pub single_output_alias: crate::collections::HashMap<String, Option<String>>,
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
            out_ctx: Vec::new(),
            single_output_alias: crate::collections::HashMap::default(),
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

/// The declared type of output `name` in the CURRENT (innermost) `out_ctx`
/// frame — the enclosing module/chip/mod's declared outputs, nearest wins.
/// `None` when `name` isn't a declared output of that frame (e.g. a local
/// exec signal, or an unannotated top-level `out`), meaning there's nothing
/// to check against.
///
/// Note: a same-named local `var`/`let` does NOT shadow the output here, and
/// must not — `emit`/`out` lowering resolves the target via `lookup_output`
/// (a disjoint namespace) BEFORE any local, so `out r = v` inside
/// `chip … -> (r: T) { var r: … }` still wires `v` into output `r`. Checking
/// against the output type is therefore correct even when a local shares the
/// name.
pub(super) fn current_output_ty(ctx: &TypeCheckCtx, name: &str) -> Option<Type> {
    ctx.out_ctx.last()?.iter().find(|f| f.name == name).map(|f| f.ty.clone())
}
