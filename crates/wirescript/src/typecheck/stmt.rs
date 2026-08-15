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

pub(super) fn check_stmt(
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
            let t = infer::infer(ctx, &l.value);
            check_let_type_annotation(ctx, l, &t);
            record_single_output_alias(ctx, &l.binding, &l.value);
            bind_let(ctx, &l.binding, &t);
            // A body-local `let name = <constant>` is recorded in the
            // innermost `scoped_consts` frame (mirroring how a top-level
            // `let` lands in `const_env`), so a constant-only config arg
            // elsewhere in this scope (or a nested one) can resolve `name`
            // via `ctx.const_lookup()`. Only the simple `Ident` binding form
            // can name a single constant — a destructured `let` can't.
            if let LetBinding::Ident { name, .. } = &l.binding
                && let Some(lit) = crate::lower::expr_to_literal_in(&l.value, &ctx.const_lookup())
                && let Some(frame) = ctx.scoped_consts.last_mut()
            {
                frame.insert(name.clone(), lit);
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
                // something else) → nothing to check against, same as before.
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
            if let Some(ref binding) = a.binding {
                ctx.scope.declare(
                    binding,
                    SymbolInfo {
                        kind: SymbolKind::LetBinding,
                        name: binding.clone(),
                        ty: val_ty,
                        decl_range: a.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            // `let { a, b } = await sig`: type each destructured local from the
            // signal's recorded payload record (Any when unknown).
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
            check_block(ctx, &i.then_block);
            if let Some(else_b) = &i.else_block {
                check_block(ctx, else_b);
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
            // wire, not storage: `y = 5` on a `let y = x + 1` type-checked
            // clean and then emitted NO gate (the write vanished).
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
    } else {
        Type::Any
    }
}
