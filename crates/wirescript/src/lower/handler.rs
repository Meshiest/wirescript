use super::*;
use crate::typecheck::{CeNamespace, ce_slot_key};

/// Resolve a handler param's type annotation to a wire `Type` (for typing an
/// event's data-output ports — Custom Event). Delegates to the crate's single
/// canonical resolver (`types::resolve::resolve_type`); no generic params or
/// type aliases are in scope here, and any unknown-name diagnostic is
/// discarded (typecheck has already flagged it with WS002).
fn type_expr_to_type(te: &crate::ast::TypeExpr) -> Type {
    let cx = crate::types::resolve::ResolveCtx {
        params: &[],
        type_aliases: &crate::collections::HashMap::default(),
        generic_aliases: &crate::collections::HashMap::default(),
    };
    crate::types::resolve::resolve_type(te, &cx, &mut Vec::new())
}

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
        None => {
            // Not a built-in Event: `let e = on go { … }` where `go` is an exec
            // `in`put / `var` / `let` / event data param. Trigger the body off
            // that source's port and capture its exit as `e`, exactly like a
            // plain `on go` handler. Without this the whole captured body was
            // silently dropped for every non-Event trigger.
            if let Some(trig) = resolve_captured_source_port(ctx, &source_name) {
                let saved_exec = ctx.current_exec;
                let saved_entry = ctx.handler_entry_exec;
                ctx.current_exec = Some(trig);
                ctx.handler_entry_exec = Some(trig);
                reset_var_get_caches(ctx);
                lower_block(ctx, body);
                if let Some(e) = ctx.current_exec {
                    ctx.captured_events.insert(d.name.clone(), e);
                }
                ctx.current_exec = saved_exec;
                ctx.handler_entry_exec = saved_entry;
            }
            return;
        }
    };
    let mut outputs = vec![PortSpec {
        name: intern(evt.exec_out),
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
    ctx.current_exec = Some(port_ref(event_node, evt.exec_out));
    ctx.handler_entry_exec = Some(port_ref(event_node, evt.exec_out));
    reset_var_get_caches(ctx);
    lower_block(ctx, body);
    if let Some(e) = ctx.current_exec {
        ctx.captured_events.insert(d.name.clone(), e);
    }
    ctx.current_exec = saved_exec;
    ctx.handler_entry_exec = saved_entry;
}

/// Resolve a captured handler's non-event trigger source (`let e = on go { … }`)
/// to its exec/value trigger port — an exec `in`put, a `let`/local, a `var`, or
/// an enclosing event data param — the same sources a plain `on go` handler
/// fires off.
fn resolve_captured_source_port(ctx: &mut LowerCtx, name: &str) -> Option<PortRef> {
    if let Some(rec) = ctx.lookup_input(name).cloned() {
        return Some(rec.node_id.port(WirePort::RerOutput));
    }
    if let Some(rec) = ctx.lookup_local(name).cloned() {
        return Some(rec.port);
    }
    if let Some(rec) = ctx.lookup_var(name).cloned() {
        return Some(port_ref(rec.node_id, "Value"));
    }
    if let Some(Binding::EventParam(p)) = ctx.scope.get(name).cloned() {
        return Some(p);
    }
    None
}

pub(super) fn lower_handler(ctx: &mut LowerCtx, h: &Handler) {
    // Union trigger `on a | b { ... }`: run the body when ANY part fires. Lower
    // the body once per part (each part reuses the full single-trigger
    // resolution below), so it works for every trigger kind. Without this the
    // whole handler fell into the `_ => return` below and was silently dropped.
    if let Trigger::Union { parts, .. } = &h.trigger {
        for part in parts {
            let mut single = h.clone();
            single.trigger = part.clone();
            lower_handler(ctx, &single);
        }
        return;
    }
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
            // `on ns.trigger` where `ns` came from `import * as ns`: resolve the
            // member through that namespace. An imported `in` port binds as
            // `Binding::Input`, which `binding_to_port` turns into its
            // `RerOutput` exactly like a local input. Without this the trigger
            // matched nothing and the ENTIRE handler was dropped, with no
            // diagnostic from either stage.
            Some(Binding::Namespace(members)) => members.get(field).cloned(),
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

    // Try: general expression/call trigger - `on <expr> [-> <pattern>]
    // { ... }` where `<expr>` is a mod/chip call, desugared by the parser into a
    // synthetic `_on_expr_N` trigger + a queued `let _on_expr_N = <expr>`. `on`
    // triggers on the call's own exec OUTPUT (a structural `Type::Exec` field in
    // its typechecked result record - there is exactly one; see the design doc's
    // REVISION section), NOT on a value edge, and `-> <pattern>` binds the
    // record's remaining (data) fields (tuple = positional over the non-exec
    // fields in declared order, `source_field: None`; record = by name,
    // `source_field: Some(original_field_name)`).
    //
    // This form REQUIRES a materializable exec trigger. A call that exposes NO
    // bindable exec output - notably an INLINE `mod` (whose completion exec is
    // not yet surfaced as a record field - a separate lowering gap) - is a HARD
    // ERROR (WS043), NEVER a silent fall-through to the value-changed-local
    // branch below (which would miswire the call's value output as an exec
    // trigger and orphan the real `exec = ...`, or drop the whole handler).
    // The value-change form (`on ServerUptime() { }`, `on a.Dot(b) > 0 { }`) is
    // preserved: it produces neither an exec-typed result field NOR `-> ` params,
    // so it skips this block entirely and falls through to the local branch.
    if trigger_field.is_none()
        && !negated
        && matches!(&h.trigger, Trigger::Ident { name, .. } if name.starts_with("_on_expr_"))
    {
        let Trigger::Ident { range: trig_range, .. } = &h.trigger else {
            unreachable!()
        };
        // Engage on the SCOPE BINDING / arrow-params / explicit `exec =` intent,
        // NOT on the typechecked result type: a multi-output/exec call binds
        // `_on_expr_N` to a `Binding::Record` (built in `lower_let_decl` from the
        // chip's outputs or an inline mod's pending record), `-> <pattern>` fills
        // `h.params`, and an `exec = <x>` arg (`h.expr_trigger_has_exec_arg`) is
        // explicit "drive this as exec" intent. Any of the three means the
        // handler triggers on the call's completion exec — materialize it, or
        // WS043. A bare value or `on <call>.field` trigger with none of the three
        // (`on ServerUptime() { }`, `on doWork(5) { }`, `on Branch(c, a).A { }`)
        // binds a `Binding::Local`, has no params, and passed no `exec =`, so it
        // is left to the value-change branch below. (The result TYPE is NOT a
        // reliable engage signal: `type_of_expr` can carry a nested expr's record
        // type at a `.field` trigger's key via key overlap.)
        let is_record_binding = matches!(ctx.scope.get(&trigger_name), Some(Binding::Record(_)));
        if is_record_binding || !h.params.is_empty() || h.expr_trigger_has_exec_arg {
            let key = (
                trig_range.file.clone(),
                trig_range.start.offset,
                trig_range.end.offset,
            );
            // The call's typechecked result shape + its structural exec field
            // name (typecheck records the record type at the call's own range;
            // reliable here because a record-binding trigger is a plain call
            // expr, not a `.field` access whose key can overlap a nested expr).
            let shape = match ctx.type_of_expr.get(&key).cloned() {
                Some(Type::Record(s)) => Some(s),
                _ => None,
            };
            let exec_name = shape.as_ref().and_then(|s| {
                s.iter().find(|(_, t)| matches!(t, Type::Exec)).map(|(n, _)| n.clone())
            });
            // Materialize a bindable exec trigger from the call's result record,
            // or `None` when the call exposes none (e.g. an inline mod).
            let materialized = if let Some(en) = &exec_name
                && let Some(Binding::Record(fm)) = ctx.scope.get(&trigger_name).cloned()
                && let Some(exec_b) = fm.get(&crate::intern::intern(en)).cloned()
                && let Some(trig) = crate::lower::access::binding_to_port(ctx, &exec_b, &h.range)
            {
                Some((trig, fm))
            } else {
                None
            };

            match materialized {
                Some((trig, fields_map)) => {
                    let exec_name = exec_name.expect("materialized implies an exec field");
                    let shape = shape.expect("materialized implies a record shape");
                    let data_fields: Vec<&str> = shape
                        .iter()
                        .filter(|(n, _)| n != &exec_name)
                        .map(|(n, _)| n.as_str())
                        .collect();
                    let saved = (ctx.current_exec, ctx.handler_entry_exec);
                    ctx.current_exec = Some(trig);
                    ctx.handler_entry_exec = Some(trig);
                    reset_var_get_caches(ctx);
                    ctx.with_scope(
                        ScopeKind::HandlerBody {
                            trigger_label: trigger_name.clone(),
                        },
                        h.range.clone(),
                        |ctx| {
                            for (i, pname) in h.params.iter().enumerate() {
                                let field_name: Option<&str> = match &pname.source_field {
                                    Some(f) => Some(f.as_str()),
                                    None => data_fields.get(i).copied(),
                                };
                                let Some(fname) = field_name else { continue };
                                let Some(binding) =
                                    fields_map.get(&crate::intern::intern(fname)).cloned()
                                else {
                                    continue;
                                };
                                if let Some(port) = crate::lower::access::binding_to_port(
                                    ctx,
                                    &binding,
                                    &pname.range,
                                ) {
                                    ctx.scope.insert(&pname.name, Binding::EventParam(port));
                                }
                            }
                            lower_block(ctx, &h.body)
                        },
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
                None => {
                    ctx.diagnostics.push(Diagnostic::error(
                        "WS043",
                        "this `on <call>` trigger's call exposes no exec output to trigger on - \
                         `on` fires on the call's completion exec (bind its data outputs with \
                         `-> ...`, drive it with `exec = ...`). Use an event or a named `chip` \
                         call; an inline `mod` does not yet surface a completion exec, so an \
                         `exec = ...` on it has nothing to attach to"
                            .to_string(),
                        h.range.clone(),
                    ));
                    ctx.builder.current_chain_id = saved_chain;
                    ctx.handler_end_execs = saved_handler_ends;
                    return;
                }
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

    // Try: local (let binding) as trigger - fires on value change. A field
    // trigger (`on split.Jump`) selects the named output port on the local's
    // gate (e.g. InputReader's `bPressedJump`) instead of its default port.
    let local_rec = ctx.lookup_local(&trigger_name).cloned();
    if let Some(rec) = local_rec {
        let base_port = match &trigger_field {
            Some(field) => {
                crate::lower::access::resolve_output_field_port(ctx, rec.port.node_id, field)
                    .unwrap_or(rec.port)
            }
            None => rec.port,
        };
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
            ctx.connect(base_port, not_id.port(WirePort::BInput));
            not_id.port(WirePort::BOutput)
        } else {
            base_port
        };
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        ctx.current_exec = Some(trigger_port);
        ctx.handler_entry_exec = Some(trigger_port);
        reset_var_get_caches(ctx);
        let trigger_label = match &trigger_field {
            Some(field) => format!("{}.{}", trigger_name, field),
            None => trigger_name.clone(),
        };
        ctx.with_scope(
            ScopeKind::HandlerBody { trigger_label },
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

    // Try: a `var` as trigger — fires on the var's value change (`on x` / `on !x`).
    // Mirrors the local (let-binding) case: the var gate's `Value` output is the
    // trigger port; negation adds a LogicalNOT.
    if trigger_field.is_none() {
        if let Some(rec) = ctx.lookup_var(&trigger_name).cloned() {
            let base_port = port_ref(rec.node_id, "Value");
            let trigger_port = if negated {
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
                ctx.connect(base_port, not_id.port(WirePort::BInput));
                not_id.port(WirePort::BOutput)
            } else {
                base_port
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
    }

    // Try: an enclosing handler's event data param as trigger — `on p` / `on !p`
    // where `p` is e.g. `on CustomEvent("x") -> (p: character)`'s data output.
    // Fires on the param's value edge; negation adds a LogicalNOT, exactly like
    // the local (let-binding) case above.
    if let Some(Binding::EventParam(ep_port)) = ctx.scope.get(&trigger_name).cloned() {
        let base_port = match &trigger_field {
            Some(field) => {
                crate::lower::access::resolve_output_field_port(ctx, ep_port.node_id, field)
                    .unwrap_or(ep_port)
            }
            None => ep_port,
        };
        let trigger_port = if negated {
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
            ctx.connect(base_port, not_id.port(WirePort::BInput));
            not_id.port(WirePort::BOutput)
        } else {
            base_port
        };
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        ctx.current_exec = Some(trigger_port);
        ctx.handler_entry_exec = Some(trigger_port);
        reset_var_get_caches(ctx);
        let trigger_label = match &trigger_field {
            Some(field) => format!("{}.{}", trigger_name, field),
            None => trigger_name.clone(),
        };
        ctx.with_scope(
            ScopeKind::HandlerBody { trigger_label },
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

    // Named args that wire a value into a gate INPUT port (e.g. `zone = zoneA`).
    let mut input_wires: Vec<(&'static str, &Type, &Expr)> = Vec::new();
    for (surf, port, ty) in &evt.input_named {
        for arg in &h.config {
            if let HandlerConfigArg::Named { name, value } = arg {
                if name.eq_ignore_ascii_case(surf) {
                    input_wires.push((port, ty, value));
                }
            }
        }
    }
    let event_inputs: Vec<PortSpec> = input_wires
        .iter()
        .map(|&(port, ty, _)| PortSpec {
            name: intern(port),
            ty: ty.clone(),
        })
        .collect();

    let mut event_outputs = vec![PortSpec {
        name: intern(evt.exec_out),
        ty: Type::Exec,
    }];
    // Custom Event types its data-output ports from the handler's annotations
    // (`on CustomEvent("x") -> (a: int, b: float)`); a present-but-unannotated
    // param takes the inferred type (typecheck's `CeSlotMap`, resolved from
    // an in-unit sender), falling back to float when nothing was inferred.
    // Other events keep their fixed declared data types.
    let is_custom_event = evt.gate_class == crate::ir::gate_class::PSEUDO_CUSTOM_EVENT;
    // Resolved inferred slot types for this handler, if it's a custom-event
    // receiver — see `crate::typecheck::infer_custom_event_slots`.
    let inferred = CeNamespace::from_event_name(evt.surface_name)
        .and_then(|ns| ctx.ce_slots.get(&ce_slot_key(ns, h)));
    let inferred_slot = |i: usize| inferred.and_then(|v| v.get(i).cloned().flatten());
    for (i, d) in evt.data.iter().enumerate() {
        let ty = match h.params.get(i) {
            Some(p) => match &p.ty {
                Some(te) => type_expr_to_type(te),
                // Present but unannotated: use the inferred type when this is a
                // custom-event receiver (the inference pass always fills the
                // slot — an in-unit sender's type or a float fallback), else the
                // event's fixed catalog type. A non-custom event (`inferred` is
                // `None`) must keep `d.ty` — e.g. `on CharacterSpawned() -> (character)`
                // keeps its `Character` port, NOT float.
                None => inferred_slot(i).unwrap_or_else(|| d.ty.clone()),
            },
            None if is_custom_event => Type::Float,
            None => d.ty.clone(),
        };
        event_outputs.push(PortSpec {
            name: intern(d.port),
            ty,
        });
    }
    let event_node = ctx.add_event(AddNodeOpts {
        gate_class: evt.gate_class,
        source_range: trigger_range,
        ports: GateIO {
            inputs: event_inputs,
            outputs: event_outputs,
        },
        properties: event_config_props(evt, &h.config, &ctx.const_lookup()),
        ..Default::default()
    });

    // Wire each input value into the gate's named input port. Lowered here (in
    // the enclosing scope, before the handler body scope is pushed) so top-level
    // `in` ports like `zoneA` resolve.
    for &(port, _ty, value_expr) in &input_wires {
        let src = lower_expr(ctx, value_expr);
        ctx.connect(src, port_ref(event_node, port));
    }

    let saved_exec = ctx.current_exec;
    let saved_entry = ctx.handler_entry_exec;
    ctx.current_exec = Some(port_ref(event_node, evt.exec_out));
    ctx.handler_entry_exec = Some(port_ref(event_node, evt.exec_out));
    reset_var_get_caches(ctx);

    // Typed event-data params (`on CustomEvent("x") -> (a: int) { ... a ... }`)
    // are bound INSIDE the closure — after `with_scope`'s own push — so they
    // live in the handler-body frame it pushes/pops, not the enclosing one.
    ctx.with_scope(
        ScopeKind::HandlerBody {
            trigger_label: trigger_name.clone(),
        },
        h.range.clone(),
        |ctx| {
            for (i, pname) in h.params.iter().enumerate() {
                if let Some(data) = evt.data.get(i) {
                    ctx.scope.insert(
                        &pname.name,
                        Binding::EventParam(port_ref(event_node, data.port)),
                    );
                }
            }
            lower_block(ctx, &h.body)
        },
    );

    let this_end = ctx.current_exec;
    ctx.current_exec = saved_exec;
    ctx.handler_entry_exec = saved_entry;
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
/// slot are ignored. Values are folded against `consts` (`ctx.const_lookup()`)
/// so a named constant (e.g. a computed `CustomEvent` channel) bakes here just
/// as it does on the `SendCustomEvent` sender side; a genuinely non-constant
/// value still folds to `None` and is ignored (typecheck has already rejected
/// it via `validate_handler_config`).
fn event_config_props(
    evt: &crate::catalog::events::EventSpec,
    config: &[HandlerConfigArg],
    consts: &ConstEnv,
) -> HashMap<crate::intern::Sym, Literal> {
    let mut props: HashMap<crate::intern::Sym, Literal> = HashMap::default();
    let mut positional = 0;
    for arg in config {
        let (field, value) = match arg {
            HandlerConfigArg::Positional(value) => {
                let field = evt.config_positional.get(positional).copied();
                positional += 1;
                (field, value)
            }
            HandlerConfigArg::Named { name, value } => {
                let field = evt
                    .config_named
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, f)| *f);
                (field, value)
            }
        };
        if let (Some(field), Some(lit)) = (field, expr_to_literal_in(value, consts)) {
            props.insert(intern(field), lit);
        }
    }
    props
}

pub(super) fn lower_block(ctx: &mut LowerCtx, block: &Block) {
    // Every caller is an imperative body; a top-level chip comes from the decl
    // loop instead and so never sets this.
    let saved_in_body = ctx.in_handler_body;
    ctx.in_handler_body = true;
    lower_block_inner(ctx, block);
    ctx.in_handler_body = saved_in_body;
}

fn lower_block_inner(ctx: &mut LowerCtx, block: &Block) {
    // Pre-declare vars inside stmt-level anon chips.
    for s in &block.stmts {
        if let Stmt::AnonChip(ac) = s {
            pre_declare_anon_chip(ctx, ac);
        }
    }
    for s in &block.stmts {
        let is_handler_stmt = matches!(s, Stmt::Handler(_) | Stmt::AnonChip(_));
        // Flushing overwrites `current_exec` with the nested handlers' ends,
        // which is only correct when there is no ambient chain (the top-level
        // case). Inside a handler/chip body with a live chain, a statement after
        // a nested independent-trigger `on` must stay on the outer chain, not be
        // re-parented onto the nested trigger.
        if !ctx.handler_end_execs.is_empty() && !is_handler_stmt && ctx.current_exec.is_none() {
            flush_handler_end_execs(ctx);
        }
        lower_stmt(ctx, s);
    }
    // A BUFFERED `emit` as the block's final statement terminates this exec
    // chain: a loop back-edge (`buffer emit loop` last in the then-block) crosses
    // the buffer barrier, so it must NOT fall through the if-join into the mod's
    // end exec, or the caller's continuation would fire once per iteration
    // instead of on `return`. An UNBUFFERED `emit sig` is a FORK, not a
    // terminator — it fires the signal AND the exec continues. Nulling the cursor
    // for it strands whatever follows, e.g. a mod whose body ends in `emit`,
    // inlined mid-chain, would drop the caller's following exec statements to an
    // `_Unsupported` placeholder.
    if let Some(Stmt::Emit(e)) = block.stmts.last() {
        if e.buffer.is_some()
            && (ctx.signal_key(&e.name).is_some() || ctx.lookup_output(&e.name).is_some())
        {
            ctx.current_exec = None;
        }
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

/// A pure, reactive top-level declaration — `let`/`out`/`var`/`buffer` — whose
/// value defines signal flow, as opposed to an imperative continuation
/// (`assign`/`if`/expr-statement) that legitimately runs after — and chains
/// from — a preceding handler's exit. A pure declaration must be lowered with a
/// cleared `current_exec` so its reads stay pure and never splice onto that
/// handler exec chain (which would, e.g., turn `let c = "${x}"` into an
/// exec-context `Exec_Var_Get` on the handler's spine, and — via `on Change(v)`'s
/// synthetic `let _on_expr = Change(v)` — misplace a trigger's own input read).
pub(super) fn is_pure_top_decl(d: &TopDecl) -> bool {
    matches!(
        d,
        TopDecl::Let(_) | TopDecl::Out(_) | TopDecl::Var(_) | TopDecl::Buffer(_)
    )
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
