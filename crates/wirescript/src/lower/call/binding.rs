//! What an inline expansion and a microchip instance share: which caller
//! values a call captures.

use super::*;
use crate::collections::HashSet;

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
pub(super) enum RefArg {
    /// The argument names the caller's own storage (`x`, `&x`, `arr`, a record
    /// field): the parameter aliases it at compile time.
    Static(VarRecord),
    /// An `in` port carrying an array or ref.
    Input(NodeRecord),
    /// A name already bound to a runtime ref: forward the same wire.
    Forward(DynRefRecord),
    /// A ref VALUE (a ref-holding var, an array element, event data): its
    /// lowered value is the wire that carries the ref.
    Value,
    /// An array or map literal, which names no storage: the parameter binds
    /// nothing, so a container use in the body reports WS044.
    Unbound,
}

pub(super) fn classify_ref_arg(ctx: &LowerCtx, param_typ: &TypeExpr, arg: &Expr) -> RefArg {
    // `&x` always names x's own storage.
    if let Expr::RefOf { operand, .. } = arg {
        return classify_named(ctx, param_typ, operand).unwrap_or_else(|| ref_value(arg));
    }
    // A `*T` value binds the ref it carries, whatever holds it: a ref-holding
    // var passed by name binds the var it POINTS AT, not itself.
    if matches!(ctx.type_of(arg), Type::Ref(_)) {
        return RefArg::Value;
    }
    classify_named(ctx, param_typ, arg).unwrap_or_else(|| ref_value(arg))
}

/// `Unbound` for an array or map literal, which has no storage behind it;
/// `Value` otherwise. Only an argument typecheck accepted reaches here: a ref
/// value (recorded as `Ref`) was bound above, and a scalar with neither a ref
/// type nor storage behind it (`rr.n`, `m.get(k).Value` on a value map) is
/// already WS008, so what is left carries a ref on its wire.
fn ref_value(arg: &Expr) -> RefArg {
    if matches!(deref_arg(arg), Expr::Array { .. } | Expr::MapLit { .. }) {
        RefArg::Unbound
    } else {
        RefArg::Value
    }
}

fn classify_named(ctx: &LowerCtx, param_typ: &TypeExpr, arg: &Expr) -> Option<RefArg> {
    let wants_container = matches!(
        crate::lower::context::container_storage(param_typ),
        Some(VarStorage::Array | VarStorage::Map)
    );
    let binding = match arg {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        _ => resolve_field_chain(ctx, arg).cloned(),
    }?;
    match binding {
        // A var HOLDING a container ref (`cur: *int[]`) is not the container;
        // its value is the ref.
        Binding::Var(v) if wants_container && v.storage == VarStorage::Var => None,
        Binding::Var(v) => Some(RefArg::Static(v)),
        Binding::Input(inp) => Some(RefArg::Input(inp)),
        Binding::DynRef(d) => Some(RefArg::Forward(d)),
        _ => None,
    }
}

pub(super) fn resolve_caller_captures(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
) -> HashMap<String, VarRecord> {
    let positional_args = positional_args(args);
    let mut captures = HashMap::default();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };

        // Must be the same lookup `wire_chip_args_and_outputs` and
        // `explode_record_param_pins` use: if this loop disagrees about which
        // params are record-shaped, the caller wires values where the body
        // expects refs.
        let resolved_record = ctx.record_or_tuple_fields(&param.typ);

        if let Some(fields) = &resolved_record {
            if let Some(Binding::Record(rec_fields)) =
                resolve_field_chain(ctx, deref_arg(arg_expr)).cloned()
            {
                for field in fields {
                    if !matches!(&field.typ, TypeExpr::Array { .. } | TypeExpr::Ref { .. }) {
                        continue;
                    }
                    let field_sym = crate::intern::intern(&field.name);
                    if let Some(Binding::Var(var_rec)) = rec_fields.get(&field_sym) {
                        let port_name = format!("{}_{}", param.name, field.name);
                        captures.insert(port_name, var_rec.clone());
                    }
                }
            }
        } else if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
            if let RefArg::Static(var_rec) = classify_ref_arg(ctx, &param.typ, arg_expr) {
                captures.insert(param.name.clone(), var_rec);
            }
        }
    }
    captures
}

pub(crate) fn compute_scope_captures(module: &Module) -> Vec<NodeId> {
    let mut internal: HashSet<NodeId> =
        HashSet::with_capacity_and_hasher(module.nodes.len(), Default::default());
    internal.extend(module.nodes.keys().copied());
    // `external` preserves first-seen order (instantiation remaps by index);
    // the set only answers membership.
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
