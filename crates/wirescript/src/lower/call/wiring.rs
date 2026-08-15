//! Wire an instance's arguments and outputs at the call site.

use super::*;

pub(super) fn wire_chip_args_and_outputs(
    ctx: &mut LowerCtx,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    caller_captures: &HashMap<String, VarRecord>,
    child_inputs: &[NodeId],
    child_outputs: &[NodeId],
    const_folds: &mut Vec<ConstFold>,
) -> PortRef {
    let positional_args: Vec<&Expr> = args
        .iter()
        .filter_map(|a| match a {
            CallArg::Positional(e) => Some(e),
            CallArg::Named { .. } | CallArg::Spread(_) => None,
        })
        .collect();
    let mut input_idx: usize = 0;
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };

        if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
            if caller_captures.contains_key(&param.name) {
                continue;
            }
            // Non-captured ref/array: has a MicrochipInput in the child
            let mc_input = child_inputs[input_idx];
            input_idx += 1;
            let is_array = matches!(&param.typ, TypeExpr::Array { .. });
            let ref_port_id = if is_array {
                WirePort::ArrayVarRef
            } else {
                WirePort::VarRef
            };
            if let Expr::Ident { name, .. } = arg_expr {
                if let Some(var_rec) = ctx.lookup_var(name).cloned() {
                    ctx.connect(
                        var_rec.node_id.port(ref_port_id),
                        mc_input.port(WirePort::RerInput),
                    );
                } else if let Some(Binding::Input(inp)) = ctx.scope.get(name).cloned() {
                    // An `in X: T[]` / ref input passed by reference: its array
                    // ref lives at RER_Output, not an ArrayVarRef/VarRef port.
                    ctx.connect(
                        inp.node_id.port(WirePort::RerOutput),
                        mc_input.port(WirePort::RerInput),
                    );
                }
            } else if let Some(Binding::Var(var_rec)) = resolve_field_chain(ctx, arg_expr).cloned()
            {
                ctx.connect(
                    var_rec.node_id.port(ref_port_id),
                    mc_input.port(WirePort::RerInput),
                );
            }
            continue;
        }

        let resolved_rec = ctx.record_fields_of(&param.typ);
        if let Some(fields) = &resolved_rec {
            if let Some(Binding::Record(rec_fields)) = resolve_field_chain(ctx, arg_expr).cloned() {
                for field in fields {
                    let port_name = format!("{}_{}", param.name, field.name);
                    if caller_captures.contains_key(&port_name) {
                        continue;
                    }
                    let mc_input = child_inputs[input_idx];
                    input_idx += 1;
                    let field_sym = crate::intern::intern(&field.name);
                    if let Some(binding) = rec_fields.get(&field_sym) {
                        match binding {
                            Binding::Var(var_rec) => {
                                let vr = if var_rec.storage == VarStorage::Array {
                                    var_rec.node_id.port(WirePort::ArrayVarRef)
                                } else {
                                    var_rec.node_id.port(WirePort::VarRef)
                                };
                                ctx.connect(vr, mc_input.port(WirePort::RerInput));
                            }
                            Binding::Local(local) => {
                                ctx.connect(local.port, mc_input.port(WirePort::RerInput));
                            }
                            Binding::Input(inp) => {
                                ctx.connect(
                                    inp.node_id.port(WirePort::RerOutput),
                                    mc_input.port(WirePort::RerInput),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }

        let mc_input = child_inputs[input_idx];
        input_idx += 1;
        // A `MicrochipInput` is a rerouter, and a rerouter has no data struct to
        // hold inline gate data, so a constant wired in from the caller has to stay
        // a real gate in every instance. Record it instead and clone it into the
        // chip'''s own module below, where `inline_orphan_literals` folds it onto its
        // consumer — the same gates the equivalent `mod` emits.
        if let Some(value) = const_arg_literal(arg_expr) {
            let ty = type_of_type_expr(&param.typ);
            const_folds.push(ConstFold {
                pin: mc_input,
                index: input_idx - 1,
                value,
                ty,
            });
            continue;
        }
        let val_port = lower_expr(ctx, arg_expr);
        ctx.connect(val_port, mc_input.port(WirePort::RerInput));
    }

    // The chip body may have written to caller-visible vars — through a ref/
    // array param, a dissolved record field, OR a top-level var it references
    // directly (e.g. `chip Bump() { g = g + 1 }` incrementing a global). Only
    // the param cases were cleared before, so a global a chip wrote left the
    // caller's cached Var_Get stale, and a read after the call saw the
    // pre-call value. The instance body is a separate module, so we can't
    // cheaply tell which caller vars it touched — blanket-reset every cache,
    // exactly as the inline-mod path does after its body.
    reset_var_get_caches(ctx);

    if !child_outputs.is_empty() {
        child_outputs[0].port(WirePort::RerOutput)
    } else {
        // Side-effect-only chip — no output value. NodeId(0) is never
        // allocated so any wire referencing it will be caught as invalid.
        NodeId(0).port(WirePort::Output)
    }
}
