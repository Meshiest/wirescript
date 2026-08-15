//! Pass 2: declaration checking.

use super::*;
use crate::types::mono::unwrap_ref;

// ---------- decl checking (2nd pass) ----------

/// Validate a top-level (non-exec) var initializer: every element must be a
/// constant literal whose type coerces to the array's element type. Spreads and
/// non-literal values are only meaningful when the array is built at runtime, so
/// they're rejected here with a pointer to assigning the array in an exec
/// handler.
fn check_top_level_array_init(
    ctx: &mut TypeCheckCtx,
    elements: &[ArrayElem],
    elem_ty: &Type,
) {
    for el in elements {
        let e = el.expr();
        let t = ctx.in_pure(|ctx| infer::infer(ctx, e));
        if matches!(el, ArrayElem::Spread(_)) {
            ctx.emit(
                "WS003",
                "spread `...` in an array initializer is only allowed when building the array inside an exec handler",
                e.range().clone(),
            );
        } else if let Some(lit) = crate::lower::expr_to_literal_in(e, &ctx.const_env) {
            // Asset / prefab references are object references — they lower to
            // their own reference gate (e.g. AudioReference) whose output must be
            // WIRED into the array, so they can't be baked into the initializer's
            // constant value list. Inlined here they'd be silently dropped.
            if matches!(
                lit,
                crate::ir::Literal::Asset { .. }
                    | crate::ir::Literal::PrefabRef { .. }
                    | crate::ir::Literal::NestedPrefab { .. }
            ) {
                ctx.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    code: "WS024".into(),
                    message: "asset / prefab references can't be inlined into an array initializer — \
                              they're object references wired in from their own brick. Build the array \
                              with `.push(...)` inside an exec handler instead."
                        .into(),
                    range: e.range().clone(),
                });
            } else if !matches!(elem_ty, Type::Any) && coerce(&t, elem_ty) == CoerceRule::Mismatch {
                ctx.emit(
                    "WS003",
                    format!(
                        "var element: expected {}, got {}",
                        crate::analysis::type_str(elem_ty),
                        crate::analysis::type_str(&t)
                    ),
                    e.range().clone(),
                );
            }
        } else {
            ctx.emit(
                "WS003",
                "array initializer elements must be constant literals — assign the array inside an exec handler to build it from runtime values",
                e.range().clone(),
            );
        }
    }
}

/// Check a map literal's entries against `Map<K, V>`: each key must coerce to
/// `k`, each value to `v`. This is deliberately STRICTER than the `coerce_or_emit`
/// sink assignment checking uses — a map *literal* entry additionally rejects
/// `ViaString` (and any coercion that isn't `Same`/`Coerce`), because the literal
/// entry has no gate to run a string-format coercion through (an assignment does).
/// Call this from the VALID slots — a `var` initializer or an assignment
/// RHS to a map var — so the literal never reaches the generic `infer`
/// `MapLit` arm, which is the position-guard for every other use.
///
/// The entries of a map initializer for a `Map<K, V>` slot: a real `MapLit`, or
/// an empty `{}`. The parser emits `{}` as an empty `RecordLit` (with no keys it
/// can't tell an empty map from an empty record), but in a `Map`-typed slot it
/// is the natural spelling of an empty-map initializer — equivalent to no
/// initializer, since lowering's `bake_map_init` starts the map empty for any
/// non-`MapLit` init. Returns `None` for anything else (a non-empty record, a
/// scalar, …) so it still hits the generic coerce / position guard.
pub(super) fn map_init_entries(init: &Expr) -> Option<&[MapLitEntry]> {
    match init {
        Expr::MapLit { entries, .. } => Some(entries),
        Expr::RecordLit { fields, .. } if fields.is_empty() => Some(&[]),
        _ => None,
    }
}

pub(super) fn check_map_literal(
    ctx: &mut TypeCheckCtx,
    entries: &[MapLitEntry],
    k: &Type,
    v: &Type,
) {
    // A map literal entry has no gate to run a coercion through (e.g.
    // `ViaString`'s `FormatText` gate) — only coercions that hold at the
    // literal itself (`Same`/`Coerce`) are valid; anything else (notably
    // `ViaString`) would silently corrupt every non-matching entry at emit.
    for MapLitEntry { key, value, .. } in entries {
        let kt = infer::infer(ctx, key);
        let key_rule = coerce(&kt, k);
        if !matches!(key_rule, CoerceRule::Same | CoerceRule::Coerce) {
            ctx.emit(
                "WS003",
                format!(
                    "map literal key: expected {}, got {} — a map literal entry \
                     can't be string-formatted; write a matching-type literal",
                    crate::analysis::types::type_str(k),
                    crate::analysis::types::type_str(&kt),
                ),
                key.range().clone(),
            );
        }
        let vt = infer::infer(ctx, value);
        let value_rule = coerce(&vt, v);
        if !matches!(value_rule, CoerceRule::Same | CoerceRule::Coerce) {
            ctx.emit(
                "WS003",
                format!(
                    "map literal value: expected {}, got {} — a map literal entry \
                     can't be string-formatted; write a matching-type literal",
                    crate::analysis::types::type_str(v),
                    crate::analysis::types::type_str(&vt),
                ),
                value.range().clone(),
            );
        }
    }
}

pub(super) fn check_decl(
    ctx: &mut TypeCheckCtx,
    d: &TopDecl,
) {
    match d {
        TopDecl::Var(v) => {
            if let Some(init) = &v.init {
                let inner = match ctx.scope.lookup(&v.name) {
                    Some(SymbolInfo {
                        ty: Type::Ref(inner),
                        ..
                    }) => inner.as_ref().clone(),
                    _ => Type::Any,
                };
                // An array-valued `var` declared at top level (no exec context)
                // is baked into the gate just like an `array` decl — its elements
                // must be constant literals.
                if let Expr::Array { elements, .. } = init {
                    // Type the whole array expr so its element type lands in the
                    // map — lowering reads it to infer an unannotated var's type.
                    ctx.in_pure(|ctx| {
                        infer::infer(ctx, init);
                    });
                    let elem_ty = match &inner {
                        Type::Array(e) => e.as_ref().clone(),
                        _ => Type::Any,
                    };
                    check_top_level_array_init(ctx, elements, &elem_ty);
                } else if let Some(entries) = map_init_entries(init)
                    && let Type::Map(k, v_ty) = &inner
                {
                    // A map-valued `var` initializer: validate entries against
                    // the declared `Map<K, V>` directly, bypassing the generic
                    // `infer` `MapLit` arm (the position guard) — this IS
                    // the valid initializer slot. An empty `{}` is an empty-map init.
                    ctx.in_pure(|ctx| {
                        check_map_literal(ctx, entries, k, v_ty);
                    });
                } else {
                    ctx.in_pure(|ctx| {
                        let t = infer::check(ctx, init, &inner);
                        // Unannotated var with a non-literal init (`var v =
                        // Vec(…)`): refine the placeholder `any` from the RHS,
                        // like buffers do.
                        if v.typ.is_none() && matches!(inner, Type::Any) {
                            let u = unwrap_ref(&t);
                            if var_storable(&u) {
                                ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                            }
                        }
                    });
                }
            }
        }
        TopDecl::Buffer(b) => {
            ctx.in_pure(|ctx| {
                let t = infer::infer(ctx, &b.init);
                if b.typ.is_none() {
                    let unwrapped = unwrap_ref(&t);
                    ctx.scope.set_type(&b.name, unwrapped);
                }
            });
        }
        TopDecl::Array(a) => {
            // A top-level initializer is baked into the gate, so its elements
            // must be constant literals matching the element type.
            if !a.init.is_empty() {
                let inner = resolve_type_expr(ctx, &a.element_type);
                check_top_level_array_init(ctx, &a.init, &inner);
            }
        }
        TopDecl::Map(m) => {
            // Optional literal initializer: `var m: Map<K, V> = { k => v, ... }`.
            // Key/value types were already resolved in registration — reuse the
            // registered symbol instead of re-resolving (avoids double-emitting
            // an "unknown type" diagnostic if `key_type`/`value_type` is bad).
            if let Some(init) = &m.init {
                let (key, value) = match ctx.scope.lookup(&m.name) {
                    Some(SymbolInfo {
                        ty: Type::Map(k, v),
                        ..
                    }) => (k.as_ref().clone(), v.as_ref().clone()),
                    _ => (Type::Any, Type::Any),
                };
                if let Some(entries) = map_init_entries(init) {
                    // The valid initializer slot: validate entries directly,
                    // bypassing the generic `infer` `MapLit` arm (the
                    // position guard). An empty `{}` is an empty-map init.
                    ctx.in_pure(|ctx| {
                        check_map_literal(ctx, entries, &key, &value);
                    });
                } else {
                    ctx.in_pure(|ctx| {
                        infer::check(
                            ctx,
                            init,
                            &Type::Map(Box::new(key.clone()), Box::new(value.clone())),
                        );
                    });
                }
            }
        }
        TopDecl::In(_) => {
            // Already handled in registration.
        }
        TopDecl::Out(b) => {
            if let Some(value) = &b.value {
                let value_ty = ctx.in_pure(|ctx| infer::infer(ctx, value));
                // When out has ref type and value is a var, override to show "ref" in hover
                if let Some(ref te) = b.typ {
                    let resolved = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &resolved, type_expr_range(te));
                    if matches!(resolved, Type::Ref(_))
                        && let Expr::Ident { range, .. } = value
                    {
                        ctx.var_read_contexts
                            .remove(&(range.file.clone(), range.start.offset));
                    }
                    // An annotated out must accept its value (WS003 on a
                    // genuine mismatch; coercions — including string → bool,
                    // which lowers to an inserted `!= ""` compare at the
                    // port — pass). Both sides unwrap refs so `out y: *int
                    // = x` compares int against int, the ref-ness being the
                    // exposure mode rather than a value type.
                    infer::coerce_or_emit(
                        ctx,
                        &unwrap_ref(&value_ty),
                        &unwrap_ref(&resolved),
                        value.range(),
                    );
                }
                if b.typ.is_none()
                    && let Expr::Ident { name, .. } = value
                    && let Some(sym) = ctx.scope.lookup(name)
                    && sym.kind == SymbolKind::Var
                {
                    ctx.diagnostics.push(Diagnostic {
                                    severity: Severity::Warning,
                                    code: "WS017".into(),
                                    message: format!(
                                        "out '{}' infers type from var '{}' — add explicit type: \
                                         `out {}: {} = {}` for value, or `out {}: *{} = {}` for ref",
                                        b.name, name,
                                        b.name, crate::analysis::types::type_str(&unwrap_ref(&sym.ty)), name,
                                        b.name, crate::analysis::types::type_str(&unwrap_ref(&sym.ty)), name,
                                    ),
                                    range: b.range.clone(),
                                });
                }
            }
        }
        TopDecl::Let(l) => {
            let t = ctx.in_pure(|ctx| infer::infer(ctx, &l.value));
            check_let_type_annotation(ctx, l, &t);
            record_single_output_alias(ctx, &l.binding, &l.value);
            bind_let(ctx, &l.binding, &t);
        }
        TopDecl::Fn(f) => {
            ctx.push_scope();
            for p in &f.params {
                let pt = resolve_type_expr(ctx, &p.typ);
                ctx.scope.declare(
                    &p.name,
                    SymbolInfo {
                        kind: SymbolKind::Param,
                        name: p.name.clone(),
                        ty: pt,
                        decl_range: p.range.clone(),
                        signature: None,
                        event_data: None,
                    },
                );
            }
            ctx.in_pure(|ctx| {
                infer::infer(ctx, &f.body);
            });
            ctx.pop_scope();
        }
        TopDecl::Chip(c) => {
            // Bounded-polymorphism body checking. Body-check the
            // decl once per concrete member of the cartesian product of its
            // type params' masks — an operation on a type param (`a + 1`)
            // can't resolve against the signature's `Type::Param`
            // placeholder (registration, untouched by this
            // arm), and even where it somehow could, the body must be valid
            // for EVERY concrete type the mask allows, not just checked once
            // symbolically. A non-generic decl (`type_params` empty) gets
            // the single trivial "no bindings" combo below, so its check —
            // and its diagnostics — are unchanged from before this task.
            const MAX_BODY_CHECK_COMBOS: usize = 64;
            // Budget is a whole-PATH cap, not per-decl: a nested generic decl is
            // re-checked once per OUTER combo, so divide the cap by the combos
            // already committed on this path (`active_combos`). At the top level
            // `active_combos == 1`, so a single generic decl is unchanged (cap
            // 64); deeper nesting shrinks the effective cap toward 1, bounding
            // total body-check work at ~64 regardless of nesting depth.
            let per_path_cap = (MAX_BODY_CHECK_COMBOS / ctx.active_combos.max(1)).max(1);
            let combos: Vec<Vec<Type>> = if c.type_params.is_empty() {
                vec![Vec::new()]
            } else {
                let masks: Vec<Vec<Type>> = c
                    .type_params
                    .iter()
                    .map(|tp| {
                        let m = type_param_mask(ctx, tp);
                        if m.is_empty() { vec![Type::Any] } else { m }
                    })
                    .collect();
                let mut total: usize = 1;
                let mut capped = false;
                for m in &masks {
                    match total.checked_mul(m.len()) {
                        Some(t) if t <= per_path_cap => total = t,
                        _ => {
                            capped = true;
                            break;
                        }
                    }
                }
                if capped {
                    // Too many combinations for the full cartesian product.
                    // A single all-first-member combo is unsound: a param's
                    // first mask member is the MOST permissive for arithmetic
                    // (an unbounded param's is `bool`, which coerces into every
                    // numeric op), so a body op invalid for other members would
                    // slip through unchecked (a silent miscompile at any call
                    // site that picks those members). Instead vary each param
                    // across its WHOLE mask while the others hold a fixed
                    // representative (mask[0]). This checks every operation
                    // whose validity depends on a single param's type — the
                    // realistic case — at O(sum of mask sizes) rather than
                    // O(product), then truncates to the path budget so nested
                    // generics stay bounded. Residual: an op valid for every
                    // single-param variation yet invalid only for a specific
                    // multi-param COMBINATION is not caught (needs ≥2 unbounded
                    // params AND a cross-param-only type dependency).
                    let rep: Vec<Type> = masks.iter().map(|m| m[0].clone()).collect();
                    let mut combos = vec![rep.clone()];
                    for (i, m) in masks.iter().enumerate() {
                        for member in m.iter().skip(1) {
                            let mut combo = rep.clone();
                            combo[i] = member.clone();
                            combos.push(combo);
                        }
                    }
                    combos.truncate(per_path_cap.max(1));
                    combos
                } else {
                    cartesian_product(&masks)
                }
            };
            // Diagnostics are only batched + deduped for an actual
            // multi-combo generic check; the non-generic single combo
            // writes straight into `ctx.diagnostics` exactly as before.
            let multi_pass = combos.len() > 1;
            let mut seen = std::collections::HashSet::new();
            let mut deduped: Vec<Diagnostic> = Vec::new();
            // Commit this decl's combos to the path budget so any nested generic
            // decl checked inside the loop divides the REMAINING cap (restored
            // after). Non-generic decls (`combos.len() == 1`) don't move it.
            let saved_active_combos = ctx.active_combos;
            ctx.active_combos = saved_active_combos.saturating_mul(combos.len().max(1));
            for combo in &combos {
                ctx.push_scope();
                // Same registration as the signature pass (register_decl),
                // but in the body scope: covers annotations inside the body
                // (e.g. `let x: T = a`). For a generic decl each type param
                // is bound to THIS combo's concrete mask member (not
                // `Type::Param` — an operation like `a + 1` must resolve
                // against a real type); `combo` is empty for a non-generic
                // decl, so this loop is a no-op there. Scope is popped at
                // the end of each pass, so params never leak to sibling
                // decls.
                for (tp, ty) in c.type_params.iter().zip(combo.iter()) {
                    ctx.scope.declare(
                        &tp.name,
                        SymbolInfo {
                            kind: SymbolKind::Type,
                            name: tp.name.clone(),
                            ty: ty.clone(),
                            decl_range: tp.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
                }
                let before = ctx.diagnostics.len();
                for p in &c.inputs {
                    let pt = resolve_type_expr(ctx, &p.typ);
                    let kind = if matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }) {
                        SymbolKind::Var
                    } else {
                        SymbolKind::Param
                    };
                    // If the param has a destructuring pattern, register the
                    // synthetic name with the full type, then also register each
                    // destructured field with its resolved field type.
                    if let Some(pattern) = &p.pattern {
                        ctx.scope.declare(
                            &p.name,
                            SymbolInfo {
                                kind: SymbolKind::Param,
                                name: p.name.clone(),
                                ty: pt.clone(),
                                decl_range: p.range.clone(),
                                signature: None,
                                event_data: None,
                            },
                        );
                        match pattern {
                            crate::ast::ParamPattern::Record { fields, .. } => {
                                for field in fields {
                                    match field {
                                        crate::ast::RecordDestructField::Named {
                                            name, alias, ..
                                        } => {
                                            let bind_name = alias.as_ref().unwrap_or(name);
                                            let field_ty = if let Type::Record(rec_fields) = &pt {
                                                rec_fields
                                                    .iter()
                                                    .find(|(k, _)| k == name)
                                                    .map(|(_, t)| t.clone())
                                                    .unwrap_or(Type::Any)
                                            } else {
                                                Type::Any
                                            };
                                            ctx.scope.declare(
                                                bind_name,
                                                SymbolInfo {
                                                    kind: SymbolKind::Param,
                                                    name: bind_name.clone(),
                                                    ty: field_ty,
                                                    decl_range: p.range.clone(),
                                                    signature: None,
                                                    event_data: None,
                                                },
                                            );
                                        }
                                        crate::ast::RecordDestructField::Rest { name, .. } => {
                                            ctx.scope.declare(
                                                name,
                                                SymbolInfo {
                                                    kind: SymbolKind::Param,
                                                    name: name.clone(),
                                                    ty: Type::Any,
                                                    decl_range: p.range.clone(),
                                                    signature: None,
                                                    event_data: None,
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                            crate::ast::ParamPattern::Tuple { names, .. } => {
                                let field_types = if let Type::Tuple(fs) = &pt {
                                    fs.clone()
                                } else {
                                    vec![]
                                };
                                for (i, name) in names.iter().enumerate() {
                                    let field_ty = field_types.get(i).cloned().unwrap_or(Type::Any);
                                    ctx.scope.declare(
                                        name,
                                        SymbolInfo {
                                            kind: SymbolKind::Param,
                                            name: name.clone(),
                                            ty: field_ty,
                                            decl_range: p.range.clone(),
                                            signature: None,
                                            event_data: None,
                                        },
                                    );
                                }
                            }
                        }
                    } else {
                        ctx.scope.declare(
                            &p.name,
                            SymbolInfo {
                                kind,
                                name: p.name.clone(),
                                ty: pt,
                                decl_range: p.range.clone(),
                                signature: None,
                                event_data: None,
                            },
                        );
                    }
                }
                // Push this combo's declared-output frame (nearest wins over
                // the module frame) so `return`/`emit`/an unannotated `out`
                // inside the body can be checked against it. Built AFTER the
                // param binding above so a generic output type (`-> (r: T)`)
                // resolves against THIS combo's concrete type param bindings,
                // not a leftover `Type::Param`.
                //
                // `resolve_type_expr` emits (e.g. WS002 "unknown type"); these
                // same output annotations are already resolved+reported by
                // `register_decl`, so DISCARD this frame build's diagnostics by
                // truncating back to the pre-build length. The snapshot is taken
                // AFTER the input-param loop's `before` baseline, so this only
                // drops the frame build's own diagnostics — the later
                // `split_off(before)` dedup window (which still starts at
                // `before`) keeps every real input-param and body diagnostic.
                let chip_diag_mark = ctx.diagnostics.len();
                let chip_outs: Vec<EventDataField> = c
                    .outputs
                    .iter()
                    .map(|o| EventDataField {
                        name: o.name.clone(),
                        ty: resolve_type_expr(ctx, &o.typ),
                    })
                    .collect();
                ctx.diagnostics.truncate(chip_diag_mark);
                ctx.out_ctx.push(chip_outs);
                ctx.in_exec(|ctx| check_block(ctx, &c.body));
                // Warn if outputs are declared but never assigned. Assignments
                // count anywhere in the body, including nested if blocks and
                // `on` handlers: `out x = expr`, `emit x (= expr)`, or a plain
                // `x = expr` assignment.
                if !c.outputs.is_empty() && !block_has_return_value(&c.body) {
                    let mut assigned = std::collections::HashSet::default();
                    collect_output_assignments(&c.body, &mut assigned);
                    for out in &c.outputs {
                        if !assigned.contains(&out.name) {
                            ctx.emit(
                                "WS013",
                                format!("output '{}' is never assigned — use `out {} = expr`, `emit {}`, or `return expr`", out.name, out.name, out.name),
                                out.range.clone(),
                            );
                        }
                    }
                }
                if multi_pass {
                    // Dedup by (code, start offset): the same body error
                    // surfaces once per mask member it's invalid for, but
                    // should be reported once, at the definition.
                    for d in ctx.diagnostics.split_off(before) {
                        if seen.insert((d.code.clone(), d.range.start.offset)) {
                            deduped.push(d);
                        }
                    }
                }
                ctx.out_ctx.pop();
                ctx.pop_scope();
            }
            ctx.active_combos = saved_active_combos;
            if multi_pass {
                ctx.diagnostics.extend(deduped);
            }
        }
        TopDecl::AnonChip(ac) => {
            // Anon chip shares parent scope — NO scope push/pop.
            // Vars already pre-registered in pass 1; use check_decl (not
            // check_stmt) for them to avoid duplicate-declaration errors.
            check_anon_chip_stmts(ctx, &ac.body.stmts, true);
        }
        TopDecl::Event(e) => {
            if let Some(body) = &e.captured_body {
                ctx.in_exec(|ctx| check_block(ctx, body));
            } else {
                ctx.in_pure(|ctx| {
                    infer::infer(ctx, &e.source);
                });
            }
        }
        TopDecl::Handler(h) => {
            ctx.push_scope();
            bind_handler_trigger_params(ctx, h);
            check_handler_input_wires(ctx, h);
            ctx.in_exec(|ctx| check_block(ctx, &h.body));
            ctx.pop_scope();
        }
        TopDecl::ExprStmt(s) => {
            ctx.in_pure(|ctx| {
                infer::infer(ctx, &s.expr);
            });
        }
        TopDecl::Assign(a) => {
            check_stmt(ctx, &Stmt::Assign(a.clone()));
        }
        TopDecl::If(i) => {
            if ctx.exec_mode() != ExecMode::Exec {
                ctx.emit(
                    "WS007",
                    "top-level 'if' outside an exec context",
                    i.range.clone(),
                );
            }
            check_stmt(ctx, &Stmt::If(i.clone()));
        }
        TopDecl::Namespace(ns) => {
            // A namespaced (`import * as ns`) mod body references its sibling
            // constants and mods by BARE name, and those mods are inlined at
            // call sites in the importing module. Typecheck the bodies here in
            // an isolated scope (siblings registered as bare names) so operator
            // resolutions and expression types get recorded — otherwise the
            // inlined body's arithmetic and sibling calls lower to _Unsupported.
            ctx.push_scope();
            for d in &ns.decls {
                register_decl(ctx, d);
            }
            for d in &ns.decls {
                if matches!(
                    d,
                    TopDecl::Let(_) | TopDecl::Var(_) | TopDecl::Array(_) | TopDecl::Buffer(_)
                ) {
                    check_decl(ctx, d);
                }
            }
            for d in &ns.decls {
                if matches!(d, TopDecl::Chip(_) | TopDecl::Fn(_)) {
                    check_decl(ctx, d);
                }
            }
            ctx.pop_scope();
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) => {}
    }
}
