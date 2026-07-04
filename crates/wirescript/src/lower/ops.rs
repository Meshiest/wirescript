use super::*;

pub(super) fn lower_binop(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    let (_op, left, right, range) = match e {
        Expr::BinOp {
            op,
            left,
            right,
            range,
        } => (op, left, right, range),
        _ => return synthesise_unsupported(ctx, e),
    };
    let rule = match ctx.op_for(e).cloned() {
        Some(r) => r,
        None => return synthesise_unsupported(ctx, e),
    };
    let left_port = lower_expr(ctx, left);
    let right_port = lower_expr(ctx, right);

    // Players (and other objects) no longer cast directly to ints on a math gate,
    // so an object math operand is routed through `(obj || false)` first — that
    // LogicalOR coerces it to a value the gate accepts. `1 + player` becomes
    // `add(1, or(player, false))`. Only applies to the Math* gates; logical and
    // comparison gates take object operands natively.
    let (left_port, left_ty) =
        wrap_object_for_math(ctx, left_port, &rule.operands[0], rule.gate_class, range);
    let (right_port, right_ty) =
        wrap_object_for_math(ctx, right_port, &rule.operands[1], rule.gate_class, range);

    let in_a = rule.ports.inputs[0];
    let in_b = rule.ports.inputs[1];
    let out = rule.ports.output;
    let in_a_sym = intern(in_a.as_str());
    let in_b_sym = intern(in_b.as_str());
    let out_sym = intern(out.as_str());
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: rule.gate_class,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: in_a_sym,
                    ty: left_ty,
                },
                PortSpec {
                    name: in_b_sym,
                    ty: right_ty,
                },
            ],
            outputs: vec![
                PortSpec {
                    name: out_sym,
                    ty: rule.result.clone(),
                },
            ],
        },
        note: None,
        ..Default::default()
    });
    ctx.connect(left_port, node_id.port(in_a));
    ctx.connect(right_port, node_id.port(in_b));
    node_id.port(out)
}

/// Players and other object operands no longer coerce directly to ints on a math
/// gate, so route an object operand through `(obj || false)` — a LogicalOR with a
/// `false` literal — which coerces it to a value the math gate accepts. Returns
/// the (possibly rewritten) port and its effective wire type. No-op for
/// non-object operands or non-math gates (logical/compare gates take objects
/// natively).
fn wrap_object_for_math(
    ctx: &mut LowerCtx,
    port: PortRef,
    operand_ty: &Type,
    gate_class: &str,
    range: &SourceRange,
) -> (PortRef, Type) {
    let is_object = matches!(
        operand_ty,
        Type::Entity | Type::Controller | Type::Character | Type::Brick | Type::Prefab
    );
    let is_math = gate_class.starts_with("BrickComponentType_WireGraph_Expr_Math");
    if !(is_object && is_math) {
        return (port, operand_ty.clone());
    }

    // `false` literal feeding the OR's second input.
    let mut false_props = HashMap::new();
    false_props.insert(*sym::VALUE, Literal::Bool(false));
    let false_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::Bool,
            }],
        },
        properties: false_props,
        ..Default::default()
    });

    // (obj || false) → LogicalOR coerces the object to an int/bool the gate takes.
    let or_id = ctx.add_gate(AddNodeOpts {
        gate_class: "BrickComponentType_WireGraph_Expr_LogicalOR",
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: intern(WirePort::BInputA.as_str()),
                    ty: operand_ty.clone(),
                },
                PortSpec {
                    name: intern(WirePort::BInputB.as_str()),
                    ty: Type::Bool,
                },
            ],
            outputs: vec![PortSpec {
                name: intern(WirePort::BOutput.as_str()),
                ty: Type::Bool,
            }],
        },
        note: None,
        ..Default::default()
    });
    ctx.connect(port, or_id.port(WirePort::BInputA));
    ctx.connect(false_id.port(WirePort::Output), or_id.port(WirePort::BInputB));
    // The math input is declared Int (not Bool): the OR output coerces to the
    // gate's PrimMath variant at the wire level, and an Int default keeps the
    // gate's variant field valid (a Bool default is rejected by PrimMathVariant).
    (or_id.port(WirePort::BOutput), Type::Int)
}

pub(super) fn lower_unop(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    let (op, operand, range) = match e {
        Expr::UnOp { op, operand, range } => (op, operand, range),
        _ => return synthesise_unsupported(ctx, e),
    };
    // Fuse !(a && b) → LogicalNAND, !(a || b) → LogicalNOR
    // Fuse ~(a & b) → BitwiseNAND,  ~(a | b) → BitwiseNOR
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
                "BrickComponentType_WireGraph_Expr_LogicalNAND",
                WirePort::BInputA,
                WirePort::BInputB,
                WirePort::BOutput,
            )),
            ("!", "||") => Some((
                "BrickComponentType_WireGraph_Expr_LogicalNOR",
                WirePort::BInputA,
                WirePort::BInputB,
                WirePort::BOutput,
            )),
            ("~", "&") => Some((
                "BrickComponentType_WireGraph_Expr_BitwiseNAND",
                WirePort::InputA,
                WirePort::InputB,
                WirePort::Output,
            )),
            ("~", "|") => Some((
                "BrickComponentType_WireGraph_Expr_BitwiseNOR",
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
            let in_a_sym = intern(in_a.as_str());
            let in_b_sym = intern(in_b.as_str());
            let out_sym = intern(out_port.as_str());
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class,
                source_range: inner_range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: in_a_sym,
                            ty: lhs_ty.clone(),
                        },
                        PortSpec {
                            name: in_b_sym,
                            ty: rhs_ty.clone(),
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: out_sym,
                            ty: out_ty.clone(),
                        },
                    ],
                },
                note: None,
                ..Default::default()
            });
            ctx.connect(lhs, node_id.port(in_a));
            ctx.connect(rhs, node_id.port(in_b));
            return node_id.port(out_port);
        }
    }
    let rule = match ctx.op_for(e).cloned() {
        Some(r) => r,
        None => return synthesise_unsupported(ctx, e),
    };
    let in_port = lower_expr(ctx, operand);
    let in_a = rule.ports.inputs[0];
    let out = rule.ports.output;
    let in_a_sym = intern(in_a.as_str());
    let out_sym = intern(out.as_str());
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: rule.gate_class,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: in_a_sym,
                    ty: rule.operands[0].clone(),
                },
            ],
            outputs: vec![
                PortSpec {
                    name: out_sym,
                    ty: rule.result.clone(),
                },
            ],
        },
        note: None,
        ..Default::default()
    });
    ctx.connect(in_port, node_id.port(in_a));
    node_id.port(out)
}

pub(super) fn lower_interp(
    ctx: &mut LowerCtx,
    parts: &[InterpPart],
    range: &SourceRange,
) -> PortRef {
    const SLOT_LETTERS: [WirePort; 7] = [
        WirePort::InputA, WirePort::InputB, WirePort::InputC,
        WirePort::InputD, WirePort::InputE, WirePort::InputF,
        WirePort::InputG,
    ];
    let mut slots: Vec<PortRef> = Vec::new();
    let mut format_string = String::new();
    for p in parts {
        match p {
            InterpPart::Lit(s) => {
                format_string.push_str(&s.replace('{', "{{").replace('}', "}}"));
            }
            InterpPart::Expr(expr) => {
                if SLOT_LETTERS.get(slots.len()).is_some() {
                    format_string.push_str(&format!("{{{}}}", slots.len()));
                    slots.push(lower_expr(ctx, expr));
                }
            }
        }
    }
    let mut inputs = Vec::new();
    for (i, _) in slots.iter().enumerate() {
        if let Some(&port_id) = SLOT_LETTERS.get(i) {
            inputs.push(PortSpec {
                name: intern(port_id.as_str()),
                ty: Type::Any,
            });
        }
    }
    let mut props = HashMap::new();
    props.insert(
        intern_static("FormatString"),
        Literal::String(format_string),
    );
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::STRING_FORMAT_TEXT,
        source_range: range.clone(),
        ports: GateIO {
            inputs,
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::String,
            }],
        },
        properties: props,
        ..Default::default()
    });
    for (i, slot) in slots.into_iter().enumerate() {
        if let Some(&port_id) = SLOT_LETTERS.get(i) {
            ctx.connect(slot, node_id.port(port_id));
        }
    }
    node_id.port(WirePort::Output)
}
