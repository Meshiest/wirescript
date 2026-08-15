//! Build a microchip's child module.

use super::*;
use crate::collections::HashSet;

pub(super) fn build_chip_module(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    instance_name: &str,
    caller_captures: &HashMap<String, VarRecord>,
    force_exec_boundary: bool,
    mono_frame: Option<MonoFrame>,
) -> Module {
    // A generic chip is monomorphized for this instantiation: `mono_frame` is
    // `Some` only for a generic chip, and seeding the child ctx's `mono_stack`
    // with it makes every `T`-annotated storage/operator/if-expr/return in the
    // body — and its boundary ports (below) — resolve to the concrete type.
    // Non-generic chips pass `None` → empty stack → byte-identical to before.
    let is_generic = mono_frame.is_some();
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
        ce_slots: ctx.ce_slots,
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
        generic_type_aliases: ctx.generic_type_aliases.clone(),
        pending_emits: HashMap::default(),
        output_backing_vars: HashMap::default(),
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
        const_env: ctx.const_env.clone(),
        is_root_module: false,
        doc_comments: ctx.doc_comments,
        // `@nofold chip Foo(...) { ... }`: every gate lowered into this
        // child module (the chip's own body — built once and cloned for
        // every subsequent `template.instantiate` call) must carry
        // `_nofold` from the start, since the body is lowered in this
        // fresh `child_ctx`, not the caller's `ctx`.
        nofold_depth: if chip_decl.no_fold { 1 } else { 0 },
        // Generic chips ARE monomorphized: one template per `(name, subst)`
        // (keyed in `lower_chip_call_instance`), so the shared-template concern
        // is moot — each distinct instantiation builds its own body here. Seed
        // the stack with this call's frame so every `T` in the body resolves to
        // the concrete monomorph (`resolve_local_type` / `mono_op_rule` /
        // `lower_if_expr` all read `mono_stack`). `None` for a non-generic chip
        // → empty stack → byte-identical resolution to before.
        mono_stack: mono_frame.map(|f| vec![f]).unwrap_or_default(),
        // Fresh module (its own root scope, not inheriting the caller's block
        // scope — see `scope` above), so no scoped-const frame carries over
        // either; any body-local `let` inside re-populates its own frame via
        // `push_scope`/`pop_scope`.
        scoped_consts: Vec::new(),
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
            child_ctx.scope.insert_sym(
                name,
                Binding::Local(LocalRecord {
                    port: new_id.port(local.port.port),
                }),
            );
            continue;
        }
        child_ctx.scope.insert_sym(name, binding);
    }

    for inp in &chip_decl.inputs {
        let resolved_record = child_ctx.record_fields_of(&inp.typ);
        if let Some(fields) = &resolved_record {
            let mut record_fields = HashMap::default();
            for field in fields {
                let port_name = format!("{}_{}", inp.name, field.name);
                let ft = if is_generic {
                    child_ctx.resolve_local_type(&field.typ)
                } else {
                    type_of_type_expr(&field.typ)
                };
                // Array / Map / ref fields bind a container ref-port; a scalar
                // field is a plain by-value input. Classifying via
                // `container_binding` (not an `Array`/`Ref`-only match) is what
                // lets a `Map<K,V>` record field wire its `MapVarRef` instead of
                // silently lowering its methods to `_Unsupported`.
                let container = super::context::container_binding(&field.typ, &ft);

                if let Some(captured) = container
                    .is_some()
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
                let binding = match container {
                    Some((storage, inner)) => Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage,
                    }),
                    None => Binding::Input(NodeRecord {
                        node_id,
                        ty: ft.clone(),
                    }),
                };
                record_fields.insert(crate::intern::intern(&field.name), binding);
            }
            child_ctx
                .scope
                .insert(&inp.name, Binding::Record(record_fields));
        } else if super::context::container_storage(&inp.typ).is_some() {
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
                let t = if is_generic {
                    child_ctx.resolve_local_type(&inp.typ)
                } else {
                    type_of_type_expr(&inp.typ)
                };
                let (storage, inner) = super::context::container_binding(&inp.typ, &t)
                    .expect("gated by container_storage");
                let node_id = child_ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
                child_ctx.scope.insert(
                    &inp.name,
                    Binding::Var(VarRecord {
                        node_id,
                        inner_type: inner,
                        get_node_for_handler: None,
                        storage,
                    }),
                );
            }
        } else {
            let t = if is_generic {
                child_ctx.resolve_local_type(&inp.typ)
            } else {
                type_of_type_expr(&inp.typ)
            };
            let node_id = child_ctx.add_input(&inp.name, t.clone(), chip_decl.range.clone());
            child_ctx
                .scope
                .insert(&inp.name, Binding::Input(NodeRecord { node_id, ty: t }));
        }
    }
    for out in &chip_decl.outputs {
        let t = if is_generic {
            child_ctx.resolve_local_type(&out.typ)
        } else {
            type_of_type_expr(&out.typ)
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
            Stmt::Map(m) => pre_declare_map(&mut child_ctx, m),
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
                        &o.range,
                    )
                });
            }
            _ => {}
        }
    }
    // Multi-return chips hold the returned value in a PseudoVar: each `return
    // expr` does a Var_Set into it, and a single Var_Get after the return union
    // (below) feeds the value output. The inline-mod path does the same
    // (`lower_chip_call_inline`); without it here two `return`s each wired
    // straight into the output pin — a load-time fan-in with no diagnostic
    // (P0-3). Only meaningful with an exec context (returns fire on exec chains)
    // and exactly one declared output.
    if auto_exec && count_return_values(&chip_decl.body) > 1 && chip_decl.outputs.len() == 1 {
        let out_type = if is_generic {
            child_ctx.resolve_local_type(&chip_decl.outputs[0].typ)
        } else {
            type_of_type_expr(&chip_decl.outputs[0].typ)
        };
        let var_id = child_ctx.add_gate(AddNodeOpts {
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
        child_ctx.mod_return_var = Some(VarRecord {
            node_id: var_id,
            inner_type: out_type,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        });
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
        // Multi-return: Var_Get the accumulated return value on the merged exec
        // and wire it to the single value output — one wire, no fan-in. Mirrors
        // the inline path's post-union Var_Get. Runs before `_exec_out` so the
        // Var_Get's exec becomes the chip's exec tail.
        if let Some(ret_var) = child_ctx.mod_return_var.clone()
            && let Some(exec) = child_ctx.current_exec
        {
            let inner = ret_var.inner_type.clone();
            let get_id = child_ctx.add_gate(AddNodeOpts {
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
            child_ctx.connect(exec, get_id.port(WirePort::Exec));
            child_ctx.connect(
                ret_var.node_id.port(WirePort::VarRef),
                get_id.port(WirePort::VarRef),
            );
            child_ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
            if child_ctx.output_count() == 1 {
                let out = child_ctx.first_output().unwrap().1.clone();
                child_ctx.connect(
                    get_id.port(WirePort::Value),
                    out.node_id.port(WirePort::RerInput),
                );
            }
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
    super::stmt::flush_pending_emits(&mut child_ctx);

    ctx.diagnostics.extend(child_ctx.diagnostics);
    let mut module = child_ctx.builder.module;
    module.scope_captures = compute_scope_captures(&module);
    module
}
