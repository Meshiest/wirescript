//! Annotation-position type resolution and the predicates that guard what a
//! declaration may store.

use super::*;

// ---------- type expression resolution ----------
//
// `any` (in annotation position) maps onto the *wildcard* type
// (`Type::Opaque`, the same type `Opaque(...)` produces) rather than the
// internal `Type::Any` error-fallback — see `reject_any_storage` below for
// why: `any` can flow through ports/lets/params but can't back a
// var/array/buffer's storage gate. That mapping lives in the crate's single
// primitive-name table, `types::resolve::primitive`.

/// Resolve a type annotation against the current scope. Delegates to the
/// crate's single canonical resolver (`types::resolve::resolve_type`): the
/// scope's currently-visible `SymbolKind::Type` aliases are snapshotted (scope
/// doesn't mutate during a type expression's resolution, so this is
/// equivalent to the old per-`Name`-node live `ctx.scope.lookup`) and handed
/// in as `type_aliases`, and any WS002 the resolver raises is forwarded to
/// `ctx.diagnostics` — preserving both the alias-resolution and diagnostic
/// behavior of the resolver this replaced.
pub(super) fn resolve_type_expr(ctx: &mut TypeCheckCtx, t: &TypeExpr) -> Type {
    // Fast path: a primitive name needs no alias snapshot. Primitives always win
    // over aliases in `resolve_type` (it checks `primitive` before the alias
    // map), so this is behaviour-identical — but it skips building the full
    // in-scope alias map (an O(D) frame scan + deep `Type` clones) on the
    // overwhelmingly common case, where annotations are `int`/`float`/`bool`/…
    // Without it that snapshot was rebuilt for every annotation, incl. per
    // cartesian combo inside a generic body — an accidental O(D²).
    if let TypeExpr::Name { name, .. } = t
        && let Some(prim) = crate::types::resolve::primitive(name)
    {
        return prim;
    }
    let aliases = ctx.scope.type_aliases();
    let mut diags = Vec::new();
    let result = {
        let cx = crate::types::resolve::ResolveCtx {
            params: &[],
            type_aliases: &aliases,
            generic_aliases: &ctx.generic_type_aliases,
        };
        crate::types::resolve::resolve_type(t, &cx, &mut diags)
    };
    ctx.diagnostics.extend(diags);
    result
}

/// A type param's mask — the set of concrete types it may be inferred to.
/// Unbound (`T`, no `: Bound`) is the full `Variant` mask; a bound narrows it
/// (see `types::mono::mask_for_param`).
pub(super) fn type_param_mask(ctx: &TypeCheckCtx, tp: &TypeParam) -> Vec<Type> {
    // The bound → mask resolution lives in the shared `types::mono` module so
    // the lowering-side monomorphizer can rebuild the same masks.
    crate::types::mono::mask_for_param(tp.bound.as_ref(), &ctx.scope.type_aliases())
}

/// Cartesian product of per-type-param masks: each inner
/// `Vec<Type>` in `masks` is one type param's candidate concrete members;
/// the result has one `Vec<Type>` per combination, index-aligned with
/// `masks` (and so with the decl's `type_params`). Materializes the full
/// result — callers must cap the product size (e.g. against
/// `MAX_BODY_CHECK_COMBOS`) before calling this.
pub(super) fn cartesian_product(masks: &[Vec<Type>]) -> Vec<Vec<Type>> {
    let mut result: Vec<Vec<Type>> = vec![Vec::new()];
    for mask in masks {
        let mut next = Vec::with_capacity(result.len() * mask.len());
        for combo in &result {
            for ty in mask {
                let mut extended = combo.clone();
                extended.push(ty.clone());
                next.push(extended);
            }
        }
        result = next;
    }
    result
}

/// The type of a constant-literal expression, used to infer an unannotated
/// var's (or array var's element) type at registration, before the full type
/// map exists. Returns `None` for anything that isn't a compile-time literal.
pub(super) fn literal_expr_type(e: &Expr) -> Option<Type> {
    match e {
        Expr::IntLit { .. } => Some(Type::Int),
        Expr::AtomLit { .. } => Some(Type::Int),
        Expr::FloatLit { .. } => Some(Type::Float),
        Expr::BoolLit { .. } => Some(Type::Bool),
        Expr::StringLit { .. } | Expr::InterpLit { .. } => Some(Type::String),
        Expr::UnOp { op, operand, .. } if op == "-" => literal_expr_type(operand),
        // Constructor calls type by name alone — the value folds to a constant
        // later only if the args are constant, but the type holds regardless.
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident { name, .. } => match name.as_str() {
                "Vec" => Some(Type::Vector),
                "Rotation" => Some(Type::Rotator),
                "Color" | "ColorSRGB" | "ColorHex" => Some(Type::Color),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Types a Variable gate can hold as a wire variant — the safe targets when
/// refining an unannotated `var`'s placeholder `any` from its initializer.
pub(super) fn var_storable(t: &Type) -> bool {
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

pub(super) fn type_expr_range(t: &TypeExpr) -> SourceRange {
    t.range().clone()
}

/// `any` (`Type::Opaque`) is fine anywhere a value just flows through a wire
/// (ports, `let`s, mod/chip params) — but a var/array/buffer's storage gate
/// needs a concrete wire variant to hold, so an explicit `any` annotation
/// there is rejected (WS025). The same holds for the reference types
/// (`zone`/`teleport`/`prefab`): like a var ref, they can only be wired or
/// rerouted, never held in a storage gate. Only fires on an *explicit*
/// annotation: an unannotated declaration's inferred placeholder is
/// `Type::Any`, never `Type::Opaque`, so it never reaches this check.
pub(super) fn reject_any_storage(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange, what: &str) {
    if matches!(resolved, Type::Zone | Type::Teleport | Type::PrefabRef) {
        let n = crate::analysis::types::type_str(resolved);
        ctx.emit(
            "WS025",
            format!(
                "'{n}' is a reference and cannot be stored: {what} needs a concrete \
                 wire type — a '{n}' can only be wired or rerouted (like a var ref)"
            ),
            range,
        );
    } else if matches!(resolved, Type::Opaque) {
        ctx.emit(
            "WS025",
            format!(
                "'any' cannot be stored: {what} needs a concrete wire type \
                 (int/float/bool/string/vector/...)"
            ),
            range,
        );
    }
}

/// True if `t` is, or structurally contains, the wildcard `any` type
/// (`Type::Opaque`). A `*any`/`any[]`/`Map<_, any>`/`{ a: any }` annotation
/// wraps the `Opaque` under a `Ref`/`Array`/`Map`/`Record`/…, so a shallow
/// `matches!(t, Type::Opaque)` would miss it — recurse through every compound
/// so `warn_any_annotation` fires on the whole annotation regardless of nesting.
fn contains_opaque(t: &Type) -> bool {
    match t {
        Type::Opaque => true,
        Type::Ref(inner) | Type::Array(inner) => contains_opaque(inner),
        Type::Map(k, v) => contains_opaque(k) || contains_opaque(v),
        Type::Tuple(fields) | Type::Union(fields) => fields.iter().any(contains_opaque),
        Type::Record(fields) => fields.iter().any(|(_, ft)| contains_opaque(ft)),
        _ => false,
    }
}

/// True if `t` is, or structurally contains, a `Type::Param` — i.e. an
/// unresolved generic type parameter. A call whose param still carries one
/// (its `T` couldn't be inferred) is left to the WS033 inference diagnostics,
/// not the concrete arg-vs-param coercion check.
pub(crate) fn type_has_param(t: &Type) -> bool {
    match t {
        Type::Param(_) => true,
        Type::Ref(inner) | Type::Array(inner) => type_has_param(inner),
        Type::Map(k, v) => type_has_param(k) || type_has_param(v),
        Type::Tuple(fields) | Type::Union(fields) => fields.iter().any(type_has_param),
        Type::Record(fields) => fields.iter().any(|(_, ft)| type_has_param(ft)),
        _ => false,
    }
}

/// Warns (does not error) when a user's *non-storage* type annotation — an
/// `in`/`out` port, a mod/chip param or output, a handler's typed destructure
/// param, a `let`, or a `type X = ...` alias body — resolves to `any`
/// (`Type::Opaque`), including when the `any` is wrapped in a compound
/// (`*any`, `any[]`, `Map<_, any>`, `{ a: any }`, …). `any` still works there
/// (the value just flows through as a wildcard), but a generic type parameter
/// would let the type flow instead of erasing it. Storage positions
/// (var/array/buffer/map — see `reject_any_storage` above) are deliberately NOT
/// routed through this: an explicit `any` there is already a hard error
/// (WS025), and warning on top would double-fire the same annotation. Fires
/// once per annotation (at the top-level annotation range), not once per nested
/// `Opaque`.
pub(super) fn warn_any_annotation(ctx: &mut TypeCheckCtx, resolved: &Type, range: SourceRange) {
    if contains_opaque(resolved) {
        ctx.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            code: "WS032".into(),
            message: "'any' works, but prefer a generic type parameter here — it lets the type \
                       flow instead of erasing it"
                .into(),
            range,
        });
    }
}
