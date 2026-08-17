use super::*;

// ---------- context ----------

#[derive(Clone, Debug)]
pub(super) struct VarRecord {
    pub(super) node_id: NodeId,
    pub(super) inner_type: Type,
    /// Cached Var_Get node for this handler (reuse within one handler body).
    pub(super) get_node_for_handler: Option<NodeId>,
    pub(super) storage: VarStorage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VarStorage {
    Var,
    Buffer,
    Array,
    Map,
}

/// Classify a param/field type that binds a *container* — one whose value wires
/// a ref port (`ArrayVarRef`/`MapVarRef`) or is a scalar `ref T` — rather than a
/// plain by-value input. Returns the backing [`VarStorage`] and the
/// `inner_type` to record: the element type for arrays, the referent for a
/// scalar `ref T`, and the WHOLE `Type::Map(K,V)` for maps (matching
/// `pre_declare_map`). `None` for a by-value scalar.
///
/// Classifying containers in ONE place — instead of each binding site
/// re-deriving `is_array`/`is_ref` by pattern-match — is what keeps `Map<K,V>`
/// (and any future container) from silently falling through the `Array`/`Ref`
/// matches into a scalar `Var_Get` + `_Unsupported` method lowering.
/// The backing [`VarStorage`] for a param/field type, or `None` for a by-value
/// scalar. Depends only on the syntactic type (no resolution), so it can gate a
/// binding site; [`container_binding`] adds the `inner_type`.
pub(super) fn container_storage(typ: &crate::ast::TypeExpr) -> Option<VarStorage> {
    use crate::ast::TypeExpr as TE;
    let is_map = |t: &TE| matches!(t, TE::Generic { name, .. } if name == "Map");
    Some(match typ {
        TE::Array { .. } => VarStorage::Array,
        t if is_map(t) => VarStorage::Map,
        TE::Ref { inner, .. } if is_map(inner) => VarStorage::Map,
        TE::Ref { .. } => VarStorage::Var,
        _ => return None,
    })
}

pub(super) fn container_binding(
    typ: &crate::ast::TypeExpr,
    resolved: &Type,
) -> Option<(VarStorage, Type)> {
    let storage = container_storage(typ)?;
    let inner = match storage {
        VarStorage::Array => match resolved {
            Type::Array(i) => (**i).clone(),
            Type::Ref(i) => match &**i {
                Type::Array(e) => (**e).clone(),
                _ => resolved.clone(),
            },
            _ => resolved.clone(),
        },
        // A map's `inner_type` carries the whole `Type::Map(K,V)`; a `*Map`
        // unwraps its outer ref to the same.
        VarStorage::Map | VarStorage::Var => match resolved {
            Type::Ref(i) => (**i).clone(),
            _ => resolved.clone(),
        },
        VarStorage::Buffer => resolved.clone(),
    };
    Some((storage, inner))
}

#[derive(Clone, Debug)]
pub(super) struct NodeRecord {
    pub(super) node_id: NodeId,
    #[allow(dead_code)]
    pub(super) ty: Type,
}

#[derive(Clone, Debug)]
pub(super) struct LocalRecord {
    pub(super) port: PortRef,
}

#[derive(Clone, Debug)]
pub(super) enum Binding {
    Var(VarRecord),
    Local(LocalRecord),
    Buffer(NodeRecord),
    Input(NodeRecord),
    Output(NodeRecord),
    EventParam(PortRef),
    Chip(std::sync::Arc<ChipDecl>),
    Namespace(HashMap<String, std::sync::Arc<ChipDecl>>),
    Record(HashMap<crate::intern::Sym, Binding>),
}

/// One active generic-mod inline: the callee's type-param names and the
/// substitution (`Type::Param → concrete`) inferred from this call's args.
/// Pushed on `LowerCtx::mono_stack` while the generic mod's body is lowered so
/// that `T`-typed storage/return annotations in the body resolve to the
/// concrete monomorph (`pick<int>` → int gates) instead of leaking `Type::Param`
/// (or a wrong `Any`/last-combo type) to emit. Nested generic inlines push/pop;
/// the innermost frame (`mono_stack.last()`) governs annotation resolution.
#[derive(Clone, Debug)]
pub(super) struct MonoFrame {
    pub(super) params: Vec<String>,
    pub(super) subst: crate::types::infer::Subst,
}

/// Scope key for an `Output` binding. Outputs share scope frames with value
/// bindings (inline-mod MODULE frames must isolate them), but live under a
/// key no identifier can collide with (`:` can't appear in an identifier) so
/// `out X = X` can still read a var/array named `X` — a same-name output used
/// to clobber the var's slot and the init expr lowered to `_Unsupported`.
pub(super) fn output_scope_key(name: &str) -> String {
    format!("out:{name}")
}

pub(super) struct LowerCtx<'a> {
    pub(super) builder: ModuleBuilder,
    pub(super) ids: IdAllocator,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) type_of_expr: &'a HashMap<(Arc<str>, usize, usize), Type>,
    pub(super) op_resolutions: &'a HashMap<(Arc<str>, usize, usize), OpRule>,
    /// Inferred custom-event data-slot types — see `LowerInput::ce_slots`.
    pub(super) ce_slots: &'a CeSlotMap,
    pub(super) file: String,
    pub(super) scope: crate::scope::Scope<Binding>,
    pub(super) handler_end_execs: Vec<PortRef>,
    pub(super) current_exec: Option<PortRef>,
    pub(super) handler_entry_exec: Option<PortRef>,
    pub(super) captured_events: HashMap<String, PortRef>,
    pub(super) next_chain_id: u32,
    pub(super) next_scope_id: ScopeId,
    /// When inside an anonymous chip body, nodes get tagged with this
    /// chip_id so the emitter routes them to a child grid.
    pub(super) current_anon_chip: Option<NodeId>,
    /// Accumulated exec from `return` statements inside an inlined mod.
    /// Each return merges into this via chained union gates.
    pub(super) mod_return_exec: Option<PortRef>,
    /// For mods with a single output: the PseudoVar node that holds the
    /// return value. Each `return expr` writes to this var via Var_Set.
    pub(super) mod_return_var: Option<VarRecord>,
    /// Type alias map: `Name → TypeExpr::Record { ... }` for dissolving
    /// record params at chip boundaries.
    pub(super) type_aliases: HashMap<String, crate::ast::TypeExpr>,
    /// Generic type aliases: `Name → (params, body TypeExpr)` for
    /// `type Pair<T> = { … }`. Instantiated by TypeExpr-level substitution
    /// (`Pair<int>` → `{ a: int, b: int }`) so a generic-alias record
    /// annotation dissolves into per-field sub-ports exactly like a
    /// non-generic `type P = { … }` — without this, a generic-alias record
    /// port/param silently degraded to a single `any` port and its field
    /// accesses lowered to `_Unsupported`/swizzle gates.
    pub(super) generic_type_aliases: HashMap<String, crate::types::resolve::GenericAlias>,
    /// Pending emit exec paths per output name, each tagged with the exec
    /// chain (handler) it was emitted on. Accumulated during lowering, flushed
    /// to union chains at the end so each output gets one wire. The chain tag
    /// lets flush route same-chain emits through an `await`'s arm (sequenced
    /// before the hub — a parallel arm races the awaiting `Var_Get`).
    pub(super) pending_emits: HashMap<String, Vec<(PortRef, Option<u32>)>>,
    /// Outputs value-driven from more than one `emit out = v` site get a backing
    /// PseudoVar (name → its var record): each site does a `Var_Set` into it and
    /// the var's value feeds the output once (no fan-in). Populated by a pre-scan
    /// before handler lowering; empty for single-site outputs and chip bodies.
    pub(super) output_backing_vars: HashMap<String, VarRecord>,
    /// Local `let x: exec` signals declared with a stable Union "hub" gate,
    /// keyed by a per-declaration unique key (`name#hubId`) — NOT the bare
    /// name, so two mods/handlers declaring the same signal name get separate
    /// signals. `on x` triggers off the hub's `ExecOut` via the scope binding;
    /// at flush the union of every `emit x` is wired into the hub. The hub
    /// gives a forward-referenceable trigger port so `on x` works regardless
    /// of whether it appears before or after the emits in source.
    pub(super) exec_signal_hubs: HashMap<String, NodeId>,
    /// Reverse map: hub node → its unique signal key. Emit/await sites resolve
    /// a surface name to its key through the *scope* (name → hub port → key),
    /// so shadowed / same-named signals in different bodies stay distinct.
    pub(super) exec_signal_keys: HashMap<NodeId, String>,
    /// Inside `await`, the armed flag's Value port. `_` in the exec expression
    /// resolves to this, allowing `await Sleep(_, 1.0)` to wire the armed flag
    /// as Sleep's input.
    pub(super) await_armed_port: Option<PortRef>,
    /// Unconditional `await <signal>` per local exec signal: the armed flag's
    /// var node and the chain the await sits on. At flush, emits of the signal
    /// from the *same* chain are routed through a `Var_Set(armed = true)` into
    /// the hub — sequencing the arm before the union so the awaiting `Var_Get`
    /// can't race it (and so loop back-edges re-arm each iteration). Emits from
    /// other chains stay direct, guarded by the flag. Only awaits at branch
    /// depth 0 register here: a conditional `await` (inside `if`) keeps pure
    /// flag semantics, since its arm must not fire for the untaken branch.
    pub(super) signal_awaits: HashMap<String, (NodeId, Option<u32>)>,
    /// Depth of enclosing exec `if` branches; >0 means conditionally executed.
    pub(super) exec_branch_depth: usize,
    /// Hidden payload stores per local exec signal: `(field, store var, type)`.
    /// `emit sig = expr` writes the store(s) on the emit chain (before any
    /// buffer), and `await sig` reads them back on the resumed chain — the
    /// value crosses the tick through the persistent var, not the buffer.
    /// A scalar payload uses one entry with field `""`; a record payload gets
    /// one entry per field.
    pub(super) exec_signal_payloads: HashMap<String, Vec<(String, NodeId, Type)>>,
    /// Pre-compiled template cache for standalone chip instances.
    pub(super) template_cache: Arc<crate::template_cache::TemplateCache>,
    /// Field→source-port record produced by the most recent multi-output inline
    /// mod call. Its internal output nodes are removed, so `let s = mod(...)`
    /// consumes this to bind `s` as a record (`s.field` reads the source port).
    pub(super) pending_inline_record: Option<HashMap<crate::intern::Sym, Binding>>,
    /// Field→binding map stashed by a `return { ... }` record literal while an
    /// inline mod body is being lowered. A record literal is not a standalone
    /// expression, and `-> { a, b }` is a single record-typed output, so the
    /// call consumes this (instead of the removed output node) to bind the
    /// caller's record. Set only by a record-literal return; taken by the call.
    pub(super) pending_return_record: Option<HashMap<crate::intern::Sym, Binding>>,
    /// Source ranges of chips/mods whose bodies are being lowered on the current
    /// call path (child contexts inherit a copy). Every call is expanded into the
    /// wire graph at compile time, so a body that (transitively) calls itself
    /// would rebuild forever — `lower_chip_call` checks this stack and emits
    /// WS020 instead of recursing. Keyed on the decl's range (unique per decl,
    /// includes the file), NOT its name, so two distinct same-named mods — e.g. a
    /// local `drawCard` and one imported from another module — aren't conflated.
    pub(super) chip_call_stack: Vec<crate::diagnostic::SourceRange>,
    /// Every chip/mod name declared anywhere in the (import-merged) program.
    /// Chips/mods are registered into scope in source order, so a call to one
    /// not yet registered fails to resolve. Distinguishing "declared later"
    /// (a use-before-declaration → WS021 error) from "not a function at all"
    /// (e.g. an unimplemented builtin → placeholder) needs the full name set.
    /// Shared (Arc) so child contexts clone it cheaply.
    pub(super) known_fn_names: std::sync::Arc<crate::collections::HashSet<String>>,
    /// Top-level `let` constants of the (import-merged) program, so a `var` /
    /// `var … : T[]` initializer can name one (`1 << C_FLAG`) instead of restating its
    /// value. Shared (Arc) so child contexts clone it cheaply.
    pub(super) const_env: std::sync::Arc<super::predeclare::ConstEnv>,
    /// Every TOP-LEVEL name declared with the `const` keyword — see
    /// `predeclare::build_const_declared_names` and
    /// `TypeCheckCtx::const_declared`'s matching doc comment. Shared (Arc)
    /// so child contexts clone it cheaply, same as `const_env`; a chip
    /// instantiation that merges `const` PARAMETERS into its child's
    /// `const_env` (see `lower::call::instance_body`) extends this set the
    /// same way.
    pub(super) const_declared: std::sync::Arc<crate::collections::HashSet<String>>,
    /// Container gates that must never be mutated: the `Pseudo_ArrayVar` /
    /// `Pseudo_MapVar` nodes
    /// [`materialize_const_container`](super::predeclare::materialize_const_container)
    /// builds for a `const` array/map.
    ///
    /// A `const` container is BOTH a compile-time value and a runtime
    /// container, and those two are only one source of truth while nothing
    /// changes the runtime copy — `const n = xs.length()` folding to 3 while
    /// the gate holds 4 elements is the divergence this set exists to prevent.
    ///
    /// Keyed on the NODE, not the name, so the rule survives aliasing: passing
    /// a `const` table to a `ys: T[]` parameter binds the callee's `ys` to this
    /// same node, and a name-keyed check would see only the innocent-looking
    /// `ys` and let the mutation through.
    ///
    /// Not shared with a child context: a microchip body gets a fresh
    /// `IdAllocator` and a fresh `Scope`, so its node ids and its own
    /// materializations are entirely its own.
    pub(super) immutable_containers: crate::collections::HashSet<NodeId>,
    /// True only for the compiled entry file's root LowerCtx. `@side` port
    /// annotations are legal only there (WS023 elsewhere).
    pub(super) is_root_module: bool,
    /// `///` doc comments keyed by the declaration's source start offset.
    /// Consumed when stamping `DOC_TEXT` onto chip nodes.
    pub(super) doc_comments: &'a HashMap<usize, String>,
    /// Depth of enclosing `@nofold` declaration subtrees. >0 means every gate
    /// created by `add_gate`/`add_event` gets stamped with the `_nofold`
    /// pseudo-property. Incremented/decremented around the lowering of each
    /// `@nofold`-annotated `let`/`out`/`var`/`chip`/`on` declaration.
    pub(super) nofold_depth: u32,
    /// Stack of active generic-mod inlines (see [`MonoFrame`]). Empty at the
    /// top level and inside every non-generic body, so `resolve_local_type`
    /// takes the byte-identical `type_of_type_expr` fast path there — only a
    /// generic mod's own body sees a non-empty stack.
    pub(super) mono_stack: Vec<MonoFrame>,
    /// Scoped constant `let` bindings, one frame per currently-open
    /// `ctx.scope` frame (a FRAME STACK mirroring `scope` 1:1 — every
    /// `push_scope`/`pop_scope` pair pushes/pops both together, mirroring
    /// `typecheck::TypeCheckCtx::scoped_consts`). A body-local
    /// `let name = <constant>` (inside a handler/mod/if/block) records
    /// `name -> Literal` in the top frame here, so a constant-only config arg
    /// (see `const_lookup`) can resolve it the same way a top-level `let`
    /// resolves through `const_env`.
    pub(super) scoped_consts: Vec<HashMap<String, Literal>>,
    /// `const_declared`'s scoped counterpart, mirroring
    /// `typecheck::TypeCheckCtx::scoped_const_declared` 1:1: which
    /// `scoped_consts` entries, in the SAME frame, were bound with `const`
    /// rather than a plain `let`. A name present in a `scoped_consts` frame
    /// but absent from this set at that frame is a plain `let` — still
    /// resolvable through the ordinary `const_lookup`, but excluded by
    /// `const_lookup_declared_only` (the `if`-condition elision's own
    /// environment; see `lower_if`).
    pub(super) scoped_const_declared: Vec<HashSet<String>>,
    /// Source ranges of `if`/`else` blocks a const-evaluable condition
    /// dropped (`lower_if`'s const-elision path) — never lowered, so no gate
    /// exists for them at all. Mirrors `typecheck::TypeCheckCtx`'s own
    /// `dropped_ranges`; the two MUST agree on exactly which ranges they
    /// drop (see `typecheck_and_lowering_drop_exactly_the_same_ranges` in
    /// `typecheck::tests`) — a disagreement means code gets type-checked but
    /// not lowered, or lowered without ever being checked.
    pub(super) dropped_ranges: Vec<SourceRange>,
    /// Chip/mod declarations visible to the BAKE path, as a FRAME STACK
    /// mirroring `scope`/`scoped_consts` 1:1 — every `push_scope`/`pop_scope`
    /// pair pushes and drops a frame here too. Populated in source order by
    /// `pre_declare_chip_name` as each pre-declare loop reaches a declaration,
    /// and consulted ONLY by `array_elem_literal`/`map_entry_literal` (via
    /// [`resolve_mod_pass1`](Self::resolve_mod_pass1)) when baking a
    /// var/array/map initializer's `const mod` CALL.
    ///
    /// **Why a frame stack and not a flat map.** A nested declaration must win
    /// inside the body that declares it and be gone outside it. As a flat map
    /// that required a manual save/restore around every borrowed-ctx body,
    /// which is both easy to forget and easy to place wrongly — the restore
    /// sat right after pre-declaration rather than after the body was lowered,
    /// so a chip declared and INSTANTIATED inside an inlined mod cloned an
    /// already-restored map and silently resolved the outer declaration while
    /// the ordinary call path in the same chip resolved the inner one. Tying
    /// the lifetime to the scope that already exists removes the discipline
    /// (and that whole bug class) entirely.
    ///
    /// **Why not `ctx.scope` itself.** That is what the ordinary call-lowering
    /// path (`resolve_mod`) and its use-before-declaration guard (WS021,
    /// `lower/call/dispatch.rs`) read. Pass 1 runs to completion before pass 2
    /// starts, so a chip registered into `scope` during pass 1 would be visible
    /// to EVERY pass-2 call site regardless of source order, silently defeating
    /// WS021 (confirmed against
    /// `lower::tests::chip::use_before_declaration_is_ws021` while building
    /// this). Keeping a separate stack preserves "only what's declared earlier"
    /// for the bake without leaking early visibility into pass 2.
    ///
    /// Always non-empty: the base frame holds the module's top-level
    /// declarations.
    pub(super) pass1_chips: Vec<HashMap<String, std::sync::Arc<ChipDecl>>>,
}

impl<'a> LowerCtx<'a> {
    pub(super) fn alloc_chain(&mut self) -> u32 {
        let id = self.next_chain_id;
        self.next_chain_id += 1;
        id
    }

    /// The declared type of the OUTPUT port `src` refers to — the type of the
    /// value that flows out of it.
    ///
    /// Used to give a port whose catalog type is `any` the type of whatever is
    /// actually wired into it, so the emitted wire variant matches. Recurses
    /// into chips because the source may live in a nested module; NodeIds come
    /// from one allocator and are globally unique.
    pub(super) fn source_port_type(&self, src: PortRef) -> Option<Type> {
        fn find_node(m: &crate::ir::Module, id: NodeId) -> Option<&crate::ir::Node> {
            if let Some(n) = m.nodes.get(&id) {
                return Some(n);
            }
            m.chips.values().find_map(|c| find_node(c, id))
        }
        let n = find_node(&self.builder.module, src.node_id)?;
        n.ports
            .find_output(intern(src.port.as_str()))
            .map(|p| p.ty.clone())
    }

    /// Resolve a surface name to its exec-signal key, via the scope binding
    /// (name → hub port → key). `None` when the name isn't a local exec
    /// signal in the current scope.
    pub(super) fn signal_key(&self, name: &str) -> Option<String> {
        match self.scope.get(name) {
            Some(Binding::Local(l)) => self.exec_signal_keys.get(&l.port.node_id).cloned(),
            _ => None,
        }
    }

    /// Allocate a fresh `ScopeId`, record it in `module.scopes` with the
    /// given kind + range, and return it. The `parent` is taken from the
    /// builder's `current_scope_id`.
    pub(super) fn alloc_scope(&mut self, kind: ScopeKind, range: SourceRange) -> ScopeId {
        let id = self.next_scope_id;
        self.next_scope_id += 1;
        let parent = Some(self.builder.current_scope_id);
        self.builder.module.scopes.insert(
            id,
            ScopeInfo {
                kind,
                source_range: range,
                parent,
            },
        );
        id
    }

    /// Push a scope, run `f`, then restore the previous scope. Use this
    /// wrapper around any lowering call that should emit nodes under a
    /// specific scope (handler body, chip body, if branches, blocks, ...).
    ///
    /// Also pushes/pops a name-resolution `scope`/`scoped_consts` frame (via
    /// `push_scope`/`pop_scope`) around `f`, so every handler body — every
    /// trigger kind routes through this wrapper, not just the built-in-event
    /// path — gets its own BLOCK frame. Without this, a body-local `let`
    /// declared inside e.g. an `in`-port-triggered handler (`on go { let pf =
    /// ... }`) had no frame of its own to bind into: it landed in whatever
    /// scope was ambient at the call site (module root, for a top-level
    /// handler) and was never popped. Any caller that ALSO needs to bind
    /// names visible for the duration of `f` (e.g. a handler's typed event
    /// params) must do so INSIDE the closure, after this push — not before
    /// calling `with_scope` — or those bindings leak into the outer scope
    /// instead of being cleaned up with the body.
    pub(super) fn with_scope<R>(
        &mut self,
        kind: ScopeKind,
        range: SourceRange,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let id = self.alloc_scope(kind, range);
        self.push_scope(crate::scope::ScopeTag::BLOCK);
        let out = self.enter_scope(id, f);
        self.pop_scope();
        out
    }

    /// Enter an already-allocated scope. Useful when the caller needs the
    /// `ScopeId` up front (e.g., to pass it to a child scope's `parent`).
    pub(super) fn enter_scope<R>(&mut self, id: ScopeId, f: impl FnOnce(&mut Self) -> R) -> R {
        let prev = self.builder.current_scope_id;
        self.builder.current_scope_id = id;
        let out = f(self);
        self.builder.current_scope_id = prev;
        out
    }

    /// Push a new name-resolution `scope` frame together with a matching
    /// empty `scoped_consts` frame, so the two stacks can never drift out of
    /// lockstep (mirrors `typecheck::TypeCheckCtx::push_scope`). Use this
    /// (paired with `pop_scope`) everywhere lowering enters a block scope
    /// (handler body, if branch, `BlockExpr`, inline-mod call) instead of a
    /// bare `ctx.scope.push(...)` — a bare push would leave `scoped_consts`
    /// pointing at the wrong (enclosing) frame, so a body-local `let` inside
    /// the new scope would be recorded one level too shallow, potentially
    /// leaking past its own scope's lifetime or clobbering a sibling scope's
    /// constant of the same name.
    pub(super) fn push_scope(&mut self, tag: crate::scope::ScopeTag) {
        self.scope.push(tag);
        self.scoped_consts.push(HashMap::default());
        self.scoped_const_declared.push(HashSet::default());
        self.pass1_chips.push(HashMap::default());
    }

    /// Pop the frame pushed by `push_scope`.
    pub(super) fn pop_scope(&mut self) {
        self.scope.pop();
        self.scoped_consts.pop();
        self.scoped_const_declared.pop();
        // Never pop the base frame — it holds the module's own top-level
        // declarations and must outlive every scope opened inside the module.
        if self.pass1_chips.len() > 1 {
            self.pass1_chips.pop();
        }
    }

    /// The constant environment visible at the current point: the top-level
    /// `const_env` overlaid by every currently-open `scoped_consts` frame,
    /// applied outer-to-inner so an inner scope's `let` shadows an outer
    /// scope's (and both shadow a same-named top-level constant). Mirrors
    /// `typecheck::TypeCheckCtx::const_lookup` exactly, so both stages agree
    /// on which name resolves to which literal. `const_env` is small, so
    /// cloning per lookup is cheap.
    pub(super) fn const_lookup(&self) -> HashMap<String, Literal> {
        let mut env: HashMap<String, Literal> = (*self.const_env).clone();
        for frame in &self.scoped_consts {
            for (name, lit) in frame {
                env.insert(name.clone(), lit.clone());
            }
        }
        env
    }

    /// The `ConstCtx` for `const_eval::eval_expr`, built from
    /// [`const_lookup`](Self::const_lookup) so lowering agrees with typecheck
    /// (`typecheck::TypeCheckCtx::const_ctx`) on what's constant at this
    /// point. `module_consts` is the UNMERGED top-level `const_env`: a
    /// `const mod` body is evaluated against the module's constants plus its
    /// own parameters, never the call site's scope frames (see
    /// `const_eval::interp::eval_call`).
    ///
    /// `lookup_mod` is supplied by the CALLER, built from [`resolve_mod`](Self::resolve_mod)
    /// — see `TypeCheckCtx::const_ctx`'s matching doc comment for why this
    /// method can't build and embed that closure itself.
    pub(super) fn const_ctx<'b>(
        &self,
        lookup_mod: Option<&'b dyn Fn(&str) -> Option<std::sync::Arc<ChipDecl>>>,
    ) -> crate::const_eval::ConstCtx<'b> {
        crate::const_eval::ConstCtx {
            consts: self.const_lookup(),
            module_consts: (*self.const_env).clone(),
            lookup_mod,
        }
    }

    /// [`const_lookup`](Self::const_lookup), restricted to names DECLARED
    /// `const` — mirrors `typecheck::TypeCheckCtx::const_lookup_declared_only`
    /// exactly, including the outer-to-inner promote/evict shadowing. Built
    /// from `const_declared`/`scoped_const_declared` rather than
    /// `const_env`/`scoped_consts` directly.
    pub(super) fn const_lookup_declared_only(&self) -> HashMap<String, Literal> {
        let mut env: HashMap<String, Literal> = HashMap::default();
        for name in self.const_declared.iter() {
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

    /// Whether `name` currently reads as a DECLARED compile-time constant —
    /// presence in [`const_lookup_declared_only`](Self::const_lookup_declared_only),
    /// answered without materialising that whole map, and walking the frames
    /// outer-to-inner with the identical promote/evict rule so the two can
    /// never disagree.
    ///
    /// `lower_let_decl` uses it to confine its shadow-clearing to names a
    /// `const` actually introduced: a program that uses no `const` keyword has
    /// every `scoped_const_declared` frame empty, so this is always `false`
    /// there and that program's lowering is untouched — the feature's own
    /// first rule.
    pub(super) fn is_declared_const(&self, name: &str) -> bool {
        let mut declared = self.const_declared.contains(name) && self.const_env.contains_key(name);
        for (frame, marks) in self.scoped_consts.iter().zip(self.scoped_const_declared.iter()) {
            if frame.contains_key(name) {
                declared = marks.contains(name);
            }
        }
        declared
    }

    /// The `Literal::Array` / `Literal::Map` a `const`-DECLARED `name` holds,
    /// or `None` for anything else (a scalar constant, a plain `let` that
    /// merely happens to fold, an unknown name).
    ///
    /// `is_declared_const` is the gate rather than a bare `const_lookup` hit,
    /// so a program that uses no `const` keyword is untouched: its
    /// `const_declared` set and every `scoped_const_declared` frame are empty,
    /// so this is always `None` there.
    pub(super) fn const_container_literal(&self, name: &str) -> Option<Literal> {
        if !self.is_declared_const(name) {
            return None;
        }
        match self.const_lookup().get(name) {
            Some(lit @ (Literal::Array(_) | Literal::Map(_))) => Some(lit.clone()),
            _ => None,
        }
    }

    /// [`const_ctx`](Self::const_ctx), built from
    /// [`const_lookup_declared_only`](Self::const_lookup_declared_only) — the
    /// `if`-condition elision's own environment (`lower_if`). A plain `let`
    /// that merely happens to fold must not participate in the generalised
    /// const-eval elision: the feature's own rule is that a program using no
    /// `const` compiles identically to before. Mirrors
    /// `typecheck::TypeCheckCtx::if_cond_const_ctx` (which additionally
    /// strips placeholders — lowering has no placeholder concept, so that
    /// step is absent here).
    pub(super) fn if_cond_const_ctx<'b>(
        &self,
        lookup_mod: Option<&'b dyn Fn(&str) -> Option<std::sync::Arc<ChipDecl>>>,
    ) -> crate::const_eval::ConstCtx<'b> {
        crate::const_eval::ConstCtx {
            consts: self.const_lookup_declared_only(),
            module_consts: (*self.const_env).clone(),
            lookup_mod,
        }
    }
    /// Resolve `name` to its `ChipDecl` for a `const mod` CALL, via the SAME
    /// scope-based resolution an ordinary (non-const) call already goes
    /// through: `lookup_chip` only finds a chip/mod whose `lower_chip_decl`
    /// has already run (lowering processes decls in source order, unlike
    /// typecheck's two-pass forward-reference-tolerant registration), so a
    /// const-mod call resolves here exactly as far "ahead" as any other call
    /// to it would at this point in lowering.
    pub(super) fn resolve_mod(&self, name: &str) -> Option<std::sync::Arc<ChipDecl>> {
        self.lookup_chip(name).cloned()
    }

    /// [`resolve_mod`](Self::resolve_mod)'s bake-path counterpart — see
    /// [`pass1_chips`](Self::pass1_chips)'s doc comment for why baking a
    /// var/array/map initializer's `const mod` CALL cannot simply call
    /// `resolve_mod` (`ctx.scope`) here.
    ///
    /// Scans frames INNERMOST-first, so a body-local declaration shadows a
    /// same-named outer one for exactly as long as its scope is open — the
    /// same shadowing `const_lookup` gives scoped constants.
    pub(super) fn resolve_mod_pass1(&self, name: &str) -> Option<std::sync::Arc<ChipDecl>> {
        self.pass1_chips
            .iter()
            .rev()
            .find_map(|frame| frame.get(name))
            .cloned()
    }

    pub(super) fn type_of(&self, e: &Expr) -> Type {
        let r = e.range();
        self.type_of_expr
            .get(&(r.file.clone(), r.start.offset, r.end.offset))
            .cloned()
            .unwrap_or(Type::Any)
    }

    /// Resolve a lowering-side type annotation to its `Type`, monomorphized
    /// against the innermost active generic-mod inline. Outside any generic
    /// body (`mono_stack` empty) this is exactly `type_of_type_expr` — so
    /// non-generic lowering is byte-identical. Inside a generic mod's body, the
    /// callee's type params are put in scope so a `T` annotation resolves to
    /// `Type::Param(T)`, then the call's substitution replaces it with the
    /// concrete monomorph (`T` → `int` / `vector`). Without this, `T` resolved
    /// to `Any` (empty-params `type_of_type_expr`), silently emitting a wrong
    /// (Number-defaulted) variant for every generic storage/return gate.
    pub(super) fn resolve_local_type(&self, te: &crate::ast::TypeExpr) -> Type {
        // Generic aliases resolve on BOTH paths (a `Pair<int>` annotation must
        // become its record `Type`, not `Any`); non-generic name aliases stay
        // empty here, matching `type_of_type_expr`'s long-standing behavior.
        let empty: crate::collections::HashMap<String, Type> = crate::collections::HashMap::default();
        let params: &[String] = self
            .mono_stack
            .last()
            .map(|f| f.params.as_slice())
            .unwrap_or(&[]);
        let cx = crate::types::resolve::ResolveCtx {
            params,
            type_aliases: &empty,
            generic_aliases: &self.generic_type_aliases,
        };
        let resolved = crate::types::resolve::resolve_type(te, &cx, &mut Vec::new());
        match self.mono_stack.last() {
            Some(frame) => crate::types::mono::substitute(&resolved, &frame.subst),
            None => resolved,
        }
    }

    /// Expand a record-shaped type annotation into its fields, following a
    /// non-generic alias (`type P = { … }`) or instantiating a generic alias
    /// (`type Pair<T> = { … }` used as `Pair<int>`, substituting the type
    /// args). Returns `None` for a non-record type. The single source of
    /// record-port dissolution for chip inputs, mod params, and top-level
    /// ports — every such site routes through here so the `Generic` arm can't
    /// be forgotten at one of them.
    pub(super) fn record_fields_of(
        &self,
        te: &crate::ast::TypeExpr,
    ) -> Option<Vec<crate::ast::RecordTypeField>> {
        match te {
            crate::ast::TypeExpr::Record { fields, .. } => Some(fields.clone()),
            crate::ast::TypeExpr::Name { name, .. } => match self.type_aliases.get(name.as_str())? {
                crate::ast::TypeExpr::Record { fields, .. } => Some(fields.clone()),
                _ => None,
            },
            crate::ast::TypeExpr::Generic { name, args, .. } => {
                match crate::types::resolve::instantiate_generic_alias(
                    name,
                    args,
                    &self.generic_type_aliases,
                )? {
                    crate::ast::TypeExpr::Record { fields, .. } => Some(fields),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn op_for(&self, e: &Expr) -> Option<&OpRule> {
        let r = e.range();
        self.op_resolutions
            .get(&(r.file.clone(), r.start.offset, r.end.offset))
    }

    pub(super) fn add_gate(&mut self, mut opts: AddNodeOpts) -> NodeId {
        if opts.chip_id.is_none() {
            opts.chip_id = self.current_anon_chip;
        }
        if self.nofold_depth > 0 {
            opts.properties.insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        self.builder.add_gate(&mut self.ids, opts)
    }

    pub(super) fn add_event(&mut self, mut opts: AddNodeOpts) -> NodeId {
        if opts.chip_id.is_none() {
            opts.chip_id = self.current_anon_chip;
        }
        if self.nofold_depth > 0 {
            opts.properties.insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        self.builder.add_event(&mut self.ids, opts)
    }

    /// Wraps `ModuleBuilder::add_input` so a chip's boundary `MicrochipInput`
    /// rerouter node gets stamped with `_nofold` the same way `add_gate`/
    /// `add_event` stamp regular gates — `add_input`/`add_output` live on
    /// `ModuleBuilder`, not `LowerCtx`, so they don't see `nofold_depth`
    /// unless called through here.
    pub(super) fn add_input(&mut self, port_name: &str, ty: Type, source_range: SourceRange) -> NodeId {
        let id = self.builder.add_input(&mut self.ids, port_name, ty, source_range);
        if self.nofold_depth > 0
            && let Some(node) = self.builder.module.nodes.get_mut(&id)
        {
            std::sync::Arc::make_mut(&mut node.properties).insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        id
    }

    /// Wraps `ModuleBuilder::add_output` — see `add_input` above.
    pub(super) fn add_output(&mut self, port_name: &str, ty: Type, source_range: SourceRange) -> NodeId {
        let id = self.builder.add_output(&mut self.ids, port_name, ty, source_range);
        if self.nofold_depth > 0
            && let Some(node) = self.builder.module.nodes.get_mut(&id)
        {
            std::sync::Arc::make_mut(&mut node.properties).insert(*sym::NO_FOLD, Literal::Bool(true));
        }
        id
    }

    pub(super) fn connect(&mut self, src: PortRef, dst: PortRef) {
        // Language-level string → bool coercion: a string-typed source wired
        // into a bool-typed destination routes through an inserted
        // `CompareNotEqual(src, "")` gate, so the coercion means exactly
        // `src != ""` — empty is false, EVERYTHING else (including "0" and
        // "false") is true. The game's native bool ports would otherwise
        // apply their content-aware truthiness ("0"/"false" falsy but "0.0"
        // truthy — see the Branch/Select/AND chapters of
        // `data/gate_semantics.json`), which is a footgun the language
        // deliberately papers over. Native truthiness remains reachable by
        // wiring through `any` (`Type::Opaque` erases the String port type,
        // so this interception never sees it) or via the logical operators,
        // whose string overloads are native gate behavior, not coercions.
        if let Some(coerced) = self.wrap_string_to_bool(src, dst) {
            self.builder.connect(coerced, dst);
            return;
        }
        self.builder.connect(src, dst);
    }

    /// If `src` is a string-typed output and `dst` a bool-typed input,
    /// build the `!= ""` coercion gate (`CompareNotEqual` with `InputB`
    /// baked to the empty string — compare inputs are wire-variant fields,
    /// so the constant inlines as data) and return its `bOutput` for the
    /// caller to wire into `dst`. `None` when no coercion applies (either
    /// side untyped/not found, or any other type pair). NOTE: the inserted
    /// gate does NOT constant-fold — `CompareNotEqual` has no certified
    /// (str, str) signature in `data/gate_semantics.json`, so the fold pass
    /// refuses it (correct-but-unoptimized until that signature is probed).
    fn wrap_string_to_bool(&mut self, src: PortRef, dst: PortRef) -> Option<PortRef> {
        // A boundary node of an already-instantiated chip lives in a nested
        // `module.chips` entry, not the top-level node map (e.g. wiring a
        // string call argument into a chip's bool-typed `MicrochipInput`
        // pin), so the lookup recurses. NodeIds come from one allocator and
        // are globally unique.
        fn find_node(m: &crate::ir::Module, id: NodeId) -> Option<&crate::ir::Node> {
            if let Some(n) = m.nodes.get(&id) {
                return Some(n);
            }
            m.chips.values().find_map(|c| find_node(c, id))
        }
        let src_range = {
            let n = find_node(&self.builder.module, src.node_id)?;
            if n.ports.find_output(intern(src.port.as_str()))?.ty != Type::String {
                return None;
            }
            n.source_range.clone()
        };
        {
            let n = find_node(&self.builder.module, dst.node_id)?;
            if n.ports.find_input(intern(dst.port.as_str()))?.ty != Type::Bool {
                return None;
            }
        }
        let mut props = HashMap::default();
        props.insert(*sym::INPUT_B, Literal::String(String::new()));
        let ne = self.add_gate(AddNodeOpts {
            gate_class: gc::COMPARE_NOT_EQUAL,
            source_range: src_range,
            ports: GateIO {
                inputs: vec![
                    PortSpec {
                        name: *sym::INPUT_A,
                        ty: Type::String,
                    },
                    PortSpec {
                        name: *sym::INPUT_B,
                        ty: Type::String,
                    },
                ],
                outputs: vec![PortSpec {
                    name: *sym::B_OUTPUT,
                    ty: Type::Bool,
                }],
            },
            properties: props,
            note: Some("string→bool coercion (!= \"\")"),
            ..Default::default()
        });
        self.builder.connect(src, ne.port(WirePort::InputA));
        Some(ne.port(WirePort::BOutput))
    }

    /// Run `f` with `nofold_depth` incremented for its duration when `active`
    /// is true (the enclosing declaration was `@nofold`-annotated). Every
    /// gate/event `f` creates — directly or through nested lowering calls —
    /// gets the `_nofold` pseudo-property stamped by `add_gate`/`add_event`.
    /// Safe against early `return`s inside `f`: a `return` inside a closure
    /// only unwinds the closure, so the decrement below always runs.
    pub(super) fn with_nofold<R>(
        &mut self,
        active: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if active {
            self.nofold_depth += 1;
        }
        let r = f(self);
        if active {
            self.nofold_depth -= 1;
        }
        r
    }

    pub(super) fn warn(&mut self, msg: impl Into<String>, range: &SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Warning,
            code: "WSP001".into(),
            message: msg.into(),
            range: range.clone(),
        });
    }

    pub(super) fn error(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        range: &SourceRange,
    ) {
        self.diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Error,
            code: code.into(),
            message: msg.into(),
            range: range.clone(),
        });
    }

    pub(super) fn lookup_var(&self, name: &str) -> Option<&VarRecord> {
        match self.scope.get(name) {
            Some(Binding::Var(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_var_mut(&mut self, name: &str) -> Option<&mut VarRecord> {
        match self.scope.get_mut(name) {
            Some(Binding::Var(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_local(&self, name: &str) -> Option<&LocalRecord> {
        match self.scope.get(name) {
            Some(Binding::Local(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_buffer(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(name) {
            Some(Binding::Buffer(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_input(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(name) {
            Some(Binding::Input(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn lookup_output(&self, name: &str) -> Option<&NodeRecord> {
        match self.scope.get(&output_scope_key(name)) {
            Some(Binding::Output(r)) => Some(r),
            _ => None,
        }
    }

    pub(super) fn output_count(&self) -> usize {
        self.scope
            .iter_within(crate::scope::ScopeTag::MODULE)
            .filter(|(_, b)| matches!(b, Binding::Output(_)))
            .count()
    }

    pub(super) fn first_output(&self) -> Option<(&str, &NodeRecord)> {
        self.scope
            .iter_within(crate::scope::ScopeTag::MODULE)
            .find_map(|(k, b)| match b {
                Binding::Output(r) => Some((k, r)),
                _ => None,
            })
    }

    pub(super) fn lookup_chip(&self, name: &str) -> Option<&std::sync::Arc<ChipDecl>> {
        match self.scope.get(name) {
            Some(Binding::Chip(c)) => Some(c),
            _ => None,
        }
    }

    pub(super) fn lookup_ns_chip(&self, ns: &str, name: &str) -> Option<&std::sync::Arc<ChipDecl>> {
        match self.scope.get(ns) {
            Some(Binding::Namespace(members)) => members.get(name),
            _ => None,
        }
    }
}

pub(super) fn reset_var_get_caches(ctx: &mut LowerCtx) {
    for binding in ctx.scope.values_mut() {
        reset_cache_in_binding(binding);
    }
}

fn reset_cache_in_binding(binding: &mut Binding) {
    match binding {
        Binding::Var(v) => {
            v.get_node_for_handler = None;
        }
        Binding::Record(fields) => {
            for b in fields.values_mut() {
                reset_cache_in_binding(b);
            }
        }
        _ => {}
    }
}

pub(super) fn invalidate_var_cache(ctx: &mut LowerCtx, target_node_id: &NodeId) {
    for binding in ctx.scope.values_mut() {
        invalidate_cache_in_binding(binding, target_node_id);
    }
}

fn invalidate_cache_in_binding(binding: &mut Binding, target_node_id: &NodeId) {
    match binding {
        Binding::Var(v) => {
            if v.node_id == *target_node_id {
                v.get_node_for_handler = None;
            }
        }
        Binding::Record(fields) => {
            for b in fields.values_mut() {
                invalidate_cache_in_binding(b, target_node_id);
            }
        }
        _ => {}
    }
}

/// A snapshot of every in-scope var's `Var_Get` cache (recursing into record
/// bindings), keyed by the var's gate node id. `lower_if` takes one before its
/// branches so that, at the join, only the vars a branch actually TOUCHED are
/// invalidated — an unwritten var's pre-branch read survives, instead of the
/// whole cache being blanket-cleared.
pub(super) type VarCacheSnapshot = HashMap<NodeId, Option<NodeId>>;

pub(super) fn snapshot_var_caches(ctx: &LowerCtx) -> VarCacheSnapshot {
    let mut out = VarCacheSnapshot::default();
    for (_, binding) in ctx.scope.iter() {
        snapshot_cache_in_binding(binding, &mut out);
    }
    out
}

fn snapshot_cache_in_binding(binding: &Binding, out: &mut VarCacheSnapshot) {
    match binding {
        Binding::Var(v) => {
            out.insert(v.node_id, v.get_node_for_handler);
        }
        Binding::Record(fields) => {
            for b in fields.values() {
                snapshot_cache_in_binding(b, out);
            }
        }
        _ => {}
    }
}

/// The var node ids whose `Var_Get` cache CHANGED versus `snap` — every var a
/// branch invalidated, no matter which code path cleared it (a direct write, a
/// nested `if`'s join, an inline mod's blanket reset, or a chip instance's
/// ref-arg reset). Comparing actual cache state — rather than trusting each
/// write site to journal — is what makes the branch merge safe: a stale
/// pre-branch read can only survive when the cache is provably unchanged. A var
/// absent from `snap` (declared inside the branch) is ignored — it's
/// block-scoped and gone at the join.
pub(super) fn cache_touched_since(ctx: &LowerCtx, snap: &VarCacheSnapshot) -> HashSet<NodeId> {
    let mut out = HashSet::default();
    for (_, binding) in ctx.scope.iter() {
        cache_touched_in_binding(binding, snap, &mut out);
    }
    out
}

fn cache_touched_in_binding(binding: &Binding, snap: &VarCacheSnapshot, out: &mut HashSet<NodeId>) {
    match binding {
        Binding::Var(v) => {
            if let Some(&pre) = snap.get(&v.node_id)
                && pre != v.get_node_for_handler
            {
                out.insert(v.node_id);
            }
        }
        Binding::Record(fields) => {
            for b in fields.values() {
                cache_touched_in_binding(b, snap, out);
            }
        }
        _ => {}
    }
}

/// Reset every in-scope var's `Var_Get` cache back to `snap` — used to discard a
/// branch's scratch cache state (its own fresh reads live on that branch's exec
/// chain and can't be reused elsewhere) before lowering the next branch and
/// after the join.
pub(super) fn restore_var_caches(ctx: &mut LowerCtx, snap: &VarCacheSnapshot) {
    for binding in ctx.scope.values_mut() {
        restore_cache_in_binding(binding, snap);
    }
}

fn restore_cache_in_binding(binding: &mut Binding, snap: &VarCacheSnapshot) {
    match binding {
        Binding::Var(v) => {
            if let Some(&pre) = snap.get(&v.node_id) {
                v.get_node_for_handler = pre;
            }
        }
        Binding::Record(fields) => {
            for b in fields.values_mut() {
                restore_cache_in_binding(b, snap);
            }
        }
        _ => {}
    }
}
