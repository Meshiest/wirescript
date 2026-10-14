//! Resolve what a call's callee actually is, and route it.

use super::*;
use crate::typecheck::{ContainerKind, MethodTarget};

/// Whether `obj` already names a real array/map storage gate, in which case the
/// ordinary runtime lowering owns the call and no fold may run ahead of it.
fn receiver_has_runtime_container(ctx: &LowerCtx, obj: &Expr) -> bool {
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
    ctx: &mut LowerCtx,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let lit = {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()?
    };
    let ty = crate::lower::expr::wire_type_of_literal(&lit)?;
    Some(crate::lower::expr::literal_node_range(ctx, range, ty, lit))
}

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
    if let Expr::Ident { name, range: name_range } = callee.as_ref() {
        // User-defined chips/mods shadow builtins of the same name, so a program
        // can define e.g. `chip Toggle` without colliding with the builtin.
        if let Some(chip_decl) = ctx.call_target_chip(name, &name_range.file).cloned() {
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
        // An identifier callee that is neither an in-scope chip/mod nor a
        // builtin. If the name IS declared as a chip/mod somewhere in the
        // program, it's a use-before-declaration (chips/mods register in source
        // order): the call would otherwise synthesise an `_Unsupported` gate
        // that silently reads its default (0) at runtime — make it a hard error.
        // Names that are not chips/mods at all (e.g. a builtin not yet lowered)
        // fall through to the usual placeholder path. A chip/mod in scope that
        // the calling file can't name is the type checker's `WS002`.
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
        // Test-only hook: pins that typecheck's recorded resolution and
        // lowering's own re-derivation agree on the resolution KEY for nested
        // and generic bodies — something no output-level test can observe
        // (a wrong key can still lower to the right gates by luck). Costs
        // nothing in non-test builds.
        #[cfg(test)]
        crate::lower::tests::resolution::probe_method_call(ctx, e);
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
            // A read whose value is already compile-time known, on a receiver
            // that has no runtime container gate here. Typecheck exempts a
            // non-mutating call on a `const` receiver from the exec-context
            // rule (`container_call_exec_exempt`) because it is expected to
            // fold, so lowering owes it a fold: in pure context there is no
            // exec chain to hang the real gate on, and the map path answers
            // that by handing back the receiver's own ref port - the container
            // reference silently standing in for the value.
            //
            // Ordered exactly like `const_fold_index_access`: only when the
            // receiver resolves to NO runtime container binding, so a `const`
            // shadowed by a same-named runtime `var` still reads the var and no
            // program that already lowered correctly can change.
            if !receiver_has_runtime_container(ctx, obj)
                && let Some(port) = const_fold_container_call(ctx, e, range)
            {
                return port;
            }
            materialize_const_container(ctx, obj);
        }
        // `soa.field.sort(desc?)` where `soa` is a record array: sort the WHOLE
        // record by that field (reorder every sibling column via sortMultiple),
        // instead of sorting the one column and desyncing the rows. Checked
        // before the column's own container method below, which would sort in place.
        // Typecheck's kind for `soa` says whether `.field` is a column at all: a
        // record VARIABLE with array fields binds the same way but is not one.
        // A nested column (`qs.p.x`) climbs to the root record array, so every
        // column of the row moves with the key.
        if field == "sort"
            && let Expr::FieldAccess { obj: base, field: keyfield, .. } = obj.as_ref()
            && is_record_array_kind(ctx, base)
        {
            let mut root: &Expr = base;
            let mut path = vec![keyfield.clone()];
            while let Expr::FieldAccess { obj: parent, field: seg, .. } = root
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
            Some(MethodTarget::Container { kind, method }) => match container_ref(ctx, obj, &kind) {
                Some(ContainerSource::Ref(port)) => {
                    return match kind {
                        ContainerKind::Array { elem } => {
                            lower_array_method(ctx, port, elem, &method, args, range, e)
                        }
                        ContainerKind::Map { .. } => {
                            lower_map_method(ctx, port, kind.as_type(), &method, args, range, e)
                        }
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
            },
            Some(MethodTarget::Builtin(name)) => {
                if let Some(spec) = find_call(&name) {
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
            }
            // User `self`-receiver method calls: `v.dist(o)` where `dist` is a user
            // mod/chip whose first parameter is named `self`. Desugars to
            // `dist(v, o)` by prepending the receiver as positional arg 0.
            // Typecheck classifies a builtin of the same name first, so the
            // builtin wins on its own receiver type (WS035 rejects a self-mod
            // shadowing a builtin).
            Some(MethodTarget::SelfMod(name)) => {
                // Re-check `is_self_receiver` at the lowering scope: the
                // recorded target names a chip that WAS a self-receiver where
                // typecheck resolved it, but a lowering-scope shadow of a
                // non-`self` chip with the same name (e.g. a different generic
                // instantiation's local) would otherwise get the receiver
                // prepended silently.
                if let Some(chip_decl) = ctx.self_mod_chip(&name, &range.file).cloned() {
                    let mut recv_args = Vec::with_capacity(args.len() + 1);
                    recv_args.push(CallArg::Positional(obj.as_ref().clone()));
                    recv_args.extend(args.iter().cloned());
                    return lower_chip_call(ctx, &chip_decl, &recv_args, type_args, range);
                }
            }
            None => {}
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
            report_unresolved_container(ctx, &format!("`.{field}()` is an array/map method"), range);
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
    consts: &crate::const_scope::ConstNames,
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
fn is_record_array_kind(ctx: &LowerCtx, e: &Expr) -> bool {
    matches!(ctx.container_kind(e), Some(ContainerKind::Array { elem: Type::Record(_) }))
}
