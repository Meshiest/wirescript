//! What an inline expansion and a microchip instance share: which caller
//! values a call captures.

use super::*;
use crate::collections::HashSet;

pub(super) fn resolve_caller_captures(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
) -> HashMap<String, VarRecord> {
    let positional_args: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect();
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
            if let Some(Binding::Record(rec_fields)) = resolve_field_chain(ctx, arg_expr).cloned() {
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
            let var_rec = if let Expr::Ident { name, .. } = arg_expr {
                ctx.lookup_var(name).cloned()
            } else if let Some(Binding::Var(v)) = resolve_field_chain(ctx, arg_expr).cloned() {
                Some(v)
            } else {
                None
            };
            if let Some(var_rec) = var_rec {
                captures.insert(param.name.clone(), var_rec);
            }
        }
    }
    captures
}

pub(crate) fn compute_scope_captures(module: &Module) -> Vec<NodeId> {
    let internal: HashSet<NodeId> = module.nodes.keys().cloned().collect();
    let mut external = Vec::new();
    for w in &module.wires {
        if !internal.contains(&w.source.node_id) && !external.contains(&w.source.node_id) {
            external.push(w.source.node_id);
        }
        if !internal.contains(&w.target.node_id) && !external.contains(&w.target.node_id) {
            external.push(w.target.node_id);
        }
    }
    for child_module in module.chips.values() {
        for &cap_id in &child_module.scope_captures {
            if !internal.contains(&cap_id) && !external.contains(&cap_id) {
                external.push(cap_id);
            }
        }
    }
    external
}
