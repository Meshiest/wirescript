use super::*;

// ---------- declaration body pass ----------

/// True if `p`'s declared output type is `string`, read straight off the IR
/// node's port spec (no typecheck-map dependency). A label value that is already
/// a string wires into the label's `Text` port directly; only a non-string is
/// routed through a `FormatText` gate to coerce it.
fn label_source_is_string(ctx: &LowerCtx, p: PortRef) -> bool {
    ctx.builder
        .module
        .nodes
        .get(&p.node_id)
        .and_then(|n| {
            n.ports
                .outputs
                .iter()
                .find(|s| s.name == crate::intern::intern(p.port.as_str()))
        })
        .map(|s| s.ty == Type::String)
        .unwrap_or(false)
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
/// string-coerce it if needed, and record `host_var → source_port` on the
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
        if expr_to_literal_in(le, &ctx.const_env).is_some() {
            continue; // constant label — already baked as static text.
        }
        // The host var node carries the label component at emit time.
        let host = match ctx.scope.get(&v.name) {
            Some(Binding::Var(rec)) => rec.node_id,
            _ => continue,
        };
        let src = lower_label_value(ctx, le);
        ctx.builder.module.dynamic_labels.insert(host, src);
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
    if let Some(text) = resolve_label_text(None, Some(le), &ctx.const_env) {
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
        TopDecl::Handler(h) => ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h)),
        TopDecl::Event(e) => ctx.with_nofold(e.no_fold, |ctx| lower_event_decl(ctx, e)),
        TopDecl::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
        TopDecl::Buffer(b) => lower_buffer_body(ctx, b),
        // Gate created in pre-pass; top level is pure, so a non-constant init
        // has no exec reset to apply it — surface the drop.
        TopDecl::Var(v) => ctx.with_nofold(v.no_fold, |ctx| warn_unbaked_var_init(ctx, v, true)),
        TopDecl::Array(_) | TopDecl::Map(_) | TopDecl::In(_) => {} // handled in pre-pass
        TopDecl::Chip(c) => lower_chip_decl(ctx, c),
        TopDecl::AnonChip(ac) => lower_anon_chip(ctx, ac),
        TopDecl::Assign(a) => lower_assign(ctx, a),
        TopDecl::If(i) => lower_if(ctx, i),
        TopDecl::ExprStmt(es) => {
            lower_expr(ctx, &es.expr);
        }
        TopDecl::Fn(f) => {
            // `fn` has been removed (rejected at parse with a hard error). A
            // recovered `fn` decl is still lowered as an inline mod-with-return so
            // a stray one doesn't crash lowering — there is no deprecation warning.
            // Synthesize a ChipDecl from the FnDecl
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
                closed: false,
                no_fold: false,
            };
            lower_chip_decl(ctx, &chip);
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) => {}
        TopDecl::Namespace(ns) => {
            let mut ns_decls = HashMap::default();
            let mut ns_buffers = Vec::new();
            for d in &ns.decls {
                match d {
                    TopDecl::Chip(c) => {
                        ns_decls.insert(c.name.clone(), std::sync::Arc::new(c.clone()));
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
                    TopDecl::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
                    TopDecl::Array(a) if ctx.scope.get(&a.name).is_none() => {
                        pre_declare_array(ctx, a)
                    }
                    TopDecl::Map(m) if ctx.scope.get(&m.name).is_none() => {
                        pre_declare_map(ctx, m)
                    }
                    TopDecl::Var(v) if ctx.scope.get(&v.name).is_none() => {
                        ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
                        // Module-level = pure: a non-constant init is dropped.
                        ctx.with_nofold(v.no_fold, |ctx| warn_unbaked_var_init(ctx, v, true));
                    }
                    TopDecl::Buffer(b) if ctx.scope.get(&b.name).is_none() => {
                        pre_declare_buffer(ctx, b);
                        ns_buffers.push(b);
                    }
                    _ => {}
                }
            }
            // Wire buffer initializers only after every ns member is in scope
            // (an init may reference a member declared after it). Only buffers
            // pre-declared above — a name the importer already owns stays its.
            for b in ns_buffers {
                lower_buffer_body(ctx, b);
            }
            ctx.scope
                .insert(&ns.name, Binding::Namespace(ns_decls));
        }
    }
}

pub(super) fn lower_buffer_body(ctx: &mut LowerCtx, d: &BufferDecl) {
    let rec = match ctx.lookup_buffer(&d.name) {
        Some(r) => r.clone(),
        None => return,
    };
    let saved_chain = ctx.builder.current_chain_id;
    let chain = ctx.alloc_chain();
    ctx.builder.current_chain_id = Some(chain);
    if let Some(node) = ctx.builder.module.nodes.get_mut(&rec.node_id) {
        node.chain_id = Some(chain);
    }
    let rhs_port = lower_expr(ctx, &d.init);
    // Through `ctx.connect` (not `builder.connect`) so the string → bool
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
    let chip_node_id = ctx
        .builder
        .module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.kind == NodeKind::Chip
                && n.source_range == d.range
                && n.chip_id == ctx.current_anon_chip
        })
        .map(|(id, _)| *id);
    let Some(chip_node_id) = chip_node_id else {
        return;
    };

    let saved_chip = ctx.current_anon_chip.take();
    ctx.current_anon_chip = Some(chip_node_id);

    lower_chip_body(ctx, &d.body);

    ctx.current_anon_chip = saved_chip;
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
/// trailing-emit terminates the chain).
fn lower_chip_body(ctx: &mut LowerCtx, block: &Block) {
    for s in &block.stmts {
        if let Stmt::AnonChip(ac) = s {
            pre_declare_decl(ctx, &TopDecl::AnonChip(ac.clone()));
        }
    }
    for s in &block.stmts {
        let is_handler_stmt = matches!(s, Stmt::Handler(_) | Stmt::AnonChip(_));
        if !ctx.handler_end_execs.is_empty() && !is_handler_stmt {
            flush_handler_end_execs(ctx);
        }
        if is_pure_chip_stmt(s) {
            let saved_exec = ctx.current_exec.take();
            lower_stmt(ctx, s);
            ctx.current_exec = saved_exec;
        } else {
            lower_stmt(ctx, s);
        }
    }
    if let Some(Stmt::Emit(e)) = block.stmts.last()
        && (ctx.signal_key(&e.name).is_some() || ctx.lookup_output(&e.name).is_some())
    {
        ctx.current_exec = None;
    }
}

pub(super) fn lower_chip_decl(ctx: &mut LowerCtx, d: &ChipDecl) {
    // Inline declarations (mod keyword or ref params) are stored for
    // expansion at call sites, not compiled as standalone microchips.
    let has_ref_params = d
        .inputs
        .iter()
        .any(|p| matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }));
    if d.inline || has_ref_params {
        ctx.scope
            .insert(&d.name, Binding::Chip(std::sync::Arc::new(d.clone())));
        return;
    }

    // Standalone chips: register for instantiation at call sites.
    ctx.scope
        .insert(&d.name, Binding::Chip(std::sync::Arc::new(d.clone())));
}

pub(super) fn lower_let_decl(ctx: &mut LowerCtx, d: &LetDecl) {
    // Clear any leftover inline-mod record so only THIS statement's call can set
    // it (an inline mod call within `d.value` sets it definitively at its end).
    ctx.pending_inline_record = None;

    // `let name: exec` — local exec signal, register as emit target. Checked
    // first and returns unconditionally: an exec-typed `let` (parsed with a
    // placeholder `0` value when it has no `= ...` initializer — see the
    // parser) never names a constant, so it must not reach the const
    // recording below.
    if let Some(TypeExpr::Name {
        name: ref type_name,
        ..
    }) = d.typ
    {
        if type_name == "exec" {
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

    // A body-local `let name = <constant>` is recorded in the innermost
    // `scoped_consts` frame (mirroring how a top-level `let` lands in
    // `const_env` via `build_const_env`), so a constant-only config arg
    // elsewhere in this scope (or a nested one) can resolve `name` via
    // `ctx.const_lookup()` — see `literal_for_property_port`. Only the
    // simple `Ident` binding form can name a single constant. This is a
    // no-op at the top level: `lower_let_decl` also handles `TopDecl::Let`
    // (via `lower_decl`), but no `push_scope` has run there yet, so
    // `scoped_consts` is empty and `last_mut()` finds no frame — top-level
    // constants are already covered by `const_env`.
    if let LetBinding::Ident { name, .. } = &d.binding
        && let Some(lit) = expr_to_literal_in(&d.value, &ctx.const_lookup())
        && let Some(frame) = ctx.scoped_consts.last_mut()
    {
        frame.insert(name.clone(), lit);
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
        let record = lower_record_lit(ctx, fields);
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(record));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &record, destruct_fields);
                return;
            }
            LetBinding::Tuple { names, rest, .. } => {
                install_tuple_destruct(ctx, &record, names, rest.as_ref());
                return;
            }
            _ => {
                // Record name destructuring on a record lit — fall through
                // to normal handling (unlikely but safe).
            }
        }
    }

    // Handle RHS that is an ident referencing a record binding.
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let Some(Binding::Record(src)) = ctx.scope.get(rhs_name).cloned()
    {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(src));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &src, destruct_fields);
                return;
            }
            LetBinding::Tuple { names, rest, .. } => {
                install_tuple_destruct(ctx, &src, names, rest.as_ref());
                return;
            }
            _ => {}
        }
    }

    // Handle RHS that is a field-chain resolving to a record binding.
    if let Some(Binding::Record(src)) = resolve_field_chain(ctx, &d.value).cloned() {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(src));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &src, destruct_fields);
                return;
            }
            LetBinding::Tuple { names, rest, .. } => {
                install_tuple_destruct(ctx, &src, names, rest.as_ref());
                return;
            }
            _ => {}
        }
    }

    let rhs_port = lower_expr(ctx, &d.value);
    let rhs_type = ctx.type_of(&d.value);

    // Multi-output inline mod: the call stashed a field→source-port record (its
    // output nodes are internal and were removed). Bind the record directly.
    if matches!(&d.value, Expr::Call { .. })
        && let Some(record) = ctx.pending_inline_record.take()
    {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(&name, Binding::Record(record));
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &record, destruct_fields);
            }
            _ => {}
        }
        return;
    }

    // Multi-output chip/call: the rhs_port points to the first output's
    // MicrochipOutput node. If the type is a Record, look up the chip node
    // that owns these outputs and build field→port bindings.
    if let Type::Record(ref fields) = rhs_type {
        if let Expr::Call { .. } = &d.value {
            // Find the chip node whose outputs include rhs_port.node_id
            let chip_entry = ctx
                .builder
                .module
                .chips
                .iter()
                .find(|(_, child)| child.outputs.contains(&rhs_port.node_id));
            if let Some((_, child)) = chip_entry {
                let outputs = child.outputs.clone();
                let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
                for (i, (field_name, _ty)) in fields.iter().enumerate() {
                    if let Some(&out_id) = outputs.get(i) {
                        record.insert(
                            crate::intern::intern(field_name),
                            Binding::Local(LocalRecord {
                                port: out_id.port(WirePort::RerOutput),
                            }),
                        );
                    }
                }
                match &d.binding {
                    LetBinding::Ident { name, .. } => {
                        ctx.scope.insert(&name, Binding::Record(record));
                    }
                    LetBinding::RecordDestruct {
                        fields: destruct_fields,
                        ..
                    } => {
                        install_record_destruct(ctx, &record, destruct_fields);
                    }
                    _ => {}
                }
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
                    let port =
                        resolve_output_field_port(ctx, rhs_port.node_id, field_name)?;
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
            if !record.is_empty()
                && let LetBinding::RecordDestruct {
                    fields: destruct_fields,
                    ..
                } = &d.binding
            {
                install_record_destruct(ctx, &record, destruct_fields);
                return;
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

/// Lower a record literal into a `HashMap<Sym, Binding>`.
pub(super) fn lower_record_lit(
    ctx: &mut LowerCtx,
    fields: &[RecordLitField],
) -> HashMap<crate::intern::Sym, Binding> {
    let mut map = HashMap::default();
    for field in fields {
        match field {
            RecordLitField::Named { name, value, .. } => {
                // Check if value is itself a record literal (nested records).
                if let Expr::RecordLit {
                    fields: inner_fields,
                    ..
                } = value
                {
                    let inner = lower_record_lit(ctx, inner_fields);
                    map.insert(crate::intern::intern(name), Binding::Record(inner));
                } else if let Some(binding) = resolve_field_chain(ctx, value).cloned() {
                    // Value references something in scope (possibly through a record chain).
                    map.insert(crate::intern::intern(name), binding);
                } else {
                    // Otherwise evaluate as expression and store as Local.
                    let port = lower_expr(ctx, value);
                    map.insert(
                        crate::intern::intern(name),
                        Binding::Local(LocalRecord { port }),
                    );
                }
            }
            RecordLitField::Shorthand { name, .. } => {
                // { foo } means { foo: foo } — look up foo in scope.
                if let Some(binding) = ctx.scope.get(name).cloned() {
                    map.insert(crate::intern::intern(name), binding);
                }
            }
            RecordLitField::Spread { value, .. } => {
                // ...expr — expr must resolve to a Binding::Record.
                if let Some(Binding::Record(src_fields)) = resolve_field_chain(ctx, value).cloned()
                {
                    for (k, v) in src_fields {
                        map.insert(k, v); // later fields override
                    }
                }
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
                        .map(|f| format!(" — did you mean `{f}`?"))
                        .unwrap_or_else(|| {
                            let mut names = available.clone();
                            names.sort();
                            format!(" — available fields: {}", names.join(", "))
                        });
                    ctx.diagnostics.push(Diagnostic::error(
                        "WS002",
                        format!("no field `{name}` on this value{suggestion}"),
                        range.clone(),
                    ));
                }
            }
            RecordDestructField::Rest { name, .. } => {
                ctx.scope
                    .insert(&name, Binding::Record(remaining.clone()));
            }
        }
    }
}

/// Bind a tuple pattern's names against a record source. Tuple literals lower
/// to a `Binding::Record` keyed by the element index (`"0"`, `"1"`, ...), so
/// positional names read straight out of that map. `rest` collects the tail,
/// re-indexed from zero so it stays a well-formed tuple.
pub(super) fn install_tuple_destruct(
    ctx: &mut LowerCtx,
    src: &HashMap<crate::intern::Sym, Binding>,
    names: &[String],
    rest: Option<&String>,
) {
    for (i, name) in names.iter().enumerate() {
        if let Some(binding) = src.get(&crate::intern::intern(&i.to_string())).cloned() {
            ctx.scope.insert(name, binding);
        }
    }
    if let Some(rest_name) = rest {
        let mut tail: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for (i, key) in (names.len()..src.len()).enumerate() {
            if let Some(binding) = src.get(&crate::intern::intern(&key.to_string())).cloned() {
                tail.insert(crate::intern::intern(&i.to_string()), binding);
            }
        }
        ctx.scope.insert(rest_name, Binding::Record(tail));
    }
}
