//! Statement checking and assignment-target resolution.

use super::*;
use crate::types::mono::unwrap_ref;

pub(super) fn check_block(
    ctx: &mut TypeCheckCtx,
    block: &Block,
) {
    ctx.push_scope();
    for s in &block.stmts {
        check_stmt(ctx, s);
    }
    ctx.pop_scope();
}

pub(super) fn check_anon_chip_stmts(
    ctx: &mut TypeCheckCtx,
    stmts: &[Stmt],
    pre_registered: bool,
) {
    if !pre_registered {
        for s in stmts {
            match s {
                Stmt::Var(v) => register_decl(ctx, &TopDecl::Var(v.clone())),
                Stmt::Buffer(b) => register_decl(ctx, &TopDecl::Buffer(b.clone())),
                Stmt::Array(a) => register_decl(ctx, &TopDecl::Array(a.clone())),
                Stmt::In(i) => register_decl(ctx, &TopDecl::In(i.clone())),
                _ => {}
            }
        }
    }
    for s in stmts {
        // A chip's PURE statements (`let`/`var`/`out`/`buffer`/declarations) are
        // reactive signal flow — their var reads are continuous, so check them in
        // PURE context so `var_read_contexts` records them as pure (correct hover;
        // mirrors the lowering's per-statement purity in `lower_chip_body`). This
        // holds even when the chip as a whole is exec-wrapped (a top-level chip
        // after a handler), which keeps its EXEC statements (`if`, …) valid.
        if matches!(
            s,
            Stmt::Let(_)
                | Stmt::Var(_)
                | Stmt::Buffer(_)
                | Stmt::OutBinding(_)
                | Stmt::Array(_)
                | Stmt::In(_)
        ) {
            ctx.in_pure(|ctx| check_one_chip_stmt(ctx, s));
        } else {
            check_one_chip_stmt(ctx, s);
        }
    }
}

fn check_one_chip_stmt(ctx: &mut TypeCheckCtx, s: &Stmt) {
    match s {
        Stmt::Var(v) => check_decl(ctx, &TopDecl::Var(v.clone())),
        Stmt::Buffer(b) => check_decl(ctx, &TopDecl::Buffer(b.clone())),
        Stmt::Array(a) => check_decl(ctx, &TopDecl::Array(a.clone())),
        Stmt::In(i) => check_decl(ctx, &TopDecl::In(i.clone())),
        other => check_stmt(ctx, other),
    }
}

/// Whether this statement carries `@nofold`. Mirrors the set of
/// `ctx.with_nofold(<stmt>.no_fold, …)` wraps in `lower::stmt::lower_stmt`
/// (Await, OutBinding, Handler, Let, Var) plus `ChipDecl`, whose body lowering
/// marks via `build_chip_module`'s own `nofold_depth` seed. Wrapping the whole
/// dispatch on this — rather than per arm — is what keeps the two stages from
/// drifting as arms are added: a variant with no `no_fold` field simply
/// reports `false` and behaves exactly as before.
fn stmt_no_fold(s: &Stmt) -> bool {
    match s {
        Stmt::Await(a) => a.no_fold,
        Stmt::OutBinding(b) => b.no_fold,
        Stmt::Handler(h) => h.no_fold,
        Stmt::Let(l) => l.no_fold,
        Stmt::Var(v) => v.no_fold,
        Stmt::ChipDecl(c) => c.no_fold,
        _ => false,
    }
}

pub(super) fn check_stmt(
    ctx: &mut TypeCheckCtx,
    s: &Stmt,
) {
    ctx.with_nofold(stmt_no_fold(s), |ctx| check_stmt_inner(ctx, s));
}

fn check_stmt_inner(
    ctx: &mut TypeCheckCtx,
    s: &Stmt,
) {
    match s {
        Stmt::Var(v) => {
            register_decl(ctx, &TopDecl::Var(v.clone()));
            // Statement-level vars inherit the current exec context (not forced pure)
            // so that `var x: int = arr[i]` works inside handlers.
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                if let Some(entries) = map_init_entries(init)
                    && let Type::Map(k, v_ty) = &inner
                {
                    // A map-valued `var` initializer inside a handler: validate
                    // entries directly (the valid initializer slot) instead of
                    // routing through the generic `infer` position guard.
                    // An empty `{}` is an empty-map init.
                    check_map_literal(ctx, entries, k, v_ty);
                } else {
                    let t = infer::check(ctx, init, &inner);
                    // Reject a void-mutation `Never` init on an unannotated var
                    // (annotated already mismatches via `check`), same as the
                    // top-level decl and `let` paths.
                    if v.typ.is_none() {
                        reject_never_value(ctx, &unwrap_ref(&t), init.range(), &v.name);
                    }
                    // Refine an unannotated var's placeholder `any` from a
                    // non-literal init, same as the top-level decl path.
                    if v.typ.is_none() && matches!(inner, Type::Any) {
                        let u = unwrap_ref(&t);
                        if var_storable(&u) {
                            ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                        }
                    }
                }
            }
        }
        Stmt::Buffer(b) => {
            register_decl(ctx, &TopDecl::Buffer(b.clone()));
            check_decl(ctx, &TopDecl::Buffer(b.clone()));
        }
        Stmt::Array(a) => {
            register_decl(ctx, &TopDecl::Array(a.clone()));
            check_decl(ctx, &TopDecl::Array(a.clone()));
        }
        Stmt::Map(m) => {
            register_decl(ctx, &TopDecl::Map(m.clone()));
            check_decl(ctx, &TopDecl::Map(m.clone()));
        }
        Stmt::Let(l) => {
            let t = infer_let_init(ctx, l);
            if !reject_never_binding(ctx, l, &t) {
                check_let_type_annotation(ctx, l, &t);
            }
            record_single_output_alias(ctx, &l.binding, &l.value);
            bind_let(ctx, &l.binding, &t);
            // A body-local `let name = <constant>` is recorded in the
            // innermost `scoped_consts` frame (mirroring how a top-level
            // `let` lands in `const_env`), so a constant-only config arg
            // elsewhere in this scope (or a nested one) can resolve `name`
            // via `ctx.const_lookup()`. Only the simple `Ident` binding form
            // can name a single constant — a destructured `let` can't.
            //
            // `let` folds OPPORTUNISTICALLY: a failed evaluation just leaves
            // it as a runtime value, same as always. `const` is a GUARANTEE —
            // the same failure is the evaluator's own error (WS046, or
            // WS047/WS048 for a refused/budget-exceeded evaluation).
            if let LetBinding::Ident { name, .. } = &l.binding {
                // Evaluate against the FULL environment, and — for a value
                // that does evaluate — probe again with
                // placeholders removed. Placeholder-ness is TRANSITIVE:
                // `const t = m + 1` inside a body with a `const` param `m`
                // only evaluates because `m` is seeded as a type-shaped zero,
                // so `t`'s value is just as fictional and must not be allowed
                // to decide a branch either. Detecting it by re-evaluation
                // rather than by scanning the expression for names keeps it
                // exact: any read of a placeholder, however deeply nested,
                // makes the clean probe fail. Both evaluations happen up
                // front so their borrows of `ctx` end before the recording
                // below needs `&mut ctx`.
                let (evaluated, derives_from_placeholder) = {
                    let lookup = |n: &str| ctx.resolve_mod(n);
                    let mut budget = crate::const_eval::Budget::default();
                    let evaluated =
                        crate::const_eval::eval_expr(&l.value, &ctx.const_ctx(Some(&lookup)), &mut budget);
                    let derived = evaluated.is_ok() && {
                        let mut probe = crate::const_eval::Budget::default();
                        crate::const_eval::eval_expr(
                            &l.value,
                            &ctx.const_ctx_without_placeholders(Some(&lookup)),
                            &mut probe,
                        )
                        .is_err()
                    };
                    (evaluated, derived)
                };
                match evaluated {
                    Ok(lit) => {
                        // The value itself is still recorded from the full
                        // environment — the presence-only readers this was
                        // built for (e.g. `validate_scalar_config_arg`'s
                        // WS028) must keep seeing `name` as constant.
                        if let Some(frame) = ctx.scoped_consts.last_mut() {
                            frame.insert(name.clone(), lit);
                        }
                        // Record whether THIS binding is spelled `const` —
                        // see `const_declared`'s doc comment. Same
                        // rebind-clears-the-mark discipline as the
                        // placeholder set just below.
                        if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                            if l.is_const {
                                frame.insert(name.clone());
                            } else {
                                frame.remove(name);
                            }
                        }
                        if let Some(frame) = ctx.scoped_const_placeholders.last_mut() {
                            // Re-binding the same name in the same frame must
                            // be able to CLEAR the mark as well as set it.
                            if derives_from_placeholder {
                                frame.insert(name.clone());
                            } else {
                                frame.remove(name);
                            }
                        }
                        check_const_recorded(ctx, l);
                    }
                    Err(err) if l.is_const => ctx.emit(err.code(), err.message(), err.range.clone()),
                    Err(_) => {}
                }
            } else {
                // Same shape as the `Ident` arm above, generalized to every
                // name a destructuring binding introduces: evaluate the WHOLE
                // right-hand side once, split it via `bind_destructured`, and
                // probe again with placeholders removed to detect whether the
                // split-out values are themselves fictional. The same
                // transitivity the `Ident` arm's comment describes applies
                // here unchanged — every name THIS destructure binds comes
                // from the SAME evaluated record, so they are all equally
                // placeholder-derived (or not) together.
                let (evaluated, derives_from_placeholder) = {
                    let lookup = |n: &str| ctx.resolve_mod(n);
                    let mut budget = crate::const_eval::Budget::default();
                    let evaluated =
                        crate::const_eval::eval_expr(&l.value, &ctx.const_ctx(Some(&lookup)), &mut budget)
                            .and_then(|lit| crate::const_eval::bind_destructured(&l.binding, lit));
                    let derived = evaluated.is_ok() && {
                        let mut probe = crate::const_eval::Budget::default();
                        crate::const_eval::eval_expr(
                            &l.value,
                            &ctx.const_ctx_without_placeholders(Some(&lookup)),
                            &mut probe,
                        )
                        .and_then(|lit| crate::const_eval::bind_destructured(&l.binding, lit))
                        .is_err()
                    };
                    (evaluated, derived)
                };
                match evaluated {
                    // ONLY a `const` destructure is recorded, mirroring
                    // `lower::decl`'s own `if d.is_const` gate exactly.
                    //
                    // Recording a plain `let` here too would make the two
                    // sides disagree in the one direction that is never
                    // safe: typecheck would treat the name as a compile-time
                    // constant while lowering — whose non-`const` path is
                    // the NARROW `expr_to_literal_in`, and which therefore
                    // cannot fold the record literal such a binding
                    // destructures — would not. The name then satisfies a
                    // constant-only config slot at check time and is
                    // silently dropped at fold time (`lower::call::builtin`),
                    // so `let { chan } = { chan: "evt" }` +
                    // `SendCustomEvent(chan, v)` would compile clean and ship
                    // a gate with an EMPTY channel name. Gating here keeps
                    // WS028 rejecting that program.
                    //
                    // Widening LOWERING to match instead would not work: its
                    // non-`const` path is narrow by design (see
                    // `lower::decl`'s own comment on why widening it changes
                    // how a `const`-free program compiles), and the narrow
                    // evaluator cannot fold a record literal at all — so the
                    // value would still be dropped.
                    Ok(pairs) if l.is_const => {
                        for (name, lit) in pairs {
                            if let Some(frame) = ctx.scoped_consts.last_mut() {
                                frame.insert(name.clone(), lit);
                            }
                            if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                                frame.insert(name.clone());
                            }
                            if let Some(frame) = ctx.scoped_const_placeholders.last_mut() {
                                if derives_from_placeholder {
                                    frame.insert(name.clone());
                                } else {
                                    frame.remove(&name);
                                }
                            }
                        }
                        check_const_recorded(ctx, l);
                    }
                    // A plain `let` destructure re-binding a name must still
                    // CLEAR any `const` mark and value it shadows, or the
                    // outer constant would keep resolving through the shadow.
                    Ok(pairs) => {
                        for (name, _) in pairs {
                            if let Some(frame) = ctx.scoped_consts.last_mut() {
                                frame.remove(&name);
                            }
                            if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                                frame.remove(&name);
                            }
                            if let Some(frame) = ctx.scoped_const_placeholders.last_mut() {
                                frame.remove(&name);
                            }
                        }
                    }
                    Err(err) if l.is_const => ctx.emit(err.code(), err.message(), err.range.clone()),
                    Err(_) => {}
                }
            }
        }
        Stmt::Assign(a) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    format!(
                        "var write '{}' outside an exec context",
                        target_name(&a.target).unwrap_or("<expr>".into())
                    ),
                    a.range.clone(),
                );
            }
            let target_ty = infer_assign_target(ctx, &a.target);
            if let Expr::MapLit { entries, .. } = &a.value
                && let Type::Map(k, v) = &target_ty
            {
                // Assigning a map literal to a map var: the other valid slot —
                // validate entries directly instead of routing through the
                // generic `infer` position guard.
                check_map_literal(ctx, entries, k, v);
            } else {
                infer::check(ctx, &a.value, &target_ty);
            }
        }
        Stmt::OutBinding(b) => {
            // An anon-chip's statement-level `out` carries a type annotation
            // too (`chip { @bottom out done: any = 5 }`); mirror the top-level
            // `TopDecl::Out` any-warn so `any` there is flagged like anywhere
            // else.
            if let Some(te) = &b.typ {
                let resolved = resolve_type_expr(ctx, te);
                warn_any_annotation(ctx, &resolved, type_expr_range(te));
                if let Some(value) = &b.value {
                    // An annotated out must accept its value (WS003 on a
                    // genuine mismatch; coercions — including string → bool
                    // and primitive → string — pass). Both sides unwrap refs
                    // so the ref-ness is treated as exposure mode rather
                    // than a value-type difference; mirrors `TopDecl::Out`
                    // at ~1624.
                    let value_ty = infer::infer(ctx, value);
                    infer::coerce_or_emit(
                        ctx,
                        &unwrap_ref(&value_ty),
                        &unwrap_ref(&resolved),
                        value.range(),
                    );
                }
            } else if let Some(value) = &b.value {
                // No annotation: `b.name`'s declared type comes from the
                // enclosing decl's signature (`out r = v` inside `chip Foo(..)
                // -> (r: T)`) — check the value against it via the current
                // `out_ctx` frame. Both sides unwrap refs, same as the
                // annotated branch above. Not an output (e.g. shadowed by
                // something else) → nothing to check against.
                let vty = infer::infer(ctx, value);
                if let Some(out_ty) = current_output_ty(ctx, &b.name) {
                    infer::coerce_or_emit(
                        ctx,
                        &unwrap_ref(&vty),
                        &unwrap_ref(&out_ty),
                        value.range(),
                    );
                }
            }
        }
        Stmt::Emit(e) => {
            if e.value.is_none() && ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    format!("emit '{}' outside an exec context", e.name),
                    e.range.clone(),
                );
            }
            if let Some(ref val) = e.value {
                let t = infer::infer(ctx, val);
                // If `e.name` is a declared output (not a local exec
                // signal), its payload must match the declared type — both
                // sides unwrap refs, matching every other coercion check.
                // A local signal (no `out_ctx` entry for the name) is left
                // alone: nothing to check against.
                if let Some(out_ty) = current_output_ty(ctx, &e.name) {
                    infer::coerce_or_emit(ctx, &unwrap_ref(&t), &unwrap_ref(&out_ty), val.range());
                }
                // Remember the ferried payload type so a later
                // `let { .. } = await sig` can type its destructured fields.
                ctx.signal_payload_types.insert(e.name.clone(), t);
            }
        }
        Stmt::Await(a) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit("WS007", "await outside an exec context", a.range.clone());
            }
            // Push scope with `_` as Bool (the armed flag) for exec expression
            ctx.push_scope();
            ctx.scope.declare(
                "_",
                SymbolInfo {
                    kind: SymbolKind::LetBinding,
                    name: "_".into(),
                    ty: Type::Bool,
                    decl_range: a.range.clone(),
                    signature: None,
                    event_data: None,
                },
            );
            let exec_ty = infer::infer(ctx, &a.exec_expr);
            ctx.pop_scope();
            let val_ty = if let Some(ref val) = a.value_expr {
                infer::infer(ctx, val)
            } else {
                exec_ty
            };
            // Awaiting a CustomEvent/GlobalCustomEvent captures its DATA outputs
            // (`DataOut1..8`): a single `let x = await CustomEvent("c")` binds `x`
            // to the first data output, and a tuple `let (p, t) = ...` binds them
            // POSITIONALLY (p = DataOut1, t = DataOut2). The gate's data ports are
            // `any` in-game unless typed, so an untyped capture defaults to a float
            // wire and mis-delivers non-float data; the single-binding value is
            // typed by the `let x: T` annotation, and an untyped one is a WS055
            // nudge. `let v = await x on CustomEvent(...)` (a value IS captured) is
            // the plain trigger form and is unaffected.
            let event_capture = is_custom_event_call(&a.exec_expr) && a.value_expr.is_none();

            if let Some(ref binding) = a.binding {
                let ty = if event_capture {
                    match &a.binding_type {
                        Some(te) => resolve_type_expr(ctx, te),
                        None => {
                            ctx.warn(
                                "WS055",
                                format!(
                                    "the awaited event's data type can't be inferred here - annotate it \
                                     (`let {binding}: T = await CustomEvent(\"...\")`); an untyped capture \
                                     wires as a float and mis-delivers non-float data"
                                ),
                                a.range.clone(),
                            );
                            Type::Any
                        }
                    }
                } else {
                    val_ty
                };
                ctx.scope.declare(
                    binding,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: binding.clone(),
                        ty,
                        decl_range: a.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            // `let (p, t) = await CustomEvent(...)`: positional data-output capture.
            // No per-field annotation surface, so each is `any` with one WS055 nudge.
            if let Some(ref names) = a.tuple_destructure {
                if event_capture {
                    ctx.warn(
                        "WS055",
                        "tuple-captured event data is untyped (each field wires as a float) - capture typed \
                         values one at a time (`let p: T = await CustomEvent(\"...\")`), or receive them with \
                         a handler (`on CustomEvent(\"...\") -> (p: T, ...)`)",
                        a.range.clone(),
                    );
                }
                for local in names {
                    ctx.scope.declare(
                        local,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: local.clone(),
                            ty: Type::Any,
                            decl_range: a.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
            }
            // `let { a, b } = await sig`: type each destructured local from the
            // signal's recorded ferried-payload record (Any when unknown).
            if let Some(ref fields) = a.destructure {
                let payload_ty = match &a.exec_expr {
                    Expr::Ident { name, .. } => ctx.signal_payload_types.get(name).cloned(),
                    _ => None,
                };
                for (field, local) in fields {
                    let fty = match &payload_ty {
                        Some(Type::Record(fs)) => fs
                            .iter()
                            .find(|(n, _)| n == field)
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Type::Any),
                        _ => Type::Any,
                    };
                    ctx.scope.declare(
                        local,
                        SymbolInfo {
                            kind: SymbolKind::LetBinding,
                            name: local.clone(),
                            ty: fty,
                            decl_range: a.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
            }
        }
        Stmt::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "'if' statement outside an exec context",
                    i.range.clone(),
                );
            }
            ctx.if_contexts.insert(
                (i.range.file.clone(), i.range.start.offset),
                ctx.exec_mode() == ExecMode::Exec,
            );
            infer::infer(ctx, &i.cond);
            // `if constexpr` semantics: a const-evaluable condition is
            // resolved right here, through the SAME evaluator lowering's
            // `lower_if` uses, so the two stages agree on exactly which
            // condition is decidable and which way it goes — see
            // `dropped_ranges`'s doc comment for why that agreement matters.
            // The untaken block is NOT type-checked at all: it may not even
            // be valid code for this const value (e.g. a branch calling a mod
            // that only exists for a different mode), which is the point of
            // the feature.
            //
            // THREE guards keep the agreement exact, and all exist because
            // dropping a block HERE that lowering still EMITS ships
            // never-type-checked code:
            //
            // 1. `nofold_depth == 0` — the identical condition `lower_if`
            //    gates its own elision on. Under `@nofold` lowering builds a
            //    real `Branch` and lowers BOTH blocks, so this stage must
            //    check both too.
            // 2. Placeholder removal — a `const` PARAMETER is seeded into
            //    `scoped_consts` as a type-shaped zero while a mod body is
            //    checked (once, before any call site exists), and a
            //    placeholder must never decide a branch: lowering inlines
            //    the REAL argument and would pick the other way. Dropping
            //    placeholders from the environment makes such a condition
            //    simply unevaluable here, so both blocks are checked — the
            //    safe over-checking direction.
            // 3. const-DECLARED-only — a plain `let` that merely happens to
            //    fold (no `const` keyword anywhere) must NOT gain this
            //    elision: the feature's own rule is that a program using no
            //    `const` compiles identically to before. `if_cond_const_ctx`
            //    restricts the environment to names actually spelled `const`
            //    (see `const_declared`'s doc comment), so a condition built
            //    from a plain `let` simply fails to evaluate here and falls
            //    through to the general Branch path below.
            let cond_result = if ctx.nofold_depth > 0 {
                None
            } else {
                let lookup = |n: &str| ctx.resolve_mod(n);
                let mut budget = crate::const_eval::Budget::default();
                let cond_cx = ctx.if_cond_const_ctx(Some(&lookup));
                crate::const_eval::eval_expr(&i.cond, &cond_cx, &mut budget).ok()
            };
            match cond_result {
                Some(crate::ir::Literal::Bool(true)) => {
                    check_block(ctx, &i.then_block);
                    if let Some(else_b) = &i.else_block {
                        ctx.dropped_ranges.push((
                            else_b.range.clone(),
                            format!("`{}` is true here", describe_cond(&i.cond)),
                        ));
                    }
                }
                Some(crate::ir::Literal::Bool(false)) => {
                    ctx.dropped_ranges.push((
                        i.then_block.range.clone(),
                        format!("`{}` is false here", describe_cond(&i.cond)),
                    ));
                    if let Some(else_b) = &i.else_block {
                        check_block(ctx, else_b);
                    }
                }
                _ => {
                    check_block(ctx, &i.then_block);
                    if let Some(else_b) = &i.else_block {
                        check_block(ctx, else_b);
                    }
                }
            }
        }
        Stmt::ExprStmt(es) => {
            infer::infer(ctx, &es.expr);
        }
        Stmt::In(i) => {
            register_decl(ctx, &TopDecl::In(i.clone()));
        }
        Stmt::Handler(h) => {
            ctx.push_scope();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body));
            ctx.pop_scope();
        }
        Stmt::AnonChip(ac) => {
            // Anon chip shares parent scope — register + check inline.
            check_anon_chip_stmts(ctx, &ac.body.stmts, false);
        }
        Stmt::ChipDecl(c) => {
            register_decl(ctx, &TopDecl::Chip(c.clone()));
            check_decl(ctx, &TopDecl::Chip(c.clone()));
        }
        Stmt::Return { value, range } => {
            if ctx.exec_mode() != ExecMode::Exec && value.is_none() {
                ctx.emit(
                    "WS007",
                    "'return' (without value) outside an exec context",
                    range.clone(),
                );
            }
            if let Some(expr) = value {
                // `return <value>` wires into the enclosing single output (see
                // lowering's `output_count() == 1` path) — this fires the same
                // whether the `return` is in a mod/chip body OR a top-level
                // handler with one module output, so it's checked in both.
                // Clone the frame out first: `ctx.out_ctx.last()` borrows `ctx`
                // immutably, but `infer::check`/`infer::infer` below need
                // `&mut ctx`.
                let frame: Option<Vec<EventDataField>> = ctx.out_ctx.last().cloned();
                match frame.as_deref() {
                    Some([only]) => {
                        infer::check(ctx, expr, &unwrap_ref(&only.ty));
                    }
                    // Zero outputs, or multiple: nothing to check `return`'s
                    // value against for zero (there's no declared output);
                    // for multiple, a bare `Type::Tuple` of the outputs'
                    // types is the wrong shape to check against — the
                    // working multi-output mechanism is `return { a: .., b:
                    // .. }` (a NAME-keyed record, special-cased earlier in
                    // lowering's `Stmt::Return`, forwarded per-field rather
                    // than through a single value port), and `coerce`'s
                    // tuple/record-shape matching (`as_tuple_elems`) only
                    // treats an INDEX-keyed record (an actual tuple literal)
                    // as tuple-shaped — a name-keyed record return would
                    // false-positive WS003 against a positional
                    // `Type::Tuple`. Left unchecked rather than risk
                    // rejecting the legitimate case.
                    // TODO(P0-11): multi-output `return` value not yet
                    // checked.
                    _ => {
                        infer::infer(ctx, expr);
                    }
                }
            }
        }
    }
}

pub(super) fn target_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// Non-emitting predicate mirroring `infer_assign_target`'s accepted
/// writable-target shapes, used to validate `&`/`ref` operands (`WS008`)
/// without recording a diagnostic itself — the caller decides what to emit.
pub(crate) fn is_ref_able(ctx: &TypeCheckCtx, e: &Expr) -> bool {
    match e {
        Expr::Ident { name, .. } => match ctx.scope.lookup(name) {
            Some(s) => matches!(s.kind, SymbolKind::Var | SymbolKind::Array | SymbolKind::Map)
                // A `let` is only writable/ref-able when it's reference-backed
                // (an array/map binding); a scalar `let` is a computed wire, so
                // `&scalarLet` (and `scalarLet = …`) would silently no-op.
                || (s.kind == SymbolKind::LetBinding
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)))
                || (s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)))
                || (s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_))),
            None => false,
        },
        Expr::IndexAccess { obj, .. } => is_ref_able(ctx, obj),
        // A namespaced state member (`S.g`): ref-able when the member is a
        // var/array/map, exactly like its plain-imported form. Lowering wires
        // `&S.g` correctly, so without this arm the `&` check would wrongly
        // reject it as WS008.
        Expr::FieldAccess { obj, field, .. } => {
            if let Expr::Ident { name: ns, .. } = obj.as_ref()
                && ctx.scope.lookup(ns).map(|s| s.kind) == Some(SymbolKind::Namespace)
            {
                return ctx
                    .namespaces
                    .get(ns)
                    .and_then(|m| m.get(field))
                    .map(|info| {
                        matches!(
                            info.kind,
                            SymbolKind::Var | SymbolKind::Array | SymbolKind::Map
                        )
                    })
                    .unwrap_or(false);
            }
            false
        }
        _ => false,
    }
}

fn infer_assign_target(
    ctx: &mut TypeCheckCtx,
    e: &Expr,
) -> Type {
    if let Expr::Ident { name, range } = e {
        if name == "_" {
            return Type::Any;
        }
        let sym = ctx.scope.lookup(name).cloned();
        match sym {
            None => {
                ctx.emit(
                    "WS002",
                    format!("unknown identifier '{name}'"),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::Var => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Array => unwrap_ref(&s.ty),
            Some(s) if s.kind == SymbolKind::Map => unwrap_ref(&s.ty),
            // A `let` is only a writable target when it's reference-backed
            // (an array/map binding it aliases). A scalar `let` is a computed
            // wire, not storage: writing to it (`y = 5` on a `let y = x + 1`)
            // type-checks clean but emits no gate — the write vanishes.
            // A `const` container is the one reference-backed `let` that is
            // NOT a writable target. It is a compile-time value as well as a
            // runtime container, and a write would make those two disagree —
            // `xs[0] = 99` while `const z = xs[0]` still folds to the original
            // element.
            Some(s)
                if s.kind == SymbolKind::LetBinding
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _))
                    && ctx.is_const_container(name) =>
            {
                ctx.emit(
                    "WS007",
                    format!(
                        "'{name}' is a `const` array/map and can't be written — a `const` \
                         container is immutable so its compile-time value and its runtime \
                         contents can never disagree; declare it `var` to make it mutable"
                    ),
                    range.clone(),
                );
                Type::Any
            }
            Some(s)
                if s.kind == SymbolKind::LetBinding
                    && matches!(unwrap_ref(&s.ty), Type::Array(_) | Type::Map(_, _)) =>
            {
                unwrap_ref(&s.ty)
            }
            Some(s) if s.kind == SymbolKind::LetBinding => {
                ctx.emit(
                    "WS007",
                    format!(
                        "'{name}' is a `let` binding and can't be assigned — declare it `var` to make it mutable"
                    ),
                    range.clone(),
                );
                Type::Any
            }
            Some(s) if s.kind == SymbolKind::Param && matches!(&s.ty, Type::Ref(_)) => {
                unwrap_ref(&s.ty)
            }
            Some(s) if s.kind == SymbolKind::In && matches!(&s.ty, Type::Array(_)) => {
                unwrap_ref(&s.ty)
            }
            _ => {
                ctx.emit(
                    "WS007",
                    format!("'{name}' isn't a writable target"),
                    range.clone(),
                );
                Type::Any
            }
        }
    } else if let Expr::IndexAccess { obj, index, .. } = e {
        let obj_ty = infer_assign_target(ctx, obj);
        infer::infer(ctx, index);
        match obj_ty {
            Type::Array(inner) => *inner,
            // A map subscript write's value type is the map's VALUE type —
            // mirrors the array arm above and the read-position
            // `Expr::IndexAccess` arm in `infer.rs` (which also unwraps to
            // the value type). Without this arm `m[k] = <wrong type>`
            // type-checked clean (nothing to coerce the RHS against below),
            // silently miscompiling the MapVar_Set wire.
            Type::Map(_, v) => *v,
            _ => Type::Any,
        }
    } else if let Expr::FieldAccess { obj, .. } | Expr::TuplePick { obj, .. } = e {
        // A record/tuple field target (`p.x = 5`, `pts[i].inner.a = v`) is an
        // lvalue that lowering resolves; the target itself stays permissive
        // (typed `any`) as before. Infer the OBJECT for its side effect —
        // recording its type in the type_map — so hover can resolve a field
        // access written as an assignment target (`tk[i].phase = v`): without a
        // recorded type for `tk[i]`, the field hover had nothing to read.
        infer::infer(ctx, obj);
        Type::Any
    } else {
        // Any other target shape (a call result `f() = 5`, a literal, an
        // operator expression) is not an lvalue. It used to fall through to
        // `any` and be silently dropped at lowering; inside a handler even the
        // WS058/WS007 guards don't fire. Reject it.
        ctx.emit(
            "WS007",
            "this isn't a writable target; only a variable, an array/map \
             element, or a record field can be assigned"
                .to_string(),
            e.range().clone(),
        );
        Type::Any
    }
}

/// Best-effort rendering of a const `if` condition for the dropped-block
/// reason text (`` `N > 1` is true here ``) — NOT a general expression
/// pretty-printer (there isn't one in this crate; the `.ws` formatter lives
/// in a separate JS plugin), just enough to name the common shapes a
/// const-evaluable condition actually takes: a bare name, a literal, a unary
/// or binary operator over sub-expressions, and a `.field` read. Anything
/// else (a call, an index, ...) falls back to a generic placeholder — this
/// is purely explanatory text for a diagnostic, never parsed back.
fn describe_cond(e: &Expr) -> String {
    match e {
        Expr::Ident { name, .. } => name.clone(),
        Expr::BoolLit { value, .. } => value.to_string(),
        Expr::IntLit { text, .. } => text.clone(),
        Expr::FloatLit { text, .. } => text.clone(),
        Expr::StringLit { value, .. } => format!("\"{value}\""),
        Expr::UnOp { op, operand, .. } => format!("{op}{}", describe_cond(operand)),
        Expr::BinOp { op, left, right, .. } => {
            format!("{} {op} {}", describe_cond(left), describe_cond(right))
        }
        Expr::FieldAccess { obj, field, .. } => format!("{}.{field}", describe_cond(obj)),
        _ => "the condition".to_string(),
    }
}

/// True when `e` is a `CustomEvent("name")` / `GlobalCustomEvent("name")`
/// event-trigger call: the receiver side of a custom-event channel, which
/// exposes `DataOut1..8`. Used to type an `await CustomEvent(...)` data capture.
fn is_custom_event_call(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Call { callee, .. }
            if matches!(callee.as_ref(), Expr::Ident { name, .. } if name == "CustomEvent" || name == "GlobalCustomEvent")
    )
}
