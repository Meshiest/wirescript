//! Build a microchip's child module.

use super::*;

/// What a chip instance's body is built from at one call site.
pub(super) struct ChipCallSite<'a> {
    pub(super) chip_decl: &'a ChipDecl,
    pub(super) instance_name: &'a str,
    pub(super) caller_captures: &'a HashMap<String, VarRecord>,
    /// Whether the body gets `_exec_in`/`_exec_out` boundary pins: the call has
    /// (or is handed) an exec context and the chip's first param is not exec.
    pub(super) auto_exec: bool,
    pub(super) mono_frame: Option<&'a MonoFrame>,
    pub(super) const_params: &'a ConstEnv,
    pub(super) immutable_params: &'a [String],
}

/// What the body of a chip inherits from the scope its declaration sees.
struct InheritedScope {
    cursor: Option<crate::scope::top::ReadAt>,
    consts: crate::const_scope::ConstFrames,
    hidden: Vec<crate::const_scope::HiddenAtTop>,
    frames: Vec<crate::scope::FrameCopy<Binding>>,
    chips: Vec<Arc<HashMap<String, Arc<ChipDecl>>>>,
    /// Every `_Literal` of this module (or one it reads from further out) that
    /// the body can reach: through a binding it inherits, or through the
    /// declaration scope of a mod declared in one of those frames, which may
    /// hold a binding the frame has replaced since.
    foreign_literals: HashMap<NodeId, Arc<Node>>,
}

/// The scope the body of `chip_decl` closes over: its declaration's, never the
/// call site's, so a caller's same-named local cannot capture a body name (see
/// `LowerCtx::enter_decl_scope`).
fn inherit_decl_scope(ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) -> InheritedScope {
    let decl_scope = ctx.enter_decl_scope(chip_decl);
    let cursor = ctx.consts.cursor();
    let consts = ctx.consts.snapshot();
    let hidden = ctx.consts.hidden_at_top();
    let frames: Vec<crate::scope::FrameCopy<Binding>> = ctx
        .scope
        .frames()
        .map(|(tag, id, entries)| (tag, id, entries.map(|(k, b)| (k, b.clone())).collect()))
        .collect();
    let chips = ctx.pass1_chips.clone();
    ctx.leave_decl_scope(decl_scope);
    let mut foreign_literals = ctx.foreign_literals.clone();
    {
        let frame_ids: HashSet<u64> = frames.iter().map(|(_, id, _)| *id).collect();
        let mut note = |b: &Binding| collect_literals(ctx, b, &mut foreign_literals);
        for (_, _, entries) in &frames {
            for (_, b) in entries {
                note(b);
            }
        }
        ctx.top_dup_bindings.values().for_each(&mut note);
        let keys: HashSet<&StartKey> = frame_ids
            .iter()
            .filter_map(|id| ctx.views_by_frame.get(id))
            .flatten()
            .collect();
        for key in keys {
            if let Some(view) = ctx.decl_views.get(key) {
                view.bindings().for_each(&mut note);
            }
        }
    }
    InheritedScope {
        cursor,
        consts,
        hidden,
        frames,
        chips,
        foreign_literals,
    }
}

pub(super) fn build_chip_module(ctx: &mut LowerCtx<'_>, site: &ChipCallSite<'_>) -> Module {
    let chip_decl = site.chip_decl;
    // The frame the body lowers under: a generic chip's `mono_frame`, extended
    // from any enclosing generic body's, so every `T` in the body and its
    // boundary ports resolves to the concrete type. `None` for a non-generic
    // chip declared outside every generic body.
    let body_frame = ctx.body_frame(chip_decl, site.mono_frame);
    let mut child_builder = ModuleBuilder::new(site.instance_name);
    child_builder.module.scopes.insert(
        ROOT_SCOPE_ID,
        ScopeInfo {
            kind: ScopeKind::ChipBody {
                name: chip_decl.name.clone(),
            },
            source_range: chip_decl.range.clone(),
            parent: None,
        },
    );
    let InheritedScope {
        cursor,
        consts,
        hidden,
        frames,
        chips,
        foreign_literals,
    } = inherit_decl_scope(ctx, chip_decl);
    let mut child_ctx = ctx.for_chip_body(
        site,
        child_builder,
        body_frame,
        consts,
        chips,
        foreign_literals,
    );

    child_ctx.scope = inherited_frames_scope(ctx, &mut child_ctx, frames);
    child_ctx.consts.set_cursor(cursor);
    child_ctx.push_scope(crate::scope::ScopeTag::MODULE);
    child_ctx.consts.hide_held_at_top(hidden);

    bind_chip_params(&mut child_ctx, site);
    bind_ns_consts(ctx, &mut child_ctx, chip_decl);
    bind_chip_outputs(&mut child_ctx, chip_decl);

    // Auto-exec: if the caller has exec context (or supplies an `exec =`
    // named arg from a pure context) and the chip doesn't explicitly take
    // exec as its first param, create exec entry/exit boundary ports so the
    // chip body receives the exec chain.
    let auto_exec = site.auto_exec;
    if auto_exec {
        let exec_in = child_ctx.add_input("_exec_in", Type::Exec, chip_decl.range.clone());
        child_ctx.current_exec = Some(exec_in.port(WirePort::RerOutput));
    }

    // Returns on the body's own path, and returns inside nested handlers,
    // which run on the handler's trigger (see `count_handler_return_values`).
    let returns = count_return_values(&chip_decl.body);
    let handler_returns = count_handler_return_values(&chip_decl.body);

    pre_declare_chip_body(&mut child_ctx, chip_decl);
    alloc_output_backing(&mut child_ctx, chip_decl, returns, handler_returns);
    bind_return_var(
        &mut child_ctx,
        chip_decl,
        auto_exec,
        returns,
        handler_returns,
    );

    for stmt in &chip_decl.body.stmts {
        lower_stmt(&mut child_ctx, stmt);
    }

    if auto_exec {
        close_exec_boundary(&mut child_ctx, chip_decl);
    }
    wire_output_backing(&mut child_ctx);
    // The body's own pending emits target its own outputs and hubs, so it
    // flushes them itself; they would be dropped with `child_ctx` otherwise.
    super::stmt::flush_pending_emits(&mut child_ctx);

    ctx.diagnostics.extend(child_ctx.diagnostics);
    ctx.dropped_ranges.extend(child_ctx.dropped_ranges);
    let mut module = child_ctx.builder.module;
    module.scope_captures = compute_scope_captures(&module);
    module
}

/// The scope a chip body starts from: wire refs cross the chip boundary freely,
/// so the body closes over the whole lexical scope of its declaration, kept as
/// frames so a mod it calls resolves in its own declaration's frame. The nearest
/// `let X = <const>` per name gets its `_Literal` copied into the chip's module,
/// so `inline_orphan_literals` can fold it into its consumers as inline gate
/// data.
fn inherited_frames_scope(
    ctx: &LowerCtx<'_>,
    child_ctx: &mut LowerCtx<'_>,
    mut frames: Vec<crate::scope::FrameCopy<Binding>>,
) -> crate::scope::Scope<Binding> {
    let mut seen = HashSet::default();
    for (_, _, entries) in frames.iter_mut().rev() {
        // A chip body cannot target the enclosing module's `out`s; inheriting
        // them would inflate `output_count()`, and `Stmt::Return` wires its
        // value only when that is 1.
        entries.retain(|(_, b)| !matches!(b, Binding::Output(_)));
        for (name, binding) in entries.iter_mut() {
            if !seen.insert(*name) {
                continue;
            }
            if let Some(copy) = literal_copy(ctx, child_ctx, binding) {
                *binding = copy;
            }
        }
    }
    crate::scope::Scope::from_frames(frames)
}

/// Bind each parameter of the chip in the body's scope: a `const` as a
/// constant, anything else as the boundary pins that carry its value, then
/// expand destructuring patterns.
fn bind_chip_params(child_ctx: &mut LowerCtx<'_>, site: &ChipCallSite<'_>) {
    let chip_decl = site.chip_decl;
    for inp in &chip_decl.inputs {
        // A `const` parameter is a compile-time constant with no pin.
        // `wire_chip_args_and_outputs` skips it on the same `is_const`
        // predicate; any mismatch shifts every later pin index and mis-wires
        // the call.
        if inp.is_const {
            if let Some(lit) = site.const_params.get(&inp.name) {
                let marks = crate::const_scope::Marks {
                    declared: true,
                    ..Default::default()
                };
                child_ctx.consts.record(&inp.name, lit.clone(), marks);
                let ty = child_ctx
                    .declared_type(chip_decl, &inp.typ, child_ctx.mono_stack.last())
                    .unwrap_or_else(|| child_ctx.resolve_local_type(&inp.typ));
                bind_shadowing_const_param(
                    child_ctx,
                    &inp.name,
                    lit.clone(),
                    &ty,
                    &chip_decl.range,
                    &chip_decl.body,
                );
            }
            continue;
        }
        let resolved_record = child_ctx.record_or_tuple_fields(&inp.typ);
        if let Some(fields) = &resolved_record {
            let record_fields = explode_record_param_pins(
                child_ctx,
                &inp.name,
                fields,
                site.caller_captures,
                &chip_decl.range,
            );
            child_ctx
                .scope
                .insert(&inp.name, Binding::Record(record_fields));
        } else if child_ctx.ref_storage(Some(chip_decl), &inp.typ).is_some() {
            bind_container_pin(child_ctx, site, inp);
        } else {
            let t = if super::inline::is_exec_type(child_ctx, &inp.typ) {
                Type::Exec
            } else {
                child_ctx.resolve_local_type(&inp.typ)
            };
            let node_id = child_ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
            child_ctx
                .scope
                .insert(&inp.name, Binding::Input(NodeRecord { node_id, ty: t }));
        }
    }
    // Destructuring params (`chip f({x, y}: P)`): the record bound above under
    // the synthetic param name expands into the named fields the body reads.
    for inp in &chip_decl.inputs {
        let Some(pattern) = &inp.pattern else {
            continue;
        };
        let base = child_ctx.scope.get(&inp.name).cloned();
        match pattern {
            ParamPattern::Record { fields, .. } => {
                if let Some(Binding::Record(src)) = base {
                    install_record_destruct(child_ctx, &src, fields);
                }
            }
            ParamPattern::Tuple { names, rest } => {
                if let Some(Binding::Record(src)) = base {
                    let order: Vec<String> = (0..names.len().max(src.len()))
                        .map(|i| i.to_string())
                        .collect();
                    install_tuple_destruct(child_ctx, &src, names, rest.as_ref(), &order);
                }
            }
        }
    }
}

/// Bind a container parameter (`T[]`, `Map<K,V>`, `ref T`): the caller's
/// captured storage when it names one, otherwise a ref pin.
fn bind_container_pin(child_ctx: &mut LowerCtx<'_>, site: &ChipCallSite<'_>, inp: &Param) {
    let chip_decl = site.chip_decl;
    if let Some(captured) = site.caller_captures.get(&inp.name) {
        if site.immutable_params.contains(&inp.name) {
            child_ctx.immutable_containers.insert(captured.node_id);
        }
        child_ctx.scope.insert(
            &inp.name,
            Binding::Var(VarRecord::new(
                captured.node_id,
                captured.inner_type.clone(),
                captured.storage,
            )),
        );
    } else {
        let t = child_ctx.resolve_local_type(&inp.typ);
        let (storage, inner) =
            super::context::container_binding(child_ctx.ref_storage(Some(chip_decl), &inp.typ), &t)
                .expect("gated by ref_storage");
        let node_id = child_ctx.add_input(&inp.name, t, chip_decl.range.clone());
        if site.immutable_params.contains(&inp.name) {
            child_ctx.immutable_containers.insert(node_id);
        }
        // An uncaptured `*T` pin carries a ref VALUE from the caller,
        // so reads and writes go through the pin's wire.
        let binding = if storage == VarStorage::Var {
            Binding::DynRef(DynRefRecord {
                port: node_id.port(WirePort::RerOutput),
                storage,
                inner_type: inner,
            })
        } else {
            Binding::Var(VarRecord::new(node_id, inner, storage))
        };
        child_ctx.scope.insert(&inp.name, binding);
    }
}

/// A chip declared in an `import * as` module reads that module's
/// constants, each one a parameter does not rebind, as a namespaced `mod`
/// body does (see `inline.rs`).
fn bind_ns_consts(ctx: &LowerCtx<'_>, child_ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) {
    let ns_key = chip_decl.range.start_key();
    if let (Some(ns_scope), Some(ns_consts)) = (
        child_ctx.ns_mod_scopes.get(&ns_key).cloned(),
        child_ctx.ns_mod_consts.get(&ns_key).cloned(),
    ) {
        for (name, lit, marks) in ns_consts.iter() {
            if child_ctx.scope.get_current(name).is_some() {
                continue;
            }
            if let Some(member) = ns_scope.get(name) {
                let member = literal_copy(ctx, child_ctx, member).unwrap_or_else(|| member.clone());
                child_ctx.scope.insert(name, member);
            }
            child_ctx.consts.record(name, lit.clone(), *marks);
            own_constant_container(child_ctx, name, lit, &chip_decl.range);
        }
    }
}

/// Add an output pin for each signature output of the chip.
fn bind_chip_outputs(child_ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) {
    for out in &chip_decl.outputs {
        // A record or tuple signature output gets one pin per leaf, bound in
        // the `Binding::Record` shape `pre_declare_output` gives a top-level
        // record `out`, so the body's `out p = { ... }` has pins to land on.
        if child_ctx.record_or_tuple_fields(&out.typ).is_some() {
            let fields = record_output_pins(child_ctx, &out.typ, &out.name, &chip_decl.range, 0);
            child_ctx
                .scope
                .insert(&output_scope_key(&out.name), Binding::Record(fields));
            continue;
        }
        let t = if super::inline::is_exec_type(child_ctx, &out.typ) {
            Type::Exec
        } else {
            child_ctx.resolve_local_type(&out.typ)
        };
        let node_id = child_ctx.add_output(&out.name, t.clone(), chip_decl.range.clone());
        child_ctx.scope.insert(
            &output_scope_key(&out.name),
            Binding::Output(NodeRecord { node_id, ty: t }),
        );
    }
}

/// Pre-declare the chip body's top-level storage, ports and nested chips, in
/// source order.
fn pre_declare_chip_body(child_ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) {
    let sig_output_names: HashSet<&str> =
        chip_decl.outputs.iter().map(|o| o.name.as_str()).collect();
    record_later_storage(child_ctx, &chip_decl.body);
    let stmts = &Mentions::new(&chip_decl.body.stmts);
    for (at, stmt) in chip_decl.body.stmts.iter().enumerate() {
        match stmt {
            // This chip's own nested chip/mod declarations overlay the
            // inherited `pass1_chips`, so a chip-local `const mod` shadows a
            // same-named top-level one. Must stay in this source-ordered loop
            // (see `pre_declare_chip_name`).
            Stmt::ChipDecl(c) => pre_declare_chip_name(child_ctx, c),
            // An anonymous `chip { }` in this body needs its shell declared:
            // `lower_anon_chip` silently skips a chip with none, dropping its
            // whole body.
            Stmt::AnonChip(ac) => pre_declare_anon_chip(child_ctx, ac, false),
            Stmt::In(i) => pre_declare_input(child_ctx, i),
            // A name the body already binds (a parameter) is declared by its
            // statement instead (see `hoist_or_defer`).
            Stmt::Var(v) if hoist_or_defer(child_ctx, stmts, at, &v.name, &v.range) => {
                child_ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v));
            }
            Stmt::Buffer(b) if hoist_or_defer(child_ctx, stmts, at, &b.name, &b.range) => {
                pre_declare_buffer(child_ctx, b);
            }
            Stmt::Array(a) if hoist_or_defer(child_ctx, stmts, at, &a.name, &a.range) => {
                pre_declare_array(child_ctx, a);
            }
            Stmt::Map(m) if hoist_or_defer(child_ctx, stmts, at, &m.name, &m.range) => {
                pre_declare_map(child_ctx, m);
            }
            Stmt::OutBinding(o) if !sig_output_names.contains(o.name.as_str()) => {
                child_ctx.with_nofold(o.no_fold, |ctx| {
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
            }
            _ => {}
        }
    }
}

/// Whether the chip's one output is a record or tuple, backed field by field.
fn has_record_only_output(child_ctx: &LowerCtx<'_>, chip_decl: &ChipDecl) -> bool {
    chip_decl.outputs.len() == 1
        && child_ctx
            .record_or_tuple_fields(&chip_decl.outputs[0].typ)
            .is_some()
}

/// An output this body emits from two sites, from a branch, beside its own
/// `out` default, or beside a `return` is backed by a var, as a top-level
/// one is (see `create_output_backing_var`): each site stores, and the var
/// drives the output once. Two sites wired straight into the output are a
/// load-time fan-in. A record output is backed field by field.
fn alloc_output_backing(
    child_ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    returns: usize,
    handler_returns: usize,
) {
    let value_outputs: Vec<&str> = chip_decl
        .outputs
        .iter()
        .filter(|o| !super::inline::is_exec_type(child_ctx, &o.typ))
        .map(|o| o.name.as_str())
        .collect();
    if chip_decl.outputs.len() > 1 || has_record_only_output(child_ctx, chip_decl) {
        child_ctx.chip_return_outputs = chip_decl
            .outputs
            .iter()
            .map(|o| {
                (
                    o.name.clone(),
                    super::inline::is_exec_type(child_ctx, &o.typ),
                )
            })
            .collect();
    }
    let mut counts = HashMap::default();
    crate::lower::count_emits_in_block(&chip_decl.body, false, &mut counts);
    crate::lower::collect_value_emit_names(&chip_decl.body, &mut child_ctx.emitted_outputs);
    // Two returns in a chip with several outputs, or with one record
    // output, each write every value output, so each is backed like an
    // output emitted from two sites. So is every output a return in a
    // nested handler writes: it holds the value the handler last stored.
    if (returns > 1 && !child_ctx.chip_return_outputs.is_empty()) || handler_returns > 0 {
        for name in &value_outputs {
            let entry = counts.entry(name.to_string()).or_insert((0, false));
            entry.0 += 1;
            entry.1 = true;
        }
    }
    let defaults: HashSet<&str> = chip_decl
        .body
        .stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::OutBinding(o) if o.value.is_some() => Some(o.name.as_str()),
            _ => None,
        })
        .collect();
    let mut names: Vec<(String, (usize, bool))> = counts.into_iter().collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, (count, in_branch)) in names {
        // A body-level `out name = v` was counted as a site; it is the default.
        let has_default = defaults.contains(name.as_str());
        let emits = count - usize::from(has_default);
        if emits == 0 || (emits < 2 && !in_branch && !has_default && returns == 0) {
            continue;
        }
        let record_out = chip_decl
            .outputs
            .iter()
            .find(|o| o.name == name)
            .and_then(|o| child_ctx.record_or_tuple_fields(&o.typ));
        match record_out {
            Some(fields) => {
                let backing = build_record_fields(
                    child_ctx,
                    VarStorage::Var,
                    &fields,
                    &name,
                    None,
                    None,
                    &chip_decl.range,
                );
                child_ctx.output_record_backing.insert(name, backing);
            }
            None => crate::lower::create_output_backing_var(child_ctx, &name),
        }
    }
}

/// Pick the var each `return` of the body stores into.
fn bind_return_var(
    child_ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    auto_exec: bool,
    returns: usize,
    handler_returns: usize,
) {
    // A `return` beside an `emit` of the one output, or in a nested handler,
    // stores into its backing var, like the emit.
    if ((auto_exec && returns >= 1) || handler_returns >= 1)
        && chip_decl.outputs.len() == 1
        && let Some(backing) = child_ctx
            .output_backing_vars
            .get(&chip_decl.outputs[0].name)
            .cloned()
    {
        child_ctx.mod_return_var = Some(backing);
    }
    // A multi-return chip with one output holds the returned value in a
    // PseudoVar: each `return expr` stores into it and one Var_Get after the
    // return union feeds the output, since two returns wired straight into
    // the pin are a load-time fan-in. Needs an exec context.
    if auto_exec
        && returns > 1
        && chip_decl.outputs.len() == 1
        && !has_record_only_output(child_ctx, chip_decl)
        && child_ctx.mod_return_var.is_none()
    {
        let out_type = child_ctx.resolve_local_type(&chip_decl.outputs[0].typ);
        child_ctx.mod_return_var =
            Some(return_value_var(child_ctx, out_type, &chip_decl.body.range));
    }
}

/// Drive `_exec_out` from the body's exec tail, after reading a multi-return
/// value on it.
fn close_exec_boundary(child_ctx: &mut LowerCtx<'_>, chip_decl: &ChipDecl) {
    // A trailing `return` moved the body's tail exec into mod_return_exec
    // (leaving current_exec = None); merge it with any fallthrough so a
    // body that ends in `return` still drives `_exec_out`.
    if let Some(ret) = child_ctx.mod_return_exec.take() {
        let fall = child_ctx.current_exec.take();
        child_ctx.current_exec = merge_exec(child_ctx, fall, Some(ret), &chip_decl.range);
    }
    // Multi-return: read the return value on the merged exec into the
    // single value output. Runs before `_exec_out` so the read's exec is
    // the chip's exec tail. A backing var drives its output itself (below).
    if let Some(ret_var) = child_ctx.mod_return_var.clone()
        && !child_ctx
            .output_backing_vars
            .values()
            .any(|v| v.node_id == ret_var.node_id)
        && let Some(exec) = child_ctx.current_exec
    {
        // A backed output is driven by its backing var, wired below.
        let drive_output = child_ctx.output_backing_vars.is_empty();
        read_return_value(child_ctx, &ret_var, exec, drive_output);
    }
    if let Some(tail_exec) = child_ctx.current_exec {
        let exec_out = child_ctx.add_output("_exec_out", Type::Exec, chip_decl.range.clone());
        child_ctx.connect(tail_exec, exec_out.port(WirePort::RerInput));
    }
}

/// Wire each output backing, record and var, into its output once, after every
/// site's write.
fn wire_output_backing(child_ctx: &mut LowerCtx<'_>) {
    // Each record backing drives its output's field pins once.
    let mut records: Vec<(String, Fields)> = child_ctx
        .output_record_backing
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, backing) in records {
        if let Some(Binding::Record(pins)) = child_ctx.scope.get(&output_scope_key(&name)).cloned()
        {
            wire_record_backing(child_ctx, &backing, &pins);
        }
    }
    // Each backing var drives its output once, after every site's write.
    let mut backings: Vec<(String, VarRecord)> = child_ctx
        .output_backing_vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    backings.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, backing) in backings {
        if let Some(out) = child_ctx.lookup_output(&name).cloned() {
            child_ctx.connect(
                backing.node_id.port(WirePort::Value),
                out.node_id.port(WirePort::RerInput),
            );
        }
    }
}

impl LowerCtx<'_> {
    /// The context a chip body lowers in: a fresh module over `builder` that
    /// inherits the declaration's constants, frames and caches from `self`.
    fn for_chip_body(
        &self,
        site: &ChipCallSite<'_>,
        child_builder: ModuleBuilder,
        body_frame: Option<MonoFrame>,
        inherited_consts: crate::const_scope::ConstFrames,
        inherited_chips: Vec<Arc<HashMap<String, Arc<ChipDecl>>>>,
        foreign_literals: HashMap<NodeId, Arc<Node>>,
    ) -> Self {
        let ChipCallSite {
            chip_decl,
            mono_frame,
            const_params,
            ..
        } = *site;
        LowerCtx {
            file: self.file.clone(),
            type_aliases: self.type_aliases.clone(),
            enum_defs: std::sync::Arc::clone(&self.enum_defs),
            template_cache: std::sync::Arc::clone(&self.template_cache),
            chip_call_stack: {
                let mut stack = self.chip_call_stack.clone();
                if let Some(top) = stack.last_mut()
                    && top.0 == chip_decl.range
                {
                    top.1 = true;
                }
                stack
            },
            known_fn_names: std::sync::Arc::clone(&self.known_fn_names),
            imports: std::sync::Arc::clone(&self.imports),
            known_let_names: std::sync::Arc::clone(&self.known_let_names),
            // `@nofold chip Foo(...)`: every gate of the body (built once,
            // cloned per `template.instantiate`) carries `_nofold`, set here
            // because the body lowers in this fresh ctx.
            nofold_depth: u32::from(chip_decl.no_fold),
            // The body's resolution key: this chip's declaration key, then its
            // type arguments when it is generic.
            resolution_subst: self.body_resolution_subst(chip_decl, mono_frame),
            // Carried so a mod declared inside an enclosing generic body keeps
            // its declaration key when called from this chip.
            decl_resolution_subst: self.decl_resolution_subst.clone(),
            // Carried for the same reason: a chip declared inside an enclosing
            // generic body keeps that body's frame when instanced from this chip.
            decl_mono_frame: self.decl_mono_frame.clone(),
            // Carried so a mod declared inside an enclosing body keeps its
            // declaration frame when called from this chip.
            decl_views: self.decl_views.clone(),
            views_by_frame: self.views_by_frame.clone(),
            decl_later_storage: self.decl_later_storage.clone(),
            // A mod the body calls reads the top level as its declaration sees it.
            top_names: Arc::new(self.top_names.dups_only()),
            // The body inherits the caller's frames, and with them the reads the
            // caller has cached.
            var_gets: self.var_gets.clone(),
            top_dup_bindings: std::sync::Arc::clone(&self.top_dup_bindings),
            foreign_literals,
            // A chip instantiated inside a template build is part of that build.
            template_build: self.template_build,
            // One template per `(name, subst)` (keyed in
            // `lower_chip_call_instance`), so each instantiation builds its
            // own body under its frame, and every `T` in it resolves to the
            // concrete type. Empty for a body outside every generic.
            mono_stack: body_frame.map(|f| vec![f]).unwrap_or_default(),
            // The declaration scope's constants, frame by frame, with the
            // `const` parameters as module constants that resolve through
            // `const_lookup()` and count as const-declared. Their values are
            // part of the template cache key, so different constants never
            // share a body. A body-level `const` lands in the body's own frame
            // (pushed below) and shadows an outer one, as typecheck has it.
            consts: self.consts.for_chip_body(const_params, inherited_consts),
            // The `pass1_chips` frame stack the declaration sees, so a body
            // initializer resolves a `const mod` call as the enclosing code
            // would (a chip inside an inlined `mod` sees that mod's frame). A
            // clone, so this chip's nested declarations never reach the caller.
            pass1_chips: inherited_chips,
            // The body inherits the caller's bindings (below), so a constant
            // container it reaches through one stays immutable here.
            immutable_containers: self.immutable_containers.clone(),
            importer_names: self.importer_names.clone(),
            ns_mod_scopes: self.ns_mod_scopes.clone(),
            ns_mod_consts: self.ns_mod_consts.clone(),
            // Everything else starts empty. `import_state_dedup` and
            // `import_behavior_lowered` must: a chip body performs no import
            // merge, and its per-instance state must never dedup across
            // instances by source location.
            ..LowerCtx::empty(
                child_builder,
                self.type_of_expr,
                self.op_resolutions,
                self.resolutions,
                self.ce_slots,
                self.doc_comments,
            )
        }
    }
}

/// Explode a record-typed chip param into its leaf input pins, returning the
/// `Binding::Record` the body reads fields through: a container field is one
/// ref pin, a nested record recurses, a scalar is one by-value pin. The
/// caller-side `wire_record_param_pins` must mirror this order exactly.
fn explode_record_param_pins(
    child_ctx: &mut LowerCtx<'_>,
    prefix: &str,
    fields: &[RecordTypeField],
    caller_captures: &HashMap<String, VarRecord>,
    chip_range: &SourceRange,
) -> Fields {
    let mut record_fields = HashMap::default();
    for field in fields {
        let port_name = format!("{prefix}_{}", field.name);
        let ft = child_ctx.resolve_local_type(&field.typ);
        // Array / Map / ref fields bind a container ref-port; classified via
        // `container_binding` so a `Map<K,V>` field wires its `MapVarRef`.
        let container =
            super::context::container_binding(child_ctx.ref_storage(None, &field.typ), &ft);
        if let Some(captured) = container
            .is_some()
            .then(|| caller_captures.get(&port_name))
            .flatten()
        {
            record_fields.insert(
                intern(&field.name),
                Binding::Var(VarRecord::new(
                    captured.node_id,
                    captured.inner_type.clone(),
                    captured.storage,
                )),
            );
            continue;
        }
        if let Some((storage, inner)) = container {
            let node_id = child_ctx.add_input(&port_name, ft.clone(), chip_range.clone());
            // An uncaptured `*T` field's pin carries a ref, so the field is
            // read and written through it, as an uncaptured `*T` parameter is.
            let binding = if storage == VarStorage::Var {
                Binding::DynRef(DynRefRecord {
                    port: node_id.port(WirePort::RerOutput),
                    storage,
                    inner_type: inner,
                })
            } else {
                Binding::Var(VarRecord::new(node_id, inner, storage))
            };
            record_fields.insert(intern(&field.name), binding);
            continue;
        }
        // A nested record/tuple recurses into per-leaf pins; a scalar is one pin.
        if let Some(sub) = child_ctx.record_or_tuple_fields(&field.typ) {
            let nested =
                explode_record_param_pins(child_ctx, &port_name, &sub, caller_captures, chip_range);
            record_fields.insert(intern(&field.name), Binding::Record(nested));
        } else {
            let node_id = child_ctx.add_input(&port_name, ft.clone(), chip_range.clone());
            record_fields.insert(
                intern(&field.name),
                Binding::Input(NodeRecord {
                    node_id,
                    ty: ft.clone(),
                }),
            );
        }
    }
    record_fields
}

/// Wire each scalar field of a record output's backing into the matching
/// output pin, recursing through nested record fields.
pub(in crate::lower) fn wire_record_backing(
    ctx: &mut LowerCtx<'_>,
    backing: &Fields,
    pins: &Fields,
) {
    let mut names: Vec<Sym> = pins.keys().copied().collect();
    names.sort_by_key(|k| resolve(*k));
    for k in names {
        match (backing.get(&k), &pins[&k]) {
            (Some(Binding::Record(b)), Binding::Record(p)) => {
                wire_record_backing(ctx, &b.clone(), &p.clone());
            }
            (Some(Binding::Var(v)), Binding::Output(out)) => {
                ctx.connect(
                    v.node_id.port(WirePort::Value),
                    out.node_id.port(WirePort::RerInput),
                );
            }
            _ => {}
        }
    }
}

/// Record every `_Literal` of this module that `b` names a port of, into
/// `out` (see `LowerCtx::foreign_literals`).
fn collect_literals(
    ctx: &LowerCtx<'_>,
    b: &Binding,
    out: &mut HashMap<NodeId, std::sync::Arc<Node>>,
) {
    match b {
        Binding::Local(l) => {
            if let Some(n) = ctx.builder.module.nodes.get(&l.port.node_id)
                && n.gate_class == gc::LITERAL
            {
                out.entry(l.port.node_id)
                    .or_insert_with(|| std::sync::Arc::new(n.clone()));
            }
        }
        Binding::Record(fields) => fields.values().for_each(|f| collect_literals(ctx, f, out)),
        _ => {}
    }
}

/// `binding` read from the chip's own module: a `let` of a constant in the
/// enclosing module points at a `_Literal` there, so the chip gets a copy of
/// that literal (`None` for any other binding).
fn literal_copy(
    ctx: &LowerCtx<'_>,
    child_ctx: &mut LowerCtx<'_>,
    binding: &Binding,
) -> Option<Binding> {
    let Binding::Local(local) = binding else {
        return None;
    };
    let src = ctx
        .builder
        .module
        .nodes
        .get(&local.port.node_id)
        .filter(|src| src.gate_class == gc::LITERAL)?;
    let opts = AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: src.source_range.clone(),
        ports: (*src.ports).clone(),
        properties: (*src.properties).clone(),
        ..Default::default()
    };
    let new_id = child_ctx.add_gate(opts);
    let copy = new_id.port(local.port.port);
    child_ctx.literal_copies.entry(local.port).or_insert(copy);
    Some(Binding::Local(LocalRecord { port: copy }))
}
