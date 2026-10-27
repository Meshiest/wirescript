//! Wire an instance's arguments and outputs at the call site.

use super::*;

pub(super) fn wire_chip_args_and_outputs(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    args: &[CallArg],
    caller_captures: &HashMap<String, VarRecord>,
    child_inputs: &[NodeId],
    child_outputs: &[NodeId],
    const_folds: &mut Vec<ConstFold>,
) -> PortRef {
    let positional_args = positional_args(args);
    let mut input_idx: usize = 0;
    // Each argument is fixed at its value before a later argument runs code
    // that may change what it reads; a pin already wired from it is rewired.
    let mut args_in_order = ordered::ArgOrder::default();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        args_in_order.next(ctx);
        // A `const` parameter has no pin (`build_chip_module` bakes it on the
        // same `is_const` predicate), so it takes no `input_idx` slot. Checked
        // before the argument lookup: any divergence between the two loops
        // shifts every later pin index and mis-wires the call.
        if param.is_const {
            continue;
        }
        let Some(raw_arg) = positional_args.get(i) else {
            continue;
        };
        // `&x` / `ref x` names the same storage as bare `x`, so the sigil is
        // stripped for a param that binds storage or a record. Value params
        // keep the argument as written.
        let arg_expr: &Expr = if ctx.ref_storage(Some(chip_decl), &param.typ).is_some()
            || ctx.record_or_tuple_fields(&param.typ).is_some()
        {
            deref_arg(raw_arg)
        } else {
            raw_arg
        };

        // A `*Record` is not a scalar ref: it takes the record branch below,
        // one pin per leaf.
        if ctx.ref_or_array(Some(chip_decl), &param.typ)
            && ctx.record_or_tuple_fields(&param.typ).is_none()
        {
            if caller_captures.contains_key(&param.name) {
                continue;
            }
            // A non-captured ref/array has a MicrochipInput in the child.
            let mc_input = child_inputs[input_idx];
            input_idx += 1;
            let is_array = matches!(
                ctx.declared_shape(Some(chip_decl), &param.typ),
                TypeExpr::Array { .. }
            );
            // An array of enums is a column per slot with no one ref for the
            // pin; typecheck reports the parameter (WS049), once.
            if is_array
                && matches!(
                    resolve_field_chain(ctx, deref_arg(raw_arg)),
                    Some(Binding::Record(r)) if r.contains_key(&*DISC)
                )
            {
                continue;
            }
            let ref_port_id = if is_array {
                WirePort::ArrayVarRef
            } else {
                WirePort::VarRef
            };
            let src = match classify_ref_arg(
                ctx,
                ctx.ref_storage(Some(chip_decl), &param.typ),
                raw_arg,
            ) {
                RefArg::Static(var_rec) => var_rec.node_id.port(ref_port_id),
                // An `in X: T[]` / ref input passed by reference: its ref lives
                // at RER_Output, not an ArrayVarRef/VarRef port.
                RefArg::Input(inp) => inp.node_id.port(WirePort::RerOutput),
                RefArg::Forward(d) => d.port,
                RefArg::Value => lower_expr(ctx, raw_arg),
                RefArg::Unbound => {
                    ctx.error(
                        "WS044",
                        format!(
                            "argument `{}` is an array/map literal with non-constant elements, \
                             which names no storage for the chip's container parameter to use, \
                             so operations on it would be silently dropped. Pass an array or \
                             map `var`",
                            param.name
                        ),
                        raw_arg.range(),
                    );
                    continue;
                }
            };
            ctx.connect(src, mc_input.port(WirePort::RerInput));
            continue;
        }

        let resolved_rec = ctx.record_or_tuple_fields(&param.typ);
        if let Some(fields) = &resolved_rec {
            // The record argument's per-field bindings, from any value form,
            // wire each leaf pin; `input_idx` advances one per leaf in lockstep
            // with the body-side explode.
            let rec_fields = value_record_fields(ctx, arg_expr);
            let wires = ctx.builder.module.wires.len();
            let wired = wire_record_param_pins(
                ctx,
                &param.name,
                fields,
                rec_fields.as_ref(),
                child_inputs,
                &mut input_idx,
                caller_captures,
                raw_arg.range(),
            );
            args_in_order.bind(
                Binding::Record(wired),
                &param.name,
                raw_arg.range(),
                Some(wires),
            );
            continue;
        }

        let mc_input = child_inputs[input_idx];
        input_idx += 1;
        // A `MicrochipInput` is a rerouter with no data struct for inline gate
        // data, so a constant argument is recorded and cloned into the chip's
        // own module, where `inline_orphan_literals` folds it onto its consumer.
        if let Some((ty, value)) = const_arg_literal(ctx, arg_expr) {
            const_folds.push(ConstFold {
                pin: mc_input,
                index: input_idx - 1,
                value,
                ty,
            });
            continue;
        }
        let val_port = lower_expr(ctx, arg_expr);
        let wires = ctx.builder.module.wires.len();
        args_in_order.bind(
            Binding::Local(LocalRecord { port: val_port }),
            &param.name,
            raw_arg.range(),
            Some(wires),
        );
        ctx.connect(val_port, mc_input.port(WirePort::RerInput));
    }
    args_in_order.finish(ctx);

    // The body may have written any caller-visible var (through a param or a
    // global it names directly), and which ones is not known here, so every
    // cached read is dropped. The writes draw no wire out of the caller's
    // storage, so they count as dynamic writes for an enclosing `for`.
    note_dynamic_write(ctx);

    // A record-typed signature output has one child pin per leaf, so the
    // per-leaf record is stashed in `pending_inline_record` for consumers to
    // read through `value_record_fields`.
    if chip_decl.outputs.len() == 1
        && ctx
            .record_or_tuple_fields(&chip_decl.outputs[0].typ)
            .is_some()
    {
        let mut idx = 0usize;
        let rec = child_output_record(ctx, &chip_decl.outputs[0].typ, child_outputs, &mut idx);
        ctx.pending_inline_record = Some(rec);
    }
    if !child_outputs.is_empty() {
        child_outputs[0].port(WirePort::RerOutput)
    } else {
        // A side-effect-only chip has no output value. NodeId(0) is never
        // allocated, so any wire referencing it is caught as invalid.
        NodeId(0).port(WirePort::Output)
    }
}

/// The caller-side `Binding::Record` for a record-typed chip signature output:
/// one leaf per child output pin, consumed in the declaration order
/// `predeclare::record_output_pins` created them in.
fn child_output_record(
    ctx: &mut LowerCtx<'_>,
    te: &TypeExpr,
    child_outputs: &[NodeId],
    idx: &mut usize,
) -> Fields {
    let fields = ctx.record_or_tuple_fields(te).unwrap_or_default();
    let mut out = HashMap::default();
    for field in &fields {
        if ctx.record_or_tuple_fields(&field.typ).is_some() {
            let inner = child_output_record(ctx, &field.typ, child_outputs, idx);
            out.insert(intern(&field.name), Binding::Record(inner));
            continue;
        }
        let Some(pin) = child_outputs.get(*idx) else {
            break;
        };
        *idx += 1;
        out.insert(
            intern(&field.name),
            Binding::Local(LocalRecord {
                port: pin.port(WirePort::RerOutput),
            }),
        );
    }
    out
}

/// Wire a record argument's leaf sources into the child's per-leaf input pins,
/// recursing exactly as `explode_record_param_pins` does (decided from the
/// field's syntactic type alone), so `input_idx` stays in lockstep.
fn wire_record_param_pins(
    ctx: &mut LowerCtx<'_>,
    prefix: &str,
    fields: &[RecordTypeField],
    value_fields: Option<&Fields>,
    child_inputs: &[NodeId],
    input_idx: &mut usize,
    caller_captures: &HashMap<String, VarRecord>,
    arg_range: &SourceRange,
) -> Fields {
    let mut wired = HashMap::default();
    for field in fields {
        let port_name = format!("{prefix}_{}", field.name);
        let is_container = ctx.ref_storage(None, &field.typ).is_some();
        if !is_container && let Some(sub) = ctx.record_or_tuple_fields(&field.typ) {
            let nested_value = value_fields
                .and_then(|vf| vf.get(&intern(&field.name)))
                .and_then(|b| match b {
                    Binding::Record(m) => Some(m.clone()),
                    _ => None,
                });
            let sub_wired = wire_record_param_pins(
                ctx,
                &port_name,
                &sub,
                nested_value.as_ref(),
                child_inputs,
                input_idx,
                caller_captures,
                arg_range,
            );
            wired.insert(intern(&field.name), Binding::Record(sub_wired));
            continue;
        }
        if caller_captures.contains_key(&port_name) {
            continue;
        }
        let mc_input = child_inputs[*input_idx];
        *input_idx += 1;
        let binding = value_fields
            .and_then(|vf| vf.get(&intern(&field.name)))
            .cloned();
        // A `*T` field's pin carries a ref: a place's own, or the one a value holds.
        let scalar_ref = ctx.ref_storage(None, &field.typ) == Some(VarStorage::Var);
        if let Some(binding) = binding
            && let Some(src) = if scalar_ref {
                ref_fields::ref_port_of(ctx, &binding, arg_range)
            } else {
                binding_to_port(ctx, &binding, arg_range)
            }
        {
            ctx.connect(src, mc_input.port(WirePort::RerInput));
            wired.insert(
                intern(&field.name),
                Binding::Local(LocalRecord { port: src }),
            );
        }
    }
    wired
}
