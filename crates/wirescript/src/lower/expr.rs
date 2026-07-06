use super::*;

// ---------- expressions ----------

pub(super) fn lower_expr(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    match e {
        Expr::IntLit { value, .. } => literal_node(ctx, e, Type::Int, Literal::Int(*value)),
        Expr::FloatLit { value, .. } => literal_node(ctx, e, Type::Float, Literal::Float(*value)),
        Expr::BoolLit { value, .. } => literal_node(ctx, e, Type::Bool, Literal::Bool(*value)),
        Expr::StringLit { value, .. } => {
            literal_node(ctx, e, Type::String, Literal::String(value.clone()))
        }
        Expr::InterpLit { parts, range } => lower_interp(ctx, parts, range),
        Expr::Ident { name, range } => {
            if name == "_" {
                if let Some(port) = ctx.await_armed_port {
                    return port;
                }
            }
            lower_ident(ctx, name, range)
        }
        Expr::BinOp { .. } => lower_binop(ctx, e),
        Expr::UnOp { .. } => lower_unop(ctx, e),
        Expr::Deref { operand, range } => {
            if let Expr::Ident { name, .. } = operand.as_ref()
                && let Some(var_rec) = ctx.lookup_var(name).cloned()
            {
                let inner = var_rec.inner_type.clone();
                if let Some(exec) = ctx.current_exec {
                    let get_id = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::VAR_GET,
                        source_range: range.clone(),
                        ports: GateIO {
                            inputs: vec![
                                PortSpec {
                                    name: *sym::EXEC,
                                    ty: Type::Exec,
                                },
                                PortSpec {
                                    name: *sym::VAR_REF,
                                    ty: Type::Ref(Box::new(inner.clone())),
                                },
                            ],
                            outputs: vec![
                                PortSpec {
                                    name: *sym::VALUE,
                                    ty: inner.clone(),
                                },
                                PortSpec {
                                    name: *sym::EXEC_OUT,
                                    ty: Type::Exec,
                                },
                            ],
                        },
                        note: None,
                        ..Default::default()
                    });
                    ctx.connect(exec, get_id.port(WirePort::Exec));
                    ctx.connect(
                        var_rec.node_id.port(WirePort::VarRef),
                        get_id.port(WirePort::VarRef),
                    );
                    ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
                    return get_id.port(WirePort::Value);
                }
                ctx.warn(
                    format!(
                        "'*{}' deref requires exec context — use .Value for pure reads",
                        name
                    ),
                    range,
                );
                return var_rec.node_id.port(WirePort::Value);
            }
            lower_expr(ctx, operand)
        }
        Expr::TuplePick { range, .. } => {
            if let Some(binding) = resolve_field_chain(ctx, e).cloned()
                && let Some(port) = binding_to_port(ctx, &binding, range)
            {
                return port;
            }
            synthesise_unsupported(ctx, e)
        }
        Expr::FieldAccess { obj, field, range } => lower_field_access(ctx, obj, field, range, e),
        Expr::IndexAccess { obj, index, range } => lower_index_access(ctx, obj, index, range, e),
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range,
        } => lower_if_expr(ctx, cond, then_branch, else_branch, range),
        Expr::BlockExpr { stmts, value, .. } => {
            ctx.scope.push(crate::scope::ScopeTag::BLOCK);
            for s in stmts {
                lower_stmt(ctx, s);
            }
            let result = lower_expr(ctx, value);
            ctx.scope.pop();
            result
        }
        Expr::Call { .. } => {
            // Constant constructor calls (`Vec/Rotation/Color` on literal
            // args) lower to a _Literal so consumers inline them as component
            // data; `materialize_unfoldable_constants` re-creates the Make*
            // gate for any consumer that can't absorb an inlined value.
            if let Some(lit) = expr_to_literal(e) {
                let ty = match &lit {
                    Literal::Vector { .. } => Some(Type::Vector),
                    Literal::Rotator { .. } => Some(Type::Rotator),
                    Literal::LinearColor { .. } => Some(Type::Color),
                    _ => None,
                };
                if let Some(ty) = ty {
                    return literal_node(ctx, e, ty, lit);
                }
            }
            lower_call(ctx, e)
        }
        Expr::RecordLit { range, .. } => {
            // Record literals are handled in lower_let_decl, not as standalone expressions.
            synthesise_unsupported_range(ctx, range)
        }
        _ => synthesise_unsupported(ctx, e),
    }
}

pub(super) fn literal_node(ctx: &mut LowerCtx, e: &Expr, ty: Type, lit: Literal) -> PortRef {
    // String literals can't be inlined as wire_graph_variant immediate values
    // on consumer gates (e.g. Select). Emit them as String_Concatenate gates
    // whose str-typed fields accept inline strings, producing a wire signal.
    if let Literal::String(ref s) = lit {
        let mut props = HashMap::new();
        props.insert(*sym::INPUT_A, Literal::String(s.clone()));
        props.insert(*sym::INPUT_B, Literal::String(String::new()));
        props.insert(intern_static("Separator"), Literal::String(String::new()));
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::STRING_CONCATENATE,
            source_range: e.range().clone(),
            ports: GateIO {
                inputs: vec![],
                outputs: vec![PortSpec {
                    name: *sym::OUTPUT,
                    ty: Type::String,
                }],
            },
            properties: props,
            ..Default::default()
        });
        return node_id.port(WirePort::Output);
    }
    let mut props = HashMap::new();
    props.insert(*sym::VALUE, lit);
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: e.range().clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty,
            }],
        },
        properties: props,
        ..Default::default()
    });
    node_id.port(WirePort::Output)
}

pub(super) fn lower_ident(ctx: &mut LowerCtx, name: &str, range: &SourceRange) -> PortRef {
    let binding = ctx.scope.get(name).cloned();
    match binding {
        Some(Binding::Var(var_rec)) => {
            if var_rec.storage == VarStorage::Buffer {
                return var_rec.node_id.port(WirePort::Output);
            }
            if var_rec.storage == VarStorage::Array {
                return var_rec.node_id.port(WirePort::ArrayVarRef);
            }
            if let Some(exec) = ctx.current_exec {
                if let Some(cached) = var_rec.get_node_for_handler {
                    return cached.port(WirePort::Value);
                }
                let inner = var_rec.inner_type.clone();
                let mut get_props = HashMap::new();
                if let Some(lit) = default_literal_for_var_type(&inner) {
                    get_props.insert(*sym::VALUE, lit);
                }
                let get_id = ctx.add_gate(AddNodeOpts {
                    gate_class: gc::VAR_GET,
                    source_range: range.clone(),
                    properties: get_props,
                    ports: GateIO {
                        inputs: vec![
                            PortSpec {
                                name: *sym::EXEC,
                                ty: Type::Exec,
                            },
                            PortSpec {
                                name: *sym::VAR_REF,
                                ty: Type::Ref(Box::new(inner.clone())),
                            },
                        ],
                        outputs: vec![
                            PortSpec {
                                name: *sym::VALUE,
                                ty: inner.clone(),
                            },
                            PortSpec {
                                name: *sym::EXEC_OUT,
                                ty: Type::Exec,
                            },
                        ],
                    },
                    note: None,
                    ..Default::default()
                });
                ctx.connect(exec, get_id.port(WirePort::Exec));
                ctx.connect(
                    var_rec.node_id.port(WirePort::VarRef),
                    get_id.port(WirePort::VarRef),
                );
                ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
                if let Some(Binding::Var(v)) = ctx.scope.get_mut(name) {
                    v.get_node_for_handler = Some(get_id);
                }
                return get_id.port(WirePort::Value);
            }
            var_rec.node_id.port(WirePort::Value)
        }
        Some(Binding::Buffer(buf)) => buf.node_id.port(WirePort::Output),
        Some(Binding::Input(inp)) => inp.node_id.port(WirePort::RerOutput),
        Some(Binding::EventParam(p)) => p,
        Some(Binding::Local(local)) => local.port,
        Some(Binding::Record(_)) => {
            // Records are compile-time bundles; they don't produce a single port.
            // Field access on records is handled in lower_field_access.
            synthesise_unsupported_range(ctx, range)
        }
        Some(Binding::Output(_) | Binding::Chip(_) | Binding::Namespace(_)) => {
            synthesise_unsupported_range(ctx, range)
        }
        None => synthesise_unsupported_range(ctx, range),
    }
}

pub(super) fn lower_if_expr(
    ctx: &mut LowerCtx,
    cond: &Expr,
    then_br: &Expr,
    else_br: &Expr,
    range: &SourceRange,
) -> PortRef {
    let cond_port = lower_expr(ctx, cond);
    let then_port = lower_expr(ctx, then_br);
    let else_port = lower_expr(ctx, else_br);
    let result_ty = unwrap_ref(&ctx.type_of(else_br));
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::SELECT,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::INPUT_A,
                    ty: result_ty.clone(),
                },
                PortSpec {
                    name: *sym::INPUT_B,
                    ty: result_ty.clone(),
                },
                PortSpec {
                    name: *sym::B_SELECT_B,
                    ty: Type::Bool,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: result_ty.clone(),
            }],
        },
        note: Some("if-expr select".into()),
        ..Default::default()
    });
    ctx.connect(cond_port, node_id.port(WirePort::BSelectB));
    ctx.connect(then_port, node_id.port(WirePort::InputB));
    ctx.connect(else_port, node_id.port(WirePort::InputA));
    node_id.port(WirePort::Output)
}

pub(super) fn literal_for_property_port(e: &Expr, _port_ty: &Type) -> Option<Literal> {
    // Return the literal as-is without type promotion. The emit layer
    // handles the native type (i32/f64/str) based on the data struct schema.
    expr_to_literal(e)
}

pub(super) fn synthesise_unsupported(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    synthesise_unsupported_range(ctx, e.range())
}

pub(super) fn synthesise_unsupported_range(ctx: &mut LowerCtx, range: &SourceRange) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNSUPPORTED,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::Any,
            }],
        },
        note: Some("unsupported expression".into()),
        ..Default::default()
    });
    ctx.warn(
        "IR lowering not yet supported for this expression — emitted placeholder",
        range,
    );
    node_id.port(WirePort::Output)
}
