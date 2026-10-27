//! What an inline expansion and a microchip instance share: which caller
//! values a call captures.

use super::*;

/// Strips a call argument's `&` / `ref` sigil. `&x` names the same storage as
/// bare `x`, but it parses as `Expr::RefOf`, which resolves to no binding at
/// all, so a ref/container argument must be unwrapped before it is looked up.
pub(super) fn deref_arg(arg: &Expr) -> &Expr {
    match arg {
        Expr::RefOf { operand, .. } => operand.as_ref(),
        other => other,
    }
}

/// How a call argument binds a `*T` or container (`T[]`, `Map<K, V>`)
/// parameter. Shared by the inline binder, the chip capture pass and the chip
/// pin wiring, so the three cannot disagree about which arguments name storage.
pub(in crate::lower) enum RefArg {
    /// The argument names the caller's own storage (`x`, `&x`, `arr`, a record
    /// field): the parameter aliases it at compile time.
    Static(VarRecord),
    /// An `in` port carrying an array. A scalar `*T` port carries a ref
    /// value and classifies as [`RefArg::Value`].
    Input(NodeRecord),
    /// A name already bound to a runtime ref: forward the same wire.
    Forward(DynRefRecord),
    /// A ref VALUE (a ref-holding var, an array element, event data): its
    /// lowered value is the wire that carries the ref.
    Value,
    /// An array or map literal, which names no storage. A constant one binds
    /// its immutable container gate (`materialize_literal_container`); any
    /// other is WS044 at the call.
    Unbound,
}

pub(in crate::lower) fn classify_ref_arg(
    ctx: &LowerCtx<'_>,
    param_storage: Option<VarStorage>,
    arg: &Expr,
) -> RefArg {
    // `&x` always names x's own storage.
    if let Expr::RefOf { operand, .. } = arg {
        return classify_named(ctx, param_storage, operand).unwrap_or_else(|| ref_value(ctx, arg));
    }
    // The argument's binding decides: storage binds the parameter to itself,
    // and a ref value (a ref-holding var, a field or pin carrying a ref,
    // a `*T` `in` port) binds it to the ref's wire.
    match classify_named(ctx, param_storage, arg) {
        Some(RefArg::Input(_)) if matches!(ctx.type_of(arg), Type::Ref(_)) => RefArg::Value,
        Some(bound) => bound,
        None => ref_value(ctx, arg),
    }
}

/// `Unbound` for an array or map literal (`{}` included), which has no storage
/// behind it; `Value` otherwise. A scalar with neither a ref type nor storage
/// is typecheck's WS008, so any other argument here carries a ref on its wire.
fn ref_value(ctx: &LowerCtx<'_>, arg: &Expr) -> RefArg {
    if matches!(
        ctx.with_map_literals(deref_arg(arg)).as_ref(),
        Expr::Array { .. } | Expr::MapLit { .. }
    ) {
        RefArg::Unbound
    } else {
        RefArg::Value
    }
}

fn classify_named(
    ctx: &LowerCtx<'_>,
    param_storage: Option<VarStorage>,
    arg: &Expr,
) -> Option<RefArg> {
    let wants_container = matches!(param_storage, Some(VarStorage::Array | VarStorage::Map));
    let binding = match arg {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        _ => resolve_field_chain(ctx, arg).cloned(),
    }?;
    match binding {
        // A var HOLDING a container ref (`cur: *int[]`) is not the container,
        // nor is a var holding a scalar ref its target: their value is the ref.
        Binding::Var(v) if wants_container && v.storage == VarStorage::Var => None,
        Binding::Var(v) if v.storage == VarStorage::Var && matches!(v.inner_type, Type::Ref(_)) => {
            None
        }
        Binding::Var(v) => Some(RefArg::Static(v)),
        Binding::Input(inp) => Some(RefArg::Input(inp)),
        Binding::DynRef(d) => Some(RefArg::Forward(d)),
        _ => None,
    }
}

pub(super) fn resolve_caller_captures(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    args: &[CallArg],
) -> HashMap<String, VarRecord> {
    let positional_args = positional_args(args);
    let mut captures = HashMap::default();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };

        // The same lookup `wire_chip_args_and_outputs` and
        // `explode_record_param_pins` use; disagreeing on which params are
        // record-shaped wires values where the body expects refs.
        let resolved_record = ctx.record_or_tuple_fields(&param.typ);

        if let Some(fields) = &resolved_record {
            if let Some(Binding::Record(rec_fields)) =
                resolve_field_chain(ctx, deref_arg(arg_expr)).cloned()
            {
                for field in fields {
                    if !ctx.ref_or_array(Some(chip_decl), &field.typ) {
                        continue;
                    }
                    let field_sym = intern(&field.name);
                    // A var holding a ref is a ref value: it crosses on the pin.
                    if let Some(b @ Binding::Var(var_rec)) = rec_fields.get(&field_sym)
                        && (var_rec.storage != VarStorage::Var || ref_fields::binds_place(b))
                    {
                        let port_name = format!("{}_{}", param.name, field.name);
                        captures.insert(port_name, var_rec.clone());
                    }
                }
            }
        } else if ctx.ref_or_array(Some(chip_decl), &param.typ) {
            // A constant array argument (a `const` table, a `let` of one, or an
            // array literal) is captured as its immutable container gate, the
            // way the inline-mod binder takes it, for a `*T[]` param too.
            let arg = deref_arg(arg_expr);
            materialize_const_container(ctx, arg);
            if matches!(
                ctx.declared_shape(Some(chip_decl), &param.typ),
                TypeExpr::Array { .. }
            ) && let Some(rec) = materialize_literal_container(ctx, arg, &param.name)
            {
                captures.insert(param.name.clone(), rec);
                continue;
            }
            let storage = ctx.ref_storage(Some(chip_decl), &param.typ);
            if let RefArg::Static(var_rec) = classify_ref_arg(ctx, storage, arg_expr) {
                captures.insert(param.name.clone(), var_rec);
            }
        }
    }
    captures
}

pub(crate) fn compute_scope_captures(module: &Module) -> Vec<NodeId> {
    let mut internal: HashSet<NodeId> =
        HashSet::with_capacity_and_hasher(module.nodes.len(), rustc_hash::FxBuildHasher);
    internal.extend(module.nodes.keys().copied());
    // `external` keeps first-seen order, so the list is deterministic; the
    // set only answers membership.
    let mut external = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::default();
    let visit = |id: NodeId, external: &mut Vec<NodeId>, seen: &mut HashSet<NodeId>| {
        if !internal.contains(&id) && seen.insert(id) {
            external.push(id);
        }
    };
    for w in &module.wires {
        visit(w.source.node_id, &mut external, &mut seen);
        visit(w.target.node_id, &mut external, &mut seen);
    }
    for child_module in module.chips.values() {
        for &cap_id in &child_module.scope_captures {
            visit(cap_id, &mut external, &mut seen);
        }
    }
    external
}
