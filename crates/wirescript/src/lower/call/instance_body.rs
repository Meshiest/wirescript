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
    const_params: &ConstEnv,
) -> Module {
    // A generic chip is monomorphized for this instantiation: `mono_frame` is
    // `Some` only for a generic chip, and seeding the child ctx's `mono_stack`
    // with it makes every `T`-annotated storage/operator/if-expr/return in the
    // body — and its boundary ports (below) — resolve to the concrete type.
    // Non-generic chips pass `None`, leaving the stack empty and resolution
    // unaffected.
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
        in_handler_body: false,
        current_exec: None,
        handler_entry_exec: None,
        captured_events: HashMap::default(),
        next_chain_id: 0,
        current_anon_chip: None,
        anon_chip_nodes: crate::collections::HashMap::default(),
        mod_return_exec: None,
        mod_return_var: None,
        mod_return_record: None,
        type_aliases: ctx.type_aliases.clone(),
        generic_type_aliases: ctx.generic_type_aliases.clone(),
        enum_defs: ctx.enum_defs.clone(),
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
        last_value_record_port: None,
        ns_by_file: HashMap::default(),
        pending_return_record: None,
        pending_out_records: HashMap::default(),
        chip_call_stack: ctx.chip_call_stack.clone(),
        known_fn_names: ctx.known_fn_names.clone(),
        // A `const` parameter's call-site value overlays the module constants
        // for this body, so the parameter name resolves through
        // `const_lookup()` inside the chip exactly as it does inside an
        // inlined mod body — which is what lets it reach a literal-requiring
        // position (gate config, event channel names, array/map baking).
        // Overlaying (rather than a separate frame) also gives a param the
        // right shadowing over a same-named module constant. The value is part
        // of the template cache key (`lower_chip_call_instance`), so two calls
        // with different constants can never share this body.
        const_env: if const_params.is_empty() {
            ctx.const_env.clone()
        } else {
            let mut env = (*ctx.const_env).clone();
            for (name, lit) in const_params {
                env.insert(name.clone(), lit.clone());
            }
            std::sync::Arc::new(env)
        },
        // `name: const T` chip params merged above are const-DECLARED by
        // construction (the ONLY way a chip input lands in `const_params` at
        // all is `inp.is_const`) — extend the declared-names set to match, so
        // an `if`-condition inside this body naming the param is eligible for
        // the widened elision exactly like a real `const` binding.
        const_declared: if const_params.is_empty() {
            ctx.const_declared.clone()
        } else {
            let mut set = (*ctx.const_declared).clone();
            for name in const_params.keys() {
                set.insert(name.clone());
            }
            std::sync::Arc::new(set)
        },
        immutable_containers: HashSet::default(),
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
        // leaves the stack empty, so resolution is unaffected.
        mono_stack: mono_frame.map(|f| vec![f]).unwrap_or_default(),
        // Fresh module (its own root scope, not inheriting the caller's block
        // scope — see `scope` above), so no scoped-const frame carries OVER
        // from the caller. It does get one EMPTY frame of its own, and that
        // frame is load-bearing rather than bookkeeping.
        //
        // A chip body is a `Block` of statements, not a list of `TopDecl`s, so
        // its own `const` bindings never go through `build_const_env` the way
        // the root module's do — they are recorded by `lower_let_decl`, which
        // writes into `scoped_consts.last_mut()`. With no frame open at all
        // that is `None`, and the recording was silently discarded: a chip-body
        // `const ch = "…"` was invisible to the chip's OWN handlers, so
        // `SendCustomEvent(ch, 1)` inside it baked an EMPTY channel name with
        // no diagnostic from either stage (the `mod` spelling of the identical
        // code, a top-level `const`, and the same `const` moved inside the
        // handler all baked correctly — which is what made it look like a
        // language rule rather than a dropped frame).
        //
        // Typecheck has always had a frame here: it checks a chip body inside
        // a pushed scope, so a body-level `const` lands in ITS `scoped_consts`
        // and shadows a same-named module constant. Opening one frame here is
        // what makes lowering agree — without it a chip-body `const a = 222`
        // shadowing a top-level `const a = 111` left lowering deciding an `if`
        // on the OUTER value while typecheck decided it on the inner one, the
        // two stages emitting and checking OPPOSITE arms.
        //
        // An empty frame changes nothing for a chip that declares no constant:
        // `const_lookup`, `const_lookup_declared_only` and `is_declared_const`
        // all iterate frames and an empty one contributes no entry.
        scoped_consts: vec![HashMap::default()],
        scoped_const_declared: vec![HashSet::default()],
        dropped_ranges: Vec::new(),
        // Snapshot the caller's whole `pass1_chips` FRAME STACK exactly as it
        // stands at this instantiation point, so a body-local var/array/map
        // initializer inside THIS chip resolves a `const mod` call to the same
        // declaration the enclosing code would. Capturing the stack (rather
        // than a flattened map) is what makes a chip declared inside an
        // inlined `mod` see that mod's frame — the shadowing declaration is
        // still on the stack here, because the frame is dropped by the
        // enclosing `pop_scope` only after this body has been built.
        //
        // Cloning also isolates it: this chip's own nested declarations
        // (recorded by the pre-declare loop below) can never reach the
        // caller's stack.
        pass1_chips: ctx.pass1_chips.clone(),
        importer_names: ctx.importer_names.clone(),
        ns_mod_scopes: ctx.ns_mod_scopes.clone(),
        // A chip body is its own module and performs no import merge — these
        // stay empty (its per-instance state must NEVER dedup across instances
        // by source location, which is exactly why the dedup lives only at the
        // entry module's import sites, not inside `pre_declare_var`).
        import_state_dedup: crate::collections::HashMap::default(),
        import_behavior_lowered: crate::collections::HashSet::default(),
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
        // A `const` parameter is a compile-time value seeded into `const_env`
        // above, NOT a wire — so it gets no `MicrochipInput` pin and consumes
        // no pin slot. `wire_chip_args_and_outputs` skips it on the identical
        // `is_const` predicate (deliberately NOT on "did it evaluate", so the
        // two loops can never disagree): a mismatch here would shift every
        // later param's pin index and silently mis-wire the whole call.
        if inp.is_const {
            continue;
        }
        let resolved_record = child_ctx.record_or_tuple_fields(&inp.typ);
        if let Some(fields) = &resolved_record {
            let record_fields = explode_record_param_pins(
                &mut child_ctx,
                &inp.name,
                fields,
                is_generic,
                caller_captures,
                &chip_decl.range,
            );
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
    for out in &chip_decl.outputs {
        // A record-typed signature output needs one pin per leaf, bound as the
        // same `Binding::Record` shape `pre_declare_output` gives a top-level
        // record `out`. Given a single pin the body's `out p = { … }` has
        // nowhere to land and emits nothing at all.
        if child_ctx.record_fields_of(&out.typ).is_some() {
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
            // This chip's OWN nested chip/mod declarations overlay the
            // `pass1_chips` inherited from the caller above, so a chip-local
            // `const mod` shadowing a same-named top-level one wins in the
            // bake path exactly as it does in every other path. Must stay in
            // this source-ordered loop — see `pre_declare_chip_name`.
            Stmt::ChipDecl(c) => pre_declare_chip_name(&mut child_ctx, c),
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
                        false,
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
    is_generic: bool,
    caller_captures: &HashMap<String, VarRecord>,
    chip_range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut record_fields = HashMap::default();
    for field in fields {
        let port_name = format!("{prefix}_{}", field.name);
        let ft = if is_generic {
            child_ctx.resolve_local_type(&field.typ)
        } else {
            type_of_type_expr(&field.typ)
        };
        // Array / Map / ref fields bind a container ref-port; classified via
        // `container_binding` so a `Map<K,V>` field wires its `MapVarRef`.
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
        if let Some((storage, inner)) = container {
            let node_id = child_ctx.add_input(&port_name, ft.clone(), chip_range.clone());
            record_fields.insert(
                crate::intern::intern(&field.name),
                Binding::Var(VarRecord {
                    node_id,
                    inner_type: inner,
                    get_node_for_handler: None,
                    storage,
                }),
            );
            continue;
        }
        // A nested record/tuple recurses into per-leaf pins; a scalar is one pin.
        if let Some(sub) = child_ctx.record_or_tuple_fields(&field.typ) {
            let nested =
                explode_record_param_pins(child_ctx, &port_name, &sub, is_generic, caller_captures, chip_range);
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
