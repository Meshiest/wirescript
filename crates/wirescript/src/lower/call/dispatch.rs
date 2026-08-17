//! Resolve what a call's callee actually is, and route it.

use super::*;

pub(in crate::lower) fn lower_call(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    let (callee, args, type_args, range) = match e {
        Expr::Call {
            callee,
            args,
            type_args,
            range,
        } => (callee, args, type_args, range),
        _ => return synthesise_unsupported(ctx, e),
    };
    if let Expr::Ident { name, .. } = callee.as_ref() {
        // User-defined chips/mods shadow builtins of the same name, so a program
        // can define e.g. `chip Toggle` without colliding with the builtin.
        if let Some(chip_decl) = ctx.lookup_chip(name).cloned() {
            return lower_chip_call(ctx, &chip_decl, args, type_args, range);
        }
        if let Some(spec) = find_call(name) {
            return lower_builtin_call(ctx, spec, None, args, range, e);
        }
        // An event used as an expression (`RoundStart()`, `Clock(interval = 2.0)`,
        // `CharacterSpawned()`): emit the event gate — wiring named args into its
        // input ports and baking its config — and yield its exec output (data
        // outputs are reachable by field access on the returned node, e.g.
        // `CharacterSpawned().character`). Mirrors the `on <event>` handler path.
        if let Some(evt) = crate::catalog::events::find_event(name) {
            // Named args wired into a gate INPUT port (`interval`, `enabled`,
            // `zone`, …); positional/named literals bake config below.
            let mut input_wires: Vec<(&'static str, &Type, &Expr)> = Vec::new();
            for (surf, port, ty) in &evt.input_named {
                for arg in args {
                    if let CallArg::Named { name: an, value, .. } = arg
                        && an.eq_ignore_ascii_case(surf)
                    {
                        input_wires.push((port, ty, value));
                    }
                }
            }
            let inputs: Vec<PortSpec> = input_wires
                .iter()
                .map(|&(port, ty, _)| PortSpec {
                    name: intern(port),
                    ty: ty.clone(),
                })
                .collect();
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
                source_range: range.clone(),
                ports: GateIO { inputs, outputs },
                properties: event_config_props_from_call_args(evt, args),
                ..Default::default()
            });
            for &(port, _ty, value_expr) in &input_wires {
                let src = lower_expr(ctx, value_expr);
                ctx.connect(src, port_ref(event_node, port));
            }
            return port_ref(event_node, evt.exec_out);
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
        return lower_chip_call(ctx, &chip_decl, args, type_args, range);
    }
    // Method calls: arr.push(val), arr.pop()
    if let Expr::FieldAccess { obj, field, .. } = callee.as_ref() {
        // A `const` array/map used as a method receiver: give it its runtime
        // form first, so the receiver shapes below resolve it exactly like a
        // `var` instead of falling through to the WS044 backstop. A no-op for
        // every other receiver (and for a name that already has a binding), so
        // the resolution order below is unchanged.
        //
        // MUTATION is not filtered here — `lower_array_method` /
        // `lower_map_method` reject it on the resolved gate node (see
        // `reject_const_container_mutation`), which also covers the aliased
        // spellings this site can't see, such as the container arriving through
        // a `ys: T[]` parameter.
        if crate::catalog::arrays::is_array_method(field)
            || crate::catalog::maps::is_map_method(field)
        {
            materialize_const_container(ctx, obj);
        }
        if let Expr::Ident { name, .. } = obj.as_ref()
            && crate::catalog::arrays::is_array_method(field)
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
        // Map method: m.get(k), m.set(k, v), m.has(k), etc.
        if let Expr::Ident { name, .. } = obj.as_ref()
            && crate::catalog::maps::is_map_method(field)
            && let Some(var_rec) = ctx.lookup_var(name).cloned()
            && var_rec.storage == VarStorage::Map
        {
            return lower_map_method(
                ctx,
                var_rec.node_id.port(WirePort::MapVarRef),
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
        // Map method on an `in X: Map<K,V>` input — mirrors the array-input case
        // (the map ref rides the input's RER_Output).
        if let Expr::Ident { name, .. } = obj.as_ref()
            && crate::catalog::maps::is_map_method(field)
            && let Some(Binding::Input(inp)) = ctx.scope.get(name).cloned()
            && matches!(inp.ty, Type::Map(_, _))
        {
            return lower_map_method(
                ctx,
                inp.node_id.port(WirePort::RerOutput),
                inp.ty.clone(),
                field,
                args,
                range,
                e,
            );
        }
        // Record-resolved var methods: cpu.regs.push(val)
        if crate::catalog::arrays::is_array_method(field)
            && let Some(Binding::Var(var_rec)) = resolve_field_chain(ctx, obj).cloned()
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
        // Record-resolved var map methods: b.counts.set(k, v)
        if crate::catalog::maps::is_map_method(field)
            && let Some(Binding::Var(var_rec)) = resolve_field_chain(ctx, obj).cloned()
            && var_rec.storage == VarStorage::Map
        {
            return lower_map_method(
                ctx,
                var_rec.node_id.port(WirePort::MapVarRef),
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
            // A named-target receiver (`entity.SendCustomEvent(…)`) binds the
            // object to a specific param (`target`) rather than the first, so
            // append it as a named arg and let the normal binding path place it.
            if let Some(tp) = spec.receiver_target_param() {
                let mut recv_args: Vec<CallArg> = args.to_vec();
                recv_args.push(CallArg::Named {
                    name: tp.to_string(),
                    name_range: obj.range().clone(),
                    value: obj.as_ref().clone(),
                });
                return lower_builtin_call(ctx, spec, None, &recv_args, range, e);
            }
            // The receiver fills the spec's first param; passing it separately
            // avoids deep-cloning the receiver + args into a new arg vector.
            return lower_builtin_call(ctx, spec, Some(obj), args, range, e);
        }
        // User `self`-receiver method calls: `v.dist(o)` where `dist` is a user
        // mod/chip whose first parameter is named `self`. Desugars to
        // `dist(v, o)` by prepending the receiver as positional arg 0. Placed
        // AFTER the builtin-receiver case so a builtin of the same name wins
        // (typecheck's WS035 rejects a self-mod shadowing a builtin).
        if let Some(chip_decl) = ctx.lookup_chip(field).cloned()
            && chip_decl.is_self_receiver()
        {
            let mut recv_args = Vec::with_capacity(args.len() + 1);
            recv_args.push(CallArg::Positional(obj.as_ref().clone()));
            recv_args.extend(args.iter().cloned());
            return lower_chip_call(ctx, &chip_decl, &recv_args, type_args, range);
        }
        // Backstop for the whole container-method class: `field` IS an array/map
        // method, but its receiver resolved to none of the container bindings
        // above. Without this it would fall through to a silent `_Unsupported`
        // gate (the exact `Map<K,V>`-passed-by-value failure mode). Report it so
        // a future container type that misses a binding site can't ship a
        // no-diagnostic miscompile.
        if crate::catalog::arrays::is_array_method(field)
            || crate::catalog::maps::is_map_method(field)
        {
            ctx.error(
                "WS044",
                format!(
                    "`.{field}()` is an array/map method, but its receiver did not \
                     resolve to an array or map here (e.g. a container in an \
                     unsupported position) — the operation would be silently dropped"
                ),
                range,
            );
        }
    }
    synthesise_unsupported(ctx, e)
}

/// Bake an event's config args (`ChatCommand("greet", Description = "…")`) into
/// its gate data-struct properties, for the event-as-expression form. Positional
/// literals fill `config_positional` in order; a named arg targets a `config_named`
/// field (case-insensitive). Non-matching or non-literal args are ignored — input
/// wires (`config_named` is disjoint from `input_named`) and non-constant values
/// are handled separately. Mirrors `handler::event_config_props`.
fn event_config_props_from_call_args(
    evt: &crate::catalog::events::EventSpec,
    args: &[CallArg],
) -> HashMap<crate::intern::Sym, Literal> {
    let mut props: HashMap<crate::intern::Sym, Literal> = HashMap::default();
    let mut positional = 0;
    for arg in args {
        let (field, value) = match arg {
            CallArg::Positional(value) => {
                let field = evt.config_positional.get(positional).copied();
                positional += 1;
                (field, value)
            }
            CallArg::Named { name, value, .. } => {
                let key = name.to_ascii_lowercase();
                let field = evt
                    .config_named
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, f)| *f);
                (field, value)
            }
            CallArg::Spread(_) => continue,
        };
        if let (Some(field), Some(lit)) = (field, expr_to_literal(value)) {
            props.insert(intern(field), lit);
        }
    }
    props
}
