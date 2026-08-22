use super::*;

pub(super) fn lower_stmt(ctx: &mut LowerCtx, s: &Stmt) {
    match s {
        Stmt::Assign(a) => lower_assign(ctx, a),
        Stmt::If(i) => lower_if(ctx, i),
        Stmt::Emit(e) => lower_emit(ctx, e),
        Stmt::Await(a) => ctx.with_nofold(a.no_fold, |ctx| lower_await(ctx, a)),
        Stmt::OutBinding(b) => ctx.with_nofold(b.no_fold, |ctx| {
            lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range)
        }),
        Stmt::ExprStmt(es) => {
            lower_expr(ctx, &es.expr);
        }
        Stmt::Handler(h) => ctx.with_nofold(h.no_fold, |ctx| lower_handler(ctx, h)),
        Stmt::Let(l) => ctx.with_nofold(l.no_fold, |ctx| lower_let_decl(ctx, l)),
        Stmt::AnonChip(ac) => lower_anon_chip(ctx, ac),
        Stmt::ChipDecl(c) => lower_chip_decl(ctx, c),
        Stmt::In(_) => {}
        Stmt::Var(v) => ctx.with_nofold(v.no_fold, |ctx| {
            if needs_declaration(ctx, &v.name) {
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
            // Map-typed vars have no `VarRef` port either (only `MapVarRef`),
            // same hazard as arrays above; rebuild via the map-literal assign
            // (clear + set per entry) each time this scope is entered.
            if !v.is_static
                && ctx.current_exec.is_some()
                && let Some(var_rec) = ctx.lookup_var(&v.name).cloned()
                && var_rec.storage == VarStorage::Map
            {
                if let Some(Expr::MapLit { entries, .. }) = &v.init {
                    let map_ref = var_rec.node_id.port(WirePort::MapVarRef);
                    lower_map_literal_assign(ctx, map_ref, entries, &v.range);
                } else if let Some(init) = &v.init {
                    ctx.warn(
                        format!(
                            "'var {}' map initializer must be a map literal — this value is dropped; build the map with methods like set/copyFrom instead",
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
        }),
        Stmt::Array(a) => {
            if needs_declaration(ctx, &a.name) {
                pre_declare_array(ctx, a);
            }
        }
        Stmt::Map(m) => {
            if needs_declaration(ctx, &m.name) {
                pre_declare_map(ctx, m);
            }
        }
        Stmt::Buffer(b) => {
            // Current-frame check, mirroring `needs_declaration`: an ancestor
            // buffer of the same name is a distinct binding being shadowed, so
            // this statement must get its own gate rather than re-driving the
            // outer buffer's `Input` (a fan-in that fails to load).
            if ctx.lookup_buffer_current_frame(&b.name).is_none() {
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
            } else if let Some(expr) = value
                && let Some(Binding::Record(fields)) = resolve_field_chain(ctx, expr).cloned()
            {
                // `return r` / `return r.sub` where the value is a record
                // binding: forward its field map — a record has no single
                // value port to wire to the output node.
                ctx.pending_return_record = Some(fields);
            } else if let Some(expr) = value {
                // Clear any leftover inline-mod record so only THIS
                // expression's call can set it.
                ctx.pending_inline_record = None;
                let val_port = lower_expr(ctx, expr);
                if matches!(expr, Expr::Call { .. })
                    && let Some(record) = ctx.pending_inline_record.take()
                {
                    // `return make(x)` where `make` returns a record: the call
                    // stashed a field→source map (its value "port" is a
                    // placeholder, not a real node). Forward the record to the
                    // caller instead of wiring the placeholder.
                    ctx.pending_return_record = Some(record);
                } else if let Some(ref var_rec) = ctx.mod_return_var.clone() {
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
fn needs_declaration(ctx: &LowerCtx, name: &str) -> bool {
    match ctx.lookup_var_current_frame(name) {
        Some(rec) => ctx.immutable_containers.contains(&rec.node_id),
        None => true,
    }
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

/// Emit a `Var_Set` (or, for buffer storage, a direct `Input` wire) writing an
/// ALREADY-LOWERED `value_port` into `var_rec` on the current exec chain,
/// advancing `current_exec` and invalidating the read cache. Extracted from the
/// field-assignment path so the whole-record decomposition writes each leaf
/// field identically. A non-buffer write in pure position (no `current_exec`)
/// is a no-op, matching the caller's own guard.
fn set_scalar_var(ctx: &mut LowerCtx, var_rec: &VarRecord, value_port: PortRef, range: &SourceRange) {
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

/// Resolve a record-typed VALUE expression to its per-field binding map: a
/// record literal (`{ x, y }`) lowers each field expr in place; an identifier or
/// field chain naming a `Binding::Record` (a record var/let/input) forwards its
/// field map. Returns `None` for a source with no field map yet (e.g. a
/// record-returning call — handled by the `pending_*` machinery elsewhere, not
/// here), leaving the assignment to fall through rather than miswire.
pub(super) fn value_record_fields(
    ctx: &mut LowerCtx,
    value: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    if let Expr::RecordLit { fields, .. } = value {
        return Some(lower_record_lit(ctx, fields));
    }
    // `pts[i]` reads a record ARRAY element as a record value (per-field
    // ArrayGet), so it can be assigned/pushed like any other record.
    if let Expr::IndexAccess { obj, index, range } = value
        && let Some(rec) = lower_record_array_index_value(ctx, obj, index, range)
    {
        return Some(rec);
    }
    // `m[k]` / `m.get(k)` reads a record MAP value as a record (per-field MapGet).
    if matches!(value, Expr::IndexAccess { .. } | Expr::Call { .. })
        && let Some(rec) = lower_record_map_key_value(ctx, value, value.range())
    {
        return Some(rec);
    }
    if let Some(Binding::Record(f)) = resolve_field_chain(ctx, value).cloned() {
        return Some(f);
    }
    // A record-returning CALL that isn't one of the shapes above - `pts.pop()`, a
    // record-returning mod/chip - stashes its per-field record in
    // `pending_inline_record` when lowered. Lower it and take that.
    if matches!(value, Expr::Call { .. }) {
        let _ = lower_expr(ctx, value);
        if let Some(rec) = ctx.pending_inline_record.take() {
            return Some(rec);
        }
    }
    None
}

/// Write a record value into a `Binding::Record` target, field by field: each
/// leaf `Var` field is read from the matching source field (`binding_to_port`,
/// which emits a `Var_Get` for a stored source) and written with `set_scalar_var`;
/// nested record fields recurse. Container-typed fields (a record with an
/// array/map field) are left for a follow-up and skipped here.
fn assign_record(
    ctx: &mut LowerCtx,
    target_fields: &HashMap<crate::intern::Sym, Binding>,
    value: &Expr,
    range: &SourceRange,
) {
    let Some(src) = value_record_fields(ctx, value) else {
        return;
    };
    assign_record_fields(ctx, target_fields, &src, range);
}

fn assign_record_fields(
    ctx: &mut LowerCtx,
    target: &HashMap<crate::intern::Sym, Binding>,
    src: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) {
    for (fname, tbind) in target.clone() {
        let Some(sbind) = src.get(&fname).cloned() else {
            continue;
        };
        match tbind {
            Binding::Record(tf) => {
                if let Binding::Record(sf) = sbind {
                    assign_record_fields(ctx, &tf, &sf, range);
                }
            }
            // Only a plain scalar var field is written here; array/map fields
            // (a record holding a container) need their own rebuild and are a
            // follow-up.
            Binding::Var(tvar) if tvar.storage == VarStorage::Var => {
                if let Some(port) = binding_to_port(ctx, &sbind, range) {
                    set_scalar_var(ctx, &tvar, port, range);
                }
            }
            _ => {}
        }
    }
}

pub(super) fn lower_assign(ctx: &mut LowerCtx, s: &Assign) {
    if let Expr::IndexAccess { obj, index, .. } = &s.target {
        // Map subscript `m[k] = v` desugars to a MapVar_Set (the same gate
        // `m.set(k, v)` lowers to) — mirrors the read desugar in
        // `lower_index_access`, via the shared `resolve_map_target` (storage-
        // based dispatch; see its doc comment for why `ctx.type_of(obj)`
        // doesn't work here). Falls through to the array path below for
        // non-map subscript targets.
        if let Some((map_ref, Type::Map(k, v))) = resolve_map_target(ctx, obj) {
            if ctx.current_exec.is_none() {
                return;
            }
            // `m[k] = v` is a mutation — the `const` container rule applies to
            // it exactly as it does to `m.set(k, v)`. Mirrors the array
            // subscript write in `lower_array_set`.
            if reject_const_container_mutation(ctx, map_ref.node_id, true, "an index write", &s.range) {
                return;
            }
            let key = lower_expr(ctx, index);
            let val = lower_expr(ctx, &s.value);
            map_exec_op(
                ctx,
                &s.range,
                map_ref,
                gc::MAP_SET,
                vec![
                    (WirePort::Key, k.as_ref().clone(), key),
                    (WirePort::Value, v.as_ref().clone(), val),
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

    // `pts[i].field = v` — write one field of a record array's element by
    // indexing that field's parallel array. The target is a FieldAccess whose
    // object is an IndexAccess, so neither the IndexAccess branch above nor the
    // record field-chain branch below reaches it.
    if let Expr::FieldAccess { obj, field, .. } = &s.target
        && lower_record_array_field_index_set(ctx, obj, field, &s.value, &s.range)
    {
        return;
    }

    // Handle field-access targets that resolve through records to vars.
    // e.g. `cpu.x = 5` where `cpu` is a record and `cpu.x` is a Var binding.
    if let Expr::FieldAccess { .. } = &s.target
        && let Some(binding) = resolve_field_chain(ctx, &s.target).cloned()
        && let Binding::Var(var_rec) = binding
    {
        if ctx.current_exec.is_none() {
            return;
        }
        let value_port = lower_expr(ctx, &s.value);
        set_scalar_var(ctx, &var_rec, value_port, &s.range);
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
        assign_record(ctx, &target_fields, &s.value, &s.range);
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

    // `foo = [items, ...spreads]` on an var var: rebuild the contents at
    // runtime. There's no single "set array" gate, so clear it then push each
    // item / append each spread in order.
    if var_rec.storage == VarStorage::Array
        && let Expr::Array { elements, .. } = &s.value
    {
        lower_array_literal_assign(ctx, &var_rec, elements, &s.range, &s.value);
        return;
    }

    // `foo = { k => v, ... }` on a map var: rebuild the contents at runtime.
    // There's no single "set map" gate, so clear it then set each entry in
    // order (mirrors the array literal assign above). A non-literal map RHS
    // (`m = m2`) is NOT handled here — see the guard below.
    if var_rec.storage == VarStorage::Map
        && let Expr::MapLit { entries, .. } = &s.value
    {
        let map_ref = var_rec.node_id.port(WirePort::MapVarRef);
        lower_map_literal_assign(ctx, map_ref, entries, &s.range);
        return;
    }

    // Assigning a whole map from a non-literal expression (`m = m2`) is
    // UNCONDITIONALLY unsatisfiable: there is no whole-map-copy-by-assignment
    // gate, and — unlike arrays — a bare map Ident/expr has no valid port to
    // read either (a map var's backing gate only exposes `MapVarRef`, not
    // `VarRef`/`Value`). Falling through to the generic Var_Set path below
    // would wire a nonexistent source port into the gate — a silent no-op
    // that only surfaces as a load failure in-game. Since no fallback can
    // ever satisfy it, this is a hard ERROR (mirrors WS021's "would silently
    // produce a placeholder → error" policy), not a droppable warning.
    // `.copyFrom(src)` is the supported way to copy a whole map.
    if var_rec.storage == VarStorage::Map {
        ctx.diagnostics.push(Diagnostic::error(
            "WS027",
            format!(
                "assigning a whole map from a non-literal is not supported; use {var_name}.copyFrom(src) instead"
            ),
            s.value.range().clone(),
        ));
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

/// Lower `foo = [items, ...spreads]` on an var var: clear it, then push each
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
        let cond_result = crate::const_eval::eval_expr(&s.cond, &cond_cx, &mut budget);
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
            Ok(Literal::Bool(taken)) => {
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
            // An output value-driven from more than one `emit` site is
            // backed by a PseudoVar (allocated by the pre-scan in `lower`). Each
            // site does a Var_Set into it on its own exec chain; the var's value
            // feeds the output once, after all handlers (so no fan-in). Never
            // direct-wire a backed output — that wire would fan in with the
            // var→output wire.
            if let Some(backing) = ctx.output_backing_vars.get(&s.name).cloned() {
                if let Some(exec) = ctx.current_exec {
                    let inner = backing.inner_type.clone();
                    let value_port = lower_expr(ctx, value_expr);
                    let set_node = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::VAR_SET,
                        source_range: SourceRange::default(),
                        note: Some("out_set"),
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
    // A RECORD assigned to an output (`out card = someRecord`). A record has no
    // single value port, so lowering it as an expression yields an
    // `_Unsupported` placeholder and the output silently carries a default.
    // Stash the field map under this output's name instead; the inline-call
    // machinery hands it to the caller as a real record (see
    // `pending_out_records`). Mirrors what `Stmt::Return` already does through
    // `pending_return_record`.
    if let Some(Binding::Record(fields)) = resolve_field_chain(ctx, value).cloned() {
        ctx.pending_out_records.insert(name.to_string(), fields);
        return;
    }
    if let Expr::RecordLit { fields, .. } = value {
        let record = lower_record_lit(ctx, fields);
        ctx.pending_out_records.insert(name.to_string(), record);
        return;
    }
    // A var-backed output (it is also emitted to) must NOT take a direct driver
    // from its initializer — that would fan-in with the backing var's own feed.
    // A constant default seeds the backing var's `InitialValue`; the var then
    // drives the output once. A non-constant default is left to the direct wire
    // below (rare, and it is the sole driver only when there is no emit, which
    // would not have created a backing var).
    if let Some(backing) = ctx.output_backing_vars.get(name).cloned()
        && let Some(lit) = expr_to_literal_in(value, &ctx.const_env)
    {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&backing.node_id) {
            let props = std::sync::Arc::make_mut(&mut node.properties);
            props.insert(*sym::INITIAL_VALUE, lit);
        }
        return;
    }
    // `out x = makeRecord(...)`: the call stashes its own field map, exactly as
    // `let`/`return` consume it.
    ctx.pending_inline_record = None;
    let port = lower_expr(ctx, value);
    if matches!(value, Expr::Call { .. })
        && let Some(record) = ctx.pending_inline_record.take()
    {
        ctx.pending_out_records.insert(name.to_string(), record);
        return;
    }
    ctx.connect(port, out.node_id.port(WirePort::RerInput));
}
