use super::*;

// Declaration body pass

/// True if `p`'s declared output type is `string`, read off the IR node's port
/// spec. A string label value wires into the label's `Text` port directly; a
/// non-string goes through a `FormatText` gate.
fn label_source_is_string(ctx: &LowerCtx<'_>, p: PortRef) -> bool {
    ctx.builder
        .module
        .get(p.node_id)
        .and_then(|n| n.output_type(p.port))
        == Some(Type::String)
}

/// Lower a label expression to a string-typed value port: the value directly if
/// it is already a string (e.g. a string var, or an interpolation that already
/// built its own `FormatText`), otherwise coerced through a single `FormatText`.
fn lower_label_value(ctx: &mut LowerCtx<'_>, le: &Expr) -> PortRef {
    let value = lower_expr(ctx, le);
    if label_source_is_string(ctx, value) {
        value
    } else {
        build_format_text(ctx, "{0}".to_string(), vec![value], le.range())
    }
}

/// Resolve runtime `@label(expr)` on top-level `var`s into dynamic labels.
///
/// For each top-level var whose label does not fold to a constant (constant
/// labels are baked in pass 1), lower the expression to a string value port and
/// record `host_var -> source_port` in `dynamic_labels`. Emit wires each entry
/// into the var's label `Component_TextDisplay.Text` (`wirescript-ir`'s
/// `emit/module.rs`, dynamic label wires pass).
///
/// Runs after the body pass so every var/gate the label references exists, and
/// in a pure context so a var read resolves to the var's live `Value` output
/// rather than an exec-chained `Var_Get`.
pub(super) fn resolve_dynamic_var_labels(ctx: &mut LowerCtx<'_>, decls: &[TopDecl]) {
    ctx.pure(|ctx| {
        for d in decls {
            let TopDecl::Var(v) = d else { continue };
            let Some(le) = &v.label_expr else { continue };
            let view = ctx.top_view(crate::scope::top::ReadAt::code(&v.range));
            // The host var node carries the label component at emit time. A
            // constant label is baked as static text already.
            if expr_to_literal_in(le, &ctx.const_lookup()).is_none()
                && let Some(Binding::Var(rec)) = ctx.scope.get(&v.name)
            {
                let host = rec.node_id;
                let src = lower_label_value(ctx, le);
                ctx.builder.module.dynamic_labels.insert(host, src);
            }
            ctx.restore_top_view(view);
        }
    });
}

/// Resolve a module-level `@label(<expr>)` into the root microchip's label.
///
/// A constant expression folds to static title text (`root_label_override`);
/// a runtime expression is lowered (string-coerced) and recorded as
/// `root_dynamic_label`, which emit wires into the root shell's `Text` port.
/// Runs in the same post-declaration pass as [`resolve_dynamic_var_labels`], so
/// the expression may forward-reference declarations below it (hoisting).
pub(super) fn resolve_module_label(ctx: &mut LowerCtx<'_>, module_label: Option<&Expr>) {
    let Some(le) = module_label else { return };
    if let Some(text) = resolve_label_text(None, Some(le), &ctx.const_lookup()) {
        // Constant: baked static title text.
        ctx.builder.module.root_label_override = Some(text);
        return;
    }
    // Runtime value: lowered in a pure context, coerced to string only if it is
    // not one. Emit wires this into the root shell's `Text`.
    let src = ctx.pure(|ctx| lower_label_value(ctx, le));
    ctx.builder.module.root_dynamic_label = Some(src);
}

pub(super) fn lower_decl(ctx: &mut LowerCtx<'_>, d: &TopDecl) {
    match d {
        TopDecl::Out(b) => ctx.with_nofold(b.no_fold, |ctx| {
            lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range);
        }),
        // Dedup an imported handler reached through several imports of one file
        // (a plain + `import * as` pair) so its behaviour installs once, not
        // once per import. A non-imported handler is always first-seen.
        TopDecl::Handler(h) => {
            if ctx.first_import_of_behavior(&h.range) {
                ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h));
            }
        }
        TopDecl::Event(e) => ctx.with_nofold(e.no_fold, |ctx| lower_event_decl(ctx, e)),
        TopDecl::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
        TopDecl::Buffer(b) => lower_buffer_body(ctx, b),
        // Gate created in pre-pass. The top level is pure, so a non-constant
        // init has no exec reset to apply it; warn that it is dropped.
        TopDecl::Var(v) => ctx.with_nofold(v.no_fold, |ctx| {
            // Its initializer reads the declaration before it (see `scope::top`).
            let above = ctx
                .top_names
                .initializer_read(&v.name, &v.range)
                .map(|at| ctx.top_view(at));
            warn_unbaked_var_init(ctx, v, true);
            if let Some(view) = above {
                ctx.restore_top_view(view);
            }
        }),
        TopDecl::Array(_) | TopDecl::Map(_) | TopDecl::In(_) => {} // handled in pre-pass
        TopDecl::Chip(c) => lower_chip_decl(ctx, c),
        TopDecl::AnonChip(ac) => lower_anon_chip(ctx, ac),
        TopDecl::Assign(a) => lower_assign(ctx, a),
        TopDecl::If(i) => lower_if(ctx, i),
        // Shared struct with the statement forms - lower identically.
        TopDecl::IfLet(i) => lower_if_let(ctx, i),
        TopDecl::LetElse(l) => lower_let_else(ctx, l),
        TopDecl::ExprStmt(es) => lower_expr_stmt(ctx, &es.expr),
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) | TopDecl::Enum(_) => {}
        TopDecl::Namespace(ns) => lower_namespace(ctx, ns),
    }
}

/// Record the gate a freshly pre-declared namespace state member produced, so
/// a later re-import of the same source location (another `import * as`, or a
/// plain/named import of the same file) reuses it instead of duplicating.
/// Call right after pre-declaration, while the fresh binding is still the
/// member's bare name, before `route_ns_member` may restore an importer-owned
/// name over it. See [`LowerCtx::import_state_dedup`].
fn record_ns_member_state(ctx: &mut LowerCtx<'_>, range: &SourceRange, name: &str) {
    if let Some(binding) = ctx.scope.get(name).cloned() {
        ctx.record_import_state(range, binding);
    }
}

/// What lowering one `import * as` namespace gathers while it walks the
/// module's members, and holds back until every member is in scope.
#[derive(Default)]
struct NsLowering<'a> {
    /// The member map, published as the namespace's `Binding::Namespace`.
    decls: HashMap<String, Binding>,
    buffers: Vec<&'a BufferDecl>,
    outputs: Vec<&'a OutBinding>,
    /// Value members (plain idents) whose binding is captured into the map
    /// after the member loop, so `A.foo` reads A's own `foo` even when a later
    /// `import * as B` overwrites the shared bare `foo`.
    value_names: Vec<String>,
    /// Importer-owned names (state whose bare name the entry file itself
    /// declares), restored after this namespace's deferred buffer/out bodies
    /// have wired against the member's own gate. Every member is pre-declared
    /// fresh, so same-named state from two modules lands on distinct gates; the
    /// restore keeps `g` the importer's and `A.g` A's.
    restores: Vec<(String, Option<Binding>)>,
    /// Top-level `on` handlers in the imported module, lowered after the member
    /// loop so their bodies resolve this namespace's members by bare name even
    /// when another namespace exports the same names.
    handlers: Vec<&'a Handler>,
    /// Anonymous chips (`chip { ... }` / `chip on t { ... }`) in the imported
    /// module. Pass 1 never descends into `ns.decls`, so they are pre-declared
    /// and lowered at the end, while the module's members still hold the bare
    /// names.
    anon_chips: Vec<&'a AnonChipDecl>,
}

/// Lower an `import * as` namespace: pre-declare and lower its members, then
/// publish the member map under the namespace name.
fn lower_namespace(ctx: &mut LowerCtx<'_>, ns: &NamespaceDecl) {
    let mut st = NsLowering::default();
    for d in &ns.decls {
        st.member(ctx, d);
    }
    st.hoist_handler_ports(ctx);
    st.create_output_backings(ctx, ns);
    st.lower_deferred_members(ctx);
    st.lower_handlers_and_chips(ctx);
    st.restore_importer_names(ctx);
    st.publish(ctx, ns);
}

impl<'a> NsLowering<'a> {
    /// Pre-declare or lower one member of the namespace.
    fn member(&mut self, ctx: &mut LowerCtx<'_>, d: &'a TopDecl) {
        match d {
            TopDecl::Chip(c) => {
                self.decls.insert(
                    c.name.clone(),
                    Binding::Chip(std::sync::Arc::new(c.clone())),
                );
                // A namespaced mod's body calls its sibling mods by bare
                // name; register them so those calls resolve when the
                // body is inlined in the importing module.
                if ctx.scope.get(&c.name).is_none() {
                    ctx.scope
                        .insert(&c.name, Binding::Chip(std::sync::Arc::new(c.clone())));
                }
            }
            TopDecl::Let(l) => self.let_member(ctx, l),
            // Each state member is pre-declared fresh even when its bare
            // name is bound, so `B.g` never aliases `A.g`'s gate.
            TopDecl::Array(a) => {
                self.state_member(ctx, &a.range, &a.name, |ctx| pre_declare_array(ctx, a));
            }
            TopDecl::Map(m) => {
                self.state_member(ctx, &m.range, &m.name, |ctx| pre_declare_map(ctx, m));
            }
            TopDecl::Var(v) => {
                self.state_member(ctx, &v.range, &v.name, |ctx| {
                    ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
                    // Module-level = pure: a non-constant init is dropped.
                    ctx.with_nofold(v.no_fold, |ctx| warn_unbaked_var_init(ctx, v, true));
                });
            }
            TopDecl::Buffer(b) => {
                if self.state_member(ctx, &b.range, &b.name, |ctx| pre_declare_buffer(ctx, b)) {
                    self.buffers.push(b);
                }
            }
            // An imported module's `in`/`out` ports become ports of the
            // importing module's chip, as a local `in`/`out` does, and
            // are reachable both bare and as `ns.name`.
            TopDecl::In(i) => {
                self.state_member(ctx, &i.range, &i.name, |ctx| pre_declare_input(ctx, i));
            }
            TopDecl::Out(o) => self.out_member(ctx, o),
            TopDecl::Handler(h) => self.handlers.push(h),
            TopDecl::AnonChip(ac) => self.anon_chips.push(ac),
            _ => {}
        }
    }

    /// A namespaced mod inlined in the importer reads its module's `let`s by
    /// bare name, so they are lowered into the enclosing scope (an unbound read
    /// is an `_Unsupported` that reads 0). An importer-owned name is captured
    /// into this namespace's map and the importer's binding restored, so an
    /// imported `let start` never replaces a local `in start: exec`.
    fn let_member(&mut self, ctx: &mut LowerCtx<'_>, l: &LetDecl) {
        let owned_name = match &l.binding {
            crate::ast::LetBinding::Ident { name, .. } if ctx.importer_names.contains(name) => {
                Some(name.clone())
            }
            _ => None,
        };
        if let Some(name) = owned_name {
            let saved = ctx.scope.get(&name).cloned();
            ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l));
            if let Some(binding) = ctx.scope.get(&name).cloned() {
                self.decls.insert(name.clone(), binding);
            }
            if let Some(b) = saved {
                ctx.scope.insert(&name, b);
            }
        } else {
            ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l));
            if let crate::ast::LetBinding::Ident { name, .. } = &l.binding {
                self.value_names.push(name.clone());
            }
        }
    }

    /// Pre-declare a state member with `declare`, or reuse the gate a previous
    /// import of the same source location made. Returns whether it was
    /// declared fresh.
    fn state_member(
        &mut self,
        ctx: &mut LowerCtx<'_>,
        range: &SourceRange,
        name: &str,
        declare: impl FnOnce(&mut LowerCtx<'_>),
    ) -> bool {
        if let Some(existing) = ctx.reuse_import_state(range) {
            self.decls.insert(name.to_string(), existing);
            return false;
        }
        let owned = ctx.importer_names.contains(name);
        let saved = owned.then(|| ctx.scope.get(name).cloned()).flatten();
        declare(ctx);
        record_ns_member_state(ctx, range, name);
        self.route_member(ctx, name, saved, owned);
        true
    }

    /// Pre-declare an `out` member; its value is wired once every member is in
    /// scope.
    fn out_member(&mut self, ctx: &mut LowerCtx<'_>, o: &'a OutBinding) {
        let owned = ctx.importer_names.contains(&o.name);
        let saved = owned.then(|| ctx.scope.get(&o.name).cloned()).flatten();
        ctx.with_nofold(o.no_fold, |ctx| {
            pre_declare_output(
                ctx,
                &o.name,
                o.value.as_ref(),
                o.typ.as_ref(),
                o.side,
                o.label.as_deref(),
                o.label_expr.as_ref(),
                o.invisible,
                false,
                &o.range,
            );
        });
        self.outputs.push(o);
        self.route_member(ctx, &o.name, saved, owned);
        // The output's binding lives under its mangled scope key,
        // which `route_member`'s bare-name capture misses;
        // insert it here so `L.count` reads the output's value
        // instead of lowering to an `_Unsupported` placeholder.
        if let Some(binding) = ctx.scope.get(&output_scope_key(&o.name)).cloned() {
            self.decls.insert(o.name.clone(), binding);
        }
    }

    /// Route a state member's freshly pre-declared binding. `owned` is whether
    /// the entry file itself declares this bare name (`saved` is the importer's
    /// prior binding, captured before the pre-declare):
    /// - owned: capture the fresh binding into the namespace map and queue the
    ///   importer's binding for restoration once the namespace's deferred
    ///   buffer/out bodies have wired (`A.g` is A's gate, the bare `g` the
    ///   importer's);
    /// - not owned: defer capture to the end-of-loop `value_names` sweep; the
    ///   fresh binding stays the shared bare name until the next `import * as`.
    fn route_member(
        &mut self,
        ctx: &LowerCtx<'_>,
        name: &str,
        saved: Option<Binding>,
        owned: bool,
    ) {
        if owned {
            if let Some(binding) = ctx.scope.get(name).cloned() {
                self.decls.insert(name.to_string(), binding);
            }
            self.restores.push((name.to_string(), saved));
        } else {
            self.value_names.push(name.to_string());
        }
    }

    /// Hoist the boundary ports a namespaced handler declares
    /// (`on trig { out hport: int = 7 }`), as pass 1a does for the entry
    /// file, which does not walk `ns.decls`. After the member loop, so a
    /// handler port binds to an explicit `out` member of the same name
    /// instead of minting a second port. `boundary` matches the
    /// `TopDecl::Out` arm.
    fn hoist_handler_ports(&mut self, ctx: &mut LowerCtx<'_>) {
        for h in &self.handlers {
            pre_declare_handler_outputs(ctx, &h.body, false);
            // Capture into the namespace map as an `out` member does,
            // never overwriting an explicit `out` member of the same name.
            for o in crate::ast::handler_port_sites(&h.body) {
                if let Some(binding) = ctx.scope.get(&output_scope_key(&o.name)).cloned() {
                    self.decls.entry(o.name.clone()).or_insert(binding);
                }
            }
        }
    }

    /// A namespaced `out` reached by 2+ emits, a conditional emit, or a
    /// default plus an emit needs a backing var like a top-level one; the
    /// pass-1b prescan does not descend into `ns.decls`. Created after the
    /// rerouter and before the default/emit wiring, so the drivers route
    /// through the var: fan-in on the rerouter's `RER_Input` fails to load
    /// in game.
    fn create_output_backings(&self, ctx: &mut LowerCtx<'_>, ns: &NamespaceDecl) {
        // Its handlers' and anonymous chips' writes, and its mods'
        // writes once per instantiation (`output_write_counts`).
        let emit_counts = ctx
            .namespace_output_writes
            .get(&crate::lower::namespace_scope(ns))
            .cloned()
            .unwrap_or_default();
        let handler_ports: Vec<&OutBinding> = self
            .handlers
            .iter()
            .flat_map(|h| crate::ast::handler_port_sites(&h.body))
            .collect();
        // A value on an `out` member is a persistent default driver, so
        // any emit beside it fans in with it. A value on a
        // handler-declared port is its own driving site, already counted
        // by `count_emits_in_handler` (as pass 1b's `defaulted_outputs`).
        let sites = self
            .outputs
            .iter()
            .map(|o| (*o, o.value.is_some()))
            .chain(handler_ports.into_iter().map(|o| (o, false)));
        for (o, has_default) in sites {
            let (count, in_branch) = emit_counts.get(&o.name).copied().unwrap_or((0, false));
            // An output with no emit is a plain direct drive. An emitted
            // one needs a backing var only with 2+ emits, a conditional
            // emit, or a default it would fan in with.
            if count == 0 || (count < 2 && !in_branch && !has_default) {
                continue;
            }
            crate::lower::create_output_backing_var(ctx, &o.name);
        }
    }

    /// Wire the buffer initializers and `out` values, and capture the value
    /// members, once every member is in scope.
    fn lower_deferred_members(&mut self, ctx: &mut LowerCtx<'_>) {
        // Wire buffer initializers only after every ns member is in scope
        // (an init may reference a member declared after it). Only buffers
        // pre-declared above; a name the importer already owns stays its.
        for b in std::mem::take(&mut self.buffers) {
            lower_buffer_body(ctx, b);
        }
        // Same ordering rule as buffers: an `out x = <expr>` initializer may
        // name a member declared after it, so wire the values only once the
        // whole module is in scope. Lowered pure: a preceding namespace's
        // handler exec end can still be in `current_exec` here, and a
        // member's default would then take a `Var_Set` off that unrelated
        // handler instead of baking `InitialValue`.
        ctx.pure(|ctx| {
            for o in std::mem::take(&mut self.outputs) {
                ctx.with_nofold(o.no_fold, |ctx| {
                    lower_out_binding(ctx, &o.name, o.value.as_ref(), &o.range);
                });
            }
        });
        // Capture each value member's binding before the next `import * as`
        // overwrites the shared bare names. Importer-owned members were
        // captured by `route_member` and never join `value_names`.
        for name in std::mem::take(&mut self.value_names) {
            if let Some(binding) = ctx.scope.get(&name).cloned() {
                self.decls.insert(name, binding);
            }
        }
    }

    /// Lower this module's own `on` handlers while its members still hold the
    /// bare names (before the restore), so a handler body uses this module's
    /// members. A free name resolving to the importer's same-named state is
    /// rejected at typecheck, which checks the namespace body under a sealed
    /// scope floor. No floor here: it would also hide the namespace's own
    /// outputs from `lookup_output` and drop a legitimate `emit <own out>`.
    /// Then pre-declare and lower its anonymous chips (the chip node and inner
    /// vars, which pass 1 skips for a namespaced decl).
    fn lower_handlers_and_chips(&mut self, ctx: &mut LowerCtx<'_>) {
        for h in std::mem::take(&mut self.handlers) {
            // Dedup by source location: a handler reached through several
            // imports of one file installs once, or every write to shared
            // state would count once per import.
            if !ctx.first_import_of_behavior(&h.range) {
                continue;
            }
            ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h));
        }
        for ac in std::mem::take(&mut self.anon_chips) {
            pre_declare_anon_chip(ctx, ac, true);
            lower_anon_chip(ctx, ac);
        }
    }

    /// Hand each importer-owned bare name back to the importer, after this
    /// namespace's deferred buffer/out bodies resolved against the member's own
    /// gate.
    fn restore_importer_names(&mut self, ctx: &mut LowerCtx<'_>) {
        for (name, saved) in std::mem::take(&mut self.restores) {
            if let Some(binding) = saved {
                ctx.scope.insert(&name, binding);
            }
        }
    }

    /// Record this namespace's member map for each mod it declares, keyed
    /// by the mod's decl location (shared by `ns.f` and a bare sibling
    /// call), so the inline path pushes this module's members into the
    /// callee's body frame (`LowerCtx::ns_mod_scopes`).
    /// The frame also carries the aliases the module imported for itself
    /// (`import * as Other` inside it), which would otherwise lose to a
    /// same-named alias in the importer at the inline site.
    fn publish(self, ctx: &mut LowerCtx<'_>, ns: &NamespaceDecl) {
        let ns_decls = self.decls;
        let mut body_scope = ns_decls.clone();
        if let Some(file) = ns
            .decls
            .first()
            .map(|d| std::sync::Arc::clone(&d.range().file))
            && let Some(own) = ctx.ns_by_file.get(&file)
        {
            for (n, b) in own {
                body_scope.entry(n.clone()).or_insert_with(|| b.clone());
            }
        }
        let ns_scope = std::sync::Arc::new(body_scope);
        let ns_consts = std::sync::Arc::new(ctx.consts.namespace_members(&ns.name));
        for d in &ns.decls {
            if let TopDecl::Chip(c) = d {
                let key = c.range.start_key();
                ctx.ns_mod_scopes
                    .insert(key.clone(), std::sync::Arc::clone(&ns_scope));
                ctx.ns_mod_consts
                    .insert(key, std::sync::Arc::clone(&ns_consts));
            }
        }
        ctx.ns_by_file
            .entry(std::sync::Arc::clone(&ns.range.file))
            .or_default()
            .insert(ns.name.clone(), Binding::Namespace(ns_decls.clone()));
        ctx.scope.insert(&ns.name, Binding::Namespace(ns_decls));
    }
}

pub(super) fn lower_buffer_body(ctx: &mut LowerCtx<'_>, d: &BufferDecl) {
    // A name the top level declares more than once may name a `let` here:
    // the buffer is the one this declaration made.
    let key = (
        std::sync::Arc::clone(&d.range.file),
        d.range.start.offset,
        d.name.clone(),
    );
    let rec = match ctx
        .top_dup_bindings
        .get(&key)
        .or_else(|| ctx.scope.get(&d.name))
    {
        Some(Binding::Buffer(r)) => r.clone(),
        _ => return,
    };
    let saved_chain = ctx.builder.current_chain_id;
    let chain = ctx.alloc_chain();
    ctx.builder.current_chain_id = Some(chain);
    if let Some(node) = ctx.builder.module.nodes.get_mut(&rec.node_id) {
        node.chain_id = Some(chain);
    }
    let rhs_port = lower_expr(ctx, &d.init);
    // Through `ctx.connect`, not `builder.connect`, so the string -> bool
    // coercion applies: `buffer buf: bool = s` needs its `!= ""` compare.
    ctx.connect(rhs_port, rec.node_id.port(WirePort::Input));
    ctx.builder.current_chain_id = saved_chain;
}

/// Anonymous chip: reuses the Chip node created during pre-declare and lowers
/// the body in the parent scope with `current_anon_chip` set, so the emitter
/// routes its nodes into a child grid.
pub(super) fn lower_anon_chip(ctx: &mut LowerCtx<'_>, d: &AnonChipDecl) {
    // `pre_declare_anon_chip` records the node under this (range, enclosing
    // chip) pair; the scan matches the same pair for a node created elsewhere.
    let chip_node_id = ctx
        .anon_chip_nodes
        .get(&(d.range.clone(), ctx.current_anon_chip))
        .copied()
        .or_else(|| {
            ctx.builder
                .module
                .nodes
                .iter()
                .find(|(_, n)| {
                    n.kind == NodeKind::Chip
                        && n.source_range == d.range
                        && n.chip_id == ctx.current_anon_chip
                })
                .map(|(id, _)| *id)
        });
    let Some(chip_node_id) = chip_node_id else {
        return;
    };
    // At the top level, a name the chip declares means its outer binding again
    // after the chip, and inside the chip no read takes it for a top-level
    // constant (as typecheck's `check_anon_chip_stmts`; see `scope::top`).
    let storage = ctx
        .anon_chip_storage
        .get(&chip_node_id)
        .cloned()
        .unwrap_or_default();
    let top_level = ctx.scope.depth() == 1;
    let names: Vec<String> = match ctx.top_names.chip_at(&d.range) {
        Some(chip) if top_level => chip.names.clone(),
        _ => Vec::new(),
    };
    let mut earlier: Vec<(String, Binding)> = Vec::new();
    for name in &names {
        let own = storage.iter().find(|(n, _)| n == name).map(|(_, b)| b);
        if let Some(now) = ctx.scope.get_current(name).cloned()
            && !own.is_some_and(|b| same_binding(&now, b))
        {
            earlier.push((name.clone(), now));
        }
    }
    for (name, binding) in &storage {
        ctx.scope.insert(name, binding.clone());
    }
    let hidden = top_level.then(|| ctx.consts.hide_at_top(names.iter().cloned()));
    if let Some(hidden_mark) = hidden {
        let outside = earlier.clone();
        ctx.anon_chip_shadows.push(AnonChipShadow {
            body: d.range.clone(),
            outside,
            hidden_mark,
        });
    }

    let saved_chip = ctx.current_anon_chip.take();
    ctx.current_anon_chip = Some(chip_node_id);

    lower_chip_body(ctx, &d.body);

    ctx.current_anon_chip = saved_chip;
    if let Some(mark) = hidden {
        let left = names
            .iter()
            .filter_map(|n| Some((n.clone(), ctx.scope.get_current(n)?.clone())))
            .collect();
        let held = ctx.consts.hidden_at_top().split_off(mark);
        ctx.anon_chip_names.insert(d.range.clone(), (left, held));
    }
    for (name, binding) in earlier {
        ctx.scope.insert(&name, binding);
    }
    if let Some(mark) = hidden {
        ctx.anon_chip_shadows.pop();
        ctx.consts.unhide_at_top(mark);
    }
}

/// Whether a chip-body declaration's initializer contains a call. Must agree
/// with `typecheck::stmt::chip_stmt_init_calls`, or one phase emits a gate the
/// other refuses to check.
fn chip_stmt_init_calls(s: &Stmt) -> bool {
    let has_call = crate::analysis::visit::contains_call;
    match s {
        Stmt::Let(l) => has_call(&l.value),
        Stmt::Var(v) => v.init.as_ref().is_some_and(has_call),
        Stmt::OutBinding(o) => o.value.as_ref().is_some_and(has_call),
        _ => false,
    }
}

/// True for a chip-body statement that is a pure, reactive declaration: the
/// same set `is_pure_top_decl` uses at the top level.
fn is_pure_chip_stmt(s: &Stmt) -> bool {
    matches!(
        s,
        Stmt::Let(_) | Stmt::OutBinding(_) | Stmt::Var(_) | Stmt::Buffer(_)
    )
}

/// Lower an anon-chip body, whose statements behave like top-level
/// declarations. A pure statement (`let`/`var`/`out`/`buffer`) must not inherit
/// the ambient `current_exec`, or its var reads become a one-shot
/// `Exec_Var_Get` instead of the live `.Value`; an exec statement keeps it.
/// Otherwise as `lower_block`: nested chips pre-declared, handler execs
/// flushed, a trailing buffered emit closes the chain.
fn lower_chip_body(ctx: &mut LowerCtx<'_>, block: &Block) {
    for s in &block.stmts {
        if let Stmt::AnonChip(ac) = s {
            pre_declare_anon_chip(ctx, ac, false);
        }
    }
    for s in &block.stmts {
        if ctx.current_exec.is_some() {
            ctx.exec_closed_by_buffer_emit = false;
        }
        let is_handler_stmt = matches!(s, Stmt::Handler(_) | Stmt::AnonChip(_));
        // Only flush when there is no ambient exec chain (see the same guard in
        // `lower/handler.rs::lower_block`): a statement after a nested `on`
        // inside a chip body must stay on the outer chain.
        if !ctx.handler_end_execs.is_empty() && !is_handler_stmt && ctx.current_exec.is_none() {
            flush_handler_end_execs(ctx);
        }
        // A call has no continuous reading (an exec-requiring mod or chip only
        // runs on a chain), so inside a handler body it keeps the ambient exec.
        // Everything else stays pure, including every declaration in a
        // top-level chip, whose `current_exec` is a leak, not its own trigger.
        let joins_chain = ctx.in_handler_body && chip_stmt_init_calls(s);
        if is_pure_chip_stmt(s) && !joins_chain {
            ctx.pure(|ctx| lower_stmt(ctx, s));
        } else {
            lower_stmt(ctx, s);
        }
    }
    close_on_trailing_buffered_emit(ctx, block);
}

pub(super) fn lower_chip_decl(ctx: &mut LowerCtx<'_>, d: &ChipDecl) {
    ctx.record_decl_resolution_subst(d);
    // A mod is expanded and a chip instanced at each call site; neither is
    // compiled here.
    ctx.scope
        .insert(&d.name, Binding::Chip(Arc::new(d.clone())));
    ctx.record_decl_view(d);
}

/// Bind a record-valued RHS through the `let`'s binding form: the whole record
/// under one name, or destructured by field name or tuple position.
fn install_record_binding(ctx: &mut LowerCtx<'_>, d: &LetDecl, src: Fields) {
    match &d.binding {
        LetBinding::Ident { name, .. } => {
            ctx.scope.insert(name, Binding::Record(src));
        }
        LetBinding::RecordDestruct {
            fields: destruct_fields,
            ..
        } => install_record_destruct(ctx, &src, destruct_fields),
        LetBinding::Tuple { names, rest, .. } => {
            install_tuple_value(ctx, &d.value, &src, names, rest.as_ref());
        }
    }
}

pub(super) fn lower_let_decl(ctx: &mut LowerCtx<'_>, d: &LetDecl) {
    // Whether the initializer's value is the gates' (see
    // `crate::const_scope::Marks::uncertified`), decided before the binding
    // exists, like the value itself.
    let uncertified = ctx.consts.in_frame()
        && !ctx.with_const_ctx(ConstScope::Certified, |cx| {
            crate::const_eval::integer_ops_certified(&d.value, cx)
        });
    let mut records = Vec::new();
    lower_let_binding(ctx, d, &mut records);
    if ctx.scope.depth() == 1 {
        ctx.consts.bound_at_top(&d.range);
    }
    record_let_constants(ctx, d, records, uncertified);
}

/// A constant a `let` binds: the name, its value, and whether it is spelled
/// `const`.
type LetRecord = (String, Literal, bool);

/// Record what `d` bound in the innermost frame, mirroring
/// `typecheck::stmt::record_let`: each name in `records` as that constant (a
/// later entry replaces an earlier one), and every other name `d` binds
/// forgotten. Called after the binding exists, so the initializer reads the
/// names around the `let`.
fn record_let_constants(
    ctx: &mut LowerCtx<'_>,
    d: &LetDecl,
    records: Vec<LetRecord>,
    uncertified: bool,
) {
    let mut recorded: HashSet<String> = HashSet::default();
    for (name, lit, declared) in records {
        let marks = crate::const_scope::Marks {
            declared,
            placeholder: false,
            uncertified,
        };
        ctx.consts.record(&name, lit, marks);
        recorded.insert(name);
    }
    for name in crate::const_eval::bound_names(&d.binding) {
        if !recorded.contains(&name) {
            ctx.consts.forget(&name);
        }
    }
}

/// Record the constants `d` binds into `records`. Returns whether that leaves
/// nothing to lower: a constant array or map, or a value only a `const mod`
/// call yields.
fn record_let_value(ctx: &mut LowerCtx<'_>, d: &LetDecl, records: &mut Vec<LetRecord>) -> bool {
    // A plain `let` re-binding a declared `const` name clears its mark exactly
    // as `typecheck::stmt::record_let` does; both sides' `if` arms consult the
    // marks (`if_cond_const_ctx`), so a mismatch emits one arm of a condition
    // typecheck checked both ways. An `Ident` re-records the evaluated value
    // (evicting an outer or top-level constant); a destructured name is not
    // recorded, so `record_let_constants` forgets it. Skipped when evaluation
    // fails, as typecheck's `Err` arm leaves the mark.
    if !d.is_const && matches!(&d.binding, LetBinding::Ident { .. }) {
        let rebound: Vec<(String, Literal)> = ctx
            .const_eval_let(d, ConstScope::Plain)
            .and_then(|lit| crate::const_eval::bind_destructured(&d.binding, lit))
            .unwrap_or_default();
        for (name, lit) in rebound {
            if ctx.is_declared_const(&name) {
                records.push((name, lit, false));
            }
        }
    }

    // A body-local plain `let name = <constant>` is recorded in the innermost
    // frame so a constant-only config arg in this or a nested scope resolves
    // `name` via `const_lookup()` (`literal_for_property_port`). At the top
    // level no frame is open; the module's constants already hold it.
    //
    // Uses the narrow `expr_to_literal_in`: the full evaluator would bake config
    // args whose producing gate must survive (`@nofold let s = "a${1 + 1}b"`).
    // It also folds a prefab reference, which `eval_expr` refuses. Runs after
    // the clearing above, whose recorded value it supersedes.
    if let LetBinding::Ident { name, .. } = &d.binding
        && !d.is_const
        && let Some(lit) = expr_to_literal_in(&d.value, &ctx.const_lookup())
    {
        // Recorded unmarked: a plain `let` clears any `const` mark the name had.
        records.push((
            name.clone(),
            crate::const_eval::in_let_annotation(d, lit, ctx.consts.types()),
            false,
        ));
    }

    // A `const` binding of any form is recorded with the full evaluator, as
    // `typecheck::stmt::record_let` records it. Recording less here makes
    // typecheck skip an untaken block that lowering still emits, unchecked
    // (guarded by `typecheck_and_lowering_drop_exactly_the_same_ranges`). The
    // narrow evaluator cannot fold records, arrays, maps, interpolation, `if`,
    // indexing or certified method calls.
    //
    // Apart from the container case below this only records: `lower_ident`
    // consults `const_lookup()` only for a name with no scope binding, so
    // runtime reads still use the port binding the ordinary lowering installs.
    if d.is_const {
        let pairs = ctx
            .const_eval_let(d, ConstScope::Plain)
            .and_then(|lit| crate::const_eval::bind_destructured(&d.binding, lit))
            .unwrap_or_default();
        // A `const` whose value is an array or map literal is recorded and not
        // lowered: a runtime read materializes the container on demand
        // (`predeclare::materialize_const_container`). Gated on the syntactic
        // literal form as well as the evaluated kind.
        let container = matches!(&d.value, Expr::Array { .. } | Expr::MapLit { .. })
            && pairs
                .iter()
                .any(|(_, l)| matches!(l, Literal::Array(_) | Literal::Map(_)));
        let mut shadowing = Vec::new();
        for (name, lit) in pairs {
            if container && matches!(lit, Literal::Array(_) | Literal::Map(_)) {
                shadowing.push((name.clone(), lit.clone()));
            }
            records.push((name, lit, true));
        }
        if container {
            for (name, lit) in shadowing {
                shadow_with_const_container(ctx, &name, lit, &d.value);
            }
            return true;
        }
    }

    // `let x = T` / `let xs = [1, 2]`: a name bound to one constant array or
    // map is that constant, like a `const` of it, and lowers nothing here. A
    // runtime read materializes it on demand
    // (`predeclare::materialize_const_container`); a compile-time read folds.
    // Recorded in the innermost frame; at the top level `build_const_env`
    // holds it, and a name it holds with another value lowers normally.
    if let LetBinding::Ident { name, .. } = &d.binding
        && let Some(lit) = constant_container_init(ctx, &d.value)
    {
        let recorded = if ctx.consts.in_frame() {
            records.push((name.clone(), lit.clone(), d.is_const));
            true
        } else if ctx.top_names.is_dup(&d.range.file, name) {
            ctx.consts.dup_constant(&d.range, name) == Some(&lit)
        } else {
            ctx.const_lookup().get(name) == Some(&lit)
        };
        if recorded {
            shadow_with_const_container(ctx, name, lit, &d.value);
            return true;
        }
    }

    // An initializer whose value only a `const mod` call yields: record the
    // value and skip the ordinary lowering. A `const mod` that assembles a
    // collection (`t.push(x)` on a `const t`) has no valid gate form; inlining
    // it reports WS044 once per mutation. Three conditions, all required:
    //
    // 1. `expr_to_literal_in` failed. Its successes must keep a real wire: a
    //    named Vector/Rotator/Quat/Color constant keeps its `Make*` producer
    //    (`literal_for_property_port` excludes those four for the same reason),
    //    or a receiver like `dir.RotationByAngle(...)` prunes it as an orphan.
    // 2. The value is unreachable without `lookup_mod`, so a const-mod call is
    //    required. Otherwise interpolation and `if` expressions the full
    //    evaluator folds would lose their gates, even under `@nofold`.
    // 3. No bound value is a record (stated at its condition below).
    //
    // Not gated on `@nofold`: a mutating `const mod` has no unoptimized form to
    // fall back to. A skipped name still resolves in a wire position through
    // `lower_ident`'s `const_lookup()` fallback.
    if expr_to_literal_in(&d.value, &ctx.const_lookup()).is_none() {
        // Condition 2: with no `lookup_mod`, `eval_expr` cannot resolve any
        // const-mod call, so success means the value never needed one.
        let mut budget = crate::const_eval::Budget::default();
        let evaluated =
            if crate::const_eval::eval_let_value(d, &ctx.const_ctx(None), &mut budget).is_ok() {
                None
            } else {
                ctx.const_eval_let(d, ConstScope::Plain).ok()
            };
        // Split the value across the names the binding introduces, through the
        // `bind_destructured` that `build_const_env` and typecheck's
        // `record_let` use: `const { a, b } = pair(2)` is the only way to bind
        // a multi-output const mod's outputs. A `Tuple` binding never splits.
        if let Some(lit) = evaluated
            && let Ok(pairs) = crate::const_eval::bind_destructured(&d.binding, lit)
            // Condition 3: a record already has a per-field lowering below
            // (`Binding::Record`), and baking past it leaves `p.x` reads on the
            // vector-swizzle path and a `Literal::Record` reaching emit's
            // `unreachable!`. Checked on the split values, so `const { a, b } =
            // pair(2)` bakes while `const r = pair(2)` and a `...rest` keep the
            // ordinary lowering.
            && !pairs.iter().any(|(_, l)| matches!(l, Literal::Record(_)))
        {
            // Recorded with the binding's own `const` mark; a plain `let` that
            // needed a const-mod call stays a plain `let`.
            for (name, lit) in pairs {
                records.push((name, lit, d.is_const));
            }
            // A name the top level declares again binds nothing here, so a
            // read of this declaration finds its constant, not the binding
            // an earlier declaration left (see `keep_top_dup_bindings`).
            if ctx.scope.depth() == 1 {
                for name in crate::const_eval::bound_names(&d.binding) {
                    if ctx.top_names.is_dup(&d.range.file, &name) {
                        ctx.scope.set_root(&name, None);
                    }
                }
            }
            return true;
        }
    }
    false
}

fn lower_let_binding(ctx: &mut LowerCtx<'_>, d: &LetDecl, records: &mut Vec<LetRecord>) {
    // Clear any leftover inline-mod record so only THIS statement's call can set
    // it (an inline mod call within `d.value` sets it definitively at its end).
    ctx.pending_inline_record = None;

    // `let name: exec` is a local exec signal (an emit target). Checked first:
    // without an initializer the parser gives it a placeholder `0` value, which
    // must not reach the const recording below.
    if let Some(
        te @ TypeExpr::Name {
            name: type_name, ..
        },
    ) = &d.typ
    {
        // Through an alias only when written with no value: `let s: Sig =
        // go` names `go` again.
        if type_name == "exec" || (!d.has_initializer() && is_exec_type(ctx, te)) {
            if let LetBinding::Ident { name, .. } = &d.binding {
                // Top-level signals are hubbed by the pre-declare pass. A
                // body-level declaration always builds a fresh hub: each
                // mod/handler instance is its own signal, shadowing any outer
                // one of the same name.
                if ctx.current_exec.is_none() && ctx.signal_key(name).is_some() {
                    return;
                }
                build_exec_signal_hub(ctx, name, &d.range);
            }
            return;
        }
    }

    if record_let_value(ctx, d, records) {
        return;
    }

    // A prefab reference (`let pf = $./file.brz` or an inline nested-prefab
    // block) is constant-only: recorded above, resolved by
    // `literal_for_property_port` at a `prefab = pf` config arg, with no wire
    // value. `lower_expr` would lower it to an `_Unsupported` placeholder.
    if matches!(&d.value, Expr::PrefabRef { .. } | Expr::NestedPrefab { .. }) {
        return;
    }

    // A record literal binds a `Binding::Record`, not a single port.
    if let Expr::RecordLit { fields, .. } = &d.value {
        let lit = ctx.type_of(&d.value);
        let record = lower_record_lit(ctx, fields, Slots::Of(&lit));
        install_record_binding(ctx, d, record);
        return;
    }

    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let Some(Binding::Record(src)) = ctx.scope.get(rhs_name).cloned()
    {
        install_record_binding(ctx, d, src);
        return;
    }

    // An ident aliasing an array/map var binds the new name to the same `Var`:
    // containers are references, so this aliases rather than snapshots (a
    // scalar `let x = v` snapshots through the value path below).
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let LetBinding::Ident { name, .. } = &d.binding
        && let Some(Binding::Var(var_rec)) = ctx.scope.get(rhs_name).cloned()
        && matches!(var_rec.storage, VarStorage::Array | VarStorage::Map)
    {
        ctx.scope.insert(name, Binding::Var(var_rec));
        return;
    }

    // Same aliasing for an input port of container type (`in a: int[]` then
    // `let x = a`), reached through its `RerOutput`: `x` binds the same `Input`.
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let LetBinding::Ident { name, .. } = &d.binding
        && let Some(Binding::Input(inp)) = ctx.scope.get(rhs_name).cloned()
        && matches!(unwrap_ref(&inp.ty), Type::Array(_) | Type::Map(..))
    {
        ctx.scope.insert(name, Binding::Input(inp));
        return;
    }

    if let Some(Binding::Record(src)) = resolve_field_chain(ctx, &d.value).cloned() {
        install_record_binding(ctx, d, src);
        return;
    }

    // A record read from an array/map element (`let row = pts[i]`) or chosen
    // per leaf field (`let m = if c then p else q`, `match`) lowers its
    // per-field reads through `value_record_fields`; `resolve_field_chain` above
    // forwards only record locals. A record-returning call is handled below
    // via `pending_inline_record`.
    // `let q = p as Q` converts field by field, a tuple as much as a record.
    let converted_aggregate = matches!(&d.value, Expr::As { .. })
        && matches!(ctx.type_of(&d.value), Type::Record(_) | Type::Tuple(_));
    if (converted_aggregate
        || matches!(
            &d.value,
            Expr::IndexAccess { .. }
                | Expr::FieldAccess { .. }
                | Expr::TuplePick { .. }
                | Expr::IfExpr { .. }
                | Expr::MatchExpr { .. }
                | Expr::RefOf { .. }
        ) && matches!(ctx.type_of(&d.value), Type::Record(_) | Type::Enum { .. }))
        && let Some(src) = value_record_fields(ctx, &d.value)
    {
        install_record_binding(ctx, d, src);
        return;
    }

    // Clear any leftover inline record so only THIS value's lowering can set it
    // (mirrors the return / out / `value_record_fields` consumers).
    ctx.pending_inline_record = None;
    let saved_record_call = std::mem::replace(
        &mut ctx.record_result_call,
        matches!(&d.value, Expr::Call { .. }).then(|| d.value.range().clone()),
    );
    let rhs_port = lower_expr(ctx, &d.value);
    ctx.record_result_call = saved_record_call;
    let rhs_type = ctx.type_of(&d.value);

    // A multi-output inline mod call or an enum construction (`Shape.Empty`,
    // `Shape.Circle(x)`, `Box.Dims { .. }`) stashes its field -> port map in
    // `pending_inline_record`. Gated as in `value_record_fields`, so every
    // construction binds a `Binding::Record` carrying its payload, not the bare
    // `__disc` scalar.
    if (matches!(&d.value, Expr::Call { .. } | Expr::VariantCtor { .. })
        || matches!(&rhs_type, Type::Enum { .. }))
        && let Some(record) = ctx
            .pending_inline_record
            .take()
            .map(|rec| unwrap_call_record(ctx, &d.value, rec))
        // Annotated as an enum, a call with several outputs is its first.
        && let record = match d.typ.as_ref().map(|t| ctx.resolve_local_type(t)) {
            Some(Type::Enum { .. }) => one_enum_value(ctx, &d.value, record),
            _ => record,
        }
    {
        install_record_binding(ctx, d, record);
        return;
    }

    // Multi-output chip/call: the rhs_port points to the first output's
    // MicrochipOutput node. If the type is a Record, look up the chip node
    // that owns these outputs and build field->port bindings.
    if let Type::Record(ref fields) = rhs_type
        && let Expr::Call { .. } = &d.value
    {
        if let Some(record) = multi_output_chip_record(ctx, rhs_port, fields) {
            install_record_binding(ctx, d, record);
            return;
        }
        // A builtin multi-output gate (e.g. `character.InputReader()`) owns its
        // outputs directly, with no chip. Bind each declared field to the
        // gate's matching output port, the mapping `r.Forward` resolves through.
        let record: Fields = fields
            .iter()
            .filter_map(|(field_name, _ty)| {
                let port = resolve_output_field_port(ctx, rhs_port.node_id, field_name)?;
                Some((intern(field_name), Binding::Local(LocalRecord { port })))
            })
            .collect();
        // Only destructuring needs this. Binding the whole call to a name
        // must stay a `Local` on the gate's default output: field access
        // already resolves siblings through the node's ports, and making it
        // a record would break bare use of the result (`let p = a.pop()`
        // reads the popped element, not a record).
        if !record.is_empty() {
            match &d.binding {
                LetBinding::RecordDestruct {
                    fields: destruct_fields,
                    ..
                } => {
                    install_record_destruct(ctx, &record, destruct_fields);
                    return;
                }
                LetBinding::Tuple { names, rest, .. } => {
                    install_tuple_value(ctx, &d.value, &record, names, rest.as_ref());
                    return;
                }
                LetBinding::Ident { .. } => {}
            }
        }
    }

    match &d.binding {
        LetBinding::Ident { name, .. } => {
            // Tag the value node with the binding name, so a boundary pin that
            // captures it across a chip edge is labelled with it rather than a
            // synthetic `ext1`. Emit ignores it; a node already named keeps its
            // name.
            if let Some(node) = ctx.builder.module.nodes.get_mut(&rhs_port.node_id) {
                let props = std::sync::Arc::make_mut(&mut node.properties);
                if !props.contains_key(&*sym::NAME_LABEL)
                    && !props.contains_key(&*sym::BINDING_NAME)
                {
                    props.insert(*sym::BINDING_NAME, Literal::String(name.clone()));
                }
            }
            ctx.scope
                .insert(name, Binding::Local(LocalRecord { port: rhs_port }));
        }
        // A gate read held on its gate's first output (`let k = fs.max()`)
        // destructures by the names its outputs are declared with.
        LetBinding::Tuple { .. } | LetBinding::RecordDestruct { .. }
            if ctx.gate_outputs(d.value.range()).is_some() =>
        {
            let record: Fields = ctx
                .gate_outputs(d.value.range())
                .unwrap_or_default()
                .iter()
                .filter_map(|(n, _)| {
                    let port = resolve_output_field_port(ctx, rhs_port.node_id, n)?;
                    Some((intern(n), Binding::Local(LocalRecord { port })))
                })
                .collect();
            match &d.binding {
                LetBinding::Tuple { names, rest, .. } => {
                    install_tuple_value(ctx, &d.value, &record, names, rest.as_ref());
                }
                LetBinding::RecordDestruct { fields, .. } => {
                    install_record_destruct(ctx, &record, fields);
                }
                LetBinding::Ident { .. } => {}
            }
        }
        LetBinding::Tuple { names, .. } => {
            let source_node = ctx.builder.module.nodes.get(&rhs_port.node_id).cloned();
            if let Some(node) = source_node {
                for (i, name) in names.iter().enumerate() {
                    if let Some(port) = node.ports.outputs.get(i) {
                        ctx.scope.insert(
                            name,
                            Binding::Local(LocalRecord {
                                port: port_ref(node.id, resolve(port.name)),
                            }),
                        );
                    }
                }
            }
        }
        LetBinding::RecordDestruct { .. } => {
            // Record destructuring of a non-record RHS binds nothing.
            // TODO: report this as an error.
        }
    }
}

/// The array/map literal a `let` initializer evaluates to, when it is an
/// array/map literal or names a constant with no runtime binding. A name that
/// is bound (a `var`, a materialized `const`) is left to the aliasing paths.
fn constant_container_init(ctx: &LowerCtx<'_>, value: &Expr) -> Option<Literal> {
    match value {
        Expr::Array { .. } | Expr::MapLit { .. } => {}
        Expr::Ident { name, .. } if ctx.scope.get(name).is_none() => {}
        _ => return None,
    }
    match ctx.const_eval(value, ConstScope::Plain) {
        Ok(lit @ (Literal::Array(_) | Literal::Map(_))) => Some(lit),
        _ => None,
    }
}

/// Whether a record-literal field value may be record-shaped, so it is worth
/// probing with `value_record_fields`: anything typecheck did not type as a
/// single-wire value. A scalar field skips the probe and is lowered once.
pub(super) fn record_valued(ctx: &LowerCtx<'_>, value: &Expr) -> bool {
    !matches!(
        unwrap_ref(&ctx.type_of(value)),
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
            | Type::Brick
    )
}

/// Lower a record literal into a `Fields`. A field naming
/// storage binds that storage and is read where the record is used, so an
/// earlier field is held at its value before a later field's gates may change
/// what it reads (see `ordered::Ordered`): `{ x: q, y: bump() }` keeps
/// the `q` from before `bump()`.
pub(super) fn lower_record_lit(
    ctx: &mut LowerCtx<'_>,
    fields: &[RecordLitField],
    lit: Slots<'_>,
) -> Fields {
    let mut out = HashMap::default();
    let mut ord = ordered::Ordered::default();
    let mut slots: HashMap<Sym, usize> = HashMap::default();
    for (i, field) in fields.iter().enumerate() {
        // A field a later one supplies again is never read, so never held.
        let later = supplied_by(ctx, &fields[i + 1..]);
        let range = match field {
            RecordLitField::Named { range, .. }
            | RecordLitField::Shorthand { range, .. }
            | RecordLitField::Spread { range, .. } => range,
        };
        let mut keys = Vec::new();
        let slot = ord.source(ctx, "", None, range, |ctx| {
            let map = lower_record_field(ctx, field, lit);
            let mut kept = HashMap::default();
            for (k, b) in map {
                keys.push(k);
                out.insert(k, b.clone());
                if !later.contains(&k) {
                    kept.insert(k, b);
                }
            }
            Some(Binding::Record(kept))
        });
        for k in keys {
            if let Some(slot) = slot
                && let Some(earlier) = slots.insert(k, slot)
            {
                ord.drop_key(earlier, k);
            }
        }
    }
    for (k, b) in &mut out {
        if let Some(Binding::Record(now)) = slots.get(k).and_then(|s| ord.value(ctx, *s))
            && let Some(now) = now.get(k)
        {
            *b = now.clone();
        }
    }
    out
}

/// The field names `fields` supply, as far as they are known before lowering
/// them: a spread supplies every field of its record type.
pub(super) fn supplied_by(ctx: &LowerCtx<'_>, fields: &[RecordLitField]) -> HashSet<Sym> {
    let mut out = HashSet::default();
    for f in fields {
        match f {
            RecordLitField::Named { name, .. } | RecordLitField::Shorthand { name, .. } => {
                out.insert(intern(name));
            }
            RecordLitField::Spread { value, .. } => {
                if let Type::Record(fs) = unwrap_ref(&ctx.type_of(value)) {
                    out.extend(fs.iter().map(|(n, _)| intern(n)));
                }
            }
        }
    }
    out
}

/// What a record literal's fields hold, as typecheck typed the literal.
#[derive(Clone, Copy)]
pub(super) enum Slots<'t> {
    /// Unknown: each field holds what its value is.
    Unknown,
    /// The literal's type: a record's fields by name, a tuple's by position.
    Of(&'t Type),
    /// Every field holds one value.
    OneValue,
    /// Every field holds one enum value.
    OneEnum,
}

impl Slots<'_> {
    /// The type the literal gives its field `name`.
    pub(super) fn of(self, name: &str) -> Option<Type> {
        let Slots::Of(lit) = self else { return None };
        match unwrap_ref(lit) {
            Type::Record(fields) | Type::Outputs(fields) => {
                fields.into_iter().find(|(n, _)| n == name).map(|(_, t)| t)
            }
            Type::Tuple(elems) => elems.into_iter().nth(name.parse().ok()?),
            _ => None,
        }
    }

    /// Whether the literal's field `name` holds one value: a gate read
    /// there is its first output.
    pub(super) fn one_value(self, name: &str) -> bool {
        match self {
            Slots::OneValue | Slots::OneEnum => true,
            _ => self.of(name).is_some_and(|t| takes_one_value(&t)),
        }
    }

    /// Whether the literal's field `name` holds one enum value, one value
    /// held in several slots.
    pub(super) fn one_enum(self, name: &str) -> bool {
        match self {
            Slots::OneEnum => true,
            _ => matches!(
                self.of(name).map(|t| unwrap_ref(&t)),
                Some(Type::Enum { .. })
            ),
        }
    }

    /// What the literal's field `name` alone holds.
    pub(super) fn field(self, name: &str) -> Slots<'static> {
        match (self.one_value(name), self.one_enum(name)) {
            (true, true) => Slots::OneEnum,
            (true, false) => Slots::OneValue,
            _ => Slots::Unknown,
        }
    }
}

/// `binding`, the value at `at` of the record literal field `name`: a gate
/// read in a field holding one value is its first output.
fn field_value(
    ctx: &LowerCtx<'_>,
    binding: Binding,
    at: &SourceRange,
    lit: Slots<'_>,
    name: &str,
) -> Binding {
    match &binding {
        Binding::Record(rec) if lit.one_value(name) => {
            held_output(ctx, rec, at, OutputPick::First).unwrap_or(binding)
        }
        _ => binding,
    }
}

/// One field of a record literal whose fields hold `lit`, lowered: the
/// fields it supplies.
fn lower_record_field(
    ctx: &mut LowerCtx<'_>,
    field: &RecordLitField,
    lit: Slots<'_>,
) -> Vec<(Sym, Binding)> {
    let mut map = Vec::new();
    match field {
        RecordLitField::Named { name, value, .. } => {
            let one_value = lit.one_value(name) && ctx.gate_outputs(value.range()).is_some();
            let enum_slot = lit.one_enum(name);
            if let Expr::RecordLit {
                fields: inner_fields,
                ..
            } = value
            {
                let slot = lit.of(name);
                let slots = slot.as_ref().map_or(Slots::Unknown, Slots::Of);
                let inner = lower_record_lit(ctx, inner_fields, slots);
                map.push((intern(name), Binding::Record(inner)));
            } else if let Some(binding) = resolve_field_chain(ctx, value)
                .filter(|_| !reads_constant_as_other_number(ctx, value))
                .cloned()
            {
                // Value references something in scope (possibly through a record chain).
                let binding = field_value(ctx, binding, value.range(), lit, name);
                map.push((intern(name), binding));
            } else if let Some(inner) = (record_valued(ctx, value) && (!one_value || enum_slot))
                .then(|| value_record_fields(ctx, value))
                .flatten()
            {
                // A record-shaped value with no binding of its own (a
                // record-returning call, an enum construction, a record element).
                // `lower_expr` would keep one port, and consumers walking a
                // `Binding::Record` silently skip a `Local`, desyncing
                // parallel-array column lengths. Same resolver as the spread arm.
                let binding = Binding::Record(inner);
                map.push((
                    intern(name),
                    field_value(ctx, binding, value.range(), lit, name),
                ));
            } else {
                // A call the record probe above already lowered for this
                // same value keeps that one gate.
                let port = match take_value_record_port(ctx, value) {
                    Some(port) => port,
                    None => lower_expr(ctx, value),
                };
                map.push((intern(name), Binding::Local(LocalRecord { port })));
            }
        }
        RecordLitField::Shorthand { name, range } => {
            // `{ foo }` means `{ foo: foo }`.
            if let Some(binding) = ctx.scope.get(name).cloned() {
                let binding = field_value(ctx, binding, range, lit, name);
                map.push((intern(name), binding));
            }
        }
        RecordLitField::Spread { value, .. } => {
            // A `...expr` spread merges the fields of any record value (a
            // scope binding, a nested record literal, an index/map read, or
            // a record-returning call), later fields overriding earlier ones.
            if let Some(src_fields) = value_record_fields(ctx, value) {
                map.extend(src_fields);
            }
        }
    }
    map
}

/// Install record destructure bindings from a source record into the scope.
pub(super) fn install_record_destruct(
    ctx: &mut LowerCtx<'_>,
    src: &Fields,
    destruct_fields: &[RecordDestructField],
) {
    let mut remaining = src.clone();
    for field in destruct_fields {
        match field {
            RecordDestructField::Named {
                name, alias, range, ..
            } => {
                let key = intern(name);
                if let Some(binding) = remaining.remove(&key) {
                    let bind_name = alias.as_deref().unwrap_or(name);
                    ctx.scope.insert(bind_name, binding);
                } else {
                    // Binding nothing would leave every use an `_Unsupported`
                    // placeholder that reads 0. Field names are case-sensitive,
                    // so point at a differently-cased field when there is one.
                    let available: Vec<String> =
                        src.keys().map(|k| resolve(*k).to_string()).collect();
                    let suggestion = available
                        .iter()
                        .find(|f| f.eq_ignore_ascii_case(name))
                        .map(|f| format!("; did you mean `{f}`?"))
                        .unwrap_or_else(|| {
                            let mut names = available.clone();
                            names.sort();
                            format!("; available fields: {}", names.join(", "))
                        });
                    ctx.diagnostics.push(Diagnostic::error(
                        "WS002",
                        format!("no field `{name}` on this value{suggestion}"),
                        range.clone(),
                    ));
                }
            }
            RecordDestructField::Rest { name, .. } => {
                ctx.scope.insert(name, Binding::Record(remaining.clone()));
            }
        }
    }
}

/// The field names of `ty` in declaration order: the positions a tuple pattern
/// binds against. A `Binding::Record` is a `HashMap` with no order; the type
/// has one (a tuple literal is index-keyed, `"0"`, `"1"`, and a multi-output
/// `mod` call name-keyed in signature order). Index keys when `ty` is not a
/// record.
pub(super) fn tuple_positions(ty: &Type, arity: usize) -> Vec<String> {
    match ty {
        Type::Record(fields) => fields.iter().map(|(n, _)| n.clone()).collect(),
        _ => (0..arity).map(|i| i.to_string()).collect(),
    }
}

/// Bind a tuple pattern against `src`, the value of `value`: a gate read's
/// outputs by position in the order it declares them (see [`held_output`]),
/// any other record by [`tuple_positions`].
fn install_tuple_value(
    ctx: &mut LowerCtx<'_>,
    value: &Expr,
    src: &Fields,
    names: &[String],
    rest: Option<&String>,
) {
    if let Some(Binding::Record(positions)) =
        held_output(ctx, src, value.range(), OutputPick::Positions)
    {
        let order: Vec<String> = (0..positions.len()).map(|i| i.to_string()).collect();
        install_tuple_destruct(ctx, &positions, names, rest, &order);
        return;
    }
    let order = tuple_positions(&ctx.type_of(value), names.len());
    install_tuple_destruct(ctx, src, names, rest, &order);
}

/// Bind a tuple pattern positionally against `src`, reading the positions in
/// `order` (see [`tuple_positions`]). `rest` collects everything past `names`,
/// re-keyed by its new position so `rest.0` is the first leftover. Must match
/// the re-keying in `const_eval::destructure`'s `Tuple` arm, so compile-time and
/// runtime destructuring agree.
pub(super) fn install_tuple_destruct(
    ctx: &mut LowerCtx<'_>,
    src: &Fields,
    names: &[String],
    rest: Option<&String>,
    order: &[String],
) {
    for (i, name) in names.iter().enumerate() {
        if let Some(key) = order.get(i)
            && let Some(binding) = src.get(&intern(key)).cloned()
        {
            ctx.scope.insert(name, binding);
        }
    }
    if let Some(rest_name) = rest {
        let mut tail: Fields = HashMap::default();
        for (i, key) in order.iter().skip(names.len()).enumerate() {
            if let Some(binding) = src.get(&intern(key)).cloned() {
                tail.insert(intern(&i.to_string()), binding);
            }
        }
        ctx.scope.insert(rest_name, Binding::Record(tail));
    }
}
