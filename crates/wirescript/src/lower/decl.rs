use super::*;

// ---------- declaration body pass ----------

/// True if `p`'s declared output type is `string`, read straight off the IR
/// node's port spec (no typecheck-map dependency). A label value that is already
/// a string wires into the label's `Text` port directly; only a non-string is
/// routed through a `FormatText` gate to coerce it.
fn label_source_is_string(ctx: &LowerCtx, p: PortRef) -> bool {
    ctx.builder
        .module
        .get(p.node_id)
        .and_then(|n| n.output_type(p.port))
        == Some(Type::String)
}

/// Lower a label expression to a string-typed value port: the value directly if
/// it is already a string (e.g. a string var, or an interpolation that already
/// built its own `FormatText`), otherwise coerced through a single `FormatText`.
fn lower_label_value(ctx: &mut LowerCtx, le: &Expr) -> PortRef {
    let value = lower_expr(ctx, le);
    if label_source_is_string(ctx, value) {
        value
    } else {
        build_format_text(ctx, "{0}".to_string(), vec![value], le.range())
    }
}

/// Resolve runtime `@label(expr)` on top-level `var`s into dynamic labels.
///
/// For each top-level var whose label doesn't fold to a constant (a constant
/// baked its text statically in pass 1), lower the expression to a value port,
/// string-coerce it if needed, and record `host_var -> source_port` on the
/// module. Emit turns each entry into a wire from that port into the var's
/// label `Component_TextDisplay.Text` (see `emit.rs` Pass 3.5).
///
/// Runs after the body pass so any var/gate the label references already exists,
/// and in a *pure* context (`current_exec` cleared) so a var read resolves to
/// the var's live `Value` output rather than an exec-chained `Var_Get`.
pub(super) fn resolve_dynamic_var_labels(ctx: &mut LowerCtx, decls: &[TopDecl]) {
    let saved_exec = ctx.current_exec.take();
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
    ctx.current_exec = saved_exec;
}

/// Resolve a module-level `@label(<expr>)` into the root microchip's label.
///
/// A constant expression folds to static title text (`root_label_override`);
/// a runtime expression is lowered (string-coerced) and recorded as
/// `root_dynamic_label`, which emit wires into the root shell's `Text` port.
/// Runs in the same post-declaration pass as [`resolve_dynamic_var_labels`], so
/// the expression may forward-reference declarations below it (hoisting).
pub(super) fn resolve_module_label(ctx: &mut LowerCtx, module_label: Option<&Expr>) {
    let Some(le) = module_label else { return };
    if let Some(text) = resolve_label_text(None, Some(le), &ctx.const_lookup()) {
        // Constant — baked static title text.
        ctx.builder.module.root_label_override = Some(text);
        return;
    }
    // Runtime value — lower in a pure context and coerce to string only if it
    // isn't already one. Emit wires this into the root shell's `Text`.
    let saved_exec = ctx.current_exec.take();
    let src = lower_label_value(ctx, le);
    ctx.builder.module.root_dynamic_label = Some(src);
    ctx.current_exec = saved_exec;
}

pub(super) fn lower_decl(ctx: &mut LowerCtx, d: &TopDecl) {
    match d {
        TopDecl::Out(b) => ctx.with_nofold(b.no_fold, |ctx| {
            lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range)
        }),
        // Dedup an imported handler reached through several imports of one file
        // (a plain + `import * as` pair) so its behaviour installs once, not
        // once per import. A non-imported handler has a unique location and is
        // always first-seen, so this never affects an ordinary program. See N2.
        TopDecl::Handler(h) => {
            if ctx.first_import_of_behavior(&h.range) {
                ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h));
            }
        }
        TopDecl::Event(e) => ctx.with_nofold(e.no_fold, |ctx| lower_event_decl(ctx, e)),
        TopDecl::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
        TopDecl::Buffer(b) => lower_buffer_body(ctx, b),
        // Gate created in pre-pass; top level is pure, so a non-constant init
        // has no exec reset to apply it — surface the drop.
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
        TopDecl::ExprStmt(es) => crate::lower::stmt::lower_expr_stmt(ctx, &es.expr),
        TopDecl::Fn(f) => {
            // `fn` has been removed (rejected at parse with a hard error). A
            // recovered `fn` decl is still lowered as an inline mod-with-return so
            // a stray one doesn't crash lowering — there is no deprecation warning.
            let outputs = if let Some(ref ret_type) = f.return_type {
                vec![NamedOutput {
                    name: "_".into(),
                    typ: ret_type.clone(),
                    range: f.range.clone(),
                }]
            } else {
                Vec::new()
            };
            let chip = ChipDecl {
                name: f.name.clone(),
                type_params: Vec::new(),
                inputs: f.params.clone(),
                rest: None,
                outputs,
                body: Block {
                    stmts: vec![Stmt::Return {
                        value: Some(f.body.clone()),
                        range: f.range.clone(),
                    }],
                    range: f.range.clone(),
                },
                range: f.range.clone(),
                inline: true,
                label: None,
                label_expr: None,
                color_expr: None,
                material_expr: None,
                closed: false,
                no_fold: false,
                is_const: false,
                is_fn: false,
            };
            lower_chip_decl(ctx, &chip);
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) | TopDecl::Enum(_) => {}
        TopDecl::Namespace(ns) => {
            let mut ns_decls: HashMap<String, Binding> = HashMap::default();
            let mut ns_buffers = Vec::new();
            let mut ns_outputs = Vec::new();
            // Value members whose lowered binding we capture into this
            // namespace's map AFTER the loop (so `A.foo` reads A's own `foo`
            // even when a later `import * as B` overwrites the shared bare
            // `foo`). Bare-name value members are only ever `let name`/`array
            // name`/… idents, so a plain name is all we need here.
            let mut ns_value_names: Vec<String> = Vec::new();
            // Importer-owned names (`var`/`array`/`map`/`buffer`/`in`/`out`
            // whose bare name the ENTRY file itself declares) to restore AFTER
            // this namespace's deferred buffer/out bodies have wired against the
            // member's own fresh gate. Each member is always pre-declared fresh
            // (never skipped), so two modules' same-named state — or a local
            // plus an imported one — land on DISTINCT storage gates instead of
            // collapsing onto one; the restore then hands the bare name back to
            // the importer so `g` stays the importer's while `A.g` is A's own.
            let mut ns_restores: Vec<(String, Option<Binding>)> = Vec::new();
            // Top-level `on` handlers in the imported module. They are lowered
            // AFTER the member loop (below), so their bodies resolve THIS
            // namespace's members by bare name — correct even when another
            // namespace exports the same names, because handlers are lowered in
            // place here rather than inlined at a call site. Without this the
            // handlers fell into `_ => {}` and were silently dropped: importing a
            // module as a namespace ran none of its `on Clock` / event / exec
            // handlers.
            let mut ns_handlers: Vec<&Handler> = Vec::new();
            // Anonymous chips (`chip { … }` / `chip on t { … }`) in the imported
            // module. Like handlers, they install behaviour and are lowered at
            // the end of the arm (pre-declared then lowered, since pass 1 never
            // descended into `ns.decls`), while the module's members still hold
            // the bare names. Without this they fell into `_ => {}` and vanished.
            let mut ns_anon_chips: Vec<&AnonChipDecl> = Vec::new();
            for d in &ns.decls {
                match d {
                    TopDecl::Chip(c) => {
                        ns_decls.insert(
                            c.name.clone(),
                            Binding::Chip(std::sync::Arc::new(c.clone())),
                        );
                        // A namespaced mod's body also calls its SIBLING mods by
                        // bare name (`drawCardBg(...)`, not `card.drawCardBg`);
                        // register them so those calls resolve when the body is
                        // inlined at a call site in the importing module. (The
                        // namespaced form stays available via the Namespace
                        // binding below.)
                        if ctx.scope.get(&c.name).is_none() {
                            ctx.scope
                                .insert(&c.name, Binding::Chip(std::sync::Arc::new(c.clone())));
                        }
                    }
                    // A namespaced (`import * as ns`) mod's body references its
                    // OWN module's `let` constants / `array` / `var` by bare
                    // name. Those mods are inlined at call sites in the importing
                    // module, where the members aren't otherwise in scope — so
                    // lower them here, into the enclosing scope, or every such
                    // reference drops to an `_Unsupported` placeholder that reads
                    // 0 at runtime. (Constant `array` initializers bake straight
                    // into the ArrayVar node during pre-declaration.)
                    //
                    // The bare-name insertion is shared across every namespace,
                    // so a name two modules both export collides there. Each
                    // member's binding is ALSO captured per-namespace below, and
                    // `A.member` resolves through that map, so explicit
                    // namespaced access stays correct regardless of the clash.
                    // An imported `let name = …` must not clobber a bare name
                    // the IMPORTER itself owns (its own `in`/`out`/`var`/…):
                    // without this, an imported `let start` overwrites a local
                    // `in start: exec`, so `on start` then finds a
                    // `Binding::Record`/`Local` instead of the Input and
                    // silently drops the whole handler body. But the member
                    // must STILL be reachable as `ns.name` — so when the name is
                    // importer-owned we lower it, capture the resulting binding
                    // into THIS namespace's own map, then RESTORE the importer's
                    // bare binding. Without the capture, `Other.start` fell
                    // through `resolve_field_chain`'s bare-scope fallback to the
                    // local `in start` input and `Other.start.0` lowered to
                    // `_Unsupported`. (Only `importer_names` is checked, not "any
                    // prior binding", so a member shadowed by an EARLIER
                    // `import * as` still lowers to the shared bare name — two
                    // namespaces exporting the same `let` keep `A.foo`/`B.foo`.)
                    TopDecl::Let(l) => {
                        let owned_name = match &l.binding {
                            crate::ast::LetBinding::Ident { name, .. }
                                if ctx.importer_names.contains(name) =>
                            {
                                Some(name.clone())
                            }
                            _ => None,
                        };
                        if let Some(name) = owned_name {
                            let saved = ctx.scope.get(&name).cloned();
                            ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l));
                            if let Some(binding) = ctx.scope.get(&name).cloned() {
                                ns_decls.insert(name.clone(), binding);
                            }
                            if let Some(b) = saved {
                                ctx.scope.insert(&name, b);
                            }
                        } else {
                            ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l));
                            if let crate::ast::LetBinding::Ident { name, .. } = &l.binding {
                                ns_value_names.push(name.clone());
                            }
                        }
                    }
                    // Each state member is pre-declared FRESH (no `is_none()`
                    // skip): the old guard silently dropped a second module's
                    // same-named `var`/`array`/… and let `B.g` alias `A.g`'s gate.
                    // `route_ns_member` then captures the fresh binding per
                    // namespace and, for an importer-owned name, restores the
                    // importer's own bare binding at the end of the arm.
                    TopDecl::Array(a) => {
                        if let Some(existing) = ctx.reuse_import_state(&a.range) {
                            ns_decls.insert(a.name.clone(), existing);
                        } else {
                            let owned = ctx.importer_names.contains(&a.name);
                            let saved = owned.then(|| ctx.scope.get(&a.name).cloned()).flatten();
                            pre_declare_array(ctx, a);
                            record_ns_member_state(ctx, &a.range, &a.name);
                            route_ns_member(
                                ctx,
                                &a.name,
                                &mut ns_decls,
                                &mut ns_value_names,
                                &mut ns_restores,
                                saved,
                                owned,
                            );
                        }
                    }
                    TopDecl::Map(m) => {
                        if let Some(existing) = ctx.reuse_import_state(&m.range) {
                            ns_decls.insert(m.name.clone(), existing);
                        } else {
                            let owned = ctx.importer_names.contains(&m.name);
                            let saved = owned.then(|| ctx.scope.get(&m.name).cloned()).flatten();
                            pre_declare_map(ctx, m);
                            record_ns_member_state(ctx, &m.range, &m.name);
                            route_ns_member(
                                ctx,
                                &m.name,
                                &mut ns_decls,
                                &mut ns_value_names,
                                &mut ns_restores,
                                saved,
                                owned,
                            );
                        }
                    }
                    TopDecl::Var(v) => {
                        if let Some(existing) = ctx.reuse_import_state(&v.range) {
                            ns_decls.insert(v.name.clone(), existing);
                        } else {
                            let owned = ctx.importer_names.contains(&v.name);
                            let saved = owned.then(|| ctx.scope.get(&v.name).cloned()).flatten();
                            ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
                            // Module-level = pure: a non-constant init is dropped.
                            ctx.with_nofold(v.no_fold, |ctx| warn_unbaked_var_init(ctx, v, true));
                            record_ns_member_state(ctx, &v.range, &v.name);
                            route_ns_member(
                                ctx,
                                &v.name,
                                &mut ns_decls,
                                &mut ns_value_names,
                                &mut ns_restores,
                                saved,
                                owned,
                            );
                        }
                    }
                    TopDecl::Buffer(b) => {
                        if let Some(existing) = ctx.reuse_import_state(&b.range) {
                            ns_decls.insert(b.name.clone(), existing);
                        } else {
                            let owned = ctx.importer_names.contains(&b.name);
                            let saved = owned.then(|| ctx.scope.get(&b.name).cloned()).flatten();
                            pre_declare_buffer(ctx, b);
                            record_ns_member_state(ctx, &b.range, &b.name);
                            ns_buffers.push(b);
                            route_ns_member(
                                ctx,
                                &b.name,
                                &mut ns_decls,
                                &mut ns_value_names,
                                &mut ns_restores,
                                saved,
                                owned,
                            );
                        }
                    }
                    // An imported module's `in`/`out` PORTS. Without these the
                    // declarations were dropped entirely: `on ns.trigger { … }`
                    // type-checked clean and lowered to nothing at all, taking
                    // the whole handler with it. They become ports of the
                    // importing module's chip, exactly as a local `in`/`out`
                    // does, and are reachable both bare and as `ns.name`.
                    TopDecl::In(i) => {
                        if let Some(existing) = ctx.reuse_import_state(&i.range) {
                            ns_decls.insert(i.name.clone(), existing);
                        } else {
                            let owned = ctx.importer_names.contains(&i.name);
                            let saved = owned.then(|| ctx.scope.get(&i.name).cloned()).flatten();
                            pre_declare_input(ctx, i);
                            record_ns_member_state(ctx, &i.range, &i.name);
                            route_ns_member(
                                ctx,
                                &i.name,
                                &mut ns_decls,
                                &mut ns_value_names,
                                &mut ns_restores,
                                saved,
                                owned,
                            );
                        }
                    }
                    TopDecl::Out(o) => {
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
                            )
                        });
                        ns_outputs.push(o);
                        route_ns_member(
                            ctx,
                            &o.name,
                            &mut ns_decls,
                            &mut ns_value_names,
                            &mut ns_restores,
                            saved,
                            owned,
                        );
                        // The output's binding lives under its mangled scope key,
                        // so `route_ns_member`'s bare-name capture misses it.
                        // Insert it into this namespace's map so `L.count` reads
                        // the output's value (`binding_to_port` on `Binding::Output`
                        // sources its rerouter); without it the read lowered to an
                        // `_Unsupported` placeholder.
                        if let Some(binding) = ctx
                            .scope
                            .get(&crate::lower::context::output_scope_key(&o.name))
                            .cloned()
                        {
                            ns_decls.insert(o.name.clone(), binding);
                        }
                    }
                    TopDecl::Handler(h) => ns_handlers.push(h),
                    TopDecl::AnonChip(ac) => ns_anon_chips.push(ac),
                    _ => {}
                }
            }
            // A namespaced module's own top-level `on` handler may declare a
            // boundary port (`on trig { out hport: int = 7 }`), the same
            // hoist pass 1a gives the entry file's handlers. `ns.decls` is not
            // walked by pass 1a, so do it here, after the member loop above so
            // a handler-declared name binds to an explicit `out` member of the
            // same name rather than minting a second port. `boundary` is false
            // to match what the `TopDecl::Out` member arm passes.
            for h in &ns_handlers {
                pre_declare_handler_outputs(ctx, &h.body, false);
                // Same namespace-map capture the `TopDecl::Out` arm does, so
                // `L.hport` reads the port's value. An explicit `out` member of
                // the same name already governs, so never overwrite one.
                for o in crate::ast::handler_port_sites(&h.body) {
                    if let Some(binding) = ctx
                        .scope
                        .get(&crate::lower::context::output_scope_key(&o.name))
                        .cloned()
                    {
                        ns_decls.entry(o.name.clone()).or_insert(binding);
                    }
                }
            }
            // A namespaced `out` reached by 2+ emits, a conditional emit, or a
            // default-plus-emit needs a backing var exactly like a top-level one,
            // but the top-level pass-1b prescan does not descend into `ns.decls`.
            // Create it HERE — the output rerouter exists (pre-declared above) and
            // this runs before the default/emit wiring below — so those drivers
            // route through the var instead of fanning in on the rerouter's
            // `RER_Input`, which fails to load in-game (4b).
            {
                // Its handlers' and anonymous chips' writes, and its mods'
                // writes once per instantiation (`output_write_counts`).
                let emit_counts = ctx
                    .namespace_output_writes
                    .get(&crate::lower::namespace_scope(ns))
                    .cloned()
                    .unwrap_or_default();
                let ns_handler_ports: Vec<&OutBinding> = ns_handlers
                    .iter()
                    .flat_map(|h| crate::ast::handler_port_sites(&h.body))
                    .collect();
                // A value on an `out` MEMBER is a persistent default driver, so
                // any emit beside it fans in with it. A value on a
                // handler-declared port is that port's own driving site, which
                // `count_emits_in_handler` has already counted - the split
                // top-level pass 1b makes with `defaulted_outputs`.
                let sites = ns_outputs
                    .iter()
                    .map(|o| (*o, o.value.is_some()))
                    .chain(ns_handler_ports.into_iter().map(|o| (o, false)));
                for (o, has_default) in sites {
                    let (count, in_branch) =
                        emit_counts.get(&o.name).copied().unwrap_or((0, false));
                    // Only an output actually EMITTED to needs a backing var (an
                    // output with no emit is a plain direct drive), and then only
                    // when it has 2+ emits, a conditional emit, or a default it
                    // would otherwise fan in with. The top-level prescan gets the
                    // `count == 0` exclusion for free by iterating only emitted
                    // outputs; here we scan every output, so exclude it directly.
                    if count == 0 || (count < 2 && !in_branch && !has_default) {
                        continue;
                    }
                    crate::lower::create_output_backing_var(ctx, &o.name);
                }
            }
            // Wire buffer initializers only after every ns member is in scope
            // (an init may reference a member declared after it). Only buffers
            // pre-declared above — a name the importer already owns stays its.
            for b in ns_buffers {
                lower_buffer_body(ctx, b);
            }
            // Same ordering rule as buffers: an `out x = <expr>` initializer may
            // name a member declared after it, so wire the values only once the
            // whole module is in scope. A declaration is not a step on any
            // handler's exec chain: `mod.rs`'s pass-2 loop clears
            // `current_exec` around a top-level `out`'s own lowering, but this
            // namespace arm is reached through `lower_decl` for `TopDecl::Namespace`,
            // which that pass-2 loop does not recognize as pure, so a preceding
            // namespace's handler exec end can still be sitting in
            // `current_exec` here. Clear it for the same reason `mod.rs` does,
            // or a member's default value takes a `Var_Set` off an unrelated
            // handler instead of baking `InitialValue`.
            let saved_exec = ctx.current_exec.take();
            for o in ns_outputs {
                ctx.with_nofold(o.no_fold, |ctx| {
                    lower_out_binding(ctx, &o.name, o.value.as_ref(), &o.range)
                });
            }
            ctx.current_exec = saved_exec;
            // Capture each value member's binding into this namespace's map,
            // NOW — before the next `import * as` lowers its own members and
            // overwrites the shared bare names. An importer-owned member was
            // already captured into `ns_decls` by `route_ns_member` (its bare
            // name still belongs to the importer, so it never joins
            // `ns_value_names`), and is restored below.
            for name in ns_value_names {
                if let Some(binding) = ctx.scope.get(&name).cloned() {
                    ns_decls.insert(name, binding);
                }
            }
            // Lower this module's own `on` handlers in a frame SEALED to this
            // module's own members. Seeding the frame from `ns_decls` and
            // raising the scope floor to it means a free name in the handler
            // resolves to a sibling member (or a language global — events/math
            // resolve via the catalog, not this stack), never falling through
            // to the importer's same-named top-level state. Without the seal,
            // `lib` `on go { q = q + 1 }` with no `q`/`go` of its own silently
            // wired to the importer's `go`/`q` (N1). `ns_decls` is fully
            // populated by now (it's consumed into the Namespace binding below).
            // Lower this module's own `on` handlers, now that its members are in
            // scope and still bound to the bare names (before the restore below),
            // so a handler body reads/writes THIS module's members even when
            // another namespace exports the same names. The N1 leak (a free name
            // resolving to the importer's same-named state) is caught at
            // TYPECHECK — where the namespace body is checked under a sealed
            // scope floor — so a leaking program never reaches emit. Sealing
            // HERE too was redundant and actively harmful: the floor also hid the
            // namespace's own outputs (which `lookup_output` reads through the
            // scope), silently dropping a legitimate `emit <own out>`.
            for h in ns_handlers {
                // Dedup by source location: the same imported `on` handler
                // reached through several imports of one file installs its
                // behaviour ONCE, not once per import (which on shared state
                // would N-count every write). See N2.
                if !ctx.first_import_of_behavior(&h.range) {
                    continue;
                }
                ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h));
            }
            // Anonymous chips: pre-declare (create the chip node + inner vars,
            // the pass-1 step that never ran for a namespaced decl) then lower
            // the body.
            for ac in ns_anon_chips {
                pre_declare_anon_chip(ctx, ac, true);
                lower_anon_chip(ctx, ac);
            }
            // Hand each importer-owned bare name back to the importer, only now
            // that this namespace's deferred buffer/out bodies have resolved
            // against the member's own fresh gate.
            for (name, saved) in ns_restores {
                if let Some(binding) = saved {
                    ctx.scope.insert(&name, binding);
                }
            }
            // Record this namespace's member map for each mod it declares, keyed
            // by the mod's decl location, so the inline path can push THIS
            // module's members into the callee's body frame (see
            // `LowerCtx::ns_mod_scopes`). Both the `ns.f` access and a bare
            // sibling call reach the same source ChipDecl, so the location key is
            // shared.
            // A namespaced body also reaches through the aliases ITS module
            // imported for itself (`Other.bump(n)` inside a module that wrote
            // `import * as Other`). Those travel in as their own top-level
            // namespaces, but the ambient scope keeps one binding per name, so
            // a same-named alias in the importer would win at the inline site.
            // Carry them in the module's own frame, where they shadow it.
            let mut body_scope = ns_decls.clone();
            if let Some(file) = ns.decls.first().map(|d| d.range().file.clone())
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
                    let key = (c.range.file.clone(), c.range.start.offset);
                    ctx.ns_mod_scopes.insert(key.clone(), ns_scope.clone());
                    ctx.ns_mod_consts.insert(key, ns_consts.clone());
                }
            }
            ctx.ns_by_file
                .entry(ns.range.file.clone())
                .or_default()
                .insert(ns.name.clone(), Binding::Namespace(ns_decls.clone()));
            ctx.scope.insert(&ns.name, Binding::Namespace(ns_decls));
        }
    }
}

/// Route a namespace state member's freshly pre-declared binding. `owned` is
/// whether the ENTRY file itself declares this bare name (`saved` is the
/// importer's prior binding, captured before the fresh pre-declare):
/// - owned: capture the fresh binding into the namespace map and record the
///   importer's binding for restoration once the namespace's deferred
///   buffer/out bodies have wired (so `A.g` is A's own gate while the bare `g`
///   goes back to the importer);
/// - not owned: defer capture to the end-of-loop `ns_value_names` sweep, and
///   let the fresh binding stay as the shared bare name until the next
///   `import * as` overwrites it.
///
/// Either way the member always gets its OWN gate — the collapse bug was the
/// old path SKIPPING a member whose bare name a prior namespace/local already
/// held, which aliased the two onto one storage gate.
/// Record the gate a freshly pre-declared namespace state member produced, so
/// a LATER re-import of the same source location (another `import * as`, or a
/// plain/named import of the same file) reuses it instead of duplicating.
/// Captured right after pre-declaration, while the fresh binding is still the
/// member's bare name in scope — before `route_ns_member` may restore an
/// importer-owned name over it. See [`LowerCtx::import_state_dedup`] (N2).
fn record_ns_member_state(ctx: &mut LowerCtx, range: &SourceRange, name: &str) {
    if let Some(binding) = ctx.scope.get(name).cloned() {
        ctx.record_import_state(range, binding);
    }
}

fn route_ns_member(
    ctx: &LowerCtx,
    name: &str,
    ns_decls: &mut HashMap<String, Binding>,
    value_names: &mut Vec<String>,
    restores: &mut Vec<(String, Option<Binding>)>,
    saved: Option<Binding>,
    owned: bool,
) {
    if owned {
        if let Some(binding) = ctx.scope.get(name).cloned() {
            ns_decls.insert(name.to_string(), binding);
        }
        restores.push((name.to_string(), saved));
    } else {
        value_names.push(name.to_string());
    }
}

pub(super) fn lower_buffer_body(ctx: &mut LowerCtx, d: &BufferDecl) {
    // A name the top level declares more than once may name a `let` here:
    // the buffer is the one this declaration made.
    let key = (d.range.file.clone(), d.range.start.offset, d.name.clone());
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
    // Through `ctx.connect` (not `builder.connect`) so the string -> bool
    // coercion choke point sees this wire — `buffer buf: bool = s` must get
    // its `!= ""` compare like every other bool-typed destination.
    ctx.connect(rhs_port, rec.node_id.port(WirePort::Input));
    ctx.builder.current_chain_id = saved_chain;
}

/// Anonymous chip: reuses the Chip node created during pre-declare.
/// Processes the body in the PARENT scope with chip_id set so nodes
/// get tagged for the emitter to route into a child grid.
pub(super) fn lower_anon_chip(ctx: &mut LowerCtx, d: &AnonChipDecl) {
    // Find the chip node that was created during pre_declare_decl.
    // Fast path: `pre_declare_anon_chip` recorded the node under this exact
    // (range, enclosing chip) pair. The scan below matches on the same pair
    // and remains only as a safety net for a node created out of band.
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
    // At the top level, a binding a name the chip declares shares is what
    // the name means again after the chip, and inside the chip no read takes
    // the name for a top-level constant (as typecheck's
    // `check_anon_chip_stmts`; see `scope::top`).
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
            && !own.is_some_and(|b| crate::lower::context::same_binding(&now, b))
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
        ctx.anon_chip_shadows
            .push(crate::lower::context::AnonChipShadow {
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

/// True for a chip-body statement that is a pure, reactive DECLARATION — the
/// same set `is_pure_top_decl` uses at the top level.
fn is_pure_chip_stmt(s: &Stmt) -> bool {
    matches!(
        s,
        Stmt::Let(_) | Stmt::OutBinding(_) | Stmt::Var(_) | Stmt::Buffer(_)
    )
}

/// Lower an anon-chip body. A `chip { }` is a visual grouping whose top-level
/// statements behave like TOP-LEVEL declarations, not steps on a handler's exec
/// spine: a PURE statement (`let`/`var`/`out`/`buffer`) is reactive signal flow
/// and must NOT inherit the ambient (e.g. post-handler) `current_exec`, or its
/// var reads latch onto that chain as a one-shot `Exec_Var_Get` frozen at the
/// var's init instead of the var's live `.Value`. An EXEC statement (`if`,
/// assignment, expr) KEEPS the ambient exec, so a chip that runs imperative
/// logic (e.g. draining an input queue with `if q.length() > 0 { … }`) still
/// fires. Mirrors the top-level decl loop's `is_pure_top_decl` handling; the
/// rest matches `lower_block` (pre-declare nested chips, flush handler execs,
/// a trailing buffered emit terminates the chain).
fn lower_chip_body(ctx: &mut LowerCtx, block: &Block) {
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
            let saved_exec = ctx.current_exec.take();
            lower_stmt(ctx, s);
            ctx.current_exec = saved_exec;
        } else {
            lower_stmt(ctx, s);
        }
    }
    crate::lower::handler::close_on_trailing_buffered_emit(ctx, block);
}

pub(super) fn lower_chip_decl(ctx: &mut LowerCtx, d: &ChipDecl) {
    ctx.record_decl_resolution_subst(d);
    // Inline declarations (mod keyword or ref params) are stored for
    // expansion at call sites, not compiled as standalone microchips.
    let has_ref_params = d.inputs.iter().any(|p| ctx.ref_or_array(Some(d), &p.typ));
    if d.inline || has_ref_params {
        ctx.scope
            .insert(&d.name, Binding::Chip(std::sync::Arc::new(d.clone())));
        ctx.record_decl_view(d);
        return;
    }

    // Standalone chips: register for instantiation at call sites.
    ctx.scope
        .insert(&d.name, Binding::Chip(std::sync::Arc::new(d.clone())));
    ctx.record_decl_view(d);
}

/// Bind a record-valued RHS through the `let`'s binding form: the whole record
/// under one name, or destructured by field name or tuple position. `false` for
/// a binding form this does not cover, which leaves the caller to keep looking
/// for a way to lower the declaration.
fn install_record_binding(
    ctx: &mut LowerCtx,
    d: &LetDecl,
    src: HashMap<crate::intern::Sym, Binding>,
) -> bool {
    match &d.binding {
        LetBinding::Ident { name, .. } => {
            ctx.scope.insert(name, Binding::Record(src));
        }
        LetBinding::RecordDestruct {
            fields: destruct_fields,
            ..
        } => install_record_destruct(ctx, &src, destruct_fields),
        LetBinding::Tuple { names, rest, .. } => {
            install_tuple_value(ctx, &d.value, &src, names, rest.as_ref())
        }
        _ => return false,
    }
    true
}

pub(super) fn lower_let_decl(ctx: &mut LowerCtx, d: &LetDecl) {
    // Whether the initializer's value is the gates' (see
    // `crate::const_scope::Marks::uncertified`), decided before the binding
    // exists, like the value itself.
    let uncertified = ctx.consts.in_frame() && {
        let lookup = |n: &str| ctx.resolve_mod(n);
        !crate::const_eval::integer_ops_certified(&d.value, &ctx.certified_const_ctx(Some(&lookup)))
    };
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

/// Record what `d` bound, now that its binding exists, mirroring
/// `typecheck::stmt::record_let`: each name in `records` as that constant
/// (a later entry for a name replaces an earlier one), in the innermost
/// frame; every other name `d` binds is forgotten there, since the binding
/// replaced whatever the frame held under it. Recording only after the
/// binding exists keeps the initializer, lowered first, reading the names
/// around the `let`. At the top level no frame is open and the module's
/// constants already hold every top-level one.
fn record_let_constants(
    ctx: &mut LowerCtx,
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
fn record_let_value(ctx: &mut LowerCtx, d: &LetDecl, records: &mut Vec<LetRecord>) -> bool {
    // A plain (non-`const`) `let` re-binding a name CLEARS the `const` mark
    // and value it shadows — for EVERY binding form, and using the FULL
    // evaluator, because that is exactly what `typecheck::stmt::record_let`
    // does on both of its sides (`Ident` and destructuring alike).
    //
    // This is the CLEARING half of the const environment, and it has to be
    // symmetric with typecheck's for the same reason the recording half is:
    // the `const` marks are what BOTH sides' `if` arms consult
    // (`if_cond_const_ctx`) to decide a condition is compile-time decidable. Clearing LESS here than typecheck clears
    // there is a silent wrong-branch miscompile — typecheck sees a shadowed,
    // no-longer-constant name and checks both arms, while lowering still holds
    // the stale value, decides the condition, and emits only one arm with no
    // Branch gate at all. Three spellings reached that, all measured:
    //   `const { x } = { x: 111 }` + `let { x } = { x: 222 }` — only the
    //     `Ident` binding form was handled here at all;
    //   `const x = 111` + `let x = if true then 222 else 0` — an `Ident`
    //     whose value only the FULL evaluator folds, so the narrow recording
    //     below never ran;
    //   the same `Ident` case with the `const` at the TOP level — the eviction
    //     a declared-only read performs is driven by the name being PRESENT in
    //     a frame and unmarked there, so merely removing the mark evicts
    //     nothing when the value lives in the module's constants.
    //
    // Each arm mirrors its typecheck counterpart exactly, and the difference
    // between them is load-bearing rather than an inconsistency to tidy away:
    // typecheck's `Ident` arm RECORDS the newly evaluated value (which is what
    // makes the eviction above fire for an outer/top-level constant, and is
    // also the value a constant-only config slot must now see), while its
    // destructuring arm REMOVES. Inserting on the destructuring side too would
    // evict where typecheck does not — lowering emitting an arm typecheck
    // elided and never checked, the one direction that is never safe.
    //
    // Gated on the evaluation SUCCEEDING, again mirroring typecheck exactly:
    // its `Err(_)` arm leaves the mark alone, so clearing unconditionally
    // (from `bound_names`, say) would diverge in that same bad direction.
    // Routed through the shared `bind_destructured` so "which names does this
    // binding re-bind" cannot drift from the answer the recording sites use.
    //
    // Confined to names that currently read as a DECLARED `const`
    // (`is_declared_const`): a shadow of anything else is not part of this
    // feature, and skipping it keeps the promise that a program using no
    // `const` keyword lowers exactly as it did before: nothing is declared
    // there, so this whole block is inert. A destructured name is not recorded
    // at all, so `record_let_constants` forgets it.
    if !d.is_const && matches!(&d.binding, LetBinding::Ident { .. }) {
        let rebound: Vec<(String, Literal)> = {
            let lookup = |n: &str| ctx.resolve_mod(n);
            let mut budget = crate::const_eval::Budget::default();
            crate::const_eval::eval_let_value(d, &ctx.const_ctx(Some(&lookup)), &mut budget)
                .and_then(|lit| crate::const_eval::bind_destructured(&d.binding, lit))
                .unwrap_or_default()
        };
        for (name, lit) in rebound {
            if ctx.is_declared_const(&name) {
                records.push((name, lit, false));
            }
        }
    }

    // A body-local `let name = <constant>` is recorded in the innermost
    // frame (mirroring how a top-level `let` lands in the module's constants
    // via `build_const_env`), so a constant-only config arg elsewhere in this
    // scope (or a nested one) can resolve `name` via `ctx.const_lookup()`
    // (see `literal_for_property_port`). This is a no-op at the top level:
    // `lower_let_decl` also handles `TopDecl::Let` (via `lower_decl`), but
    // no `push_scope` has run there yet, so no frame is open, and the
    // module's constants already cover the top-level ones.
    //
    // A plain (non-`const`) `let`: NARROW evaluator, exactly as before this
    // feature existed. Widening this one would change how a program using no
    // `const` at all compiles — `literal_for_property_port` would start baking
    // config args whose producing gate must survive (the `@nofold let s =
    // "a${1 + 1}b"` case gate 2 below describes) — so it is deliberately left
    // on `expr_to_literal_in`. It re-inserts the value the clearing above just
    // dropped, which is why it must run AFTER it. The clearing's full
    // evaluator does not subsume this one: `expr_to_literal_in` also folds a
    // prefab reference, which `eval_expr` refuses.
    if let LetBinding::Ident { name, .. } = &d.binding
        && !d.is_const
        && let Some(lit) = expr_to_literal_in(&d.value, &ctx.const_lookup())
    {
        // A plain `let` re-binding a name CLEARS any `const` mark it had.
        records.push((
            name.clone(),
            crate::const_eval::in_let_annotation(d, lit, ctx.consts.types()),
            false,
        ));
    }

    // A `const` binding — EVERY binding form, `Ident` and destructuring
    // alike. This is a CORRECTNESS requirement, not a convenience:
    // `typecheck::stmt::record_let` records these into its own frame using
    // the FULL evaluator, and the `const` marks are what BOTH sides' `if`
    // arms consult (`if_cond_const_ctx`) to decide a condition is
    // compile-time decidable.
    // Recording less here than typecheck records there makes typecheck skip
    // an untaken block that lowering still EMITS — ill-typed code reaching
    // the graph with no diagnostic at all, which is exactly the divergence
    // `const_eval`'s module doc and `lower_if`'s "the same evaluator ... so
    // the two sides agree" comment exist to prevent. Guarded by
    // `typecheck_and_lowering_drop_exactly_the_same_ranges`.
    //
    // The narrow `expr_to_literal_in` above is NOT sufficient here: it cannot
    // fold a record/array/map literal, string interpolation, an `if`
    // expression, indexing, or a certified method call, all of which the full
    // evaluator (and therefore typecheck) folds. A block-scope
    // `const p = { x: 1 }` was recorded by typecheck and NOT by lowering, so
    // `if p.x == 1 { … } else { … }` elided only on the typecheck side — the
    // same one-sided elision, reachable with no destructuring at all.
    //
    // Restricted to `is_const` so a program using no `const` keyword compiles
    // byte-identically to before (the feature's own first design rule); a
    // `const` is a semantic guarantee that the value IS compile-time, so
    // widening it cannot change the meaning of a program that had none.
    //
    // Unlike the const-mod path below this only RECORDS and never
    // early-returns, so it cannot delete a gate: `lower_ident` consults
    // `const_lookup()` only for a name with NO scope binding at all, so every
    // runtime read still resolves through the port binding the ordinary
    // lowering installs. Recording happens once that binding exists
    // (`record_let_constants`).
    if d.is_const {
        let pairs = {
            let lookup = |n: &str| ctx.resolve_mod(n);
            let mut budget = crate::const_eval::Budget::default();
            crate::const_eval::eval_let_value(d, &ctx.const_ctx(Some(&lookup)), &mut budget)
                .and_then(|lit| crate::const_eval::bind_destructured(&d.binding, lit))
                .unwrap_or_default()
        };
        // A `const` whose value IS an array or map literal. Recorded above like
        // any other constant, and then NOT lowered — the same early-return
        // shape as the prefab-reference and const-mod skips below, and for the
        // same reason: `lower_expr` has no `Expr::Array` arm at all and an
        // explicit `Expr::MapLit` -> unsupported arm, so falling through emits
        // a placeholder `_Unsupported` gate plus a WSP001 warning for a value
        // the compiler has already computed correctly.
        //
        // Gated on the SYNTACTIC form, not merely on the evaluated literal's
        // kind, so this can only ever delete a placeholder: those are the two
        // expression forms with no runtime lowering to lose. Gated on
        // `d.is_const` as well, so a plain `let xs = [1, 2, 3]` keeps its
        // existing behavior and a program using no `const` compiles unchanged.
        //
        // A runtime read of the name still works: it materializes the container
        // on demand — see `predeclare::materialize_const_container`.
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
    // map is that constant, exactly like a `const` of it. It lowers nothing
    // here: a runtime read materializes it on demand (see
    // `predeclare::materialize_const_container`) and a compile-time read
    // folds, where lowering the value would bind a placeholder.
    //
    // Recorded in the innermost frame so those reads find THIS value; at the
    // top level `build_const_env` already holds it, and a name it holds with
    // any other value keeps the ordinary lowering.
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

    // An initializer whose value can ONLY be obtained by CALLING a `const
    // mod`: record the value and STOP, skipping the ordinary lowering of the
    // expression.
    //
    // This is a correctness requirement, not an optimization. A `const mod`
    // that ASSEMBLES a collection (`const t = []` … `t.push(x)` … `return t`)
    // has NO valid ordinary lowering at all: its mutations target a `const`
    // binding, which has no array/map var for `lower_array_method` to point
    // at, so inlining the body emitted WS044 "the operation would be silently
    // dropped" — once per mutation — for a `const r = rooms(2)` whose value
    // the compiler had ALREADY computed correctly. Binding such a mod's result
    // to a name is the natural way to reuse one, so that failure hit exactly
    // the spelling users reach for first.
    //
    // THREE gates, and each is load-bearing — any one alone is wrong (gate 3
    // is stated at its own `.filter`, below):
    //
    // 1. `expr_to_literal_in` must have FAILED. Its successes are the
    //    pre-existing folding surface, and other parts of lowering depend on
    //    those still producing a real WIRE: a named Vector/Rotator/Quat/Color
    //    constant must keep its wired `Make*` producer (see
    //    `literal_for_property_port`, which excludes exactly those four from
    //    its own bare-`Ident` constant path for this reason — resolving them
    //    to a literal lets a receiver like `dir.RotationByAngle(…)` skip the
    //    wire and silently prunes the producing gate as a wireless orphan).
    //
    // 2. The value must be UNREACHABLE without `lookup_mod` — i.e. a const-mod
    //    call is genuinely required, not merely present-in-principle. Gate 1
    //    alone also catches string interpolation and `if`-expressions, which
    //    the full evaluator can fold but the narrow one cannot; eliding those
    //    silently deleted real gates from programs containing no `const` at
    //    all (`@nofold let s = "a${1 + 1}b"` lost its FormatText + MathAdd,
    //    4 gates -> 2), violating both `@nofold`'s "nothing folded or elided"
    //    promise and this feature's rule that a program using no `const` must
    //    compile identically. Re-running the evaluator with `lookup_mod: None`
    //    answers "did this NEED a const mod?" exactly, and — unlike scanning
    //    the AST for a call node — cannot drift from what the evaluator
    //    actually does, since it IS the evaluator.
    //
    // Deliberately NOT gated on `nofold_depth`/`FoldMode`. `@nofold` is a
    // barrier against the fold pass, and a barrier can only suppress an
    // optimization that has a valid unoptimized form to fall back to; a
    // mutating `const mod` has none, so honoring `@nofold` here would just
    // restore the WS044 breakage under an attribute. (`@nofold const` is a
    // parse error anyway — the attribute only precedes `var`/`static
    // var`/`let`/`on` — and a `const` binding is a semantic guarantee that
    // the value is compile-time, which no fold setting can revoke.) The
    // `@nofold`-observable surface is unaffected in practice: gate 2 confines
    // this to expressions that had no faithful gate form to preserve.
    //
    // Every name skipped here still resolves in a WIRE position through
    // `lower_ident`'s `const_lookup()` fallback.
    if expr_to_literal_in(&d.value, &ctx.const_lookup()).is_none() {
        // Scope the immutable borrows of `ctx` (the `resolve_mod` closure and
        // `const_ctx`) so they end before the lowering below needs `&mut ctx`.
        let evaluated = {
            let mut budget = crate::const_eval::Budget::default();
            // Gate 2, first: with no `lookup_mod`, `eval_expr` cannot resolve
            // any const-mod call. Success here means the value never needed
            // one, so this initializer keeps lowering exactly as it did
            // before this feature existed.
            if crate::const_eval::eval_let_value(d, &ctx.const_ctx(None), &mut budget).is_ok() {
                None
            } else {
                let lookup = |n: &str| ctx.resolve_mod(n);
                let mut budget = crate::const_eval::Budget::default();
                crate::const_eval::eval_let_value(d, &ctx.const_ctx(Some(&lookup)), &mut budget)
                    .ok()
            }
        };
        // Split the one evaluated value across whatever names the binding
        // introduces, through the SAME `bind_destructured` the const
        // recording above, `build_const_env` and typecheck's `record_let` use.
        // A destructuring binding is NOT a second, lesser spelling here: a
        // multi-output `const mod` (`-> (a: …, b: …)`) evaluates to a
        // `Literal::Record`, and `const { a, b } = pair(2)` is the only way
        // to bind its outputs — so restricting this to `Ident` left every
        // multi-output const mod lowering its body as ordinary gates, which
        // is precisely the failure the comment above says this path exists
        // to prevent (a mutating one hit WS044; one whose `out` sits inside
        // a const `if` hit WS002 "no field ... available fields: ..."; a
        // plain one just emitted dead gates for every `out` value).
        // `Tuple` still splits nowhere, so it simply never takes this path.
        if let Some(lit) = evaluated
            && let Ok(pairs) = crate::const_eval::bind_destructured(&d.binding, lit)
            // GATE 3, and it exists only for records — stated over the
            // BOUND VALUES rather than the whole evaluated one, which is the
            // same rule with the destructuring case included. Unlike every
            // other bakeable kind, a record ALREADY has a non-literal
            // lowering below (`Expr::RecordLit` -> `Binding::Record`, and the
            // multi-output-call paths further down, all of which bind each
            // field to its own port), and baking here early-returns straight
            // past it. That is not a missed optimization, it is a miscompile:
            // `let p: Point = { x: mk(3), y: 10 }` (a field needing a
            // const-mod call, so gates 1 and 2 both pass it through) lost its
            // `Binding::Record`, and the later `p.x`/`p.y` reads fell through
            // to the vector-swizzle path — a bogus `Expr_SplitVector`
            // `.X`/`.Y` on a record, plus the `Literal::Record` reaching emit
            // and tripping its `unreachable!`. Arrays and maps have no such
            // existing lowering to hijack, which is why only this one kind
            // needs excluding. A record's FIELDS still bake through
            // `lower_record_lit`'s own per-field constant folding.
            //
            // Checking the SPLIT values is what lets a multi-output const
            // mod through while keeping that guarantee exactly: `const { a, b
            // } = pair(2)` binds two scalars and the record itself is never
            // stored under any name, whereas `const dummy = pair(2)` (whole
            // record under one name) and `const { x, ...rest } = p` (a
            // `...rest` re-collects one) still keep their existing lowering.
            && !pairs.iter().any(|(_, l)| matches!(l, Literal::Record(_)))
        {
            // Body-local only, exactly like the narrow recording above: a
            // top-level `let`/`const` has no open frame here and is already
            // covered by `build_const_env`. A plain `let` that reaches this
            // path (its value NEEDED a `const mod` call) is still just a plain
            // `let`.
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

fn lower_let_binding(ctx: &mut LowerCtx, d: &LetDecl, records: &mut Vec<LetRecord>) {
    // Clear any leftover inline-mod record so only THIS statement's call can set
    // it (an inline mod call within `d.value` sets it definitively at its end).
    ctx.pending_inline_record = None;

    // `let name: exec` — local exec signal, register as emit target. Checked
    // first and returns unconditionally: an exec-typed `let` (parsed with a
    // placeholder `0` value when it has no `= ...` initializer — see the
    // parser) never names a constant, so it must not reach the const
    // recording below.
    if let Some(
        te @ TypeExpr::Name {
            name: type_name, ..
        },
    ) = &d.typ
    {
        // Through an alias only when written with no value: `let s: Sig =
        // go` names `go` again.
        if type_name == "exec"
            || (!d.has_initializer() && crate::lower::call::is_exec_type(ctx, te))
        {
            if let LetBinding::Ident { name, .. } = &d.binding {
                // Top-level signals were already hubbed by the pre-declare
                // pass; skip the pass-2 revisit (no exec context at top level).
                // Body-level declarations always build a fresh hub — each
                // mod/handler instance is its own signal, even under the same
                // name (shadowing any outer signal).
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

    // A prefab-only reference (`let pf = $./file.brz` / an inline nested-
    // prefab block): constant-only — it lives in the const env (recorded
    // above, resolved by `literal_for_property_port` wherever `pf` is later
    // passed as a `prefab = pf` config arg) and has no wire value of its
    // own. `Expr::PrefabRef`/`Expr::NestedPrefab` aren't handled by the
    // general expression lowerer (`lower_expr` falls through to
    // `synthesise_unsupported` for them), so without this early return —
    // mirroring the record-lit/record-alias early-returns below — the `let`
    // would emit a placeholder `_Unsupported` gate plus a warning even
    // though the reference resolves fine as a constant.
    if matches!(&d.value, Expr::PrefabRef { .. } | Expr::NestedPrefab { .. }) {
        return;
    }

    // Handle record literals specially — they produce a Binding::Record,
    // not a single PortRef.
    if let Expr::RecordLit { fields, .. } = &d.value {
        let lit = ctx.type_of(&d.value);
        let record = lower_record_lit(ctx, fields, Slots::Of(&lit));
        if install_record_binding(ctx, d, record) {
            return;
        }
    }

    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let Some(Binding::Record(src)) = ctx.scope.get(rhs_name).cloned()
    {
        if install_record_binding(ctx, d, src) {
            return;
        }
    }

    // RHS that is an ident aliasing an array/map var. Bind the new name to the
    // SAME `Var` so `x[i]` / `x.push(..)` / `x.get(k)` resolve exactly like the
    // original — arrays/maps are reference containers, so this aliases rather
    // than snapshots (a scalar `let x = v` still snapshots through the value
    // path below). Without it, `let ar = myarray; ar[0]` lost the array binding
    // and the index lowered to a `_Unsupported` placeholder.
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let LetBinding::Ident { name, .. } = &d.binding
        && let Some(Binding::Var(var_rec)) = ctx.scope.get(rhs_name).cloned()
        && matches!(var_rec.storage, VarStorage::Array | VarStorage::Map)
    {
        ctx.scope.insert(name, Binding::Var(var_rec));
        return;
    }

    // Same aliasing, but where the RHS is an INPUT port of container type
    // (`in a: int[]` then `let x = a`). An input array/map is a reference
    // container too, reached through its `RerOutput`; bind `x` to the same
    // `Input` so `x[i]` / `x.length()` resolve exactly like `a[i]`. Without this
    // the alias fell through to the scalar value path and the index lowered to
    // an `_Unsupported` placeholder.
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let LetBinding::Ident { name, .. } = &d.binding
        && let Some(Binding::Input(inp)) = ctx.scope.get(rhs_name).cloned()
        && matches!(unwrap_ref(&inp.ty), Type::Array(_) | Type::Map(..))
    {
        ctx.scope.insert(name, Binding::Input(inp));
        return;
    }

    if let Some(Binding::Record(src)) = resolve_field_chain(ctx, &d.value).cloned() {
        if install_record_binding(ctx, d, src) {
            return;
        }
    }

    // `let row = pts[i]` / `let inr = pts[i].inner` — a record value read from a
    // record ARRAY/MAP element, and `let m = if c then p else q` /
    // `let m = match s { .. }` — a record CHOSEN between per leaf field. The
    // `resolve_field_chain` block above forwards only record LOCALS; every other
    // record source lowers its per-field reads through `value_record_fields`
    // here. Gated on a record type and a non-call source (a record-returning
    // CALL is handled below via `pending_inline_record`), so a scalar or call
    // RHS never routes here.
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
        && let Some(src) = crate::lower::stmt::value_record_fields(ctx, &d.value)
    {
        if install_record_binding(ctx, d, src) {
            return;
        }
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

    // Record-shaped RHS: a multi-output inline mod call OR an enum CONSTRUCTION
    // (unit `Shape.Empty` / `None`, positional `Shape.Circle(x)`, or named-payload
    // `Box.Dims { .. }`), each of which stashes its field->source-port map in
    // `pending_inline_record` when lowered. Gated on the same
    // Call/VariantCtor/enum-typed condition `value_record_fields` uses, so every
    // construction spelling binds a `Binding::Record` rather than the bare
    // `__disc` scalar (which dropped the payload for the non-Call forms).
    if (matches!(&d.value, Expr::Call { .. } | Expr::VariantCtor { .. })
        || matches!(&rhs_type, Type::Enum { .. }))
        && let Some(record) = ctx
            .pending_inline_record
            .take()
            .map(|rec| crate::lower::stmt::unwrap_call_record(ctx, &d.value, rec))
        // Annotated as an enum, a call with several outputs is its first.
        && let record = match d.typ.as_ref().map(|t| ctx.resolve_local_type(t)) {
            Some(Type::Enum { .. }) => crate::lower::stmt::one_enum_value(ctx, &d.value, record),
            _ => record,
        }
    {
        install_record_binding(ctx, d, record);
        return;
    }

    // Multi-output chip/call: the rhs_port points to the first output's
    // MicrochipOutput node. If the type is a Record, look up the chip node
    // that owns these outputs and build field->port bindings.
    if let Type::Record(ref fields) = rhs_type {
        if let Expr::Call { .. } = &d.value {
            if let Some(record) = multi_output_chip_record(ctx, rhs_port, fields) {
                install_record_binding(ctx, d, record);
                return;
            }
            // A builtin multi-output gate (e.g. `character.InputReader()`) owns
            // its outputs directly — no chip wraps them, so the lookup above
            // finds nothing. Bind each declared field to the gate's matching
            // output port, the same mapping `r.Forward` resolves through.
            // Without this a destructure bound nothing and every use became an
            // `_Unsupported` placeholder wired to no source.
            let record: HashMap<crate::intern::Sym, Binding> = fields
                .iter()
                .filter_map(|(field_name, _ty)| {
                    let port = resolve_output_field_port(ctx, rhs_port.node_id, field_name)?;
                    Some((
                        crate::intern::intern(field_name),
                        Binding::Local(LocalRecord { port }),
                    ))
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
                    _ => {}
                }
            }
        }
    }

    match &d.binding {
        LetBinding::Ident { name, .. } => {
            // Tag this `let`'s value node with the binding name, so a boundary
            // pin that later captures it across a chip edge derives its label
            // from `progress`/`init` rather than a synthetic `ext1`/`ext2`.
            // Derivation-only (emit ignores it), and it never overrides a node
            // that already carries a name (a var/param keeps its own).
            if let Some(node) = ctx.builder.module.nodes.get_mut(&rhs_port.node_id) {
                let props = std::sync::Arc::make_mut(&mut node.properties);
                if !props.contains_key(&*crate::intern::sym::NAME_LABEL)
                    && !props.contains_key(&*crate::intern::sym::BINDING_NAME)
                {
                    props.insert(
                        *crate::intern::sym::BINDING_NAME,
                        crate::ir::Literal::String(name.clone()),
                    );
                }
            }
            ctx.scope
                .insert(&name, Binding::Local(LocalRecord { port: rhs_port }));
        }
        // A gate read held on its gate's first output (`let k = fs.max()`)
        // destructures by the names its outputs are declared with.
        LetBinding::Tuple { .. } | LetBinding::RecordDestruct { .. }
            if ctx.gate_outputs(d.value.range()).is_some() =>
        {
            let record: HashMap<crate::intern::Sym, Binding> = ctx
                .gate_outputs(d.value.range())
                .unwrap_or_default()
                .iter()
                .filter_map(|(n, _)| {
                    let port = resolve_output_field_port(ctx, rhs_port.node_id, n)?;
                    Some((
                        crate::intern::intern(n),
                        Binding::Local(LocalRecord { port }),
                    ))
                })
                .collect();
            match &d.binding {
                LetBinding::Tuple { names, rest, .. } => {
                    install_tuple_value(ctx, &d.value, &record, names, rest.as_ref());
                }
                LetBinding::RecordDestruct { fields, .. } => {
                    install_record_destruct(ctx, &record, fields)
                }
                _ => {}
            }
        }
        LetBinding::Tuple { names, .. } | LetBinding::Record { names, .. } => {
            let source_node = ctx.builder.module.nodes.get(&rhs_port.node_id).cloned();
            if let Some(node) = source_node {
                for (i, name) in names.iter().enumerate() {
                    if let Some(port) = node.ports.outputs.get(i) {
                        ctx.scope.insert(
                            &name,
                            Binding::Local(LocalRecord {
                                port: port_ref(node.id, crate::intern::resolve(port.name)),
                            }),
                        );
                    }
                }
            }
        }
        LetBinding::RecordDestruct { .. } => {
            // Record destructuring of non-record RHS — nothing to do.
            // TODO: this is an error..?
        }
    }
}

/// The array/map literal a `let` initializer evaluates to, when it is an
/// array/map literal or names a constant with no runtime binding. A name that
/// is bound (a `var`, a materialized `const`) is left to the aliasing paths.
fn constant_container_init(ctx: &LowerCtx, value: &Expr) -> Option<Literal> {
    match value {
        Expr::Array { .. } | Expr::MapLit { .. } => {}
        Expr::Ident { name, .. } if ctx.scope.get(name).is_none() => {}
        _ => return None,
    }
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    match crate::const_eval::eval_expr(value, &ctx.const_ctx(Some(&lookup)), &mut budget) {
        Ok(lit @ (Literal::Array(_) | Literal::Map(_))) => Some(lit),
        _ => None,
    }
}

/// Whether a record-literal field value may be record-shaped, so it is worth
/// probing with `value_record_fields`: anything typecheck did not type as a
/// single-wire value. A scalar field skips the probe and is lowered once.
pub(super) fn record_valued(ctx: &LowerCtx, value: &Expr) -> bool {
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

/// Lower a record literal into a `HashMap<Sym, Binding>`. A field naming
/// storage binds that storage and is read where the record is used, so an
/// earlier field is held at its value before a later field's gates may change
/// what it reads (see `ordered::Ordered`): `{ x: q, y: bump() }` keeps
/// the `q` from before `bump()`.
pub(super) fn lower_record_lit(
    ctx: &mut LowerCtx,
    fields: &[RecordLitField],
    lit: Slots,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut out = HashMap::default();
    let mut ord = crate::lower::ordered::Ordered::default();
    let mut slots: HashMap<crate::intern::Sym, usize> = HashMap::default();
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
    for (k, b) in out.iter_mut() {
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
pub(super) fn supplied_by(
    ctx: &LowerCtx,
    fields: &[RecordLitField],
) -> HashSet<crate::intern::Sym> {
    let mut out = HashSet::default();
    for f in fields {
        match f {
            RecordLitField::Named { name, .. } | RecordLitField::Shorthand { name, .. } => {
                out.insert(crate::intern::intern(name));
            }
            RecordLitField::Spread { value, .. } => {
                if let Type::Record(fs) = unwrap_ref(&ctx.type_of(value)) {
                    out.extend(fs.iter().map(|(n, _)| crate::intern::intern(n)));
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
    ctx: &LowerCtx,
    binding: Binding,
    at: &SourceRange,
    lit: Slots,
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
    ctx: &mut LowerCtx,
    field: &RecordLitField,
    lit: Slots,
) -> Vec<(crate::intern::Sym, Binding)> {
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
                map.push((crate::intern::intern(name), Binding::Record(inner)));
            } else if let Some(binding) = resolve_field_chain(ctx, value)
                .filter(|_| !crate::lower::expr::reads_constant_as_other_number(ctx, value))
                .cloned()
            {
                // Value references something in scope (possibly through a record chain).
                let binding = field_value(ctx, binding, value.range(), lit, name);
                map.push((crate::intern::intern(name), binding));
            } else if let Some(inner) = (record_valued(ctx, value) && (!one_value || enum_slot))
                .then(|| crate::lower::stmt::value_record_fields(ctx, value))
                .flatten()
            {
                // A record-shaped field value with no binding of its own:
                // a record-returning call, an enum construction, a record
                // array/map element. The `lower_expr` fallback below keeps
                // one port and discards the rest, so such a field arrives as
                // a `Binding::Local`, which every consumer that walks a
                // `Binding::Record` target skips without a diagnostic. For
                // the parallel-array consumers that desyncs the columns'
                // LENGTHS, misaligning every later row. Uses the same
                // resolver as the spread arm below. Only a value typecheck
                // did not type as a scalar is probed, so a scalar field is
                // lowered once, through `lower_expr` below.
                let binding = Binding::Record(inner);
                map.push((
                    crate::intern::intern(name),
                    field_value(ctx, binding, value.range(), lit, name),
                ));
            } else {
                // A call the record probe above already lowered for this
                // same value keeps that one gate.
                let port = match crate::lower::stmt::take_value_record_port(ctx, value) {
                    Some(port) => port,
                    None => lower_expr(ctx, value),
                };
                map.push((
                    crate::intern::intern(name),
                    Binding::Local(LocalRecord { port }),
                ));
            }
        }
        RecordLitField::Shorthand { name, range } => {
            // { foo } means { foo: foo } — look up foo in scope.
            if let Some(binding) = ctx.scope.get(name).cloned() {
                let binding = field_value(ctx, binding, range, lit, name);
                map.push((crate::intern::intern(name), binding));
            }
        }
        RecordLitField::Spread { value, .. } => {
            // A `...expr` spread merges the fields of any record value (a
            // scope binding, a nested record literal, an index/map read, or
            // a record-returning call), later fields overriding earlier ones.
            if let Some(src_fields) = crate::lower::stmt::value_record_fields(ctx, value) {
                map.extend(src_fields);
            }
        }
    }
    map
}

/// Install record destructure bindings from a source record into the scope.
pub(super) fn install_record_destruct(
    ctx: &mut LowerCtx,
    src: &HashMap<crate::intern::Sym, Binding>,
    destruct_fields: &[RecordDestructField],
) {
    let mut remaining = src.clone();
    for field in destruct_fields {
        match field {
            RecordDestructField::Named {
                name, alias, range, ..
            } => {
                let key = crate::intern::intern(name);
                if let Some(binding) = remaining.remove(&key) {
                    let bind_name = alias.as_deref().unwrap_or(name);
                    ctx.scope.insert(&bind_name, binding);
                } else {
                    // Binding nothing would leave every use of the name an
                    // `_Unsupported` placeholder wired to no source — a circuit
                    // that silently reads 0. Field names are case-sensitive, so
                    // point at a differently-cased field when there is one.
                    let available: Vec<String> = src
                        .keys()
                        .map(|k| crate::intern::resolve(*k).to_string())
                        .collect();
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
                ctx.scope.insert(&name, Binding::Record(remaining.clone()));
            }
        }
    }
}

/// The field names of `ty`, in declaration order — the POSITIONS a tuple
/// pattern binds against. A `Binding::Record` is a `HashMap` and so carries no
/// order of its own; the static type does, for both spellings that reach a
/// tuple pattern: a tuple literal types as an index-keyed record (`"0"`,
/// `"1"`, …) and a multi-output `mod` call as a NAME-keyed one in its
/// signature's declaration order. Falls back to index keys when the type is
/// not a record, which is exactly the pre-existing behaviour.
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
    ctx: &mut LowerCtx,
    value: &Expr,
    src: &HashMap<crate::intern::Sym, Binding>,
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
/// re-keyed by its new position so `rest.0` is the first leftover — the same
/// re-keying `const_eval::destructure`'s `Tuple` arm does, so the compile-time
/// and runtime halves of the same pattern agree.
pub(super) fn install_tuple_destruct(
    ctx: &mut LowerCtx,
    src: &HashMap<crate::intern::Sym, Binding>,
    names: &[String],
    rest: Option<&String>,
    order: &[String],
) {
    for (i, name) in names.iter().enumerate() {
        if let Some(key) = order.get(i)
            && let Some(binding) = src.get(&crate::intern::intern(key)).cloned()
        {
            ctx.scope.insert(name, binding);
        }
    }
    if let Some(rest_name) = rest {
        let mut tail: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for (i, key) in order.iter().skip(names.len()).enumerate() {
            if let Some(binding) = src.get(&crate::intern::intern(key)).cloned() {
                tail.insert(crate::intern::intern(&i.to_string()), binding);
            }
        }
        ctx.scope.insert(rest_name, Binding::Record(tail));
    }
}
