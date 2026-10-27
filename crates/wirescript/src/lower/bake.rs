use super::*;

/// Fold a single array-literal element to a constant [`Literal`]. A spread has
/// no constant form and folds to `None`, leaving the initializer empty (the
/// type checker has already reported the error).
///
/// Evaluates in [`ConstScope::Pass1`], which resolves a callee via
/// [`resolve_mod_pass1`](LowerCtx::resolve_mod_pass1): baking runs in pass 1,
/// before `ctx.scope` has any chips registered.
pub(super) fn array_elem_literal(el: &ArrayElem, ctx: &LowerCtx<'_>) -> Option<Literal> {
    match el {
        ArrayElem::Item(e) => ctx.const_eval(e, ConstScope::Pass1).ok(),
        ArrayElem::Spread(_) => None,
    }
}

/// Fold a map-literal entry to a `(key, value)` literal pair coerced to the
/// declared `key_ty`/`val_ty`, or `None` if either side isn't a compile-time
/// constant. Evaluates in pass-1 scope, as [`array_elem_literal`] does.
fn map_entry_literal(
    e: &MapLitEntry,
    ctx: &LowerCtx<'_>,
    key_ty: &Type,
    val_ty: &Type,
) -> Option<(Literal, Literal)> {
    let key_lit = ctx.const_eval(&e.key, ConstScope::Pass1).ok()?;
    let val_lit = ctx.const_eval(&e.value, ConstScope::Pass1).ok()?;
    Some((
        crate::const_eval::scalar_in_type(key_lit, key_ty),
        crate::const_eval::scalar_in_type(val_lit, val_ty),
    ))
}

/// The storage gate a ref initializer element names: `&x` of a var, array or
/// map, or an array or map named bare (an array or map value is its own ref).
/// `None` for anything else, including a var that holds a ref: what it holds
/// is only known at run time.
pub(super) fn storage_ref_target(ctx: &LowerCtx<'_>, e: &Expr) -> Option<NodeId> {
    let (operand, explicit) = match e {
        Expr::RefOf { operand, .. } => (operand.as_ref(), true),
        other => (other, false),
    };
    let binding = match operand {
        Expr::Ident { name, .. } => ctx.scope.get(name),
        Expr::FieldAccess { .. } | Expr::TuplePick { .. } => resolve_field_chain(ctx, operand),
        _ => None,
    }?;
    match binding {
        Binding::Var(v) => match v.storage {
            VarStorage::Var if explicit && !matches!(v.inner_type, Type::Ref(_)) => Some(v.node_id),
            VarStorage::Array | VarStorage::Map => Some(v.node_id),
            VarStorage::Var | VarStorage::Buffer => None,
        },
        _ => None,
    }
}

/// The node id a `HANDLE_TARGET(S)` property stores for `target`.
pub(super) fn target_literal(target: NodeId) -> Literal {
    Literal::Int(i64::from(target.0))
}

/// The storage each element of an array initializer names, when every element
/// names one (see [`storage_ref_target`]), as a `HANDLE_TARGETS` list.
fn array_ref_targets(ctx: &LowerCtx<'_>, elements: &[ArrayElem]) -> Option<Literal> {
    let targets = elements
        .iter()
        .map(|el| match el {
            ArrayElem::Item(e) => storage_ref_target(ctx, e).map(target_literal),
            ArrayElem::Spread(_) => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!targets.is_empty()).then_some(Literal::Array(targets))
}

/// Bake an array initializer that isn't all constants as refs, into
/// `properties` as `HANDLE_TARGETS`. For an array of refs whose elements don't
/// all name storage declared above, `WS003` on the first that doesn't (the type
/// checker passed it as [`ref_shaped`]).
pub(super) fn bake_array_refs(
    ctx: &mut LowerCtx<'_>,
    properties: &mut HashMap<Sym, Literal>,
    elem_type: &Type,
    elements: &[ArrayElem],
) {
    if let Some(targets) = array_ref_targets(ctx, elements) {
        properties.insert(*sym::HANDLE_TARGETS, targets);
        return;
    }
    if !holds_refs(elem_type) {
        return;
    }
    let unresolved = elements.iter().find(|el| match el {
        ArrayElem::Item(e) => ref_shaped(e) && storage_ref_target(ctx, e).is_none(),
        ArrayElem::Spread(_) => false,
    });
    if let Some(el) = unresolved {
        ctx.error(
            "WS003",
            "a ref in an array initializer is `&x` of a var, or an array or map by name, declared above it",
            el.expr().range(),
        );
    }
}

/// The constant key and the storage the value names of each entry of a map
/// initializer, when every entry has both, as a `HANDLE_TARGETS` map.
fn map_ref_targets(ctx: &LowerCtx<'_>, entries: &[MapLitEntry], key_ty: &Type) -> Option<Literal> {
    let targets = entries
        .iter()
        .map(|en| {
            let key = ctx.const_eval(&en.key, ConstScope::Pass1).ok()?;
            let target = storage_ref_target(ctx, &en.value)?;
            Some((
                crate::const_eval::scalar_in_type(key, key_ty),
                target_literal(target),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    (!targets.is_empty()).then_some(Literal::Map(targets))
}

/// Why a map literal has no constant form.
pub(super) enum MapBakeMiss {
    /// Object/asset-family keys have no literal representation: `key_of`
    /// bakes every one as `Object(None)`, so two or more entries would collapse
    /// onto the same null key (a corrupt map with duplicate keys).
    ObjectKeys,
    /// Some entry doesn't fold to a constant.
    NonConstant,
}

/// The constant `(key, value)` pairs of a map literal, coerced to the declared
/// `Map<K, V>` (see [`map_entry_literal`]), or why it has none. The one bake
/// rule for a map var's initializer and a record's map field initializer.
pub(super) fn map_literal_pairs(
    ctx: &LowerCtx<'_>,
    entries: &[MapLitEntry],
    key_ty: &Type,
    val_ty: &Type,
) -> Result<Vec<(Literal, Literal)>, MapBakeMiss> {
    if matches!(key_ty, Type::Entity | Type::Character | Type::Controller) {
        return Err(MapBakeMiss::ObjectKeys);
    }
    let pairs: Vec<(Literal, Literal)> = entries
        .iter()
        .filter_map(|en| map_entry_literal(en, ctx, key_ty, val_ty))
        .collect();
    if pairs.len() == entries.len() {
        Ok(pairs)
    } else {
        Err(MapBakeMiss::NonConstant)
    }
}

/// Bake a constant map-literal initializer (`var m: Map<K, V> = {...}`) into
/// `properties` as an `InitialValue` (`Literal::Map`), or as `HANDLE_TARGETS`
/// when every value names storage. Shared by [`pre_declare_map`] and the
/// `Map<K, V>` branch of [`pre_declare_var`]. A literal that can't bake (see
/// [`map_literal_pairs`]) leaves the map empty with a warning.
pub(super) fn bake_map_init(
    ctx: &mut LowerCtx<'_>,
    properties: &mut HashMap<Sym, Literal>,
    name: &str,
    init: Option<&Expr>,
    key_ty: &Type,
    val_ty: &Type,
) {
    let Some(Expr::MapLit { entries, .. }) = init else {
        return;
    };
    if let Some(targets) = map_ref_targets(ctx, entries, key_ty) {
        properties.insert(*sym::HANDLE_TARGETS, targets);
        return;
    }
    match map_literal_pairs(ctx, entries, key_ty, val_ty) {
        Ok(pairs) => {
            properties.insert(*sym::INITIAL_VALUE, Literal::Map(pairs));
        }
        Err(MapBakeMiss::ObjectKeys) => ctx.warn(
            format!(
                "'{name}' initializer has object/asset-typed keys, which can't bake as literals, so it starts empty; assign entries inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        ),
        Err(MapBakeMiss::NonConstant) => ctx.warn(
            format!(
                "'{name}' initializer has non-constant entries, which are dropped here; assign them inside an exec handler"
            ),
            init.as_ref().unwrap().range(),
        ),
    }
}

/// A `var` initializer that can't bake into the gate as a constant: returns it
/// for diagnosis. `None` = no initializer, or it bakes fine. Must apply the
/// same checks `pre_declare_var` bakes with (e.g. [`array_elem_literal`]), or it
/// warns about an initializer that baked.
fn var_init_unbaked<'a>(v: &'a VarDecl, ctx: &LowerCtx<'_>) -> Option<&'a Expr> {
    let init = v.init.as_ref()?;
    let unbaked = match init {
        Expr::Array { elements, .. } => {
            elements
                .iter()
                .any(|el| array_elem_literal(el, ctx).is_none())
                && array_ref_targets(ctx, elements).is_none()
        }
        // `bake_map_init` owns every map-init diagnostic.
        Expr::MapLit { .. } => false,
        // A record literal bakes per field into each backing gate's
        // InitialValue (see `record_field_storage`).
        Expr::RecordLit { .. } => record_init_unbaked(ctx, init),
        // `null` bakes to the var's type default, which is its value.
        Expr::NullLit { .. } => false,
        // An enum-typed initializer bakes into `__disc` plus any foldable
        // payload slots (see `declare_enum_container`); the discriminant
        // always bakes.
        e if matches!(ctx.type_of(e), Type::Enum { .. }) => false,
        e => {
            expr_to_literal_in(e, &ctx.const_lookup()).is_none()
                && const_container_init(ctx, e).is_none()
                && !constant_record(ctx, e)
                && !bakes_as_ref(ctx, v, e)
        }
    };
    unbaked.then_some(init)
}

/// Whether `var v: *T = e` bakes `e` as a reference: `v` holds a ref and `e`
/// names storage (see [`storage_ref_target`]).
fn bakes_as_ref(ctx: &LowerCtx<'_>, v: &VarDecl, e: &Expr) -> bool {
    let holds_ref = matches!(ctx.scope.get(&v.name), Some(Binding::Var(r)) if matches!(r.inner_type, Type::Ref(_)) && r.storage == VarStorage::Var);
    holds_ref && storage_ref_target(ctx, e).is_some()
}

/// Whether `e` names a constant record (`CP`, `ns.LP`), which bakes field by
/// field (see `bake_const_record`).
fn constant_record(ctx: &LowerCtx<'_>, e: &Expr) -> bool {
    matches!(e, Expr::Ident { .. } | Expr::FieldAccess { .. })
        && matches!(fold_const(ctx, e), Some(Literal::Record(_)))
}

/// Whether some field of a record-literal initializer can't bake into its
/// backing gate: a scalar that doesn't fold, a container whose value isn't a
/// fully constant literal, a shorthand or spread (neither bakes). Nested record
/// literals recurse, as `record_field_storage` does.
fn record_init_unbaked(ctx: &LowerCtx<'_>, e: &Expr) -> bool {
    let Expr::RecordLit { fields, .. } = e else {
        return false;
    };
    fields.iter().any(|f| match f {
        RecordLitField::Named { value, .. } => field_init_unbaked(ctx, value),
        RecordLitField::Shorthand { .. } | RecordLitField::Spread { .. } => true,
    })
}

fn field_init_unbaked(ctx: &LowerCtx<'_>, value: &Expr) -> bool {
    match value {
        Expr::RecordLit { .. } => record_init_unbaked(ctx, value),
        Expr::Array { elements, .. } => elements.iter().any(|el| match el {
            ArrayElem::Item(row @ Expr::RecordLit { .. }) => record_init_unbaked(ctx, row),
            ArrayElem::Item(_) => array_elem_literal(el, ctx).is_none(),
            ArrayElem::Spread(_) => true,
        }),
        Expr::MapLit { entries, .. } => {
            let Type::Map(k, v) = ctx.type_of(value) else {
                return true;
            };
            // A record-valued map is columns, baked per column from each
            // entry's record literal; any other map bakes by the one map rule.
            if entries
                .iter()
                .any(|en| matches!(en.value, Expr::RecordLit { .. }))
            {
                entries.iter().any(|en| match &en.value {
                    row @ Expr::RecordLit { .. } => {
                        fold_const(ctx, &en.key).is_none() || record_init_unbaked(ctx, row)
                    }
                    _ => true,
                })
            } else {
                map_literal_pairs(ctx, entries, &k, &v).is_err()
            }
        }
        Expr::NullLit { .. } => false,
        e if matches!(ctx.type_of(e), Type::Enum { .. }) => false,
        e => {
            expr_to_literal_in(e, &ctx.const_lookup()).is_none()
                && const_container_init(ctx, e).is_none()
                && !constant_record(ctx, e)
        }
    }
}

/// Warn when a `var` initializer is dropped: it can't bake into the Variable
/// gate, and no exec-context reset applies it (the var is in pure position, or
/// `static`), so the var starts at its type default. `skip_array_inits` skips
/// top-level array literals the type checker already errors on.
pub(super) fn warn_unbaked_var_init(ctx: &mut LowerCtx<'_>, v: &VarDecl, skip_array_inits: bool) {
    let Some(init) = var_init_unbaked(v, ctx) else {
        return;
    };
    if skip_array_inits && matches!(init, Expr::Array { .. }) {
        return;
    }
    let holds_ref = v
        .typ
        .as_ref()
        .is_some_and(|te| matches!(ctx.declared_shape(None, te), TypeExpr::Ref { .. }))
        || matches!(ctx.type_of(init), Type::Ref(_));
    let msg = if holds_ref {
        format!(
            "'var {}' holds a ref, and only a ref to a var, array or map (`&x`) is baked. This \
             initializer is dropped, so the var starts as a null ref, which halts the exec chain \
             when used; assign the ref inside an exec handler instead",
            v.shown_name()
        )
    } else if v.is_static {
        format!(
            "'static var {}' initializer must be a compile-time constant: this value is dropped and the var starts at its type default",
            v.shown_name()
        )
    } else {
        format!(
            "'var {}' initializer is not a compile-time constant. Outside an exec context it is dropped and the var starts at its type default; assign the value inside an exec handler instead",
            v.shown_name()
        )
    };
    ctx.warn(msg, init.range());
}

/// The value expression of field `name` in a record literal `row`; `None` if
/// `row` isn't a record literal or lacks the field (shorthand `{ x }` has no
/// expression to fold, so the field falls back to its default).
fn record_lit_field_expr<'a>(row: &'a Expr, name: &str) -> Option<&'a Expr> {
    let Expr::RecordLit { fields, .. } = row else {
        return None;
    };
    fields.iter().find_map(|f| match f {
        RecordLitField::Named { name: n, value, .. } if n == name => Some(value),
        _ => None,
    })
}

/// Fold an expression to a constant [`Literal`] through the certified evaluator,
/// resolving `const mod` calls via the pass-1 lookup (same seam as
/// [`array_elem_literal`]).
pub(super) fn fold_const(ctx: &LowerCtx<'_>, e: &Expr) -> Option<Literal> {
    ctx.const_eval(e, ConstScope::Pass1).ok()
}

/// Set a container gate's `InitialValue` property.
fn set_initial_value(ctx: &mut LowerCtx<'_>, node_id: NodeId, value: Literal) {
    if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(*sym::INITIAL_VALUE, value);
    }
}

/// A constant row of a per-column container, as written: the value expression,
/// and for a map entry the key expression. An array element has no key.
pub(super) type RowExprs<'a> = (Option<&'a Expr>, &'a Expr);

/// A constant row already folded to its columns (an enum value folds to
/// `__disc` plus its variant's payload columns), with the folded key of a map
/// entry. An array element has no key.
type FoldedRow = (Option<Literal>, Vec<(String, Literal)>);

/// Bake the per-field columns of record array or map rows into each backing
/// container's `InitialValue`: `var foos: Foo[] = [{foo:1}, {foo:3}]` bakes
/// `[1, 3]` into the `foo` array, and `var m: Map<int, Point> = { 0 => {x:1,
/// y:2} }` bakes `{0: 1}` into the `x` map and `{0: 2}` into `y` (recursing
/// for nested record fields). A field whose column isn't fully constant starts
/// empty, since a pure decl can't emit runtime writes.
pub(super) fn bake_record_columns(ctx: &mut LowerCtx<'_>, rec: &Fields, rows: &[RowExprs<'_>]) {
    for (fkey, binding) in rec {
        let fname = resolve(*fkey);
        let cols: Option<Vec<RowExprs<'_>>> = rows
            .iter()
            .map(|(k, r)| record_lit_field_expr(r, fname).map(|fe| (*k, fe)))
            .collect();
        let Some(cols) = cols else {
            continue;
        };
        match binding {
            Binding::Record(sub) => bake_rows(ctx, sub, &cols),
            Binding::Var(v) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let node_id = v.node_id;
                let lits: Vec<Literal> = cols
                    .iter()
                    .filter_map(|(_, e)| fold_const(ctx, e))
                    .map(|l| bake_string_bool(l, &elem))
                    .collect();
                if lits.len() == cols.len() {
                    set_initial_value(ctx, node_id, Literal::Array(lits));
                }
            }
            Binding::Var(v) if v.storage == VarStorage::Map => {
                let (key_ty, val_ty) = match &v.inner_type {
                    Type::Map(k, vv) => ((**k).clone(), (**vv).clone()),
                    _ => (Type::Any, Type::Any),
                };
                let node_id = v.node_id;
                let pairs: Vec<(Literal, Literal)> = cols
                    .iter()
                    .filter_map(|(k, ve)| {
                        Some((
                            crate::const_eval::scalar_in_type(fold_const(ctx, (*k)?)?, &key_ty),
                            crate::const_eval::scalar_in_type(fold_const(ctx, ve)?, &val_ty),
                        ))
                    })
                    .collect();
                if pairs.len() == cols.len() {
                    set_initial_value(ctx, node_id, Literal::Map(pairs));
                }
            }
            _ => {}
        }
    }
}

/// Bake constant rows already folded to their columns into each backing
/// array's or map's `InitialValue` (under the entry's key, for a map); a
/// column a row has no value for (another variant's payload) takes its type's
/// zero.
fn bake_folded_columns(ctx: &mut LowerCtx<'_>, rec: &Fields, rows: &[FoldedRow]) {
    for (fkey, binding) in rec {
        let fname = resolve(*fkey);
        let field = |row: &Vec<(String, Literal)>| {
            row.iter()
                .find(|(n, _)| *n == fname)
                .map(|(_, l)| l.clone())
        };
        match binding {
            Binding::Record(sub) => {
                let sub_rows: Vec<FoldedRow> = rows
                    .iter()
                    .map(|(k, r)| {
                        let fields = match field(r) {
                            Some(Literal::Record(fields)) => fields,
                            _ => Vec::new(),
                        };
                        (k.clone(), fields)
                    })
                    .collect();
                bake_folded_columns(ctx, sub, &sub_rows);
            }
            Binding::Var(v) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let node_id = v.node_id;
                let lits: Option<Vec<Literal>> = rows
                    .iter()
                    .map(|(_, r)| {
                        field(r)
                            .or_else(|| default_literal_for_var_type(&elem))
                            .map(|l| bake_string_bool(l, &elem))
                    })
                    .collect();
                if let Some(lits) = lits {
                    set_initial_value(ctx, node_id, Literal::Array(lits));
                }
            }
            Binding::Var(v) if v.storage == VarStorage::Map => {
                let Type::Map(key_ty, val_ty) = &v.inner_type else {
                    continue;
                };
                let node_id = v.node_id;
                let pairs: Option<Vec<(Literal, Literal)>> = rows
                    .iter()
                    .map(|(k, r)| {
                        let value = field(r).or_else(|| default_literal_for_var_type(val_ty))?;
                        Some((
                            crate::const_eval::scalar_in_type(k.clone()?, key_ty),
                            crate::const_eval::scalar_in_type(value, val_ty),
                        ))
                    })
                    .collect();
                if let Some(pairs) = pairs {
                    set_initial_value(ctx, node_id, Literal::Map(pairs));
                }
            }
            _ => {}
        }
    }
}

/// Bake constant rows into per-column arrays or maps: record literals field by
/// field, and enum values (`[Hue.Red, S.B(3)]`, `{ "a": Hue.Blue }`) by
/// folding each to its tag and payload columns.
pub(super) fn bake_rows(ctx: &mut LowerCtx<'_>, rec: &Fields, rows: &[RowExprs<'_>]) {
    if rows
        .iter()
        .all(|(_, v)| matches!(v, Expr::RecordLit { .. }))
    {
        bake_record_columns(ctx, rec, rows);
        return;
    }
    let folded: Option<Vec<FoldedRow>> = rows
        .iter()
        .map(|(k, v)| {
            let key = k.map(|k| fold_const(ctx, k));
            match (key, fold_const(ctx, v)) {
                (None, Some(Literal::Record(fields))) => Some((None, fields)),
                (Some(Some(k)), Some(Literal::Record(fields))) => Some((Some(k), fields)),
                _ => None,
            }
        })
        .collect();
    if let Some(folded) = folded {
        bake_folded_columns(ctx, rec, &folded);
    }
}

/// The value of a container initializer that names a constant rather than
/// spelling a literal (`var xs: int[] = IS`, `var m: Map<string, int> = M`):
/// the folded array or map. `None` for a literal, which its own path bakes.
pub(super) fn const_container_init(ctx: &LowerCtx<'_>, init: &Expr) -> Option<Literal> {
    if matches!(init, Expr::Array { .. } | Expr::MapLit { .. }) {
        return None;
    }
    fold_const(ctx, init).filter(|l| matches!(l, Literal::Array(_) | Literal::Map(_)))
}

/// Bake a constant record initializer named rather than spelled (`var p: P =
/// CP`, `CP` a constant record) into the per-field variables bound as `name`,
/// as a record literal's fields bake: a nested record and an enum field
/// (its tag and payload slots) field by field, an array or map field whole.
pub(super) fn bake_const_record(ctx: &mut LowerCtx<'_>, name: &str, init: Option<&Expr>) {
    let Some(init) = init.filter(|e| !matches!(e, Expr::RecordLit { .. })) else {
        return;
    };
    let Some(lit @ Literal::Record(_)) = fold_const(ctx, init) else {
        return;
    };
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    bake_record_var_fields(ctx, &rec, &lit);
}

pub(super) fn bake_record_var_fields(ctx: &mut LowerCtx<'_>, rec: &Fields, lit: &Literal) {
    let Literal::Record(values) = lit else { return };
    for (field, value) in values {
        match rec.get(&intern(field)) {
            Some(Binding::Record(sub)) => bake_record_var_fields(ctx, sub, value),
            Some(Binding::Var(v)) => {
                let baked = match (v.storage, value) {
                    (VarStorage::Array, Literal::Array(items)) => Literal::Array(
                        items
                            .iter()
                            .map(|l| crate::const_eval::scalar_in_type(l.clone(), &v.inner_type))
                            .collect(),
                    ),
                    (VarStorage::Map, Literal::Map(pairs)) => match &v.inner_type {
                        Type::Map(k, t) => {
                            let coerce = |(a, b): &(Literal, Literal)| {
                                (
                                    crate::const_eval::scalar_in_type(a.clone(), k),
                                    crate::const_eval::scalar_in_type(b.clone(), t),
                                )
                            };
                            Literal::Map(pairs.iter().map(coerce).collect())
                        }
                        _ => continue,
                    },
                    (VarStorage::Var, value) => {
                        crate::const_eval::scalar_in_type(value.clone(), &v.inner_type)
                    }
                    _ => continue,
                };
                set_initial_value(ctx, v.node_id, baked);
            }
            _ => {}
        }
    }
}

/// Bake a constant initializer named rather than spelled (see
/// [`const_container_init`]) into the per-column arrays or maps of the
/// record or enum container bound as `name`.
pub(super) fn bake_const_columns(ctx: &mut LowerCtx<'_>, name: &str, init: Option<&Expr>) {
    let Some(lit) = init.and_then(|e| const_container_init(ctx, e)) else {
        return;
    };
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    bake_constant_columns(ctx, &rec, lit);
}

/// Bake a constant array or map of records or enums, `lit`, into the
/// per-column arrays or maps `rec`.
pub(super) fn bake_constant_columns(ctx: &mut LowerCtx<'_>, rec: &Fields, lit: Literal) {
    let row = |l: &Literal| match l {
        Literal::Record(fields) => Some(fields.clone()),
        _ => None,
    };
    match lit {
        Literal::Array(rows) => {
            if let Some(rows) = rows
                .iter()
                .map(|l| row(l).map(|r| (None, r)))
                .collect::<Option<Vec<_>>>()
            {
                bake_folded_columns(ctx, rec, &rows);
            }
        }
        Literal::Map(pairs) => {
            if let Some(rows) = pairs
                .iter()
                .map(|(k, v)| row(v).map(|r| (Some(k.clone()), r)))
                .collect::<Option<Vec<_>>>()
            {
                bake_folded_columns(ctx, rec, &rows);
            }
        }
        _ => {}
    }
}

/// Bake a record ARRAY initializer (`= [{..}, ..]`) into the per-field arrays,
/// once the `Binding::Record` for `name` already exists. Spreads have no constant
/// form, so an initializer containing one bakes nothing (starts empty).
pub(super) fn bake_record_array_init(ctx: &mut LowerCtx<'_>, name: &str, elements: &[ArrayElem]) {
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    let rows: Vec<RowExprs<'_>> = elements
        .iter()
        .filter_map(|el| match el {
            ArrayElem::Item(e) => Some((None, e)),
            ArrayElem::Spread(_) => None,
        })
        .collect();
    if rows.len() != elements.len() {
        return;
    }
    bake_rows(ctx, &rec, &rows);
}

/// Bake a record MAP initializer (`= { k => {..} }`) into the per-field maps.
pub(super) fn bake_record_map_init(ctx: &mut LowerCtx<'_>, name: &str, entries: &[MapLitEntry]) {
    let Some(Binding::Record(rec)) = ctx.scope.get(name).cloned() else {
        return;
    };
    let rows: Vec<RowExprs<'_>> = entries
        .iter()
        .map(|en| (Some(&en.key), &en.value))
        .collect();
    bake_rows(ctx, &rec, &rows);
}
