use super::*;

pub(super) fn lower_event_decl(ctx: &mut LowerCtx, d: &EventDecl) {
    let body = match &d.captured_body {
        Some(b) => b,
        None => return, // alias form - deferred
    };
    let source_name = match &d.source {
        Expr::Ident { name, .. } => name.clone(),
        _ => return,
    };
    let evt = match find_event(&source_name) {
        Some(e) => e,
        None => return,
    };
    let mut outputs = vec![PortSpec {
        name: *sym::EXEC_OUT,
        ty: Type::Exec,
    }];
    for d2 in &evt.data {
        outputs.push(PortSpec {
            name: intern(d2.port),
            ty: d2.ty.clone(),
        });
    }
    let event_node = ctx.add_event(AddNodeOpts {
        gate_class: evt.gate_class,
        source_range: d.source.range().clone(),
        ports: GateIO {
            inputs: vec![],
            outputs,
        },
        ..Default::default()
    });
    let saved_exec = ctx.current_exec;
    let saved_entry = ctx.handler_entry_exec;
    ctx.current_exec = Some(event_node.port(WirePort::ExecOut));
    ctx.handler_entry_exec = Some(event_node.port(WirePort::ExecOut));
    reset_var_get_caches(ctx);
    lower_block(ctx, body);
    if let Some(e) = ctx.current_exec {
        ctx.captured_events.insert(d.name.clone(), e);
    }
    ctx.current_exec = saved_exec;
    ctx.handler_entry_exec = saved_entry;
}

pub(super) fn lower_handler(ctx: &mut LowerCtx, h: &Handler) {
    // Unwrap negated triggers: `on !foo { ... }` → lower `foo`, negate
    // Also handle `on var.value { ... }` field triggers.
    let (trigger_name, trigger_field, negated) = match &h.trigger {
        Trigger::Ident { name, .. } => (name.clone(), None, false),
        Trigger::Field { obj, field, .. } => (obj.clone(), Some(field.clone()), false),
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, .. } => (name.clone(), None, true),
            Trigger::Field { obj, field, .. } => (obj.clone(), Some(field.clone()), true),
            _ => return,
        },
        _ => return,
    };

    let saved_chain = ctx.builder.current_chain_id;
    let chain = ctx.alloc_chain();
    ctx.builder.current_chain_id = Some(chain);

    // Save handler_end_execs so inner blocks don't flush outer handler ends.
    let saved_handler_ends = std::mem::take(&mut ctx.handler_end_execs);

    // Try: record-binding field trigger - chip-call results (`on r.exec`,
    // `on r.someExecOutput`) resolve through the result record.
    if let Some(ref field) = trigger_field {
        let rec_binding = match ctx.scope.get(&trigger_name) {
            Some(Binding::Record(fields_map)) => {
                fields_map.get(&crate::intern::intern(field)).cloned()
            }
            _ => None,
        };
        if let Some(binding) = rec_binding {
            if let Some(trig) = crate::lower::access::binding_to_port(ctx, &binding, &h.range) {
                let saved = (ctx.current_exec, ctx.handler_entry_exec);
                ctx.current_exec = Some(trig);
                ctx.handler_entry_exec = Some(trig);
                reset_var_get_caches(ctx);
                ctx.with_scope(
                    ScopeKind::HandlerBody {
                        trigger_label: format!("{}.{}", trigger_name, field),
                    },
                    h.range.clone(),
                    |ctx| lower_block(ctx, &h.body),
                );
                let this_end = ctx.current_exec;
                ctx.current_exec = saved.0;
                ctx.handler_entry_exec = saved.1;
                ctx.builder.current_chain_id = saved_chain;
                ctx.handler_end_execs = saved_handler_ends;
                if let Some(e) = this_end {
                    ctx.handler_end_execs.push(e);
                }
                return;
            }
        }
    }

    // Try: var.value / var.prev field trigger
    if let Some(ref field) = trigger_field {
        let var_rec = ctx.lookup_var(&trigger_name).cloned();
        if let Some(rec) = var_rec {
            let port_name = match field.as_str() {
                "Value" | "value" => "Value",
                "prev" => "Value",
                _ => {
                    ctx.builder.current_chain_id = saved_chain;
                    ctx.handler_end_execs = saved_handler_ends;
                    return;
                }
            };
            let trig = port_ref(rec.node_id, port_name);
            let saved = (ctx.current_exec, ctx.handler_entry_exec);
            ctx.current_exec = Some(trig);
            ctx.handler_entry_exec = Some(trig);
            reset_var_get_caches(ctx);
            ctx.with_scope(
                ScopeKind::HandlerBody {
                    trigger_label: format!("{}.{}", trigger_name, field),
                },
                h.range.clone(),
                |ctx| lower_block(ctx, &h.body),
            );
            let this_end = ctx.current_exec;
            ctx.current_exec = saved.0;
            ctx.handler_entry_exec = saved.1;
            ctx.builder.current_chain_id = saved_chain;
            ctx.handler_end_execs = saved_handler_ends;
            if let Some(e) = this_end {
                ctx.handler_end_execs.push(e);
            }
            return;
        }
    }

    // Try: captured event alias
    let captured = ctx.captured_events.get(&trigger_name).cloned();
    if let Some(cap) = captured {
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        ctx.current_exec = Some(cap);
        ctx.handler_entry_exec = Some(cap);
        reset_var_get_caches(ctx);
        ctx.with_scope(
            ScopeKind::HandlerBody {
                trigger_label: trigger_name.clone(),
            },
            h.range.clone(),
            |ctx| lower_block(ctx, &h.body),
        );
        let this_end = ctx.current_exec;
        ctx.current_exec = saved.0;
        ctx.handler_entry_exec = saved.1;
        ctx.builder.current_chain_id = saved_chain;
        ctx.handler_end_execs = saved_handler_ends;
        if let Some(e) = this_end {
            ctx.handler_end_execs.push(e);
        }
        return;
    }

    // Try: chip input trigger
    let in_rec = ctx.lookup_input(&trigger_name).cloned();
    if let Some(rec) = in_rec {
        let trig = rec.node_id.port(WirePort::RerOutput);
        let trig = if negated {
            let not_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::LOGICAL_NOT,
                ports: GateIO {
                    inputs: vec![PortSpec {
                        name: *sym::B_INPUT,
                        ty: Type::Bool,
                    }],
                    outputs: vec![PortSpec {
                        name: *sym::B_OUTPUT,
                        ty: Type::Bool,
                    }],
                },
                ..Default::default()
            });
            ctx.connect(trig, not_id.port(WirePort::BInput));
            not_id.port(WirePort::BOutput)
        } else {
            trig
        };
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        ctx.current_exec = Some(trig);
        ctx.handler_entry_exec = Some(trig);
        reset_var_get_caches(ctx);
        ctx.with_scope(
            ScopeKind::HandlerBody {
                trigger_label: trigger_name.clone(),
            },
            h.range.clone(),
            |ctx| lower_block(ctx, &h.body),
        );
        let this_end = ctx.current_exec;
        ctx.current_exec = saved.0;
        ctx.handler_entry_exec = saved.1;
        ctx.builder.current_chain_id = saved_chain;
        ctx.handler_end_execs = saved_handler_ends;
        if let Some(e) = this_end {
            ctx.handler_end_execs.push(e);
        }
        return;
    }

    // Try: buffer as trigger
    let buf_rec = ctx.lookup_buffer(&trigger_name).cloned();
    if let Some(rec) = buf_rec {
        let trig = rec.node_id.port(WirePort::Output);
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        ctx.current_exec = Some(trig);
        ctx.handler_entry_exec = Some(trig);
        reset_var_get_caches(ctx);
        ctx.with_scope(
            ScopeKind::HandlerBody {
                trigger_label: trigger_name.clone(),
            },
            h.range.clone(),
            |ctx| lower_block(ctx, &h.body),
        );
        let this_end = ctx.current_exec;
        ctx.current_exec = saved.0;
        ctx.handler_entry_exec = saved.1;
        ctx.builder.current_chain_id = saved_chain;
        ctx.handler_end_execs = saved_handler_ends;
        if let Some(e) = this_end {
            ctx.handler_end_execs.push(e);
        }
        return;
    }

    // Try: local (let binding) as trigger - fires on value change
    let local_rec = ctx.lookup_local(&trigger_name).cloned();
    if let Some(rec) = local_rec {
        let trigger_port = if negated {
            // `on !x` → add LogicalNOT gate, wire x → NOT, use NOT output as trigger
            let not_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::LOGICAL_NOT,
                ports: GateIO {
                    inputs: vec![PortSpec {
                        name: *sym::B_INPUT,
                        ty: Type::Bool,
                    }],
                    outputs: vec![PortSpec {
                        name: *sym::B_OUTPUT,
                        ty: Type::Bool,
                    }],
                },
                ..Default::default()
            });
            ctx.connect(rec.port, not_id.port(WirePort::BInput));
            not_id.port(WirePort::BOutput)
        } else {
            rec.port
        };
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        ctx.current_exec = Some(trigger_port);
        ctx.handler_entry_exec = Some(trigger_port);
        reset_var_get_caches(ctx);
        ctx.with_scope(
            ScopeKind::HandlerBody {
                trigger_label: trigger_name.clone(),
            },
            h.range.clone(),
            |ctx| lower_block(ctx, &h.body),
        );
        let this_end = ctx.current_exec;
        ctx.current_exec = saved.0;
        ctx.handler_entry_exec = saved.1;
        ctx.builder.current_chain_id = saved_chain;
        ctx.handler_end_execs = saved_handler_ends;
        if let Some(e) = this_end {
            ctx.handler_end_execs.push(e);
        }
        return;
    }

    // Built-in event
    let evt = match find_event(&trigger_name) {
        Some(e) => e,
        None => {
            ctx.builder.current_chain_id = saved_chain;
            return;
        }
    };

    let trigger_range = match &h.trigger {
        Trigger::Ident { range, .. } => range.clone(),
        _ => SourceRange::default(),
    };

    let mut event_outputs = vec![PortSpec {
        name: *sym::EXEC_OUT,
        ty: Type::Exec,
    }];
    for d in &evt.data {
        event_outputs.push(PortSpec {
            name: intern(d.port),
            ty: d.ty.clone(),
        });
    }
    let event_node = ctx.add_event(AddNodeOpts {
        gate_class: evt.gate_class,
        source_range: trigger_range,
        ports: GateIO {
            inputs: vec![],
            outputs: event_outputs,
        },
        properties: event_config_props(evt, &h.config),
        ..Default::default()
    });

    ctx.scope.push(crate::scope::ScopeTag::BLOCK);
    for (i, pname) in h.params.iter().enumerate() {
        if let Some(data) = evt.data.get(i) {
            ctx.scope.insert(
                pname.clone(),
                Binding::EventParam(port_ref(event_node, data.port)),
            );
        }
    }

    let saved_exec = ctx.current_exec;
    let saved_entry = ctx.handler_entry_exec;
    ctx.current_exec = Some(event_node.port(WirePort::ExecOut));
    ctx.handler_entry_exec = Some(event_node.port(WirePort::ExecOut));
    reset_var_get_caches(ctx);

    ctx.with_scope(
        ScopeKind::HandlerBody {
            trigger_label: trigger_name.clone(),
        },
        h.range.clone(),
        |ctx| lower_block(ctx, &h.body),
    );

    let this_end = ctx.current_exec;
    ctx.current_exec = saved_exec;
    ctx.handler_entry_exec = saved_entry;
    ctx.scope.pop();
    ctx.builder.current_chain_id = saved_chain;
    ctx.handler_end_execs = saved_handler_ends;
    if let Some(e) = this_end {
        ctx.handler_end_execs.push(e);
    }
}

/// Resolve an event handler's config args (e.g. `on ChatCommand("greet",
/// Description = "Greets you")`) into the event gate's data-struct properties.
/// Positional literals fill `evt.config_positional` in order; named args target
/// a field via `evt.config_named` (case-insensitive). Args with no matching
/// slot, or non-literal values, are ignored.
fn event_config_props(
    evt: &crate::catalog::events::EventSpec,
    config: &[HandlerConfigArg],
) -> HashMap<crate::intern::Sym, Literal> {
    let mut props: HashMap<crate::intern::Sym, Literal> = HashMap::new();
    let mut positional = 0;
    for arg in config {
        let (field, value) = match arg {
            HandlerConfigArg::Positional(value) => {
                let field = evt.config_positional.get(positional).copied();
                positional += 1;
                (field, value)
            }
            HandlerConfigArg::Named { name, value } => {
                let key = name.to_ascii_lowercase();
                let field = evt
                    .config_named
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, f)| *f);
                (field, value)
            }
        };
        if let (Some(field), Some(lit)) = (field, expr_to_literal(value)) {
            props.insert(intern(field), lit);
        }
    }
    props
}

pub(super) fn lower_block(ctx: &mut LowerCtx, block: &Block) {
    // Pre-declare vars inside stmt-level anon chips.
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
        lower_stmt(ctx, s);
    }
}

pub(super) fn is_handler_like(d: &TopDecl) -> bool {
    match d {
        TopDecl::Handler(_) => true,
        TopDecl::AnonChip(ac) => ac
            .body
            .stmts
            .iter()
            .any(|s| matches!(s, Stmt::Handler(_) | Stmt::AnonChip(_))),
        _ => false,
    }
}

/// Union all accumulated handler end execs into a single Union gate,
/// setting `current_exec` so subsequent code chains from every handler's exit.
pub(super) fn flush_handler_end_execs(ctx: &mut LowerCtx) {
    let ends = std::mem::take(&mut ctx.handler_end_execs);
    if ends.is_empty() {
        return;
    }
    if ends.len() == 1 {
        ctx.current_exec = Some(ends.into_iter().next().unwrap());
        reset_var_get_caches(ctx);
        return;
    }
    // Chain Union gates pairwise: Union(a, b) → Union(prev, c) → ...
    let mut iter = ends.into_iter();
    let first = iter.next().unwrap();
    let second = iter.next().unwrap();
    let mut prev_out = {
        let union_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::UNION,
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
        ctx.connect(first, union_id.port(WirePort::ExecA));
        ctx.connect(second, union_id.port(WirePort::ExecB));
        union_id.port(WirePort::ExecOut)
    };
    for end in iter {
        let union_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::UNION,
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
        ctx.connect(prev_out, union_id.port(WirePort::ExecA));
        ctx.connect(end, union_id.port(WirePort::ExecB));
        prev_out = union_id.port(WirePort::ExecOut);
    }
    ctx.current_exec = Some(prev_out);
    reset_var_get_caches(ctx);
}
