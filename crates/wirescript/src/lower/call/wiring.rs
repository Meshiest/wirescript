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
        // A `const` parameter has no `MicrochipInput` pin: its value is baked
        // into the body's constant environment by `build_chip_module`, which
        // skips it on this identical `is_const` predicate. So it consumes no
        // `input_idx` slot and needs no wire. Checked BEFORE the argument
        // lookup so the two loops agree param-for-param — any divergence would
        // shift every later pin index and silently mis-wire the call.
        if param.is_const {
            continue;
        }
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };
        // `&x` / `ref x` names the same storage as bare `x`, so strip the sigil
        // for a param that binds storage. Every resolution below needs it: the
        // ref/array pin, the record fields, and the general value path a map
        // param takes. Value params keep the argument as written.
        let arg_expr: &Expr = if crate::lower::context::container_storage(&param.typ).is_some()
            || ctx.record_or_tuple_fields(&param.typ).is_some()
        {
            deref_arg(arg_expr)
        } else {
            arg_expr
        };

        // A `*Record` is NOT a scalar ref: `record_or_tuple_fields` reports its
        // per-field shape, so let it fall through to the record branch below,
        // which allocates one pin per leaf.
        if matches!(&param.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. })
            && ctx.record_or_tuple_fields(&param.typ).is_none()
        {
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

        let resolved_rec = ctx.record_or_tuple_fields(&param.typ);
        if let Some(fields) = &resolved_rec {
            // Resolve the record argument's per-field bindings from any value
            // form (a scope binding, a record literal, an index/map read, or a
            // record-returning call), then wire each LEAF pin. A nested record
            // field recurses into its own leaf pins (matching the body-side
            // explode), so `input_idx` advances one per leaf in lockstep with pin
            // creation and later pins stay index-aligned.
            let rec_fields = value_record_fields(ctx, arg_expr);
            wire_record_param_pins(
                ctx,
                &param.name,
                fields,
                rec_fields.as_ref(),
                child_inputs,
                &mut input_idx,
                caller_captures,
            );
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
    // directly (e.g. `chip Bump() { g = g + 1 }` incrementing a global).
    // Clearing only the param-passed vars would leave a global write's cache
    // stale, so a read after the call would see the pre-call value. The
    // instance body is a separate module, so we can't cheaply tell which
    // caller vars it touched — blanket-reset every cache, exactly as the
    // inline-mod path does after its body.
    reset_var_get_caches(ctx);

    // A record-typed signature output has one child pin per LEAF, so the call
    // does not produce a single value port. Stash the per-leaf record the way
    // every other record-producing call does (`pending_inline_record`) so an
    // assignment / `out` / argument consumer picks it up through
    // `value_record_fields`. Without it the call returned pin 0 alone and every
    // other leaf was silently unwired.
    if chip_decl.outputs.len() == 1
        && ctx.record_fields_of(&chip_decl.outputs[0].typ).is_some()
    {
        let mut idx = 0usize;
        let rec = child_output_record(ctx, &chip_decl.outputs[0].typ, child_outputs, &mut idx);
        ctx.pending_inline_record = Some(rec);
    }
    if !child_outputs.is_empty() {
        child_outputs[0].port(WirePort::RerOutput)
    } else {
        // Side-effect-only chip — no output value. NodeId(0) is never
        // allocated so any wire referencing it will be caught as invalid.
        NodeId(0).port(WirePort::Output)
    }
}


/// The caller-side `Binding::Record` for a record-typed chip signature output:
/// one leaf per child output pin, consumed in the SAME declaration order the
/// body-side explode (`predeclare::record_output_pins`) created them in, so the
/// two sides cannot drift.
fn child_output_record(
    ctx: &mut LowerCtx,
    te: &TypeExpr,
    child_outputs: &[NodeId],
    idx: &mut usize,
) -> HashMap<crate::intern::Sym, Binding> {
    let fields = ctx.record_fields_of(te).unwrap_or_default();
    let mut out = HashMap::default();
    for field in &fields {
        if ctx.record_fields_of(&field.typ).is_some() {
            let inner = child_output_record(ctx, &field.typ, child_outputs, idx);
            out.insert(crate::intern::intern(&field.name), Binding::Record(inner));
            continue;
        }
        let Some(pin) = child_outputs.get(*idx) else {
            break;
        };
        *idx += 1;
        out.insert(
            crate::intern::intern(&field.name),
            Binding::Local(LocalRecord {
                port: pin.port(WirePort::RerOutput),
            }),
        );
    }
    out
}

/// Wire a record argument's LEAF sources into the child's per-leaf input pins.
/// Recurses into a nested-record field the SAME way the body-side explode does
/// (`explode_record_param_pins`), consuming one pin per leaf so `input_idx`
/// stays lockstep with pin creation. The recurse-vs-leaf choice is made purely
/// from the field's syntactic type (a container is a single ref pin; a record
/// recurses; a scalar is one pin), so both sides always agree.
fn wire_record_param_pins(
    ctx: &mut LowerCtx,
    prefix: &str,
    fields: &[crate::ast::RecordTypeField],
    value_fields: Option<&HashMap<crate::intern::Sym, Binding>>,
    child_inputs: &[NodeId],
    input_idx: &mut usize,
    caller_captures: &HashMap<String, VarRecord>,
) {
    for field in fields {
        let port_name = format!("{prefix}_{}", field.name);
        let is_container = crate::lower::context::container_storage(&field.typ).is_some();
        if !is_container
            && let Some(sub) = ctx.record_or_tuple_fields(&field.typ)
        {
            let nested_value = value_fields
                .and_then(|vf| vf.get(&crate::intern::intern(&field.name)))
                .and_then(|b| match b {
                    Binding::Record(m) => Some(m.clone()),
                    _ => None,
                });
            wire_record_param_pins(
                ctx,
                &port_name,
                &sub,
                nested_value.as_ref(),
                child_inputs,
                input_idx,
                caller_captures,
            );
            continue;
        }
        if caller_captures.contains_key(&port_name) {
            continue;
        }
        let mc_input = child_inputs[*input_idx];
        *input_idx += 1;
        let binding = value_fields
            .and_then(|vf| vf.get(&crate::intern::intern(&field.name)))
            .cloned();
        if let Some(binding) = binding
            && let Some(src) = binding_to_port(ctx, &binding, &field.range)
        {
            ctx.connect(src, mc_input.port(WirePort::RerInput));
        }
    }
}
