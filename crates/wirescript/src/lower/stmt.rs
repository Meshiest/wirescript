use super::*;

pub(super) fn lower_stmt(ctx: &mut LowerCtx, s: &Stmt) {
    match s {
        Stmt::Assign(a) => lower_assign(ctx, a),
        Stmt::If(i) => lower_if(ctx, i),
        Stmt::Emit(e) => lower_emit(ctx, e),
        Stmt::Await(a) => lower_await(ctx, a),
        Stmt::OutBinding(b) => lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range),
        Stmt::ExprStmt(es) => {
            lower_expr(ctx, &es.expr);
        }
        Stmt::Handler(h) => lower_handler(ctx, h),
        Stmt::Let(l) => lower_let_decl(ctx, l),
        Stmt::AnonChip(ac) => lower_anon_chip(ctx, ac),
        Stmt::ChipDecl(c) => lower_chip_decl(ctx, c),
        Stmt::In(_) => {}
        Stmt::Var(v) => {
            if ctx.lookup_var(&v.name).is_none() {
                pre_declare_var(ctx, v);
            }
            // In exec context, emit a Var_Set to reset the variable to its
            // initial value each time this scope is entered. Without this,
            // PseudoVar keeps its value from the previous invocation.
            // `static var` skips this — it retains its value across calls.
            if !v.is_static
                && let Some(exec) = ctx.current_exec
                && let Some(var_rec) = ctx.lookup_var(&v.name).cloned()
            {
                let init_val = v.init.as_ref().map(|e| lower_expr(ctx, e)).or_else(|| {
                    default_literal_for_var_type(&var_rec.inner_type).map(|lit| {
                        let lit_id = ctx.add_gate(AddNodeOpts {
                            gate_class: gc::LITERAL,
                            source_range: v.range.clone(),
                            properties: {
                                let mut p = HashMap::new();
                                p.insert(*sym::VALUE, lit);
                                p
                            },
                            ports: GateIO {
                                inputs: vec![],
                                outputs: vec![PortSpec {
                                    name: *sym::OUTPUT,
                                    ty: var_rec.inner_type.clone(),
                                }],
                            },
                            ..Default::default()
                        });
                        lit_id.port(WirePort::Output)
                    })
                });
                if let Some(val_port) = init_val {
                    let exec_in = ctx.current_exec.unwrap_or(exec);
                    let inner = var_rec.inner_type.clone();
                    let set_node = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::VAR_SET,
                        source_range: v.range.clone(),
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
                                PortSpec {
                                    name: *sym::VALUE,
                                    ty: inner.clone(),
                                },
                            ],
                            outputs: vec![
                                PortSpec {
                                    name: *sym::EXEC_OUT,
                                    ty: Type::Exec,
                                },
                            ],
                        },
                        ..Default::default()
                    });
                    ctx.connect(exec_in, set_node.port(WirePort::Exec));
                    ctx.connect(
                        var_rec.node_id.port(WirePort::VarRef),
                        set_node.port(WirePort::VarRef),
                    );
                    ctx.connect(val_port, set_node.port(WirePort::Value));
                    ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
                }
            } // !is_static
        }
        Stmt::Array(a) => {
            if ctx.lookup_var(&a.name).is_none() {
                pre_declare_array(ctx, a);
            }
        }
        Stmt::Buffer(b) => {
            if ctx.lookup_buffer(&b.name).is_none() {
                pre_declare_buffer(ctx, b);
            }
        }
        Stmt::Return { value, .. } => {
            if let Some(expr) = value {
                let val_port = lower_expr(ctx, expr);
                let ret_var = ctx.mod_return_var.clone();
                if let Some(ref var_rec) = ret_var {
                    // Multi-return: Var_Set to the return var
                    if let Some(exec) = ctx.current_exec {
                        let inner = var_rec.inner_type.clone();
                        let set_node = ctx.add_gate(AddNodeOpts {
                            gate_class: gc::VAR_SET,
                            source_range: SourceRange::default(),
                            note: Some("ret_set"),
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
                                    PortSpec {
                                        name: *sym::VALUE,
                                        ty: inner.clone(),
                                    },
                                ],
                                outputs: vec![
                                    PortSpec {
                                        name: *sym::EXEC_OUT,
                                        ty: Type::Exec,
                                    },
                                ],
                            },
                            ..Default::default()
                        });
                        ctx.connect(exec, set_node.port(WirePort::Exec));
                        ctx.connect(
                            var_rec.node_id.port(WirePort::VarRef),
                            set_node.port(WirePort::VarRef),
                        );
                        ctx.connect(val_port, set_node.port(WirePort::Value));
                        ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
                    }
                } else if ctx.output_count() == 1 {
                    // Single return: wire directly to output
                    let out = ctx.first_output().unwrap().1.clone();
                    ctx.connect(val_port, out.node_id.port(WirePort::RerInput));
                }
            }
            if let Some(exec) = ctx.current_exec.take() {
                if ctx.mod_return_exec.is_some() {
                    let prev = ctx.mod_return_exec.take().unwrap();
                    let union = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::UNION,
                        source_range: SourceRange::default(),
                        ports: GateIO {
                            inputs: vec![
                                PortSpec {
                                    name: *sym::EXEC_A,
                                    ty: Type::Exec,
                                },
                                PortSpec {
                                    name: *sym::EXEC_B,
                                    ty: Type::Exec,
                                },
                            ],
                            outputs: vec![
                                PortSpec {
                                    name: *sym::EXEC_OUT,
                                    ty: Type::Exec,
                                },
                            ],
                        },
                        ..Default::default()
                    });
                    ctx.connect(prev, union.port(WirePort::ExecA));
                    ctx.connect(exec, union.port(WirePort::ExecB));
                    ctx.mod_return_exec = Some(union.port(WirePort::ExecOut));
                } else {
                    ctx.mod_return_exec = Some(exec);
                }
            }
        }
    }
}

pub(super) fn count_return_values(block: &Block) -> usize {
    let mut count = 0;
    for stmt in &block.stmts {
        match stmt {
            Stmt::Return { value: Some(_), .. } => count += 1,
            Stmt::If(if_stmt) => {
                count += count_return_values(&if_stmt.then_block);
                if let Some(else_block) = &if_stmt.else_block {
                    count += count_return_values(else_block);
                }
            }
            _ => {}
        }
    }
    count
}

pub(super) fn block_contains_return(block: &Block) -> bool {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Return { .. } => return true,
            Stmt::If(if_stmt) => {
                if block_contains_return(&if_stmt.then_block) {
                    return true;
                }
                if let Some(else_block) = &if_stmt.else_block
                    && block_contains_return(else_block)
                {
                    return true;
                }
            }
            // Don't recurse into nested handlers — a return inside
            // `on trigger { return }` is that handler's return, not the mod's.
            Stmt::Handler(_) => {}
            _ => {}
        }
    }
    false
}

pub(super) fn match_increment_self(s: &Assign) -> Option<&Expr> {
    let name = match &s.target {
        Expr::Ident { name, .. } => name,
        _ => return None,
    };
    match &s.value {
        Expr::BinOp {
            op, left, right, ..
        } if op == "+" => {
            if matches!(left.as_ref(), Expr::Ident { name: n, .. } if n == name) {
                return Some(right);
            }
            if matches!(right.as_ref(), Expr::Ident { name: n, .. } if n == name) {
                return Some(left);
            }
            None
        }
        _ => None,
    }
}

pub(super) fn lower_assign(ctx: &mut LowerCtx, s: &Assign) {
    if let Expr::IndexAccess { obj, index, .. } = &s.target {
        lower_array_set(ctx, obj, index, &s.value, &s.range);
        return;
    }

    // Handle field-access targets that resolve through records to vars.
    // e.g. `cpu.x = 5` where `cpu` is a record and `cpu.x` is a Var binding.
    if let Expr::FieldAccess { .. } = &s.target
        && let Some(binding) = resolve_field_chain(ctx, &s.target).cloned()
        && let Binding::Var(var_rec) = binding
    {
        let current_exec = match ctx.current_exec {
            Some(e) => e,
            None => return,
        };
        if var_rec.storage == VarStorage::Buffer {
            let value_port = lower_expr(ctx, &s.value);
            ctx.connect(value_port, var_rec.node_id.port(WirePort::Input));
            return;
        }
        let value_port = lower_expr(ctx, &s.value);
        let exec_in = ctx.current_exec.unwrap_or(current_exec);
        let inner = var_rec.inner_type.clone();
        let set_node = ctx.add_gate(AddNodeOpts {
            gate_class: gc::VAR_SET,
            source_range: s.range.clone(),
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
                    PortSpec {
                        name: *sym::VALUE,
                        ty: inner.clone(),
                    },
                ],
                outputs: vec![
                    PortSpec {
                        name: *sym::EXEC_OUT,
                        ty: Type::Exec,
                    },
                ],
            },
            note: None,
            ..Default::default()
        });
        ctx.connect(exec_in, set_node.port(WirePort::Exec));
        ctx.connect(
            var_rec.node_id.port(WirePort::VarRef),
            set_node.port(WirePort::VarRef),
        );
        ctx.connect(value_port, set_node.port(WirePort::Value));
        ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
        invalidate_var_cache(ctx, &var_rec.node_id);
        return;
    }

    let var_name = match &s.target {
        Expr::Ident { name, .. } => name.clone(),
        _ => return,
    };
    let var_rec = match ctx.lookup_var(&var_name).cloned() {
        Some(v) => v,
        None => return,
    };
    let current_exec = match ctx.current_exec {
        Some(e) => e,
        None => return,
    };

    // `foo = [items, ...spreads]` on an array var: rebuild the contents at
    // runtime. There's no single "set array" gate, so clear it then push each
    // item / append each spread in order.
    if var_rec.storage == VarStorage::Array
        && let Expr::Array { elements, .. } = &s.value
    {
        lower_array_literal_assign(ctx, &var_rec, elements, &s.range, &s.value);
        return;
    }

    // Buffer-backed (entity-family) var: wire value directly into Input.
    if var_rec.storage == VarStorage::Buffer {
        let value_port = lower_expr(ctx, &s.value);
        ctx.connect(value_port, var_rec.node_id.port(WirePort::Input));
        return;
    }

    // Optimization: `x = x + <expr>` → Exec_Var_Increment
    if let Some(delta_expr) = match_increment_self(s) {
        let delta = lower_expr(ctx, delta_expr);
        // Re-read current_exec after lowering the delta — lower_expr may
        // have advanced the chain via Var_Get / nested exec-taking ops.
        let exec_in = ctx.current_exec.unwrap_or(current_exec);
        let inner = var_rec.inner_type.clone();
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::VAR_INCREMENT,
            source_range: s.range.clone(),
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
                    PortSpec {
                        name: *sym::VALUE,
                        ty: inner.clone(),
                    },
                ],
                outputs: vec![
                    PortSpec {
                        name: *sym::EXEC_OUT,
                        ty: Type::Exec,
                    },
                ],
            },
            note: None,
            ..Default::default()
        });
        ctx.connect(exec_in, node_id.port(WirePort::Exec));
        ctx.connect(
            var_rec.node_id.port(WirePort::VarRef),
            node_id.port(WirePort::VarRef),
        );
        ctx.connect(delta, node_id.port(WirePort::Value));
        ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
        invalidate_var_cache(ctx, &var_rec.node_id);
        return;
    }

    // General assignment: Exec_Var_Set
    let value_port = lower_expr(ctx, &s.value);
    // Re-read current_exec after lowering the RHS — lower_expr may have
    // advanced the chain via Var_Get / nested exec-taking ops.
    let exec_in = ctx.current_exec.unwrap_or(current_exec);
    let inner = var_rec.inner_type.clone();
    let set_node = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_SET,
        source_range: s.range.clone(),
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
                PortSpec {
                    name: *sym::VALUE,
                    ty: inner.clone(),
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                },
            ],
        },
        note: None,
        ..Default::default()
    });
    ctx.connect(exec_in, set_node.port(WirePort::Exec));
    ctx.connect(
        var_rec.node_id.port(WirePort::VarRef),
        set_node.port(WirePort::VarRef),
    );
    ctx.connect(value_port, set_node.port(WirePort::Value));
    ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
    invalidate_var_cache(ctx, &var_rec.node_id);
}

/// Lower `foo = [items, ...spreads]` on an array var: clear it, then push each
/// item and append each spread, in order. Reuses the array-method lowering so
/// the gates and exec chaining match `foo.clear()` / `.push()` / `.append()`.
fn lower_array_literal_assign(
    ctx: &mut LowerCtx,
    var_rec: &VarRecord,
    elements: &[ArrayElem],
    range: &SourceRange,
    value: &Expr,
) {
    lower_array_method(ctx, var_rec, "clear", &[], range, value);
    for el in elements {
        let arg = [CallArg::Positional(el.expr().clone())];
        let method = match el {
            ArrayElem::Item(_) => "push",
            ArrayElem::Spread(_) => "append",
        };
        lower_array_method(ctx, var_rec, method, &arg, el.range(), value);
    }
}

pub(super) fn lower_if(ctx: &mut LowerCtx, s: &If) {
    if ctx.current_exec.is_none() {
        return;
    }

    // Constant-fold: if the condition is a literal bool, skip the Branch
    // gate entirely and just emit the taken branch.
    if let Expr::BoolLit { value, .. } = &s.cond {
        ctx.scope.push(crate::scope::ScopeTag::BLOCK);
        if *value {
            lower_block(ctx, &s.then_block);
        } else if let Some(else_b) = &s.else_block {
            lower_block(ctx, else_b);
        }
        ctx.scope.pop();
        return;
    }
    // Also fold idents bound to literal bools (e.g. inline mod params)
    if let Expr::Ident { name, .. } = &s.cond
        && let Some(Binding::Local(local)) = ctx.scope.get(name).cloned()
        && let Some(node) = ctx.builder.module.nodes.get(&local.port.node_id)
        && node.gate_class == gc::LITERAL
        && let Some(Literal::Bool(val)) = node.properties.get(&*sym::VALUE)
    {
        let val = *val;
        ctx.scope.push(crate::scope::ScopeTag::BLOCK);
        if val {
            lower_block(ctx, &s.then_block);
        } else if let Some(else_b) = &s.else_block {
            lower_block(ctx, else_b);
        }
        ctx.scope.pop();
        return;
    }

    let current_exec = ctx.current_exec.unwrap();

    // Enter an IfGroup wrapping IfCond / IfThen / IfElse so layout sees
    // the branches as a unit. The union (join) gate after the branches
    // lives in the outer scope, not inside the group.
    let outer_scope = ctx.builder.current_scope_id;
    let if_group_id = ctx.alloc_scope(ScopeKind::IfGroup, s.range.clone());
    ctx.builder.current_scope_id = if_group_id;

    // IfCond — condition expression + branch gate.
    let cond_id = ctx.alloc_scope(ScopeKind::IfCond, s.cond.range().clone());
    ctx.builder.current_scope_id = cond_id;
    let cond_port = lower_expr(ctx, &s.cond);
    // Lowering the condition may have inserted Exec-taking gates (e.g.
    // `Var_Get`). In that case `ctx.current_exec` has advanced past the
    // handler's entry, and the branch's Exec input must pick up from
    // that new chain head — not from the entry-time `current_exec`.
    let branch_exec_in = ctx.current_exec.unwrap_or(current_exec);
    let branch = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BRANCH,
        source_range: s.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::B_COND,
                    ty: Type::Bool,
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::EXEC_OUT_A,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::EXEC_OUT_B,
                    ty: Type::Exec,
                },
            ],
        },
        ..Default::default()
    });
    ctx.connect(branch_exec_in, branch.port(WirePort::Exec));
    ctx.connect(cond_port, branch.port(WirePort::BCond));

    // Snapshot scope before branches so that declarations in one
    // branch don't leak into the other (each branch gets its own scope).
    ctx.scope.push(crate::scope::ScopeTag::BLOCK);

    // IfThen — sibling of IfCond under IfGroup.
    ctx.builder.current_scope_id = if_group_id;
    let then_id = ctx.alloc_scope(ScopeKind::IfThen, s.then_block.range.clone());
    ctx.builder.current_scope_id = then_id;
    ctx.current_exec = Some(branch.port(WirePort::ExecOutA));
    lower_block(ctx, &s.then_block);
    let then_end = ctx.current_exec;

    // Restore scope so the else branch starts from the same state.
    ctx.scope.pop();
    ctx.scope.push(crate::scope::ScopeTag::BLOCK);

    // IfElse — allocated even when the source has no `else`, so layout
    // always gets the triplet (empty IfElse regions compose as zero-width).
    ctx.builder.current_scope_id = if_group_id;
    let else_range = s
        .else_block
        .as_ref()
        .map(|b| b.range.clone())
        .unwrap_or_else(|| s.range.clone());
    let else_id = ctx.alloc_scope(ScopeKind::IfElse, else_range);
    ctx.builder.current_scope_id = else_id;
    ctx.current_exec = Some(branch.port(WirePort::ExecOutB));
    if let Some(else_b) = &s.else_block {
        lower_block(ctx, else_b);
    }
    let else_end = ctx.current_exec;

    // Restore scope so post-if code sees the pre-branch state.
    // Variables declared inside branches are not visible after the if.
    ctx.scope.pop();

    // The join/union below is post-branch flow; back to the outer scope.
    ctx.builder.current_scope_id = outer_scope;

    let union = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: s.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC_A,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::EXEC_B,
                    ty: Type::Exec,
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                },
            ],
        },
        ..Default::default()
    });
    if let Some(e) = then_end {
        ctx.connect(e, union.port(WirePort::ExecA));
    }
    if let Some(e) = else_end {
        ctx.connect(e, union.port(WirePort::ExecB));
    }
    ctx.current_exec = Some(union.port(WirePort::ExecOut));
}

pub(super) fn lower_emit(ctx: &mut LowerCtx, s: &Emit) {
    let is_output = ctx.lookup_output(&s.name).is_some();
    let is_local_exec = ctx.pending_emits.contains_key(&s.name);
    if !is_output && !is_local_exec {
        return;
    }

    if let Some(ref value_expr) = s.value {
        if let Some(out) = ctx.lookup_output(&s.name).cloned() {
            let value_port = lower_expr(ctx, value_expr);
            ctx.connect(value_port, out.node_id.port(WirePort::RerInput));
        }
        if let Some(current_exec) = ctx.current_exec {
            ctx.pending_emits
                .entry(s.name.clone())
                .or_default()
                .push(current_exec);
        }
    } else {
        let current_exec = match ctx.current_exec {
            Some(e) => e,
            None => return,
        };
        ctx.pending_emits
            .entry(s.name.clone())
            .or_default()
            .push(current_exec);
    }
}

pub(super) fn lower_await(ctx: &mut LowerCtx, a: &AwaitStmt) {
    // 1. Create a static bool var for the armed flag (initially false)
    let armed_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec { name: *sym::VALUE, ty: Type::Bool },
                PortSpec { name: *sym::VAR_REF, ty: Type::Ref(Box::new(Type::Bool)) },
            ],
        },
        properties: {
            let mut p = HashMap::new();
            p.insert(*sym::INITIAL_VALUE, Literal::Bool(false));
            p
        },
        note: Some("await armed flag"),
        ..Default::default()
    });

    // 2. Arm: Var_Set(armed = true) on the current exec chain
    if let Some(exec_in) = ctx.current_exec {
        let true_lit = ctx.add_gate(AddNodeOpts {
            gate_class: gc::LITERAL,
            source_range: a.range.clone(),
            ports: GateIO {
                inputs: vec![],
                outputs: vec![PortSpec { name: *sym::OUTPUT, ty: Type::Bool }],
            },
            properties: {
                let mut p = HashMap::new();
                p.insert(*sym::VALUE, Literal::Bool(true));
                p
            },
            ..Default::default()
        });
        let arm_set = ctx.add_gate(AddNodeOpts {
            gate_class: gc::VAR_SET,
            source_range: a.range.clone(),
            ports: GateIO {
                inputs: vec![
                    PortSpec { name: *sym::EXEC, ty: Type::Exec },
                    PortSpec { name: *sym::VAR_REF, ty: Type::Ref(Box::new(Type::Bool)) },
                    PortSpec { name: *sym::VALUE, ty: Type::Bool },
                ],
                outputs: vec![
                    PortSpec { name: *sym::EXEC_OUT, ty: Type::Exec },
                ],
            },
            ..Default::default()
        });
        ctx.connect(exec_in, arm_set.port(WirePort::Exec));
        ctx.connect(armed_id.port(WirePort::VarRef), arm_set.port(WirePort::VarRef));
        ctx.connect(true_lit.port(WirePort::Output), arm_set.port(WirePort::Value));
        // Exec chain ends here — pre-await code is done
    }

    // 3. Lower the exec expression (the trigger to wait for)
    // Set await_armed_port so `_` in the expression resolves to the armed flag's Value
    let saved_armed = ctx.await_armed_port;
    ctx.await_armed_port = Some(armed_id.port(WirePort::Value));
    let exec_port = lower_expr(ctx, &a.exec_expr);
    ctx.await_armed_port = saved_armed;

    // 4. Var_Get(armed) on the trigger's exec
    let get_armed = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_GET,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec { name: *sym::EXEC, ty: Type::Exec },
                PortSpec { name: *sym::VAR_REF, ty: Type::Ref(Box::new(Type::Bool)) },
            ],
            outputs: vec![
                PortSpec { name: *sym::VALUE, ty: Type::Bool },
                PortSpec { name: *sym::EXEC_OUT, ty: Type::Exec },
            ],
        },
        ..Default::default()
    });
    ctx.connect(exec_port, get_armed.port(WirePort::Exec));
    ctx.connect(armed_id.port(WirePort::VarRef), get_armed.port(WirePort::VarRef));

    // 5. Branch on armed flag — true branch continues, false drops
    let branch = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BRANCH,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec { name: *sym::EXEC, ty: Type::Exec },
                PortSpec { name: *sym::B_COND, ty: Type::Bool },
            ],
            outputs: vec![
                PortSpec { name: *sym::EXEC_OUT_A, ty: Type::Exec },
                PortSpec { name: *sym::EXEC_OUT_B, ty: Type::Exec },
            ],
        },
        ..Default::default()
    });
    ctx.connect(get_armed.port(WirePort::ExecOut), branch.port(WirePort::Exec));
    ctx.connect(get_armed.port(WirePort::Value), branch.port(WirePort::BCond));

    // 6. Reset: Var_Set(armed = false) on the true branch
    let false_lit = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec { name: *sym::OUTPUT, ty: Type::Bool }],
        },
        properties: {
            let mut p = HashMap::new();
            p.insert(*sym::VALUE, Literal::Bool(false));
            p
        },
        ..Default::default()
    });
    let reset_set = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_SET,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec { name: *sym::EXEC, ty: Type::Exec },
                PortSpec { name: *sym::VAR_REF, ty: Type::Ref(Box::new(Type::Bool)) },
                PortSpec { name: *sym::VALUE, ty: Type::Bool },
            ],
            outputs: vec![
                PortSpec { name: *sym::EXEC_OUT, ty: Type::Exec },
            ],
        },
        ..Default::default()
    });
    ctx.connect(branch.port(WirePort::ExecOutA), reset_set.port(WirePort::Exec));
    ctx.connect(armed_id.port(WirePort::VarRef), reset_set.port(WirePort::VarRef));
    ctx.connect(false_lit.port(WirePort::Output), reset_set.port(WirePort::Value));

    // 7. Continuation: everything after await runs from reset_set's ExecOut
    ctx.current_exec = Some(reset_set.port(WirePort::ExecOut));

    // 8. Bind the value if `let x = await ...`
    if let Some(ref binding_name) = a.binding {
        let val_port = if let Some(ref val_expr) = a.value_expr {
            lower_expr(ctx, val_expr)
        } else {
            exec_port
        };
        ctx.scope.insert(
            binding_name.clone(),
            Binding::Local(LocalRecord { port: val_port }),
        );
    }
}

fn build_exec_union(ctx: &mut LowerCtx, ports: Vec<PortRef>) -> PortRef {
    if ports.len() == 1 {
        return ports.into_iter().next().unwrap();
    }
    let mut merged = ports.into_iter();
    let first = merged.next().unwrap();
    let second = merged.next().unwrap();
    let mut current = {
        let union = ctx.add_gate(AddNodeOpts {
            gate_class: gc::UNION,
            source_range: SourceRange::default(),
            ports: GateIO {
                inputs: vec![
                    PortSpec { name: *sym::EXEC_A, ty: Type::Exec },
                    PortSpec { name: *sym::EXEC_B, ty: Type::Exec },
                ],
                outputs: vec![
                    PortSpec { name: *sym::EXEC_OUT, ty: Type::Exec },
                ],
            },
            ..Default::default()
        });
        ctx.connect(first, union.port(WirePort::ExecA));
        ctx.connect(second, union.port(WirePort::ExecB));
        union.port(WirePort::ExecOut)
    };
    for extra in merged {
        let union = ctx.add_gate(AddNodeOpts {
            gate_class: gc::UNION,
            source_range: SourceRange::default(),
            ports: GateIO {
                inputs: vec![
                    PortSpec { name: *sym::EXEC_A, ty: Type::Exec },
                    PortSpec { name: *sym::EXEC_B, ty: Type::Exec },
                ],
                outputs: vec![
                    PortSpec { name: *sym::EXEC_OUT, ty: Type::Exec },
                ],
            },
            ..Default::default()
        });
        ctx.connect(current, union.port(WirePort::ExecA));
        ctx.connect(extra, union.port(WirePort::ExecB));
        current = union.port(WirePort::ExecOut);
    }
    current
}

pub(super) fn flush_pending_emits(ctx: &mut LowerCtx) {
    let pending = std::mem::take(&mut ctx.pending_emits);
    for (name, ports) in pending {
        if ports.is_empty() {
            continue;
        }
        // Compute targets before building the union (which borrows ctx mut).
        let out = ctx.lookup_output(&name).cloned();
        let hub = ctx.exec_signal_hubs.get(&name).copied();
        let exec_out = build_exec_union(ctx, ports);
        if let Some(out) = out {
            ctx.connect(exec_out, out.node_id.port(WirePort::RerInput));
        } else if let Some(hub) = hub {
            // Local exec signal with a pre-declared hub (top-level `let x:
            // exec`): feed the emit union into the hub `on x` already triggers
            // from.
            ctx.connect(exec_out, hub.port(WirePort::ExecA));
        } else {
            // Fallback: a signal without a pre-declared hub (e.g. declared
            // inside a handler). Bind the union output directly; `on x` for
            // these still depends on source order.
            ctx.scope.insert(
                name,
                Binding::Local(LocalRecord { port: exec_out }),
            );
        }
    }
}

pub(super) fn lower_out_binding(
    ctx: &mut LowerCtx,
    name: &str,
    value: Option<&Expr>,
    _range: &SourceRange,
) {
    let Some(value) = value else { return };
    let out = match ctx.lookup_output(name).cloned() {
        Some(o) => o,
        None => return,
    };
    let port = lower_expr(ctx, value);
    ctx.connect(port, out.node_id.port(WirePort::RerInput));
}
