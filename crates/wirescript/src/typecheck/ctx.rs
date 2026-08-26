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
    /// Mirrors `ast::Param::is_const` for a mod/chip PARAMETER field (`name:
    /// const T`, or every param of a `const mod`); always `false` for an
    /// output, event-data, or `out` field — const-ness only ever constrains
    /// an argument, never a produced value.
    pub is_const: bool,
}

#[derive(Clone, Debug)]
pub struct FnOrChipSig {
    pub params: Vec<EventDataField>,
    /// The mod has a trailing `...rest` variadic parameter: `params` lists only
    /// the FIXED params, and a call may pass any number of extra trailing args
    /// (captured per call site into a compile-time tuple bound to `rest`). Only
    /// ever set for an inline `mod`.
    pub variadic: bool,
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
    /// Number of open frames (>= 1); the top frame's index is `depth() - 1`.
    pub fn depth(&self) -> usize {
        self.inner.depth()
    }
    /// Seal by-name lookups below `floor` (see [`crate::scope::Scope::set_floor`]).
    /// Returns the previous floor for restoration.
    pub fn set_floor(&mut self, floor: usize) -> usize {
        self.inner.set_floor(floor)
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
    /// (`ns.f(args)`) can be arity/type-checked via `check_args`. A
    /// generic member's params may still carry `Type::Param` (there's no
    /// call-site `subst` here to resolve them against); `check_args`'s own
    /// `type_has_param` guard skips those, same as it does elsewhere.
    pub params: Vec<EventDataField>,
    /// For a VALUE member (`let`/`var`/`array`/`map`/`buffer`), its resolved
    /// type — what `ns.member` evaluates to when read rather than called.
    /// `None` for callables, and for a value whose type can't be determined at
    /// registration time (an unannotated non-literal initializer), which reads
    /// as `any`. Without a resolved type here, a namespaced value reference
    /// types as `any` everywhere, so passing it anywhere typed becomes a
    /// spurious mismatch.
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
    /// Every `enum` declaration in the script, keyed by name, discriminants
    /// already assigned. Populated by `enums::collect_enum_defs`'s pre-pass
    /// (same placement as `generic_type_aliases`, run before decl
    /// registration) so a use of the enum name resolves regardless of where
    /// in the file the `enum` itself is declared.
    pub enum_defs: Arc<crate::collections::HashMap<String, crate::typecheck::enums::EnumDef>>,
    /// Top-level `let` constants, so a `var` initializer may name one
    /// (`1 << C_FLAG`) rather than restating its value. Populated before decl
    /// checking; must stay in step with lowering's own environment so both
    /// agree on exactly which initializers are constant.
    pub const_env: crate::lower::ConstEnv,
    /// Every TOP-LEVEL name declared with the `const` keyword — see
    /// [`build_const_declared_names`](crate::lower::build_const_declared_names)
    /// and [`const_lookup_declared_only`](Self::const_lookup_declared_only).
    /// Unlike `const_env` (every top-level `let` that happens to fold), this
    /// only ever holds names actually spelled `const`.
    pub const_declared: crate::collections::HashSet<String>,
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
    /// [`const_declared`](Self::const_declared)'s scoped counterpart: which
    /// `scoped_consts` entries, in the SAME frame, were bound with `const`
    /// rather than a plain `let` — one frame per `scoped_consts` frame,
    /// pushed/popped in lockstep by `push_scope`/`pop_scope`. A name present
    /// in a `scoped_consts` frame but ABSENT from this set at that frame is a
    /// plain `let`: still resolvable through the ordinary `const_lookup`
    /// (config args, WS028, …), but excluded by
    /// [`const_lookup_declared_only`](Self::const_lookup_declared_only).
    pub(super) scoped_const_declared: Vec<crate::collections::HashSet<String>>,
    /// Which `scoped_consts` entries are type-shaped PLACEHOLDERS rather than
    /// real constants — one frame per `scoped_consts` frame, pushed/popped in
    /// lockstep by `push_scope`/`pop_scope`. Two things land here: a `const`
    /// PARAMETER seeded by the `TopDecl::Chip` body-check
    /// (`const_param_placeholder`, a type-shaped zero, because a body is
    /// checked ONCE — before any call site exists — so there is no correct
    /// value to use), and any `let`/`const` binding whose own value derives
    /// from one (see the `Stmt::Let` arm).
    ///
    /// A placeholder's VALUE means only "some constant is here"; it must never
    /// DECIDE anything. [`const_ctx_without_placeholders`](Self::const_ctx_without_placeholders)
    /// is the enforcement: the `Stmt::If` const-elision arm evaluates its
    /// condition against an environment with these removed, so a condition
    /// reading one simply fails to evaluate and BOTH branches get checked —
    /// the safe over-checking direction. Without it, `mod f(m: const int) { if m == 1 {…}
    /// else {…} }` type-checks whichever branch the placeholder zero selects
    /// while lowering — which inlines the REAL argument — ships the other one.
    pub(super) scoped_const_placeholders: Vec<crate::collections::HashSet<String>>,
    /// Depth of enclosing `@nofold` declaration subtrees, mirroring
    /// `lower::LowerCtx::nofold_depth` 1:1 — same module-level seed
    /// (`Script::no_fold`) and the same per-declaration annotation sites.
    /// `@nofold` promises "nothing folded or elided", and `lower_if` honors
    /// that by still building a real `Branch` for a constant condition, so
    /// this stage must stop eliding there too — otherwise typecheck skips a
    /// block that lowering emits. A depth counter rather than a bool for the
    /// same reason lowering uses one: annotated declarations nest.
    pub(super) nofold_depth: u32,
    /// Inferred custom-event receiver slot types (`infer_custom_event_slots`
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
    /// Every `mod`/`chip` declaration seen so far (top-level AND nested,
    /// registered by `register_decl`'s `TopDecl::Chip` arm — the same site
    /// that declares the symbol into `scope`), keyed by bare name. `scope`
    /// only carries a `FnOrChipSig` (params/outputs/generics) for a chip
    /// symbol, not the AST — `resolve_mod` needs the full `ChipDecl` (body
    /// included) to hand a `const mod` CALL to `const_eval::interp::eval_call`.
    /// A FRAME STACK mirroring `scoped_consts` 1:1 (`push_scope`/`pop_scope`
    /// push and pop both together) — see `lower::LowerCtx::pass1_chips` for
    /// the mirrored lowering-side design this now matches. The base frame
    /// (index 0, always present) holds every top-level decl, populated by the
    /// pass-1 registration loop that runs before any scope is pushed; a
    /// nested `const mod` registers into the innermost open frame, so it
    /// shadows a same-named outer declaration only inside the body that
    /// declares it and is gone the moment that body's scope pops.
    ///
    /// A single flat map (instead of a stack) would let two DIFFERENT decls
    /// sharing a name (an outer mod shadowed by a same-named nested one)
    /// collide: whichever is registered LAST silently wins everywhere,
    /// including at a call site that should resolve the other one —
    /// typecheck and lowering (which resolve through scope) would then
    /// evaluate different `const mod` bodies for the same call, so one
    /// branch of a dependent `if` gets type-checked while lowering emits the
    /// other, unchecked. See `resolve_mod`.
    pub mod_decls: Vec<HashMap<String, Arc<ChipDecl>>>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped (`Stmt::If`'s const-elision arm), paired with a human-readable
    /// reason (`` `N > 1` is true here ``). These blocks are NOT type-checked
    /// — `if constexpr` semantics, so a branch that wouldn't even compile for
    /// this const value never gets checked. Mirrors `lower::LowerCtx`'s own
    /// `dropped_ranges`; the two MUST agree on exactly which ranges they drop
    /// (see `typecheck_and_lowering_drop_exactly_the_same_ranges` in
    /// `typecheck::tests`) — a disagreement means code gets type-checked but
    /// not lowered, or lowered without ever being checked.
    pub dropped_ranges: Vec<(SourceRange, String)>,
    /// Memoized `mod`/`chip`-is-exec-requiring answers, keyed by bare name (see
    /// [`infer::mod_is_exec_requiring`](crate::typecheck::infer::mod_is_exec_requiring)).
    /// A mod whose INLINED direct flow reads or writes a container (or calls an
    /// exec builtin, or transitively calls such a mod) is exec-requiring, so a
    /// PURE call of it is the silent-miscompile the WS007 exec-call check
    /// rejects. Scanning the body is O(body) per distinct callee name, so the
    /// answer is cached here rather than recomputed at every call site.
    pub(super) exec_requiring_memo: crate::collections::HashMap<String, bool>,
    /// The expected type `check(ctx, e, expected)` pushes down for the single
    /// node it is checking, consumed once at the top of `infer_node`. Only a
    /// generic enum construction reads it: `let n: Option<int> = None` takes
    /// its `T` from this annotation when the variant's payload can't determine
    /// it (see `infer_enum_args`). `None` outside a `check`; `infer_node`
    /// `take()`s it so a nested inference never sees a stale hint.
    pub(super) expected_ty: Option<Type>,
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
            enum_defs: Arc::new(crate::collections::HashMap::default()),
            const_env: crate::lower::ConstEnv::default(),
            const_declared: crate::collections::HashSet::default(),
            active_combos: 1,
            scoped_consts: Vec::new(),
            scoped_const_declared: Vec::new(),
            scoped_const_placeholders: Vec::new(),
            nofold_depth: 0,
            ce_slots,
            out_ctx: Vec::new(),
            single_output_alias: crate::collections::HashMap::default(),
            // Always non-empty: the base frame holds the module's top-level
            // declarations, registered before any scope is ever pushed (see
            // `mod_decls`'s own doc comment).
            mod_decls: vec![HashMap::default()],
            dropped_ranges: Vec::new(),
            exec_requiring_memo: crate::collections::HashMap::default(),
            expected_ty: None,
        }
    }
    /// Push a new `scope` frame together with a matching empty `scoped_consts`
    /// frame. Use this (paired with `pop_scope`) everywhere a bare
    /// `ctx.scope.push()`/`ctx.scope.pop()` pair would otherwise appear, so
    /// the two stacks can never drift out of lockstep.
    pub fn push_scope(&mut self) {
        self.scope.push();
        self.scoped_consts.push(crate::collections::HashMap::default());
        self.scoped_const_declared
            .push(crate::collections::HashSet::default());
        self.scoped_const_placeholders
            .push(crate::collections::HashSet::default());
        self.mod_decls.push(HashMap::default());
    }
    /// Pop the frame pushed by `push_scope`.
    pub fn pop_scope(&mut self) {
        self.scope.pop();
        self.scoped_consts.pop();
        self.scoped_const_declared.pop();
        self.scoped_const_placeholders.pop();
        // Never pop the base frame — it holds the module's top-level
        // declarations and must outlive every scope opened inside the
        // module (mirrors `lower::LowerCtx::pop_scope`'s guard on
        // `pass1_chips`).
        if self.mod_decls.len() > 1 {
            self.mod_decls.pop();
        }
    }
    /// Run `f` with `nofold_depth` incremented for its duration when `active`
    /// is true (the enclosing declaration was `@nofold`-annotated). Mirrors
    /// `lower::LowerCtx::with_nofold` exactly — including being safe against
    /// an early `return` inside `f`, since a `return` in a closure only
    /// unwinds the closure, so the decrement always runs.
    pub(super) fn with_nofold<R>(&mut self, active: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        if active {
            self.nofold_depth += 1;
        }
        let r = f(self);
        if active {
            self.nofold_depth -= 1;
        }
        r
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
    /// [`const_lookup`](Self::const_lookup), restricted to names DECLARED
    /// `const` — see [`const_declared`](Self::const_declared)'s doc comment.
    /// The `if`-condition elision's own environment (via
    /// [`if_cond_const_ctx`](Self::if_cond_const_ctx)): a plain `let` that
    /// merely happens to fold must not participate in the generalised
    /// const-eval elision, only a real `const`.
    ///
    /// Built the same outer-to-inner way as `const_lookup`, but a frame's
    /// binding either PROMOTES a name into the result (declared `const` in
    /// THIS frame) or EVICTS it (a plain `let` here shadows whatever an
    /// outer frame said, const or not) — so an inner non-const shadow of an
    /// outer `const` correctly drops out, and an inner `const` shadow of an
    /// outer plain `let` correctly appears.
    pub(super) fn const_lookup_declared_only(&self) -> crate::lower::ConstEnv {
        let mut env = crate::lower::ConstEnv::default();
        for name in &self.const_declared {
            if let Some(lit) = self.const_env.get(name) {
                env.insert(name.clone(), lit.clone());
            }
        }
        for (frame, declared) in self.scoped_consts.iter().zip(self.scoped_const_declared.iter()) {
            for (name, lit) in frame {
                if declared.contains(name) {
                    env.insert(name.clone(), lit.clone());
                } else {
                    env.remove(name);
                }
            }
        }
        env
    }
    /// Is `name` a `const`-declared binding holding an array or map?
    ///
    /// The typecheck-side counterpart of `LowerCtx::const_container_literal`,
    /// built from the same `const_lookup_declared_only` environment so both
    /// stages agree on which names are `const` containers. Such a binding is
    /// IMMUTABLE: it is a compile-time value and (once something reads it at
    /// runtime) a container gate, and only the immutability keeps those two
    /// from answering the same question differently.
    pub(super) fn is_const_container(&self, name: &str) -> bool {
        matches!(
            self.const_lookup_declared_only().get(name),
            Some(crate::ir::Literal::Array(_) | crate::ir::Literal::Map(_))
        )
    }
    /// The `ConstCtx` for `const_eval::eval_expr`, built from [`const_lookup`](Self::const_lookup)
    /// so every evaluation site (typecheck, and lowering's own matching helper)
    /// agrees on what's constant at this point. `module_consts` is the
    /// UNMERGED top-level `const_env`: a `const mod` body is evaluated
    /// against the module's constants plus its own parameters, never the
    /// call site's scope frames (see `const_eval::interp::eval_call`).
    ///
    /// `lookup_mod` is supplied by the CALLER, built from [`resolve_mod`](Self::resolve_mod)
    /// — a `&dyn Fn` borrowing `self` can't be packaged into a value THIS
    /// method returns (the closure would have to outlive the call that
    /// builds it, but `self` only lives as long as the caller's own borrow),
    /// so every const-evaluating call site builds its own short-lived
    /// closure over `ctx` and passes it straight through.
    pub fn const_ctx<'b>(
        &self,
        lookup_mod: Option<&'b dyn Fn(&str) -> Option<Arc<ChipDecl>>>,
    ) -> crate::const_eval::ConstCtx<'b> {
        crate::const_eval::ConstCtx {
            consts: self.const_lookup(),
            module_consts: self.const_env.clone(),
            enum_defs: self.enum_defs.clone(),
            lookup_mod,
        }
    }

    /// Every currently-visible PLACEHOLDER name (see
    /// [`scoped_const_placeholders`](Self::scoped_const_placeholders)),
    /// resolved with the same outer-to-inner shadowing
    /// [`const_lookup`](Self::const_lookup) applies: an inner frame binding
    /// the name to a REAL constant un-poisons it, because that inner binding
    /// is what a lookup would find.
    fn placeholder_names(&self) -> crate::collections::HashSet<String> {
        let mut out: crate::collections::HashSet<String> = crate::collections::HashSet::default();
        for (frame, placeholders) in self
            .scoped_consts
            .iter()
            .zip(self.scoped_const_placeholders.iter())
        {
            for name in frame.keys() {
                if placeholders.contains(name) {
                    out.insert(name.clone());
                } else {
                    out.remove(name);
                }
            }
        }
        out
    }

    /// [`const_ctx`](Self::const_ctx) with every visible PLACEHOLDER binding
    /// REMOVED from `consts`, so an expression that reads one fails to
    /// evaluate (`NotConstant`) instead of silently evaluating against a
    /// type-shaped zero. This is structural rather than a syntactic scan of
    /// the expression: a placeholder read anywhere inside — an operand, a
    /// constructor argument, an interpolation slot — is caught, because the
    /// name simply is not in the environment.
    ///
    /// `module_consts` is deliberately left alone: it holds top-level
    /// `const`/`let` bindings only, which are always real values, and it is
    /// what a `const mod` BODY is evaluated against — a callee cannot see the
    /// caller's scope frames at all (`const_eval::interp::eval_call`), so a
    /// const-mod call can never reach a placeholder either way.
    pub(super) fn const_ctx_without_placeholders<'b>(
        &self,
        lookup_mod: Option<&'b dyn Fn(&str) -> Option<Arc<ChipDecl>>>,
    ) -> crate::const_eval::ConstCtx<'b> {
        let mut consts = self.const_lookup();
        for name in self.placeholder_names() {
            consts.remove(&name);
        }
        crate::const_eval::ConstCtx {
            consts,
            module_consts: self.const_env.clone(),
            enum_defs: self.enum_defs.clone(),
            lookup_mod,
        }
    }
    /// [`const_ctx_without_placeholders`](Self::const_ctx_without_placeholders),
    /// additionally restricted to const-DECLARED names (see
    /// [`const_lookup_declared_only`](Self::const_lookup_declared_only)) —
    /// the `Stmt::If` condition-elision arm's own environment. A name must be
    /// BOTH a real constant (not a placeholder) AND actually spelled `const`
    /// (not a plain `let` that merely happens to fold) to make its reader
    /// eligible for the generalised const-eval elision; everything else
    /// falls through to the narrower pre-feature paths (a literal
    /// `true`/`false`, or `ident_literal_bool`'s ident-bound-to-a-literal-
    /// bool-gate fallback on the lowering side), which are unaffected by
    /// this restriction.
    pub(super) fn if_cond_const_ctx<'b>(
        &self,
        lookup_mod: Option<&'b dyn Fn(&str) -> Option<Arc<ChipDecl>>>,
    ) -> crate::const_eval::ConstCtx<'b> {
        let mut consts = self.const_lookup_declared_only();
        for name in self.placeholder_names() {
            consts.remove(&name);
        }
        crate::const_eval::ConstCtx {
            consts,
            module_consts: self.const_env.clone(),
            enum_defs: self.enum_defs.clone(),
            lookup_mod,
        }
    }
    /// Resolve `name` to its `ChipDecl` for a `const mod` CALL: `name` must
    /// currently resolve in `scope` to a chip/mod symbol (exactly the check
    /// an ordinary call to it would pass — an out-of-scope or shadowed-by-a-
    /// non-chip name is `None`, not a stale hit from `mod_decls`), and only
    /// then is the full declaration looked up by scanning `mod_decls`
    /// INNERMOST-frame-first — the same shadowing `const_lookup` gives scoped
    /// constants, and the same scan order as `lower::LowerCtx::resolve_mod_pass1`.
    /// A nested `const mod` therefore wins only for the duration of its own
    /// scope, matching how lowering already resolves the ordinary (non-const)
    /// call path through `ctx.scope`. The caller (every const-evaluating site
    /// in `decl.rs`/`stmt.rs`/`sig.rs`) wraps this in a closure and hands it
    /// to [`const_ctx`](Self::const_ctx) as `lookup_mod`.
    pub(super) fn resolve_mod(&self, name: &str) -> Option<Arc<ChipDecl>> {
        match self.scope.lookup(name) {
            Some(info) if info.kind == SymbolKind::Chip => {
                self.mod_decls.iter().rev().find_map(|frame| frame.get(name)).cloned()
            }
            _ => None,
        }
    }
    /// An `import * as ns` alias is visible ONLY within the file whose import
    /// introduced it — the namespace symbol's `decl_range.file`. A namespace a
    /// module privately imports and a pulled-in declaration calls through
    /// TRAVELS into an importer (so that declaration still resolves once
    /// inlined), but it must not leak into the importer's OWN code: the
    /// importer never wrote that `import * as`, so `ns.member` there should be
    /// an unknown identifier, not a silent resolution. `ref_file` is the file
    /// of the reference being resolved. A namespace is only ever legitimately
    /// named in its own declaring file, so this uniform rule leaves every
    /// same-file use (including a traveling namespace used by its origin
    /// file's own decls) untouched. An empty origin file (synthetic) is
    /// treated as visible everywhere. See N11.
    pub(super) fn namespace_visible(&self, ns_name: &str, ref_file: &str) -> bool {
        match self.scope.lookup(ns_name) {
            Some(sym) if sym.kind == SymbolKind::Namespace => {
                let origin = sym.decl_range.file.as_ref();
                origin.is_empty() || origin == ref_file
            }
            _ => false,
        }
    }

    /// True when `name` IS a namespace symbol but is hidden from `ref_file`
    /// (a traveling alias referenced outside its origin file) — so a call/read
    /// through it must be treated as a dangling base (WS002) rather than a
    /// silent resolution of the leaked namespace. See [`Self::namespace_visible`].
    pub(super) fn namespace_hidden_here(&self, name: &str, ref_file: &str) -> bool {
        matches!(self.scope.lookup(name), Some(s) if s.kind == SymbolKind::Namespace)
            && !self.namespace_visible(name, ref_file)
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

    pub fn warn(&mut self, code: &str, message: impl Into<String>, range: SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
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
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped (never type-checked) — see `TypeCheckCtx::dropped_ranges`.
    pub dropped_ranges: Vec<(SourceRange, String)>,
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
