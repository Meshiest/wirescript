//! The `mod` inline-expansion path: bind params into the caller's scope and
//! lower the body in place.

use super::*;

pub(in crate::lower) fn lower_chip_call_inline(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    type_args: &[TypeExpr],
    _range: &SourceRange,
) -> PortRef {
    // Expand `...tuple` spreads into per-element positional args before binding.
    let expanded = expand_spread_args(ctx, args);
    let args = &expanded[..];
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

    // Generic mod (`mod pick<T>(...)`): rebuild the call's type substitution
    // from the args and push it so the body's `T`-typed annotations
    // monomorphize (see `resolve_local_type`). Guarded on `type_params` being
    // non-empty, so a non-generic mod pushes nothing and is byte-identical.
    // Polymorphic recursion can't diverge here: any (transitive) self-call is
    // already blocked by the WS020 recursion guard in `lower_chip_call` before
    // it re-enters this inline, so the stack can't grow unbounded.
    let pushed_mono = if chip_decl.type_params.is_empty() {
        false
    } else {
        let frame = build_mono_frame(ctx, chip_decl, &positional_args, type_args);
        ctx.mono_stack.push(frame);
        true
    };

    // Collect param bindings first (before mutating ctx) so ref lookups
    // see the caller's vars.
    let mut ref_bindings: Vec<(String, VarRecord)> = Vec::new();
    let mut input_bindings: Vec<(String, NodeRecord)> = Vec::new();
    let mut val_bindings: Vec<(String, PortRef, Type)> = Vec::new();
    let mut record_bindings: Vec<(String, HashMap<crate::intern::Sym, Binding>)> = Vec::new();
    let mut const_bindings: Vec<(String, Literal)> = Vec::new();
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
            // A container param (`T[]`, `Map<K,V>`, `ref T`) captures the
            // caller's var/input binding by reference, so the mod body resolves
            // the param to the same ArrayVar/MapVar ref instead of lowering the
            // arg to a scalar value (which would then fail the container-method
            // lowering to `_Unsupported`).
            pt if super::context::container_storage(pt).is_some() => {
                // `&x` (address-of) passed to a `*T`/`ref T`/container param binds
                // the SAME var as bare `x` — unwrap it so a write through the
                // param reaches the caller's var. Without this the `&` form (the
                // documented `inc(&x)` idiom) bound the param to nothing and every
                // write through it was silently dropped.
                let arg_expr = match arg_expr {
                    Expr::RefOf { operand, .. } => operand.as_ref(),
                    other => other,
                };
                let var_rec = if let Expr::Ident { name, .. } = arg_expr {
                    // A `const` array/map argument: give it its runtime form so
                    // the callee binds a real container ref, exactly as a `var`
                    // argument does. Without this the param bound to nothing and
                    // every container use inside the body failed as WS044. The
                    // callee's binding carries the SAME gate node, so a mutating
                    // method on the param is still rejected (see
                    // `access::reject_const_container_mutation`).
                    materialize_const_container(ctx, arg_expr);
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
                // A `const` parameter is known at inline time — mods inline
                // per call site with no cache, so there is nothing to
                // specialise: the value simply IS the argument's, recorded
                // below into `scoped_consts` so it resolves through
                // `ctx.const_lookup()` exactly like a named constant (gate
                // config, event channel names, array/map baking, ...).
                // Evaluate it HERE, before falling to the ordinary
                // `lower_expr` wire lowering — an argument that's itself a
                // CALL (e.g. a `const mod` call) would otherwise ALSO get
                // fully expanded into real gates by `lower_expr` whether or
                // not those gates end up wired to anything, silently
                // defeating "a const mod call emits no gates". Typecheck
                // already reported an argument that fails to evaluate
                // (WS046), so a failure here is a defensive fallback, not
                // the expected path.
                if param.is_const {
                    let lookup = |n: &str| ctx.resolve_mod(n);
                    let mut budget = crate::const_eval::Budget::default();
                    let evaluated =
                        crate::const_eval::eval_expr(arg_expr, &ctx.const_ctx(Some(&lookup)), &mut budget);
                    if let Ok(lit) = evaluated {
                        const_bindings.push((param.name.clone(), lit));
                        continue;
                    }
                }
                // Whether this param wants a RECORD (an annotated record type,
                // or a destructuring pattern). Computed before lowering, since
                // `record_fields_of` borrows `ctx`.
                let wants_record =
                    param.pattern.is_some() || ctx.record_fields_of(&param.typ).is_some();
                let val_port = lower_expr(ctx, arg_expr);
                // A record-returning CALL as an argument (`Take(Make())`). The
                // call just stashed its field→source-port record, exactly as it
                // does for `let m = Make()`, which is what makes the hand-split
                // `let m = Make()` / `Take(m.o)` work. Consume it here so the
                // record param receives its fields.
                //
                // The two record paths above cannot cover this: a call is
                // neither an `Expr::RecordLit` nor something
                // `resolve_field_chain` can walk. Without this the param bound
                // to ONE opaque value port, so every field read in the callee
                // (and every name of a destructuring pattern) lowered to an
                // `_Unsupported` placeholder that silently read a default.
                //
                // Gated on `wants_record` so a multi-output call passed to a
                // SCALAR param keeps auto-unwrapping to its first output via
                // `val_port`, and on `Expr::Call` so a stale record from some
                // earlier lowering can never be picked up here.
                if wants_record
                    && matches!(arg_expr, Expr::Call { .. })
                    && let Some(record) = ctx.pending_inline_record.take()
                {
                    // The stashed map is keyed by the callee's OUTPUT names. A
                    // single-output call auto-unwraps to that output's value, so
                    // when that value is itself a record, pass the INNER one —
                    // the same unwrap a scalar single-output result gets through
                    // `val_port`. A multi-output result is passed whole.
                    let unwrapped = if record.len() == 1
                        && let Some(Binding::Record(inner)) = record.values().next()
                    {
                        inner.clone()
                    } else {
                        record
                    };
                    record_bindings.push((param.name.clone(), unwrapped));
                    continue;
                }
                let t = type_of_type_expr(&param.typ);
                val_bindings.push((param.name.clone(), val_port, t));
            }
        }
    }

    // A `...rest` variadic parameter captures every positional arg past the
    // fixed params into a compile-time tuple, keyed `"0"`,`"1"`,… — exactly what
    // a tuple literal `(a, b, …)` lowers to. A later `...rest` in the body then
    // splats it back through `expand_spread_args`. Built in the caller's scope
    // (like the other arg bindings) so its elements reference the caller's vars.
    if let Some(rest_name) = &chip_decl.rest {
        let start = chip_decl.inputs.len().min(positional_args.len());
        let rest_fields: Vec<RecordLitField> = positional_args[start..]
            .iter()
            .enumerate()
            .map(|(i, e)| RecordLitField::Named {
                name: i.to_string(),
                value: (*e).clone(),
                range: e.range().clone(),
            })
            .collect();
        let record = lower_record_lit(ctx, &rest_fields);
        record_bindings.push((rest_name.clone(), record));
    }

    // The callee body is lowered into the CALLER's ctx, so the caller's own
    // block scopes stay open underneath it — and `const_lookup` /
    // `const_lookup_declared_only` walk EVERY open frame. That is wrong for
    // constants: typecheck checks a `mod` body exactly ONCE, at its
    // declaration, where no call site's block scope exists, so it resolves the
    // body's constant names against the module environment alone. Lowering
    // reading the call site's frames too made the two disagree on a name the
    // CALLER shadows:
    //
    //     const a = 111
    //     mod inner() { if a == 111 { A } else { B } }
    //     on go { let a = 222   inner() }
    //
    // — typecheck resolves `inner`'s `a` to the top-level 111 and elides `B`,
    // while lowering saw the caller's `a` (222, recorded by `lower_let_decl`'s
    // shadow handling with its `const` mark cleared) EVICT the top-level
    // constant, and emitted a runtime Branch carrying the `B` typecheck never
    // checked. That is the "typecheck dropped a block lowering still emits"
    // direction, the one that is never safe.
    //
    // Swapping the const stacks out for the body's own (rather than clearing
    // the whole scope) keeps everything an inlined body legitimately needs:
    // module-level constants live in `const_env`, which is untouched; a `const`
    // ARGUMENT was already evaluated above, in the caller's environment, before
    // this point; and ordinary bindings still resolve through `ctx.scope`,
    // which is deliberately NOT isolated. It also makes this path agree with
    // the microchip path, which already builds its body against a const stack
    // of its own (`instance_body`'s `scoped_consts`).
    //
    // Restored after `pop_scope` below. Every push and pop inside the body is
    // balanced, so the swapped-in stack is back to depth 0 by then.
    let saved_scoped_consts = std::mem::take(&mut ctx.scoped_consts);
    let saved_scoped_const_declared = std::mem::take(&mut ctx.scoped_const_declared);
    // `out <name> = <record>` bindings are per-body: swap the caller's aside so
    // this expansion starts empty and cannot consume (or be polluted by) an
    // enclosing body's. Restored right after the body, before the record for
    // THIS call is assembled.
    let saved_out_records = std::mem::take(&mut ctx.pending_out_records);

    ctx.push_scope(crate::scope::ScopeTag::MODULE);

    // If this mod was declared inside an `import * as ns` module, push THAT
    // module's members into the body frame FIRST, so its body resolves its own
    // siblings/vars/consts by bare name rather than through the ambient scope
    // (which holds whatever namespace lowered last). Params/refs/inputs are
    // inserted below and shadow these, as they must. See `LowerCtx::ns_mod_scopes`.
    let ns_key = (
        chip_decl.range.file.to_string(),
        chip_decl.range.start.offset,
    );
    if let Some(ns_scope) = ctx.ns_mod_scopes.get(&ns_key).cloned() {
        for (name, binding) in ns_scope.iter() {
            ctx.scope.insert(name, binding.clone());
        }
    }

    for (name, lit) in const_bindings {
        // Every entry here is a `const` PARAMETER (see the `param.is_const`
        // gate above) — mark it declared so an `if`-condition inside this
        // inlined body naming the param is eligible for the widened elision.
        if let Some(frame) = ctx.scoped_const_declared.last_mut() {
            frame.insert(name.clone());
        }
        if let Some(frame) = ctx.scoped_consts.last_mut() {
            frame.insert(name, lit);
        }
    }

    for (name, rec) in ref_bindings {
        ctx.scope.insert(&name, Binding::Var(rec));
    }
    for (name, rec) in input_bindings {
        ctx.scope.insert(&name, Binding::Input(rec));
    }
    for (name, port, _ty) in val_bindings {
        ctx.scope
            .insert(&name, Binding::Local(LocalRecord { port }));
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
            crate::ast::ParamPattern::Tuple { names, rest } => {
                // A tuple literal argument (or a `let` bound to one) arrives as
                // a record keyed by element index, so read the names out of it.
                if let Some(Binding::Record(src)) = &base_binding {
                    let src = src.clone();
                    // Index keys: a tuple-pattern PARAMETER is only ever fed a
                    // tuple literal (or a `let` bound to one), which is keyed
                    // by element index — unlike a `let (a, b) = …` binding,
                    // whose source may be a name-keyed multi-output result.
                    let order: Vec<String> = (0..names.len().max(src.len()))
                        .map(|i| i.to_string())
                        .collect();
                    install_tuple_destruct(ctx, &src, names, rest.as_ref(), &order);
                }
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
    //
    // A nested chip/mod declaration is recorded into `pass1_chips`'s innermost
    // frame — the one `push_scope` opened above and `pop_scope` drops at the
    // end of this function — so it shadows a same-named outer declaration for
    // the WHOLE of this body (including any chip instantiated inside it, which
    // clones the stack as it stands then) and is gone afterwards. Doing this
    // save/restore by hand and undoing it HERE, before the body is lowered,
    // would leave a chip declared and instantiated inside this body
    // resolving the outer declaration while an ordinary call beside it
    // resolved the inner one.
    fn pre_declare_block_vars(ctx: &mut LowerCtx, block: &Block) {
        for s in &block.stmts {
            match s {
                // Nested declaration shadows a same-named outer one for any
                // LATER initializer in this body — see `pre_declare_chip_name`.
                Stmt::ChipDecl(c) => pre_declare_chip_name(ctx, c),
                Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| pre_declare_var(ctx, v)),
                Stmt::Array(a) => pre_declare_array(ctx, a),
                Stmt::Map(m) => pre_declare_map(ctx, m),
                Stmt::Buffer(b) => pre_declare_buffer(ctx, b),
                // Recurse into nested blocks for CHIP NAMES ONLY (their
                // forward-reference visibility), NOT for storage. A `var`/
                // `array`/`map`/`buffer` inside an `if` is block-scoped: it must
                // be declared into ITS OWN block frame when that block is
                // lowered, not hoisted into this body's frame. Hoisting a
                // nested `var k` up here collided it with a same-named body-level
                // `var k` (last-declared won the shared frame), which orphaned
                // one gate and mis-scoped the body-level reads onto the nested
                // storage. The block's own `lower_stmt` declares it fresh
                // (see `needs_declaration`, current-frame).
                Stmt::If(i) => {
                    pre_declare_block_chip_names(ctx, &i.then_block);
                    if let Some(eb) = &i.else_block {
                        pre_declare_block_chip_names(ctx, eb);
                    }
                }
                _ => {}
            }
        }
    }
    /// Recurse a block for nested `chip`/`mod` NAMES only — the forward-reference
    /// pre-declaration that `pre_declare_block_vars` keeps. Storage is
    /// intentionally skipped here.
    fn pre_declare_block_chip_names(ctx: &mut LowerCtx, block: &Block) {
        for s in &block.stmts {
            match s {
                Stmt::ChipDecl(c) => pre_declare_chip_name(ctx, c),
                Stmt::If(i) => {
                    pre_declare_block_chip_names(ctx, &i.then_block);
                    if let Some(eb) = &i.else_block {
                        pre_declare_block_chip_names(ctx, eb);
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
        pre_declare_output(
            ctx,
            &out.name,
            None,
            Some(&out.typ),
            None,
            None,
            None,
            false,
            false,
            &out.range,
        );
        if let Some(r) = ctx.lookup_output(&out.name) {
            mod_output_ids.push(r.node_id);
        }
    }

    // `exec = trigger` named arg: run this mod's body off the given trigger
    // when the caller is outside an exec context.
    let exec_arg = args.iter().find_map(|a| match a {
        CallArg::Named { name, value, .. } if name == "exec" => Some(value),
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
        let out_type = ctx.resolve_local_type(&chip_decl.outputs[0].typ);
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

    ctx.pop_scope();

    // Hand the caller back its own constant environment (see the swap above
    // `push_scope`). Assigned rather than pushed onto: the body's stack is
    // empty again after its balanced `pop_scope`, and anything a callee body
    // recorded is scoped to that body by construction.
    ctx.scoped_consts = saved_scoped_consts;
    ctx.scoped_const_declared = saved_scoped_const_declared;

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
    // Records bound by `out <name> = <record>` in THIS body, swapping the
    // caller's own map back in (see `saved_out_records` above).
    let out_records = std::mem::replace(&mut ctx.pending_out_records, saved_out_records);
    ctx.pending_inline_record = if let Some(rec) = return_record {
        // A `return { ... }` record literal: `-> { a, b }` is one record-typed
        // output, so the fields were destructured into a field->binding map
        // rather than wired to the (single) output node. Bind the caller's
        // record from that map.
        Some(rec)
    } else if chip_decl.outputs.len() == 1
        && let Some(rec) = chip_decl
            .outputs
            .first()
            .and_then(|o| out_records.get(&o.name))
    {
        // A single RECORD-typed output (`mod f() -> (o: Rec) { out o = rec }`).
        // Keyed by the OUTPUT name, exactly like the multi-output case, so
        // `f().o` (and `let d = f()` then `d.o`) keeps resolving. Consumers that
        // want the value itself unwrap the single output — see the argument
        // binding above, which mirrors the auto-unwrap a scalar result gets.
        let name = &chip_decl.outputs[0].name;
        let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        record.insert(crate::intern::intern(name), Binding::Record(rec.clone()));
        Some(record)
    } else if chip_decl.outputs.len() > 1 {
        let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for (i, out) in chip_decl.outputs.iter().enumerate() {
            // A record-typed output among several carries no wire into its
            // output node (a record has no single port), so take its field map
            // and nest it: `r.card.cardtype` then resolves through both hops.
            if let Some(rec) = out_records.get(&out.name) {
                record.insert(crate::intern::intern(&out.name), Binding::Record(rec.clone()));
                continue;
            }
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

    if pushed_mono {
        ctx.mono_stack.pop();
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
