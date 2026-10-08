use super::*;
use crate::ast::walk;
use crate::typecheck::ContainerKind;

pub(super) fn lower_stmt(ctx: &mut LowerCtx, s: &Stmt) {
    match s {
        Stmt::Assign(a) => lower_assign(ctx, a),
        Stmt::If(i) => lower_if(ctx, i),
        Stmt::Emit(e) => lower_emit(ctx, e),
        Stmt::Await(a) => ctx.with_nofold(a.no_fold, |ctx| lower_await(ctx, a)),
        Stmt::OutBinding(b) => ctx.with_nofold(b.no_fold, |ctx| {
            lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range)
        }),
        Stmt::ExprStmt(es) => lower_expr_stmt(ctx, &es.expr),
        Stmt::Handler(h) => ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h)),
        Stmt::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
        Stmt::AnonChip(ac) => lower_anon_chip(ctx, ac),
        Stmt::ChipDecl(c) => lower_chip_decl(ctx, c),
        Stmt::In(_) => {}
        Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| {
            let declares = needs_declaration(ctx, &v.name, &v.range);
            // `var x = <init reading x>`: the initializer reads the `x` around
            // the declaration, as a `let`'s does, so the var is declared under
            // a name of its own and `x` names it once it exists.
            if declares && let Some(apart) = v.declared_apart() {
                lower_stmt(ctx, &Stmt::Var(apart.clone()));
                if let Some(binding) = ctx.scope.get(&apart.name).cloned() {
                    ctx.scope.insert(&v.name, binding);
                }
                ctx.consts.forget(&v.name);
                return;
            }
            if declares {
                pre_declare_var(ctx, v);
            }
            // Pure position (chip/mod body instantiated without exec) or
            // `static var`: no exec reset runs, so a non-constant init is
            // dropped — surface it.
            if v.is_static || ctx.current_exec.is_none() {
                warn_unbaked_var_init(ctx, v, false);
            }
            // A record (or tuple or enum) var is several gates, so its
            // initializer runs as an assignment to them, which resets the var
            // each time this scope is entered like the forms below do.
            if !v.is_static
                && ctx.current_exec.is_some()
                && let Some(init) = &v.init
                && let Some(target @ Binding::Record(_)) = ctx.scope.get(&v.name).cloned()
            {
                if let Some(lit) = crate::lower::predeclare::const_container_init(ctx, init) {
                    rebuild_from_constant(ctx, &target, &v.name, init, lit, &v.range);
                    return;
                }
                lower_assign(
                    ctx,
                    &Assign {
                        target: Expr::Ident {
                            name: v.name.clone(),
                            range: v.range.clone(),
                        },
                        value: init.clone(),
                        range: v.range.clone(),
                    },
                );
                return;
            }
            // With no initializer each scalar leaf resets to its zero value,
            // as a plain `var x: int` does. A container leaf is left as the
            // plain `var xs: int[]` form leaves it.
            if !v.is_static
                && ctx.current_exec.is_some()
                && v.init.is_none()
                && let Some(Binding::Record(fields)) = ctx.scope.get(&v.name).cloned()
            {
                reset_record_leaves(ctx, &fields, &v.range);
                return;
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
                if let Some(init @ Expr::Array { .. }) = &v.init {
                    let target = Binding::Var(var_rec);
                    rebuild_container(ctx, &target, &v.name, init, &v.range, &mut lower_in_place(init, false));
                } else if let Some(init) = &v.init
                    && let Some(lit) = crate::lower::predeclare::const_container_init(ctx, init)
                {
                    rebuild_from_constant(ctx, &Binding::Var(var_rec), &v.name, init, lit, &v.range);
                } else if let Some(init) = &v.init {
                    ctx.warn(
                        format!(
                            "'var {}' array initializer must be an array literal, so this value is dropped; build the array with methods like push/copyFrom instead",
                            v.shown_name()
                        ),
                        init.range(),
                    );
                }
                return;
            }
            // Map-typed vars have no `VarRef` port either (only `MapVarRef`),
            // same hazard as arrays above; rebuild via the map-literal assign
            // (clear + set per entry) each time this scope is entered.
            if !v.is_static
                && ctx.current_exec.is_some()
                && let Some(var_rec) = ctx.lookup_var(&v.name).cloned()
                && var_rec.storage == VarStorage::Map
            {
                let init = v.init.as_ref().map(|i| ctx.with_map_literals(i));
                if let Some(init @ Expr::MapLit { .. }) = init.as_deref() {
                    let target = Binding::Var(var_rec);
                    rebuild_container(ctx, &target, &v.name, init, &v.range, &mut lower_in_place(init, false));
                } else if let Some(init) = &v.init
                    && let Some(lit) = crate::lower::predeclare::const_container_init(ctx, init)
                {
                    rebuild_from_constant(ctx, &Binding::Var(var_rec), &v.name, init, lit, &v.range);
                } else if let Some(init) = &v.init {
                    ctx.warn(
                        format!(
                            "'var {}' map initializer must be a map literal, so this value is dropped; build the map with methods like set/copyFrom instead",
                            v.shown_name()
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
                    default_literal_for_var_type(&var_rec.inner_type)
                        .map(|lit| literal_port(ctx, lit, var_rec.inner_type.clone(), &v.range))
                });
                if let Some(val_port) = init_val {
                    let exec_in = ctx.current_exec.unwrap_or(exec);
                    let inner = var_rec.inner_type.clone();
                    let set_node = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::VAR_SET,
                        source_range: v.range.clone(),
                        ports: GateIO::var_write(inner.clone()),
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
        }),
        Stmt::Array(a) => {
            if needs_declaration(ctx, &a.name, &a.range) {
                pre_declare_array(ctx, a);
            }
        }
        Stmt::Map(m) => {
            if needs_declaration(ctx, &m.name, &m.range) {
                pre_declare_map(ctx, m);
            }
        }
        Stmt::Buffer(b) => {
            // Current-frame check, mirroring `needs_declaration`: an ancestor
            // buffer of the same name is a distinct binding being shadowed, so
            // this statement must get its own gate rather than re-driving the
            // outer buffer's `Input` (a fan-in that fails to load).
            if ctx.lookup_buffer_current_frame(&b.name).is_none()
                || ctx.unhoisted.contains(&(b.range.file.clone(), b.range.start.offset))
            {
                pre_declare_buffer(ctx, b);
            }
            // Wire the initializer into the buffer's Input. Pre-declaration
            // (here or in a body pre-pass) only creates the gate; without this
            // the initializer of any statement-position buffer (chip/mod/
            // handler body) is silently dropped and the input dangles.
            lower_buffer_body(ctx, b);
        }
        Stmt::IfLet(i) => lower_if_let(ctx, i),
        Stmt::LetElse(l) => lower_let_else(ctx, l),
        Stmt::Return { value, .. } => {
            // A tuple return `(v0, v1, ...)` to a mod with multiple NAMED
            // outputs wires each element onto the matching output in declaration
            // order, the same delivery as `emit out_i = v_i`. A tuple parses to
            // an index-keyed record; without this it is stashed as one return
            // record and the caller's `let (a, b) = f()` reads the unwired output
            // nodes as `_Unsupported`. A single tuple/record output (count 1) and
            // a named `{ a, b }` record both keep the record path below.
            let names = ctx.mod_output_names.clone();
            let tuple = match value {
                Some(Expr::RecordLit { fields, .. })
                    if fields.iter().all(|f| {
                        matches!(f, RecordLitField::Named { name, .. } if name.parse::<usize>().is_ok())
                    }) =>
                {
                    Some(fields)
                }
                _ => None,
            };
            let tuple_to_multi = tuple.is_some()
                && if names.is_empty() {
                    ctx.output_count() > 1
                } else {
                    names.len() > 1
                };
            // A gate read held as its outputs, returned to the one output of
            // one value, is its first output on every return.
            let held_first = match value {
                Some(expr) if ctx.chip_return_outputs.is_empty() && ctx.output_count() == 1 => ctx
                    .first_output()
                    .map(|(_, out)| out.ty.clone())
                    .and_then(|t| held_first_output(ctx, expr, &t)),
                _ => None,
            };
            if !ctx.chip_return_outputs.is_empty() {
                if let Some(expr) = value {
                    lower_chip_return(ctx, expr);
                }
            } else if names.len() > 1
                && let Some(ret_rec) = ctx.mod_return_record.clone()
            {
                // Several outputs and several returns: every return stores each
                // output into the return storage, by output name. A tuple's
                // elements go to the outputs, whose types are its slots.
                let src = match (value, tuple) {
                    (Some(_), Some(fields)) => {
                        let targets = return_targets(&mod_outputs_with_exec(ctx, &names), fields.len());
                        let slots = Type::Tuple(
                            targets
                                .iter()
                                .map(|n| {
                                    ctx.mod_output_types
                                        .get(n)
                                        .cloned()
                                        .or_else(|| ctx.lookup_output(n).map(|o| o.ty.clone()))
                                        .unwrap_or(Type::Any)
                                })
                                .collect(),
                        );
                        Some(lower_record_lit(ctx, fields, Slots::Of(&slots)))
                    }
                    (Some(expr), None) => written_record_fields(ctx, "this returned value", expr),
                    (None, _) => None,
                };
                if let (Some(expr), Some(src)) = (value, src) {
                    let names = return_targets(&mod_outputs_with_exec(ctx, &names), src.len());
                    let src = by_output_name(ctx, src, expr, &names);
                    assign_record_fields(ctx, &ret_rec, &src, &SourceRange::default());
                }
            } else if tuple_to_multi {
                if let Some(fields) = tuple {
                    // Each element goes to its output: the outputs' types are
                    // the tuple's slots.
                    let outs = ctx.ordered_outputs();
                    let slots: Vec<Type> = if names.is_empty() {
                        outs.iter().map(|o| o.ty.clone()).collect()
                    } else {
                        return_targets(&mod_outputs_with_exec(ctx, &names), fields.len())
                            .iter()
                            .map(|n| ctx.lookup_output(n).map_or(Type::Any, |o| o.ty.clone()))
                            .collect()
                    };
                    let rec = lower_record_lit(ctx, fields, Slots::Of(&Type::Tuple(slots)));
                    if names.is_empty() {
                        for (i, out) in outs.iter().enumerate() {
                            let key = crate::intern::intern(&i.to_string());
                            if let Some(binding) = rec.get(&key).cloned()
                                && let Some(port) =
                                    binding_to_port(ctx, &binding, &SourceRange::default())
                            {
                                ctx.connect(port, out.node_id.port(WirePort::RerInput));
                            }
                        }
                    } else {
                        let names = return_targets(&mod_outputs_with_exec(ctx, &names), rec.len());
                        for (i, name) in names.iter().enumerate() {
                            let Some(binding) = rec.get(&crate::intern::intern(&i.to_string())).cloned() else {
                                continue;
                            };
                            // A record-typed output has no port of its own; its
                            // fields travel as the output's record, as
                            // `out name = rec` sends them.
                            if let Binding::Record(f) = binding {
                                ctx.pending_out_records.insert(name.clone(), f);
                            } else if let Some(out) = ctx.lookup_output(name).cloned()
                                && let Some(port) = binding_to_port(ctx, &binding, &SourceRange::default())
                            {
                                ctx.connect(port, out.node_id.port(WirePort::RerInput));
                            }
                        }
                    }
                }
            } else if names.len() > 1
                && let Some(expr) = value
                && ctx.gate_outputs(expr.range()).is_some()
            {
                // A gate read returned whole to several outputs (`return
                // fs.min()`, `return k` after `let k = g2(x)`) fills them by
                // output name, by position when the names differ, as the
                // several-returns path above stores them.
                if let Some(src) = written_record_fields(ctx, "this returned value", expr) {
                    let targets = return_targets(&mod_outputs_with_exec(ctx, &names), src.len());
                    ctx.pending_return_record = Some(by_output_name(ctx, src, expr, &targets));
                }
            } else if let Some(first) = held_first {
                match first {
                    // An enum first output is the value of the enum output.
                    Binding::Record(rec) => match ctx.mod_return_record.clone() {
                        Some(ret_rec) => assign_record_fields(ctx, &ret_rec, &rec, &SourceRange::default()),
                        None => ctx.pending_return_record = Some(rec),
                    },
                    binding => {
                        if let Some(port) = binding_to_port(ctx, &binding, &SourceRange::default()) {
                            return_one_value(ctx, port);
                        }
                    }
                }
            } else if ctx.mod_return_record.is_some() {
                // Multi-return mod with a single ENUM or RECORD output: store the
                // returned value's fields into the shared return record (per-field
                // Var_Set), so the caller reads the runtime-selected branch rather
                // than a leaked construction / record literal from one arm.
                // `value_record_fields` resolves a record literal, a record var /
                // field chain, OR an enum/record construction (through its own
                // may-produce-record lowering) to a field map uniformly, so every
                // return form routes through the same storage.
                if let Some(expr) = value {
                    let ret_rec = ctx.mod_return_record.clone().unwrap();
                    let what = "this returned value";
                    if let Some(src) = written_record_fields(ctx, what, expr)
                        && let Some(src) = fit_record_value(ctx, what, expr, &ret_rec, src)
                    {
                        assign_record_fields(ctx, &ret_rec, &src, &SourceRange::default());
                    }
                }
            } else if let Some(lit_expr @ Expr::RecordLit { fields, .. }) = value {
                // A record-literal return: `-> { a, b }` is a single record-typed
                // output, and a bare record literal is not a standalone
                // expression, so destructure it into a field->binding map. The
                // inline-mod call binds the caller's record from this (see
                // `pending_return_record`) rather than from a single value port.
                let lit = ctx.type_of(lit_expr);
                ctx.pending_return_record = Some(lower_record_lit(ctx, fields, Slots::Of(&lit)));
            } else if let Some(expr) = value
                && let Some(Binding::Record(fields)) = resolve_field_chain(ctx, expr).cloned()
            {
                // `return r` / `return r.sub` where the value is a record
                // binding: forward its field map — a record has no single
                // value port to wire to the output node. A gate read held in
                // a field the output holds one value in is its first output.
                let slot = ctx.first_output().map(|(_, out)| unwrap_ref(&out.ty));
                let fields = match (slot, ctx.checked_type_at(expr.range())) {
                    (Some(slot @ (Type::Record(_) | Type::Tuple(_))), Some(checked)) if ctx.output_count() == 1 => {
                        held_reads_where_one_value(checked, fields, Slot::Typed(slot))
                    }
                    _ => fields,
                };
                ctx.pending_return_record = Some(fields);
            } else if let Some(expr) = value
                && !matches!(expr, Expr::Call { .. })
                && ctx.output_count() == 1
                && ctx.first_output().is_some_and(|(_, out)| match unwrap_ref(&out.ty) {
                    Type::Record(_) | Type::Tuple(_) | Type::Enum { .. } => true,
                    Type::Any => matches!(
                        unwrap_ref(&ctx.type_of(expr)),
                        Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }
                    ),
                    _ => false,
                })
            {
                // Any other value of the one record, tuple or enum output
                // (a construction, an `if`/`match` choosing one, an element of
                // a column array) is handed to the caller as its fields; it has
                // no single port to wire. An output named by a type alias has
                // no type here, so the value's own type decides.
                if let Some(fields) = written_record_fields(ctx, "this returned value", expr) {
                    ctx.pending_return_record = Some(fields);
                }
            } else if let Some(expr) = value {
                // Clear any leftover inline-mod record so only THIS
                // expression's call can set it.
                ctx.pending_inline_record = None;
                let val_port = lower_expr(ctx, expr);
                // A gate read returned from an output of one value is its
                // first output, the port the call gives.
                let first_of_gate_read = ctx.output_count() == 1
                    && ctx.first_output().is_some_and(|(_, out)| holds_one_value(&unwrap_ref(&out.ty)))
                    && ctx.gate_outputs(expr.range()).is_some();
                if matches!(expr, Expr::Call { .. })
                    && let Some(record) = ctx.pending_inline_record.take()
                    && !first_of_gate_read
                {
                    // `return make(x)` where `make` returns a record: the call
                    // stashed a field→source map (its value "port" is a
                    // placeholder, not a real node). Forward the record to the
                    // caller instead of wiring the placeholder.
                    let one_enum = ctx.output_count() == 1
                        && ctx.first_output().is_some_and(|(_, out)| matches!(unwrap_ref(&out.ty), Type::Enum { .. }));
                    ctx.pending_return_record =
                        Some(if one_enum { one_enum_value(ctx, expr, record) } else { record });
                } else {
                    return_one_value(ctx, val_port);
                }
            }
            if let Some(exec) = ctx.current_exec.take() {
                if ctx.mod_return_exec.is_some() {
                    let prev = ctx.mod_return_exec.take().unwrap();
                    let union = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::UNION,
                        source_range: SourceRange::default(),
                        ports: GateIO::exec_join(),
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
        Stmt::For(f) => crate::lower::for_loop::lower_for(ctx, f),
        Stmt::Break { range } => {
            let Some(exec) = ctx.current_exec else {
                return;
            };
            let brk = ctx.add_gate(AddNodeOpts {
                gate_class: gc::CALLABLE_BREAK,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![PortSpec::exec(*sym::EXEC)],
                    outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
                },
                chip_id: ctx.current_anon_chip,
                ..Default::default()
            });
            ctx.connect(exec, brk.port(WirePort::Exec));
            // `Exec_CallableBreak` only stops SUBSEQUENT sweeps; its own
            // ExecOut still forwards, so leaving `current_exec` unset (rather
            // than wired to it) is what stops the rest of THIS sweep instead.
            ctx.current_exec = None;
        }
    }
}

/// Hand `val_port`, a `return`ed value, to the mod's one output of one value:
/// stored into its return var when several returns choose it, wired to the
/// output otherwise.
fn return_one_value(ctx: &mut LowerCtx, val_port: PortRef) {
    if let Some(var_rec) = ctx.mod_return_var.clone() {
        // Multi-return: Var_Set to the return var
        if let Some(exec) = ctx.current_exec {
            let inner = var_rec.inner_type.clone();
            let set_node = ctx.add_gate(AddNodeOpts {
                gate_class: gc::VAR_SET,
                source_range: SourceRange::default(),
                note: Some("ret_set"),
                ports: GateIO {
                    inputs: vec![
                        PortSpec::exec(*sym::EXEC),
                        PortSpec::new(*sym::VAR_REF, Type::Ref(Box::new(inner.clone()))),
                        PortSpec::new(*sym::VALUE, inner.clone()),
                    ],
                    outputs: vec![PortSpec::exec(*sym::EXEC_OUT)],
                },
                ..Default::default()
            });
            ctx.connect(exec, set_node.port(WirePort::Exec));
            ctx.connect(var_rec.node_id.port(WirePort::VarRef), set_node.port(WirePort::VarRef));
            ctx.connect(val_port, set_node.port(WirePort::Value));
            ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
        }
    } else if ctx.output_count() == 1 {
        // Single return: wire directly to output
        let out = ctx.first_output().unwrap().1.clone();
        ctx.connect(val_port, out.node_id.port(WirePort::RerInput));
    }
}

/// The value `return`s on a mod or chip body's own paths, value `match` arms
/// and anonymous chips included (not loops, nested handlers or declarations).
pub(super) fn count_return_values(block: &Block) -> usize {
    let mut count = 0;
    walk::walk(walk::Node::Block(block), walk::Ctx::default(), &mut |n, cx| {
        if cx.handler || cx.decl || cx.in_loop {
            return false;
        }
        if let walk::Node::Stmt(Stmt::Return { value: Some(_), .. }) = n {
            count += 1;
        }
        true
    });
    count
}

/// The value `return`s inside nested `on` handlers of a mod or chip body,
/// wherever the handler sits. Each ends its handler's run and stores the
/// outputs; the call's own path never reaches it.
pub(super) fn count_handler_return_values(block: &Block) -> usize {
    let mut count = 0;
    walk::walk(walk::Node::Block(block), walk::Ctx::default(), &mut |n, cx| {
        if cx.decl {
            return false;
        }
        if cx.handler
            && let walk::Node::Stmt(Stmt::Return { value: Some(_), .. }) = n
        {
            count += 1;
        }
        true
    });
    count
}

pub(super) fn block_contains_return(block: &Block) -> bool {
    walk::contains_return(block)
}

/// Does a `var`/`array`/`map` STATEMENT need to declare `name`, or does an
/// existing binding already cover it?
///
/// A statement-position declaration is skipped when the name already resolves
/// to a var IN THE CURRENT FRAME, which is what lets a body pre-pass declare it
/// once and the statement itself only run the reset. The check must be
/// CURRENT-FRAME, not a full chain walk: a same-named var in an ANCESTOR frame
/// (an outer handler/block, or a mod's top-level body around a nested `if`) is a
/// DIFFERENT variable being shadowed, and this statement must get its own fresh
/// storage gate. A chain walk found the ancestor and skipped the declaration, so
/// the inner `var` silently reused the outer's gate — type-divergent writes, a
/// `static var` reset every call, or a load-breaking buffer fan-in.
///
/// The one binding that must NOT count is a lazily materialized `const`
/// container (see `predeclare::materialize_const_container`): it exists only
/// because something READ the constant earlier in this body, so treating it as
/// "already declared" makes a genuine `var xs: int[]` reuse the const
/// container's gate instead of shadowing it — the declaration silently vanishes
/// and every later write lands on (or, being immutable, is rejected against) the
/// wrong array.
///
/// A declaration the body pre-declare left to its statement always declares:
/// the current frame binds its name to the parameter or module member it
/// shadows, which is not this declaration's storage.
fn needs_declaration(ctx: &LowerCtx, name: &str, range: &SourceRange) -> bool {
    if ctx.unhoisted.contains(&(range.file.clone(), range.start.offset)) {
        return true;
    }
    match ctx.lookup_var_current_frame(name) {
        Some(rec) => ctx.immutable_containers.contains(&rec.node_id),
        // A record (or enum) var the body pre-pass declared as its fields.
        None => !matches!(ctx.scope.get_current(name), Some(Binding::Record(_))),
    }
}

/// An expression statement: its value, if any, is discarded. In statement
/// position (a live exec chain) a match whose arms are blocks threads exec
/// through a Branch/Union tree; the exec continues via `current_exec`.
pub(super) fn lower_expr_stmt(ctx: &mut LowerCtx, e: &Expr) {
    match e {
        Expr::MatchExpr { scrutinee, arms, range }
            if ctx.current_exec.is_some() && arms.iter().any(|a| matches!(a.body, MatchBody::Block(_))) =>
        {
            lower_match_stmt(ctx, scrutinee, arms, range);
        }
        _ => {
            lower_expr(ctx, e);
        }
    }
}

pub(super) fn match_increment_self<'s>(ctx: &LowerCtx, s: &'s Assign) -> Option<(&'s Expr, bool)> {
    fn same_target(a: &Expr, b: &Expr) -> bool {
        match (a, b) {
            (Expr::Ident { name: x, .. }, Expr::Ident { name: y, .. }) => x == y,
            (Expr::Deref { operand: x, .. }, Expr::Deref { operand: y, .. }) => {
                matches!((x.as_ref(), y.as_ref()),
                    (Expr::Ident { name: p, .. }, Expr::Ident { name: q, .. }) if p == q)
            }
            _ => false,
        }
    }
    if !matches!(s.target, Expr::Ident { .. } | Expr::Deref { .. }) {
        return None;
    }
    match &s.value {
        Expr::BinOp { op, left, right, .. } if op == "+" => {
            if same_target(left, &s.target) {
                return Some((right, false));
            }
            if same_target(right, &s.target) {
                return Some((left, false));
            }
            None
        }
        // `x = x - d` increments by `-d` where both are ints or both floats.
        Expr::BinOp { op, left, right, .. } if op == "-" && same_target(left, &s.target) => {
            let (l, r) = (unwrap_ref(&ctx.type_of(left)), unwrap_ref(&ctx.type_of(right)));
            (matches!(l, Type::Int | Type::Float) && l == r).then_some((right.as_ref(), true))
        }
        _ => None,
    }
}

/// `-delta` for a numeric literal `delta`, negated where it is written.
fn negated_literal(ctx: &mut LowerCtx, delta: &Expr, range: &SourceRange) -> Option<PortRef> {
    match delta {
        Expr::IntLit { value, .. } => Some(literal_port(ctx, Literal::Int(value.wrapping_neg()), Type::Int, range)),
        Expr::FloatLit { value, .. } => Some(literal_port(ctx, Literal::Float(-value), Type::Float, range)),
        _ => None,
    }
}

/// `-delta` through a `MathNegate` gate over its lowered `port`.
fn negated(ctx: &mut LowerCtx, delta: &Expr, port: PortRef, range: &SourceRange) -> PortRef {
    let ty = unwrap_ref(&ctx.type_of(delta));
    let node = ctx.add_gate(AddNodeOpts {
        gate_class: gc::MATH_NEGATE,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![PortSpec::new(*sym::INPUT, ty.clone())],
            outputs: vec![PortSpec::new(*sym::OUTPUT, ty)],
        },
        ..Default::default()
    });
    ctx.connect(port, node.port(WirePort::Input));
    node.port(WirePort::Output)
}

/// Whether `x = x + d` names the target first, so `x` is read before `d`.
fn increments_left(s: &Assign, delta: &Expr) -> bool {
    matches!(&s.value, Expr::BinOp { right, .. } if std::ptr::eq(right.as_ref(), delta))
}

/// What `x = x + d` (or `x = x - d`, `negate`) writes, and whether it
/// increments `x` by it: `d` (`-d`), unless `x` comes first and `d`'s code may
/// change it. Then `x` (as `target` reads it) is read before `d` runs, and the
/// sum (difference) is set instead.
fn increment_value(ctx: &mut LowerCtx, s: &Assign, delta: &Expr, negate: bool, target: Binding) -> (PortRef, bool) {
    if !increments_left(s, delta) {
        return (lower_expr(ctx, delta), true);
    }
    if negate && let Some(port) = negated_literal(ctx, delta, &s.range) {
        return (port, true);
    }
    let mut ord = crate::lower::ordered::Ordered::default();
    let t = ord.push(target, "", None, &s.range);
    let d = ord.port(ctx, delta);
    let delta_port = ord.port_of(ctx, d);
    match ord.value(ctx, t) {
        Some(Binding::Local(held)) => (crate::lower::ops::binop_gate(ctx, &s.value, held.port, delta_port), false),
        _ if negate => (negated(ctx, delta, delta_port, &s.range), true),
        _ => (delta_port, true),
    }
}

/// Emit a `Var_Set` (or, for buffer storage, a direct `Input` wire) writing an
/// ALREADY-LOWERED `value_port` into `var_rec` on the current exec chain,
/// advancing `current_exec` and invalidating the read cache. Extracted from the
/// field-assignment path so the whole-record decomposition writes each leaf
/// field identically. A non-buffer write in pure position (no `current_exec`)
/// is a no-op, matching the caller's own guard.
pub(super) fn set_scalar_var(ctx: &mut LowerCtx, var_rec: &VarRecord, value_port: PortRef, range: &SourceRange) {
    if var_rec.storage == VarStorage::Buffer {
        ctx.connect(value_port, var_rec.node_id.port(WirePort::Input));
        return;
    }
    let Some(exec_in) = ctx.current_exec else {
        return;
    };
    let inner = var_rec.inner_type.clone();
    let set_node = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR_SET,
        source_range: range.clone(),
        ports: GateIO::var_write(inner.clone()),
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

/// Rebuild the container `target` from `lit`, the folded value of an
/// initializer that names a constant (`var xs: int[] = IS` in a handler), as
/// its literal twin is rebuilt: each element a literal gate.
pub(super) fn rebuild_from_constant(
    ctx: &mut LowerCtx,
    target: &Binding,
    name: &str,
    init: &Expr,
    lit: Literal,
    range: &SourceRange,
) {
    let at = init.range().clone();
    match lit {
        Literal::Array(items) => {
            let value = Expr::Array {
                elements: items.iter().map(|_| ArrayElem::Item(init.clone())).collect(),
                range: at.clone(),
            };
            let mut supply = |ctx: &mut LowerCtx, i: usize| {
                Supplied::Item(literal_element(ctx, target, items[i].clone(), &at))
            };
            rebuild_container(ctx, target, name, &value, range, &mut supply);
        }
        Literal::Map(pairs) => {
            let Some(key_ty) = container_key_type(target) else { return };
            let value = Expr::MapLit {
                entries: pairs
                    .iter()
                    .map(|_| crate::ast::MapLitEntry { key: init.clone(), value: init.clone(), range: at.clone() })
                    .collect(),
                range: at.clone(),
            };
            let mut supply = |ctx: &mut LowerCtx, i: usize| {
                let (k, v) = pairs[i].clone();
                let key = literal_node_range(ctx, &at, key_ty.clone(), coerce_literal_to_type(k, &key_ty));
                Supplied::Entry(key, literal_element(ctx, target, v, &at))
            };
            rebuild_container(ctx, target, name, &value, range, &mut supply);
        }
        _ => {}
    }
}

/// The key type of an array or map var, or of the columns of a record map.
fn container_key_type(target: &Binding) -> Option<Type> {
    match target {
        Binding::Var(v) => match &v.inner_type {
            Type::Map(k, _) => Some(k.as_ref().clone()),
            _ => None,
        },
        Binding::Record(cols) => cols.values().find_map(container_key_type),
        _ => None,
    }
}

/// The type one element of the container column `v` holds.
fn column_element_type(v: &VarRecord) -> Type {
    match (&v.storage, &v.inner_type) {
        (VarStorage::Map, Type::Map(_, val)) => val.as_ref().clone(),
        (_, t) => t.clone(),
    }
}

/// One constant element of the container `target` as literal gates: a single
/// gate for an array or map var, one per column for a record or enum
/// container (a column the value has no field for takes its type's zero).
fn literal_element(ctx: &mut LowerCtx, target: &Binding, lit: Literal, at: &SourceRange) -> ElemValue {
    fn record(
        ctx: &mut LowerCtx,
        cols: &HashMap<crate::intern::Sym, Binding>,
        fields: &[(String, Literal)],
        at: &SourceRange,
    ) -> HashMap<crate::intern::Sym, Binding> {
        let mut out = HashMap::default();
        for (k, col) in cols {
            let fname = crate::intern::resolve(*k);
            let field = fields.iter().find(|(n, _)| *n == fname).map(|(_, l)| l.clone());
            match col {
                Binding::Record(sub) => {
                    let sub_fields = match field {
                        Some(Literal::Record(f)) => f,
                        _ => Vec::new(),
                    };
                    out.insert(*k, Binding::Record(record(ctx, sub, &sub_fields, at)));
                }
                Binding::Var(v) => {
                    let ty = column_element_type(v);
                    if let Some(lit) = field.or_else(|| default_literal_for_var_type(&ty)) {
                        let lit = coerce_literal_to_type(lit, &ty);
                        let port = literal_node_range(ctx, at, ty, lit);
                        out.insert(*k, Binding::Local(LocalRecord { port }));
                    }
                }
                _ => {}
            }
        }
        out
    }
    match target {
        Binding::Record(cols) => {
            let fields = match lit {
                Literal::Record(f) => f,
                _ => Vec::new(),
            };
            ElemValue::Record(record(ctx, cols, &fields, at))
        }
        Binding::Var(v) => {
            let ty = column_element_type(v);
            let lit = coerce_literal_to_type(lit, &ty);
            ElemValue::Scalar(literal_node_range(ctx, at, ty, lit))
        }
        _ => ElemValue::Scalar(literal_node_range(ctx, at, Type::Any, lit)),
    }
}

/// A `Literal` gate producing `lit` as a `ty`.
fn literal_port(ctx: &mut LowerCtx, lit: Literal, ty: Type, range: &SourceRange) -> PortRef {
    let lit_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: range.clone(),
        properties: {
            let mut p = HashMap::default();
            p.insert(*sym::VALUE, lit);
            p
        },
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec::new(*sym::OUTPUT, ty)],
        },
        ..Default::default()
    });
    lit_id.port(WirePort::Output)
}

/// Reset every scalar leaf of a record var to the value its gate starts with,
/// which for a var declared with no initializer is its zero value (an enum's
/// first variant).
fn reset_record_leaves(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) {
    let mut keys: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    keys.sort_by_key(|k| crate::intern::resolve(*k));
    for k in keys {
        match &fields[&k] {
            Binding::Record(sub) => reset_record_leaves(ctx, sub, range),
            Binding::Var(v) if v.storage == VarStorage::Var => {
                let initial = ctx
                    .builder
                    .module
                    .nodes
                    .get(&v.node_id)
                    .and_then(|n| n.properties.get(&sym::INITIAL_VALUE).cloned());
                if let Some(lit) = initial.or_else(|| default_literal_for_var_type(&v.inner_type)) {
                    let port = literal_port(ctx, lit, v.inner_type.clone(), range);
                    set_scalar_var(ctx, v, port, range);
                }
            }
            _ => {}
        }
    }
}

/// A returned record value keyed by the mod's output names: a tuple's
/// positions, or a multi-output call's outputs, map onto the names in order.
fn by_output_name(
    ctx: &LowerCtx,
    src: HashMap<crate::intern::Sym, Binding>,
    value: &Expr,
    names: &[String],
) -> HashMap<crate::intern::Sym, Binding> {
    if names.iter().all(|n| src.contains_key(&crate::intern::intern(n))) {
        return src;
    }
    let order: Vec<String> = match unwrap_ref(&ctx.type_of(value)) {
        Type::Record(fields) if fields.len() == names.len() => fields.into_iter().map(|(n, _)| n).collect(),
        _ => (0..names.len()).map(|i| i.to_string()).collect(),
    };
    order
        .iter()
        .zip(names)
        .filter_map(|(from, to)| {
            src.get(&crate::intern::intern(from))
                .map(|b| (crate::intern::intern(to), b.clone()))
        })
        .collect()
}

/// The outputs a `return` value of `arity` fields fills, in order: only the
/// value outputs when it has one field per value output (an exec output takes
/// no value), otherwise every output.
fn return_targets(outputs: &[(String, bool)], arity: usize) -> Vec<String> {
    let values = outputs.iter().filter(|(_, exec)| !exec).count();
    outputs
        .iter()
        .filter(|(_, exec)| !(*exec && arity == values && values != outputs.len()))
        .map(|(n, _)| n.clone())
        .collect()
}

/// The inlined mod's outputs, each with whether it is an exec output.
fn mod_outputs_with_exec(ctx: &LowerCtx, names: &[String]) -> Vec<(String, bool)> {
    names
        .iter()
        .map(|n| (n.clone(), ctx.lookup_output(n).is_some_and(|o| matches!(o.ty, Type::Exec))))
        .collect()
}

/// `return` in a chip with several outputs, or with one record output: each
/// value output takes its value by position or by name (the one record output
/// takes the whole value). An output with a backing (written by several
/// returns or also by `emit`) stores into it on the exec chain; any other is
/// driven straight from the value, field by field for a record output.
fn lower_chip_return(ctx: &mut LowerCtx, expr: &Expr) {
    let range = expr.range().clone();
    let Some(src) = value_record_fields(ctx, expr) else {
        return;
    };
    let outputs = ctx.chip_return_outputs.clone();
    let value_names: Vec<String> =
        outputs.iter().filter(|(_, exec)| !exec).map(|(n, _)| n.clone()).collect();
    let src = if let [(only, _)] = outputs.as_slice() {
        let mut whole = HashMap::default();
        whole.insert(crate::intern::intern(only), Binding::Record(src));
        whole
    } else {
        let names = return_targets(&outputs, src.len());
        by_output_name(ctx, src, expr, &names)
    };
    let mut stored: HashMap<crate::intern::Sym, Binding> = HashMap::default();
    for name in &value_names {
        let key = crate::intern::intern(name);
        let Some(value) = src.get(&key).cloned() else {
            continue;
        };
        if let Some(backing) = ctx.output_record_backing.get(name).cloned() {
            stored.insert(key, Binding::Record(backing));
            continue;
        }
        if let Some(backing) = ctx.output_backing_vars.get(name).cloned() {
            stored.insert(key, Binding::Var(backing));
            continue;
        }
        match ctx.scope.get(&crate::lower::context::output_scope_key(name)).cloned() {
            Some(Binding::Record(pins)) => {
                if let Binding::Record(fields) = value {
                    wire_record_output_fields(ctx, &pins, &fields, &range);
                }
            }
            Some(Binding::Output(out)) => {
                if let Some(port) = binding_to_port(ctx, &value, &range) {
                    ctx.connect(port, out.node_id.port(WirePort::RerInput));
                }
            }
            _ => {}
        }
    }
    if stored.is_empty() {
        return;
    }
    // With no chain the store cannot run; the call reports that as WS007
    // (`unsequenced_chip_returns`).
    if ctx.current_exec.is_none() {
        return;
    }
    assign_record_fields(ctx, &stored, &src, &range);
}

/// A record-valued `if`/`match` chooses per leaf, and a record whose field is
/// an array or map of records has several containers with no single reference
/// to choose, so the value has no form: WS071, as `if c then pts else pts2` is.
/// `true` when it reported.
fn reject_column_record_choice(ctx: &mut LowerCtx, value: &Expr, range: &SourceRange) -> bool {
    fn column_field(fields: &[(String, Type)]) -> Option<(String, &'static str)> {
        fields.iter().find_map(|(name, ft)| match unwrap_ref(ft) {
            Type::Array(e) | Type::Map(_, e) => match *e {
                Type::Record(_) => Some((name.clone(), "records")),
                Type::Tuple(_) => Some((name.clone(), "tuples")),
                Type::Enum { .. } => Some((name.clone(), "enum values")),
                _ => None,
            },
            Type::Record(sub) => column_field(&sub).map(|(p, k)| (format!("{name}.{p}"), k)),
            _ => None,
        })
    }
    let Type::Record(fields) = unwrap_ref(&ctx.type_of(value)) else {
        return false;
    };
    let Some((path, kind)) = column_field(&fields) else {
        return false;
    };
    ctx.error(
        "WS071",
        format!(
            "this conditional chooses between records whose field `{path}` is an array or map \
             of {kind}. That field is several containers with no single reference to choose, \
             so the choice can't be a value. Branch on the condition and use each record \
             directly"
        ),
        range,
    );
    true
}

/// Resolve a record-typed VALUE expression to its per-field binding map: a
/// record literal (`{ x, y }`) lowers each field expr in place; an identifier or
/// field chain naming a `Binding::Record` (a record var/let/input) forwards its
/// field map; a record-valued `if`/`match` chooses per leaf; a record array or
/// map element reads every column; a record-returning call or an enum
/// construction is lowered here and its stashed per-field record taken.
/// Returns `None` for a value with no field map, leaving the caller to fall
/// through rather than miswire.
pub(super) fn value_record_fields(
    ctx: &mut LowerCtx,
    value: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    if let Expr::RecordLit { fields, .. } = value {
        let lit = ctx.type_of(value);
        return Some(lower_record_lit(ctx, fields, Slots::Of(&lit)));
    }
    if let Expr::As { value: inner, .. } = value {
        return crate::lower::convert::as_record_fields(ctx, inner, value);
    }
    // `f(c).r` on `-> (r: Hue)` is the value of `f(c)`.
    if let Some(call) = crate::lower::access::named_enum_output_call(ctx, value) {
        return value_record_fields(ctx, call);
    }
    // `f(c).o` on `-> (o: Opt, q: int)`: that output of the call's outputs.
    if let Expr::FieldAccess { obj, field, .. } = value
        && matches!(unwrap_ref(&ctx.type_of(value)), Type::Enum { .. } | Type::Record(_))
        && crate::lower::access::called_chip_decl(ctx, obj)
            .is_some_and(|d| d.outputs.len() > 1 && d.outputs.iter().any(|o| o.name == *field))
        && let Some(outputs) = value_record_fields(ctx, obj)
        && let Some(Binding::Record(output)) = outputs.get(&crate::intern::intern(field)).cloned()
    {
        return Some(output);
    }
    if let Expr::RefOf { operand, range } = value
        && let Some(Binding::Record(fields)) = resolve_field_chain(ctx, operand).cloned()
    {
        return Some(crate::lower::ref_fields::refs_of(ctx, &fields, range));
    }
    // `if c then x else y` yielding a RECORD. A record has no single-wire form,
    // so the choice is made per leaf: one Select each, all sharing the one
    // lowered condition. Every caller bails silently on a `None` from here, so
    // without this arm assignment, the `out` position and a chip argument all
    // produce nothing at all.
    if let Expr::IfExpr {
        cond,
        then_branch,
        else_branch,
        range,
    } = value
    {
        if reject_column_record_choice(ctx, value, range) {
            return None;
        }
        if ctx.current_exec.is_some()
            && (crate::lower::expr::has_effects(ctx, then_branch)
                || crate::lower::expr::has_effects(ctx, else_branch))
        {
            report_effectful_aggregate_choice(ctx, range);
            return None;
        }
        let then_fields = choice_branch_fields(ctx, then_branch, range)?;
        let else_fields = choice_branch_fields(ctx, else_branch, range)?;
        let cond_port = lower_expr(ctx, cond);
        let rec = select_record_fields(ctx, cond_port, &then_fields, &else_fields, range);
        if let Some(outputs) = ctx.gate_outputs(range) {
            mark_gate_read_choice(ctx, &rec, outputs);
        }
        return Some(rec);
    }
    // `match s { A => p, B => q }` yielding a RECORD or an ENUM value: the same
    // per-leaf choice the if-expr arm above makes, spread over the match's
    // decision tree. Must precede the `Call`/enum-typed fallback at the bottom,
    // which would lower an enum-typed match through the SCALAR match path and
    // report each arm as WS071. Without this arm every consumer bailed on the
    // `None` and a record-valued match assigned nothing at all - no Select, no
    // store, no diagnostic.
    if let Expr::MatchExpr { range, .. } = value
        && reject_column_record_choice(ctx, value, range)
    {
        return None;
    }
    if let Expr::MatchExpr { scrutinee, arms, range } = value
        && let Some(rec) =
            crate::lower::expr::lower_match_expr_record(ctx, scrutinee, arms, range)
    {
        if let Some(outputs) = ctx.gate_outputs(range) {
            mark_gate_read_choice(ctx, &rec, outputs);
        }
        return Some(rec);
    }
    // `pts[i]` reads a record ARRAY element as a record value (per-field
    // ArrayGet), so it can be assigned/pushed like any other record.
    if let Expr::IndexAccess { obj, index, range } = value
        && let Some(rec) = lower_record_array_index_value(ctx, obj, index, range)
    {
        return Some(rec);
    }
    // `.Value` / `.prev` on a record VALUE opens the variable, the same way
    // `resolve_field_chain` opens a NAMED one. A map `get` is a call, so it has
    // no scope binding for that path to walk, which left `m.get(k).Value`
    // without a record even though `m[k]` had one. That combination was the
    // worst kind: `WS066` on `m.get(k)` names `.Value` as the fix, and `.Value`
    // then failed too.
    if let Expr::FieldAccess { obj, field, .. } = value
        && crate::catalog::is_var_pseudo_field(field)
        && matches!(
            obj.as_ref(),
            Expr::Call { .. } | Expr::IndexAccess { .. }
        )
        && let Some(inner) = value_record_fields(ctx, obj)
    {
        // A real `Value` field (`a.pop().Value`) wins over the identity, exactly
        // as it does in `resolve_field_chain`.
        return match inner.get(&crate::intern::intern(field)) {
            Some(Binding::Record(f)) => Some(f.clone()),
            Some(_) => None,
            None => Some(inner),
        };
    }
    // `pts[i].inner` reads a NESTED record field of a record ARRAY element as a
    // record value (per-field ArrayGet), so it flows like any other record
    // source. A `pts[i].scalar` field goes through the scalar path instead
    // (this returns `None` for it), so only whole nested records land here.
    if matches!(value, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && let Some(rec) = lower_record_array_field_path_value(ctx, value, value.range())
    {
        return Some(rec);
    }
    // `m[k].inner` / `m.get(k).inner` — the same for a NESTED record field of a
    // record MAP value (per-field MapGet).
    if matches!(value, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && let Some(rec) = lower_record_map_field_path_value(ctx, value, value.range())
    {
        return Some(rec);
    }
    // `m[k]` / `m.get(k)` reads a record MAP value as a record (per-field MapGet).
    if matches!(value, Expr::IndexAccess { .. } | Expr::Call { .. })
        && let Some(rec) = lower_record_map_key_value(ctx, value, value.range())
    {
        return Some(rec);
    }
    match resolve_field_chain(ctx, value).cloned() {
        Some(Binding::Record(f)) => return Some(f),
        Some(Binding::Local(LocalRecord { port })) => {
            if let Some(rec) = gate_port_outputs(ctx, value, port) {
                return Some(rec);
            }
        }
        _ => {}
    }
    // A record-returning CALL that isn't one of the shapes above - `pts.pop()`,
    // a record-returning mod/chip - OR an enum CONSTRUCTION reaching this
    // point (`Dir.E`, `Shape.Circle(5.0)`, `Box.Dims { w, h }` - every check
    // above only resolves an EXISTING record binding, never a fresh
    // construction) stashes its per-field record in `pending_inline_record`
    // when lowered (see `try_lower_enum_ctor` in `lower/expr.rs` for the
    // construction case). Lower it and take that. The construction forms are
    // gated on the value's own CHECKED type (rather than unconditionally
    // routing every `FieldAccess` through here) so an ordinary non-record
    // field access - which this function correctly has no binding for -
    // still falls through to `None` below without a wasted lowering.
    let may_produce_record = matches!(value, Expr::Call { .. } | Expr::VariantCtor { .. })
        || matches!(ctx.type_of(value), Type::Enum { .. });
    ctx.last_value_record_port = None;
    if may_produce_record {
        ctx.pending_inline_record = None;
        let saved_record_call = std::mem::replace(
            &mut ctx.record_result_call,
            matches!(value, Expr::Call { .. }).then(|| value.range().clone()),
        );
        let port = lower_expr(ctx, value);
        ctx.record_result_call = saved_record_call;
        // Kept for `branch_record_fields`, the one caller that can still make
        // something of a record-shaped value with no field map.
        ctx.last_value_record_port = Some((value.range().clone(), port));
        if let Some(rec) = ctx.pending_inline_record.take() {
            return Some(unwrap_call_record(ctx, value, rec));
        }
        // A MULTI-output chip call stashes no record; its outputs are the
        // chip's own rerouters.
        if matches!(value, Expr::Call { .. })
            && let Type::Record(fields) = unwrap_ref(&ctx.type_of(value))
            && let Some(rec) = crate::lower::access::multi_output_chip_record(ctx, port, &fields)
        {
            return Some(rec);
        }
        // A gate read lowered to its gate's first output is its gate's
        // outputs.
        if let Some(rec) = gate_port_outputs(ctx, value, port) {
            return Some(rec);
        }
        // A payload-free enum on one wire (an element of a port's column of
        // tags) is the enum value whose tag is that wire.
        if !matches!(value, Expr::Call { .. } | Expr::VariantCtor { .. })
            && crate::lower::predeclare::unit_enum_port(ctx, &unwrap_ref(&ctx.type_of(value)))
            && !ctx.builder.module.nodes.get(&port.node_id).is_some_and(|n| n.gate_class == gc::UNSUPPORTED)
        {
            let mut rec = HashMap::default();
            rec.insert(crate::intern::intern("__disc"), Binding::Local(LocalRecord { port }));
            return Some(rec);
        }
    }
    None
}

/// The outputs of `value`, a gate read held on `port`, its gate's first
/// output (`fs.max()`, `k` after `let k = fs.max()`), each by the name it is
/// declared with. `None` for any other value.
fn gate_port_outputs(ctx: &LowerCtx, value: &Expr, port: PortRef) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let outputs = ctx.gate_outputs(value.range())?;
    let rec: HashMap<crate::intern::Sym, Binding> = outputs
        .iter()
        .filter_map(|(n, _)| {
            let port = crate::lower::access::resolve_output_field_port(ctx, port.node_id, n)?;
            Some((crate::intern::intern(n), Binding::Local(LocalRecord { port })))
        })
        .collect();
    (rec.len() == outputs.len()).then_some(rec)
}

/// The port [`value_record_fields`] lowered for exactly `value`, if its last
/// probe lowered one. A port from a probe of a different expression (such as
/// the `obj` of `obj.Value`) is dropped, never handed back.
pub(super) fn take_value_record_port(ctx: &mut LowerCtx, value: &Expr) -> Option<PortRef> {
    match ctx.last_value_record_port.take() {
        Some((range, port)) if &range == value.range() => Some(port),
        _ => None,
    }
}

/// The value of a call's stashed record. A mod or chip call's stash is
/// already it: a single output's is that output's record (typecheck types the
/// call as the record itself, so `mk().x` is its field), and several outputs'
/// is keyed by output name, each a real member. Any other call gets
/// [`unwrap_single_record_output`].
pub(super) fn unwrap_call_record(
    ctx: &LowerCtx,
    value: &Expr,
    rec: HashMap<crate::intern::Sym, Binding>,
) -> HashMap<crate::intern::Sym, Binding> {
    match crate::lower::access::called_chip_decl(ctx, value) {
        Some(_) => rec,
        None if crate::lower::call::is_builtin_select(ctx, value) => rec,
        None => unwrap_single_record_output(rec),
    }
}

pub(super) fn unwrap_single_record_output(
    rec: HashMap<crate::intern::Sym, Binding>,
) -> HashMap<crate::intern::Sym, Binding> {
    if rec.len() == 1
        && let Some(Binding::Record(inner)) = rec.values().next()
    {
        return inner.clone();
    }
    rec
}

/// One branch of a record-valued `if` or `match`, or an argument of a
/// `Select`, the choice at `join`. The record it chooses is a value, so a
/// `*T` field that names storage is chosen as that storage's ref. A gate read
/// in a choice that is no gate read gives it its first output (an enum
/// first output joined with an enum).
pub(super) fn choice_branch_fields(
    ctx: &mut LowerCtx,
    value: &Expr,
    join: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let mut fields = branch_record_fields(ctx, value)?;
    let mut ty = ctx.type_of(value);
    if ctx.gate_outputs(join).is_none()
        && let Some(Binding::Record(first)) = held_output(ctx, &fields, value.range(), OutputPick::First)
    {
        fields = first;
        ty = ctx.one_value_type_at(value.range()).unwrap_or(ty);
    }
    crate::lower::ref_fields::rebind_typed(
        ctx,
        &ty,
        &mut fields,
        crate::lower::ref_fields::FieldKind::Value,
        value.range(),
    );
    Some(fields)
}

/// [`value_record_fields`] for a BRANCH of a record-valued `if`/`match`, which
/// resolves one shape more: a MULTI-OUTPUT GATE result (`m.get(k)` is
/// `{Value, Found}`) as its per-port record, so the conditional chooses each
/// port separately.
///
/// Deliberately not in `value_record_fields` itself. Everywhere else a
/// multi-output result auto-unwraps to its FIRST port, which is what a scalar
/// sink expects (`var n: int = m.get(k)`); returning a record there would
/// change what those consumers receive. In a conditional it is the opposite:
/// one Select over port 0 is all the branches ever produced, so `c.Found` read
/// the *value* Select and the `bFound` port was wired nowhere at all.
pub(super) fn branch_record_fields(
    ctx: &mut LowerCtx,
    value: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    if let Some(rec) = value_record_fields(ctx, value) {
        return Some(rec);
    }
    // Reuses the port `value_record_fields` just lowered rather than lowering
    // the call a second time.
    let port = take_value_record_port(ctx, value)?;
    let Type::Record(fields) = unwrap_ref(&ctx.type_of(value)) else {
        return None;
    };
    if fields.len() < 2 {
        return None;
    }
    let outputs = ctx.builder.module.nodes.get(&port.node_id)?.ports.outputs.len();
    if outputs < 2 {
        return None;
    }
    let mut out = HashMap::default();
    for (name, _) in &fields {
        // All or nothing: a field with no matching port would leave a hole the
        // Select tree would silently drop, which is the bug being fixed.
        let p = crate::lower::access::resolve_output_field_port(ctx, port.node_id, name)?;
        out.insert(
            crate::intern::intern(name),
            Binding::Local(LocalRecord { port: p }),
        );
    }
    Some(out)
}

/// What a `Select` choosing one output of two gate reads is noted as: no
/// place may read that output, and then it is dropped (see
/// `lower::prune_unread_gate_read_choices`).
pub(super) const GATE_READ_CHOICE: &str = "gate read choice select";

/// Note every `Select` of `rec`, a choice between two gate reads' outputs,
/// as [`GATE_READ_CHOICE`], its ports typed as the output it chooses is.
pub(in crate::lower) fn mark_gate_read_choice(
    ctx: &mut LowerCtx,
    rec: &HashMap<crate::intern::Sym, Binding>,
    outputs: &[(String, Type)],
) {
    for (name, b) in rec {
        if let Binding::Local(LocalRecord { port }) = b
            && let Some(node) = ctx.builder.module.nodes.get_mut(&port.node_id)
            && node.gate_class == gc::SELECT
        {
            node.note = Some(GATE_READ_CHOICE);
            if let Some((_, ty)) = outputs.iter().find(|(n, _)| crate::intern::intern(n) == *name) {
                let ports = std::sync::Arc::make_mut(&mut node.ports);
                let values = ports.inputs.iter_mut().chain(ports.outputs.iter_mut());
                for p in values.filter(|p| p.name != *sym::B_SELECT_B) {
                    p.ty = unwrap_ref(ty).as_record();
                }
            }
        }
    }
}

/// Choose between two record values per LEAF FIELD: one `Select` gate for each
/// scalar leaf the two sides share, recursing into nested record fields, with
/// every gate reading the SAME already-lowered `cond_port` (re-lowering the
/// condition per field would multiply it by the record's width).
///
/// Fields present on only one side are dropped: typecheck has already required
/// both branches to be the same record type, so a one-sided field cannot appear
/// in a well-formed program, and taking the intersection keeps a mis-typed one
/// from wiring a Select with a missing input.
pub(super) fn select_record_fields(
    ctx: &mut LowerCtx,
    cond_port: PortRef,
    then_fields: &HashMap<crate::intern::Sym, Binding>,
    else_fields: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut out = HashMap::default();
    for (fname, then_b) in in_field_order(then_fields) {
        let Some(else_b) = else_fields.get(&fname) else {
            continue;
        };
        match (then_b, else_b) {
            (Binding::Record(tf), Binding::Record(ef)) => {
                out.insert(
                    fname,
                    Binding::Record(select_record_fields(ctx, cond_port, tf, ef, range)),
                );
            }
            _ => {
                let (Some(then_port), Some(else_port)) = (
                    crate::lower::access::binding_to_port(ctx, then_b, range),
                    crate::lower::access::binding_to_port(ctx, else_b, range),
                ) else {
                    continue;
                };
                let ty = crate::lower::call::arg_port_type(ctx, then_port)
                    .or_else(|| crate::lower::call::arg_port_type(ctx, else_port))
                    .unwrap_or(Type::Any);
                let node_id = ctx.add_gate(AddNodeOpts {
                    gate_class: gc::SELECT,
                    source_range: range.clone(),
                    ports: GateIO {
                        inputs: vec![
                            PortSpec::new(*sym::INPUT_A, ty.clone()),
                            PortSpec::new(*sym::INPUT_B, ty.clone()),
                            PortSpec::new(*sym::B_SELECT_B, Type::Bool),
                        ],
                        outputs: vec![PortSpec::new(*sym::OUTPUT, ty.clone())],
                    },
                    note: Some("record if-expr select".into()),
                    ..Default::default()
                });
                // Same polarity as the scalar `lower_if_expr`: the THEN value is
                // InputB (picked when bSelectB is true), the ELSE value InputA.
                ctx.connect(cond_port, node_id.port(WirePort::BSelectB));
                ctx.connect(then_port, node_id.port(WirePort::InputB));
                ctx.connect(else_port, node_id.port(WirePort::InputA));
                out.insert(
                    fname,
                    Binding::Local(LocalRecord {
                        port: node_id.port(WirePort::Output),
                    }),
                );
            }
        }
    }
    out
}

/// `lit`, a record or tuple literal's type, as the storage `target` holds it:
/// a field the target stores as one value takes one value there, so a gate
/// read in it is its first output (`var t = { m: fs.max(), k: 7 }`).
fn held_as_stored(lit: &Type, target: &HashMap<crate::intern::Sym, Binding>) -> Type {
    let slot = |k: &str, t: &Type| match target.get(&crate::intern::intern(k)) {
        Some(Binding::Record(inner)) => held_as_stored(t, inner),
        Some(Binding::Var(v)) if matches!(t, Type::Record(_)) && holds_one_value(&v.inner_type) => v.inner_type.clone(),
        _ => t.clone(),
    };
    match lit {
        Type::Record(fs) => Type::Record(fs.iter().map(|(k, t)| (k.clone(), slot(k, t))).collect()),
        Type::Tuple(ts) => Type::Tuple(ts.iter().enumerate().map(|(i, t)| slot(&i.to_string(), t)).collect()),
        other => other.clone(),
    }
}

/// Write a record value into a `Binding::Record` target, field by field: each
/// leaf `Var` field is read from the matching source field (`binding_to_port`,
/// which emits a `Var_Get` for a stored source) and written with `set_scalar_var`;
/// nested record fields recurse. A record literal goes through
/// [`crate::lower::record_assign::assign_literal`]. A source that reads storage
/// the assignment writes (`p = t` after `let t = { x: p.y, y: p.x }`) is read
/// before the first write, and a container is copied before anything replaces
/// it (see [`crate::lower::record_assign::assign_value`]).
fn assign_record(
    ctx: &mut LowerCtx,
    target_fields: &HashMap<crate::intern::Sym, Binding>,
    path: &str,
    value: &Expr,
    range: &SourceRange,
) {
    if let Expr::RecordLit { fields, .. } = value {
        let lit = held_as_stored(&ctx.type_of(value), target_fields);
        crate::lower::record_assign::assign_literal(ctx, target_fields, path, fields, Slots::Of(&lit), range);
        return;
    }
    let what = "the value assigned here";
    let Some(src) = written_record_fields(ctx, what, value) else {
        return;
    };
    let Some(mut src) = fit_record_value(ctx, what, value, target_fields, src) else {
        return;
    };
    src.retain(|k, b| !target_fields.get(k).is_some_and(|t| same_storage(t, b)));
    crate::lower::record_assign::assign_value(ctx, target_fields, path, src, range);
}

/// A record's fields in declaration order: by the storage gate each holds,
/// which is made in that order, and by name for fields holding none. Hash
/// order would move with the order names were interned.
pub(super) fn in_field_order(fields: &HashMap<crate::intern::Sym, Binding>) -> Vec<(crate::intern::Sym, &Binding)> {
    fn first_gate(b: &Binding) -> Option<NodeId> {
        match b {
            Binding::Var(v) => Some(v.node_id),
            Binding::Record(f) => f.values().filter_map(first_gate).min(),
            _ => None,
        }
    }
    let mut out: Vec<(crate::intern::Sym, &Binding)> = fields.iter().map(|(k, b)| (*k, b)).collect();
    out.sort_by_cached_key(|(k, b)| {
        let gate = first_gate(b);
        (gate.is_none(), gate, crate::intern::resolve(*k))
    });
    out
}

/// Whether two bindings name the very same storage gates.
pub(super) fn same_storage(a: &Binding, b: &Binding) -> bool {
    match (a, b) {
        (Binding::Var(x), Binding::Var(y)) => x.node_id == y.node_id,
        (Binding::Record(x), Binding::Record(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, xb)| y.get(k).is_some_and(|yb| same_storage(xb, yb)))
        }
        _ => false,
    }
}

pub(super) fn report_non_record_map_entry(ctx: &mut LowerCtx, range: &SourceRange) {
    ctx.error(
        "WS071",
        "this map entry's value is not a record, so it can't be split across the \
         record map's fields",
        range,
    );
}

pub(super) fn assign_record_fields(
    ctx: &mut LowerCtx,
    target: &HashMap<crate::intern::Sym, Binding>,
    src: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) {
    for (fname, tbind) in in_field_order(target) {
        let tbind = tbind.clone();
        let Some(sbind) = src.get(&fname).cloned() else {
            continue;
        };
        match tbind {
            Binding::Record(tf) => {
                if let Binding::Record(sf) = sbind {
                    assign_record_fields(ctx, &tf, &sf, range);
                }
            }
            Binding::Var(tvar) if tvar.storage == VarStorage::Var => {
                // A slot holding a ref stores a place's own ref.
                let port = if matches!(tvar.inner_type, Type::Ref(_)) {
                    crate::lower::ref_fields::ref_port_of(ctx, &sbind, range)
                } else {
                    binding_to_port(ctx, &sbind, range)
                };
                if let Some(port) = port {
                    set_scalar_var(ctx, &tvar, port, range);
                }
            }
            // A container field copies its CONTENTS. Skipping it (the old
            // behavior) made `q = p` a clean-compiling partial copy: the scalar
            // fields moved and `q.xs` silently kept whatever it held before.
            // `copyFrom` replaces the destination's contents wholesale, which is
            // exactly whole-record-copy semantics.
            Binding::Var(tvar) => copy_container(ctx, &tvar, &sbind, range),
            _ => {}
        }
    }
}

/// `copyFrom` of the container `src` into the array or map `tvar`; nothing for
/// a scalar `tvar`.
pub(super) fn copy_container(ctx: &mut LowerCtx, tvar: &VarRecord, src: &Binding, range: &SourceRange) {
    let (gate, port, src_ty) = match tvar.storage {
        VarStorage::Array => (
            gc::ARRAY_COPY_FROM,
            WirePort::ArrayVarRef,
            Type::Array(Box::new(Type::Any)),
        ),
        VarStorage::Map => (gc::MAP_COPY_FROM, WirePort::MapVarRef, Type::Ref(Box::new(Type::Any))),
        _ => return,
    };
    let Some(src_ref) = binding_to_port(ctx, src, range) else {
        return;
    };
    let target = tvar.node_id.port(port);
    let extra = vec![(WirePort::SourceRef, src_ty, src_ref)];
    if tvar.storage == VarStorage::Array {
        crate::lower::access::array_exec_op(ctx, range, target, gate, extra, vec![], WirePort::ExecOut);
    } else {
        crate::lower::access::map_exec_op(ctx, range, target, gate, extra, vec![], WirePort::ExecOut);
    }
}

pub(super) fn lower_assign(ctx: &mut LowerCtx, s: &Assign) {
    if crate::lower::props::lower_environment_write(ctx, s) || crate::lower::props::lower_component_write(ctx, s) {
        return;
    }
    let with_map_literals;
    let s = match ctx.with_map_literals(&s.value) {
        std::borrow::Cow::Owned(value) => {
            with_map_literals = Assign { value, ..s.clone() };
            &with_map_literals
        }
        std::borrow::Cow::Borrowed(_) => s,
    };
    // An assignment is an exec statement, so with no exec chain to run on it is
    // dropped: a mod/chip body used as a value (`let x = f()`), or a top-level
    // assignment outside any handler. `Assign` never reaches here through the
    // pure-chip declaration path, so a missing exec chain always means a drop.
    //
    // `dead_after_break` means WS081 already covered this exact statement,
    // so this stays quiet instead of also blaming a pure position.
    if ctx.current_exec.is_none() && !ctx.dead_after_break {
        let msg = if ctx.exec_closed_by_buffer_emit {
            "this assignment has no exec chain to run on, so it never happens. A `buffer \
             emit` ending an earlier block (or a mod it calls) closed the chain, and nothing \
             after it on that chain runs. Move the assignment before that emit."
        } else {
            "this assignment has no exec chain to run on, so it never happens. It sits in a \
             pure position - a mod or chip body used as a value (`let x = f()`), or a \
             statement outside any `on` handler. Run it in an exec context, or pass \
             `exec = <trigger>` to the call."
        };
        ctx.warn_code("WS058", msg, &s.range);
    }
    // `unsafe v.Variant.field = x` writes the payload slot the assertion names.
    // The slot is ordinary storage, so a scalar goes through `set_scalar_var`
    // and a record payload through `assign_record`, exactly as a capture write
    // does. `__disc` is not touched: the spelling asserts the tag rather than
    // setting it.
    if let Expr::Unsafe { inner, range } = &s.target {
        if ctx.current_exec.is_none() {
            return;
        }
        match crate::lower::access::resolve_unsafe_slot(ctx, inner).cloned() {
            Some(Binding::Var(var_rec)) => {
                let value_port = lower_expr(ctx, &s.value);
                set_scalar_var(ctx, &var_rec, value_port, &s.range);
            }
            Some(Binding::Record(fields)) => {
                assign_record(ctx, &fields, &target_path(inner), &s.value, &s.range)
            }
            _ => ctx.error(
                "WS007",
                "this `unsafe` payload access resolves to no storage slot, so the write would emit nothing",
                range,
            ),
        }
        return;
    }

    // `*e = v`: write through the ref `e` evaluates to. `*x = v` on a plain
    // var or a `*T` parameter is the explicit spelling of `x = v`.
    if let Expr::Deref { operand, .. } = &s.target {
        if ctx.current_exec.is_none() {
            return;
        }
        if !matches!(ctx.type_of(operand), Type::Ref(_)) {
            // `*x = *x + d` is `x = x + d`, so it increments as that does.
            let own = |e: &Expr| match (e, operand.as_ref()) {
                (Expr::Deref { operand: o, .. }, Expr::Ident { name, .. })
                    if matches!(o.as_ref(), Expr::Ident { name: n, .. } if n == name) =>
                {
                    o.as_ref().clone()
                }
                _ => e.clone(),
            };
            let value = match &s.value {
                Expr::BinOp { op, left, right, range } if op == "+" || op == "-" => Expr::BinOp {
                    op: op.clone(),
                    left: Box::new(own(left)),
                    right: Box::new(own(right)),
                    range: range.clone(),
                },
                other => other.clone(),
            };
            let plain = Assign {
                target: operand.as_ref().clone(),
                value,
                range: s.range.clone(),
            };
            lower_assign(ctx, &plain);
            return;
        }
        let inner = match ctx.type_of(operand) {
            Type::Ref(t) => *t,
            _ => unreachable!("checked above"),
        };
        let (value, ref_port, increment) = match match_increment_self(ctx, s) {
            Some((delta, negate)) if increments_left(s, delta) => {
                let ref_port = lower_expr(ctx, operand);
                let target = Binding::DynRef(DynRefRecord {
                    port: ref_port,
                    storage: VarStorage::Var,
                    inner_type: inner.clone(),
                });
                let (value, increment) = increment_value(ctx, s, delta, negate, target);
                (value, ref_port, increment)
            }
            // The ref is the target, evaluated before the value.
            increment => {
                let delta = increment.map(|(d, _)| d);
                let ports = crate::lower::ordered::lower_in_order(ctx, &[operand, delta.unwrap_or(&s.value)]);
                (ports[1], ports[0], increment.is_some())
            }
        };
        write_through_ref(ctx, ref_port, &inner, value, increment, &s.range);
        return;
    }

    if let Expr::IndexAccess { obj, index, .. } = &s.target {
        // Map subscript `m[k] = v` desugars to a MapVar_Set (the same gate
        // `m.set(k, v)` lowers to) — mirrors the read desugar in
        // `lower_index_access`. Typecheck's recorded kind for the target
        // object picks the map path. Falls through to the array path below
        // for non-map subscript targets, which reports (WS044) a map target
        // that neither map path could take.
        if let Some(ContainerKind::Map { key: k, value: v }) = ctx.container_kind(obj)
            && let Some(ContainerSource::Ref(map_ref)) =
                container_ref(ctx, obj, &ContainerKind::Map { key: k.clone(), value: v.clone() })
        {
            if ctx.current_exec.is_none() {
                return;
            }
            // `m[k] = v` is a mutation — the `const` container rule applies to
            // it exactly as it does to `m.set(k, v)`. Mirrors the array
            // subscript write in `lower_array_set`.
            if reject_const_container_mutation(ctx, map_ref.node_id, true, "an index write", &s.range) {
                return;
            }
            let ports = crate::lower::ordered::lower_in_order(ctx, &[index, &s.value]);
            let (key, val) = (ports[0], ports[1]);
            map_exec_op(
                ctx,
                &s.range,
                map_ref,
                gc::MAP_SET,
                vec![
                    (WirePort::Key, k.clone(), key),
                    (WirePort::Value, v.clone(), val),
                ],
                vec![],
                WirePort::ExecOut,
            );
            return;
        }
        // `m[k] = rec` where `m` is a record map: fan the record value across the
        // parallel field maps at the shared key. Checked before the array path,
        // which resolves only single arrays / record arrays.
        if let Some(fields) = resolve_record_map(ctx, obj)
            && lower_record_map_key_set(ctx, &fields, index, &s.value, &s.range)
        {
            return;
        }
        lower_array_set(ctx, obj, index, &s.value, &s.range);
        return;
    }

    // `pts[i].f1.f2… = v` — write a field (possibly nested) of a record array's
    // element by fanning across the parallel arrays at the shared index. The
    // target is a FieldAccess/TuplePick chain over an IndexAccess, so neither
    // the IndexAccess branch above nor the record field-chain branch below
    // reaches it.
    if matches!(&s.target, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && lower_record_array_field_path_set(ctx, &s.target, &s.value, &s.range)
    {
        return;
    }

    // `m[k].f1.f2… = v` — the same for a record MAP value, fanning across the
    // parallel maps at the shared key (the map spelling of the branch above).
    if matches!(&s.target, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && lower_record_map_field_path_set(ctx, &s.target, &s.value, &s.range)
    {
        return;
    }

    // Field targets that resolve through a record to a var: `cpu.x = 5`, and
    // the tuple spelling `pair.0 = 5` (a `TuplePick` over an index-keyed
    // record).
    if matches!(&s.target, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && let Some(binding) = resolve_field_chain(ctx, &s.target).cloned()
        && let Binding::Var(var_rec) = binding
    {
        if ctx.current_exec.is_none() {
            return;
        }
        // `ns.xs = [..]` / `rec.m = { .. }`: a container field or namespace
        // member is written like the bare container var.
        if lower_container_assign(ctx, &var_rec, &target_path(&s.target), &s.value, &s.range) {
            return;
        }
        let value_port = lower_expr(ctx, &s.value);
        set_scalar_var(ctx, &var_rec, value_port, &s.range);
        return;
    }
    // A record parameter's `*T` field given a ref value names the var the ref
    // points at, so the write goes through the ref.
    if matches!(&s.target, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && let Some(Binding::DynRef(d)) = resolve_field_chain(ctx, &s.target).cloned()
        && d.storage == VarStorage::Var
    {
        if ctx.current_exec.is_none() {
            return;
        }
        let value = lower_expr(ctx, &s.value);
        write_through_ref(ctx, d.port, &d.inner_type, value, false, &s.range);
        return;
    }

    // `recArr = [rec0, rec1, ...]` on a RECORD array (a `Binding::Record` of
    // parallel per-field arrays): rebuild it at runtime like the scalar array
    // path, fanned across the fields — clear all field arrays, then push / append
    // each element. Must precede the whole-record-assign branch below: a record
    // array is ALSO a `Binding::Record`, so that branch would otherwise claim it
    // and `assign_record` would silently drop the array literal. Reuses the
    // record-array method lowering, so an element that is a record literal, a
    // record var, or a tuple-pick of a record all wire.
    if ctx.current_exec.is_some()
        && let Expr::Array { .. } = &s.value
        && let Some(fields) = crate::lower::access::resolve_record_array(ctx, &s.target)
    {
        let target = Binding::Record(fields);
        rebuild_container(ctx, &target, "", &s.value, &s.range, &mut lower_in_place(&s.value, true));
        return;
    }
    // `m = { k: rec, .. }` on a RECORD map: the map spelling of the branch above.
    if ctx.current_exec.is_some()
        && let Expr::MapLit { .. } = &s.value
        && let Some(fields) = crate::lower::access::resolve_record_map(ctx, &s.target)
    {
        let target = Binding::Record(fields);
        rebuild_container(ctx, &target, "", &s.value, &s.range, &mut lower_in_place(&s.value, true));
        return;
    }

    // Whole-record assignment: `p = {..}` / `p = q` / `big.sub = {..}` where the
    // target resolves to a `Binding::Record`. A record has no single storage
    // gate, so decompose both sides field-by-field and write each leaf via its
    // own `Var_Set`. Without this the assignment silently did nothing (no
    // `_Unsupported`, no wire — a typechecked no-op).
    let target_record = match &s.target {
        Expr::Ident { name, .. } => match ctx.scope.get(name) {
            Some(Binding::Record(f)) => Some(f.clone()),
            _ => None,
        },
        Expr::FieldAccess { .. } => match resolve_field_chain(ctx, &s.target).cloned() {
            Some(Binding::Record(f)) => Some(f),
            _ => None,
        },
        _ => None,
    };
    if let Some(target_fields) = target_record {
        if ctx.current_exec.is_none() {
            return;
        }
        assign_record(ctx, &target_fields, &target_path(&s.target), &s.value, &s.range);
        return;
    }

    // A record field that resolves to a non-`var` backing (a field of a
    // `let`/input/literal record) has no storage gate, so writing it cannot
    // produce any wire. Reject it like assigning a `let` rather than dropping
    // it silently. A `var`-backed field is written by the `Binding::Var` branch
    // above and never reaches here.
    if let Expr::FieldAccess { field, .. } = &s.target
        && let Some(binding) = resolve_field_chain(ctx, &s.target).cloned()
        && matches!(binding, Binding::Local(_) | Binding::Input(_))
    {
        ctx.error(
            "WS007",
            format!(
                "cannot assign to `{field}`: it is a field of a `let`/input record and is \
                 read-only; only `var`-backed record fields are assignable"
            ),
            &s.range,
        );
        return;
    }

    // Everything below writes a bare `Ident`, so a FieldAccess/TuplePick target
    // reaching here was claimed by no branch above and cannot produce a gate.
    // Nothing upstream catches it either: an assignment target that is a field
    // access is deliberately typed `any` (see
    // `typecheck::stmt::infer_assign_target`) precisely because lowering is what
    // resolves it. Without a diagnostic here the statement type-checks clean and
    // emits nothing.
    //
    // The common shape is an ENUM payload field (`e.s = v`). An enum value is a
    // `Binding::Record` of `__disc` plus one `__{Variant}_{field}` slot per
    // payload field, so the SURFACE name never resolves; naming the variants
    // whose payload carries the field points at the destructure that does reach
    // the slot.
    if let Expr::FieldAccess { obj, field, .. } = &s.target {
        let owner = resolve_field_chain(ctx, obj).cloned();
        let variants: Vec<&'static str> = match &owner {
            Some(Binding::Record(fields))
                if fields.contains_key(&crate::intern::intern("__disc")) =>
            {
                let suffix = format!("_{field}");
                let mut vs: Vec<&'static str> = fields
                    .keys()
                    .map(|k| crate::intern::resolve(*k))
                    .filter(|k| k.starts_with("__") && k.ends_with(&suffix))
                    .map(|k| &k[2..k.len() - suffix.len()])
                    .collect();
                vs.sort_unstable();
                vs
            }
            _ => Vec::new(),
        };
        if let Some(v) = variants.first() {
            ctx.error(
                "WS007",
                format!(
                    "cannot assign to `{field}` directly: it is payload of variant `{v}`, and an enum's payload has no field write. Destructure and assign the capture, which writes the slot in place: `if let {v} {{ {field} }} = <value> {{ {field} = ... }}` (or the same capture in a `match` arm). To write the slot without testing the tag, `unsafe <value>.{v}.{field} = ...`"
                ),
                &s.range,
            );
            return;
        }
        // Not an enum: an unknown field, or a field of something with no
        // storage (a scalar, a call result). Either way the write is a no-op.
        ctx.error(
            "WS007",
            format!(
                "cannot assign to `{field}`: nothing writable is behind it, so the write would emit no gate. Only a variable, an array/map element, or a `var`-backed record field can be assigned"
            ),
            &s.range,
        );
        return;
    }

    let var_name = match &s.target {
        Expr::Ident { name, .. } => name.clone(),
        _ => return,
    };
    if let Some(Binding::DynRef(d)) = ctx.scope.get(&var_name).cloned() {
        if ctx.current_exec.is_none() {
            return;
        }
        if d.storage != VarStorage::Var {
            ctx.error(
                "WS027",
                format!(
                    "assigning a whole container through a ref is not supported; use \
                     {var_name}.clear() and push, or {var_name}.copyFrom(src)"
                ),
                &s.range,
            );
            return;
        }
        let (value, increment) = match match_increment_self(ctx, s) {
            Some((delta, negate)) => increment_value(ctx, s, delta, negate, Binding::DynRef(d.clone())),
            None => (lower_expr(ctx, &s.value), false),
        };
        write_through_ref(ctx, d.port, &d.inner_type, value, increment, &s.range);
        return;
    }
    let var_rec = match ctx.lookup_var(&var_name).cloned() {
        Some(v) => v,
        None => return,
    };
    let current_exec = match ctx.current_exec {
        Some(e) => e,
        None => return,
    };

    if lower_container_assign(ctx, &var_rec, &var_name, &s.value, &s.range) {
        return;
    }

    // Buffer-backed (entity-family) var: wire value directly into Input.
    if var_rec.storage == VarStorage::Buffer {
        let value_port = lower_expr(ctx, &s.value);
        ctx.connect(value_port, var_rec.node_id.port(WirePort::Input));
        return;
    }

    // Optimization: `x = x + <expr>` → Exec_Var_Increment
    if let Some((delta_expr, negate)) = match_increment_self(ctx, s) {
        let (delta, increment) = increment_value(ctx, s, delta_expr, negate, Binding::Var(var_rec.clone()));
        if !increment {
            set_scalar_var(ctx, &var_rec, delta, &s.range);
            return;
        }
        // Re-read current_exec after lowering the delta — lower_expr may
        // have advanced the chain via Var_Get / nested exec-taking ops.
        let exec_in = ctx.current_exec.unwrap_or(current_exec);
        let inner = var_rec.inner_type.clone();
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::VAR_INCREMENT,
            source_range: s.range.clone(),
            ports: GateIO::var_write(inner.clone()),
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
        ports: GateIO::var_write(inner.clone()),
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

/// Whole-container assignment to an array or map var `name`, shared by the bare
/// `xs = ..` target and the field-chain `ns.xs = ..` / `rec.xs = ..` ones.
/// Returns false, emitting nothing, when `var_rec` is not a container.
pub(super) fn lower_container_assign(
    ctx: &mut LowerCtx,
    var_rec: &VarRecord,
    name: &str,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    let what = match var_rec.storage {
        VarStorage::Array => "array",
        VarStorage::Map => "map",
        _ => return false,
    };
    match value {
        // `foo = [items, ...spreads]` on an array var: rebuild the contents at
        // runtime. There's no single "set array" gate, so clear it then push
        // each item / append each spread in order.
        Expr::Array { .. } if var_rec.storage == VarStorage::Array => {
            let target = Binding::Var(var_rec.clone());
            rebuild_container(ctx, &target, name, value, range, &mut lower_in_place(value, false));
        }
        // `foo = { k => v, ... }` on a map var: rebuild the contents at runtime.
        // There's no single "set map" gate, so clear it then set each entry in
        // order (mirrors the array literal assign above).
        Expr::MapLit { .. } if var_rec.storage == VarStorage::Map => {
            let target = Binding::Var(var_rec.clone());
            rebuild_container(ctx, &target, name, value, range, &mut lower_in_place(value, false));
        }
        // Assigning a whole container from a non-literal expression (`m = m2`)
        // is UNCONDITIONALLY unsatisfiable: there is no whole-container
        // copy-by-assignment gate, and a container's backing gate only exposes
        // `ArrayVarRef`/`MapVarRef`, not `VarRef`. Falling through to the
        // generic Var_Set path would wire a nonexistent source port into the
        // gate, which does nothing and only shows up as a load failure in-game.
        // Since no fallback can ever satisfy it, this is a hard ERROR (mirrors
        // WS021's "would silently produce a placeholder → error" policy), not a
        // droppable warning. `.copyFrom(src)` is the supported way to copy a
        // whole container.
        _ => ctx.diagnostics.push(Diagnostic::error(
            "WS027",
            format!(
                "assigning a whole {what} from a non-literal is not supported; use {name}.copyFrom(src) instead"
            ),
            value.range().clone(),
        )),
    }
    true
}

/// The dotted spelling of an assignment target (`u.xs`, `rec.xs`), for
/// diagnostics.
fn target_path(e: &Expr) -> String {
    match e {
        Expr::Ident { name, .. } => name.to_string(),
        Expr::FieldAccess { obj, field, .. } => format!("{}.{field}", target_path(obj)),
        Expr::TuplePick { obj, index, .. } => format!("{}.{index}", target_path(obj)),
        _ => "_".to_string(),
    }
}

/// One element of a container literal, ready for its rebuild.
pub(super) enum Supplied {
    Item(ElemValue),
    Entry(PortRef, ElemValue),
    /// The element has no value of the form its container stores.
    Missing,
    /// Nothing to write for this element.
    Skip,
}

pub(super) enum ElemValue {
    Scalar(PortRef),
    Record(HashMap<crate::intern::Sym, Binding>),
}

/// Rebuild the container `target` (an array or map var, or the columns of a
/// record array or map) from the literal `value`: clear it, then push each
/// item and append each spread, or set each entry, in order. `supply` gives the
/// item or entry at an index when the rebuild reaches it, so a caller can lower
/// it there ([`lower_in_place`]) or hand over one it evaluated earlier. A
/// leading `...name` keeps the old contents in place of the `Clear` (see
/// [`self_spread`]).
pub(super) fn rebuild_container(
    ctx: &mut LowerCtx,
    target: &Binding,
    name: &str,
    value: &Expr,
    range: &SourceRange,
    supply: &mut dyn FnMut(&mut LowerCtx, usize) -> Supplied,
) {
    use crate::lower::access::{
        array_exec_op, lower_record_array_method, lower_record_map_method, map_exec_op,
        record_array_value_op, record_map_set_walk,
    };
    match (target, value) {
        (Binding::Var(v), Expr::Array { elements, .. }) => {
            let aref = v.node_id.port(WirePort::ArrayVarRef);
            let keep = self_spread(ctx, v.node_id, name, elements);
            if !keep {
                lower_array_method(ctx, aref, v.inner_type.clone(), "clear", &[], range, value);
            }
            for (i, el) in elements.iter().enumerate().skip(usize::from(keep)) {
                if let ArrayElem::Spread(e) = el {
                    let arg = [CallArg::Positional(e.clone())];
                    lower_array_method(ctx, aref, v.inner_type.clone(), "append", &arg, el.range(), value);
                } else if let Supplied::Item(ElemValue::Scalar(port)) = supply(ctx, i) {
                    let extra = vec![(WirePort::Value, Type::Any, port)];
                    array_exec_op(ctx, el.range(), aref, gc::ARRAY_PUSH, extra, vec![], WirePort::ExecOut);
                }
            }
        }
        (Binding::Var(v), Expr::MapLit { entries, .. }) => {
            if ctx.current_exec.is_none() {
                return;
            }
            let mref = v.node_id.port(WirePort::MapVarRef);
            map_exec_op(ctx, range, mref, gc::MAP_CLEAR, vec![], vec![], WirePort::ExecOut);
            for i in 0..entries.len() {
                if let Supplied::Entry(k, ElemValue::Scalar(val)) = supply(ctx, i) {
                    let extra = vec![(WirePort::Key, Type::Any, k), (WirePort::Value, Type::Any, val)];
                    map_exec_op(ctx, range, mref, gc::MAP_SET, extra, vec![], WirePort::ExecOut);
                }
            }
        }
        (Binding::Record(cols), Expr::Array { elements, .. }) => {
            lower_record_array_method(ctx, cols, "clear", &[], range, value);
            for (i, el) in elements.iter().enumerate() {
                if let ArrayElem::Spread(e) = el {
                    let arg = [CallArg::Positional(e.clone())];
                    lower_record_array_method(ctx, cols, "append", &arg, el.range(), value);
                    continue;
                }
                let pushed = match supply(ctx, i) {
                    Supplied::Item(ElemValue::Record(src)) => {
                        match fit_record_value(ctx, "this element", el.expr(), cols, src) {
                            Some(src) => record_array_value_op(ctx, cols, &src, el.range(), gc::ARRAY_PUSH, None).is_some(),
                            None => true,
                        }
                    }
                    Supplied::Skip => true,
                    _ => false,
                };
                if !pushed {
                    crate::lower::expr::synthesise_unsupported(ctx, value);
                }
            }
        }
        (Binding::Record(cols), Expr::MapLit { entries, .. }) => {
            lower_record_map_method(ctx, cols, "clear", &[], range, value);
            for (i, e) in entries.iter().enumerate() {
                match supply(ctx, i) {
                    Supplied::Entry(k, ElemValue::Record(src)) => {
                        if let Some(src) = fit_record_value(ctx, "this entry's value", &e.value, cols, src) {
                            record_map_set_walk(ctx, cols, &src, k, range);
                        }
                    }
                    Supplied::Missing => report_non_record_map_entry(ctx, e.value.range()),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// [`rebuild_container`]'s supply for a literal whose elements are lowered
/// where the rebuild reaches them. `columns` for a record array or map.
pub(super) fn lower_in_place(value: &Expr, columns: bool) -> impl FnMut(&mut LowerCtx, usize) -> Supplied + '_ {
    move |ctx, i| match value {
        Expr::Array { elements, .. } => match &elements[i] {
            ArrayElem::Item(e) if columns => {
                value_record_fields(ctx, e).map_or(Supplied::Missing, |r| Supplied::Item(ElemValue::Record(r)))
            }
            ArrayElem::Item(e) => Supplied::Item(ElemValue::Scalar(lower_expr(ctx, e))),
            ArrayElem::Spread(_) => Supplied::Skip,
        },
        Expr::MapLit { entries, .. } if !columns => {
            let key = lower_expr(ctx, &entries[i].key);
            Supplied::Entry(key, ElemValue::Scalar(lower_expr(ctx, &entries[i].value)))
        }
        Expr::MapLit { entries, .. } if ctx.current_exec.is_some() => {
            let Some(rec) = value_record_fields(ctx, &entries[i].value) else {
                return Supplied::Missing;
            };
            Supplied::Entry(lower_expr(ctx, &entries[i].key), ElemValue::Record(rec))
        }
        _ => Supplied::Skip,
    }
}

/// Whether an array literal assigned to the array `node` (spelled `name`)
/// starts with a spread of that same array, which then keeps its contents in
/// place of a `Clear` and an append of itself; later elements are added after
/// them in order. A self-spread anywhere else would read the array after the
/// rebuild has cleared or added to it, and is WS088.
pub(super) fn self_spread(ctx: &mut LowerCtx, node: NodeId, name: &str, elements: &[ArrayElem]) -> bool {
    let is_self = |ctx: &LowerCtx, el: &ArrayElem| {
        matches!(el, ArrayElem::Spread(e)
            if matches!(resolve_field_chain(ctx, e), Some(Binding::Var(v)) if v.node_id == node))
    };
    let keep = elements.first().is_some_and(|el| is_self(ctx, el));
    if let Some(el) = elements.iter().skip(1).find(|el| is_self(ctx, el)) {
        let spelled = target_path(el.expr());
        ctx.error(
            "WS088",
            format!(
                "`...{spelled}` spreads `{name}` into itself after the literal's first element, \
                 when this assignment has already cleared or added to `{name}`, so it would not \
                 add the old contents. Only a leading `...{spelled}` keeps them; to put values \
                 in front, use `{name}.insert(0, v)`, or copy `{name}` into another array first"
            ),
            el.range(),
        );
    }
    keep
}

pub(super) fn lower_if(ctx: &mut LowerCtx, s: &If) {
    if ctx.current_exec.is_none() {
        return;
    }

    // A const-evaluable condition selects one block at compile time. This
    // lowers the taken block STRAIGHT INTO THE PARENT SCOPE — no Branch gate,
    // and so no Var_Get cache snapshot/restore (see this function's doc
    // comment) — which is what makes it safe. Do not reimplement this as
    // post-hoc deletion of an already-lowered branch. This covers a bare
    // `true`/`false` and, more generally, every compile-time-decidable
    // condition built from const-DECLARED names (operators over them,
    // certified method calls, ...) via `const_eval::eval_expr` — the same
    // evaluator `typecheck::stmt`'s `Stmt::If` arm uses to decide which
    // block NOT to check, so the two sides agree on exactly what's
    // constant. `if_cond_const_ctx` restricts the environment to names
    // actually spelled `const` (NOT a plain `let` that merely happens to
    // fold — see its doc comment): a program using no `const` must compile
    // identically, so a condition built from a plain `let` falls straight
    // through to the general Branch path below. A separate case: an ident
    // bound to a literal-bool GATE (e.g. a plain mod param called with a
    // literal argument) is not something `const_eval` can see — see
    // `ident_literal_bool`'s doc comment — so it stays a narrower fallback
    // below, unaffected by the const-declared restriction (it never reads
    // `const_lookup` at all). Only when NOT under `@nofold` — that
    // annotation promises "nothing folded or elided", so a
    // `@nofold`-scoped `if true {...}` must still lower a real Branch (fall
    // through to the general path below).
    if ctx.nofold_depth == 0 {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        let cond_cx = ctx.if_cond_const_ctx(Some(&lookup));
        let cond_result = crate::const_eval::eval_expr(&s.cond, &cond_cx, &mut budget).ok();
        // `const_eval` only ever resolves NAMED constants (`const`/`let`
        // bindings, and operators/calls over them) — it has no notion of a
        // lowered gate's value, so it can't see a plain (non-const) mod
        // param that happens to be bound to a literal-bool ARGUMENT at this
        // call site (e.g. `mod foo(cond: bool) { if cond {...} }` called as
        // `foo(true)`: `cond` binds to the `_Literal` gate `lower_expr`
        // built for that argument). `ident_literal_bool` reads that gate
        // directly as a narrower, SEPARATE fallback — deliberately NOT
        // folded into `const_eval`'s consts environment, so that evaluator's
        // meaning stays "named constants only". Its hits are NOT recorded
        // into `dropped_ranges` below: typecheck checks a mod body once,
        // generically, per declaration — never re-checked per call site — so
        // it has no way to know this particular call passed a literal, and
        // always checks both branches here regardless. That's the safe
        // over-checking direction (never under-checking), so leaving this
        // case untracked cannot let a branch be lowered without having been
        // checked.
        let taken = match cond_result {
            Some(Literal::Bool(taken)) => {
                // Record the range of the block that was skipped, mirroring
                // `typecheck::stmt`'s `Stmt::If` arm exactly (see
                // `typecheck_and_lowering_drop_exactly_the_same_ranges` in
                // `typecheck::tests`) — a taken THEN with no `else` drops
                // nothing, since there is no second block to skip.
                if taken {
                    if let Some(else_b) = &s.else_block {
                        ctx.dropped_ranges.push(else_b.range.clone());
                    }
                } else {
                    ctx.dropped_ranges.push(s.then_block.range.clone());
                }
                Some(taken)
            }
            _ => ident_literal_bool(ctx, &s.cond),
        };
        if let Some(taken) = taken {
            ctx.push_scope(crate::scope::ScopeTag::BLOCK);
            if taken {
                lower_block(ctx, &s.then_block);
            } else if let Some(else_b) = &s.else_block {
                lower_block(ctx, else_b);
            }
            ctx.pop_scope();
            return;
        }
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
        ports: GateIO::exec_branch(),
        ..Default::default()
    });
    ctx.connect(branch_exec_in, branch.port(WirePort::Exec));
    ctx.connect(cond_port, branch.port(WirePort::BCond));

    // Snapshot every var's Var_Get cache before the branches. At the join only
    // the vars a branch actually wrote get invalidated, so an unwritten var's
    // pre-branch read survives instead of the whole cache being blanket-cleared
    // — Var_Get is the single most common gate, and this drops the redundant
    // re-reads. `cache_touched_since` compares actual cache state, so any write
    // (direct, nested-if, inline mod, chip-instance ref arg) is caught.
    let pre_branch_caches = snapshot_var_caches(ctx);

    // Snapshot scope before branches so that declarations in one
    // branch don't leak into the other (each branch gets its own scope).
    ctx.push_scope(crate::scope::ScopeTag::BLOCK);

    // IfThen — sibling of IfCond under IfGroup.
    ctx.builder.current_scope_id = if_group_id;
    let then_id = ctx.alloc_scope(ScopeKind::IfThen, s.then_block.range.clone());
    ctx.builder.current_scope_id = then_id;
    ctx.current_exec = Some(branch.port(WirePort::ExecOutA));
    ctx.exec_branch_depth += 1;
    lower_block(ctx, &s.then_block);
    let then_end = ctx.current_exec;
    let then_touched = cache_touched_since(ctx, &pre_branch_caches);

    // Restore scope so the else branch starts from the same state.
    ctx.pop_scope();
    ctx.push_scope(crate::scope::ScopeTag::BLOCK);

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
    // A Var_Get emitted in the THEN branch lives on the ExecOutA chain, so it
    // must not be reused on the ELSE chain (ExecOutB) - it never fires there.
    // Restoring the pre-branch caches drops exactly those THEN-created reads
    // while KEEPING the pre-branch reads, which fired before the split and so
    // dominate the ELSE chain too - the else branch can reuse them.
    restore_var_caches(ctx, &pre_branch_caches);
    if let Some(else_b) = &s.else_block {
        lower_block(ctx, else_b);
    }
    ctx.exec_branch_depth -= 1;
    let else_end = ctx.current_exec;
    let else_touched = cache_touched_since(ctx, &pre_branch_caches);

    // Restore scope so post-if code sees the pre-branch state.
    // Variables declared inside branches are not visible after the if.
    ctx.pop_scope();

    // The join/union below is post-branch flow; back to the outer scope.
    ctx.builder.current_scope_id = outer_scope;

    let union = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: s.range.clone(),
        ports: GateIO::exec_join(),
        ..Default::default()
    });
    if let Some(e) = then_end {
        ctx.connect(e, union.port(WirePort::ExecA));
    }
    if let Some(e) = else_end {
        ctx.connect(e, union.port(WirePort::ExecB));
    }
    ctx.current_exec = Some(union.port(WirePort::ExecOut));
    // Post-join cache state: restore the pre-branch reads (a Var_Get created
    // inside either branch only fires when that branch is taken, so it can't be
    // reused after the join — restoring drops them), then invalidate every var
    // EITHER branch wrote. A var neither branch touched keeps its pre-branch
    // read; a written one re-reads fresh on the (dominating) post-join chain —
    // otherwise it would read a stale value whenever the writing branch wasn't
    // taken (a total first read in one `if phase == …` block and re-read in a
    // later one).
    restore_var_caches(ctx, &pre_branch_caches);
    for id in then_touched.iter().chain(else_touched.iter()) {
        invalidate_var_cache(ctx, id);
    }
}

/// Lower a `match` used as a STATEMENT (a live exec chain, block-bodied arms)
/// `if let <pattern> = <scrutinee> { then } else { else }` - a refutable-pattern
/// conditional. It desugars to a two-arm match (`<pattern> => then`, `_ => else`)
/// - or a one-arm match with a `Fail` pass-through when there is no `else` - and
/// routes straight through [`lower_match_stmt`], so the `disc == variant`
/// Branch, the ExecOutA/ExecOutB split, the rejoining `Union`, and the capture
/// binding are all the shared match machinery, not a second walker.
pub(super) fn lower_if_let(ctx: &mut LowerCtx, i: &IfLet) {
    let mut arms = vec![MatchArm {
        pattern: i.pattern.clone(),
        body: MatchBody::Block(i.then_block.clone()),
        range: i.then_block.range.clone(),
    }];
    if let Some(else_b) = &i.else_block {
        arms.push(MatchArm {
            pattern: Pattern::Wildcard(else_b.range.clone()),
            body: MatchBody::Block(else_b.clone()),
            range: else_b.range.clone(),
        });
    }
    lower_match_stmt(ctx, &i.scrutinee, &arms, &i.range);
}

/// `let <pattern> = <scrutinee> else { <diverge> }` - a refutable binding that
/// runs the diverging `else` when it fails. It walks the SAME one-arm `Decision`
/// (`matchtree::build`) `if let` drives through `lower_match_stmt`, so EVERY
/// disc mismatch at ANY nesting level routes to the `else` - a nested pattern
/// like `Some(Some(x))` tests both discs, not just the outer one. Unlike `if
/// let`, the paths do NOT rejoin through a `Union`: the fully-matching leaf
/// binds all captures into the CURRENT scope (visible to the rest of the
/// enclosing block) and continues `current_exec`; every non-matching path runs
/// the `else` block (which typecheck forced to diverge - WS062), so its exec
/// never returns to the main chain.
pub(super) fn lower_let_else(ctx: &mut LowerCtx, l: &LetElse) {
    // Pure position (no exec chain): nothing to thread, same as `lower_if`.
    let Some(entry_exec) = ctx.current_exec else {
        return;
    };
    let range = l.range.clone();
    let scrut_ty = crate::lower::expr::scrutinee_type(ctx, &l.scrutinee);

    // The one-arm decision tree for the FULL (possibly nested) pattern. Every
    // `Switch` case leads deeper toward the single `Leaf`; every `Fail`/default
    // is a mismatch that routes to the diverging `else`.
    let decision = crate::lower::matchtree::build(
        ctx.enums(),
        &scrut_ty,
        std::slice::from_ref(&l.pattern),
    );

    // The scrutinee's `__disc` + payload-slot record (a NAMED var/let/param
    // through the scope). An enum I/O port has no such record - loud placeholder.
    let Some(root) = match_scrutinee_record(ctx, &l.scrutinee) else {
        synthesise_unsupported_range(ctx, "a `let ... else` on a value with no enum tag", &range);
        return;
    };
    // A scrutinee that is a call ran on the chain; the test follows it.
    let entry_exec = ctx.current_exec.unwrap_or(entry_exec);

    ctx.current_exec = Some(entry_exec);
    let cont = lower_let_else_decision(ctx, &decision, &root, &l.pattern, &l.else_block, entry_exec, &range);
    // Continue on the fully-matched path. A decision with no reachable match
    // leaf (should not happen for a well-typed single-variant pattern) leaves
    // exec on the entry so following statements are not silently stranded.
    ctx.current_exec = cont.or(Some(entry_exec));
}

/// Walk one `Decision` node for a `let ... else`, entering on `entry_exec`.
/// Returns the exec on which the FULLY-matched path continues (there is exactly
/// one such leaf), or `None` for a path that reaches the diverging `else`.
fn lower_let_else_decision(
    ctx: &mut LowerCtx,
    decision: &crate::lower::matchtree::Decision,
    root: &HashMap<crate::intern::Sym, Binding>,
    pattern: &Pattern,
    else_block: &Block,
    entry_exec: PortRef,
    range: &SourceRange,
) -> Option<PortRef> {
    use crate::lower::matchtree::Decision;
    match decision {
        Decision::Leaf(_) => {
            // Full match: bind every capture (all nesting levels) as a
            // compile-time slot move into the CURRENT scope, then continue here.
            ctx.current_exec = Some(entry_exec);
            let mut captures = Vec::new();
            collect_pattern_captures(pattern, &mut Vec::new(), &mut captures);
            for (name, slot_path) in captures {
                if let Some(binding) = navigate_capture(root, &slot_path) {
                    ctx.scope.insert(&name, binding);
                }
                ctx.consts.forget(&name);
            }
            Some(entry_exec)
        }
        // A disc mismatch at some level: run the diverging `else`; never rejoin.
        Decision::Fail => {
            ctx.current_exec = Some(entry_exec);
            lower_block(ctx, else_block);
            None
        }
        Decision::Switch { path, cases, default } => {
            ctx.current_exec = Some(entry_exec);
            let disc_port = read_disc_at_path(ctx, root, path, range)
                .unwrap_or_else(|| synthesise_unsupported_range(ctx, "an enum tag read", range));
            let head_exec = ctx.current_exec.unwrap_or(entry_exec);
            lower_let_else_switch(ctx, disc_port, cases, default.as_deref(), root, pattern, else_block, head_exec, range)
        }
    }
}

/// Chain a `Switch`'s cases as `Branch`es on `disc == k` for a `let ... else`.
/// The matching case's sub-decision runs on `ExecOutA` (toward the leaf), the
/// remaining cases/default on `ExecOutB` (toward the `else`). No `Union`: the
/// single match leaf's continuation propagates up, and each `else` path
/// diverges. The `else` chain's Var_Get reads fire only on its `ExecOutB`, so
/// they are snapshot/restored around it and never leak to the match path.
#[allow(clippy::too_many_arguments)]
fn lower_let_else_switch(
    ctx: &mut LowerCtx,
    disc_port: PortRef,
    cases: &[(i64, crate::lower::matchtree::Decision)],
    default: Option<&crate::lower::matchtree::Decision>,
    root: &HashMap<crate::intern::Sym, Binding>,
    pattern: &Pattern,
    else_block: &Block,
    entry_exec: PortRef,
    range: &SourceRange,
) -> Option<PortRef> {
    let Some(((k, sub), rest)) = cases.split_first() else {
        // Past the last case: the default sub-decision (a `Fail` -> `else`), or
        // the `else` directly when there is no default.
        return match default {
            Some(d) => lower_let_else_decision(ctx, d, root, pattern, else_block, entry_exec, range),
            None => {
                ctx.current_exec = Some(entry_exec);
                lower_block(ctx, else_block);
                None
            }
        };
    };

    ctx.current_exec = Some(entry_exec);
    let cond_port = emit_disc_eq(ctx, disc_port, *k, range);
    let branch_exec_in = ctx.current_exec.unwrap_or(entry_exec);
    let branch = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BRANCH,
        source_range: range.clone(),
        ports: GateIO::exec_branch(),
        ..Default::default()
    });
    ctx.connect(branch_exec_in, branch.port(WirePort::Exec));
    ctx.connect(cond_port, branch.port(WirePort::BCond));

    // NON-matching (ExecOutB): the remaining cases, ending in the diverging
    // `else`. Bracket its var caches so its reads never leak to the match path.
    let pre_branch_caches = snapshot_var_caches(ctx);
    let else_cont = lower_let_else_switch(
        ctx,
        disc_port,
        rest,
        default,
        root,
        pattern,
        else_block,
        branch.port(WirePort::ExecOutB),
        range,
    );
    restore_var_caches(ctx, &pre_branch_caches);

    // MATCHING (ExecOutA): descend toward the leaf - this carries the
    // continuation. Lowered last so `current_exec` ends on the match path.
    let match_cont =
        lower_let_else_decision(ctx, sub, root, pattern, else_block, branch.port(WirePort::ExecOutA), range);

    match_cont.or(else_cont)
}

/// to an exec-threaded `Branch`/`Union` tree - the statement twin of
/// `lower_match_expr`. Both walk the same Task-13 `Decision`: a `Switch` reads
/// `__disc` at its path and each case becomes an `if disc == k { arm } else
/// { rest }` (a `Branch`, the arm on `ExecOutA`, the remaining cases/default on
/// `ExecOutB`, the two exits rejoined by a `Union`). The returned port is the
/// final exec continuation; the `Stmt::ExprStmt` caller discards it and reads
/// `current_exec` for what follows.
pub(super) fn lower_match_stmt(
    ctx: &mut LowerCtx,
    scrutinee: &Expr,
    arms: &[MatchArm],
    range: &SourceRange,
) -> PortRef {
    // Mirror `lower_if`'s guard: no exec chain, nothing to thread.
    let Some(entry_exec) = ctx.current_exec else {
        return synthesise_unsupported_range(ctx, "a `match` with block arms outside an exec chain", range);
    };
    let scrut_ty = crate::lower::expr::scrutinee_type(ctx, scrutinee);
    let arm_patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
    let decision = crate::lower::matchtree::build(ctx.enums(), &scrut_ty, &arm_patterns);

    // The scrutinee resolves to its `__disc` + payload-slot `Binding::Record`:
    // a NAMED scrutinee through the scope, an INLINE construction by lowering
    // it (see `match_scrutinee_record`, shared with `lower_match_expr`).
    // A scrutinee with no record decomposition (an enum INPUT port, a
    // typecheck-error program) is a loud `_Unsupported` placeholder.
    let Some(root) = match_scrutinee_record(ctx, scrutinee) else {
        return synthesise_unsupported_range(ctx, "a `match` on a value with no enum tag", range);
    };
    // A scrutinee that is a call ran on the chain; the test follows it.
    let entry_exec = ctx.current_exec.unwrap_or(entry_exec);

    // Const-elision fast path: the statement twin of
    // `lower_match_expr`'s own -- see that function's doc comment for the
    // `nofold_depth`/`if_cond_const_ctx` rationale, identical here. On
    // success, every arm OTHER than the taken one is dropped: none of their
    // statements ran, so typecheck's blanket "check every arm" (there is no
    // const-elision on that side for `match` -- every arm is always checked,
    // the safe over-checking direction) still agrees with lowering on which
    // ranges never executed, matching `lower_if`'s `dropped_ranges` bookkeeping.
    if ctx.nofold_depth == 0
        && let Some(leaf) = try_const_decision(ctx, &decision, scrutinee)
    {
        let taken = match &leaf {
            crate::lower::matchtree::Decision::Leaf(i) => Some(*i),
            _ => None,
        };
        for (j, arm) in arms.iter().enumerate() {
            if Some(j) != taken {
                ctx.dropped_ranges.push(arm.range.clone());
            }
        }
        let exit = lower_decision_stmt(ctx, &leaf, &root, arms, entry_exec, range);
        ctx.current_exec = exit;
        return exit.unwrap_or(entry_exec);
    }

    let exit = lower_decision_stmt(ctx, &decision, &root, arms, entry_exec, range);
    ctx.current_exec = exit;
    exit.unwrap_or(entry_exec)
}

/// Thread exec through one `Decision`, entering on `entry_exec`. Returns the
/// exit exec, or `None` when the taken path terminates its own chain (e.g. a
/// buffered `emit` as an arm's last statement).
fn lower_decision_stmt(
    ctx: &mut LowerCtx,
    decision: &crate::lower::matchtree::Decision,
    root: &HashMap<crate::intern::Sym, Binding>,
    arms: &[MatchArm],
    entry_exec: PortRef,
    range: &SourceRange,
) -> Option<PortRef> {
    use crate::lower::matchtree::Decision;
    match decision {
        Decision::Leaf(i) => {
            ctx.current_exec = Some(entry_exec);
            lower_match_arm_stmt(ctx, &arms[*i], root);
            ctx.current_exec.take()
        }
        // No arm matched (only reachable for a non-exhaustive match, already a
        // WS054): the exec falls through unchanged, running nothing.
        Decision::Fail => Some(entry_exec),
        Decision::Switch { path, cases, default } => {
            ctx.current_exec = Some(entry_exec);
            let disc_port = read_disc_at_path(ctx, root, path, range)
                .unwrap_or_else(|| synthesise_unsupported_range(ctx, "an enum tag read", range));
            // Reading `__disc` may have inserted an exec-taking `Var_Get`; the
            // first branch's Exec input picks up from the advanced chain head.
            let head_exec = ctx.current_exec.unwrap_or(entry_exec);
            lower_switch_cases(ctx, disc_port, cases, default.as_deref(), root, arms, head_exec, range)
        }
    }
}

/// Chain a `Switch`'s cases as nested `Branch`es on `disc == k`: the head case's
/// arm runs on `ExecOutA`, the remaining cases (then the `default`, or a `Fail`
/// pass-through) on `ExecOutB`, and the two exits rejoin through a `Union`. The
/// snapshot/restore var-cache discipline is `lower_if`'s: only vars an arm
/// actually wrote get invalidated at the join, so a read that dominates the
/// split survives while a branch-local read never leaks to a sibling.
#[allow(clippy::too_many_arguments)]
fn lower_switch_cases(
    ctx: &mut LowerCtx,
    disc_port: PortRef,
    cases: &[(i64, crate::lower::matchtree::Decision)],
    default: Option<&crate::lower::matchtree::Decision>,
    root: &HashMap<crate::intern::Sym, Binding>,
    arms: &[MatchArm],
    entry_exec: PortRef,
    range: &SourceRange,
) -> Option<PortRef> {
    let Some(((k, sub), rest)) = cases.split_first() else {
        // Past the last case: the default sub-decision, or a Fail pass-through.
        return match default {
            Some(d) => lower_decision_stmt(ctx, d, root, arms, entry_exec, range),
            None => Some(entry_exec),
        };
    };

    let pre_branch_caches = snapshot_var_caches(ctx);

    ctx.current_exec = Some(entry_exec);
    let cond_port = emit_disc_eq(ctx, disc_port, *k, range);
    // Building the compare is pure, but re-read the chain head the way
    // `lower_if` does so a future exec-taking disc path stays wired correctly.
    let branch_exec_in = ctx.current_exec.unwrap_or(entry_exec);
    let branch = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BRANCH,
        source_range: range.clone(),
        ports: GateIO::exec_branch(),
        ..Default::default()
    });
    ctx.connect(branch_exec_in, branch.port(WirePort::Exec));
    ctx.connect(cond_port, branch.port(WirePort::BCond));

    // THEN: this case's arm (or nested switch) on ExecOutA.
    let then_end = lower_decision_stmt(ctx, sub, root, arms, branch.port(WirePort::ExecOutA), range);
    let then_touched = cache_touched_since(ctx, &pre_branch_caches);

    // ELSE: the remaining cases / default on ExecOutB. Drop the THEN chain's
    // scratch reads first - they only fire on ExecOutA.
    restore_var_caches(ctx, &pre_branch_caches);
    let else_end = lower_switch_cases(
        ctx,
        disc_port,
        rest,
        default,
        root,
        arms,
        branch.port(WirePort::ExecOutB),
        range,
    );
    let else_touched = cache_touched_since(ctx, &pre_branch_caches);

    let union = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: range.clone(),
        ports: GateIO::exec_join(),
        ..Default::default()
    });
    if let Some(e) = then_end {
        ctx.connect(e, union.port(WirePort::ExecA));
    }
    if let Some(e) = else_end {
        ctx.connect(e, union.port(WirePort::ExecB));
    }
    // Post-join cache state: restore the pre-branch reads, then invalidate every
    // var either side wrote (mirrors `lower_if`'s join).
    restore_var_caches(ctx, &pre_branch_caches);
    for id in then_touched.iter().chain(else_touched.iter()) {
        invalidate_var_cache(ctx, id);
    }
    Some(union.port(WirePort::ExecOut))
}

/// Lower one arm's body on the current exec chain, its payload captures bound as
/// compile-time slot moves (no gate) - the statement twin of `lower_match_arm`.
fn lower_match_arm_stmt(
    ctx: &mut LowerCtx,
    arm: &MatchArm,
    root: &HashMap<crate::intern::Sym, Binding>,
) {
    // Only this match's own arms store its value, not a match nested in one.
    let store = ctx.match_value_store.take();
    crate::lower::expr::with_arm_captures(ctx, arm, root, |ctx| match &arm.body {
        MatchBody::Block(block) => lower_block(ctx, block),
        // A statement-position expression arm runs for its side effects; its
        // value has no consumer (the match statement yields nothing). A value
        // match's arm stores its value.
        MatchBody::Expr(expr) => {
            let port = lower_expr(ctx, expr);
            if let Some(var) = &store {
                set_scalar_var(ctx, var, port, expr.range());
            }
        }
    });
    ctx.match_value_store = store;
}

/// A `match` used as a value whose arms include a block, which carries no value
/// and leaves through a `return` (typecheck's WS095): the arms run as the
/// statement form's Branch/Union tree, each value arm storing its value into a
/// scratch var on its own chain, and the value is that var read after the arms
/// rejoin.
pub(super) fn lower_match_value(
    ctx: &mut LowerCtx,
    scrutinee: &Expr,
    arms: &[MatchArm],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let ty = unwrap_ref(&ctx.type_of(e));
    if matches!(ty, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }) {
        if arms.iter().any(|a| matches!(a.body, MatchBody::Block(_))) {
            ctx.error(
                "WS095",
                "a `match` whose value is a record, tuple or enum can't have a block arm: bind \
                 the value with a statement `match` whose arms assign a `var`",
                range,
            );
        } else {
            report_effectful_aggregate_choice(ctx, range);
        }
        return synthesise_unsupported_no_warn(ctx, range);
    }
    let mut props = HashMap::default();
    props.insert(*sym::NAME_LABEL, Literal::String("_match".to_string()));
    let node = crate::lower::predeclare::make_scalar_var_gate(ctx, &ty, props, range);
    let var = VarRecord {
        node_id: node,
        inner_type: ty,
        get_node_for_handler: None,
        storage: VarStorage::Var,
    };
    let saved = ctx.match_value_store.replace(var.clone());
    lower_match_stmt(ctx, scrutinee, arms, range);
    ctx.match_value_store = saved;
    binding_to_port(ctx, &Binding::Var(var), range).unwrap_or_else(|| synthesise_unsupported_no_warn(ctx, range))
}

/// WS097: an `if` or `match` expression whose value is several wires has no
/// scratch var to choose through, so an arm that acts cannot be confined to
/// its branch.
pub(super) fn report_effectful_aggregate_choice(ctx: &mut LowerCtx, range: &SourceRange) {
    ctx.error(
        "WS097",
        "an `if` or `match` expression whose value is a record, tuple or enum can't run an arm \
         that does more than give its value (a write, a send, a `return`, or a call to a `mod` \
         that does one): use an `if` or `match` statement whose arms assign a `var`",
        range,
    );
}

/// WS097 for a field read of such an `if` or `match` expression
/// (`(if c then mk() else p).a`): the read itself is one wire, but the arms
/// give records, so the arm that acts has nowhere to be confined.
pub(super) fn report_effectful_aggregate_field_read(ctx: &mut LowerCtx, field: &str, range: &SourceRange) {
    ctx.error(
        "WS097",
        format!(
            "this reads `.{field}` of an `if` or `match` expression whose arms give records, tuples or \
             enums, and an arm does more than give its value (a write, a send, a `return`, or a call \
             to a `mod` that does one), which can't be confined to its branch here: read `.{field}` \
             in each arm (`if c then mk().{field} else p.{field}`), or use an `if` or `match` \
             statement whose arms assign a `var`"
        ),
        range,
    );
}

/// An `if` expression on an exec chain whose branch does more than give its
/// value (`expr::has_effects`: a write, a send, a `return`, a call to a `mod`
/// that does one): the branches run as an `if` statement's do, each storing
/// its value into a scratch var on its own chain, and the value is that var
/// read after they rejoin. A branch's effects then run only when it is taken,
/// where the pure `Select` form would run both on every evaluation.
pub(super) fn lower_if_value(
    ctx: &mut LowerCtx,
    cond: &Expr,
    then_br: &Expr,
    else_br: &Expr,
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let ty = unwrap_ref(&ctx.type_of(e));
    if matches!(ty, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }) {
        report_effectful_aggregate_choice(ctx, range);
        return synthesise_unsupported_no_warn(ctx, range);
    }
    let Some(entry) = ctx.current_exec else {
        return synthesise_unsupported_no_warn(ctx, range);
    };
    let mut props = HashMap::default();
    props.insert(*sym::NAME_LABEL, Literal::String("_if".to_string()));
    let node = crate::lower::predeclare::make_scalar_var_gate(ctx, &ty, props, range);
    let var = VarRecord {
        node_id: node,
        inner_type: ty,
        get_node_for_handler: None,
        storage: VarStorage::Var,
    };
    let cond_port = lower_expr(ctx, cond);
    let branch_exec_in = ctx.current_exec.unwrap_or(entry);
    let branch = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BRANCH,
        source_range: range.clone(),
        ports: GateIO::exec_branch(),
        ..Default::default()
    });
    ctx.connect(branch_exec_in, branch.port(WirePort::Exec));
    ctx.connect(cond_port, branch.port(WirePort::BCond));
    let pre_branch_caches = snapshot_var_caches(ctx);
    ctx.exec_branch_depth += 1;
    let arm = |ctx: &mut LowerCtx, start: WirePort, value: &Expr| {
        ctx.current_exec = Some(branch.port(start));
        let port = lower_expr(ctx, value);
        set_scalar_var(ctx, &var, port, value.range());
        let end = ctx.current_exec;
        let touched = cache_touched_since(ctx, &pre_branch_caches);
        restore_var_caches(ctx, &pre_branch_caches);
        (end, touched)
    };
    let (then_end, then_touched) = arm(ctx, WirePort::ExecOutA, then_br);
    let (else_end, else_touched) = arm(ctx, WirePort::ExecOutB, else_br);
    ctx.exec_branch_depth -= 1;
    let union = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: range.clone(),
        ports: GateIO::exec_join(),
        ..Default::default()
    });
    if let Some(end) = then_end {
        ctx.connect(end, union.port(WirePort::ExecA));
    }
    if let Some(end) = else_end {
        ctx.connect(end, union.port(WirePort::ExecB));
    }
    ctx.current_exec = Some(union.port(WirePort::ExecOut));
    for id in then_touched.iter().chain(else_touched.iter()) {
        invalidate_var_cache(ctx, id);
    }
    binding_to_port(ctx, &Binding::Var(var), range).unwrap_or_else(|| synthesise_unsupported_no_warn(ctx, range))
}

/// `cond` resolves (through the current scope) to a `_Literal` gate carrying
/// a bool — the case a plain (non-`const`) mod parameter ends up in when its
/// call-site argument was itself a literal (`mod foo(cond: bool) { if cond
/// {...} }` called as `foo(true)`): the param binds to whatever gate
/// `lower_expr` produced for the argument, which for a bare literal is a
/// `_Literal` node. `const_eval` cannot see this — its `consts` environment
/// only ever holds named `const`/`let` bindings, never a lowered gate's
/// value — so `lower_if` falls back to this narrower, direct-gate-inspection
/// check when `const_eval` reports the condition isn't constant.
fn ident_literal_bool(ctx: &LowerCtx, cond: &Expr) -> Option<bool> {
    let Expr::Ident { name, .. } = cond else {
        return None;
    };
    let Some(Binding::Local(local)) = ctx.scope.get(name).cloned() else {
        return None;
    };
    let node = ctx.builder.module.nodes.get(&local.port.node_id)?;
    if node.gate_class != gc::LITERAL {
        return None;
    }
    match node.properties.get(&*sym::VALUE) {
        Some(Literal::Bool(val)) => Some(*val),
        _ => None,
    }
}

/// A write to a stored mod output with no exec chain to sequence it on: the
/// storage is a var, and a var is only written on a chain.
pub(in crate::lower) fn report_unsequenced_output_write(ctx: &mut LowerCtx, name: &str, range: &SourceRange) {
    ctx.error(
        "WS007",
        format!(
            "output `{name}` is set in more than one place or under a branch, so choosing \
             which write it keeps needs an exec context: call the mod from a handler or a \
             `mod` body, or pass `exec = ...`"
        ),
        range,
    );
}

/// `emit name = value` into the inlined mod's return storage: the scalar
/// return var, or the output's field of the return record.
fn store_mod_output(ctx: &mut LowerCtx, name: &str, value: &Expr, range: &SourceRange) {
    if let Some(var) = ctx.mod_return_var.clone() {
        let port = lower_expr(ctx, value);
        if let Some(exec) = ctx.current_exec {
            let set = ctx.add_gate(AddNodeOpts {
                gate_class: gc::VAR_SET,
                source_range: range.clone(),
                note: Some("ret_set"),
                ports: GateIO::var_write(var.inner_type.clone()),
                ..Default::default()
            });
            ctx.connect(exec, set.port(WirePort::Exec));
            ctx.connect(var.node_id.port(WirePort::VarRef), set.port(WirePort::VarRef));
            ctx.connect(port, set.port(WirePort::Value));
            ctx.current_exec = Some(set.port(WirePort::ExecOut));
        }
        return;
    }
    let Some(ret) = ctx.mod_return_record.clone() else {
        return;
    };
    let key = crate::intern::intern(name);
    // The output's storage: its field of the per-output record, or the whole
    // record for the only output, which is then a record or enum.
    let several = ctx.mod_output_names.len() > 1;
    let target = if several {
        let Some(target) = ret.get(&key).cloned() else {
            return;
        };
        target
    } else {
        Binding::Record(ret.clone())
    };
    // Lowered once: into a record or enum output by field, anything else as
    // one port.
    let src = match &target {
        Binding::Record(tf) => {
            let what = format!("the value of `emit {name}`");
            let Some(src) = written_record_fields(ctx, &what, value) else {
                return;
            };
            let Some(src) = fit_record_value(ctx, &what, value, tf, src) else {
                return;
            };
            src
        }
        _ => {
            let mut v = HashMap::default();
            v.insert(key, Binding::Local(LocalRecord { port: lower_expr(ctx, value) }));
            let mut t = HashMap::default();
            t.insert(key, target);
            assign_record_fields(ctx, &t, &v, range);
            return;
        }
    };
    if several {
        let mut t = HashMap::default();
        t.insert(key, target);
        let mut v = HashMap::default();
        v.insert(key, Binding::Record(src));
        assign_record_fields(ctx, &t, &v, range);
    } else {
        assign_record_fields(ctx, &ret, &src, range);
    }
}

/// Whether an output of type `t` holds one value: a gate read written to it
/// gives it its first output.
pub(super) fn holds_one_value(t: &Type) -> bool {
    matches!(
        t,
        Type::Bool
            | Type::Int
            | Type::Float
            | Type::String
            | Type::Vector
            | Type::Rotator
            | Type::Quat
            | Type::Color
            | Type::Entity
            | Type::Character
            | Type::Controller
    )
}

pub(super) fn lower_emit(ctx: &mut LowerCtx, s: &Emit) {
    let is_output = ctx.lookup_output(&s.name).is_some()
        || matches!(
            ctx.scope.get(&crate::lower::context::output_scope_key(&s.name)),
            Some(Binding::Record(_))
        );
    // Local exec signals resolve to their per-declaration key via the scope,
    // so same-named signals in different bodies stay separate.
    let sig_key = ctx.signal_key(&s.name);
    if !is_output && sig_key.is_none() {
        // `emit` targets an `out` port or a `let ...: exec` signal. Every other
        // target has no wire and would otherwise compile to nothing: an `in`
        // port, a `var`, an undefined name, or an `out`/signal declared outside
        // an enclosing named `chip`, whose fresh scope cannot see it.
        ctx.error(
            "WS057",
            format!(
                "`emit {}` has no target in scope: `emit` fires an `out` port or a \
                 `let ...: exec` signal. An input port, a `var`, or an `out`/signal declared \
                 outside an enclosing named `chip` is not a valid target",
                s.name
            ),
            &s.range,
        );
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
        // An output of the mod being inlined that is written in more than one
        // place: store into the return storage the caller reads, so the write
        // on the path that ran is the one it sees.
        if ctx.mod_emit_storage.contains(&s.name) {
            if ctx.current_exec.is_some() {
                store_mod_output(ctx, &s.name, value_expr, &s.range);
            } else if !ctx.mod_unsequenced.contains(&s.name) {
                report_unsequenced_output_write(ctx, &s.name, &s.range);
            }
            return;
        }
        if is_output && ctx.output_tag(&s.name).is_some() {
            drive_enum_output(ctx, &s.name, value_expr, &s.range);
            return;
        }
        // A record output has no single port: route the record the way
        // `out name = rec` does (field pins on a chip, the record handed to
        // the caller from an inline mod). An output with no single port is
        // always one, and an inline mod hands an enum to its caller the same
        // way.
        let mod_output = ctx.mod_output_names.contains(&s.name);
        let value_ty = unwrap_ref(&ctx.type_of(value_expr));
        let declared_enum = matches!(ctx.mod_output_types.get(&s.name), Some(Type::Enum { .. }));
        // A record of gate outputs written to an output of one value is its
        // first output, as `x = e` reads it.
        let one_value = ctx
            .mod_output_types
            .get(&s.name)
            .or_else(|| ctx.lookup_output(&s.name).map(|o| &o.ty))
            .is_some_and(holds_one_value);
        if is_output
            && ((matches!(value_ty, Type::Record(_)) && !one_value)
                || ctx.lookup_output(&s.name).is_none()
                || (mod_output && (declared_enum || matches!(value_ty, Type::Enum { .. }))))
        {
            if mod_output {
                let what = format!("the value of `emit {}`", s.name);
                stash_mod_output_record(ctx, &s.name, &what, value_expr);
            } else {
                lower_out_binding(ctx, &s.name, Some(value_expr), &s.range);
            }
            return;
        }
        if let Some(out) = ctx.lookup_output(&s.name).cloned() {
            // An output value-driven from more than one `emit` site is
            // backed by a PseudoVar (allocated by the pre-scan in `lower`). Each
            // site does a Var_Set into it on its own exec chain; the var's value
            // feeds the output once, after all handlers (so no fan-in). Never
            // direct-wire a backed output — that wire would fan in with the
            // var→output wire.
            if let Some(backing) = ctx.output_backing_vars.get(&s.name).cloned() {
                if ctx.current_exec.is_some() {
                    let inner = backing.inner_type.clone();
                    let value_port = lower_expr(ctx, value_expr);
                    // After the value: its reads (an `Exec_Var_Get`) advance
                    // the chain, and the set must run after them.
                    let Some(exec) = ctx.current_exec else { return };
                    let set_node = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::VAR_SET,
                        source_range: SourceRange::default(),
                        note: Some("out_set"),
                        ports: GateIO::var_write(inner.clone()),
                        ..Default::default()
                    });
                    ctx.connect(exec, set_node.port(WirePort::Exec));
                    ctx.connect(
                        backing.node_id.port(WirePort::VarRef),
                        set_node.port(WirePort::VarRef),
                    );
                    ctx.connect(value_port, set_node.port(WirePort::Value));
                    ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
                }
                return;
            }
            let value_port = lower_expr(ctx, value_expr);
            ctx.connect(value_port, out.node_id.port(WirePort::RerInput));
            // A value output's RerInput carries the value, and the value update is
            // itself the signal -- do NOT also queue the exec onto that same
            // RerInput (double-driving it is a load-time wire fan-in). Lowering the
            // value above already advanced current_exec, so following statements
            // (e.g. a subsequent `emit`) are unaffected.
            return;
        } else if let Some(ref key) = sig_key
            && ctx.current_exec.is_some()
        {
            // Local exec signal: the value is a ferried payload. Write it into
            // the signal's hidden store(s) on the emit chain — sequenced before
            // any buffer, so the value is stored this tick and read on the
            // resumed chain after the exec crosses the barrier.
            write_signal_payload(ctx, key, value_expr);
        } else if sig_key.is_some() {
            report_signal_without_chain(ctx, &s.name, &s.range);
            return;
        }
        if let Some(current_exec) = ctx.current_exec {
            let src_exec = match &s.buffer {
                Some(spec) => buffered_exec(ctx, spec, current_exec),
                None => current_exec,
            };
            let chain = ctx.builder.current_chain_id;
            ctx.signal_emits += u32::from(sig_key.is_some());
            queue_emit(ctx, pending_key, src_exec, chain, &s.range);
        }
    } else {
        // A record or enum output crosses the boundary as value pins with no
        // exec of its own, so a bare `emit` of one has nothing to fire, and
        // neither has any other value output: an inlined mod's caller reads
        // only its value, and the pin of a scalar `out` carries only its value.
        // An `any` output takes whatever is wired to it, an exec included.
        let mod_value = ctx.mod_output_types.get(&s.name).is_some_and(|t| !matches!(t, Type::Exec));
        let scalar_value = ctx
            .lookup_output(&s.name)
            .is_some_and(|o| !matches!(unwrap_ref(&o.ty), Type::Exec | Type::Any | Type::Opaque));
        if is_output && (ctx.lookup_output(&s.name).is_none() || mod_value || scalar_value) {
            let kind = if mod_value {
                "a value output of this mod"
            } else if scalar_value {
                "a value output"
            } else {
                "a record or enum output"
            };
            ctx.error(
                "WS057",
                format!(
                    "`emit {0}` has nothing to fire: `{0}` is {kind}, which carries only its \
                     value. Write `emit {0} = value`",
                    s.name
                ),
                &s.range,
            );
            return;
        }
        let Some(current_exec) = ctx.current_exec else {
            if !is_output && sig_key.is_some() {
                report_signal_without_chain(ctx, &s.name, &s.range);
            }
            return;
        };
        let src_exec = match &s.buffer {
            Some(spec) => buffered_exec(ctx, spec, current_exec),
            None => current_exec,
        };
        let chain = ctx.builder.current_chain_id;
        ctx.signal_emits += u32::from(sig_key.is_some());
        queue_emit(ctx, pending_key, src_exec, chain, &s.range);
    }
}

/// `emit name` of an exec signal where no exec chain runs: nothing would ever
/// fire it.
fn report_signal_without_chain(ctx: &mut LowerCtx, name: &str, range: &SourceRange) {
    ctx.error(
        "WS007",
        format!(
            "`emit {name}` fires an exec signal, and there is no exec chain here to fire it \
             from. Emit it inside an `on` handler"
        ),
        range,
    );
}

/// Queue an emit of `key` from `src_exec`, logged in `emit_log` so a splice
/// can tell the emits queued since a point from those queued before it.
fn queue_emit(ctx: &mut LowerCtx, key: String, src_exec: PortRef, chain: Option<u32>, site: &SourceRange) {
    ctx.emit_sites.entry(key.clone()).or_insert_with(|| site.clone());
    let queued = ctx.pending_emits.entry(key.clone()).or_default();
    ctx.emit_log.push((key, queued.len()));
    queued.push((src_exec, chain));
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
    let mut inputs = vec![PortSpec::exec(*sym::INPUT)];
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
                inputs.push(PortSpec::new(delay_sym, unit_ty.clone()));
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
                inputs.push(PortSpec::new(hold_sym, unit_ty.clone()));
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
            outputs: vec![PortSpec::exec(*sym::OUTPUT)],
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
        gate_class: gc::VAR,
        source_range: SourceRange::default(),
        ports: GateIO::var_storage(ty.clone()),
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
        ports: GateIO::var_write(ty),
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
        ports: GateIO::var_read(ty),
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
            let ty = unwrap_ref(&ctx.one_value_type_at(fexpr.range()).unwrap_or(Type::Any));
            let value_port = lower_expr(ctx, &fexpr);
            let store = payload_store(ctx, sig, &name, ty.clone());
            chain_var_set(ctx, store, value_port, ty);
        }
        return;
    }
    // A gate read ferries its first output.
    let ty = unwrap_ref(&ctx.one_value_type_at(value_expr.range()).unwrap_or(Type::Any));
    let value_port = lower_expr(ctx, value_expr);
    let store = payload_store(ctx, sig, "", ty.clone());
    chain_var_set(ctx, store, value_port, ty);
}

/// True when `node` is a CustomEvent/GlobalCustomEvent receiver gate: the ones
/// that expose `DataOut1..8` for an `await CustomEvent(...)` data capture.
fn is_event_data_gate(ctx: &LowerCtx, node: NodeId) -> bool {
    ctx.builder.module.nodes.get(&node).is_some_and(|n| {
        n.gate_class == gc::CUSTOM_EVENT || n.gate_class == gc::CUSTOM_EVENT_GLOBAL
    })
}

/// Set the declared type of `node`'s output `port` (used to type an awaited
/// event's captured `DataOut` port from the `let x: T` annotation, so the game's
/// wire variant matches instead of defaulting to float).
fn retype_output_port(ctx: &mut LowerCtx, node: NodeId, port: WirePort, ty: Type) {
    if let Some(n) = ctx.builder.module.nodes.get_mut(&node) {
        let io = std::sync::Arc::make_mut(&mut n.ports);
        let name = port.sym();
        for p in io.outputs.iter_mut() {
            if p.name == name {
                p.ty = ty;
                return;
            }
        }
    }
}

pub(super) fn lower_await(ctx: &mut LowerCtx, a: &AwaitStmt) {
    // Backstop for typecheck's loop-depth check: a mod inlines per call
    // site, so an `await` with no `for` of its own can still land inside a
    // loop plane through the caller. One written in the body typecheck
    // walked was already reported there, so this speaks only for the rest.
    if ctx.in_loop_plane() {
        if !ctx.inside_written_loop_body(&a.range) {
            ctx.error(
                "WS079",
                "a loop body runs synchronously within one tick, so it cannot \
                 suspend. Move the `await` outside the loop, or use a back-edge \
                 loop (`emit` + `await`) if you need work spread across ticks.",
                &a.range,
            );
        }
        return;
    }

    // 1. Create a static bool var for the armed flag (initially false)
    let armed_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::VAR,
        source_range: a.range.clone(),
        ports: GateIO::var_storage(Type::Bool),
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
            ports: GateIO::source(Type::Bool),
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
            ports: GateIO::var_write(Type::Bool),
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
        ports: GateIO::var_read(Type::Bool),
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
        ports: GateIO::exec_branch(),
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
        ports: GateIO::source(Type::Bool),
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
        ports: GateIO::var_write(Type::Bool),
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
    // A var can change while the exec is suspended, so the resumed chain must
    // re-read from a fresh Var_Get rather than reuse the pre-await gate — the
    // same invalidation lower_if does at its branch boundaries.
    reset_var_get_caches(ctx);

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
        } else if is_event_data_gate(ctx, exec_port.node_id) {
            // `let foo[: T] = await CustomEvent("c")`: capture the event's first
            // data output (`DataOut1`). Retype that port to the annotation so the
            // game delivers the right variant (an unset data port wires as float).
            if let Some(te) = &a.binding_type {
                let declared = ctx.resolve_local_type(te);
                retype_output_port(ctx, exec_port.node_id, WirePort::DataOut1, declared);
            }
            exec_port.node_id.port(WirePort::DataOut1)
        } else {
            // `let v = await sig` on a signal with no payload: there is nothing
            // to capture. Wiring `exec_port` (the signal's exec source) into a
            // value port produces a garbage value, so report it and bind a
            // literal default instead of emitting an exec-into-value wire.
            ctx.error(
                "WS056",
                format!(
                    "`await {binding_name}` binds no value: the signal carries no payload. \
                     Emit a value (`emit <sig> = ...`), capture with `await <expr> on <sig>`, \
                     or drop the `let`"
                ),
                &a.range,
            );
            let zero = Expr::IntLit {
                value: 0,
                text: "0".to_string(),
                range: a.range.clone(),
            };
            lower_expr(ctx, &zero)
        };
        ctx.scope.insert(
            &binding_name,
            Binding::Local(LocalRecord { port: val_port }),
        );
    }

    // 9a. `let (p, t) = await CustomEvent("c")`: capture the event's data outputs
    // POSITIONALLY (p = DataOut1, t = DataOut2). Untyped (typecheck warns WS055).
    if let Some(ref names) = a.tuple_destructure {
        if is_event_data_gate(ctx, exec_port.node_id) {
            const DATA: [WirePort; 8] = [
                WirePort::DataOut1, WirePort::DataOut2, WirePort::DataOut3, WirePort::DataOut4,
                WirePort::DataOut5, WirePort::DataOut6, WirePort::DataOut7, WirePort::DataOut8,
            ];
            for (i, local) in names.iter().enumerate() {
                if let Some(&port) = DATA.get(i) {
                    ctx.scope.insert(
                        local,
                        Binding::Local(LocalRecord { port: exec_port.node_id.port(port) }),
                    );
                }
            }
        }
        return;
    }

    // 9b. `let { a, b } = await sig`: read each destructured payload store on
    // the resumed chain and bind the locals (a local signal's ferried payload).
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
                        "awaited signal has no ferried payload field `{field}`; \
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
            ports: GateIO::exec_join(),
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
            ports: GateIO::exec_join(),
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
    ctx.emit_log.clear();
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
        } else if ctx.scope.get(&crate::lower::context::output_scope_key(&name)).is_some() {
            // An output with no single pin (a record or enum) has nothing an
            // exec can drive; `lower_emit` routes every write to one elsewhere.
            let range = ctx.emit_sites.get(&name).cloned().unwrap_or_default();
            report_internal_drop(ctx, &format!("an `emit {name}`"), &range);
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
        ports: GateIO::source(Type::Bool),
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
        ports: GateIO::var_write(Type::Bool),
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
///
/// Spans the WHOLE chip tree, not just the module the hub lives in: a signal
/// declared at top level can be emitted or consumed from inside a `chip`
/// (`chip C() { Timer(1, restart = sig) }`), which lowers to a sub-module wire
/// naming the hub as an external capture. Counting only the hub's own module
/// would misjudge a pass-through, and redirecting only that module left the
/// sub-module wire pointing at the hub node this then deletes — a dangling
/// endpoint that emit rejects as `EmitError::DroppedWire`. `boundary_pins`
/// routes the redirected source through pins afterwards, exactly as it does
/// for any other captured node.
fn splice_single_input_union(ctx: &mut LowerCtx, hub: NodeId) {
    let mut sources = Vec::new();
    collect_hub_sources(&ctx.builder.module, hub, &mut sources);
    if sources.len() != 1 {
        return;
    }
    let src = sources.remove(0);
    redirect_hub(&mut ctx.builder.module, hub, &src);
}

/// The source of every wire feeding `hub`, anywhere in the chip tree.
fn collect_hub_sources(m: &Module, hub: NodeId, acc: &mut Vec<PortRef>) {
    for w in &m.wires {
        if w.target.node_id == hub {
            acc.push(w.source.clone());
        }
    }
    for child in m.chips.values() {
        collect_hub_sources(child, hub, acc);
    }
}

/// Drop `hub` and every wire into it, and repoint its readers at `src` —
/// through the whole chip tree, since node ids are unique across it.
fn redirect_hub(m: &mut Module, hub: NodeId, src: &PortRef) {
    m.wires.retain(|w| w.target.node_id != hub);
    for w in m.wires.iter_mut() {
        if w.source.node_id == hub {
            w.source = src.clone();
        }
    }
    m.nodes.remove(&hub);
    for child in m.chips.values_mut() {
        redirect_hub(child, hub, src);
    }
}

/// A continuous `out name = expr` beside another write of the same output in
/// a handler or branch: both would drive it. Only a constant default can seed
/// the output's backing var.
fn report_default_beside_emit(ctx: &mut LowerCtx, name: &str, value: &Expr, range: &SourceRange) {
    // With no other write, the output is stored only because this one comes
    // after an early exit (a `return` or `break`) that has no exec chain to
    // run on, which typecheck already rejects.
    if !ctx.emitted_outputs.contains(name) && ctx.output_write_sites.get(name).is_some_and(|n| *n < 2) {
        ctx.error(
            crate::diagnostic::INTERNAL_ERROR,
            format!("`out {name} = ...` follows an early exit with no exec chain to store it on"),
            range,
        );
        return;
    }
    let other = if ctx.emitted_outputs.contains(name) {
        format!("an `emit {name} = ...`")
    } else {
        format!("another `out {name} = ...` in a handler or branch")
    };
    let value_ty = unwrap_ref(&ctx.type_of(value));
    let default = match &value_ty {
        Type::Enum { name: en, .. } => match ctx.enum_defs.of(&value_ty).and_then(|d| d.variants.first()) {
            Some(v) => Some(format!("`out {name} = {en}.{}`", v.name)),
            None => Some(format!("`out {name} = {en}.<variant>`")),
        },
        Type::Record(_) | Type::Tuple(_) => None,
        _ if ctx.output_tag(name).is_some() => Some("one of its variants".to_string()),
        _ => Some(format!("`out {name} = 0`")),
    };
    let fix = match default {
        Some(d) => format!("Give this one a constant default ({d}) and let the other write change it, or remove it"),
        None => format!("A record output takes no default beside another write: set it with `emit {name} = ...` on a handler's chain, or remove this one"),
    };
    ctx.error(
        "WS013",
        format!(
            "this `out {name} = ...` drives `{name}` continuously, and {other} also writes \
             it, so the two would fight over the output. {fix}"
        ),
        range,
    );
}

/// Drive the pin of the payload-free enum output `name` with `value`'s tag,
/// the way a scalar output takes its value: directly, or, when the output has
/// a backing var, by storing the tag on the chain (a constant default seeds
/// the var instead).
fn drive_enum_output(ctx: &mut LowerCtx, name: &str, value: &Expr, range: &SourceRange) {
    let Some(out) = ctx.output_tag(name).cloned() else {
        return;
    };
    let what = format!("the enum value driving `{name}`");
    let Some(fields) = written_record_fields(ctx, &what, value) else {
        return;
    };
    let fields = one_enum_value(ctx, value, fields);
    let Some(tag) = enum_disc(&fields).cloned().and_then(|b| binding_to_port(ctx, &b, value.range())) else {
        report_lowered_to_nothing(ctx, &what, value);
        return;
    };
    let Some(backing) = ctx.output_backing_vars.get(name).cloned() else {
        ctx.connect(tag, out.node_id.port(WirePort::RerInput));
        return;
    };
    if let Some(exec) = ctx.current_exec {
        let set_node = ctx.add_gate(AddNodeOpts {
            gate_class: gc::VAR_SET,
            source_range: range.clone(),
            note: Some("out_set"),
            ports: GateIO::var_write(backing.inner_type.clone()),
            ..Default::default()
        });
        ctx.connect(exec, set_node.port(WirePort::Exec));
        ctx.connect(backing.node_id.port(WirePort::VarRef), set_node.port(WirePort::VarRef));
        ctx.connect(tag, set_node.port(WirePort::Value));
        ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
        return;
    }
    let constant = ctx
        .builder
        .module
        .nodes
        .get(&tag.node_id)
        .filter(|n| n.gate_class == gc::LITERAL)
        .and_then(|n| n.properties.get(&*sym::VALUE).cloned());
    let Some(lit) = constant else {
        report_default_beside_emit(ctx, name, value, range);
        return;
    };
    if let Some(node) = ctx.builder.module.nodes.get_mut(&backing.node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(*sym::INITIAL_VALUE, lit);
    }
}

pub(super) fn lower_out_binding(
    ctx: &mut LowerCtx,
    name: &str,
    value: Option<&Expr>,
    range: &SourceRange,
) {
    let Some(value) = value else { return };
    // `out y: *int = &x` drives the port with x's ref (`lower_ref_of`, through
    // `lower_expr`). A record has no single ref: `*T` on one distributes over
    // its fields, so `&rec` wires field by field like the bare record does.
    let value = match value {
        Expr::RefOf { operand, .. }
            if matches!(resolve_field_chain(ctx, operand), Some(Binding::Record(_))) =>
        {
            operand.as_ref()
        }
        other => other,
    };
    // An output of the inlined mod that is written in more than one place goes
    // through its return storage (see `store_mod_output`).
    if ctx.mod_emit_storage.contains(name) {
        if ctx.current_exec.is_some() {
            store_mod_output(ctx, name, value, range);
        } else if !ctx.mod_unsequenced.contains(name) {
            report_unsequenced_output_write(ctx, name, range);
        }
        return;
    }
    // An ENUM value driving an output port has no materialization yet: the
    // out-port isn't decomposed into `__disc` + slot pins (unlike a record,
    // which `pre_declare_output` DOES explode), so both the construction path
    // and a record-returning enum call below would stash the value into
    // `pending_out_records` and, for a top-level (non-inlined) out, silently
    // drop it - a dead, unwired output with no feedback. Make that LOUD (the
    // same WSP001 not-yet-materialized idiom the earlier `VariantCtor` stub
    // used) and stop, rather than emit dead gates. Scoped strictly to
    // `Type::Enum` values - a record out-port (which IS wired, field-wise,
    // below) is untouched, and a missing/`Any` type falls through unchanged.
    // An inline mod's output is the exception: the enum reaches its caller as
    // its fields, as a record output does.
    if ctx.mod_output_names.iter().any(|n| n == name)
        && (matches!(unwrap_ref(&ctx.type_of(value)), Type::Enum { .. })
            || matches!(ctx.mod_output_types.get(name), Some(Type::Enum { .. })))
    {
        stash_mod_output_record(ctx, name, &format!("the value of `out {name}`"), value);
        return;
    }
    if matches!(ctx.type_of(value), Type::Enum { .. })
        && !matches!(
            ctx.scope.get(&crate::lower::context::output_scope_key(name)),
            Some(Binding::Record(_))
        )
    {
        ctx.warn(
            format!(
                "an enum value can't drive the output `{name}` yet - enum output-port \
                 materialization is not implemented, so this output is left unwired and \
                 the enum's tag and payload are dropped"
            ),
            value.range(),
        );
        return;
    }
    if ctx.output_tag(name).is_some() {
        drive_enum_output(ctx, name, value, range);
        return;
    }
    // A record-typed boundary output was exploded into per-field pins
    // (`pre_declare_output`); wire each field's source into its own pin instead
    // of falling through to the single-wire path below.
    if let Some(Binding::Record(out_fields)) = ctx
        .scope
        .get(&crate::lower::context::output_scope_key(name))
        .cloned()
    {
        // A record output written from several sites stores into its backing,
        // which drives the pins once (see `output_record_backing`).
        if let Some(backing) = ctx.output_record_backing.get(name).cloned() {
            if ctx.current_exec.is_none() {
                report_default_beside_emit(ctx, name, value, range);
            } else {
                let what = "the record value driving this output";
                if let Some(src) = written_record_fields(ctx, what, value)
                    && let Some(src) = fit_record_value(ctx, what, value, &backing, src)
                {
                    assign_record_fields(ctx, &backing, &src, range);
                }
            }
            return;
        }
        wire_record_output(ctx, &out_fields, value, range);
        return;
    }
    let out = match ctx.lookup_output(name).cloned() {
        Some(o) => o,
        None => {
            // Nothing pre-declared a port for this binding, so the value has
            // nowhere to go.
            ctx.error(
                "WS073",
                format!(
                    "`out {name}` has no port to bind to, so its value is dropped. \
                     Declare the port at the top level of the file, in a chip body, \
                     or in the chip's signature."
                ),
                range,
            );
            return;
        }
    };
    // A RECORD assigned to an output (`out card = someRecord`). A record has no
    // single value port, so lowering it as an expression yields an
    // `_Unsupported` placeholder and the output silently carries a default.
    // Stash the field map under this output's name instead; the inline-call
    // machinery hands it to the caller as a real record (see
    // `pending_out_records`). Mirrors what `Stmt::Return` already does through
    // `pending_return_record`.
    // An output declared with a single-value type takes a record of named
    // gate outputs as its first output, as `x = e` does (see below); any other
    // record there is the type check's error.
    let one_value = holds_one_value(&out.ty);
    if let Some(Binding::Record(fields)) = resolve_field_chain(ctx, value).cloned()
        && !one_value
    {
        stash_out_record(ctx, name, fields, value);
        return;
    }
    if let Expr::RecordLit { fields, .. } = value {
        let lit = ctx.type_of(value);
        let record = lower_record_lit(ctx, fields, Slots::Of(&lit));
        stash_out_record(ctx, name, record, value);
        return;
    }
    // A backed output written from an exec chain does the same `Var_Set` an
    // `emit name = value` does, regardless of whether the value happens to be a
    // literal: this must run before the constant-seed branch below, or a
    // literal-valued handler site (e.g. `out n: int = 1`) would instead rebake
    // the var's `InitialValue` on every call, producing zero `Var_Set` gates
    // for what pass 1 already counted as a value-driving site. Direct-wiring
    // here instead would fan in with the var-to-rerouter wire `lower` adds
    // once at the end.
    if let Some(backing) = ctx.output_backing_vars.get(name).cloned()
        && ctx.current_exec.is_some()
    {
        let inner = backing.inner_type.clone();
        let value_port = lower_expr(ctx, value);
        // After the value: its reads advance the chain.
        let Some(exec) = ctx.current_exec else { return };
        let set_node = ctx.add_gate(AddNodeOpts {
            gate_class: gc::VAR_SET,
            source_range: range.clone(),
            note: Some("out_set"),
            ports: GateIO::var_write(inner.clone()),
            ..Default::default()
        });
        ctx.connect(exec, set_node.port(WirePort::Exec));
        ctx.connect(
            backing.node_id.port(WirePort::VarRef),
            set_node.port(WirePort::VarRef),
        );
        ctx.connect(value_port, set_node.port(WirePort::Value));
        ctx.current_exec = Some(set_node.port(WirePort::ExecOut));
        return;
    }
    // A var-backed output (it is also emitted to) must NOT take a direct driver
    // from its initializer, since that would fan in with the backing var's own
    // feed. A constant default seeds the backing var's `InitialValue`; the var
    // then drives the output once. A declaration's value is always lowered
    // with no current exec chain (`mod.rs`'s pass-2 loop and the
    // namespace-member loop above both clear it before lowering one), so this
    // branch only ever sees a genuine default, never a handler-body write. A
    // non-constant default is left to the direct wire below (rare, and it is
    // the sole driver only when there is no emit, which would not have
    // created a backing var).
    if let Some(backing) = ctx.output_backing_vars.get(name).cloned()
        && let Some(lit) = expr_to_literal_in(value, &ctx.const_lookup())
    {
        // Bake in the output's DECLARED type, exactly as a `var`'s
        // initializer does: the backing gate's wire variant is picked from
        // this literal's kind, so `out y: float = 0` would otherwise ship an
        // integer variable (see `bake_literal_for_type`).
        let lit = bake_literal_for_type(lit, &backing.inner_type);
        if let Some(node) = ctx.builder.module.nodes.get_mut(&backing.node_id) {
            let props = std::sync::Arc::make_mut(&mut node.properties);
            props.insert(*sym::INITIAL_VALUE, lit);
        }
        return;
    }
    // Any other default beside an emit would drive the output alongside its
    // backing var.
    if ctx.output_backing_vars.contains_key(name) {
        report_default_beside_emit(ctx, name, value, range);
        return;
    }
    // `out x = makeRecord(...)`: the call stashes its own field map, exactly as
    // `let`/`return` consume it.
    ctx.pending_inline_record = None;
    let port = lower_expr(ctx, value);
    if matches!(value, Expr::Call { .. })
        && let Some(record) = ctx.pending_inline_record.take()
        // A gate read gives an output that is not a record its first output.
        && (ctx.gate_outputs(value.range()).is_none()
            || matches!(unwrap_ref(&out.ty), Type::Record(_) | Type::Tuple(_)))
    {
        stash_out_record(ctx, name, record, value);
        return;
    }
    ctx.connect(port, out.node_id.port(WirePort::RerInput));
}

/// Hand the record written to the output `name` to the inline call that
/// reads it (see `pending_out_records`). Only an output of the mod being
/// inlined is read there; any other output with one pin would be left with
/// nothing driving it, so that write is WS105.
fn stash_out_record(
    ctx: &mut LowerCtx,
    name: &str,
    fields: HashMap<crate::intern::Sym, Binding>,
    value: &Expr,
) {
    if ctx.mod_output_names.iter().any(|n| n == name) || ctx.template_build {
        ctx.pending_out_records.insert(name.to_string(), fields);
        return;
    }
    let ty = unwrap_ref(&ctx.type_of(value));
    let tuple = match &ty {
        Type::Tuple(_) => true,
        Type::Record(fs) => fs.iter().enumerate().all(|(i, (k, _))| *k == i.to_string()),
        _ => false,
    };
    let what = if tuple { "a tuple" } else { "a record" };
    // An alias naming exactly this type is how the program spells it; of
    // several, the one declared first.
    let mut aliases: Vec<&String> = ctx
        .type_aliases
        .iter()
        .filter(|(_, te)| ctx.resolve_local_type(te) == ty)
        .map(|(n, _)| n)
        .collect();
    aliases.sort_by_key(|n| (ctx.consts.types().declared_at(n).unwrap_or(usize::MAX), *n));
    let spelled = match aliases.first() {
        Some(alias) => alias.to_string(),
        None => crate::analysis::types::type_str(&ty),
    };
    // Written apart from its declaration (`out z` then `emit z = ..`): the
    // annotation goes on the declaration.
    let declared = ctx
        .lookup_output(name)
        .and_then(|o| ctx.builder.module.nodes.get(&o.node_id))
        .map(|n| n.source_range.clone())
        .filter(|d| {
            let v = value.range();
            d.file != v.file || v.start.offset < d.start.offset || v.end.offset > d.end.offset
        });
    let (fix, at) = match declared {
        Some(decl) => {
            (format!("Annotate its declaration so each field gets its own pin: `out {name}: {spelled}`"), decl)
        }
        None => (
            format!("Annotate the output so each field gets its own pin: `out {name}: {spelled} = ...`"),
            value.range().clone(),
        ),
    };
    ctx.error(
        "WS105",
        format!(
            "output `{name}` has no type and is written {what}, which a port with no type \
             carries as one pin, so nothing would drive it. {fix}"
        ),
        &at,
    );
}

/// Wire a record value into a record-typed boundary output's per-field pins.
/// The value resolves to a field -> source-binding map (a record literal, a
/// record var/`let`, or a record-returning inline call), and each field's source
/// port drives the matching output pin. Nested-record fields carry no single
/// port and are left for the caller's field-array path, matching the input side.
fn wire_record_output(
    ctx: &mut LowerCtx,
    out_fields: &crate::collections::HashMap<crate::intern::Sym, Binding>,
    value: &Expr,
    range: &SourceRange,
) {
    // The shared resolver, not a private copy of it. Re-implementing
    // record-literal, field-chain and call resolution inline here leaves every
    // other source shape (a record-valued ternary, an enum construction, a
    // record array/map element) producing nothing in the `out` position while
    // working in assignment position.
    let what = "the record value driving this output";
    let Some(src) = written_record_fields(ctx, what, value) else {
        return;
    };
    let Some(src) = fit_record_value(ctx, what, value, out_fields, src) else {
        return;
    };
    wire_record_output_fields(ctx, out_fields, &src, range);
}

/// `value`'s fields for the record or enum write `what`: [`value_record_fields`],
/// reporting a value that lowered to none (see [`report_lowered_to_nothing`])
/// unless lowering it already said why.
fn written_record_fields(
    ctx: &mut LowerCtx,
    what: &str,
    value: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let reported = ctx.diagnostics.len();
    let fields = value_record_fields(ctx, value);
    if fields.is_none() && ctx.diagnostics.len() == reported {
        report_lowered_to_nothing(ctx, what, value);
    }
    fields
}

/// `src`, the fields `value` lowered to, as one enum value. A gate read is
/// its first output, which is how typecheck coerces it (`d = f(c)` on
/// `-> (r: Hue, e: exec)`, `d = k` after `let k = f(c)`).
pub(super) fn one_enum_value(
    ctx: &LowerCtx,
    value: &Expr,
    src: HashMap<crate::intern::Sym, Binding>,
) -> HashMap<crate::intern::Sym, Binding> {
    if enum_disc(&src).is_none()
        && let Some(Binding::Record(first)) = held_output(ctx, &src, value.range(), OutputPick::First)
        && enum_disc(&first).is_some()
    {
        return first;
    }
    src
}

/// `src`, the fields `value` lowered to, fitted to `target`, the storage of a
/// record or enum: as one enum value when `target` is an enum's (see
/// [`one_enum_value`]). `None`, reported, when none of them reach `target`.
pub(super) fn fit_record_value(
    ctx: &mut LowerCtx,
    what: &str,
    value: &Expr,
    target: &HashMap<crate::intern::Sym, Binding>,
    src: HashMap<crate::intern::Sym, Binding>,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let src = if enum_disc(target).is_some() { one_enum_value(ctx, value, src) } else { src };
    let src = match ctx.checked_type_at(value.range()) {
        Some(checked) => held_reads_where_one_value(checked, src, Slot::Storage(target)),
        None => src,
    };
    if shares_a_field(target, &src) {
        return Some(src);
    }
    report_lowered_to_nothing(ctx, what, value);
    None
}

/// What a record value is written into, field by field.
pub(super) enum Slot<'a> {
    /// Storage (or pins): each field's binding.
    Storage(&'a HashMap<crate::intern::Sym, Binding>),
    /// A place of this type.
    Typed(Type),
}

impl Slot<'_> {
    /// The field `name` of this slot: `Some(None)` where it takes one value,
    /// `Some(Some(_))` where it takes a record, `None` where it has none.
    fn field(&self, name: &str) -> Option<Option<Slot<'_>>> {
        match self {
            Slot::Storage(fields) => match fields.get(&crate::intern::intern(name))? {
                Binding::Record(inner) => Some(Some(Slot::Storage(inner))),
                _ => Some(None),
            },
            Slot::Typed(t) => {
                let ft = match unwrap_ref(t) {
                    Type::Record(fs) => fs.into_iter().find(|(n, _)| n == name).map(|(_, t)| t),
                    Type::Tuple(ts) => ts.into_iter().nth(name.parse().ok()?),
                    _ => None,
                }?;
                Some(match unwrap_ref(&ft) {
                    Type::Record(_) | Type::Tuple(_) => Some(Slot::Typed(ft)),
                    t if holds_one_value(&t) => None,
                    _ => return None,
                })
            }
        }
    }
}

/// `src`, the fields of a value typecheck typed `checked`, as `slot` takes
/// them: a gate read held whole in a field (a `let` holding `{ m: fs.max() }`)
/// gives a field taking one value its first output, as in every other place
/// for one value; a field taking a record keeps it.
pub(super) fn held_reads_where_one_value(
    checked: &Type,
    mut src: HashMap<crate::intern::Sym, Binding>,
    slot: Slot,
) -> HashMap<crate::intern::Sym, Binding> {
    let fields: Vec<(String, Type)> = match unwrap_ref(checked) {
        Type::Record(fs) => fs,
        Type::Tuple(ts) => ts.into_iter().enumerate().map(|(i, t)| (i.to_string(), t)).collect(),
        _ => return src,
    };
    for (name, ty) in fields {
        let key = crate::intern::intern(&name);
        let Some(Binding::Record(inner)) = src.get(&key).cloned() else { continue };
        match (unwrap_ref(&ty), slot.field(&name)) {
            (Type::Outputs(outputs), Some(None)) => {
                if let Some(first) = inner.get(&crate::intern::intern(&outputs[0].0)).cloned() {
                    src.insert(key, first);
                }
            }
            (inner_ty @ (Type::Record(_) | Type::Tuple(_)), Some(Some(nested))) => {
                src.insert(key, Binding::Record(held_reads_where_one_value(&inner_ty, inner, nested)));
            }
            _ => {}
        }
    }
    src
}

/// Hand `value` to the caller as `name`, a record or enum output of the mod
/// being inlined.
fn stash_mod_output_record(ctx: &mut LowerCtx, name: &str, what: &str, value: &Expr) {
    let Some(src) = written_record_fields(ctx, what, value) else {
        return;
    };
    let src = match ctx.mod_output_types.get(name) {
        Some(Type::Enum { .. }) => one_enum_value(ctx, value, src),
        _ => src,
    };
    ctx.pending_out_records.insert(name.to_string(), src);
}

/// Whether writing `src` into `target` reaches any of `target`'s leaves.
fn shares_a_field(
    target: &HashMap<crate::intern::Sym, Binding>,
    src: &HashMap<crate::intern::Sym, Binding>,
) -> bool {
    target.is_empty()
        || target.iter().any(|(k, t)| match (t, src.get(k)) {
            (Binding::Record(tf), Some(Binding::Record(sf))) => shares_a_field(tf, sf),
            (Binding::Record(_), _) | (_, None) => false,
            _ => true,
        })
}

/// A value the type checker accepted that lowered to nothing its target can
/// take, so the write it makes would silently vanish. An `any` value is one
/// wire with no fields to split, which the program can fix (WS003); anything
/// else is a compiler bug.
fn report_lowered_to_nothing(ctx: &mut LowerCtx, what: &str, value: &Expr) {
    if matches!(unwrap_ref(&crate::lower::call::lowered_arg_type(ctx, value)), Type::Any | Type::Opaque) {
        ctx.error(
            "WS003",
            format!(
                "{what} {}, so it can't be a record or enum value. Give it a concrete type",
                crate::diagnostic::ANY_HAS_NO_FIELDS
            ),
            value.range(),
        );
        return;
    }
    report_internal_drop(ctx, what, value.range());
}

/// A write the type checker accepted that lowered to nothing: a compiler bug,
/// reported as WS094. A compile that reports another error drops it (see
/// `diagnostic::drop_internal_errors_beside_errors`): that error rejects the
/// program and may be why nothing was lowered.
fn report_internal_drop(ctx: &mut LowerCtx, what: &str, range: &SourceRange) {
    ctx.error(
        "WS094",
        format!("internal compiler error: {what} lowered to nothing. This is a compiler bug; please report it."),
        range,
    );
}

/// Wire one record source's fields onto the matching output pins, recursing
/// into nested record fields. The flat loop skipped a `Binding::Record` output
/// (a nested record field, which is a pin per leaf), leaving those pins
/// dangling.
fn wire_record_output_fields(
    ctx: &mut LowerCtx,
    out_fields: &crate::collections::HashMap<crate::intern::Sym, Binding>,
    src: &crate::collections::HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) {
    for (fname, out_binding) in out_fields {
        let Some(src_binding) = src.get(fname) else {
            continue;
        };
        match out_binding {
            Binding::Record(inner_out) => {
                if let Binding::Record(inner_src) = src_binding {
                    wire_record_output_fields(ctx, inner_out, inner_src, range);
                }
            }
            Binding::Output(out_rec) => {
                // A `*T` pin carries a ref: a place's own, or the one a value holds.
                let port = if out_rec.ty.ref_kind() == Some(crate::ir::RefKind::Var) {
                    crate::lower::ref_fields::ref_port_of(ctx, src_binding, range)
                } else {
                    crate::lower::access::binding_to_port(ctx, src_binding, range)
                };
                if let Some(port) = port {
                    ctx.connect(port, out_rec.node_id.port(WirePort::RerInput));
                }
            }
            _ => {}
        }
    }
}
