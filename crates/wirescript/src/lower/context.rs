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
    Chip(Box<ChipDecl>),
    Namespace(HashMap<String, ChipDecl>),
    Record(HashMap<crate::intern::Sym, Binding>),
}

pub(super) struct LowerCtx<'a> {
    pub(super) builder: ModuleBuilder,
    pub(super) ids: IdAllocator,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) type_of_expr: &'a HashMap<(Arc<str>, usize, usize), Type>,
    pub(super) op_resolutions: &'a HashMap<(Arc<str>, usize, usize), OpRule>,
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
    /// Pending emit exec paths per output name, each tagged with the exec
    /// chain (handler) it was emitted on. Accumulated during lowering, flushed
    /// to union chains at the end so each output gets one wire. The chain tag
    /// lets flush route same-chain emits through an `await`'s arm (sequenced
    /// before the hub — a parallel arm races the awaiting `Var_Get`).
    pub(super) pending_emits: HashMap<String, Vec<(PortRef, Option<u32>)>>,
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
    /// Source ranges of chips/mods whose bodies are being lowered on the current
    /// call path (child contexts inherit a copy). Every call is expanded into the
    /// wire graph at compile time, so a body that (transitively) calls itself
    /// would rebuild forever — `lower_chip_call` checks this stack and emits
    /// WS020 instead of recursing. Keyed on the decl's range (unique per decl,
    /// includes the file), NOT its name, so two distinct same-named mods — e.g. a
    /// local `drawCard` and one imported from another module — aren't conflated.
    pub(super) chip_call_stack: Vec<crate::diagnostic::SourceRange>,
}

impl<'a> LowerCtx<'a> {
    pub(super) fn alloc_chain(&mut self) -> u32 {
        let id = self.next_chain_id;
        self.next_chain_id += 1;
        id
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
    pub(super) fn with_scope<R>(
        &mut self,
        kind: ScopeKind,
        range: SourceRange,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let id = self.alloc_scope(kind, range);
        self.enter_scope(id, f)
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

    pub(super) fn type_of(&self, e: &Expr) -> Type {
        let r = e.range();
        self.type_of_expr
            .get(&(r.file.clone(), r.start.offset, r.end.offset))
            .cloned()
            .unwrap_or(Type::Any)
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
        self.builder.add_gate(&mut self.ids, opts)
    }

    pub(super) fn add_event(&mut self, mut opts: AddNodeOpts) -> NodeId {
        if opts.chip_id.is_none() {
            opts.chip_id = self.current_anon_chip;
        }
        self.builder.add_event(&mut self.ids, opts)
    }

    pub(super) fn connect(&mut self, src: PortRef, dst: PortRef) {
        self.builder.connect(src, dst);
    }

    pub(super) fn warn(&mut self, msg: impl Into<String>, range: &SourceRange) {
        self.diagnostics.push(Diagnostic {
            severity: crate::diagnostic::Severity::Warning,
            code: "WSP001".into(),
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
        match self.scope.get(name) {
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

    pub(super) fn lookup_chip(&self, name: &str) -> Option<&ChipDecl> {
        match self.scope.get(name) {
            Some(Binding::Chip(c)) => Some(c),
            _ => None,
        }
    }

    pub(super) fn lookup_ns_chip(&self, ns: &str, name: &str) -> Option<&ChipDecl> {
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
