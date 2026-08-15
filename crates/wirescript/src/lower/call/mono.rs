//! Generic instantiation: read an argument's lowered type, build a mono
//! frame, and key it.

use super::*;

/// Type of `port` as declared on its node (searching this module and any
/// already-built chip children). Used to read an argument's ACTUAL lowered type.
pub(in crate::lower) fn arg_port_type(ctx: &LowerCtx, port: PortRef) -> Option<Type> {
    fn find(m: &Module, id: NodeId) -> Option<&crate::ir::Node> {
        m.nodes
            .get(&id)
            .or_else(|| m.chips.values().find_map(|c| find(c, id)))
    }
    let n = find(&ctx.builder.module, port.node_id)?;
    let sym = intern(port.port.as_str());
    n.ports
        .find_output(sym)
        .or_else(|| n.ports.find_input(sym))
        .map(|p| p.ty.clone())
}

/// The concrete type an argument will lower to in the CURRENT context, read
/// from its scope binding / port type — NOT from typecheck's `type_of_expr`
/// map. This is the crux of nested-generic correctness: inside `outer<T>`'s
/// body a forwarded value param (`inner(v)`) is bound to the caller's already
/// concrete arg port (int under `outer<int>`, vector under `outer<vector>`),
/// so its real lowered type is that concrete type. `type_of_expr`, by contrast,
/// holds the STALE last-mask-member type the typecheck per-combo body check wrote
/// there (e.g. `Prefab`), which silently collapsed every nested monomorph to
/// the wrong variant. Falls back to `type_of` for non-ident args (literals,
/// compound expressions) — those aren't type-param-forwarded in practice, and
/// their `type_of_expr` entry is already concrete.
fn lowered_arg_type(ctx: &LowerCtx, e: &Expr) -> Type {
    match e {
        Expr::Ident { name, .. } => match ctx.scope.get(name) {
            Some(Binding::Local(l)) => {
                if let Some(t) = arg_port_type(ctx, l.port) {
                    return t;
                }
            }
            Some(Binding::Var(v)) => return v.inner_type.clone(),
            Some(Binding::Input(i)) => return i.ty.clone(),
            Some(Binding::Buffer(b)) => return b.ty.clone(),
            _ => {}
        },
        // A COMPOUND arg forwarded into a NESTED generic call (`Box(a + b)`,
        // `Box(if c then a else b)`) inside a generic body must report the
        // CURRENT monomorph's type — recurse structurally, resolving operators
        // and if-branches from their operands' monomorph types, exactly as the
        // emit side does (`mono_op_rule` / `lower_if_expr`). Otherwise the
        // `type_of` fallback below returns the STALE last-mask-member type the
        // per-combo body check wrote, and the inner call monomorphizes to the
        // wrong variant (`Numeric` → Color) — a silent miscompile. Gated on a
        // non-empty `mono_stack` so the non-generic path stays byte-identical.
        Expr::BinOp { op, left, right, .. } if !ctx.mono_stack.is_empty() => {
            let l = lowered_arg_type(ctx, left);
            let r = lowered_arg_type(ctx, right);
            if let Some(rule) = crate::catalog::operators::resolve_op(op, &[l, r]) {
                return rule.result.clone();
            }
        }
        Expr::UnOp { op, operand, .. } if !ctx.mono_stack.is_empty() => {
            let o = lowered_arg_type(ctx, operand);
            if let Some(rule) = crate::catalog::operators::resolve_op(op, &[o]) {
                return rule.result.clone();
            }
        }
        Expr::IfExpr {
            then_branch,
            else_branch,
            ..
        } if !ctx.mono_stack.is_empty() => {
            let t = unwrap_ref(&lowered_arg_type(ctx, then_branch));
            let e2 = unwrap_ref(&lowered_arg_type(ctx, else_branch));
            return crate::types::coerce::widening_join(&t, &e2).unwrap_or(e2);
        }
        // A single-output generic-mod call forwarded as an arg (`Box(id(a))`):
        // its monomorph return type is its declared return with the callee's
        // OWN subst applied, inferred from the (monomorph) arg types — not the
        // stale `type_of`. Mirrors what the inner call itself will do when
        // lowered. Recursion is bounded by expression nesting.
        Expr::Call { callee, args, .. } if !ctx.mono_stack.is_empty() => {
            if let Expr::Ident { name, .. } = callee.as_ref()
                && let Some(Binding::Chip(decl)) = ctx.scope.get(name)
            {
                let decl = decl.clone();
                if !decl.type_params.is_empty() && decl.outputs.len() == 1 {
                    let pos: Vec<&Expr> = args
                        .iter()
                        .filter_map(|a| match a {
                            CallArg::Positional(e) => Some(e),
                            _ => None,
                        })
                        .collect();
                    let frame = build_mono_frame(ctx, &decl, &pos, &[]);
                    let params: Vec<String> =
                        decl.type_params.iter().map(|tp| tp.name.clone()).collect();
                    let empty_aliases = crate::collections::HashMap::default();
                    let empty_generic = crate::collections::HashMap::default();
                    let cx = crate::types::resolve::ResolveCtx {
                        params: &params,
                        type_aliases: &empty_aliases,
                        generic_aliases: &empty_generic,
                    };
                    let ret = crate::types::resolve::resolve_type(
                        &decl.outputs[0].typ,
                        &cx,
                        &mut Vec::new(),
                    );
                    return crate::types::mono::substitute(&ret, &frame.subst);
                }
            }
        }
        _ => {}
    }
    ctx.type_of(e)
}

/// Rebuild a generic mod's call-site type substitution and package it (with the
/// callee's type-param names) into a [`MonoFrame`]. For each positional param,
/// resolve its declared type with the type params in scope (so `T` →
/// `Type::Param(T)`) and pair it with the argument's ACTUAL lowered type (see
/// [`lowered_arg_type`]); then `types::mono::infer_call_subst` runs the same
/// `collect` + `solve` inference typecheck used at the call site. Only
/// positional value args participate; masks come from each param's bound
/// (`T: Numeric`) so the solver's out-of-mask check matches typecheck's.
pub(super) fn build_mono_frame(
    ctx: &LowerCtx,
    chip_decl: &ChipDecl,
    positional_args: &[&Expr],
    type_args: &[TypeExpr],
) -> MonoFrame {
    let param_names: Vec<String> = chip_decl
        .type_params
        .iter()
        .map(|tp| tp.name.clone())
        .collect();
    // Explicit type arguments (`pick<int>(...)`): the caller pinned each type
    // param. Typecheck already validated arity + mask, so bind each `T_i` to its
    // resolved type arg directly — `resolve_local_type` monomorphizes a type
    // argument that itself references an outer `T` (`inner<T>(...)` inside
    // `outer<T>`'s body).
    if !type_args.is_empty() {
        let mut subst = crate::types::infer::Subst::new();
        for (tp, te) in chip_decl.type_params.iter().zip(type_args.iter()) {
            subst.insert(tp.name.clone(), ctx.resolve_local_type(te));
        }
        return MonoFrame {
            params: param_names,
            subst,
        };
    }
    let empty_aliases: crate::collections::HashMap<String, Type> = crate::collections::HashMap::default();
    let empty_generic_aliases: crate::collections::HashMap<String, crate::types::resolve::GenericAlias> =
        crate::collections::HashMap::default();
    let resolve_cx = crate::types::resolve::ResolveCtx {
        params: &param_names,
        type_aliases: &empty_aliases,
        generic_aliases: &empty_generic_aliases,
    };
    let mut param_types = Vec::new();
    let mut arg_types = Vec::new();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };
        param_types.push(crate::types::resolve::resolve_type(
            &param.typ,
            &resolve_cx,
            &mut Vec::new(),
        ));
        arg_types.push(lowered_arg_type(ctx, arg_expr));
    }
    let masks: Vec<(String, Vec<Type>)> = chip_decl
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                crate::types::mono::mask_for_param(tp.bound.as_ref(), &empty_aliases),
            )
        })
        .collect();
    let subst = crate::types::mono::infer_call_subst(&param_types, &arg_types, &masks);
    MonoFrame {
        params: param_names,
        subst,
    }
}

/// Canonical cache/template key for one generic-chip instantiation: the chip
/// name plus the concrete type each type param resolves to under this call's
/// substitution (`Boxed<int>`, `Boxed<vector>`). `{:?}` on a resolved `Type` is
/// a stable, unique-per-type string, so two instantiations collide iff they
/// pick the same concrete types — exactly when their emitted grids are
/// interchangeable and safe to dedup. A non-generic chip never calls this: its
/// key stays the bare name (byte-identical to before).
pub(super) fn mono_key(chip_decl: &ChipDecl, frame: &MonoFrame) -> String {
    let args: Vec<String> = frame
        .params
        .iter()
        .map(|p| {
            format!(
                "{:?}",
                crate::types::mono::substitute(&Type::Param(p.clone()), &frame.subst)
            )
        })
        .collect();
    format!("{}<{}>", chip_decl.name, args.join(","))
}
