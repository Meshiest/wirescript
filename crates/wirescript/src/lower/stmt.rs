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
            // Pure position (chip/mod body instantiated without exec) or
            // `static var`: no exec reset runs, so a non-constant init is
            // dropped — surface it.
            if v.is_static || ctx.current_exec.is_none() {
                warn_unbaked_var_init(ctx, v, false);
            }
            // In exec context, emit a Var_Set to reset the variable to its
            // initial value each time this scope is entered. Without this,
            // PseudoVar keeps its value from the previous invocation.
            // `static var` skips this — it retains its value across calls.
            // Array-typed vars have no `VarRef` port (only `ArrayVarRef`), so
            // the generic Var_Set reset would emit a wire from a nonexistent
            // source port ("Wire source port VarRef does not exist" in-game);
            // rebuild them with the array-literal assign (clear + push) instead.
            if !v.is_static
                && ctx.current_exec.is_some()
                && let Some(var_rec) = ctx.lookup_var(&v.name).cloned()
                && var_rec.storage == VarStorage::Array
            {
                if let Some(init @ Expr::Array { elements, .. }) = &v.init {
                    lower_array_literal_assign(ctx, &var_rec, elements, &v.range, init);
                } else if let Some(init) = &v.init {
                    ctx.warn(
                        format!(
                            "'var {}' array initializer must be an array literal — this value is dropped; build the array with methods like push/copyFrom instead",
                            v.name
                        ),
                        init.range(),
                    );
                }
                return;
            }
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
                                let mut p = HashMap::default();
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
                            outputs: vec![PortSpec {
                                name: *sym::EXEC_OUT,
                                ty: Type::Exec,
                            }],
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
            // Wire the initializer into the buffer's Input. Pre-declaration
            // (here or in a body pre-pass) only creates the gate; without this
            // the initializer of any statement-position buffer (chip/mod/
            // handler body) is silently dropped and the input dangles.
            lower_buffer_body(ctx, b);
        }
        Stmt::Return { value, .. } => {
            if let Some(Expr::RecordLit { fields, .. }) = value {
                // A record-literal return: `-> { a, b }` is a single record-typed
                // output, and a bare record literal is not a standalone
                // expression, so destructure it into a field->binding map. The
                // inline-mod call binds the caller's record from this (see
                // `pending_return_record`) rather than from a single value port.
                ctx.pending_return_record = Some(lower_record_lit(ctx, fields));
            } else if let Some(expr) = value {
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
                                outputs: vec![PortSpec {
                                    name: *sym::EXEC_OUT,
                                    ty: Type::Exec,
                                }],
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
                            outputs: vec![PortSpec {
                                name: *sym::EXEC_OUT,
                                ty: Type::Exec,
                            }],
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
                outputs: vec![PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                }],
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
                outputs: vec![PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                }],
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
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
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
    let array_ref = var_rec.node_id.port(WirePort::ArrayVarRef);
    let elem_ty = var_rec.inner_type.clone();
    lower_array_method(ctx, array_ref, elem_ty.clone(), "clear", &[], range, value);
    for el in elements {
        let arg = [CallArg::Positional(el.expr().clone())];
        let method = match el {
            ArrayElem::Item(_) => "push",
            ArrayElem::Spread(_) => "append",
        };
        lower_array_method(ctx, array_ref, elem_ty.clone(), method, &arg, el.range(), value);
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
    ctx.exec_branch_depth += 1;
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
    // A Var_Get emitted in the THEN branch lives on the ExecOutA chain, so it must
    // not be reused on the ELSE chain (ExecOutB) - it never fires there, so the read
    // would be stale. Drop the cache so the else branch takes its own fresh reads.
    reset_var_get_caches(ctx);
    if let Some(else_b) = &s.else_block {
        lower_block(ctx, else_b);
    }
    ctx.exec_branch_depth -= 1;
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
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
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
    // Var_Gets created inside either branch only fire when that branch is taken, so
    // they must not be reused after the join. A post-if read gets a fresh Var_Get on
    // the (dominating) post-join chain - otherwise it reads a stale value whenever the
    // branch that created the Var_Get wasn't taken (e.g. `gamblersTotal` first read in
    // an `if phase == P_SNIPER` block and re-read in a later `if phase == P_GATHER`).
    reset_var_get_caches(ctx);
}

pub(super) fn lower_emit(ctx: &mut LowerCtx, s: &Emit) {
    let is_output = ctx.lookup_output(&s.name).is_some();
    // Local exec signals resolve to their per-declaration key via the scope,
    // so same-named signals in different bodies stay separate.
    let sig_key = ctx.signal_key(&s.name);
    if !is_output && sig_key.is_none() {
        return;
    }
    // Outputs are keyed by their plain name (resolved via lookup_output at
    // flush); signals by their unique key.
    let pending_key = if is_output {
        s.name.clone()
    } else {
        sig_key.clone().expect("checked above")
    };

    if let Some(ref value_expr) = s.value {
        if let Some(out) = ctx.lookup_output(&s.name).cloned() {
            let value_port = lower_expr(ctx, value_expr);
            ctx.connect(value_port, out.node_id.port(WirePort::RerInput));
        } else if let Some(ref key) = sig_key
            && ctx.current_exec.is_some()
        {
            // Local exec signal: the value is a ferried payload. Write it into
            // the signal's hidden store(s) on the emit chain — sequenced before
            // any buffer, so the value is stored this tick and read on the
            // resumed chain after the exec crosses the barrier.
            write_signal_payload(ctx, key, value_expr);
        }
        if let Some(current_exec) = ctx.current_exec {
            let src_exec = match &s.buffer {
                Some(spec) => buffered_exec(ctx, spec, current_exec),
                None => current_exec,
            };
            let chain = ctx.builder.current_chain_id;
            ctx.pending_emits
                .entry(pending_key)
                .or_default()
                .push((src_exec, chain));
        }
    } else {
        let current_exec = match ctx.current_exec {
            Some(e) => e,
            None => return,
        };
        let src_exec = match &s.buffer {
            Some(spec) => buffered_exec(ctx, spec, current_exec),
            None => current_exec,
        };
        let chain = ctx.builder.current_chain_id;
        ctx.pending_emits
            .entry(pending_key)
            .or_default()
            .push((src_exec, chain));
    }
}

/// Route an emit's exec through a Buffer gate per its `buffer(delay, hold)`
/// spec: the tick/seconds barrier that legalises loop back-edges (WS005) and
/// delays the signal delivery. Constant durations bake into gate properties;
/// `hold` defaults to `-1` (= use `delay`, the gate's "off-time follows
/// on-time" mode). Returns the buffer's `Output` as the new emit source.
fn buffered_exec(ctx: &mut LowerCtx, spec: &crate::ast::BufferSpec, exec_in: PortRef) -> PortRef {
    let class = if spec.seconds {
        gc::BUFFER_SECONDS
    } else {
        gc::BUFFER_TICKS
    };
    let (delay_sym, hold_sym) = if spec.seconds {
        (*sym::SECONDS_TO_WAIT, *sym::ZERO_SECONDS_TO_WAIT)
    } else {
        (*sym::TICKS_TO_WAIT, *sym::ZERO_TICKS_TO_WAIT)
    };
    let (delay_port, hold_port) = if spec.seconds {
        (WirePort::SecondsToWait, WirePort::ZeroSecondsToWait)
    } else {
        (WirePort::TicksToWait, WirePort::ZeroTicksToWait)
    };
    let unit_ty = if spec.seconds { Type::Float } else { Type::Int };
    // Coerce a constant duration to the gate's unit type (int ticks / float s).
    let unit_lit = |lit: Literal| -> Literal {
        match (spec.seconds, lit) {
            (true, Literal::Int(n)) => Literal::Float(n as f64),
            (false, Literal::Float(f)) => Literal::Int(f as i64),
            (_, other) => other,
        }
    };
    // Constant durations bake into properties; anything else lowers to a value
    // wire into the duration port. Lower the expressions *before* taking the
    // buffer's exec source so a duration var read chains on the emit path.
    let mut props = HashMap::default();
    let mut inputs = vec![PortSpec {
        name: *sym::INPUT,
        ty: Type::Exec,
    }];
    let delay_wire = match &spec.delay {
        // Bare `buffer emit`: one tick.
        None => {
            props.insert(
                delay_sym,
                if spec.seconds {
                    Literal::Float(1.0)
                } else {
                    Literal::Int(1)
                },
            );
            None
        }
        Some(d) => match crate::lower::predeclare::expr_to_literal(d) {
            Some(lit) => {
                props.insert(delay_sym, unit_lit(lit));
                None
            }
            None => {
                inputs.push(PortSpec {
                    name: delay_sym,
                    ty: unit_ty.clone(),
                });
                Some(lower_expr(ctx, d))
            }
        },
    };
    let hold_wire = match &spec.hold {
        Some(h) => match crate::lower::predeclare::expr_to_literal(h) {
            Some(lit) => {
                props.insert(hold_sym, unit_lit(lit));
                None
            }
            None => {
                inputs.push(PortSpec {
                    name: hold_sym,
                    ty: unit_ty.clone(),
                });
                Some(lower_expr(ctx, h))
            }
        },
        None => {
            // No hold given: -1 = hold follows the delay.
            props.insert(
                hold_sym,
                if spec.seconds {
                    Literal::Float(-1.0)
                } else {
                    Literal::Int(-1)
                },
            );
            None
        }
    };
    let exec_src = ctx.current_exec.unwrap_or(exec_in);
    let buf = ctx.add_gate(AddNodeOpts {
        gate_class: class,
        source_range: spec.range.clone(),
        ports: GateIO {
            inputs,
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::Exec,
            }],
        },
        properties: props,
        note: Some("buffered emit"),
        ..Default::default()
    });
    ctx.connect(exec_src, buf.port(WirePort::Input));
    if let Some(p) = delay_wire {
        ctx.connect(p, buf.port(delay_port));
    }
    if let Some(p) = hold_wire {
        ctx.connect(p, buf.port(hold_port));
    }
    buf.port(WirePort::Output)
}

/// The bare signal name an `await` (or exec expression) refers to, when it's a
/// plain identifier (a local exec signal). `None` for `Sleep(...)`, `a || b`, etc.
fn signal_name_of(e: &Expr) -> Option<&str> {
    match e {
        Expr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

/// Get (or create) the hidden payload store var for `sig`'s `field`
/// (`""` = scalar payload).
fn payload_store(ctx: &mut LowerCtx, sig: &str, field: &str, ty: Type) -> NodeId {
    if let Some(list) = ctx.exec_signal_payloads.get(sig) {
        if let Some((_, id, _)) = list.iter().find(|(f, _, _)| f == field) {
            return *id;
        }
    }
    let mut props = HashMap::default();
    if let Some(lit) = default_literal_for_var_type(&ty) {
        props.insert(*sym::INITIAL_VALUE, lit);
    }
    let store = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: SourceRange::default(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: ty.clone(),
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(ty.clone())),
                },
            ],
        },
        properties: props,
        note: Some("signal payload store"),
        ..Default::default()
    });
    ctx.exec_signal_payloads
        .entry(sig.to_string())
        .or_default()
        .push((field.to_string(), store, ty));
    store
}

/// `Var_Set(<var> = <value_port>)` chained on the current exec (advances it).
fn chain_var_set(ctx: &mut LowerCtx, var: NodeId, value_port: PortRef, ty: Type) {
    let Some(exec_in) = ctx.current_exec else {
        return;
    };
    let set = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_SET,
        source_range: SourceRange::default(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(ty.clone())),
                },
                PortSpec {
                    name: *sym::VALUE,
                    ty,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
        },
        note: Some("signal payload write"),
        ..Default::default()
    });
    ctx.connect(exec_in, set.port(WirePort::Exec));
    ctx.connect(var.port(WirePort::VarRef), set.port(WirePort::VarRef));
    ctx.connect(value_port, set.port(WirePort::Value));
    ctx.current_exec = Some(set.port(WirePort::ExecOut));
}

/// `Var_Get(<var>)` chained on the current exec (advances it); returns the
/// Value port.
fn chain_var_get(ctx: &mut LowerCtx, var: NodeId, ty: Type) -> PortRef {
    let exec_in = ctx.current_exec;
    let get = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_GET,
        source_range: SourceRange::default(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(ty.clone())),
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty,
                },
                PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                },
            ],
        },
        note: Some("signal payload read"),
        ..Default::default()
    });
    if let Some(e) = exec_in {
        ctx.connect(e, get.port(WirePort::Exec));
        ctx.current_exec = Some(get.port(WirePort::ExecOut));
    }
    ctx.connect(var.port(WirePort::VarRef), get.port(WirePort::VarRef));
    get.port(WirePort::Value)
}

/// Write `emit sig = <value>` into the signal's payload store(s), chained on
/// the current exec. A record literal writes one store per field; any other
/// value uses the scalar `""` store.
fn write_signal_payload(ctx: &mut LowerCtx, sig: &str, value_expr: &Expr) {
    if let Expr::RecordLit { fields, .. } = value_expr {
        for f in fields {
            let (name, fexpr) = match f {
                RecordLitField::Named { name, value, .. } => (name.clone(), value.clone()),
                // `{ sum, index }` shorthand: the value is the same-named local.
                RecordLitField::Shorthand { name, range } => (
                    name.clone(),
                    Expr::Ident {
                        name: name.clone(),
                        range: range.clone(),
                    },
                ),
                RecordLitField::Spread { .. } => continue,
            };
            let ty = unwrap_ref(&ctx.type_of(&fexpr));
            let value_port = lower_expr(ctx, &fexpr);
            let store = payload_store(ctx, sig, &name, ty.clone());
            chain_var_set(ctx, store, value_port, ty);
        }
        return;
    }
    let ty = unwrap_ref(&ctx.type_of(value_expr));
    let value_port = lower_expr(ctx, value_expr);
    let store = payload_store(ctx, sig, "", ty.clone());
    chain_var_set(ctx, store, value_port, ty);
}

pub(super) fn lower_await(ctx: &mut LowerCtx, a: &AwaitStmt) {
    // 1. Create a static bool var for the armed flag (initially false)
    let armed_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: Type::Bool,
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(Type::Bool)),
                },
            ],
        },
        properties: {
            let mut p = HashMap::default();
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
                outputs: vec![PortSpec {
                    name: *sym::OUTPUT,
                    ty: Type::Bool,
                }],
            },
            properties: {
                let mut p = HashMap::default();
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
                    PortSpec {
                        name: *sym::EXEC,
                        ty: Type::Exec,
                    },
                    PortSpec {
                        name: *sym::VAR_REF,
                        ty: Type::Ref(Box::new(Type::Bool)),
                    },
                    PortSpec {
                        name: *sym::VALUE,
                        ty: Type::Bool,
                    },
                ],
                outputs: vec![PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                }],
            },
            ..Default::default()
        });
        ctx.connect(exec_in, arm_set.port(WirePort::Exec));
        ctx.connect(
            armed_id.port(WirePort::VarRef),
            arm_set.port(WirePort::VarRef),
        );
        ctx.connect(
            true_lit.port(WirePort::Output),
            arm_set.port(WirePort::Value),
        );
        // Exec chain ends here — pre-await code is done
    }

    // Register an unconditional `await <signal>` so flush can route same-chain
    // emits through a `Var_Set(armed = true)` sequenced *before* the hub — a
    // parallel arm races the `Var_Get` below (it may read `false` and drop the
    // continuation), and loop back-edges must re-arm every iteration. Awaits
    // inside `if` branches don't register: their arm only fires when the branch
    // is taken, so same-chain emits stay flag-guarded (ordering is ambiguous by
    // design there).
    if ctx.exec_branch_depth == 0 {
        if let Some(key) = signal_name_of(&a.exec_expr).and_then(|sig| ctx.signal_key(sig)) {
            ctx.signal_awaits
                .entry(key)
                .or_insert((armed_id, ctx.builder.current_chain_id));
        }
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
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(Type::Bool)),
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: Type::Bool,
                },
                PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                },
            ],
        },
        ..Default::default()
    });
    ctx.connect(exec_port, get_armed.port(WirePort::Exec));
    ctx.connect(
        armed_id.port(WirePort::VarRef),
        get_armed.port(WirePort::VarRef),
    );

    // 5. Branch on armed flag — true branch continues, false drops
    let branch = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BRANCH,
        source_range: a.range.clone(),
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
    ctx.connect(
        get_armed.port(WirePort::ExecOut),
        branch.port(WirePort::Exec),
    );
    ctx.connect(
        get_armed.port(WirePort::Value),
        branch.port(WirePort::BCond),
    );

    // 6. Reset: Var_Set(armed = false) on the true branch
    let false_lit = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: a.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::Bool,
            }],
        },
        properties: {
            let mut p = HashMap::default();
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
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(Type::Bool)),
                },
                PortSpec {
                    name: *sym::VALUE,
                    ty: Type::Bool,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
        },
        ..Default::default()
    });
    ctx.connect(
        branch.port(WirePort::ExecOutA),
        reset_set.port(WirePort::Exec),
    );
    ctx.connect(
        armed_id.port(WirePort::VarRef),
        reset_set.port(WirePort::VarRef),
    );
    ctx.connect(
        false_lit.port(WirePort::Output),
        reset_set.port(WirePort::Value),
    );

    // 7. Continuation: everything after await runs from reset_set's ExecOut
    ctx.current_exec = Some(reset_set.port(WirePort::ExecOut));

    // 8. Bind the value if `let x = await ...`. For a local signal carrying a
    // ferried payload, read the payload store on the resumed chain (the emit
    // wrote it before the exec crossed any buffer).
    if let Some(ref binding_name) = a.binding {
        let payload = signal_name_of(&a.exec_expr)
            .and_then(|sig| ctx.signal_key(sig))
            .and_then(|key| ctx.exec_signal_payloads.get(&key))
            .and_then(|list| {
                list.iter()
                    .find(|(f, _, _)| f.is_empty())
                    .map(|(_, id, ty)| (*id, ty.clone()))
            });
        let val_port = if let Some((store, ty)) = payload {
            chain_var_get(ctx, store, ty)
        } else if let Some(ref val_expr) = a.value_expr {
            lower_expr(ctx, val_expr)
        } else {
            exec_port
        };
        ctx.scope.insert(
            &binding_name,
            Binding::Local(LocalRecord { port: val_port }),
        );
    }

    // 9. `let { a, b } = await sig`: read each destructured payload store on
    // the resumed chain and bind the locals.
    if let Some(ref fields) = a.destructure {
        for (field, local) in fields {
            let store = signal_name_of(&a.exec_expr)
                .and_then(|sig| ctx.signal_key(sig))
                .and_then(|key| ctx.exec_signal_payloads.get(&key))
                .and_then(|list| {
                    list.iter()
                        .find(|(f, _, _)| f == field)
                        .map(|(_, id, ty)| (*id, ty.clone()))
                });
            let Some((store, ty)) = store else {
                ctx.warn(
                    format!(
                        "awaited signal has no ferried payload field `{field}` — \
                         emit a value first (`emit <sig> = {{ {field}: ... }}`)"
                    ),
                    &a.range,
                );
                continue;
            };
            let val_port = chain_var_get(ctx, store, ty);
            ctx.scope.insert(
                &local,
                Binding::Local(LocalRecord { port: val_port }),
            );
        }
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
                    PortSpec {
                        name: *sym::EXEC_A,
                        ty: Type::Exec,
                    },
                    PortSpec {
                        name: *sym::EXEC_B,
                        ty: Type::Exec,
                    },
                ],
                outputs: vec![PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                }],
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
                    PortSpec {
                        name: *sym::EXEC_A,
                        ty: Type::Exec,
                    },
                    PortSpec {
                        name: *sym::EXEC_B,
                        ty: Type::Exec,
                    },
                ],
                outputs: vec![PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                }],
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
    for (name, entries) in pending {
        if entries.is_empty() {
            continue;
        }
        // Compute targets before building the union (which borrows ctx mut).
        let out = ctx.lookup_output(&name).cloned();
        let hub = ctx.exec_signal_hubs.get(&name).copied();
        if let Some(out) = out {
            let ports = entries.into_iter().map(|(p, _)| p).collect();
            let exec_out = build_exec_union(ctx, ports);
            ctx.connect(exec_out, out.node_id.port(WirePort::RerInput));
        } else if let Some(hub) = hub {
            // Local exec signal with a pre-declared hub. Emits on the same
            // chain as an unconditional `await` of this signal route through a
            // `Var_Set(armed = true)` *before* entering the hub — sequenced, so
            // the awaiting `Var_Get` can't race the arm, and loop back-edges
            // re-arm every iteration. Emits from other chains enter directly,
            // guarded by the armed flag.
            let awaited = ctx.signal_awaits.get(&name).copied();
            let (armed, direct): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .partition(|(_, chain)| awaited.is_some_and(|(_, ac)| *chain == ac));
            let mut next_hub_port = WirePort::ExecA;
            if !armed.is_empty() {
                let (armed_var, _) = awaited.expect("armed partition implies an await");
                let union_out = build_exec_union(ctx, armed.into_iter().map(|(p, _)| p).collect());
                let arm = build_arm_set(ctx, armed_var);
                ctx.connect(union_out, arm.port(WirePort::Exec));
                ctx.connect(arm.port(WirePort::ExecOut), hub.port(next_hub_port));
                next_hub_port = WirePort::ExecB;
            }
            if !direct.is_empty() {
                let union_out = build_exec_union(ctx, direct.into_iter().map(|(p, _)| p).collect());
                ctx.connect(union_out, hub.port(next_hub_port));
            }
        } else {
            // Fallback: a signal without a pre-declared hub (e.g. declared
            // inside a handler). Bind the union output directly; `on x` for
            // these still depends on source order.
            let ports = entries.into_iter().map(|(p, _)| p).collect();
            let exec_out = build_exec_union(ctx, ports);
            ctx.scope
                .insert(&name, Binding::Local(LocalRecord { port: exec_out }));
        }
    }
    // A hub that ended up with a single input is a pass-through: splice it out
    // (its one source drives everything that hung off the hub's ExecOut).
    let hubs: Vec<NodeId> = ctx.exec_signal_hubs.values().copied().collect();
    for hub in hubs {
        splice_single_input_union(ctx, hub);
    }
}

/// `Var_Set(<armed_var> = true)` gate pair (literal + set) used to arm an
/// await's flag on the emit path, sequenced upstream of the signal hub.
/// Mirrors the arm built in `lower_await` step 2.
fn build_arm_set(ctx: &mut LowerCtx, armed_var: NodeId) -> NodeId {
    let true_lit = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: SourceRange::default(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::Bool,
            }],
        },
        properties: {
            let mut p = HashMap::default();
            p.insert(*sym::VALUE, Literal::Bool(true));
            p
        },
        ..Default::default()
    });
    let arm_set = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_SET,
        source_range: SourceRange::default(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(Type::Bool)),
                },
                PortSpec {
                    name: *sym::VALUE,
                    ty: Type::Bool,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
        },
        note: Some("emit arms await"),
        ..Default::default()
    });
    ctx.connect(
        armed_var.port(WirePort::VarRef),
        arm_set.port(WirePort::VarRef),
    );
    ctx.connect(
        true_lit.port(WirePort::Output),
        arm_set.port(WirePort::Value),
    );
    arm_set
}

/// If `hub` has exactly one incoming wire, it's a degenerate pass-through
/// union: redirect everything hanging off its `ExecOut` to the single source
/// and remove the hub (so e.g. one emitter drives an `await`/`on` directly,
/// with no Union gate in between).
fn splice_single_input_union(ctx: &mut LowerCtx, hub: NodeId) {
    let module = &mut ctx.builder.module;
    let incoming: Vec<usize> = module
        .wires
        .iter()
        .enumerate()
        .filter(|(_, w)| w.target.node_id == hub)
        .map(|(i, _)| i)
        .collect();
    if incoming.len() != 1 {
        return;
    }
    let src = module.wires[incoming[0]].source.clone();
    for w in module.wires.iter_mut() {
        if w.source.node_id == hub {
            w.source = src.clone();
        }
    }
    module.wires.remove(incoming[0]);
    module.nodes.remove(&hub);
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
