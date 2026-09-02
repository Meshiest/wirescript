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
    let positional_args: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect();
    let first_param_is_exec = chip_decl
        .inputs
        .first()
        .map(|p| matches!(&p.typ, TypeExpr::Name { name, .. } if name == "exec"))
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
            const_params.insert(param.name.clone(), lit.clone());
            const_key_parts.push((i, lit));
        }
    }

    let (mono_frame, base_key) = if chip_decl.type_params.is_empty() {
        (None, chip_decl.name.clone())
    } else {
        let frame = build_mono_frame(ctx, chip_decl, &positional_args, type_args);
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

    // All wiring goes directly to child MicrochipInput/Output nodes.
    // The chip node exists only for layout grouping + microchip link.
    // (`first_param_is_exec` / `auto_exec` were computed above for the cache key.)
    let chip_node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::MICROCHIP,
        source_range: range.clone(),
        ..Default::default()
    });
    if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
        node.kind = NodeKind::Chip;
        let props = std::sync::Arc::make_mut(&mut node.properties);
        if chip_decl.closed {
            props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
        }
        if let Some(label) = resolve_label_text(
            chip_decl.label.as_deref(),
            chip_decl.label_expr.as_ref(),
            &ctx.const_env,
        ) {
            props.insert(*sym::NAME_LABEL, Literal::String(label));
        }
        if let Some(doc) = ctx.doc_comments.get(&chip_decl.range.start.offset) {
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
