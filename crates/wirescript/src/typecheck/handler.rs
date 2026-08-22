//! Handler binding: trigger params, output assignments, and event config
//! validation.

use super::*;
use crate::types::mono::unwrap_ref;

pub(super) fn bind_handler_trigger_params(ctx: &mut TypeCheckCtx, h: &Handler) {
    let (name, range) = match &h.trigger {
        Trigger::Ident { name, range } => (name, range),
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, range } => (name, range),
            _ => return,
        },
        _ => return,
    };
    {
        let evt = find_event(name);
        let sym = ctx.scope.lookup(name).cloned();
        let known_event = evt.is_some();
        let known_capture = matches!(&sym, Some(s) if s.kind == SymbolKind::Event);
        let known_input_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::In && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
        );
        let known_buffer_trigger = matches!(
            &sym,
            Some(s)
                if s.kind == SymbolKind::Buffer
                    && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Any)
        );
        let known_let_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::LetBinding
        );
        // A mod/chip param (`Param`) or an event data output bound by the
        // enclosing handler (`EventParam`, e.g. `on CustomEvent("x") -> (p:
        // character)`) can trigger a nested handler on its value/edge — `on p`
        // / `on !p`.
        let known_param_trigger = matches!(
            &sym,
            Some(s) if matches!(s.kind, SymbolKind::Param | SymbolKind::EventParam)
                && matches!(s.ty, Type::Exec | Type::Bool | Type::Int | Type::Float | Type::Character | Type::Controller | Type::Entity)
        );
        // A `var` can trigger a handler on its value change — `on x` / `on !x`.
        let known_var_trigger = matches!(
            &sym,
            Some(s) if s.kind == SymbolKind::Var
                && matches!(s.ty, Type::Bool | Type::Int | Type::Float | Type::Vector | Type::Character | Type::Controller | Type::Entity)
        );
        if !known_event
            && !known_capture
            && !known_input_trigger
            && !known_buffer_trigger
            && !known_let_trigger
            && !known_param_trigger
            && !known_var_trigger
        {
            ctx.emit(
                "WS001",
                format!("unknown event or trigger '{name}'"),
                range.clone(),
            );
        }
        // Event config args (`on Clock(enabled = ...)`) must be compile-time
        // constants: they bake into the event gate's data and have no wire pin.
        // Validated here — before the no-params early-return below, since a
        // config-only handler has no destructure params.
        if let Some(e) = evt {
            validate_handler_config(ctx, e, &h.config);
        }
        if h.params.is_empty() {
            return;
        }
        let Some(evt) = evt else {
            // Non-event trigger with `-> (…)`/`-> {…}` capture params — a
            // general mod/chip CALL trigger (`on pair(5, exec = go) -> (a, b)`).
            // Type each capture from the trigger call's OUTPUT RECORD instead of
            // `Any`, or the captured names read as `Any` and arithmetic on them
            // fails WS004. The record's single `Type::Exec` field is consumed by
            // `on` (it drives the body) and is EXCLUDED — the pattern binds the
            // remaining DATA fields.
            //
            // Record source: prefer the trigger binding's own type — the
            // synthesized `_on_expr_N` let binds the call's `Type::Record`
            // (declared before the handler, so it's in scope here). Only if that
            // isn't a record (auto-unwrapped to exec, or a folded `.field`
            // trigger) fall back to the trigger expr's recorded type; capture
            // params only ever come from `-> ` on a plain call, so the binding
            // source is the reliable one and the `type_of_expr` fallback avoids
            // the `.field`-trigger key overlap seen in lowering.
            let record_fields: Option<Vec<(String, Type)>> = match sym.as_ref().map(|s| &s.ty) {
                Some(Type::Record(fs)) => Some(fs.clone()),
                _ => {
                    let key = (range.file.clone(), range.start.offset, range.end.offset);
                    match ctx.type_of_expr.get(&key) {
                        Some(Type::Record(fs)) => Some(fs.clone()),
                        _ => None,
                    }
                }
            };
            let data_fields: Vec<(String, Type)> = record_fields
                .map(|fs| {
                    fs.into_iter()
                        .filter(|(_, t)| !matches!(t, Type::Exec))
                        .collect()
                })
                .unwrap_or_default();
            for (i, pname) in h.params.iter().enumerate() {
                // An explicit annotation wins; a record capture (`-> { f: a }`)
                // resolves by the original field name; a tuple capture
                // (`-> (a, b)`) is positional over the data fields; `Any` when
                // nothing resolves (a genuinely-untyped trigger).
                let ty = if let Some(te) = &pname.ty {
                    let t = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &t, type_expr_range(te));
                    t
                } else if let Some(field) = &pname.source_field {
                    data_fields
                        .iter()
                        .find(|(n, _)| n == field)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                } else {
                    data_fields
                        .get(i)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Type::Any)
                };
                ctx.scope.declare(
                    &pname.name,
                    SymbolInfo {
                        kind: SymbolKind::EventParam,
                        name: pname.name.clone(),
                        ty,
                        decl_range: h.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            return;
        };
        if evt.data.len() < h.params.len() {
            ctx.emit(
                "WS010",
                format!(
                    "destructure shape: expected {} param(s), got {}",
                    evt.data.len(),
                    h.params.len()
                ),
                h.range.clone(),
            );
        }
        for (i, pname) in h.params.iter().enumerate() {
            // A handler type annotation (`a: int` on Custom Event) overrides the
            // event's declared data type (which is `any` for such events).
            let ty = match &pname.ty {
                Some(te) => {
                    let t = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &t, type_expr_range(te));
                    t
                }
                None => {
                    // Custom Event's data outputs are untyped in the catalog, so an
                    // unannotated param has no declared type to fall back on. Look
                    // it up in `ce_slots` (`infer_custom_event_slots`, keyed by
                    // this handler's source range): pass 2 has it resolved
                    // from an in-unit sender (or defaulted to float with a WS042
                    // warning already emitted by the inference pass); pass 1 (and
                    // any non-custom-event handler) has nothing there yet, so fall
                    // back to the event's declared data type (Any for untyped events).
                    let inferred = CeNamespace::from_event_name(evt.surface_name)
                        .and_then(|ns| ctx.ce_slots.get(&ce_slot_key(ns, h)))
                        .and_then(|v| v.get(i).cloned().flatten());
                    match inferred {
                        Some(t) => t,
                        None => evt.data.get(i).map(|d| d.ty.clone()).unwrap_or(Type::Any),
                    }
                }
            };
            ctx.scope.declare(
                &pname.name,
                SymbolInfo {
                    kind: SymbolKind::EventParam,
                    name: pname.name.clone(),
                    ty,
                    decl_range: h.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
        }
        return;
    }

    // TrigField: TODO, treat params as Any if any.
    #[allow(unreachable_code)]
    for pname in &h.params {
        ctx.scope.declare(
            &pname.name,
            SymbolInfo {
                kind: SymbolKind::EventParam,
                name: pname.name.clone(),
                ty: Type::Any,
                decl_range: h.range.clone(),
                signature: None,
                event_data: None,
            },
        );
    }
}

/// Collect names assigned as outputs anywhere in a block: `out name = expr`
/// bindings, `emit name (= expr)`, and bare `name = expr` assignments (an
/// over-approximation — variable assigns land here too — but this set only
/// suppresses the WS013 unassigned-output warning). Recurses into if blocks,
/// `on` handlers, and anonymous chip blocks; nested named chips own their
/// outputs and are skipped.
pub(super) fn collect_output_assignments(block: &Block, assigned: &mut std::collections::HashSet<String>) {
    for s in &block.stmts {
        match s {
            Stmt::OutBinding(o) => {
                assigned.insert(o.name.clone());
            }
            Stmt::Emit(e) => {
                assigned.insert(e.name.clone());
            }
            Stmt::Assign(a) => {
                if let Expr::Ident { name, .. } = &a.target {
                    assigned.insert(name.clone());
                }
            }
            Stmt::If(i) => {
                collect_output_assignments(&i.then_block, assigned);
                if let Some(eb) = &i.else_block {
                    collect_output_assignments(eb, assigned);
                }
            }
            Stmt::Handler(h) => collect_output_assignments(&h.body, assigned),
            Stmt::AnonChip(ac) => collect_output_assignments(&ac.body, assigned),
            _ => {}
        }
    }
}

pub(super) fn block_has_return_value(block: &Block) -> bool {
    for s in &block.stmts {
        match s {
            Stmt::Return { value: Some(_), .. } => return true,
            Stmt::If(i) => {
                if block_has_return_value(&i.then_block) {
                    return true;
                }
                if let Some(eb) = &i.else_block
                    && block_has_return_value(eb)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Reject non-constant values in an event handler's constant-only config slots
/// (`on Clock(enabled = flag)`, `on ChatCommand(description = s)`). These
/// settings-menu fields bake into the event gate's data and have no wire pin, so
/// a variable/computed value is silently dropped at lowering (the pre-existing
/// `event_config_props` gap). Named args that WIRE into a gate input port
/// (`input_named`, e.g. Clock's `interval`) may be non-constant and are skipped.
fn validate_handler_config(
    ctx: &mut TypeCheckCtx,
    evt: &crate::catalog::events::EventSpec,
    config: &[crate::ast::HandlerConfigArg],
) {
    use crate::ast::HandlerConfigArg;
    // Built ONCE for the whole arg list — `const_lookup` deep-clones the
    // constant map, and nothing in this loop can change it.
    let consts = ctx.const_lookup();
    let mut positional = 0usize;
    for arg in config {
        match arg {
            HandlerConfigArg::Positional(value) => {
                let field = evt.config_positional.get(positional).copied();
                positional += 1;
                if let Some(field) = field {
                    check_event_config_value(ctx, field, value, &consts);
                }
            }
            HandlerConfigArg::Named { name, value } => {
                use crate::catalog::events::EventArgKind;
                match evt.classify_arg(name) {
                    // A named arg that wires into a gate input port may be dynamic.
                    EventArgKind::InputWire(..) => {}
                    EventArgKind::ConfigField(..) => {
                        check_event_config_value(ctx, name, value, &consts);
                    }
                    // Matches no input port and no config field: nothing lowers
                    // it (both typecheck and emit silently drop it), so `on
                    // Clock(intreval = 2.0)` quietly no-ops. Flag the typo — the
                    // handler analog of WS041 on calls.
                    EventArgKind::Unknown => {
                        ctx.emit(
                            "WS041",
                            format!(
                                "'{}' has no config or input '{name}'",
                                evt.surface_name
                            ),
                            value.range().clone(),
                        );
                    }
                }
            }
        }
    }
}

/// Type-check the values wired into an event's `input_named` ports
/// (`on ZoneEntered(zone = z) -> (character)` — `z` must be a `zone`; Clock's
/// `interval`/`enabled` must be float/bool). The value flows on a pure wire, so
/// it is inferred in pure context.
pub(super) fn check_handler_input_wires(
    ctx: &mut TypeCheckCtx,
    h: &Handler,
) {
    let name = match &h.trigger {
        Trigger::Ident { name, .. } => name,
        Trigger::Not { inner, .. } => match inner.as_ref() {
            Trigger::Ident { name, .. } => name,
            _ => return,
        },
        _ => return,
    };
    let Some(evt) = find_event(name) else { return };
    if evt.input_named.is_empty() {
        return;
    }
    for arg in &h.config {
        let HandlerConfigArg::Named { name: argname, value } = arg else {
            continue;
        };
        let Some((_, _, port_ty)) = evt
            .input_named
            .iter()
            .find(|(surf, _, _)| surf.eq_ignore_ascii_case(argname))
        else {
            continue;
        };
        let vty = unwrap_ref(&ctx.in_pure(|ctx| infer::infer(ctx, value)));
        if coerce(&vty, port_ty) == CoerceRule::Mismatch {
            ctx.emit(
                "WS003",
                format!(
                    "event input '{argname}': expected {}, got {}",
                    crate::analysis::types::type_str(port_ty),
                    crate::analysis::types::type_str(&vty),
                ),
                value.range().clone(),
            );
        }
    }
}

/// One constant-only event-config slot: the value must be constant AND scalar.
/// The scalar half mirrors the call-side
/// [`non_scalar_config_kind`](crate::typecheck::config::non_scalar_config_kind)
/// check — an event's config fields bake into the SAME kind of gate data field
/// a call's do, so a constant record/array/map is just as unwritable here, and
/// leaving it unchecked let one reach emit (silently wrong data for an array, a
/// compile-aborting `unreachable!` for a record).
fn check_event_config_value(
    ctx: &mut TypeCheckCtx,
    field: &str,
    e: &Expr,
    consts: &crate::lower::ConstEnv,
) {
    match crate::lower::expr_to_literal_in(e, consts) {
        Some(lit) => {
            if let Some(kind) = crate::typecheck::config::non_scalar_config_kind(&lit) {
                ctx.emit(
                    "WS028",
                    format!(
                        "'{field}' is constant-only event config and takes a single scalar value, not {kind}"
                    ),
                    e.range().clone(),
                );
            }
        }
        None => ctx.emit(
            "WS028",
            format!(
                "'{field}' is constant-only event config and cannot take a variable or computed value"
            ),
            e.range().clone(),
        ),
    }
}
