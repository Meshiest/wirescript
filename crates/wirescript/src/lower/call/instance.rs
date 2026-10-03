//! The microchip instance call site. The only module that touches the
//! template cache.

use super::*;

pub(in crate::lower) fn lower_chip_call_instance(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    type_args: &[TypeExpr],
    range: &SourceRange,
) -> PortRef {
    static INSTANCE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let idx = INSTANCE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let instance_name = format!("{}_{}", chip_decl.name, idx);

    let caller_captures = resolve_caller_captures(ctx, chip_decl, args);

    // `exec = trigger` named arg: how exec chips get their chain when invoked
    // outside an exec context (mirrors the builtin exec-call convention).
    let exec_arg = args.iter().find_map(|a| match a {
        CallArg::Named { name, value, .. } if name == "exec" => Some(value),
        _ => None,
    });

    // Monomorphize a generic chip per distinct type instantiation. Rebuild this
    // call's type substitution (same inference the inline path + typecheck use)
    // and key the template + emitted grid on `(name, concrete subst)` so
    // `Box<int>` and `Box<vector>` get separate bodies AND separate grids (they
    // dedup on `template_key`, which would otherwise collapse them into one).
    // A non-generic chip keeps the bare name.
    let positional_args = positional_args(args);
    let first_param_is_exec = chip_decl
        .inputs
        .first()
        .map(|p| super::inline::is_exec_type(ctx, &p.typ))
        .unwrap_or(false);
    // Auto-exec boundary pins are created when the call has (or is handed) an
    // exec context and the chip doesn't take exec as its first param. Compute it
    // here — BEFORE the cache lookup — because it decides the compiled body's pin
    // shape and so must be part of the cache key (below). `build_chip_module`
    // recomputes the identical predicate internally.
    let auto_exec = (ctx.current_exec.is_some() || exec_arg.is_some()) && !first_param_is_exec;

    // A `const` parameter is a compile-time value, not a wire. Evaluate each
    // one's argument HERE — before the cache key is built — because the value
    // is baked INTO the compiled body: `build_chip_module` seeds it into the
    // body's constant environment and gives the param no `MicrochipInput`
    // pin, so it decides the body's content and pin shape exactly as the
    // monomorph and the capture list do. Typecheck already reported any
    // argument that fails to evaluate (WS046), so a failure is skipped here
    // rather than double-reported.
    let mut const_params: ConstEnv = ConstEnv::default();
    let mut const_key_parts: Vec<(usize, Literal)> = Vec::new();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        if !param.is_const {
            continue;
        }
        let Some(arg) = positional_args.get(i) else {
            continue;
        };
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        let cx = ctx.const_ctx(Some(&lookup));
        if let Ok(lit) = crate::const_eval::eval_expr(arg, &cx, &mut budget) {
            let lit = match ctx.declared_type(chip_decl, &param.typ, None) {
                Some(ty) => crate::const_eval::coerce_const_to_type(lit, &ty, ctx.consts.types()),
                None => lit,
            };
            const_params.insert(param.name.clone(), lit.clone());
            const_key_parts.push((i, lit));
        }
    }

    let (mono_frame, base_key) = if chip_decl.type_params.is_empty() {
        (None, chip_decl.name.clone())
    } else {
        let Some(frame) = super::mono::mono_frame(ctx, chip_decl, range, &positional_args, type_args) else {
            return super::mono::report_unchecked_instantiation(ctx, chip_decl, range);
        };
        let key = mono_key(chip_decl, &frame);
        (Some(frame), key)
    };
    // The compiled body's shape is set by call-site context that the bare name
    // omits: the DECLARATION identity (two same-named chips in different
    // namespaces collapse to one name), whether auto-exec pins exist, and which
    // params were CAPTURED (no `MicrochipInput` pin) vs. pinned, and the VALUE
    // of every `const` param (baked into the body, so two calls passing
    // different constants compile to genuinely different bodies). Two calls may
    // share the cached body only when ALL of these match — otherwise a second
    // call in a different context reuses the first's body and mis-wires against a
    // stale pin list, or silently runs with the first call's baked constants.
    // The `chip_call_stack`
    // recursion guard already keys on `chip_decl.range` for the same reason. This
    // key doubles as the instance `template_key` (grid dedup) and `fold_key`
    // base; making it more specific only ever makes those safer.
    let mut cap_names: Vec<&str> = caller_captures.keys().map(String::as_str).collect();
    cap_names.sort_unstable();
    let dr = &chip_decl.range;
    let mut key = format!(
        "{base_key}\u{1}@{}:{}:{}\u{1}e{}\u{1}c[{}]",
        dr.file,
        dr.start.offset,
        dr.end.offset,
        auto_exec as u8,
        cap_names.join(",")
    );
    // `k`-prefixed so a const param's entry can never be confused with a
    // `fold_key` entry (which appends the same `{index}:{value:?}` shape
    // un-prefixed onto this same string).
    for (index, value) in &const_key_parts {
        key.push_str(&format!("\u{1}k{index}:{value:?}"));
    }
    // The body's resolution key: the enclosing generic instantiation it is
    // declared in, then its own substitution. A chip declared inside a generic
    // mod lowers its body under the outer `T`, so each outer instantiation is a
    // different body even though the chip itself is not generic. Empty for a
    // top-level non-generic chip, which then keys exactly as before.
    let body_subst = ctx.body_resolution_subst(chip_decl, mono_frame.as_ref());
    if !super::mono::instantiation_checked(ctx, chip_decl, &body_subst) {
        return super::mono::report_unchecked_instantiation(ctx, chip_decl, range);
    }
    if !body_subst.is_empty() {
        key.push_str(&format!("\u{1}r{body_subst:?}"));
    }
    // A chip declared inside a body captures the names that body bound where
    // it declared the chip, which are another body's each time the enclosing
    // mod expands.
    if ctx.decl_views.contains_key(&(dr.file.clone(), dr.start.offset)) {
        let captured = ctx.with_decl_scope(chip_decl, |ctx| body_captures(ctx, chip_decl));
        if !captured.is_empty() {
            key.push_str(&format!("\u{1}v{captured}"));
        }
    }
    // Container params whose argument is an immutable container: the body
    // rejects a mutation of them, so a body built for a mutable argument
    // cannot be shared with this call.
    let immutable_params = immutable_container_params(ctx, chip_decl, &caller_captures);
    if !immutable_params.is_empty() {
        key.push_str(&format!("\u{1}i[{}]", immutable_params.join(",")));
    }
    // The constants of the scope the body inherits are baked into it. The
    // body inherits its declaration's scope, not this call site's.
    let inherited = ctx.with_decl_scope(chip_decl, |ctx| ctx.consts.snapshot()).describe();
    if !inherited.is_empty() {
        key.push_str(&format!("\u{1}s{inherited}"));
    }

    let mut child_module = if let Some(template) = ctx.template_cache.get(&key) {
        // Build remap: for each param name in the template's capture_names,
        // look up the caller's VarRecord and map old_id -> new_id.
        // Both keys are required: `stamp_module` resolves this module's own
        // externals by param name (`external_refs`, rebuilt that way below) and
        // a nested chip body's by `NodeId::to_string()` (`scope_captures`). A
        // capture missing from either domain is left unremapped, so that body
        // still points at the first call site's nodes.
        let mut captures = std::collections::HashMap::default();
        for (name, old_id) in &template.external_refs {
            if let Some(var_rec) = caller_captures.get(name) {
                captures.insert(name.clone(), var_rec.node_id);
                captures.insert(old_id.to_string(), var_rec.node_id);
            }
        }
        template.instantiate(&instance_name, &captures)
    } else {
        let module = build_chip_module(
            ctx,
            chip_decl,
            &instance_name,
            &caller_captures,
            exec_arg.is_some(),
            mono_frame,
            &const_params,
            &immutable_params,
        );
        // Cache the first instance as a template for subsequent calls.
        // Store capture_names so future instantiations can remap by param name.
        let mut template = crate::template::CompiledTemplate::from_module(module.clone());
        // Rebuild external_refs keyed by param name instead of node_id string
        template.external_refs = caller_captures
            .iter()
            .map(|(name, var_rec)| (name.clone(), var_rec.node_id))
            .collect();
        ctx.template_cache.insert(&key, template);
        module
    };
    child_module.template_key = Some(intern(&key));

    if !auto_exec && let Some(cause) = unsequenced_chip_returns(chip_decl) {
        report_unsequenced_chip_returns(ctx, chip_decl, cause, first_param_is_exec, range);
    }

    // All wiring goes directly to child MicrochipInput/Output nodes.
    // The chip node exists only for layout grouping + microchip link.
    // (`first_param_is_exec` / `auto_exec` were computed above for the cache key.)
    let chip_node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::MICROCHIP,
        source_range: range.clone(),
        ..Default::default()
    });
    let consts = ctx.const_lookup();
    if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
        node.kind = NodeKind::Chip;
        let props = std::sync::Arc::make_mut(&mut node.properties);
        if chip_decl.closed {
            props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        }
        if let Some(label) = resolve_label_text(chip_decl.label.as_deref(), chip_decl.label_expr.as_ref(), &consts) {
            props.insert(*sym::NAME_LABEL, Literal::String(label));
        }
        if let Some(doc) = ctx.doc_comments.get(&crate::parser::doc_key(&chip_decl.range)) {
            props.insert(*sym::DOC_TEXT, Literal::String(doc.clone()));
        }
    }

    let child_inputs = child_module.inputs.clone();
    let child_outputs = child_module.outputs.clone();
    ctx.builder.module.chips.insert(chip_node_id, child_module);

    // Wire args FIRST — this may create Var_Gets in the exec chain
    let mut const_folds: Vec<ConstFold> = Vec::new();
    let result = wire_chip_args_and_outputs(
        ctx,
        chip_decl,
        args,
        &caller_captures,
        &child_inputs,
        &child_outputs,
        &mut const_folds,
    );
    check_constant_pins(ctx, chip_node_id, &child_inputs, range);

    // Wire auto-exec AFTER args so the exec chain is:
    //   ... -> Var_Get(a) -> Var_Get(b) -> chip._exec_in -> chip._exec_out -> ...
    // Not: ... -> chip._exec_in -> chip._exec_out -> Var_Get(a) -> chip.param (cycle!)
    if auto_exec {
        if let Some(exec_expr) = exec_arg {
            // Explicit trigger from a pure context: wire it to the boundary
            // and leave the caller's (non-)context untouched.
            let src = lower_expr(ctx, exec_expr);
            let exec_in_node = *child_inputs.last().unwrap();
            ctx.connect(src, exec_in_node.port(WirePort::RerInput));
        } else if let Some(caller_exec) = ctx.current_exec {
            // Wire exec directly to child's _exec_in/_exec_out MicrochipInput/Output
            let exec_in_node = *child_inputs.last().unwrap();
            let exec_out_node = *child_outputs.last().unwrap();
            ctx.connect(caller_exec, exec_in_node.port(WirePort::RerInput));
            ctx.current_exec = Some(exec_out_node.port(WirePort::RerOutput));
        }
    }

    // Constants live inside the instance, so instances that folded different
    // values are no longer interchangeable. Fold the values into the template
    // key as well, or grid dedup would hand one instance another's body; calls
    // passing the same constants still share a key. The base is the monomorph
    // `key` (not the bare name), so a generic chip folding the same constant at
    // two DIFFERENT types (`Box<int>(5)` vs `Box<vector>(...)`) still keys apart
    // — and `key` already carries every `const` param's value, so those key
    // apart here too.
    if !const_folds.is_empty() {
        let mut fold_key = key.clone();
        for fold in &const_folds {
            fold_key.push_str(&format!("\u{1}{}:{:?}", fold.index, fold.value));
        }
        if let Some(child) = ctx.builder.module.chips.get_mut(&chip_node_id) {
            child.template_key = Some(intern(&fold_key));
        }
        for fold in &const_folds {
            fold_const_chip_input(ctx, chip_node_id, fold);
        }
    }

    result
}

/// Whether the chip body's own `return`s store into output storage, which
/// needs the body's exec chain: several returns, or one beside a return in a
/// nested handler or a value `emit`/`out` of an output. With no chain in the
/// body none of those stores run. Gives the cause, worded for WS007.
fn unsequenced_chip_returns(chip_decl: &ChipDecl) -> Option<&'static str> {
    let returns = count_return_values(&chip_decl.body);
    if returns > 1 {
        return Some("returns from inside a branch");
    }
    if returns == 0 {
        return None;
    }
    if count_handler_return_values(&chip_decl.body) > 0 {
        return Some("returns from its body and from a nested `on` handler");
    }
    let mut counts = HashMap::default();
    crate::lower::count_emits_in_block(&chip_decl.body, false, &mut counts);
    counts
        .iter()
        .any(|(name, (count, _))| {
            let default = chip_decl
                .body
                .stmts
                .iter()
                .any(|s| matches!(s, Stmt::OutBinding(o) if o.value.is_some() && &o.name == name));
            *count > usize::from(default)
        })
        .then_some("returns a value and also writes an output with `emit` or `out`")
}

/// WS007 at a call whose chip returns can't run (`unsequenced_chip_returns`).
/// A chip whose first parameter is `exec` never gets the caller's chain: its
/// body runs only under `on <param>`.
fn report_unsequenced_chip_returns(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    cause: &str,
    first_param_is_exec: bool,
    range: &SourceRange,
) {
    let msg = if first_param_is_exec {
        let trigger = &chip_decl.inputs[0].name;
        format!(
            "`{}` {cause}, but its body has no exec chain to choose between its output \
             writes: its first parameter `{trigger}` is its trigger, so put the returns \
             inside `on {trigger} {{ ... }}`",
            chip_decl.name
        )
    } else {
        format!(
            "`{}` {cause}, so choosing between its output writes needs an exec context: \
             call it from a handler or a `mod` body, or pass `exec = ...`",
            chip_decl.name
        )
    };
    ctx.error("WS007", msg, range);
}

/// The container params CAPTURED as an immutable container (a `const` table,
/// a `let` of one, or a literal): the body is built knowing it, so a mutation
/// there is WS044 at the mutation.
fn immutable_container_params(
    ctx: &LowerCtx,
    chip_decl: &ChipDecl,
    caller_captures: &HashMap<String, VarRecord>,
) -> Vec<String> {
    chip_decl
        .inputs
        .iter()
        .filter(|p| {
            caller_captures
                .get(&p.name)
                .is_some_and(|rec| is_immutable_container(ctx, rec.node_id))
        })
        .map(|p| p.name.clone())
        .collect()
}

/// A pin fed an immutable container, whatever computed the value (an inline
/// mod's result, an `if`, a buffer), must not be changed inside the instance.
/// Decided from the node actually wired into each pin, then followed through
/// the instance's own modules: into nested chip pins, `Select`s and buffers,
/// up to a gate that changes the container or stores its reference.
fn check_constant_pins(ctx: &mut LowerCtx, chip_node: NodeId, pins: &[NodeId], range: &SourceRange) {
    if ctx.immutable_containers.is_empty() {
        return;
    }
    let mut tainted: crate::collections::HashSet<NodeId> = pins
        .iter()
        .copied()
        .filter(|p| {
            ctx.builder.module.wires.iter().any(|w| {
                w.target.node_id == *p
                    && w.target.port == WirePort::RerInput
                    && is_immutable_container(ctx, w.source.node_id)
            })
        })
        .collect();
    if tainted.is_empty() {
        return;
    }
    let Some(child) = ctx.builder.module.chips.get(&chip_node) else {
        return;
    };
    let mut classes: HashMap<NodeId, &'static str> = HashMap::default();
    let mut wires: Vec<crate::ir::Wire> = Vec::new();
    let mut stack = vec![child];
    while let Some(m) = stack.pop() {
        classes.extend(m.nodes.iter().map(|(id, n)| (*id, n.gate_class)));
        wires.extend(m.wires.iter().copied());
        stack.extend(m.chips.values());
    }
    let changes = |class: &str, port: WirePort| {
        crate::catalog::storage::access(class, port) == Some(crate::catalog::storage::Access::Write)
            || (port == WirePort::Value
                && matches!(
                    class,
                    gc::VAR_SET
                        | gc::ARRAY_PUSH
                        | gc::ARRAY_SET_AT_INDEX
                        | gc::ARRAY_INSERT
                        | gc::ARRAY_FILL
                        | gc::ARRAY_RESIZE
                        | gc::MAP_SET
                ))
            || (port == WirePort::RerInput && class == gc::MICROCHIP_OUTPUT)
    };
    loop {
        let mut grew = false;
        for w in &wires {
            if !tainted.contains(&w.source.node_id) {
                continue;
            }
            let class = classes.get(&w.target.node_id).copied().unwrap_or("");
            if changes(class, w.target.port) {
                ctx.error(
                    "WS044",
                    "this call passes a constant array/map (a `const`, or a `let` or literal \
                     holding a constant one) to a chip that would change it or store its \
                     reference, which would make its runtime contents disagree with its \
                     compile-time value. Pass a `var` copy instead",
                    range,
                );
                return;
            }
            let passes = match class {
                gc::MICROCHIP_INPUT => w.target.port == WirePort::RerInput,
                gc::SELECT => matches!(w.target.port, WirePort::InputA | WirePort::InputB),
                gc::BUFFER_TICKS | gc::BUFFER_SECONDS => w.target.port == WirePort::Input,
                _ => false,
            };
            if passes && tainted.insert(w.target.node_id) {
                grew = true;
            }
        }
        if !grew {
            return;
        }
    }
}

/// The names `d`'s body uses that resolve inside an enclosing body rather
/// than at the top level, each with what it denotes there, in name order.
/// Empty when the body uses only its own params and top-level names. Read in
/// `d`'s declaration scope.
fn body_captures(ctx: &LowerCtx, d: &ChipDecl) -> String {
    let mut names = crate::collections::HashSet::default();
    crate::resolve::collect_idents_in_block(&d.body, &mut names);
    collect_emit_targets(&d.body, &mut names);
    for p in &d.inputs {
        names.remove(&p.name);
    }
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort_unstable();
    let mut out = String::new();
    for name in names {
        let sym = intern(&name);
        let Some(frame) = ctx.scope.frame_of(sym, |_| true) else { continue };
        if frame == 0 {
            continue;
        }
        let Some(binding) = ctx.scope.get_sym(sym) else { continue };
        out.push_str(&name);
        out.push('=');
        push_binding_identity(ctx, binding, &mut out);
        out.push(';');
    }
    out
}

/// What `b` denotes, independent of its read cache: its gate or port, a
/// nested declaration's registration, a record's fields in name order.
fn push_binding_identity(ctx: &LowerCtx, b: &Binding, out: &mut String) {
    use std::fmt::Write;
    let _ = match b {
        Binding::Var(v) => write!(out, "v{}", v.node_id),
        Binding::Local(l) => write!(out, "l{:?}", l.port),
        Binding::Buffer(n) | Binding::Input(n) | Binding::Output(n) => write!(out, "n{}", n.node_id),
        Binding::EventParam(p) => write!(out, "e{p:?}"),
        Binding::DynRef(d) => write!(out, "d{:?}", d.port),
        Binding::Chip(c) => match ctx.decl_views.get(&(c.range.file.clone(), c.range.start.offset)) {
            Some(view) => write!(out, "c{}", view.serial),
            None => write!(out, "c{}", c.range.start.offset),
        },
        Binding::Namespace(_) => write!(out, "ns"),
        Binding::Record(fields) => {
            let mut keys: Vec<_> = fields.keys().copied().collect();
            keys.sort_by_key(|k| crate::intern::resolve(*k));
            out.push('{');
            for k in keys {
                out.push_str(crate::intern::resolve(k));
                out.push(':');
                push_binding_identity(ctx, &fields[&k], out);
                out.push(',');
            }
            out.push('}');
            Ok(())
        }
    };
}

/// Every name `block` emits, at any depth, nested declarations included.
fn collect_emit_targets(block: &Block, names: &mut crate::collections::HashSet<String>) {
    walk::walk(walk::Node::Block(block), walk::Ctx::default(), &mut |n, _| {
        if let walk::Node::Stmt(Stmt::Emit(e)) = n {
            names.insert(e.name.clone());
        }
        true
    });
}
