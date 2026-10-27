//! Resolve what a call's callee actually is, and route it.

use super::*;
use crate::typecheck::{ContainerKind, MethodTarget};

/// Whether `obj` already names a real array/map storage gate, in which case the
/// ordinary runtime lowering owns the call and no fold may run ahead of it.
fn receiver_has_runtime_container(ctx: &LowerCtx<'_>, obj: &Expr) -> bool {
    let Expr::Ident { name, .. } = obj else {
        // Any other receiver shape (a field, an element, a parameter) is
        // resolved by the paths below, which this fold must not pre-empt.
        return true;
    };
    match ctx.scope.get(name) {
        Some(Binding::Var(v)) => {
            matches!(v.storage, VarStorage::Array | VarStorage::Map)
                || v.inner_type.ref_kind().is_some()
        }
        Some(Binding::DynRef(_)) => true,
        _ => false,
    }
}

/// A container method call whose value is a compile-time constant, materialized
/// as a literal source gate. The method-call analogue of
/// `access::const_fold_index_access`, sharing its `wire_type_of_literal` +
/// `literal_node_range` pair so both agree on which literals have a wire form.
fn const_fold_container_call(
    ctx: &mut LowerCtx<'_>,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let lit = ctx.const_eval(e, ConstScope::Plain).ok()?;
    let ty = crate::lower::expr::wire_type_of_literal(&lit)?;
    Some(literal_node_range(ctx, range, ty, lit))
}

pub(in crate::lower) fn lower_call(ctx: &mut LowerCtx<'_>, e: &Expr) -> PortRef {
    let Expr::Call {
        callee,
        args,
        type_args,
        range,
    } = e
    else {
        return synthesise_unsupported(ctx, e);
    };
    if let Expr::Ident {
        name,
        range: name_range,
    } = callee.as_ref()
    {
        // User-defined chips/mods shadow builtins of the same name, so a program
        // can define e.g. `chip Toggle` without colliding with the builtin.
        if let Some(chip_decl) = ctx.call_target_chip(name, &name_range.file).cloned() {
            return lower_chip_call(ctx, &chip_decl, args, type_args, range);
        }
        if let Some(spec) = find_call(name) {
            return lower_builtin_call(ctx, spec, None, args, range, e);
        }
        // An event used as an expression (`Clock(interval = 2.0)`) emits the
        // event gate, wiring named args into its inputs and baking its config,
        // and yields its exec output; data outputs are read by field access
        // (`CharacterSpawned().character`). Mirrors the `on <event>` path.
        if let Some(evt) = crate::catalog::events::find_event(name) {
            // Named args wired into a gate input port (`interval`, `enabled`,
            // `zone` and so on); other literals bake config below.
            let mut input_wires: Vec<(&'static str, &Type, &Expr)> = Vec::new();
            for (surf, port, ty) in &evt.input_named {
                for arg in args {
                    if let CallArg::Named {
                        name: an, value, ..
                    } = arg
                        && an.eq_ignore_ascii_case(surf)
                    {
                        input_wires.push((port, ty, value));
                    }
                }
            }
            let inputs: Vec<PortSpec> = input_wires
                .iter()
                .map(|&(port, ty, _)| PortSpec::new(intern(port), ty.clone()))
                .collect();
            let mut outputs = vec![PortSpec::exec(intern(evt.exec_out))];
            for d2 in &evt.data {
                outputs.push(PortSpec::new(intern(d2.port), d2.ty.clone()));
            }
            let event_node = ctx.add_event(AddNodeOpts {
                gate_class: evt.gate_class,
                source_range: range.clone(),
                ports: GateIO { inputs, outputs },
                properties: event_config_props_from_call_args(evt, args, &ctx.const_lookup()),
                ..Default::default()
            });
            for &(port, _ty, value_expr) in &input_wires {
                let src = lower_expr(ctx, value_expr);
                ctx.connect(src, port_ref(event_node, port));
            }
            return port_ref(event_node, evt.exec_out);
        }
        // Neither a callable chip/mod nor a builtin. A chip/mod in scope that
        // the calling file cannot name is typecheck's WS002. One declared later
        // in the program is a use before declaration (chips/mods register in
        // source order), a hard error rather than a placeholder reading 0.
        if ctx.lookup_chip(name).is_some() {
            return synthesise_unsupported_range(ctx, &format!("the call `{name}(..)`"), range);
        }
        if ctx.known_fn_names.contains(name) {
            ctx.diagnostics.push(Diagnostic::error(
                "WS021",
                format!(
                    "call to undeclared function `{name}`: chips and mods must be \
                     declared before the point where they are used (move the \
                     declaration above its first caller)"
                ),
                range.clone(),
            ));
            return synthesise_unsupported_range(ctx, &format!("the call `{name}(..)`"), range);
        }
    }
    // Namespace calls: ns.foo(args)
    if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
        && let Expr::Ident { name: ns_name, .. } = obj.as_ref()
        && let Some(chip_decl) = ctx.lookup_ns_chip(ns_name, field).cloned()
    {
        return lower_chip_call(ctx, &chip_decl, args, type_args, range);
    }
    // Method calls: arr.push(val), arr.pop()
    if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
        // Test-only hook: checks that typecheck's recorded resolution and
        // lowering's re-derivation agree on the resolution key for nested and
        // generic bodies, which output-level tests cannot observe.
        #[cfg(test)]
        crate::lower::tests::resolution::probe_method_call(ctx, e);
        // A `const` array/map receiver gets its runtime form first, so the
        // shapes below resolve it like a `var` (a no-op for any other
        // receiver). Mutation is rejected on the resolved gate node by
        // `reject_const_container_mutation`, which also sees aliases.
        if crate::catalog::arrays::is_array_method(field)
            || crate::catalog::maps::is_map_method(field)
        {
            // A compile-time-known read on a receiver with no runtime
            // container binding must fold: typecheck exempts it from the
            // exec-context rule (`container_call_exec_exempt`), and the map
            // path in pure context would return the container ref as the value.
            // A runtime `var` shadowing the `const` still reads the var.
            if !receiver_has_runtime_container(ctx, obj)
                && let Some(port) = const_fold_container_call(ctx, e, range)
            {
                return port;
            }
            materialize_const_container(ctx, obj);
        }
        // `soa.field.sort(desc?)` on a record array sorts whole rows by that
        // field (every column of the root record array moves with the key),
        // checked before the column's own in-place sort. Typecheck's kind tells
        // a column from an array field of a record variable.
        if field == "sort"
            && let Expr::FieldAccess {
                obj: base,
                field: keyfield,
                ..
            } = obj.as_ref()
            && is_record_array_kind(ctx, base)
        {
            let mut root: &Expr = base;
            let mut path = vec![keyfield.clone()];
            while let Expr::FieldAccess {
                obj: parent,
                field: seg,
                ..
            } = root
                && is_record_array_kind(ctx, parent)
            {
                path.insert(0, seg.clone());
                root = parent;
            }
            if let Some(rec) = resolve_record_columns(ctx, Columns::Array, root)
                && let Some(port) = lower_record_array_field_sort(ctx, &rec, &path, args, range, e)
            {
                return port;
            }
        }
        match ctx.method_target(e) {
            Some(MethodTarget::Container { kind, method }) => {
                match container_ref(ctx, obj, &kind) {
                    Some(ContainerSource::Ref(port)) => {
                        return match kind {
                            ContainerKind::Array { elem } => {
                                lower_array_method(ctx, port, elem, &method, args, range, e)
                            }
                            ContainerKind::Map { .. } => lower_map_method(
                                ctx,
                                port,
                                &kind.as_type(),
                                &method,
                                args,
                                range,
                                e,
                            ),
                        };
                    }
                    Some(ContainerSource::Columns(fields)) => {
                        return match kind {
                            ContainerKind::Array { .. } => {
                                lower_record_array_method(ctx, &fields, &method, args, range, e)
                            }
                            ContainerKind::Map { .. } => {
                                lower_record_map_method(ctx, &fields, &method, args, range, e)
                            }
                        };
                    }
                    None => {}
                }
            }
            Some(MethodTarget::Builtin(name)) => {
                if let Some(spec) = find_call(&name) {
                    // A named-target receiver (`entity.SendCustomEvent(...)`) binds
                    // to a specific param (`target`), so it is appended as a named arg.
                    if let Some(tp) = spec.receiver_target_param() {
                        let mut recv_args: Vec<CallArg> = args.clone();
                        recv_args.push(CallArg::Named {
                            name: tp.to_string(),
                            name_range: obj.range().clone(),
                            value: obj.as_ref().clone(),
                        });
                        return lower_builtin_call(ctx, spec, None, &recv_args, range, e);
                    }
                    // The receiver fills the spec's first param.
                    return lower_builtin_call(ctx, spec, Some(obj), args, range, e);
                }
            }
            // `v.dist(o)` on a user mod/chip whose first parameter is `self`
            // desugars to `dist(v, o)`. A same-named builtin wins (WS035
            // rejects a self-mod shadowing one).
            Some(MethodTarget::SelfMod(name)) => {
                // Re-checked at the lowering scope, where a same-named
                // non-`self` chip may shadow the one typecheck resolved.
                if let Some(chip_decl) = ctx.self_mod_chip(&name, &range.file).cloned() {
                    let mut recv_args = Vec::with_capacity(args.len() + 1);
                    recv_args.push(CallArg::Positional(obj.as_ref().clone()));
                    recv_args.extend(args.iter().cloned());
                    return lower_chip_call(ctx, &chip_decl, &recv_args, type_args, range);
                }
            }
            None => {}
        }
        // An array/map method whose receiver resolved to no container binding
        // is reported, not left as a silent `_Unsupported` gate.
        if crate::catalog::arrays::is_array_method(field)
            || crate::catalog::maps::is_map_method(field)
        {
            report_unresolved_container(
                ctx,
                &format!("`.{field}()` is an array/map method"),
                range,
            );
        }
    }
    synthesise_unsupported(ctx, e)
}

/// Bake an event's config args (`ChatCommand("greet", Description = "...")`)
/// into its gate properties, for the event-as-expression form. Positional
/// literals fill `config_positional` in order; a named arg targets a
/// `config_named` field (case-insensitive). Other args are ignored. Mirrors
/// `handler::event_config_props`.
fn event_config_props_from_call_args(
    evt: &crate::catalog::events::EventSpec,
    args: &[CallArg],
    consts: &crate::const_scope::ConstNames,
) -> HashMap<Sym, Literal> {
    let mut props: HashMap<Sym, Literal> = HashMap::default();
    let mut positional = 0;
    for arg in args {
        let (field, value) = match arg {
            CallArg::Positional(value) => {
                let field = evt.config_positional.get(positional).copied();
                positional += 1;
                (field, value)
            }
            CallArg::Named { name, value, .. } => {
                let field = evt
                    .config_named
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, f)| *f);
                (field, value)
            }
            CallArg::Spread(_) => continue,
        };
        if let (Some(field), Some(lit)) = (field, expr_to_literal_in(value, consts)) {
            props.insert(intern(field), lit);
        }
    }
    props
}

/// Whether typecheck resolved `e` as an array of records.
fn is_record_array_kind(ctx: &LowerCtx<'_>, e: &Expr) -> bool {
    matches!(
        ctx.container_kind(e),
        Some(ContainerKind::Array {
            elem: Type::Record(_)
        })
    )
}
