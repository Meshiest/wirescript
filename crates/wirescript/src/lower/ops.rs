use std::fmt::Write as _;

use super::*;
use crate::catalog::operators::OpRule;
use crate::lower::fold::eval::{FORMAT_SLOTS, interp_groups};

/// The operator rule for the current monomorph. A non-generic body (including
/// one inlined from a generic caller) reads the baked `op_resolutions` map.
/// Inside a generic mod that map holds the last mask member's rule (a
/// `square<int>` would emit a Color `MathMultiply`), so the rule is re-resolved
/// from each operand's type in this instantiation, else its port's type. The
/// operand type wins over a narrower port: an `int` passed for a `float`
/// parameter still makes `x / 2` divide as floats.
fn mono_op_rule(
    ctx: &LowerCtx<'_>,
    e: &Expr,
    op: &str,
    operand_ports: &[PortRef],
) -> Option<OpRule> {
    if ctx.mono_stack.is_empty() {
        return ctx.op_for(e).cloned();
    }
    let port_types = || -> Vec<Type> {
        operand_ports
            .iter()
            .map(|&p| super::call::arg_port_type(ctx, p).unwrap_or(Type::Any))
            .collect()
    };
    if !ctx.in_generic_instance(e.range()) {
        return ctx
            .op_for(e)
            .cloned()
            .or_else(|| crate::catalog::operators::resolve_op(op, &port_types()).cloned());
    }
    let operands: Vec<&Expr> = match e {
        Expr::BinOp { left, right, .. } => vec![left, right],
        Expr::UnOp { operand, .. } => vec![operand],
        _ => Vec::new(),
    };
    let tys: Vec<Type> = port_types()
        .into_iter()
        .enumerate()
        .map(
            |(i, port)| match operands.get(i).map(|x| operand_type(&ctx.type_of(x))) {
                Some(t) if !matches!(t, Type::Any) => t,
                _ => port,
            },
        )
        .collect();
    crate::catalog::operators::resolve_op(op, &tys)
        .cloned()
        .or_else(|| ctx.op_for(e).cloned())
}

/// The type an operand takes part in an operator as: through a ref, and a
/// record (a multi-output result) as its first field, as typecheck does.
fn operand_type(t: &Type) -> Type {
    match unwrap_ref(t) {
        Type::Record(fields) if !fields.is_empty() => operand_type(&fields[0].1),
        other => other,
    }
}

pub(super) fn lower_binop(ctx: &mut LowerCtx<'_>, e: &Expr) -> PortRef {
    let Expr::BinOp { left, right, .. } = e else {
        return synthesise_unsupported(ctx, e);
    };
    // Non-generic path short-circuits on a missing rule (no operand gates
    // created). A generic body defers rule resolution until the operands are
    // lowered so `mono_op_rule` can read their concrete port types.
    if ctx.mono_stack.is_empty() && ctx.op_for(e).is_none() {
        return synthesise_unsupported(ctx, e);
    }
    // A brick or ref compared with `null` compares with the port left empty,
    // which holds that type's null reference: no literal spells one.
    let null_ref =
        |x: &Expr| matches!(x, Expr::NullLit { .. }) && ctx.type_of(x).ref_kind().is_some();
    if null_ref(left) || null_ref(right) {
        let (left_port, right_port) = if null_ref(left) {
            (None, Some(lower_expr(ctx, right)))
        } else {
            (Some(lower_expr(ctx, left)), None)
        };
        return binop_gate_of(ctx, e, left_port, right_port);
    }
    let ports = ordered::lower_in_order(ctx, &[left, right]);
    binop_gate(ctx, e, ports[0], ports[1])
}

/// The gate of the binary operator `e` over its lowered operands.
pub(super) fn binop_gate(
    ctx: &mut LowerCtx<'_>,
    e: &Expr,
    left_port: PortRef,
    right_port: PortRef,
) -> PortRef {
    binop_gate_of(ctx, e, Some(left_port), Some(right_port))
}

/// [`binop_gate`], an operand of `None` leaving its input unwired.
fn binop_gate_of(
    ctx: &mut LowerCtx<'_>,
    e: &Expr,
    left_port: Option<PortRef>,
    right_port: Option<PortRef>,
) -> PortRef {
    let Expr::BinOp { op, range, .. } = e else {
        return synthesise_unsupported(ctx, e);
    };
    let rule = match (left_port, right_port) {
        (Some(l), Some(r)) => mono_op_rule(ctx, e, op, &[l, r]),
        _ => ctx.op_for(e).cloned(),
    };
    let Some(rule) = rule else {
        return synthesise_unsupported(ctx, e);
    };

    // An object operand of a Math* gate goes through `(obj || false)` first
    // (see `wrap_object_for_math`): `1 + player` becomes `add(1, or(player, false))`.
    let operand = |ctx: &mut LowerCtx<'_>, port: Option<PortRef>, ty: &Type| match port {
        Some(p) => {
            let (p, t) = wrap_object_for_math(ctx, p, ty, rule.gate_class, range);
            (Some(p), t)
        }
        None => (None, ty.clone()),
    };
    let (left_port, left_ty) = operand(ctx, left_port, &rule.operands[0]);
    let (right_port, right_ty) = operand(ctx, right_port, &rule.operands[1]);

    rule_gate(
        ctx,
        &rule,
        &[left_port, right_port],
        &[left_ty, right_ty],
        None,
        range,
    )
}

/// An object operand does not coerce to a number on a math gate, so it is
/// routed through `(obj || false)`, a `LogicalOR` whose output the gate
/// accepts. Returns the port and its wire type. Non-object operands and
/// non-math gates (logical and compare gates take objects natively) pass through.
fn wrap_object_for_math(
    ctx: &mut LowerCtx<'_>,
    port: PortRef,
    operand_ty: &Type,
    gate_class: &str,
    range: &SourceRange,
) -> (PortRef, Type) {
    let is_object = matches!(
        operand_ty,
        Type::Entity | Type::Controller | Type::Character
    );
    let is_math = gate_class.starts_with(gc::EXPR_MATH_PREFIX);
    if !(is_object && is_math) {
        return (port, operand_ty.clone());
    }

    // `false` literal feeding the OR's second input.
    let false_port = raw_literal(ctx, range, Type::Bool, Literal::Bool(false), None);

    // (obj || false) -> LogicalOR coerces the object to an int/bool the gate takes.
    let or_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LOGICAL_OR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::new(WirePort::BInputA.sym(), operand_ty.clone()),
                PortSpec::new(WirePort::BInputB.sym(), Type::Bool),
            ],
            outputs: vec![PortSpec::new(WirePort::BOutput.sym(), Type::Bool)],
        },
        note: None,
        ..Default::default()
    });
    ctx.connect(port, or_id.port(WirePort::BInputA));
    ctx.connect(false_port, or_id.port(WirePort::BInputB));
    // The math input is declared Int (not Bool): the OR output coerces to the
    // gate's PrimMath variant at the wire level, and an Int default keeps the
    // gate's variant field valid (a Bool default is rejected by PrimMathVariant).
    (or_id.port(WirePort::BOutput), Type::Int)
}

pub(super) fn lower_unop(ctx: &mut LowerCtx<'_>, e: &Expr) -> PortRef {
    let Expr::UnOp { op, operand, range } = e else {
        return synthesise_unsupported(ctx, e);
    };
    // Fuse !(a && b) -> LogicalNAND, !(a || b) -> LogicalNOR
    // Fuse ~(a & b) -> BitwiseNAND,  ~(a | b) -> BitwiseNOR
    if (op == "!" || op == "~")
        && let Expr::BinOp {
            op: inner_op,
            left,
            right,
            range: inner_range,
            ..
        } = operand.as_ref()
    {
        let fused = match (op.as_str(), inner_op.as_str()) {
            ("!", "&&") => Some((
                gc::LOGICAL_NAND,
                WirePort::BInputA,
                WirePort::BInputB,
                WirePort::BOutput,
            )),
            ("!", "||") => Some((
                gc::LOGICAL_NOR,
                WirePort::BInputA,
                WirePort::BInputB,
                WirePort::BOutput,
            )),
            ("~", "&") => Some((
                gc::BITWISE_NAND,
                WirePort::InputA,
                WirePort::InputB,
                WirePort::Output,
            )),
            ("~", "|") => Some((
                gc::BITWISE_NOR,
                WirePort::InputA,
                WirePort::InputB,
                WirePort::Output,
            )),
            _ => None,
        };
        if let Some((gate_class, in_a, in_b, out_port)) = fused {
            let lhs = lower_expr(ctx, left);
            let rhs = lower_expr(ctx, right);
            let inner_rule = ctx.op_for(operand).cloned();
            let (lhs_ty, rhs_ty, out_ty) = if let Some(r) = &inner_rule {
                (
                    r.operands[0].clone(),
                    r.operands[1].clone(),
                    r.result.clone(),
                )
            } else {
                (Type::Any, Type::Any, Type::Any)
            };
            let in_a_sym = in_a.sym();
            let in_b_sym = in_b.sym();
            let out_sym = out_port.sym();
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class,
                source_range: inner_range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec::new(in_a_sym, lhs_ty),
                        PortSpec::new(in_b_sym, rhs_ty),
                    ],
                    outputs: vec![PortSpec::new(out_sym, out_ty)],
                },
                note: None,
                ..Default::default()
            });
            ctx.connect(lhs, node_id.port(in_a));
            ctx.connect(rhs, node_id.port(in_b));
            return node_id.port(out_port);
        }
    }
    if ctx.mono_stack.is_empty() && ctx.op_for(e).is_none() {
        return synthesise_unsupported(ctx, e);
    }
    // A negated integer constant (`--2`, `-(-K)`, `-K`) is the integer it
    // names, in every fold mode: the game's MathNegate answers an integer
    // with a float. One computed through an integer operation the certified
    // fold table has no result for keeps its gates.
    if op == "-"
        && ctx
            .op_for(e)
            .is_some_and(|r| r.operands.first() == Some(&Type::Int))
        && let (Ok(Literal::Int(n)), budget) = ctx.const_eval_budget(e, ConstScope::Certified)
        && !budget.uncertified
    {
        return literal_node_range(ctx, range, Type::Int, Literal::Int(n));
    }
    let in_port = lower_expr(ctx, operand);
    let Some(rule) = mono_op_rule(ctx, e, op, &[in_port]) else {
        return synthesise_unsupported(ctx, e);
    };
    rule_gate(ctx, &rule, &[Some(in_port)], rule.operands, None, range)
}

/// Build one `FormatText` gate from a finished format string and its already
/// lowered substitution ports (`slots.len()` must be `<= FORMAT_SLOTS.len()`).
pub(super) fn build_format_text(
    ctx: &mut LowerCtx<'_>,
    format_string: String,
    slots: Vec<PortRef>,
    range: &SourceRange,
) -> PortRef {
    let inputs = (0..slots.len())
        .map(|i| PortSpec::new(FORMAT_SLOTS[i].sym(), Type::Any))
        .collect();
    let mut props = HashMap::default();
    props.insert(
        intern_static("FormatString"),
        Literal::String(format_string),
    );
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::STRING_FORMAT_TEXT,
        source_range: range.clone(),
        ports: GateIO {
            inputs,
            outputs: vec![PortSpec::new(*sym::OUTPUT, Type::String)],
        },
        properties: props,
        ..Default::default()
    });
    for (i, slot) in slots.into_iter().enumerate() {
        ctx.connect(slot, node_id.port(FORMAT_SLOTS[i]));
    }
    node_id.port(WirePort::Output)
}

/// Concatenate string ports into one, chaining `FormatText` gates in groups of
/// `FORMAT_SLOTS.len()` (recursively) so any number of parts fits the 7-input
/// gate.
fn concat_string_ports(
    ctx: &mut LowerCtx<'_>,
    ports: Vec<PortRef>,
    range: &SourceRange,
) -> PortRef {
    if ports.len() <= 1 {
        return ports.into_iter().next().expect("at least one part");
    }
    let mut outs: Vec<PortRef> = Vec::new();
    for chunk in ports.chunks(FORMAT_SLOTS.len()) {
        if chunk.len() == 1 {
            outs.push(chunk[0]);
        } else {
            let fmt = (0..chunk.len()).fold(String::new(), |mut fmt, i| {
                let _ = write!(fmt, "{{{i}}}");
                fmt
            });
            outs.push(build_format_text(ctx, fmt, chunk.to_vec(), range));
        }
    }
    concat_string_ports(ctx, outs, range)
}

pub(super) fn lower_interp(
    ctx: &mut LowerCtx<'_>,
    parts: &[InterpPart],
    range: &SourceRange,
) -> PortRef {
    // A FormatText gate has only 7 substitution inputs, so a template with more
    // than 7 `${...}` values is split across several gates whose outputs are
    // concatenated. Literals attach to the current group; a group is flushed
    // once it holds `max` substitutions and another arrives. Parts stay in
    // order, so the concatenation reads identically to one long template.
    // Every `${...}` is evaluated in order before the template is built.
    let exprs: Vec<&Expr> = parts
        .iter()
        .filter_map(|p| match p {
            InterpPart::Expr(e) => Some(e.as_ref()),
            InterpPart::Lit(_) => None,
        })
        .collect();
    let mut ports = ordered::lower_in_order(ctx, &exprs).into_iter();
    let Ok(groups) = interp_groups(parts, |_| Ok::<_, std::convert::Infallible>(ports.next()));
    let outs: Vec<PortRef> = groups
        .into_iter()
        .map(|(fmt, slots)| {
            build_format_text(ctx, fmt, slots.into_iter().flatten().collect(), range)
        })
        .collect();
    concat_string_ports(ctx, outs, range)
}
