use super::*;
use crate::collections::HashSet;

pub(super) fn lower_call(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    let (callee, args, range) = match e {
        Expr::Call {
            callee,
            args,
            range,
        } => (callee, args, range),
        _ => return synthesise_unsupported(ctx, e),
    };
    if let Expr::Ident { name, .. } = callee.as_ref() {
        // User-defined chips/mods shadow builtins of the same name, so a program
        // can define e.g. `chip Toggle` without colliding with the builtin.
        if let Some(chip_decl) = ctx.lookup_chip(name).cloned() {
            return lower_chip_call(ctx, &chip_decl, args, range);
        }
        if let Some(spec) = find_call(name) {
            return lower_builtin_call(ctx, spec, None, args, range, e);
        }
        // An identifier callee that is neither an in-scope chip/mod nor a
        // builtin. If the name IS declared as a chip/mod somewhere in the
        // program, it's a use-before-declaration (chips/mods register in source
        // order): the call would otherwise synthesise an `_Unsupported` gate
        // that silently reads its default (0) at runtime — make it a hard error.
        // Names that are not chips/mods at all (e.g. a builtin not yet lowered)
        // fall through to the usual placeholder path.
        if ctx.known_fn_names.contains(name) {
            ctx.diagnostics.push(Diagnostic::error(
                "WS021",
                format!(
                    "call to undeclared function `{name}` — chips and mods must be \
                     declared before the point where they are used (move the \
                     declaration above its first caller)"
                ),
                range.clone(),
            ));
            return synthesise_unsupported_range(ctx, range);
        }
    }
    // Namespace calls: ns.foo(args)
    if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
        && let Expr::Ident { name: ns_name, .. } = obj.as_ref()
        && let Some(chip_decl) = ctx.lookup_ns_chip(ns_name, field).cloned()
    {
        return lower_chip_call(ctx, &chip_decl, args, range);
    }
    // Method calls: arr.push(val), arr.pop()
    if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
        if let Expr::Ident { name, .. } = obj.as_ref()
            && let Some(var_rec) = ctx.lookup_var(name).cloned()
            && var_rec.storage == VarStorage::Array
        {
            return lower_array_method(
                ctx,
                var_rec.node_id.port(WirePort::ArrayVarRef),
                var_rec.inner_type.clone(),
                field,
                args,
                range,
                e,
            );
        }
        // Array method on an `in X: T[]` input. The array ref lives at the
        // input's RER_Output (not an ArrayVarRef port), but is otherwise usable
        // exactly like a var array — inputs are first-class wherever in scope.
        if let Expr::Ident { name, .. } = obj.as_ref()
            && crate::catalog::arrays::is_array_method(field)
            && let Some(Binding::Input(inp)) = ctx.scope.get(name).cloned()
            && let Type::Array(elem) = inp.ty.clone()
        {
            return lower_array_method(
                ctx,
                inp.node_id.port(WirePort::RerOutput),
                *elem,
                field,
                args,
                range,
                e,
            );
        }
        // Record-resolved array methods: cpu.regs.push(val)
        if let Some(Binding::Var(var_rec)) = resolve_field_chain(ctx, obj).cloned()
            && var_rec.storage == VarStorage::Array
        {
            return lower_array_method(
                ctx,
                var_rec.node_id.port(WirePort::ArrayVarRef),
                var_rec.inner_type.clone(),
                field,
                args,
                range,
                e,
            );
        }
        // Receiver method calls: entity.SetLocation(pos) -> SetLocation(entity, pos)
        if let Some(spec) = find_call(field)
            && spec.receiver.is_some()
        {
            // The receiver fills the spec's first param; passing it separately
            // avoids deep-cloning the receiver + args into a new arg vector.
            return lower_builtin_call(ctx, spec, Some(obj), args, range, e);
        }
    }
    synthesise_unsupported(ctx, e)
}

pub(super) fn lower_chip_call(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    range: &SourceRange,
) -> PortRef {
    let named = !chip_decl.name.is_empty();
    if named && ctx.chip_call_stack.contains(&chip_decl.range) {
        ctx.diagnostics.push(Diagnostic::error(
            "WS020",
            format!(
                "recursive call to `{}` — chips and mods cannot call themselves \
                 (directly or mutually): every call is expanded into the wire \
                 graph at compile time. Re-trigger an exec input or use a \
                 buffer-based loop instead.",
                chip_decl.name
            ),
            range.clone(),
        ));
        return synthesise_unsupported_range(ctx, range);
    }
    if named {
        ctx.chip_call_stack.push(chip_decl.range.clone());
    }

    let result = if chip_decl.inline {
        lower_chip_call_inline(ctx, chip_decl, args, range)
    } else {
        lower_chip_call_instance(ctx, chip_decl, args, range)
    };

    if named {
        ctx.chip_call_stack.pop();
    }
    result
}

pub(super) fn lower_chip_call_inline(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    _range: &SourceRange,
) -> PortRef {
    // This call's output nodes don't exist yet, so any wire touching them
    // lands at an index >= this. The output-source lookups and the
    // output-node removal below only scan this tail instead of the whole
    // module wire list (which made deep inline-call chains quadratic).
    let wire_start = ctx.builder.module.wires.len();
    let positional_args: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect();

    // Collect param bindings first (before mutating ctx) so ref lookups
    // see the caller's vars.
    let mut ref_bindings: Vec<(String, VarRecord)> = Vec::new();
    let mut input_bindings: Vec<(String, NodeRecord)> = Vec::new();
    let mut val_bindings: Vec<(String, PortRef, Type)> = Vec::new();
    let mut record_bindings: Vec<(String, HashMap<crate::intern::Sym, Binding>)> = Vec::new();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };
        // A record literal arg lowers to a Binding::Record (as `let x = {..}`
        // does), so record and destructured params receive their fields instead
        // of a single unsupported value port.
        if let Expr::RecordLit { fields, .. } = arg_expr {
            let record = lower_record_lit(ctx, fields);
            record_bindings.push((param.name.clone(), record));
            continue;
        }
        if let Some(Binding::Record(fields)) = resolve_field_chain(ctx, arg_expr).cloned() {
            record_bindings.push((param.name.clone(), fields));
            continue;
        }
        match &param.typ {
            TypeExpr::Ref { .. } | TypeExpr::Array { .. } => {
                let var_rec = if let Expr::Ident { name, .. } = arg_expr {
                    ctx.lookup_var(name).cloned()
                } else if let Some(Binding::Var(v)) = resolve_field_chain(ctx, arg_expr).cloned() {
                    Some(v)
                } else {
                    None
                };
                if let Some(var_rec) = var_rec {
                    ref_bindings.push((
                        param.name.clone(),
                        VarRecord {
                            node_id: var_rec.node_id,
                            inner_type: var_rec.inner_type,
                            get_node_for_handler: None,
                            storage: var_rec.storage,
                        },
                    ));
                } else if let Expr::Ident { name, .. } = arg_expr
                    && let Some(Binding::Input(inp)) = ctx.scope.get(name)
                {
                    // An `in X: T[]` / ref input passed by reference: forward the
                    // input binding so the mod body resolves the param to the
                    // input's RER_Output ref, exactly like a var array/ref.
                    input_bindings.push((param.name.clone(), inp.clone()));
                }
            }
            _ => {
                let val_port = lower_expr(ctx, arg_expr);
                let t = type_of_type_expr(&param.typ);
                val_bindings.push((param.name.clone(), val_port, t));
            }
        }
    }

    ctx.scope.push(crate::scope::ScopeTag::MODULE);
    for (name, rec) in ref_bindings {
        ctx.scope.insert(&name, Binding::Var(rec));
    }
    for (name, rec) in input_bindings {
        ctx.scope.insert(&name, Binding::Input(rec));
    }
    for (name, port, _ty) in val_bindings {
        ctx.scope.insert(&name, Binding::Local(LocalRecord { port }));
    }
    for (name, fields) in record_bindings {
        ctx.scope.insert(&name, Binding::Record(fields));
    }

    // Apply destructuring patterns: for each param with a pattern, look up
    // the synthetic binding just inserted and expand it into the named fields.
    for param in &chip_decl.inputs {
        let Some(pattern) = &param.pattern else {
            continue;
        };
        let base_binding = ctx.scope.get(&param.name).cloned();
        match pattern {
            crate::ast::ParamPattern::Record { fields, .. } => {
                let record_map = match &base_binding {
                    Some(Binding::Record(m)) => Some(m.clone()),
                    _ => None,
                };
                if let Some(src) = record_map {
                    install_record_destruct(ctx, &src, fields);
                }
            }
            crate::ast::ParamPattern::Tuple { names, .. } => {
                // For tuple patterns, extract by index from the local binding.
                if let Some(Binding::Local(local)) = &base_binding {
                    let source_node = ctx.builder.module.nodes.get(&local.port.node_id).cloned();
                    if let Some(node) = source_node {
                        let outputs: Vec<_> = node.ports.outputs.iter().collect();
                        for (i, name) in names.iter().enumerate() {
                            if let Some(port) = outputs.get(i) {
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
            }
        }
    }

    // Pre-declare var/array/buffer inside the mod body (recursively into
    // nested if/else blocks) so they're registered in ctx.vars.
    fn pre_declare_block_vars(ctx: &mut LowerCtx, block: &Block) {
        for s in &block.stmts {
            match s {
                Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
                Stmt::Array(a) => pre_declare_array(ctx, a),
                Stmt::Buffer(b) => pre_declare_buffer(ctx, b),
                Stmt::If(i) => {
                    pre_declare_block_vars(ctx, &i.then_block);
                    if let Some(eb) = &i.else_block {
                        pre_declare_block_vars(ctx, eb);
                    }
                }
                _ => {}
            }
        }
    }
    pre_declare_block_vars(ctx, &chip_decl.body);

    // Install this mod's output nodes (for `return value`).
    // Track their IDs so cleanup only removes these, not parent outputs.
    let mut mod_output_ids = Vec::new();
    for out in &chip_decl.outputs {
        pre_declare_output(ctx, &out.name, None, Some(&out.typ), None, None, &out.range);
        if let Some(r) = ctx.lookup_output(&out.name) {
            mod_output_ids.push(r.node_id);
        }
    }

    // `exec = trigger` named arg: run this mod's body off the given trigger
    // when the caller is outside an exec context.
    let exec_arg = args.iter().find_map(|a| match a {
        CallArg::Named { name, value } if name == "exec" => Some(value),
        _ => None,
    });
    let saved_caller_exec = ctx.current_exec;
    if let Some(exec_expr) = exec_arg {
        let src = lower_expr(ctx, exec_expr);
        ctx.current_exec = Some(src);
    }

    let body_has_return = block_contains_return(&chip_decl.body);
    let saved_return_exec = ctx.mod_return_exec.take();
    let saved_return_var = ctx.mod_return_var.take();

    // For multi-return mods with an output, create a PseudoVar to hold
    // the return value. Each `return expr` does a Var_Set; after the
    // return union we Var_Get the result.
    let num_return_values = count_return_values(&chip_decl.body);
    if num_return_values > 1 && chip_decl.outputs.len() == 1 {
        let out_type = type_of_type_expr(&chip_decl.outputs[0].typ);
        let var_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::PSEUDO_VAR,
            source_range: chip_decl.body.range.clone(),
            ports: GateIO {
                inputs: vec![],
                outputs: vec![
                    PortSpec {
                        name: *sym::VALUE,
                        ty: out_type.clone(),
                    },
                    PortSpec {
                        name: *sym::VAR_REF,
                        ty: Type::Ref(Box::new(out_type.clone())),
                    },
                ],
            },
            note: Some("ret_val"),
            ..Default::default()
        });
        ctx.mod_return_var = Some(VarRecord {
            node_id: var_id,
            inner_type: out_type,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        });
    }

    lower_block(ctx, &chip_decl.body);

    if body_has_return {
        // Merge fallthrough (if any) with accumulated return paths
        let fallthrough = ctx.current_exec.take();
        let ret_path = ctx.mod_return_exec.take();
        match (fallthrough, ret_path) {
            (Some(fall), Some(ret)) => {
                let union = ctx.add_gate(AddNodeOpts {
                    gate_class: gc::UNION,
                    source_range: chip_decl.body.range.clone(),
                    ports: GateIO {
                        inputs: vec![
                            PortSpec {
                                name: *sym::EXEC_A,
                                ty: Type::Exec,
                            },
                            PortSpec {
                                name: *sym::EXEC_B,
                                ty: Type::Exec,
                            },
                        ],
                        outputs: vec![PortSpec {
                            name: *sym::EXEC_OUT,
                            ty: Type::Exec,
                        }],
                    },
                    ..Default::default()
                });
                ctx.connect(fall, union.port(WirePort::ExecA));
                ctx.connect(ret, union.port(WirePort::ExecB));
                ctx.current_exec = Some(union.port(WirePort::ExecOut));
            }
            (Some(fall), None) => ctx.current_exec = Some(fall),
            (None, Some(ret)) => ctx.current_exec = Some(ret),
            (None, None) => {}
        }
    }

    // For multi-return mods: Var_Get the return value after the union,
    // then wire to the output node.
    let ret_var_clone = ctx.mod_return_var.clone();
    let multi_return_port = if let Some(ref ret_var) = ret_var_clone {
        if let Some(exec) = ctx.current_exec {
            let inner = ret_var.inner_type.clone();
            let get_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::VAR_GET,
                source_range: SourceRange::default(),
                note: Some("ret_get"),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::EXEC,
                            ty: Type::Exec,
                        },
                        PortSpec {
                            name: *sym::VAR_REF,
                            ty: Type::Ref(Box::new(inner.clone())),
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: *sym::VALUE,
                            ty: inner.clone(),
                        },
                        PortSpec {
                            name: *sym::EXEC_OUT,
                            ty: Type::Exec,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(exec, get_id.port(WirePort::Exec));
            ctx.connect(
                ret_var.node_id.port(WirePort::VarRef),
                get_id.port(WirePort::VarRef),
            );
            ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
            // Wire Var_Get value to the output node
            if ctx.output_count() == 1 {
                let out = ctx.first_output().unwrap().1.clone();
                ctx.connect(
                    get_id.port(WirePort::Value),
                    out.node_id.port(WirePort::RerInput),
                );
            }
            Some(get_id.port(WirePort::Value))
        } else {
            None
        }
    } else {
        None
    };

    ctx.mod_return_exec = saved_return_exec;
    ctx.mod_return_var = saved_return_var;

    // An explicit trigger's chain must not leak into the caller's context.
    if exec_arg.is_some() {
        ctx.current_exec = saved_caller_exec;
    }

    let inline_output_ids = &mod_output_ids;
    // For single-output mods, capture the value source before removing
    let return_output_port = if multi_return_port.is_some() {
        multi_return_port
    } else if chip_decl.outputs.len() == 1 {
        let out_id = &inline_output_ids[0];
        ctx.builder.module.wires[wire_start..]
            .iter()
            .find(|w| w.target.node_id == *out_id && w.target.port == WirePort::RerInput)
            .map(|w| w.source)
    } else {
        None
    };

    ctx.scope.pop();

    // The mod body may have written to vars passed through records.
    // Those writes invalidated caches inside the mod scope (now popped),
    // but the caller's copies of those Var bindings still have stale
    // caches. Clear all caches to ensure subsequent reads produce fresh
    // Var_Gets.
    reset_var_get_caches(ctx);

    // Multi-output inline mod: capture each output's value source into a record
    // so `let s = mod(...); s.field` resolves to the right port (the output
    // nodes below are internal and removed). Set definitively for THIS call —
    // `None` for single-output — so a nested multi-output arg call doesn't leak.
    let return_record = ctx.pending_return_record.take();
    ctx.pending_inline_record = if let Some(rec) = return_record {
        // A `return { ... }` record literal: `-> { a, b }` is one record-typed
        // output, so the fields were destructured into a field->binding map
        // rather than wired to the (single) output node. Bind the caller's
        // record from that map.
        Some(rec)
    } else if chip_decl.outputs.len() > 1 {
        let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for (i, out) in chip_decl.outputs.iter().enumerate() {
            let Some(&out_id) = inline_output_ids.get(i) else {
                continue;
            };
            if let Some(src) = ctx.builder.module.wires[wire_start..]
                .iter()
                .find(|w| w.target.node_id == out_id && w.target.port == WirePort::RerInput)
                .map(|w| w.source)
            {
                record.insert(
                    crate::intern::intern(&out.name),
                    Binding::Local(LocalRecord { port: src }),
                );
            }
        }
        Some(record)
    } else {
        None
    };

    // Inline mod outputs are internal — remove the MicrochipOutput nodes.
    // Their wires all live in the tail added during this call; compact it in
    // place (order-preserving) rather than retain-scanning the whole list.
    if !inline_output_ids.is_empty() {
        for id in inline_output_ids {
            ctx.builder.module.nodes.remove(id);
            ctx.builder.module.outputs.retain(|o| o != id);
        }
        let wires = &mut ctx.builder.module.wires;
        let mut write = wire_start;
        for read in wire_start..wires.len() {
            let w = wires[read];
            if !inline_output_ids.contains(&w.source.node_id)
                && !inline_output_ids.contains(&w.target.node_id)
            {
                wires[write] = w;
                write += 1;
            }
        }
        wires.truncate(write);
    }

    for (i, param) in chip_decl.inputs.iter().enumerate() {
        if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. })
            && let Some(arg_expr) = positional_args.get(i)
            && let Expr::Ident { name, .. } = arg_expr
            && let Some(v) = ctx.lookup_var_mut(name.as_str())
        {
            v.get_node_for_handler = None;
        }
    }

    let result = if let Some(out_port) = return_output_port {
        out_port
    } else {
        ctx.current_exec.unwrap_or_else(|| PortRef {
            node_id: NodeId(0),
            port: WirePort::ExecOut,
        })
    };

    result
}

fn resolve_caller_captures(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
) -> HashMap<String, VarRecord> {
    let positional_args: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect();
    let mut captures = HashMap::default();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };

        let resolved_record = match &param.typ {
            TypeExpr::Record { fields, .. } => Some(fields.clone()),
            TypeExpr::Name { name, .. } => {
                ctx.type_aliases.get(name.as_str()).and_then(|te| match te {
                    TypeExpr::Record { fields, .. } => Some(fields.clone()),
                    _ => None,
                })
            }
            _ => None,
        };

        if let Some(fields) = &resolved_record {
            if let Some(Binding::Record(rec_fields)) = resolve_field_chain(ctx, arg_expr).cloned() {
                for field in fields {
                    if !matches!(&field.typ, TypeExpr::Array { .. } | TypeExpr::Ref { .. }) {
                        continue;
                    }
                    let field_sym = crate::intern::intern(&field.name);
                    if let Some(Binding::Var(var_rec)) = rec_fields.get(&field_sym) {
                        let port_name = format!("{}_{}", param.name, field.name);
                        captures.insert(port_name, var_rec.clone());
                    }
                }
            }
        } else if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
            let var_rec = if let Expr::Ident { name, .. } = arg_expr {
                ctx.lookup_var(name).cloned()
            } else if let Some(Binding::Var(v)) = resolve_field_chain(ctx, arg_expr).cloned() {
                Some(v)
            } else {
                None
            };
            if let Some(var_rec) = var_rec {
                captures.insert(param.name.clone(), var_rec);
            }
        }
    }
    captures
}

fn compute_scope_captures(module: &Module) -> Vec<NodeId> {
    let internal: HashSet<NodeId> = module.nodes.keys().cloned().collect();
    let mut external = Vec::new();
    for w in &module.wires {
        if !internal.contains(&w.source.node_id) && !external.contains(&w.source.node_id) {
            external.push(w.source.node_id);
        }
        if !internal.contains(&w.target.node_id) && !external.contains(&w.target.node_id) {
            external.push(w.target.node_id);
        }
    }
    for child_module in module.chips.values() {
        for &cap_id in &child_module.scope_captures {
            if !internal.contains(&cap_id) && !external.contains(&cap_id) {
                external.push(cap_id);
            }
        }
    }
    external
}

fn build_chip_module(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    instance_name: &str,
    caller_captures: &HashMap<String, VarRecord>,
    force_exec_boundary: bool,
) -> Module {
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
    let mut child_ctx = LowerCtx {
        builder: child_builder,
        ids: IdAllocator::default(),
        diagnostics: Vec::new(),
        type_of_expr: ctx.type_of_expr,
        op_resolutions: ctx.op_resolutions,
        file: ctx.file.clone(),
        scope: crate::scope::Scope::new(),
        handler_end_execs: Vec::new(),
        current_exec: None,
        handler_entry_exec: None,
        captured_events: HashMap::default(),
        next_chain_id: 0,
        current_anon_chip: None,
        mod_return_exec: None,
        mod_return_var: None,
        type_aliases: ctx.type_aliases.clone(),
        pending_emits: HashMap::default(),
        exec_signal_hubs: HashMap::default(),
        exec_signal_keys: HashMap::default(),
        next_scope_id: ROOT_SCOPE_ID + 1,
        template_cache: ctx.template_cache.clone(),
        await_armed_port: None,
        signal_awaits: HashMap::default(),
        exec_branch_depth: 0,
        exec_signal_payloads: HashMap::default(),
        pending_inline_record: None,
        pending_return_record: None,
        chip_call_stack: ctx.chip_call_stack.clone(),
        known_fn_names: ctx.known_fn_names.clone(),
        is_root_module: false,
        doc_comments: ctx.doc_comments,
        // `@nofold chip Foo(...) { ... }`: every gate lowered into this
        // child module (the chip's own body — built once and cloned for
        // every subsequent `template.instantiate` call) must carry
        // `_nofold` from the start, since the body is lowered in this
        // fresh `child_ctx`, not the caller's `ctx`.
        nofold_depth: if chip_decl.no_fold { 1 } else { 0 },
    };

    // A chip is visual grouping only — wire refs cross the boundary freely — so
    // its body closes over the ENTIRE enclosing lexical scope: module globals
    // plus any handler-local `let`s, event params, and block locals in scope at
    // the instantiation point. `iter()` yields innermost-first; keep the first
    // (nearest) binding per name so inner shadows outer. Chip params declared
    // below shadow these in turn.
    //
    // Constants get one extra step: a `let X = <const>` is a `Local` pointing at
    // a `_Literal` node in the parent module. Cloning that literal into the
    // chip's own module lets `inline_orphan_literals` fold it into its consumers
    // as inline gate data (fewer gates) rather than a separate constant brick.
    let mut seen = crate::collections::HashSet::default();
    let inherited: Vec<(crate::intern::Sym, Binding)> = ctx
        .scope
        .iter_syms()
        .filter(|(name, _)| seen.insert(*name))
        .map(|(name, b)| (name, b.clone()))
        .collect();
    for (name, binding) in inherited {
        // A chip body can't target the enclosing module's `out`s, and inheriting
        // them inflates `output_count()`. That makes a single-`return` chip skip
        // its own value-output wiring (`Stmt::Return` only wires when
        // `output_count() == 1`) whenever the parent module declares any `out`.
        if matches!(&binding, Binding::Output(_)) {
            continue;
        }
        if let Binding::Local(local) = &binding
            && let Some(src) = ctx.builder.module.nodes.get(&local.port.node_id)
            && src.gate_class == gc::LITERAL
        {
            let opts = AddNodeOpts {
                gate_class: gc::LITERAL,
                source_range: src.source_range.clone(),
                ports: (*src.ports).clone(),
                properties: (*src.properties).clone(),
                ..Default::default()
            };
            let new_id = child_ctx.add_gate(opts);
            child_ctx
                .scope
                .insert_sym(name, Binding::Local(LocalRecord { port: new_id.port(local.port.port) }));
            continue;
        }
        child_ctx.scope.insert_sym(name, binding);
    }

    for inp in &chip_decl.inputs {
        let resolved_record = match &inp.typ {
            TypeExpr::Record { fields, .. } => Some(fields.clone()),
            TypeExpr::Name { name, .. } => {
                child_ctx
                    .type_aliases
                    .get(name.as_str())
                    .and_then(|te| match te {
                        TypeExpr::Record { fields, .. } => Some(fields.clone()),
                        _ => None,
                    })
            }
            _ => None,
        };
        if let Some(fields) = &resolved_record {
            let mut record_fields = HashMap::default();
            for field in fields {
                let port_name = format!("{}_{}", inp.name, field.name);
                let ft = type_of_type_expr(&field.typ);
                let is_array = matches!(&field.typ, TypeExpr::Array { .. });
                let is_ref = matches!(&field.typ, TypeExpr::Ref { .. });

                if let Some(captured) = (is_array || is_ref)
                    .then(|| caller_captures.get(&port_name))
                    .flatten()
                {
                    record_fields.insert(
                        crate::intern::intern(&field.name),
                        Binding::Var(VarRecord {
                            node_id: captured.node_id,
                            inner_type: captured.inner_type.clone(),
                            get_node_for_handler: None,
                            storage: captured.storage,
                        }),
                    );
                    continue;
                }

                let node_id = child_ctx.add_input(&port_name, ft.clone(), chip_decl.range.clone());
                let binding = if is_array {
                    let inner = match &ft {
                        Type::Array(inner) => inner.as_ref().clone(),
                        Type::Ref(inner) => match inner.as_ref() {
                            Type::Array(inner) => inner.as_ref().clone(),
                            _ => ft.clone(),
                        },
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: VarStorage::Array,
                    })
                } else if is_ref {
                    let inner = match &ft {
                        Type::Ref(inner) => inner.as_ref().clone(),
                        _ => ft.clone(),
                    };
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: VarStorage::Var,
                    })
                } else {
                    Binding::Input(NodeRecord {
                        node_id,
                        ty: ft.clone(),
                    })
                };
                record_fields.insert(crate::intern::intern(&field.name), binding);
            }
            child_ctx
                .scope
                .insert(&inp.name, Binding::Record(record_fields));
        } else if matches!(&inp.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
            if let Some(captured) = caller_captures.get(&inp.name) {
                child_ctx.scope.insert(
                    &inp.name,
                    Binding::Var(VarRecord {
                        node_id: captured.node_id,
                        inner_type: captured.inner_type.clone(),
                        get_node_for_handler: None,
                        storage: captured.storage,
                    }),
                );
            } else {
                let t = type_of_type_expr(&inp.typ);
                let is_array = matches!(&inp.typ, TypeExpr::Array { .. });
                let inner = match &t {
                    Type::Ref(inner) => inner.as_ref().clone(),
                    Type::Array(inner) => inner.as_ref().clone(),
                    _ => t.clone(),
                };
                let node_id = child_ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
                child_ctx.scope.insert(
                    &inp.name,
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage: if is_array {
                            VarStorage::Array
                        } else {
                            VarStorage::Var
                        },
                    }),
                );
            }
        } else {
            let t = type_of_type_expr(&inp.typ);
            let node_id = child_ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
            child_ctx.scope.insert(
                &inp.name,
                Binding::Input(NodeRecord { node_id, ty: t }),
            );
        }
    }
    for out in &chip_decl.outputs {
        let t = type_of_type_expr(&out.typ);
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
        .map(|p| matches!(&p.typ, TypeExpr::Name { name, .. } if name == "exec"))
        .unwrap_or(false);
    let auto_exec = (ctx.current_exec.is_some() || force_exec_boundary) && !first_param_is_exec;
    if auto_exec {
        let exec_in = child_ctx.add_input("_exec_in", Type::Exec, chip_decl.range.clone());
        child_ctx.current_exec = Some(exec_in.port(WirePort::RerOutput));
    }

    let sig_output_names: HashSet<&str> =
        chip_decl.outputs.iter().map(|o| o.name.as_str()).collect();
    for stmt in &chip_decl.body.stmts {
        match stmt {
            Stmt::In(i) => pre_declare_input(&mut child_ctx, i),
            Stmt::Var(v) => child_ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
            Stmt::Buffer(b) => pre_declare_buffer(&mut child_ctx, b),
            Stmt::Array(a) => pre_declare_array(&mut child_ctx, a),
            Stmt::OutBinding(o) if !sig_output_names.contains(o.name.as_str()) => {
                child_ctx.with_nofold(o.no_fold, |ctx| {
                    pre_declare_output(
                        ctx,
                        &o.name,
                        o.value.as_ref(),
                        o.typ.as_ref(),
                        o.side,
                        o.label.as_deref(),
                        &o.range,
                    )
                });
            }
            _ => {}
        }
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
            let merged = match child_ctx.current_exec.take() {
                Some(fall) => {
                    let union = child_ctx.add_gate(AddNodeOpts {
                        gate_class: gc::UNION,
                        source_range: chip_decl.range.clone(),
                        ports: GateIO {
                            inputs: vec![
                                PortSpec {
                                    name: *sym::EXEC_A,
                                    ty: Type::Exec,
                                },
                                PortSpec {
                                    name: *sym::EXEC_B,
                                    ty: Type::Exec,
                                },
                            ],
                            outputs: vec![PortSpec {
                                name: *sym::EXEC_OUT,
                                ty: Type::Exec,
                            }],
                        },
                        ..Default::default()
                    });
                    child_ctx.connect(fall, union.port(WirePort::ExecA));
                    child_ctx.connect(ret, union.port(WirePort::ExecB));
                    union.port(WirePort::ExecOut)
                }
                None => ret,
            };
            child_ctx.current_exec = Some(merged);
        }
        if let Some(tail_exec) = child_ctx.current_exec {
            let exec_out = child_ctx.add_output("_exec_out", Type::Exec, chip_decl.range.clone());
            child_ctx.connect(tail_exec, exec_out.port(WirePort::RerInput));
        }
    }

    ctx.diagnostics.extend(child_ctx.diagnostics);
    let mut module = child_ctx.builder.module;
    module.scope_captures = compute_scope_captures(&module);
    module
}

pub(super) fn lower_chip_call_instance(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    range: &SourceRange,
) -> PortRef {
    static INSTANCE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let idx = INSTANCE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let instance_name = format!("{}_{}", chip_decl.name, idx);

    let caller_captures = resolve_caller_captures(ctx, chip_decl, args);

    // `exec = trigger` named arg: how exec chips get their chain when invoked
    // outside an exec context (mirrors the builtin exec-call convention).
    let exec_arg = args.iter().find_map(|a| match a {
        CallArg::Named { name, value } if name == "exec" => Some(value),
        _ => None,
    });

    let mut child_module = if let Some(template) = ctx.template_cache.get(&chip_decl.name) {
        // Build remap: for each param name in the template's capture_names,
        // look up the caller's VarRecord and map old_id -> new_id.
        let mut captures = std::collections::HashMap::default();
        for (name, old_id) in &template.external_refs {
            if let Some(var_rec) = caller_captures.get(name) {
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
        );
        // Cache the first instance as a template for subsequent calls.
        // Store capture_names so future instantiations can remap by param name.
        let mut template = crate::template::CompiledTemplate::from_module(module.clone());
        // Rebuild external_refs keyed by param name instead of node_id string
        template.external_refs = caller_captures
            .iter()
            .map(|(name, var_rec)| (name.clone(), var_rec.node_id))
            .collect();
        ctx.template_cache.insert(&chip_decl.name, template);
        module
    };
    child_module.template_key = Some(intern(&chip_decl.name));

    // All wiring goes directly to child MicrochipInput/Output nodes.
    // The chip node exists only for layout grouping + microchip link.
    let first_param_is_exec = chip_decl
        .inputs
        .first()
        .map(|p| matches!(&p.typ, TypeExpr::Name { name, .. } if name == "exec"))
        .unwrap_or(false);
    let auto_exec = (ctx.current_exec.is_some() || exec_arg.is_some()) && !first_param_is_exec;

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
        if let Some(label) = &chip_decl.label {
            props.insert(*sym::NAME_LABEL, Literal::String(label.clone()));
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
    // passing the same constants still share a key.
    if !const_folds.is_empty() {
        let mut key = chip_decl.name.clone();
        for fold in &const_folds {
            key.push_str(&format!("\u{1}{}:{:?}", fold.index, fold.value));
        }
        if let Some(child) = ctx.builder.module.chips.get_mut(&chip_node_id) {
            child.template_key = Some(intern(&key));
        }
        for fold in &const_folds {
            fold_const_chip_input(ctx, chip_node_id, fold);
        }
    }

    result
}

/// A constant argument to be folded into the chip instance's own module,
/// replacing the input rerouter it would otherwise have been wired to.
pub(super) struct ConstFold {
    pin: NodeId,
    /// Parameter position, so two calls that fold different params are keyed apart.
    index: usize,
    value: Literal,
    ty: Type,
}

/// A literal argument that can live inside the chip instead of crossing its
/// boundary. Only self-contained scalars qualify — anything that has to be
/// computed still needs a real wire in.
fn const_arg_literal(e: &Expr) -> Option<Literal> {
    match e {
        Expr::IntLit { value, .. } => Some(Literal::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Literal::Float(*value)),
        Expr::BoolLit { value, .. } => Some(Literal::Bool(*value)),
        _ => None,
    }
}

/// Move a constant argument inside the chip instance and drop its input pin.
///
/// A constant that crosses the boundary costs a gate per instance, because the
/// rerouter it feeds can't carry inline gate data. Cloning it into the chip's
/// own module lets the literal-inlining pass fold it onto its consumer, so a
/// `chip` call emits exactly the gates its `mod` equivalent does. This is the
/// same trick already applied to captured constants when the body is built.
///
/// Safe to do per instance: every call site builds its own instance, and the
/// shared template was cloned before any argument wiring ran.
fn fold_const_chip_input(ctx: &mut LowerCtx, chip_node_id: NodeId, fold: &ConstFold) {
    let Some(child) = ctx.builder.module.chips.get_mut(&chip_node_id) else {
        return;
    };
    let Some(pin_node) = child.nodes.get(&fold.pin) else {
        return;
    };
    let mut props = HashMap::default();
    props.insert(*sym::VALUE, fold.value.clone());
    let lit_id = NodeId::fresh();
    let lit = crate::ir::Node {
        id: lit_id,
        kind: NodeKind::Gate,
        gate_class: gc::LITERAL,
        properties: std::sync::Arc::new(props),
        ports: std::sync::Arc::new(GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: fold.ty.clone(),
            }],
        }),
        source_range: pin_node.source_range.clone(),
        chip_id: pin_node.chip_id,
        chain_id: None,
        scope_id: pin_node.scope_id,
        note: None,
    };
    child.nodes.insert(lit_id, lit);
    // Everything the pin fed now reads the literal directly.
    for w in &mut child.wires {
        if w.source.node_id == fold.pin {
            w.source = lit_id.port(WirePort::Output);
        }
    }
    child.wires.retain(|w| w.target.node_id != fold.pin);
    child.nodes.remove(&fold.pin);
    child.inputs.retain(|p| *p != fold.pin);
}

fn wire_chip_args_and_outputs(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    caller_captures: &HashMap<String, VarRecord>,
    child_inputs: &[NodeId],
    child_outputs: &[NodeId],
    const_folds: &mut Vec<ConstFold>,
) -> PortRef {
    let positional_args: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect();
    let mut input_idx: usize = 0;
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };

        if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
            if caller_captures.contains_key(&param.name) {
                continue;
            }
            // Non-captured ref/array: has a MicrochipInput in the child
            let mc_input = child_inputs[input_idx];
            input_idx += 1;
            let is_array = matches!(&param.typ, TypeExpr::Array { .. });
            let ref_port_id = if is_array {
                WirePort::ArrayVarRef
            } else {
                WirePort::VarRef
            };
            if let Expr::Ident { name, .. } = arg_expr {
                if let Some(var_rec) = ctx.lookup_var(name).cloned() {
                    ctx.connect(
                        var_rec.node_id.port(ref_port_id),
                        mc_input.port(WirePort::RerInput),
                    );
                } else if let Some(Binding::Input(inp)) = ctx.scope.get(name).cloned() {
                    // An `in X: T[]` / ref input passed by reference: its array
                    // ref lives at RER_Output, not an ArrayVarRef/VarRef port.
                    ctx.connect(
                        inp.node_id.port(WirePort::RerOutput),
                        mc_input.port(WirePort::RerInput),
                    );
                }
            } else if let Some(Binding::Var(var_rec)) = resolve_field_chain(ctx, arg_expr).cloned()
            {
                ctx.connect(
                    var_rec.node_id.port(ref_port_id),
                    mc_input.port(WirePort::RerInput),
                );
            }
            continue;
        }

        let resolved_rec = match &param.typ {
            TypeExpr::Record { fields, .. } => Some(fields.clone()),
            TypeExpr::Name { name, .. } => {
                ctx.type_aliases.get(name.as_str()).and_then(|te| match te {
                    TypeExpr::Record { fields, .. } => Some(fields.clone()),
                    _ => None,
                })
            }
            _ => None,
        };
        if let Some(fields) = &resolved_rec {
            if let Some(Binding::Record(rec_fields)) = resolve_field_chain(ctx, arg_expr).cloned() {
                for field in fields {
                    let port_name = format!("{}_{}", param.name, field.name);
                    if caller_captures.contains_key(&port_name) {
                        continue;
                    }
                    let mc_input = child_inputs[input_idx];
                    input_idx += 1;
                    let field_sym = crate::intern::intern(&field.name);
                    if let Some(binding) = rec_fields.get(&field_sym) {
                        match binding {
                            Binding::Var(var_rec) => {
                                let vr = if var_rec.storage == VarStorage::Array {
                                    var_rec.node_id.port(WirePort::ArrayVarRef)
                                } else {
                                    var_rec.node_id.port(WirePort::VarRef)
                                };
                                ctx.connect(vr, mc_input.port(WirePort::RerInput));
                            }
                            Binding::Local(local) => {
                                ctx.connect(local.port, mc_input.port(WirePort::RerInput));
                            }
                            Binding::Input(inp) => {
                                ctx.connect(
                                    inp.node_id.port(WirePort::RerOutput),
                                    mc_input.port(WirePort::RerInput),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }

        let mc_input = child_inputs[input_idx];
        input_idx += 1;
        // A `MicrochipInput` is a rerouter, and a rerouter has no data struct to
        // hold inline gate data, so a constant wired in from the caller has to stay
        // a real gate in every instance. Record it instead and clone it into the
        // chip'''s own module below, where `inline_orphan_literals` folds it onto its
        // consumer — the same gates the equivalent `mod` emits.
        if let Some(value) = const_arg_literal(arg_expr) {
            let ty = type_of_type_expr(&param.typ);
            const_folds.push(ConstFold { pin: mc_input, index: input_idx - 1, value, ty });
            continue;
        }
        let val_port = lower_expr(ctx, arg_expr);
        ctx.connect(val_port, mc_input.port(WirePort::RerInput));
    }

    // Invalidate var caches for ref/array params — the chip body may have
    // written to these vars, so subsequent reads need fresh Var_Gets.
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. })
            && let Some(arg_expr) = positional_args.get(i)
            && let Expr::Ident { name, .. } = arg_expr
        {
            if let Some(v) = ctx.lookup_var_mut(name.as_str()) {
                v.get_node_for_handler = None;
            }
        }
    }
    // Also invalidate for dissolved record fields
    for (i, _param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };
        if let Some(Binding::Record(rec_fields)) = resolve_field_chain(ctx, arg_expr).cloned() {
            for (field_sym, binding) in &rec_fields {
                if let Binding::Var(var_rec) = binding {
                    if var_rec.storage == VarStorage::Array || var_rec.storage == VarStorage::Var {
                        if let Some(v) = ctx.lookup_var_mut(&crate::intern::resolve(*field_sym)) {
                            v.get_node_for_handler = None;
                        }
                    }
                }
            }
        }
    }

    if !child_outputs.is_empty() {
        child_outputs[0].port(WirePort::RerOutput)
    } else {
        // Side-effect-only chip — no output value. NodeId(0) is never
        // allocated so any wire referencing it will be caught as invalid.
        NodeId(0).port(WirePort::Output)
    }
}

pub(super) fn lower_builtin_call(
    ctx: &mut LowerCtx,
    spec: &crate::catalog::calls::CallSpec,
    receiver: Option<&Expr>,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // Check for explicit `exec` named arg — allows exec gates in pure contexts
    let explicit_exec = args.iter().find_map(|a| match a {
        CallArg::Named { name, value } if name == "exec" => Some(value),
        _ => None,
    });
    if spec.exec && ctx.current_exec.is_none() && explicit_exec.is_none() {
        return synthesise_unsupported(ctx, e);
    }

    // Match args to params
    let mut bound: HashMap<&str, &Expr> = HashMap::default();
    let mut next_pos = 0usize;
    // A method-call receiver is the first positional argument.
    if let Some(recv) = receiver {
        if let Some(p) = spec.params.first() {
            bound.insert(p.name, recv);
        }
        next_pos = 1;
    }
    for a in args {
        match a {
            CallArg::Named { name, value } => {
                if spec.params.iter().any(|p| p.name == name) {
                    bound.insert(name, value);
                }
            }
            CallArg::Positional(value) => {
                if let Some(p) = spec.params.get(next_pos) {
                    bound.insert(p.name, value);
                }
                next_pos += 1;
            }
            CallArg::Spread(_) => {
                // TODO: handle spread in call lowering
            }
        }
    }

    // Lower args first (adapters may advance exec)
    struct WireEntry {
        port: WirePort,
        val_port: PortRef,
    }
    let mut wires: Vec<WireEntry> = Vec::new();
    let mut properties: HashMap<crate::intern::Sym, Literal> = HashMap::default();

    for p in spec.params.iter() {
        let Some(&arg_expr) = bound.get(p.name) else {
            continue;
        };
        // Literal check — inline constant arguments as properties so they
        // go into the data struct. With negative literal folding in the
        // parser, all constant args (positive and negative) are consistent.
        if let Some(lit) = literal_for_property_port(arg_expr, &p.ty) {
            // Struct-valued constants (folded Vec/Rotation/Color) only
            // inline when the gate's data field is a wire variant; other
            // gates (entity Set*, Split*) need a wired Make* gate, which
            // the fallthrough + materialize pass provides.
            //
            // Rerouter (`Opaque`) has no data struct at all — an inlined
            // property would just be silently dropped at emit time, so it
            // must always keep a real wired literal source instead.
            let inlinable = spec.gate_class != gc::REROUTER
                && (!matches!(
                    lit,
                    Literal::Vector { .. }
                        | Literal::Rotator { .. }
                        | Literal::Quat { .. }
                        | Literal::LinearColor { .. }
                ) || crate::emit::port_accepts_inline_variant(spec.gate_class, p.port));
            if inlinable {
                properties.insert(intern(p.port.as_str()), lit);
                continue;
            }
        }
        let val_port = lower_expr(ctx, arg_expr);
        // character and controller wire directly into each other's ports in
        // Brickadia, so no adapter gate is inserted for a character passed to
        // a controller param (or vice versa). The old char->controller adapter
        // used `GetFromEntity` ("Get Player (Persistent)"), an admin-only gate
        // that gets blocked on paste for non-admins — wire straight through.
        wires.push(WireEntry {
            port: p.port,
            val_port,
        });
    }

    // Build gate ports
    let mut ports = GateIO::default();
    if spec.exec {
        ports.inputs.push(PortSpec {
            name: *sym::EXEC,
            ty: Type::Exec,
        });
        ports.outputs.push(PortSpec {
            name: *sym::EXEC_OUT,
            ty: Type::Exec,
        });
    }
    for p in spec.params.iter() {
        ports.inputs.push(PortSpec {
            name: intern(p.port.as_str()),
            ty: p.ty.clone(),
        });
    }
    for out in &spec.outputs {
        ports.outputs.push(PortSpec {
            name: intern(out.port.as_str()),
            ty: out.ty.clone(),
        });
    }

    // Ensure all gate ports are present — the catalog may define ports
    // not covered by the CallSpec params/output (e.g. InputSplitter has
    // multiple outputs but CallSpec only declares one).
    if let Some(gate) = crate::catalog::default_catalog().find_by_class(spec.gate_class) {
        let existing: std::collections::HashSet<crate::intern::Sym> =
            ports.all_port_names().collect();
        for p in &gate.component.inputs {
            let sym = intern(&p.name);
            if !existing.contains(&sym) {
                ports.inputs.push(PortSpec {
                    name: sym,
                    ty: Type::Any,
                });
            }
        }
        for p in &gate.component.outputs {
            let sym = intern(&p.name);
            if !existing.contains(&sym) {
                ports.outputs.push(PortSpec {
                    name: sym,
                    ty: Type::Any,
                });
            }
        }
    }

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: spec.gate_class,
        source_range: range.clone(),
        ports,
        properties,
        note: Some(spec.name.into()),
        ..Default::default()
    });

    if spec.exec {
        let exec_source = ctx
            .current_exec
            .or_else(|| explicit_exec.map(|e| lower_expr(ctx, e)));
        if let Some(exec) = exec_source {
            ctx.connect(exec, node_id.port(WirePort::Exec));
            if ctx.current_exec.is_some() {
                ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
            }
        }
    }
    for w in wires {
        ctx.connect(w.val_port, node_id.port(w.port));
    }

    // Named record outputs (e.g. Edge's rising/falling): stash a field->port
    // record so a `let` binding resolves fields through the spec instead of
    // port-name matching. Set definitively for THIS call — `None` otherwise —
    // so a nested record-returning arg call doesn't leak into the outer let.
    ctx.pending_inline_record = if spec.outputs.iter().any(|o| o.field.is_some()) {
        let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for out in &spec.outputs {
            if let Some(field) = out.field {
                record.insert(
                    crate::intern::intern(field),
                    Binding::Local(LocalRecord {
                        port: node_id.port(out.port),
                    }),
                );
            }
        }
        Some(record)
    } else {
        None
    };

    if spec.outputs.len() == 1 {
        return node_id.port(spec.outputs[0].port);
    }
    if !spec.outputs.is_empty() {
        return node_id.port(spec.outputs[0].port);
    }
    if spec.exec {
        return node_id.port(WirePort::ExecOut);
    }
    if let Some(p) = spec.params.first() {
        return node_id.port(p.port);
    }
    node_id.port(WirePort::Output)
}
