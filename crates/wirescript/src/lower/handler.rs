use super::*;
use crate::typecheck::{CeNamespace, ce_slot_key};

pub(super) fn lower_event_decl(ctx: &mut LowerCtx<'_>, d: &EventDecl) {
    // A captured body runs as the `on` handler of its trigger, config and
    // output captures included, and its exit is the event. An alias has no
    // body: each `on` of it is that trigger, written there.
    let Some(h) = &d.captured else {
        return;
    };
    let before = ctx.handler_end_execs.len();
    lower_handler(ctx, h);
    let ends: Vec<PortRef> = ctx.handler_end_execs.drain(before..).collect();
    let end = ends
        .into_iter()
        .fold(None, |acc, e| merge_exec(ctx, acc, Some(e), &d.range));
    if let Some(e) = end {
        ctx.captured_events.insert(d.name.clone(), e);
    }
}

/// `on !x`: route the trigger through a `LogicalNOT` so the handler fires when
/// the source goes false. The identity when the trigger was not negated.
fn negate_trigger(ctx: &mut LowerCtx<'_>, port: PortRef, negated: bool) -> PortRef {
    if !negated {
        return port;
    }
    let not_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LOGICAL_NOT,
        ports: GateIO::bool_not(),
        ..Default::default()
    });
    ctx.connect(port, not_id.port(WirePort::BInput));
    not_id.port(WirePort::BOutput)
}

/// How a handler's trigger reads in the emitted scope label: `name`, or
/// `name.field` for a field trigger.
fn trigger_label(name: &str, field: Option<&str>) -> String {
    match field {
        Some(f) => format!("{name}.{f}"),
        None => name.to_string(),
    }
}

/// The slice of [`LowerCtx`] a handler body may clobber, taken at the top of
/// [`lower_handler`]. Every exit must restore it: `handler_ends` holds the exits
/// of the preceding handlers, and dropping it deletes the next statement (WS058).
struct HandlerSave {
    chain: Option<u32>,
    handler_ends: Vec<PortRef>,
}

impl HandlerSave {
    /// Take the state, and open a fresh chain for the handler about to be
    /// lowered.
    fn take(ctx: &mut LowerCtx<'_>) -> Self {
        let chain = ctx.builder.current_chain_id;
        let fresh = ctx.alloc_chain();
        ctx.builder.current_chain_id = Some(fresh);
        // Taken so an inner block does not flush the outer handler ends.
        let handler_ends = std::mem::take(&mut ctx.handler_end_execs);
        Self {
            chain,
            handler_ends,
        }
    }

    /// Give up on the handler without lowering its body.
    fn abandon(self, ctx: &mut LowerCtx<'_>) {
        ctx.builder.current_chain_id = self.chain;
        ctx.handler_end_execs = self.handler_ends;
    }

    /// Lower a handler body with `trig` as its entry exec, restore, and hand
    /// this handler's own exit exec to whatever the enclosing block chains next.
    fn lower_body<R>(
        self,
        ctx: &mut LowerCtx<'_>,
        h: &Handler,
        trig: PortRef,
        trigger_label: String,
        body: impl FnOnce(&mut LowerCtx<'_>) -> R,
    ) {
        let saved = (ctx.current_exec, ctx.handler_entry_exec);
        let saved_closed = std::mem::take(&mut ctx.exec_closed_by_buffer_emit);
        // A `return` in this body ends the handler's own run, not an enclosing
        // mod or chip call, so its exec does not join that call's continuation.
        let saved_return_exec = ctx.mod_return_exec.take();
        ctx.current_exec = Some(trig);
        ctx.handler_entry_exec = Some(trig);
        reset_var_get_caches(ctx);
        ctx.with_scope(
            ScopeKind::HandlerBody { trigger_label },
            h.range.clone(),
            body,
        );
        let this_end = ctx.current_exec;
        ctx.current_exec = saved.0;
        ctx.handler_entry_exec = saved.1;
        ctx.exec_closed_by_buffer_emit = saved_closed;
        ctx.mod_return_exec = saved_return_exec;
        self.abandon(ctx);
        if let Some(e) = this_end {
            ctx.handler_end_execs.push(e);
        }
    }
}

/// What a handler's trigger resolved to before any event gate is built.
enum SimpleTrigger {
    /// The body fires on `port`, shown as `label` in the scope.
    Fires { port: PortRef, label: String },
    /// The trigger names something that cannot fire; the handler is dropped.
    Unfireable,
    /// Not a port-backed trigger: the name is a built-in event or nothing.
    NotSimple,
}

/// How an `on <call>` trigger resolved.
enum CallTrigger {
    /// Not the exec form of a call trigger.
    NotCall,
    /// The call exposes no exec output to fire on.
    NoExec,
    /// Fires on `trig`; `fields` is the call's result record and `data_fields`
    /// the names of its non-exec fields, in order.
    Exec {
        trig: PortRef,
        fields: Fields,
        data_fields: Vec<String>,
    },
}

/// Resolve a call trigger `on <call> [-> <pattern>] { ... }`, which the parser
/// desugars into a synthetic `_on_expr_N` trigger plus `let _on_expr_N = <call>`.
/// It fires on the call's one `Type::Exec` result field.
///
/// A call with no bindable exec output (an inline `mod` exposes none) is
/// `NoExec`: falling through to the value-change branch would wire a value
/// output as the exec trigger. Value-change forms (`on ServerUptime()`) are
/// `NotCall`.
fn resolve_call_trigger(
    ctx: &mut LowerCtx<'_>,
    h: &Handler,
    trigger_name: &str,
    field: Option<&str>,
    negated: bool,
) -> CallTrigger {
    let Trigger::Ident {
        name,
        range: trig_range,
    } = &h.trigger
    else {
        return CallTrigger::NotCall;
    };
    if field.is_some() || negated || !name.starts_with("_on_expr_") {
        return CallTrigger::NotCall;
    }
    // The exec form applies when `_on_expr_N` is a `Binding::Record`, has
    // `-> <pattern>` params, or passed `exec = <x>`; otherwise the value-change
    // branch takes it. The result type cannot decide this, since a
    // `.field` trigger's range can carry a nested expr's record type.
    let is_record_binding = matches!(ctx.scope.get(trigger_name), Some(Binding::Record(_)));
    if !(is_record_binding || !h.params.is_empty() || h.expr_trigger_has_exec_arg) {
        return CallTrigger::NotCall;
    }
    // The call's typechecked result shape + its structural exec field
    // name (typecheck records the record type at the call's own range;
    // reliable here because a record-binding trigger is a plain call
    // expr, not a `.field` access whose key can overlap a nested expr).
    let Some(Type::Record(shape)) = ctx.type_at(trig_range) else {
        return CallTrigger::NoExec;
    };
    let Some(exec_name) = shape
        .iter()
        .find(|(_, t)| matches!(t, Type::Exec))
        .map(|(n, _)| n.clone())
    else {
        return CallTrigger::NoExec;
    };
    let Some(Binding::Record(fields)) = ctx.scope.get(trigger_name).cloned() else {
        return CallTrigger::NoExec;
    };
    let Some(exec_binding) = fields.get(&intern(&exec_name)).cloned() else {
        return CallTrigger::NoExec;
    };
    let Some(trig) = binding_to_port(ctx, &exec_binding, &h.range) else {
        return CallTrigger::NoExec;
    };
    let data_fields = shape
        .iter()
        .filter(|(n, _)| n != &exec_name)
        .map(|(n, _)| n.clone())
        .collect();
    CallTrigger::Exec {
        trig,
        fields,
        data_fields,
    }
}

/// Resolve a trigger that names a port already in scope: a record or namespace
/// member, a `var.value` field, a captured event, an `in` port, a buffer, a
/// local, a `var`, or an event data param. A `!` adds a NOT where the trigger
/// is a value that can go false.
fn resolve_simple_trigger(
    ctx: &mut LowerCtx<'_>,
    h: &Handler,
    trigger_name: &str,
    trigger_field: Option<&str>,
    negated: bool,
) -> SimpleTrigger {
    let fires = |port: PortRef, label: String| SimpleTrigger::Fires { port, label };
    let field_label = |field: &str| format!("{trigger_name}.{field}");

    // Record-binding field trigger: chip-call results (`on r.exec`,
    // `on r.someExecOutput`) resolve through the result record.
    if let Some(field) = trigger_field {
        let rec_binding = match ctx.scope.get(trigger_name) {
            Some(Binding::Record(fields_map)) => fields_map.get(&intern(field)).cloned(),
            // `on ns.trigger` where `ns` came from `import * as ns`: resolve the
            // member through that namespace. An imported `in` port binds as
            // `Binding::Input`, which `binding_to_port` turns into its `RerOutput`.
            Some(Binding::Namespace(members)) => members.get(field).cloned(),
            _ => None,
        };
        if let Some(binding) = rec_binding
            && let Some(trig) = binding_to_port(ctx, &binding, &h.range)
        {
            return fires(trig, field_label(field));
        }
    }

    // `var.value` / `var.prev`: both pseudo-fields read the storage gate's
    // `Value` port.
    if let Some(field) = trigger_field
        && let Some(rec) = ctx.lookup_var(trigger_name).cloned()
    {
        if !crate::catalog::is_var_pseudo_field(field) {
            return SimpleTrigger::Unfireable;
        }
        return fires(port_ref(rec.node_id, "Value"), field_label(field));
    }

    if let Some(cap) = ctx.captured_events.get(trigger_name).copied() {
        return fires(cap, trigger_name.to_string());
    }

    if let Some(rec) = ctx.lookup_input(trigger_name).cloned() {
        let trig = negate_trigger(ctx, rec.node_id.port(WirePort::RerOutput), negated);
        return fires(trig, trigger_name.to_string());
    }

    if let Some(rec) = ctx.lookup_buffer(trigger_name).cloned() {
        return fires(rec.node_id.port(WirePort::Output), trigger_name.to_string());
    }

    // A local (let binding) fires on value change. A field trigger
    // (`on split.Jump`) selects the named output port on the local's gate
    // (e.g. InputReader's `bPressedJump`) instead of its default port.
    if let Some(rec) = ctx.lookup_local(trigger_name).cloned() {
        let base_port = match trigger_field {
            Some(field) => {
                resolve_output_field_port(ctx, rec.port.node_id, field).unwrap_or(rec.port)
            }
            None => rec.port,
        };
        let port = negate_trigger(ctx, base_port, negated);
        return fires(port, trigger_label(trigger_name, trigger_field));
    }

    // A `var` (`on x` / `on !x`) fires on its value change; the var gate's
    // `Value` output is the trigger port.
    if trigger_field.is_none()
        && let Some(rec) = ctx.lookup_var(trigger_name).cloned()
    {
        let port = negate_trigger(ctx, port_ref(rec.node_id, "Value"), negated);
        return fires(port, trigger_name.to_string());
    }

    // An enclosing handler's event data param (`on p` / `on !p` for the `p` of
    // `on CustomEvent("x") -> (p: character)`) fires on the param's value edge.
    if let Some(Binding::EventParam(ep_port)) = ctx.scope.get(trigger_name).cloned() {
        let base_port = match trigger_field {
            Some(field) => {
                resolve_output_field_port(ctx, ep_port.node_id, field).unwrap_or(ep_port)
            }
            None => ep_port,
        };
        let port = negate_trigger(ctx, base_port, negated);
        return fires(port, trigger_label(trigger_name, trigger_field));
    }

    SimpleTrigger::NotSimple
}

/// Lower a handler whose trigger is a built-in event: build the event gate with
/// its config and input wires, then lower the body with the data params bound.
fn lower_event_handler(ctx: &mut LowerCtx<'_>, h: &Handler, save: HandlerSave, trigger_name: &str) {
    let Some(evt) = find_event(trigger_name) else {
        save.abandon(ctx);
        // No port and no built-in event, so the body has nothing to fire on.
        // Typecheck admits any `let` as a trigger, so a record-, tuple- or
        // multi-output-valued one lands here and needs this error.
        let at = match &h.trigger {
            Trigger::Not { inner, .. } => inner.as_ref(),
            other => other,
        };
        let at = match at {
            Trigger::Ident { range, .. } | Trigger::Field { range, .. } => range,
            _ => &h.range,
        };
        ctx.error(
            "WS001",
            format!(
                "'{trigger_name}' is not an event and has no exec or value port to \
                     trigger on, so this handler would never run"
            ),
            at,
        );
        return;
    };

    let trigger_range = match &h.trigger {
        Trigger::Ident { range, .. } => range.clone(),
        _ => SourceRange::default(),
    };

    // Named args that wire a value into a gate INPUT port (e.g. `zone = zoneA`).
    let mut input_wires: Vec<(&'static str, &Type, &Expr)> = Vec::new();
    for (surf, port, ty) in &evt.input_named {
        for arg in &h.config {
            if let HandlerConfigArg::Named { name, value } = arg
                && name.eq_ignore_ascii_case(surf)
            {
                input_wires.push((port, ty, value));
            }
        }
    }
    let event_inputs: Vec<PortSpec> = input_wires
        .iter()
        .map(|&(port, ty, _)| PortSpec::new(intern(port), ty.clone()))
        .collect();

    let mut event_outputs = vec![PortSpec::exec(intern(evt.exec_out))];
    // Custom Event types its data outputs from the handler's annotations
    // (`-> (a: int, b: float)`); an unannotated param takes the type typecheck
    // inferred from an in-unit sender, else float. Other events keep their
    // catalog data types.
    let is_custom_event = evt.gate_class == crate::ir::gate_class::CUSTOM_EVENT;
    // This handler's inferred slot types when it is a custom-event receiver
    // (see `crate::typecheck::infer_custom_event_slots`).
    let inferred = CeNamespace::from_event_name(evt.surface_name)
        .and_then(|ns| ctx.ce_slots.get(&ce_slot_key(ns, h)));
    let inferred_slot = |i: usize| inferred.and_then(|v| v.get(i).cloned().flatten());
    for (i, d) in evt.data.iter().enumerate() {
        let ty = match h.params.get(i) {
            Some(p) => match &p.ty {
                Some(te) => ctx.resolve_local_type(te),
                // Unannotated: a custom-event receiver takes the inferred type;
                // any other event (`inferred` is `None`) keeps its catalog type,
                // so `on CharacterSpawned() -> (character)` stays `Character`.
                None => inferred_slot(i).unwrap_or_else(|| d.ty.clone()),
            },
            None if is_custom_event => Type::Float,
            None => d.ty.clone(),
        };
        event_outputs.push(PortSpec::new(intern(d.port), ty));
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

    // Event-data params are bound inside the closure, after `with_scope` pushes
    // the handler-body frame, so they live in that frame and not the enclosing one.
    let trig = port_ref(event_node, evt.exec_out);
    save.lower_body(ctx, h, trig, trigger_name.to_string(), |ctx| {
        for (i, pname) in h.params.iter().enumerate() {
            if let Some(data) = evt.data.get(i) {
                ctx.scope.insert(
                    &pname.name,
                    Binding::EventParam(port_ref(event_node, data.port)),
                );
            }
        }
        lower_block(ctx, &h.body);
    });
}

pub(super) fn lower_handler(ctx: &mut LowerCtx<'_>, h: &Handler) {
    // Union trigger `on a | b { ... }`: run the body when ANY part fires. The
    // body is lowered once per part, each through the full single-trigger
    // resolution below, so every trigger kind works as a part.
    if let Trigger::Union { parts, .. } = &h.trigger {
        for part in parts {
            let mut single = h.clone();
            single.trigger = part.clone();
            lower_handler(ctx, &single);
        }
        return;
    }
    // Split the trigger into its name, optional field (`on var.value`) and
    // whether it is negated (`on !foo` lowers `foo` and adds a NOT).
    let (trigger_name, trigger_field, negated) = match &h.trigger {
        Trigger::Ident { name, .. } => (name.clone(), None, false),
        Trigger::Field { obj, field, .. } => (obj.clone(), Some(field.clone()), false),
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, .. } => (name.clone(), None, true),
            Trigger::Field { obj, field, .. } => (obj.clone(), Some(field.clone()), true),
            _ => return,
        },
        Trigger::Union { .. } => return,
    };

    let save = HandlerSave::take(ctx);
    let field = trigger_field.as_deref();

    match resolve_call_trigger(ctx, h, &trigger_name, field, negated) {
        CallTrigger::NotCall => {}
        CallTrigger::NoExec => {
            ctx.diagnostics.push(Diagnostic::error(
                "WS043",
                "this `on <call>` trigger's call exposes no exec output to trigger on - \
                 `on` fires on the call's completion exec (bind its data outputs with \
                 `-> ...`, drive it with `exec = ...`). Use an event, or a named `chip` \
                 or `fn` call; an inline `mod` does not yet surface a completion exec, so an \
                 `exec = ...` on it has nothing to attach to"
                    .to_string(),
                h.range.clone(),
            ));
            save.abandon(ctx);
            return;
        }
        CallTrigger::Exec {
            trig,
            fields,
            data_fields,
        } => {
            save.lower_body(ctx, h, trig, trigger_name.clone(), |ctx| {
                for (i, pname) in h.params.iter().enumerate() {
                    let field_name: Option<&str> = match &pname.source_field {
                        Some(f) => Some(f.as_str()),
                        None => data_fields.get(i).map(String::as_str),
                    };
                    let Some(fname) = field_name else { continue };
                    let Some(binding) = fields.get(&intern(fname)).cloned() else {
                        continue;
                    };
                    if let Some(port) = binding_to_port(ctx, &binding, &pname.range) {
                        ctx.scope.insert(&pname.name, Binding::EventParam(port));
                    }
                }
                lower_block(ctx, &h.body);
            });
            return;
        }
    }

    match resolve_simple_trigger(ctx, h, &trigger_name, field, negated) {
        SimpleTrigger::Fires { port, label } => {
            save.lower_body(ctx, h, port, label, |ctx| lower_block(ctx, &h.body));
        }
        SimpleTrigger::Unfireable => save.abandon(ctx),
        SimpleTrigger::NotSimple => lower_event_handler(ctx, h, save, &trigger_name),
    }
}

/// Resolve an event handler's config args (e.g. `on ChatCommand("greet",
/// Description = "Greets you")`) into the event gate's data-struct properties.
/// Positional args fill `evt.config_positional` in order; named args match
/// `evt.config_named` case-insensitively; unmatched args are ignored. Values fold
/// against `consts`, and a non-constant one is ignored (typecheck rejects it).
fn event_config_props(
    evt: &crate::catalog::events::EventSpec,
    config: &[HandlerConfigArg],
    consts: &crate::const_scope::ConstNames,
) -> HashMap<Sym, Literal> {
    let mut props: HashMap<Sym, Literal> = HashMap::default();
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

pub(super) fn lower_block(ctx: &mut LowerCtx<'_>, block: &Block) {
    // Every caller is an imperative body; a top-level chip comes from the decl
    // loop instead and so never sets this.
    let saved_in_body = ctx.in_handler_body;
    ctx.in_handler_body = true;
    lower_block_inner(ctx, block);
    ctx.in_handler_body = saved_in_body;
}

fn lower_block_inner(ctx: &mut LowerCtx<'_>, block: &Block) {
    // Pre-declare vars inside stmt-level anon chips.
    for s in &block.stmts {
        if let Stmt::AnonChip(ac) = s {
            pre_declare_anon_chip(ctx, ac, false);
        }
    }
    // A `break` or `continue` ends this block's exec chain: nothing after it
    // in the same block ever runs, so each such statement gets one WS081 in
    // place of its own WS058. Holds the keyword that ended it.
    let mut dead_after: Option<&str> = None;
    // The chain a `buffer emit` in this block closed: an `await` right after
    // it still arms on it.
    let mut closed_chain: Option<PortRef> = None;
    for s in &block.stmts {
        if ctx.current_exec.is_some() {
            ctx.exec_closed_by_buffer_emit = false;
            closed_chain = None;
        }
        if ctx.current_exec.is_none() && ctx.exec_closed_by_buffer_emit {
            if matches!(s, Stmt::Await(_)) && closed_chain.is_some() {
                ctx.current_exec = closed_chain.take();
            } else if runs_on_chain(s) {
                ctx.warn_code(
                    "WS058",
                    "this statement never runs: a `buffer emit` before it (or in a mod it \
                     calls) closed the exec chain, and nothing after it on that chain runs. \
                     Move the statement before that emit.",
                    s.range(),
                );
                continue;
            }
        }
        let is_handler_stmt = matches!(s, Stmt::Handler(_) | Stmt::AnonChip(_));
        // Flushing overwrites `current_exec` with the nested handlers' ends,
        // which is only correct when there is no ambient chain (the top-level
        // case). Inside a handler/chip body with a live chain, a statement after
        // a nested independent-trigger `on` must stay on the outer chain, not be
        // re-parented onto the nested trigger.
        if !ctx.handler_end_execs.is_empty() && !is_handler_stmt && ctx.current_exec.is_none() {
            flush_handler_end_execs(ctx);
        }
        if let Some(kw) = dead_after {
            ctx.warn_code(
                "WS081",
                format!(
                    "this statement never runs: a `{kw}` earlier in the same body already \
                     ended the loop's exec chain, so nothing after it in this block executes."
                ),
                s.range(),
            );
            let saved = ctx.dead_after_break;
            ctx.dead_after_break = true;
            lower_stmt(ctx, s);
            ctx.dead_after_break = saved;
        } else {
            lower_stmt(ctx, s);
        }
        match s {
            Stmt::Break { .. } => dead_after = Some("break"),
            Stmt::Continue { .. } => dead_after = Some("continue"),
            _ => {}
        }
        if closes_chain(ctx, s) && ctx.current_exec.is_some() {
            closed_chain = ctx.current_exec.take();
            ctx.exec_closed_by_buffer_emit = true;
        }
    }
    close_on_trailing_buffered_emit(ctx, block);
}

/// Whether `s` only does something when the exec chain it stands on runs: a
/// bare `emit`, an `if`, `if let`, `for` or expression statement, unless the
/// statement's call names its own trigger (`exec = ...`).
fn runs_on_chain(s: &Stmt) -> bool {
    match s {
        Stmt::Emit(e) => e.value.is_none(),
        Stmt::If(_) | Stmt::IfLet(_) | Stmt::For(_) => true,
        Stmt::ExprStmt(x) => !matches!(&x.expr, Expr::Call { args, .. }
            if args.has_exec_arg()),
        _ => false,
    }
}

/// Whether `s` is a `buffer emit` of a signal or output, which ends the exec
/// chain it stands on: the emit crosses the buffer and continues from there,
/// and nothing after it on the chain runs.
fn closes_chain(ctx: &LowerCtx<'_>, s: &Stmt) -> bool {
    matches!(s, Stmt::Emit(e) if e.buffer.is_some()
        && (ctx.signal_key(&e.name).is_some() || ctx.lookup_output(&e.name).is_some()))
}

/// A buffered `emit` as a block's last statement ends its exec chain, so a loop
/// back-edge does not fall through the if-join and fire the caller's continuation
/// every iteration. An unbuffered `emit` forks and the chain continues.
pub(super) fn close_on_trailing_buffered_emit(ctx: &mut LowerCtx<'_>, block: &Block) {
    if let Some(Stmt::Emit(e)) = block.stmts.last()
        && e.buffer.is_some()
        && (ctx.signal_key(&e.name).is_some() || ctx.lookup_output(&e.name).is_some())
    {
        ctx.current_exec = None;
        ctx.exec_closed_by_buffer_emit = true;
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

/// A pure top-level declaration (`let`/`out`/`var`/`buffer`), as opposed to an
/// imperative statement that chains from the preceding handler's exit. It is
/// lowered with `current_exec` cleared, or its reads become `Exec_Var_Get`s on that chain.
pub(super) fn is_pure_top_decl(d: &TopDecl) -> bool {
    matches!(
        d,
        TopDecl::Let(_) | TopDecl::Out(_) | TopDecl::Var(_) | TopDecl::Buffer(_)
    )
}

/// Union all accumulated handler end execs into a single Union gate,
/// setting `current_exec` so subsequent code chains from every handler's exit.
pub(super) fn flush_handler_end_execs(ctx: &mut LowerCtx<'_>) {
    let ends = std::mem::take(&mut ctx.handler_end_execs);
    if ends.is_empty() {
        return;
    }
    if ends.len() == 1 {
        ctx.current_exec = Some(ends.into_iter().next().unwrap());
        reset_var_get_caches(ctx);
        return;
    }
    // Chain Union gates pairwise: Union(a, b) -> Union(prev, c) -> ...
    let mut iter = ends.into_iter();
    let first = iter.next().unwrap();
    let second = iter.next().unwrap();
    let no_range = SourceRange::default();
    let mut prev_out = exec_join(ctx, Some(first), Some(second), &no_range);
    for end in iter {
        prev_out = exec_join(ctx, Some(prev_out), Some(end), &no_range);
    }
    ctx.current_exec = Some(prev_out);
    reset_var_get_caches(ctx);
}
