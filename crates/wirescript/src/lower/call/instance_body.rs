//! Build a microchip's child module.

use super::*;
use crate::collections::HashSet;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_chip_module(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    instance_name: &str,
    caller_captures: &HashMap<String, VarRecord>,
    force_exec_boundary: bool,
    mono_frame: Option<MonoFrame>,
    const_params: &ConstEnv,
    immutable_params: &[String],
) -> Module {
    // A generic chip is monomorphized for this instantiation: `mono_frame` is
    // `Some` only for a generic chip, and seeding the child ctx's `mono_stack`
    // with it makes every `T`-annotated storage/operator/if-expr/return in the
    // body — and its boundary ports (below) — resolve to the concrete type.
    //
    // A chip declared inside a generic body (generic itself or not) lowers
    // under that body's `T` too: its frame is the enclosing one extended by
    // its own, and its `T`-typed pins resolve through `resolve_local_type`
    // like a generic chip's. `body_frame` is `None`
    // exactly for a non-generic chip declared outside every generic body,
    // which leaves the stack empty and resolution unaffected.
    let body_frame = ctx.body_frame(chip_decl, mono_frame.as_ref());
    let mut child_builder = ModuleBuilder::new(instance_name);
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
    // The body closes over the scope its DECLARATION sees, never the call
    // site's: a caller's local or param named like a name the body uses must
    // not capture it (see `LowerCtx::enter_decl_scope`). Everything taken from
    // the caller's scope below is read inside it.
    let decl_scope = ctx.enter_decl_scope(chip_decl);
    let inherited_cursor = ctx.consts.cursor();
    let inherited_consts = ctx.consts.snapshot();
    let inherited_hidden = ctx.consts.hidden_at_top();
    let inherited_frames: Vec<crate::scope::FrameCopy<Binding>> = ctx
        .scope
        .frames()
        .map(|(tag, id, entries)| (tag, id, entries.map(|(k, b)| (k, b.clone())).collect()))
        .collect();
    let inherited_chips = ctx.pass1_chips.clone();
    ctx.leave_decl_scope(decl_scope);
    // Every `_Literal` of this module (or one it reads from further out) that
    // the body can reach: through a binding it inherits, or through the
    // declaration scope of a mod declared in one of those frames, which may
    // hold a binding the frame has replaced since.
    let mut foreign_literals = ctx.foreign_literals.clone();
    {
        let frame_ids: HashSet<u64> = inherited_frames.iter().map(|(_, id, _)| *id).collect();
        let mut note = |b: &Binding| collect_literals(ctx, b, &mut foreign_literals);
        for (_, _, entries) in &inherited_frames {
            entries.iter().for_each(|(_, b)| note(b));
        }
        ctx.top_dup_bindings.values().for_each(&mut note);
        let keys: HashSet<&(std::sync::Arc<str>, usize)> =
            frame_ids.iter().filter_map(|id| ctx.views_by_frame.get(id)).flatten().collect();
        for key in keys {
            if let Some(view) = ctx.decl_views.get(key) {
                view.bindings().for_each(&mut note);
            }
        }
    }
    let mut child_ctx = LowerCtx {
        file: ctx.file.clone(),
        type_aliases: ctx.type_aliases.clone(),
        enum_defs: ctx.enum_defs.clone(),
        template_cache: ctx.template_cache.clone(),
        chip_call_stack: {
            let mut stack = ctx.chip_call_stack.clone();
            if let Some(top) = stack.last_mut()
                && top.0 == chip_decl.range
            {
                top.1 = true;
            }
            stack
        },
        known_fn_names: ctx.known_fn_names.clone(),
        imports: ctx.imports.clone(),
        known_let_names: ctx.known_let_names.clone(),
        // `@nofold chip Foo(...) { ... }`: every gate lowered into this
        // child module (the chip's own body — built once and cloned for
        // every subsequent `template.instantiate` call) must carry
        // `_nofold` from the start, since the body is lowered in this
        // fresh `child_ctx`, not the caller's `ctx`.
        nofold_depth: if chip_decl.no_fold { 1 } else { 0 },
        // The body's resolution key: this chip's declaration key, then its
        // type arguments when it is generic.
        resolution_subst: ctx.body_resolution_subst(chip_decl, mono_frame.as_ref()),
        // Carried so a mod declared inside an enclosing generic body keeps
        // its declaration key when called from this chip.
        decl_resolution_subst: ctx.decl_resolution_subst.clone(),
        // Carried for the same reason: a chip declared inside an enclosing
        // generic body keeps that body's frame when instanced from this chip.
        decl_mono_frame: ctx.decl_mono_frame.clone(),
        // And so a mod declared inside an enclosing body keeps the frame it
        // was declared in when called from this chip.
        decl_views: ctx.decl_views.clone(),
        views_by_frame: ctx.views_by_frame.clone(),
        decl_later_storage: ctx.decl_later_storage.clone(),
        // A mod the body calls reads the top level as its declaration sees it.
        top_names: Arc::new(ctx.top_names.dups_only()),
        // The body inherits the caller's frames, and with them the reads the
        // caller has cached.
        var_gets: ctx.var_gets.clone(),
        top_dup_bindings: ctx.top_dup_bindings.clone(),
        foreign_literals,
        // A chip instantiated inside a template build is part of that build.
        template_build: ctx.template_build,
        // Generic chips ARE monomorphized: one template per `(name, subst)`
        // (keyed in `lower_chip_call_instance`), so the shared-template concern
        // is moot — each distinct instantiation builds its own body here. Seed
        // the stack with this call's frame so every `T` in the body resolves to
        // the concrete monomorph (`resolve_local_type` / `mono_op_rule` /
        // `lower_if_expr` all read `mono_stack`). A non-generic chip
        // declared outside every generic body leaves the stack empty, so
        // resolution is unaffected.
        mono_stack: body_frame.map(|f| vec![f]).unwrap_or_default(),
        // The constants of the scope the declaration sees, each frame kept
        // beside the scope frame it belongs to (the body inherits those
        // frames, below), with the `const` parameters as module constants:
        // a parameter name resolves through `const_lookup()` inside the chip
        // exactly as it does inside an inlined mod body, which is what lets it
        // reach a literal-requiring position (gate config, event channel
        // names, array/map baking). The value is part of the template cache
        // key (`lower_chip_call_instance`), so two calls with different
        // constants can never share this body. A `const` parameter is
        // const-DECLARED by construction (the ONLY way a chip input lands in
        // `const_params` at all is `inp.is_const`), so an `if`-condition
        // inside this body naming it is eligible for the widened elision
        // exactly like a real `const` binding.
        //
        // The body's own frame is pushed on top with the scope's (below), so
        // a body-level `const` lands there and shadows a same-named outer
        // constant, as typecheck, which checks a chip body inside a pushed
        // scope, has it: a chip-body `const ch = "…"` must reach the chip's
        // OWN handlers (`SendCustomEvent(ch, 1)` baked an EMPTY channel name
        // when it did not), and a chip-body `const a = 222` shadowing a
        // top-level `const a = 111` must decide an `if` on the inner value in
        // both stages.
        consts: ctx.consts.for_chip_body(const_params, inherited_consts),
        // Snapshot the `pass1_chips` FRAME STACK the declaration sees, so a
        // body-local var/array/map initializer inside THIS chip resolves a
        // `const mod` call to the same declaration the enclosing code would.
        // Capturing the stack (rather than a flattened map) is what makes a
        // chip declared inside an inlined `mod` see that mod's frame — the
        // shadowing declaration is still on the stack here, because the frame
        // is dropped by the enclosing `pop_scope` only after this body has
        // been built.
        //
        // Cloning also isolates it: this chip's own nested declarations
        // (recorded by the pre-declare loop below) can never reach the
        // caller's stack.
        pass1_chips: inherited_chips,
        // The body inherits the caller's bindings (below), so a constant
        // container it reaches through one stays immutable here.
        immutable_containers: ctx.immutable_containers.clone(),
        importer_names: ctx.importer_names.clone(),
        ns_mod_scopes: ctx.ns_mod_scopes.clone(),
        ns_mod_consts: ctx.ns_mod_consts.clone(),
        // Everything not named above stays at `empty`'s zero, and two of those
        // are load-bearing rather than incidental: `import_state_dedup` and
        // `import_behavior_lowered` must stay empty because a chip body is its
        // own module and performs no import merge - its per-instance state must
        // NEVER dedup across instances by source location, which is why the
        // dedup lives only at the entry module's import sites and not inside
        // `pre_declare_var`.
        ..LowerCtx::empty(
            child_builder,
            ctx.type_of_expr,
            ctx.op_resolutions,
            ctx.resolutions,
            ctx.ce_slots,
            ctx.doc_comments,
        )
    };

    // A chip is visual grouping only — wire refs cross the boundary freely — so
    // its body closes over the ENTIRE lexical scope of its declaration: module
    // globals, plus the handler-local `let`s, event params and block locals of
    // a chip declared inside a body. The frames are kept as frames, so a mod
    // the body calls still resolves in its own declaration's frame. The chip's
    // own frame is pushed on top: params declared below shadow these, and are
    // invisible to such a mod.
    //
    // Constants get one extra step: a `let X = <const>` is a `Local` pointing at
    // a `_Literal` node in the parent module. Cloning that literal into the
    // chip's own module lets `inline_orphan_literals` fold it into its consumers
    // as inline gate data (fewer gates) rather than a separate constant brick.
    // Only the nearest binding per name is cloned: a shadowed one is read, if
    // at all, by an outer mod, across the boundary.
    let mut frames = inherited_frames;
    let mut seen = crate::collections::HashSet::default();
    for (_, _, entries) in frames.iter_mut().rev() {
        // A chip body can't target the enclosing module's `out`s, and inheriting
        // them inflates `output_count()`. That makes a single-`return` chip skip
        // its own value-output wiring (`Stmt::Return` only wires when
        // `output_count() == 1`) whenever the parent module declares any `out`.
        entries.retain(|(_, b)| !matches!(b, Binding::Output(_)));
        for (name, binding) in entries.iter_mut() {
            if !seen.insert(*name) {
                continue;
            }
            if let Some(copy) = literal_copy(ctx, &mut child_ctx, binding) {
                *binding = copy;
            }
        }
    }
    child_ctx.scope = crate::scope::Scope::from_frames(frames);
    child_ctx.consts.set_cursor(inherited_cursor);
    child_ctx.push_scope(crate::scope::ScopeTag::MODULE);
    child_ctx.consts.hide_held_at_top(inherited_hidden);

    for inp in &chip_decl.inputs {
        // A `const` parameter is a compile-time value seeded into the
        // constants above, NOT a wire, so it gets no `MicrochipInput` pin and consumes
        // no pin slot. `wire_chip_args_and_outputs` skips it on the identical
        // `is_const` predicate (deliberately NOT on "did it evaluate", so the
        // two loops can never disagree): a mismatch here would shift every
        // later param's pin index and silently mis-wire the whole call.
        if inp.is_const {
            if let Some(lit) = const_params.get(&inp.name) {
                let marks = crate::const_scope::Marks { declared: true, ..Default::default() };
                child_ctx.consts.record(&inp.name, lit.clone(), marks);
                let ty = child_ctx
                    .declared_type(chip_decl, &inp.typ, child_ctx.mono_stack.last())
                    .unwrap_or_else(|| child_ctx.resolve_local_type(&inp.typ));
                crate::lower::predeclare::bind_shadowing_const_param(
                    &mut child_ctx,
                    &inp.name,
                    lit.clone(),
                    ty,
                    &chip_decl.range,
                    &chip_decl.body,
                );
            }
            continue;
        }
        let resolved_record = child_ctx.record_or_tuple_fields(&inp.typ);
        if let Some(fields) = &resolved_record {
            let record_fields = explode_record_param_pins(
                &mut child_ctx,
                &inp.name,
                fields,
                caller_captures,
                &chip_decl.range,
            );
            child_ctx
                .scope
                .insert(&inp.name, Binding::Record(record_fields));
        } else if child_ctx.ref_storage(Some(chip_decl), &inp.typ).is_some() {
            if let Some(captured) = caller_captures.get(&inp.name) {
                if immutable_params.contains(&inp.name) {
                    child_ctx.immutable_containers.insert(captured.node_id);
                }
                child_ctx.scope.insert(
                    &inp.name,
                    Binding::Var(VarRecord::new(captured.node_id, captured.inner_type.clone(), captured.storage)),
                );
            } else {
                let t = child_ctx.resolve_local_type(&inp.typ);
                let (storage, inner) =
                    super::context::container_binding(child_ctx.ref_storage(Some(chip_decl), &inp.typ), &t)
                        .expect("gated by ref_storage");
                let node_id = child_ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
                if immutable_params.contains(&inp.name) {
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
        } else {
            let t = if super::inline::is_exec_type(&child_ctx, &inp.typ) {
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
    // Apply destructuring patterns declared on chip params: the record is bound
    // above under the synthetic param name, so expand it into the named fields
    // the body reads. A `chip f({x, y}: P)` otherwise leaves `x`/`y` unbound.
    for inp in &chip_decl.inputs {
        let Some(pattern) = &inp.pattern else {
            continue;
        };
        let base = child_ctx.scope.get(&inp.name).cloned();
        match pattern {
            crate::ast::ParamPattern::Record { fields, .. } => {
                if let Some(Binding::Record(src)) = base {
                    crate::lower::decl::install_record_destruct(&mut child_ctx, &src, fields);
                }
            }
            crate::ast::ParamPattern::Tuple { names, rest } => {
                if let Some(Binding::Record(src)) = base {
                    let order: Vec<String> = (0..names.len().max(src.len()))
                        .map(|i| i.to_string())
                        .collect();
                    crate::lower::decl::install_tuple_destruct(
                        &mut child_ctx,
                        &src,
                        names,
                        rest.as_ref(),
                        &order,
                    );
                }
            }
        }
    }
    // A chip declared in an `import * as` module reads that module's
    // constants, each one a parameter does not rebind, as a namespaced `mod`
    // body does (see `inline.rs`).
    let ns_key = (chip_decl.range.file.clone(), chip_decl.range.start.offset);
    if let (Some(ns_scope), Some(ns_consts)) =
        (child_ctx.ns_mod_scopes.get(&ns_key).cloned(), child_ctx.ns_mod_consts.get(&ns_key).cloned())
    {
        for (name, lit, marks) in ns_consts.iter() {
            if child_ctx.scope.get_current(name).is_some() {
                continue;
            }
            if let Some(member) = ns_scope.get(name) {
                let member = literal_copy(ctx, &mut child_ctx, member).unwrap_or_else(|| member.clone());
                child_ctx.scope.insert(name, member);
            }
            child_ctx.consts.record(name, lit.clone(), *marks);
            crate::lower::predeclare::own_constant_container(&mut child_ctx, name, lit, &chip_decl.range);
        }
    }
    for out in &chip_decl.outputs {
        // A record-typed signature output needs one pin per leaf, bound as the
        // same `Binding::Record` shape `pre_declare_output` gives a top-level
        // record `out`. Given a single pin the body's `out p = { … }` has
        // nowhere to land and emits nothing at all. A tuple output is one pin
        // per element the same way.
        if child_ctx.record_or_tuple_fields(&out.typ).is_some() {
            let fields = crate::lower::predeclare::record_output_pins(
                &mut child_ctx,
                &out.typ,
                &out.name,
                &chip_decl.range,
                0,
            );
            child_ctx.scope.insert(
                &crate::lower::context::output_scope_key(&out.name),
                Binding::Record(fields),
            );
            continue;
        }
        let t = if super::inline::is_exec_type(&child_ctx, &out.typ) {
            Type::Exec
        } else {
            child_ctx.resolve_local_type(&out.typ)
        };
        let node_id = child_ctx.add_output(&out.name, t.clone(), chip_decl.range.clone());
        child_ctx.scope.insert(
            &crate::lower::context::output_scope_key(&out.name),
            Binding::Output(NodeRecord { node_id, ty: t }),
        );
    }

    // Auto-exec: if the caller has exec context (or supplies an `exec =`
    // named arg from a pure context) and the chip doesn't explicitly take
    // exec as its first param, create exec entry/exit boundary ports so the
    // chip body receives the exec chain.
    let first_param_is_exec = chip_decl
        .inputs
        .first()
        .map(|p| super::inline::is_exec_type(ctx, &p.typ))
        .unwrap_or(false);
    let auto_exec = (ctx.current_exec.is_some() || force_exec_boundary) && !first_param_is_exec;
    if auto_exec {
        let exec_in = child_ctx.add_input("_exec_in", Type::Exec, chip_decl.range.clone());
        child_ctx.current_exec = Some(exec_in.port(WirePort::RerOutput));
    }

    // Returns on the body's own path, and returns inside nested handlers,
    // which run on the handler's trigger (see `count_handler_return_values`).
    let returns = count_return_values(&chip_decl.body);
    let handler_returns = count_handler_return_values(&chip_decl.body);

    let sig_output_names: HashSet<&str> =
        chip_decl.outputs.iter().map(|o| o.name.as_str()).collect();
    crate::lower::predeclare::record_later_storage(&mut child_ctx, &chip_decl.body);
    let stmts = &crate::lower::predeclare::Mentions::new(&chip_decl.body.stmts);
    for (at, stmt) in chip_decl.body.stmts.iter().enumerate() {
        match stmt {
            // This chip's OWN nested chip/mod declarations overlay the
            // `pass1_chips` inherited from the caller above, so a chip-local
            // `const mod` shadowing a same-named top-level one wins in the
            // bake path exactly as it does in every other path. Must stay in
            // this source-ordered loop — see `pre_declare_chip_name`.
            Stmt::ChipDecl(c) => pre_declare_chip_name(&mut child_ctx, c),
            // An anonymous `chip { }` written straight into this chip's body.
            // `lower_anon_chip` finds the shell by (range, enclosing chip) and
            // returns silently when there is none, so without this the whole
            // nested body (vars, handlers, even `out` bindings for this chip's
            // own declared outputs) was discarded with no diagnostic.
            // The top-level walk (`predeclare::pre_declare_decl`) and the
            // handler-body walk (`handler::lower_block`) both do this already;
            // this walk was the one that did not.
            Stmt::AnonChip(ac) => pre_declare_anon_chip(&mut child_ctx, ac, false),
            Stmt::In(i) => pre_declare_input(&mut child_ctx, i),
            // A name the body already binds (a parameter) is declared by its
            // statement instead (see `hoist_or_defer`).
            Stmt::Var(v) if hoist_or_defer(&mut child_ctx, stmts, at, &v.name, &v.range) => {
                child_ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v))
            }
            Stmt::Buffer(b) if hoist_or_defer(&mut child_ctx, stmts, at, &b.name, &b.range) => {
                pre_declare_buffer(&mut child_ctx, b)
            }
            Stmt::Array(a) if hoist_or_defer(&mut child_ctx, stmts, at, &a.name, &a.range) => {
                pre_declare_array(&mut child_ctx, a)
            }
            Stmt::Map(m) if hoist_or_defer(&mut child_ctx, stmts, at, &m.name, &m.range) => {
                pre_declare_map(&mut child_ctx, m)
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
                    )
                });
            }
            _ => {}
        }
    }
    // An output this body emits from two sites, from a branch, beside its own
    // `out` default, or beside a `return` is backed by a var, as a top-level
    // one is (see `create_output_backing_var`): each site stores, and the var
    // drives the output once. Two sites wired straight into the output are a
    // load-time fan-in. A record output is backed field by field.
    let value_outputs: Vec<&str> = chip_decl
        .outputs
        .iter()
        .filter(|o| !super::inline::is_exec_type(&child_ctx, &o.typ))
        .map(|o| o.name.as_str())
        .collect();
    let record_only_output =
        chip_decl.outputs.len() == 1 && child_ctx.record_or_tuple_fields(&chip_decl.outputs[0].typ).is_some();
    if chip_decl.outputs.len() > 1 || record_only_output {
        child_ctx.chip_return_outputs = chip_decl
            .outputs
            .iter()
            .map(|o| (o.name.clone(), super::inline::is_exec_type(&child_ctx, &o.typ)))
            .collect();
    }
    {
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
                    let backing = crate::lower::predeclare::build_record_fields(
                        &mut child_ctx,
                        VarStorage::Var,
                        &fields,
                        &name,
                        None,
                        None,
                        &chip_decl.range,
                    );
                    child_ctx.output_record_backing.insert(name, backing);
                }
                None => crate::lower::create_output_backing_var(&mut child_ctx, &name),
            }
        }
    }
    // A `return` beside an `emit` of the one output, or in a nested handler,
    // stores into its backing var, like the emit.
    if ((auto_exec && returns >= 1) || handler_returns >= 1)
        && chip_decl.outputs.len() == 1
        && let Some(backing) = child_ctx.output_backing_vars.get(&chip_decl.outputs[0].name).cloned()
    {
        child_ctx.mod_return_var = Some(backing);
    }
    // Multi-return chips hold the returned value in a PseudoVar: each `return
    // expr` does a Var_Set into it, and a single Var_Get after the return union
    // (below) feeds the value output. The inline-mod path does the same
    // (`lower_chip_call_inline`); without it here two `return`s each wired
    // straight into the output pin — a load-time fan-in with no diagnostic
    // (P0-3). Only meaningful with an exec context (returns fire on exec chains)
    // and exactly one declared output.
    if auto_exec
        && returns > 1
        && chip_decl.outputs.len() == 1
        && !record_only_output
        && child_ctx.mod_return_var.is_none()
    {
        let out_type = child_ctx.resolve_local_type(&chip_decl.outputs[0].typ);
        child_ctx.mod_return_var = Some(return_value_var(&mut child_ctx, out_type, &chip_decl.body.range));
    }

    for stmt in &chip_decl.body.stmts {
        lower_stmt(&mut child_ctx, stmt);
    }

    if auto_exec {
        // A trailing `return` moved the body's tail exec into mod_return_exec
        // (leaving current_exec = None); merge it back with any fallthrough so an
        // exec-bearing body that ends in `return` still drives `_exec_out`. The
        // inline-mod path does this same merge; without it here the body's exec
        // chain (e.g. from an array find) is orphaned and no exec output is made.
        if let Some(ret) = child_ctx.mod_return_exec.take() {
            let fall = child_ctx.current_exec.take();
            child_ctx.current_exec = merge_exec(&mut child_ctx, fall, Some(ret), &chip_decl.range);
        }
        // Multi-return: Var_Get the accumulated return value on the merged exec
        // and wire it to the single value output — one wire, no fan-in. Mirrors
        // the inline path's post-union Var_Get. Runs before `_exec_out` so the
        // Var_Get's exec becomes the chip's exec tail.
        // An output backing var drives its output itself (below).
        if let Some(ret_var) = child_ctx.mod_return_var.clone()
            && !child_ctx.output_backing_vars.values().any(|v| v.node_id == ret_var.node_id)
            && let Some(exec) = child_ctx.current_exec
        {
            // A backed output is driven by its backing var, wired below.
            let drive_output = child_ctx.output_backing_vars.is_empty();
            read_return_value(&mut child_ctx, &ret_var, exec, drive_output);
        }
        if let Some(tail_exec) = child_ctx.current_exec {
            let exec_out = child_ctx.add_output("_exec_out", Type::Exec, chip_decl.range.clone());
            child_ctx.connect(tail_exec, exec_out.port(WirePort::RerInput));
        }
    }

    // Flush the chip body's own pending emits — a chip that emits a declared
    // exec output (`-> (next: exec)` + `emit next`) or a body-local exec signal
    // queues into `child_ctx.pending_emits`, which is otherwise discarded with
    // the child ctx, silently dropping the emit and leaving the output dead (an
    // awaiting caller never resumes). The root module flushes in `lower`; each
    // chip body must flush its own, since its emits target its own outputs/hubs.
    // Each record backing drives its output's field pins once.
    let mut records: Vec<(String, HashMap<crate::intern::Sym, Binding>)> =
        child_ctx.output_record_backing.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    records.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, backing) in records {
        if let Some(Binding::Record(pins)) = child_ctx
            .scope
            .get(&crate::lower::context::output_scope_key(&name))
            .cloned()
        {
            wire_record_backing(&mut child_ctx, &backing, &pins);
        }
    }
    // Each backing var drives its output once, after every site's write.
    let mut backings: Vec<(String, VarRecord)> =
        child_ctx.output_backing_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    backings.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, backing) in backings {
        if let Some(out) = child_ctx.lookup_output(&name).cloned() {
            child_ctx.connect(backing.node_id.port(WirePort::Value), out.node_id.port(WirePort::RerInput));
        }
    }
    super::stmt::flush_pending_emits(&mut child_ctx);

    ctx.diagnostics.extend(child_ctx.diagnostics);
    ctx.dropped_ranges.extend(child_ctx.dropped_ranges);
    let mut module = child_ctx.builder.module;
    module.scope_captures = compute_scope_captures(&module);
    module
}

/// Explode a record-typed chip param into its LEAF input pins, returning the
/// `Binding::Record` the body reads fields through. A container field (array /
/// map / ref) is one ref pin, a NESTED record field recurses into its own leaf
/// pins (a nested `Binding::Record`), and a scalar field is one by-value pin.
/// The caller-side `wire_record_param_pins` mirrors this recursion exactly, so
/// the child pins line up index-for-index with the argument wires.
fn explode_record_param_pins(
    child_ctx: &mut LowerCtx,
    prefix: &str,
    fields: &[crate::ast::RecordTypeField],
    caller_captures: &HashMap<String, VarRecord>,
    chip_range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut record_fields = HashMap::default();
    for field in fields {
        let port_name = format!("{prefix}_{}", field.name);
        let ft = child_ctx.resolve_local_type(&field.typ);
        // Array / Map / ref fields bind a container ref-port; classified via
        // `container_binding` so a `Map<K,V>` field wires its `MapVarRef`.
        let container = super::context::container_binding(child_ctx.ref_storage(None, &field.typ), &ft);
        if let Some(captured) = container
            .is_some()
            .then(|| caller_captures.get(&port_name))
            .flatten()
        {
            record_fields.insert(
                crate::intern::intern(&field.name),
                Binding::Var(VarRecord::new(captured.node_id, captured.inner_type.clone(), captured.storage)),
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
            record_fields.insert(crate::intern::intern(&field.name), binding);
            continue;
        }
        // A nested record/tuple recurses into per-leaf pins; a scalar is one pin.
        if let Some(sub) = child_ctx.record_or_tuple_fields(&field.typ) {
            let nested =
                explode_record_param_pins(child_ctx, &port_name, &sub, caller_captures, chip_range);
            record_fields.insert(crate::intern::intern(&field.name), Binding::Record(nested));
        } else {
            let node_id = child_ctx.add_input(&port_name, ft.clone(), chip_range.clone());
            record_fields.insert(
                crate::intern::intern(&field.name),
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
    ctx: &mut LowerCtx,
    backing: &HashMap<crate::intern::Sym, Binding>,
    pins: &HashMap<crate::intern::Sym, Binding>,
) {
    let mut names: Vec<crate::intern::Sym> = pins.keys().copied().collect();
    names.sort_by_key(|k| crate::intern::resolve(*k));
    for k in names {
        match (backing.get(&k), &pins[&k]) {
            (Some(Binding::Record(b)), Binding::Record(p)) => wire_record_backing(ctx, &b.clone(), &p.clone()),
            (Some(Binding::Var(v)), Binding::Output(out)) => {
                ctx.connect(v.node_id.port(WirePort::Value), out.node_id.port(WirePort::RerInput));
            }
            _ => {}
        }
    }
}

/// Record every `_Literal` of this module that `b` names a port of, into
/// `out` (see `LowerCtx::foreign_literals`).
fn collect_literals(ctx: &LowerCtx, b: &Binding, out: &mut HashMap<NodeId, std::sync::Arc<crate::ir::Node>>) {
    match b {
        Binding::Local(l) => {
            if let Some(n) = ctx.builder.module.nodes.get(&l.port.node_id)
                && n.gate_class == gc::LITERAL
            {
                out.entry(l.port.node_id).or_insert_with(|| std::sync::Arc::new(n.clone()));
            }
        }
        Binding::Record(fields) => fields.values().for_each(|f| collect_literals(ctx, f, out)),
        _ => {}
    }
}

/// `binding` read from the chip's own module: a `let` of a constant in the
/// enclosing module points at a `_Literal` there, so the chip gets a copy of
/// that literal (`None` for any other binding).
fn literal_copy(ctx: &LowerCtx, child_ctx: &mut LowerCtx, binding: &Binding) -> Option<Binding> {
    let Binding::Local(local) = binding else { return None };
    let src = ctx.builder.module.nodes.get(&local.port.node_id).filter(|src| src.gate_class == gc::LITERAL)?;
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
