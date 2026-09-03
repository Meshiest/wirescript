//! Pass 2: declaration checking.

use super::*;
use crate::types::mono::unwrap_ref;

// ---------- decl checking (2nd pass) ----------

/// A type-shaped stand-in for a `const` parameter's real (call-site-only)
/// value, used solely to seed `scoped_consts` so structural constant checks
/// (WS028 and friends, via `expr_to_literal_in`) see the param as constant
/// during a mod/chip body's one-time check. Never observed by anything that
/// cares about the actual value — see the call site in the `TopDecl::Chip`
/// body-check loop.
///
/// Delegates to lowering's zero-value table so the placeholder's TYPE is
/// always right (a `const vector` param gets a `Vector`, not a `Float`).
/// Harmless while every reader is presence-only, but a landmine for any
/// future check that inspects the literal's type — and sharing the one table
/// keeps the two from drifting as types are added.
fn const_param_placeholder(t: &Type) -> crate::ir::Literal {
    crate::lower::default_literal_for_var_type(t).unwrap_or(crate::ir::Literal::Float(0.0))
}

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
        } else if let Some(lit) = fold_array_init_element(ctx, e) {
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
        }
    }
}

/// Fold one array-initializer element to a constant, EMITTING the diagnostic
/// itself when it can't. Returns `None` after emitting, so the caller has
/// nothing left to report — which is the point: the two failure kinds want
/// different messages and must not both fire.
///
/// `expr_to_literal_in` alone only resolves literals, named constants and
/// operators over them — it has no notion of a `const mod` CALL. The full
/// const evaluator runs as a fallback (it retries `expr_to_literal_in` first
/// internally, so every element that already folded keeps folding exactly the
/// same way; this only ADDS the const-mod-call surface), which is what lets an
/// element like `size(1)` be recognised as constant HERE — without it, the
/// bake in `lower::predeclare` never runs at all, because this check rejects
/// the initializer before lowering ever sees it.
///
/// On failure the reason decides the message. An out-of-range index, a
/// missing map key, or a missing record field describes a collection/record
/// that IS fully constant but was subscripted/accessed past what it has, so
/// the evaluator's own wording is emitted verbatim — telling the user to
/// "build the array in an exec handler" is actively misleading advice for
/// `[[1, 2][9]]`, whose problem is the index.
///
/// Every OTHER reason keeps the generic WS003, deliberately — including
/// `Refused` (e.g. `[1 << 64]`, an operator declining its operands), whose
/// WS003 is pinned by `lower::tests::const_init::out_of_range_shift_is_not_folded`.
/// Widening this to `Refused`/`BudgetExceeded` would produce a more accurate
/// message, but it would change a diagnostic code that test asserts, so it's
/// left as WS003.
fn fold_array_init_element(ctx: &mut TypeCheckCtx, e: &Expr) -> Option<crate::ir::Literal> {
    if let Some(lit) = crate::lower::expr_to_literal_in(e, &ctx.const_env) {
        return Some(lit);
    }
    let evaluated = {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget)
    };
    match evaluated {
        Ok(lit) => Some(lit),
        Err(err) => {
            use crate::const_eval::ConstReason as R;
            match err.reason {
                R::ArrayIndexOutOfRange { .. }
                | R::MapKeyNotFound
                | R::RecordFieldNotFound(_)
                | R::TupleArityMismatch { .. } => {
                    ctx.emit(err.code(), err.message(), err.range.clone())
                }
                R::NotConstant(_)
                | R::NotAConstMod(_)
                | R::NestedConstModCall(_)
                | R::Unsupported(_)
                | R::UnsupportedMessage(_)
                | R::UnsupportedMethod(_)
                | R::Refused(_)
                | R::BudgetExceeded => ctx.emit(
                    "WS003",
                    "array initializer elements must be constant literals — assign the array inside an exec handler to build it from runtime values",
                    e.range().clone(),
                ),
            }
            None
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

/// One side (key or value) of a map literal entry, checked against the map's
/// declared type for that side.
///
/// A map literal entry has no gate to run a coercion through (e.g.
/// `ViaString`'s `FormatText` gate) — only coercions that hold at the literal
/// itself (`Same`/`Coerce`) are valid; anything else (notably `ViaString`) would
/// silently corrupt every non-matching entry at emit.
fn check_map_entry_side(
    ctx: &mut TypeCheckCtx,
    got: &Type,
    want: &Type,
    side: &str,
    at: &Expr,
) {
    if matches!(coerce(got, want), CoerceRule::Same | CoerceRule::Coerce) {
        return;
    }
    ctx.emit(
        "WS003",
        format!(
            "map literal {side}: expected {}, got {} — a map literal entry \
             can't be string-formatted; write a matching-type literal",
            crate::analysis::types::type_str(want),
            crate::analysis::types::type_str(got),
        ),
        at.range().clone(),
    );
}

pub(super) fn check_map_literal(
    ctx: &mut TypeCheckCtx,
    entries: &[MapLitEntry],
    k: &Type,
    v: &Type,
) {
    for MapLitEntry { key, value, .. } in entries {
        let kt = infer::infer(ctx, key);
        check_map_entry_side(ctx, &kt, k, "key", key);
        let vt = infer::infer(ctx, value);
        check_map_entry_side(ctx, &vt, v, "value", value);
    }
}

/// The `Map<K, V>` a `const m = { … }` binds, with its entries validated.
///
/// A `const` binding is a THIRD valid slot for a map literal, alongside a `var`
/// initializer and an assignment RHS — those two route through
/// [`check_map_literal`] rather than the generic `Expr::MapLit` arm in `infer`,
/// which is a POSITION GUARD (WS026) and would report a valid position as an
/// error. This is the same routing for the slot the other two can't serve:
/// a `const` carries no declared container type, so K and V are inferred from
/// the first entry (mirroring the array literal's element-type inference) and
/// every entry is then checked against them with the same per-side rule, so a
/// heterogeneous entry is still rejected.
///
/// The guard itself is untouched: a map literal in a genuinely invalid position
/// still reaches `infer` and still reports WS026.
pub(super) fn check_const_map_literal(ctx: &mut TypeCheckCtx, entries: &[MapLitEntry]) -> Type {
    // Every entry is inferred ONCE, up front — inferring the first entry
    // separately to source K/V would report any diagnostic inside it twice.
    let inferred: Vec<(Type, Type)> = entries
        .iter()
        .map(|e| (infer::infer(ctx, &e.key), infer::infer(ctx, &e.value)))
        .collect();
    let (k, v) = inferred
        .first()
        .cloned()
        .unwrap_or((Type::Any, Type::Any));
    for (entry, (kt, vt)) in entries.iter().zip(inferred.iter()) {
        check_map_entry_side(ctx, kt, &k, "key", &entry.key);
        check_map_entry_side(ctx, vt, &v, "value", &entry.value);
    }
    Type::Map(Box::new(k), Box::new(v))
}

/// The type of a `let`/`const` initializer. Shared by the `TopDecl::Let` and
/// `Stmt::Let` arms so the two scopes can't drift.
///
/// `infer`, except for the one position `infer`'s map-literal guard would
/// wrongly reject: `const m = { … }` (see [`check_const_map_literal`]).
pub(super) fn infer_let_init(ctx: &mut TypeCheckCtx, l: &crate::ast::LetDecl) -> Type {
    if l.is_const
        && let Expr::MapLit { entries, range } = &l.value
    {
        let t = check_const_map_literal(ctx, entries);
        // `infer` records every node it types; this replaces one `infer` call,
        // so it owes the same record.
        ctx.type_of_expr.insert(
            (range.file.clone(), range.start.offset, range.end.offset),
            t.clone(),
        );
        return t;
    }
    // Expose the let's annotation as the expected-type hint, so a generic enum
    // construction whose variant can't pin a type parameter takes it from the
    // annotation (`let n: Option<int> = Option.None`) instead of WS063 - the
    // same push `check` does for an annotated `out`. The annotation's own
    // value-vs-type check (`check_let_type_annotation`) re-resolves and owns the
    // authoritative diagnostics, so any this resolution emits is dropped to
    // avoid double-reporting a malformed annotation.
    let prev = match &l.typ {
        Some(te) => {
            let before = ctx.diagnostics.len();
            let resolved = resolve_type_expr(ctx, te);
            ctx.diagnostics.truncate(before);
            Some(ctx.expected_ty.replace(resolved))
        }
        None => None,
    };
    let t = infer::infer(ctx, &l.value);
    if let Some(prev) = prev {
        ctx.expected_ty = prev;
    }
    t
}

/// Whether this declaration carries `@nofold`. Mirrors the set of
/// `ctx.with_nofold(<decl>.no_fold, …)` wraps in `lower::decl::lower_decl` (Out,
/// Handler, Event, Let, Var) plus `ChipDecl`, whose body lowering marks via
/// `build_chip_module`'s own `nofold_depth` seed. Wrapping the whole dispatch
/// on this — rather than per arm — is what keeps the two stages from drifting
/// as arms are added: a variant with no `no_fold` field simply reports `false`
/// and behaves exactly as before.
pub(super) fn decl_no_fold(d: &TopDecl) -> bool {
    match d {
        TopDecl::Out(b) => b.no_fold,
        TopDecl::Handler(h) => h.no_fold,
        TopDecl::Event(e) => e.no_fold,
        TopDecl::Let(l) => l.no_fold,
        TopDecl::Var(v) => v.no_fold,
        TopDecl::Chip(c) => c.no_fold,
        TopDecl::Await(a) => a.no_fold,
        _ => false,
    }
}

pub(super) fn check_decl(
    ctx: &mut TypeCheckCtx,
    d: &TopDecl,
) {
    ctx.with_nofold(decl_no_fold(d), |ctx| check_decl_inner(ctx, d));
}

/// The innermost record type backing a storage declaration — unwrapping a `ref`,
/// the array element, or the map value — so an aggregate (`var p: Rec`,
/// `Rec[]`, `Map<K, Rec>`) is checked from one place regardless of container.
fn aggregate_record_fields(ty: &Type) -> Option<&Vec<(String, Type)>> {
    match ty {
        Type::Ref(inner) => aggregate_record_fields(inner),
        Type::Array(e) => aggregate_record_fields(e),
        Type::Map(_, v) => aggregate_record_fields(v),
        Type::Record(f) => Some(f),
        _ => None,
    }
}

/// A record used as var/array/map STORAGE decomposes into one backing gate per
/// field (see `lower::predeclare::declare_record_container`), so every leaf must
/// be a value that a gate can hold. A reference-only field (`ref T`/zone/
/// teleport/prefab) or an `exec` can't — reject it with WS049. Nested records
/// recurse; variant, array, and map fields are fine.
fn check_aggregate_record(ctx: &mut TypeCheckCtx, ty: &Type, range: &SourceRange) {
    fn reason(ft: &Type) -> Option<&'static str> {
        match ft {
            Type::Ref(_) => Some("a reference (`*T`)"),
            Type::Zone => Some("a `zone`"),
            Type::Teleport => Some("a `teleport`"),
            Type::PrefabRef => Some("a prefab reference"),
            Type::Exec => Some("an `exec`"),
            _ => None,
        }
    }
    fn walk(ctx: &mut TypeCheckCtx, fields: &[(String, Type)], range: &SourceRange) {
        for (name, ft) in fields {
            if let Type::Record(sub) = ft {
                walk(ctx, sub, range);
            } else if let Some(reason) = reason(ft) {
                ctx.emit(
                    "WS049",
                    format!(
                        "record field `{name}` is {reason}, which can't be stored — a \
                         record used as a variable, array, or map may only hold value \
                         fields (numbers, strings, vectors, entities, nested records, \
                         or arrays/maps)"
                    ),
                    range.clone(),
                );
            }
        }
    }
    if let Some(fields) = aggregate_record_fields(ty) {
        walk(ctx, fields, range);
    }
}

fn check_decl_inner(
    ctx: &mut TypeCheckCtx,
    d: &TopDecl,
) {
    // A record used as var/array/map storage must have only storable leaf fields.
    let storage_type = match d {
        TopDecl::Var(v) => Some((ctx.scope.lookup(&v.name).map(|s| s.ty.clone()), &v.range)),
        TopDecl::Array(a) => Some((ctx.scope.lookup(&a.name).map(|s| s.ty.clone()), &a.range)),
        TopDecl::Map(m) => Some((ctx.scope.lookup(&m.name).map(|s| s.ty.clone()), &m.range)),
        _ => None,
    };
    if let Some((Some(ty), range)) = storage_type {
        check_aggregate_record(ctx, &ty, &range.clone());
    }

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
                    // A top-level `var`'s initializer is baked, so it is checked
                    // pure; inside an exec body it runs on the chain instead.
                    let force_pure = ctx.exec_mode() != ExecMode::Exec;
                    ctx.maybe_pure(force_pure, |ctx| {
                        let t = infer::check(ctx, init, &inner);
                        // An unannotated `var x = a.push(5)` adopts the init's
                        // type; reject a void-mutation `Never` here (an annotated
                        // var already mismatches via `check` above).
                        if v.typ.is_none() {
                            reject_never_value(ctx, &unwrap_ref(&t), init.range(), &v.name);
                        }
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
                if let Some(te) = &b.typ {
                    let resolved = resolve_type_expr(ctx, te);
                    warn_any_annotation(ctx, &resolved, type_expr_range(te));
                    // When out has ref type and value is a var, override to show "ref" in hover
                    if matches!(resolved, Type::Ref(_))
                        && let Expr::Ident { range, .. } = value
                    {
                        ctx.var_read_contexts
                            .remove(&(range.file.clone(), range.start.offset));
                    }
                    // An annotated out must accept its value: `check` coerces it
                    // into the port type (WS003 on a genuine mismatch; the string
                    // → bool `!= ""` compare and friends pass) AND resolves a
                    // bidirectional literal like `null` to the port's type. Both
                    // sides unwrap refs so `out y: *int = x` compares int against
                    // int, the ref-ness being the exposure mode, not a value type.
                    ctx.in_pure(|ctx| infer::check(ctx, value, &unwrap_ref(&resolved)));
                } else {
                    // An unannotated `out y = a.push(5)` would publish a nothing;
                    // reject a void-mutation `Never`.
                    let value_ty = ctx.in_pure(|ctx| infer::infer(ctx, value));
                    reject_never_value(ctx, &unwrap_ref(&value_ty), value.range(), &b.name);
                    if let Expr::Ident { name, .. } = value
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
        }
        TopDecl::Let(l) => {
            let t = ctx.in_pure(|ctx| infer_let_init(ctx, l));
            if !reject_never_binding(ctx, l, &t) {
                check_let_type_annotation(ctx, l, &t);
            }
            record_single_output_alias(ctx, &l.binding, &l.value);
            bind_let(ctx, &l.binding, &t);
            // Same is_const/let split as the `Stmt::Let` arm (typecheck/stmt.rs):
            // `let` folds opportunistically, `const` must fold or it's an error.
            // A top-level constant is already recorded in `ctx.const_env` by the
            // whole-program fixpoint pass (`typecheck()`, run before any decl is
            // checked) — `scoped_consts` is empty at top level, so the success
            // arm below is a no-op there; this re-evaluation exists to surface
            // the WS046/WS047/WS048 a `const` binding failed with.
            if let LetBinding::Ident { name, .. } = &l.binding {
                let lookup = |n: &str| ctx.resolve_mod(n);
                let mut budget = crate::const_eval::Budget::default();
                let cx = ctx.const_ctx(Some(&lookup));
                match crate::const_eval::eval_expr(&l.value, &cx, &mut budget) {
                    Ok(lit) => {
                        if let Some(frame) = ctx.scoped_consts.last_mut() {
                            frame.insert(name.clone(), lit);
                        }
                        check_const_recorded(ctx, l);
                    }
                    Err(err) if l.is_const => ctx.emit(err.code(), err.message(), err.range.clone()),
                    Err(_) => {}
                }
            } else {
                // Same as the `Ident` arm above, generalized to every name a
                // destructuring binding introduces: evaluate the whole
                // right-hand side once, then split it via `bind_destructured`
                // (shared with `typecheck::stmt`'s block-scope site and
                // `const_eval::interp`'s const-mod-body site, so all three
                // agree on what a given binding form means). Top-level
                // `scoped_consts` is empty (see this arm's own doc comment
                // above), so the success path is a no-op here just like the
                // `Ident` arm's — this exists to surface the WS046/047/048 a
                // top-level destructuring `const` failed with.
                let lookup = |n: &str| ctx.resolve_mod(n);
                let mut budget = crate::const_eval::Budget::default();
                let cx = ctx.const_ctx(Some(&lookup));
                let result = crate::const_eval::eval_expr(&l.value, &cx, &mut budget)
                    .and_then(|lit| crate::const_eval::bind_destructured(&l.binding, lit));
                match result {
                    Ok(pairs) => {
                        if let Some(frame) = ctx.scoped_consts.last_mut() {
                            for (name, lit) in pairs {
                                frame.insert(name, lit);
                            }
                        }
                        check_const_recorded(ctx, l);
                    }
                    Err(err) if l.is_const => ctx.emit(err.code(), err.message(), err.range.clone()),
                    Err(_) => {}
                }
            }
        }
        // A top-level `let ... else` is checked exactly like the statement form
        // (`Stmt::LetElse`) - the `LetElse` struct is shared.
        TopDecl::LetElse(l) => check_stmt(ctx, &Stmt::LetElse(l.clone())),
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
            // the single trivial "no bindings" combo below.
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
            // writes straight into `ctx.diagnostics`.
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
                    // `name: const T` means "inside the body the parameter
                    // reads as [a compile-time constant]" (ast.rs's own doc
                    // comment on `Param::is_const`) — a mod is inlined per
                    // call site with the real argument value, so any call
                    // site's actual constant is always literal by the time it
                    // reaches a config port (see `lower/call/inline.rs`). This
                    // one-time, non-generic body-check can't know that value,
                    // so it seeds a type-shaped PLACEHOLDER into the body
                    // scope's `scoped_consts` frame instead — just enough for
                    // `expr_to_literal_in`/`ctx.const_lookup()`-based checks
                    // (e.g. `validate_scalar_config_arg`'s WS028) to see the
                    // param as "some constant" and accept it structurally.
                    // Its VALUE must never be used for anything beyond that
                    // presence check; lowering discards it entirely in favor
                    // of the real argument. Recording the name in
                    // `scoped_const_placeholders` alongside is what ENFORCES
                    // that: the `Stmt::If` const-elision arm evaluates against
                    // an environment with placeholders removed, so this zero
                    // can never select a branch (which lowering, holding the
                    // real argument, would then pick the other way — shipping
                    // a block this pass never checked).
                    if p.is_const {
                        if let Some(frame) = ctx.scoped_consts.last_mut() {
                            frame.insert(p.name.clone(), const_param_placeholder(&pt));
                        }
                        // `name: const T` IS a `const` declaration — mark it
                        // so `const_lookup_declared_only` sees it (though the
                        // placeholder removal just below already excludes it
                        // from the `if`-condition environment either way).
                        if let Some(frame) = ctx.scoped_const_declared.last_mut() {
                            frame.insert(p.name.clone());
                        }
                        if let Some(frame) = ctx.scoped_const_placeholders.last_mut() {
                            frame.insert(p.name.clone());
                        }
                    }
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
                                let mut consumed: crate::collections::HashSet<&str> =
                                    crate::collections::HashSet::default();
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
                                            consumed.insert(name.as_str());
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
                                            let rest_ty = if let Type::Record(rec_fields) = &pt {
                                                Type::Record(
                                                    rec_fields
                                                        .iter()
                                                        .filter(|(k, _)| {
                                                            !consumed.contains(k.as_str())
                                                        })
                                                        .cloned()
                                                        .collect(),
                                                )
                                            } else {
                                                Type::Any
                                            };
                                            ctx.scope.declare(
                                                name,
                                                SymbolInfo {
                                                    kind: SymbolKind::Param,
                                                    name: name.clone(),
                                                    ty: rest_ty,
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
                // A `...rest` variadic capture is bound once here with arity
                // unknown (the body is checked a single time; each call site
                // captures a different tuple). Type it `any` so a `...rest`
                // spread in the body forwards leniently (an arity-dynamic
                // spread the callee accepts) and any incidental `rest` use stays
                // quiet — lowering supplies the real per-call elements.
                if let Some(rest_name) = &c.rest {
                    ctx.scope.declare(
                        rest_name,
                        SymbolInfo {
                            kind: SymbolKind::Param,
                            name: rest_name.clone(),
                            ty: Type::Any,
                            decl_range: c.range.clone(),
                            signature: None,
                            event_data: None,
                        },
                    );
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
                        is_const: false,
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
        // A top-level `if let` is checked exactly like the statement form
        // (`Stmt::IfLet`) - the `IfLet` struct is shared.
        TopDecl::IfLet(i) => check_stmt(ctx, &Stmt::IfLet(i.clone())),
        TopDecl::Namespace(ns) => {
            // A namespaced (`import * as ns`) mod body references its sibling
            // constants and mods by BARE name, and those mods are inlined at
            // call sites in the importing module. Typecheck the bodies here in
            // an isolated scope (siblings registered as bare names) so operator
            // resolutions and expression types get recorded — otherwise the
            // inlined body's arithmetic and sibling calls lower to _Unsupported.
            // A namespace this module imported for ITSELF (`import * as D` in
            // the file we are namespacing) travels in beside us as its own
            // top-level namespace, and our members name it. It lives in an
            // OUTER frame, which the seal below hides, so re-declare it inside
            // this frame first. Matched by origin file, the same rule
            // `namespace_visible` applies at the use site, so this admits only
            // the namespaces our own members were written against - never the
            // importer's.
            let member_file = ns.decls.first().map(|d| d.range().file.clone());
            let traveling: Vec<(String, SymbolInfo)> = match &member_file {
                Some(file) => ctx
                    .scope
                    .iter_root()
                    .filter(|(_, s)| s.kind == SymbolKind::Namespace)
                    .filter(|(_, s)| s.decl_range.file == *file)
                    .map(|(n, s)| (n.to_string(), s.clone()))
                    .collect(),
                None => Vec::new(),
            };
            ctx.push_scope();
            for (name, info) in traveling {
                ctx.scope.declare(&name, info);
            }
            for d in &ns.decls {
                register_decl(ctx, d);
            }
            // Seal name resolution at this frame: a free name in a namespaced
            // body must bind to a SIBLING member (registered just above) or a
            // language global (events / math / builtin types resolve via the
            // catalog and `resolve_type_expr`, not this stack), never fall
            // through to the importer's same-named top-level state. Without
            // this, `lib` `on go { q = q + 1 }` with no `q`/`go` of its own
            // silently type-checks against — and wires to — main's `go`/`q`
            // (N1). Restored before the frame pops.
            let sealed_floor = ctx.scope.depth() - 1;
            let prev_floor = ctx.scope.set_floor(sealed_floor);
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
            // Handlers (and anon chips) in an imported module lower as part
            // of the importing program, so their bodies must be checked here too
            // — otherwise operators / sibling calls inside a namespaced
            // `on …` handler get no `op_resolutions` and lower to `_Unsupported`
            // (the same failure the value/chip checks above prevent). Checked
            // last, once every member is registered.
            for d in &ns.decls {
                if matches!(d, TopDecl::Handler(_) | TopDecl::AnonChip(_)) {
                    check_decl(ctx, d);
                }
            }
            ctx.scope.set_floor(prev_floor);
            ctx.pop_scope();
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) | TopDecl::Enum(_) => {}
    }
}
