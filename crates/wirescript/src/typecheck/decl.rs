//! Pass 2: declaration checking.

use super::*;
use crate::types::mono::unwrap_ref;

// ---------- decl checking (2nd pass) ----------

/// A type-shaped stand-in for a `const` parameter's real (call-site-only)
/// value, used solely to seed the body frame's constants so structural constant checks
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
/// constant literal whose type coerces to the array's element type, or, for an
/// array of refs, arrays or maps, a ref to storage (`&x`, or an array or map by
/// name), which is baked into the save as a reference. Spreads and other
/// values are only meaningful when the array is built at runtime, so they're
/// rejected here with a pointer to assigning the array in an exec handler.
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
        } else if crate::lower::holds_refs(elem_ty) && crate::lower::ref_shaped(e) {
            if !coerces_at_a_baked_literal(&t, elem_ty) && !coerces_at_a_baked_literal(&unwrap_ref(&t), elem_ty) {
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
                    message: "asset / prefab references can't be inlined into an array initializer: \
                              they're object references wired in from their own brick. Build the array \
                              with `.push(...)` inside an exec handler instead."
                        .into(),
                    range: e.range().clone(),
                });
            } else if !matches!(elem_ty, Type::Any) && !coerces_at_a_baked_literal(&t, elem_ty) {
                // A string-format coercion is the case worth naming: it reads
                // as a legal conversion everywhere else in the language, and
                // the reason it isn't one here is invisible from the source.
                let why = if coerce(&t, elem_ty) == CoerceRule::ViaString {
                    "; an array initializer element can't be string-formatted, \
                     so write a matching-type literal"
                } else {
                    ""
                };
                ctx.emit(
                    "WS003",
                    format!(
                        "var element: expected {}, got {}{why}",
                        crate::analysis::type_str(elem_ty),
                        crate::analysis::type_str(&t)
                    ),
                    e.range().clone(),
                );
            } else {
                infer::int_constants_in_slot(ctx, e, elem_ty);
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
    if let Some(lit) = crate::lower::expr_to_literal_in(e, &ctx.const_lookup()) {
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
                R::UnnamedConstMod(_) => {}
                R::NotConstant(_)
                | R::NotAConstMod(_)
                | R::NestedConstModCall(_)
                | R::Unsupported(_)
                | R::UnsupportedMessage(_)
                | R::UnsupportedMethod(_)
                | R::Refused(_)
                | R::BudgetExceeded => ctx.emit(
                    "WS003",
                    "array initializer elements must be constant literals; assign the array inside an exec handler to build it from runtime values",
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

/// Whether `got` reaches `want` at a BAKED literal, where there is no gate to
/// run a coercion through.
///
/// Map literal entries and array literal elements are both written into their
/// storage gate's `InitialValue` as constants, so a `ViaString` coercion has
/// nowhere to put its `FormatText` gate: only the coercions that hold at the
/// literal itself (`Same`/`Coerce`) are valid. `Coerce` holds because lowering
/// rewrites the literal in the declared type's shape as it bakes
/// (`bake_string_bool`, `const_eval::scalar_in_type`); `ViaString` has no such
/// rewrite, so it is baked raw and then destroyed at emit -
/// `emit::variants` renders a non-`String` literal in a `string[]` as `""`,
/// which is how `var s: string[] = [1, 2]` shipped two empty strings.
///
/// Deliberately STRICTER than the `coerce_or_emit` sink assignment checking
/// uses, where a real wire exists for the gate to sit on.
pub(super) fn coerces_at_a_baked_literal(got: &Type, want: &Type) -> bool {
    matches!(coerce(got, want), CoerceRule::Same | CoerceRule::Coerce)
}

/// One side (key or value) of a map literal entry, checked against the map's
/// declared type for that side.
fn check_map_entry_side(
    ctx: &mut TypeCheckCtx,
    got: &Type,
    want: &Type,
    side: &str,
    at: &Expr,
) {
    if coerces_at_a_baked_literal(got, want) {
        return;
    }
    ctx.emit(
        "WS003",
        format!(
            "map literal {side}: expected {}, got {}. A map literal entry \
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
        infer::int_constants_in_slot(ctx, key, k);
        let vt = infer::infer(ctx, value);
        check_map_entry_side(ctx, &vt, v, "value", value);
        infer::int_constants_in_slot(ctx, value, v);
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
    // `let s: T` with no `= value` declares a signal when `T` is `exec`, also
    // through a `type` alias; any other type needs its value.
    if !l.has_initializer()
        && let Some(te @ TypeExpr::Name { name: type_name, .. }) = &l.typ
    {
        let before = ctx.diagnostics.len();
        let resolved = resolve_type_expr(ctx, te);
        ctx.diagnostics.truncate(before);
        let name = match &l.binding {
            LetBinding::Ident { name, .. } => name.as_str(),
            _ => "x",
        };
        if l.is_const {
            ctx.emit(
                "WSP001",
                format!(
                    "expected '=': `const {name}: {type_name}` needs a value (`const {name}: {type_name} = ..`). \
                     A `const` is a value known while compiling; a signal is declared with `let {name}: exec`"
                ),
                l.range.clone(),
            );
            return Type::Any;
        }
        if !matches!(resolved, Type::Exec) {
            ctx.emit(
                "WSP001",
                format!(
                    "expected '=': `let {name}: {type_name}` needs a value (`let {name}: {type_name} = ..`). \
                     Only a signal, typed `exec` or an alias of it, is declared without one"
                ),
                l.range.clone(),
            );
            return Type::Any;
        }
    }
    // `let s: exec` declares a new signal; the parser gives the bare form a
    // placeholder `0` at the declaration's own start. A written initializer
    // would be dropped, since the new signal fires only on `emit s`.
    if let (Some(TypeExpr::Name { name: type_name, .. }), LetBinding::Ident { name, .. }) = (&l.typ, &l.binding)
        && type_name == "exec"
        && !matches!(&l.value, Expr::IntLit { range, .. } if range.start.offset == l.range.start.offset)
    {
        ctx.emit(
            "WS101",
            format!(
                "`let {name}: exec` declares a new signal, which fires only on `emit {name}`, so it \
                 takes no initializer. To run code when another signal fires, write `on <signal> \
                 {{ .. }}` with that signal directly, or give it a second name with `let {name} = \
                 <signal>` (no annotation)."
            ),
            l.value.range().clone(),
        );
    }
    if l.is_const
        && let Expr::MapLit { entries, range } = &l.value
    {
        let t = check_const_map_literal(ctx, entries);
        // `infer` records every node it types; this replaces one `infer` call,
        // so it owes the same record.
        ctx.record_expr_type(range, &t);
        return t;
    }
    // Expose the let's annotation as the expected-type hint, so a generic enum
    // construction whose variant can't pin a type parameter takes it from the
    // annotation (`let n: Option<int> = Option.None`) instead of WS063 - the
    // same push `check` does for an annotated `out`. The annotation's own
    // value-vs-type check (`check_let_type_annotation`) re-resolves and owns the
    // authoritative diagnostics, so any this resolution emits is dropped to
    // avoid double-reporting a malformed annotation.
    let annotated = l.typ.as_ref().map(|te| {
        let before = ctx.diagnostics.len();
        let resolved = resolve_type_expr(ctx, te);
        ctx.diagnostics.truncate(before);
        resolved
    });
    // `null` takes the annotation's type, as it does in every other typed slot.
    if let (Expr::NullLit { .. }, Some(slot)) = (&l.value, &annotated) {
        return infer::check(ctx, &l.value, slot);
    }
    let prev = annotated.clone().map(|a| ctx.expected_ty.replace(a));
    let t = infer::infer(ctx, &l.value);
    if let Some(prev) = prev {
        ctx.expected_ty = prev;
    }
    if let Some(slot) = &annotated {
        infer::int_constants_in_slot(ctx, &l.value, slot);
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

/// A record used as var/array/map STORAGE decomposes into one backing gate per
/// field (see `lower::predeclare::declare_record_container`), so every leaf must
/// be a value that a gate can hold. A reference-only field (`ref T`/zone/
/// teleport/prefab) or an `exec` can't — reject it with WS049. Nested records
/// recurse; variant, array, and map fields are fine on a record VARIABLE.
///
/// A record stored as an array ELEMENT or map VALUE (`var hs: H[]`, and a
/// record variable's `pts: P[]` field) is one column per field, and a column
/// can't hold an array or map, so such a record may not contain one at any
/// depth.
pub(super) fn check_aggregate_record(ctx: &mut TypeCheckCtx, ty: &Type, range: &SourceRange) {
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
    /// The named parts of a record, or the index-named elements of a tuple.
    fn parts(t: &Type) -> Option<Vec<(String, Type)>> {
        match t {
            Type::Record(fields) => Some(fields.clone()),
            Type::Tuple(elems) => {
                Some(elems.iter().enumerate().map(|(i, t)| (i.to_string(), t.clone())).collect())
            }
            _ => None,
        }
    }
    /// The dotted path to the first array or map anywhere in `fields`.
    fn container_field(fields: &[(String, Type)]) -> Option<String> {
        fields.iter().find_map(|(name, ft)| match ft {
            Type::Array(_) | Type::Map(..) => Some(name.clone()),
            _ => parts(ft)
                .and_then(|sub| container_field(&sub))
                .map(|p| format!("{name}.{p}")),
        })
    }
    fn walk(ctx: &mut TypeCheckCtx, fields: &[(String, Type)], range: &SourceRange) {
        for (name, ft) in fields {
            match ft {
                Type::Record(sub) => walk(ctx, sub, range),
                Type::Tuple(_) => tuple_containers(ctx, ft, range),
                Type::Array(elem) | Type::Map(_, elem) => element(ctx, elem, range),
                _ => {
                    if let Some(reason) = reason(ft) {
                        ctx.emit(
                            "WS049",
                            format!(
                                "record field `{name}` is {reason}, which can't be stored. \
                                 Every field of a stored record needs a value a gate can \
                                 hold, such as a number, string, vector, entity or nested \
                                 record"
                            ),
                            range.clone(),
                        );
                    }
                }
            }
        }
    }
    /// A tuple is walked only for the containers it holds; its other elements
    /// keep their existing rules.
    fn tuple_containers(ctx: &mut TypeCheckCtx, t: &Type, range: &SourceRange) {
        match t {
            Type::Array(elem) | Type::Map(_, elem) => element(ctx, elem, range),
            Type::Tuple(elems) => {
                for e in elems {
                    tuple_containers(ctx, e, range);
                }
            }
            Type::Record(sub) => walk(ctx, sub, range),
            _ => {}
        }
    }
    fn element(ctx: &mut TypeCheckCtx, elem: &Type, range: &SourceRange) {
        let Some(fields) = parts(elem) else {
            return;
        };
        if let Some(path) = container_field(&fields) {
            let (part, whole) = match elem {
                Type::Tuple(_) => ("tuple element", "tuple"),
                _ => ("record field", "record"),
            };
            ctx.emit(
                "WS049",
                format!(
                    "{part} `{path}` is an array or map, and a {whole} stored in an array or \
                     map can't contain an array or map. Each part of the element is stored \
                     as its own column, and a column can't hold a container. Keep the inner \
                     collection in a separate `var` and store an index or key in the \
                     {whole} instead"
                ),
                range.clone(),
            );
        }
        match elem {
            Type::Record(fields) => walk(ctx, fields, range),
            _ => tuple_containers(ctx, elem, range),
        }
    }
    match ty {
        Type::Ref(inner) => check_aggregate_record(ctx, inner, range),
        Type::Array(elem) | Type::Map(_, elem) => element(ctx, elem, range),
        Type::Record(fields) => walk(ctx, fields, range),
        Type::Tuple(_) => tuple_containers(ctx, ty, range),
        _ => {}
    }
}

/// A value crossing a port (an `in`/`out` port or a chip parameter/output) is
/// one pin per record field, and a pin carries a single reference. An array or
/// map of records (or tuples or payload-carrying enums) is several columns with
/// no single reference, so neither it nor a record holding one at any depth can
/// cross (WS049). Inside a record, an array of ANY enum counts, since a record
/// variable stores that field as columns.
///
/// `boundary` is a top-level `in`/`out` port, where a payload-free enum crosses
/// as its tag; at a chip parameter or output, and nested in any port type, an
/// enum does not cross at all.
pub(super) fn check_port_record(
    ctx: &mut TypeCheckCtx,
    name: &str,
    ty: &Type,
    range: &SourceRange,
    boundary: bool,
) {
    // A payload-free enum array is one column, which a single pin carries.
    let enums = ctx.enum_defs.clone();
    let columnar = |t: &Type| match t {
        Type::Record(_) | Type::Tuple(_) => true,
        t @ Type::Enum { .. } => enums.of(t).is_some_and(|d| {
            d.variants
                .iter()
                .any(|v| !matches!(v.payload, crate::typecheck::enums::Payload::Unit))
        }),
        _ => false,
    };
    /// What an element of this kind is called in the message.
    fn kind(t: &Type) -> &'static str {
        match t {
            Type::Tuple(_) => "tuples",
            Type::Enum { .. } => "enum values",
            _ => "records",
        }
    }
    fn columnar_field(
        fields: &[(String, Type)],
        columnar: &dyn Fn(&Type) -> bool,
    ) -> Option<(String, &'static str)> {
        fields.iter().find_map(|(name, ft)| match unwrap_ref(ft) {
            Type::Array(e) | Type::Map(_, e) if columnar(&e) => Some((name.clone(), kind(&e))),
            Type::Record(sub) => {
                columnar_field(&sub, columnar).map(|(p, k)| (format!("{name}.{p}"), k))
            }
            _ => None,
        })
    }
    if let Some(en) = enum_crossing(ctx, ty, boundary) {
        ctx.emit(
            "WS049",
            format!(
                "`{name}` holds an enum value (`{en}`), so it can't cross this port: an enum is \
                 a tag plus a slot per payload field and a pin carries a single wire. Only a \
                 top-level `in`/`out` port of an enum with no payloads (a built-in game enum \
                 included) carries one, as its tag. Pass it to a `mod`, pass a chip a `*{en}` \
                 (which wires each part), or pass the tag and payload as plain values \
                 (`.Discriminant`, a `match` result) and build the enum inside"
            ),
            range.clone(),
        );
        return;
    }
    match unwrap_ref(ty) {
        Type::Array(e) | Type::Map(_, e) if columnar(&e) => ctx.emit(
            "WS049",
            format!(
                "`{name}` is an array or map of {}, so it can't cross a port. It is stored \
                 as one column per field, and a port pin carries a single reference. Pass it \
                 to a `mod` instead (a mod is inlined, so nothing crosses a port), or pass \
                 the columns the chip needs",
                kind(&e)
            ),
            range.clone(),
        ),
        Type::Record(fields) => {
            // Inside a record every enum counts: a record variable stores an enum
            // array field as columns even when it has no payload.
            let any_columnar = |t: &Type| {
                matches!(t, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. })
            };
            if let Some((path, k)) = columnar_field(&fields, &any_columnar) {
                ctx.emit(
                    "WS049",
                    format!(
                        "record field `{path}` is an array or map of {k}, so this record \
                         can't cross a port. A record variable stores it as one column per \
                         part, and a port pin carries a single reference. Pass the record to \
                         a `mod` instead (a mod is inlined, so nothing crosses a port), or \
                         pass the fields the chip needs"
                    ),
                    range.clone(),
                );
            }
        }
        _ => {}
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
                // A name the top level declares again: the initializer reads
                // the declaration before this one (see `scope::top`).
                let mut above = ctx.top_names.initializer_read(&v.name, &v.range).map(|at| ctx.top_view(at));
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
                        let t = infer::check_stored_value(ctx, init, &inner);
                        if let Some(view) = above.take() {
                            ctx.restore_top_view(view);
                        }
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
                            // A ref value is what the var holds, not a place to read.
                            let u = match &t {
                                Type::Ref(_) => t.clone(),
                                _ => unwrap_ref(&t).held_by_var(),
                            };
                            if super::resolve::unannotated_var_holds(&u) {
                                ctx.scope.set_type(&v.name, Type::Ref(Box::new(u)));
                            }
                        }
                    });
                }
                if let Some(view) = above {
                    ctx.restore_top_view(view);
                }
            }
            super::untyped_arrays::declare(ctx, v);
        }
        TopDecl::Buffer(b) => {
            // An annotated buffer's initializer is checked against the declared
            // type, so `buffer b: float = 5` buffers the float 5.
            let declared = match &b.typ {
                Some(_) => ctx.scope.lookup(&b.name).map(|s| s.ty.clone()),
                None => None,
            };
            ctx.in_pure(|ctx| {
                let before = ctx.diagnostics.len();
                let t = match &declared {
                    Some(d) => infer::check(ctx, &b.init, d),
                    None => infer::infer(ctx, &b.init),
                };
                for d in &mut ctx.diagnostics[before..] {
                    if d.code == "WS003" && d.range == *b.init.range() {
                        d.message = format!("buffer '{}' initializer: {}", b.name, d.message);
                    }
                }
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
        TopDecl::In(i) => {
            // Registered already; only the port's record shape is checked here.
            if let Some(ty) = ctx.scope.lookup(&i.name).map(|s| s.ty.clone()) {
                check_port_record(ctx, &i.name, &ty, &i.range, true);
            }
        }
        TopDecl::Out(b) => {
            if b.value.is_none()
                && let Some(te) = &b.typ
            {
                let before = ctx.diagnostics.len();
                let ty = resolve_type_expr(ctx, te);
                ctx.diagnostics.truncate(before);
                check_port_record(ctx, &b.name, &ty, &b.range, true);
            }
            if let Some(value) = &b.value {
                if let Some(te) = &b.typ {
                    let resolved = resolve_type_expr(ctx, te);
                    check_port_record(ctx, &b.name, &resolved, &b.range, true);
                    // A port cannot take a type parameter, so `any` here is no WS032.
                    // When out has ref type and value is a var, override to show "ref" in hover
                    if matches!(resolved, Type::Ref(_))
                        && let Expr::Ident { range, .. } = value
                    {
                        ctx.var_read_contexts
                            .remove(&(range.file.clone(), range.start.offset));
                    }
                    // An annotated out must accept its value: `check` coerces it
                    // into the port type (WS003 on a genuine mismatch; the string
                    // -> bool `!= ""` compare and friends pass) AND resolves a
                    // bidirectional literal like `null` to the port's type. A
                    // `*T` port carries a ref, so `out y: *int = &x` fits it and
                    // `out y: *int = x` (x's value) does not; an array or map
                    // ref is the container itself, so `*int[]` and `int[]` agree.
                    let value_ty = ctx.in_pure(|ctx| {
                        infer::check(ctx, value, &resolved.clone().container_ref_as_value())
                    });
                    // The value has to fit the PORT as well as this site's own
                    // annotation, which are different types whenever an earlier
                    // site typed the port. Annotation-versus-port is settled in
                    // registration for a top-level `out`, since every one of
                    // them is a registered site.
                    check_annotated_port_write(
                        ctx,
                        &b.name,
                        &resolved,
                        &value_ty,
                        value.range(),
                    );
                } else {
                    // An unannotated `out y = a.push(5)` would publish a nothing;
                    // reject a void-mutation `Never`.
                    let value_ty = ctx.in_pure(|ctx| infer::infer(ctx, value));
                    check_port_record(ctx, &b.name, &value_ty, &b.range, true);
                    // A `Never` value has already been reported as producing
                    // nothing, and the port check would say so a second time.
                    if !reject_never_value(ctx, &unwrap_ref(&value_ty), value.range(), &b.name) {
                        // This site declares no type of its own, so the value
                        // answers to whatever type the port already carries -
                        // an earlier handler-declared `out shared: int` binds
                        // a later `out shared = "x"` here.
                        check_port_write_value(ctx, &b.name, value, &value_ty);
                        port_write_literals(ctx, &b.name, value);
                    }
                    if let Expr::Ident { name, .. } = value
                        && let Some(sym) = ctx.scope.lookup(name)
                        && sym.kind == SymbolKind::Var
                    {
                        ctx.diagnostics.push(Diagnostic {
                            severity: Severity::Warning,
                            code: "WS017".into(),
                            message: {
                                let held = unwrap_ref(&sym.ty);
                                let held_str = crate::analysis::types::type_str(&held);
                                let o = &b.name;
                                // A var holding a ref has no ref to itself to
                                // offer: `**int` is WS086.
                                let forms = if matches!(held, Type::Ref(_) | Type::Brick) {
                                    format!("`out {o}: {held_str} = {name}` for the ref it holds")
                                } else {
                                    format!(
                                        "`out {o}: {held_str} = {name}` for the value, or \
                                         `out {o}: *{held_str} = &{name}` for a ref to it"
                                    )
                                };
                                format!("out '{o}' takes its type from var '{name}'. Add the type: {forms}")
                            },
                            range: b.range.clone(),
                        });
                    }
                }
            }
        }
        TopDecl::Let(l) => {
            let t = ctx.in_pure(|ctx| infer_let_init(ctx, l));
            super::stmt::reject_event_logic_trigger(ctx, l);
            if !reject_never_binding(ctx, l, &t) {
                check_let_type_annotation(ctx, l, &t);
            }
            record_single_output_alias(ctx, &l.binding, &l.value);
            record_let_ref_values(ctx, &l.binding, &l.value);
            // Same is_const/let split as the `Stmt::Let` arm (typecheck/stmt.rs):
            // `let` folds opportunistically, `const` must fold or it's an error.
            // A top-level constant is already recorded in the module's
            // constants by the whole-program fixpoint pass (`typecheck()`, run
            // before any decl is checked), and no frame is open at the top
            // level, so recording below is a no-op there; this evaluation
            // exists to surface the WS046/WS047/WS048 a `const` binding failed
            // with. Inside the frame `TopDecl::Namespace` checks a namespaced
            // module's members in, every name the binding introduces is
            // recorded. Evaluated before the binding exists, so the
            // initializer reads the names around it.
            let (evaluated, uncertified) = {
                let lookup = |n: &str| ctx.resolve_mod(n);
                let mut budget = crate::const_eval::Budget::default();
                let evaluated = crate::const_eval::eval_let_value(l, &ctx.const_ctx(Some(&lookup)), &mut budget)
                    .and_then(|lit| crate::const_eval::bind_destructured(&l.binding, lit));
                let certified = evaluated.is_ok()
                    && crate::const_eval::integer_ops_certified(&l.value, &ctx.certified_const_ctx(Some(&lookup)));
                (evaluated, !certified)
            };
            let bound = let_binding_type(ctx, l, &t);
            bind_let(ctx, &l.binding, &bound, &l.value);
            ctx.consts.bound_at_top(&l.range);
            match evaluated {
                Ok(pairs) => {
                    let marks = crate::const_scope::Marks { uncertified, ..Default::default() };
                    for (name, lit) in pairs {
                        ctx.consts.record(&name, lit, marks);
                    }
                    check_const_recorded(ctx, l);
                }
                Err(err) => {
                    for name in crate::const_eval::bound_names(&l.binding) {
                        ctx.consts.forget(&name);
                    }
                    if l.is_const && !err.reported_elsewhere() {
                        ctx.emit(err.code(), err.message(), err.range.clone());
                    }
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
            if let Some(rt) = &f.return_type {
                // Resolved where the signature is registered, which reports
                // a malformed one.
                let before = ctx.diagnostics.len();
                let rt = resolve_type_expr(ctx, rt);
                ctx.diagnostics.truncate(before);
                infer::int_constants_in_slot(ctx, &f.body, &rt);
            }
            ctx.pop_scope();
        }
        TopDecl::Chip(c) => {
            super::fn_decl::check_signature(ctx, c);
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
            // A call may demand an instantiation the combos below leave out;
            // it is checked later, in these same surroundings.
            if !c.type_params.is_empty() {
                super::instances::capture_env(ctx, c);
            }
            // The body being checked, for the storage it uses (WS091).
            super::shadow::open_body(ctx, c);
            let mut clean = Vec::with_capacity(combos.len());
            for combo in &combos {
                let before = ctx.diagnostics.len();
                check_chip_body(ctx, c, combo);
                clean.push(!ctx.diagnostics[before..].iter().any(|d| d.severity == Severity::Error));
                if multi_pass {
                    // Dedup by (code, start offset): the same body error
                    // surfaces once per mask member it's invalid for, but
                    // should be reported once, at the definition.
                    // The one naming a bound to narrow is kept whichever
                    // member reports first.
                    for d in ctx.diagnostics.split_off(before) {
                        if seen.insert((d.code.clone(), d.range.start.offset)) {
                            deduped.push(d);
                        } else if d.message.contains(crate::diagnostic::NARROW_THE_BOUND)
                            && let Some(kept) = deduped.iter_mut().find(|k| {
                                k.code == d.code
                                    && k.range.start.offset == d.range.start.offset
                                    && !k.message.contains(crate::diagnostic::NARROW_THE_BOUND)
                            })
                        {
                            *kept = d;
                        }
                    }
                }
            }
            ctx.active_combos = saved_active_combos;
            super::shadow::close_body(ctx, c);
            if multi_pass {
                narrow_to_clean_members(c, &combos, &clean, &mut deduped);
                ctx.diagnostics.extend(deduped);
            }
            check_every_path_returns(ctx, c);
        }
        TopDecl::AnonChip(ac) => {
            // Anon chip shares parent scope — NO scope push/pop.
            // Vars already pre-registered in pass 1; use check_decl (not
            // check_stmt) for them to avoid duplicate-declaration errors.
            check_anon_chip_stmts(ctx, &ac.body.stmts, true);
        }
        TopDecl::Event(e) => {
            // A captured body is checked as the handler it runs as.
            if let Some(h) = &e.captured {
                ctx.push_scope();
                bind_handler_trigger_params(ctx, h);
                check_handler_input_wires(ctx, h);
                ctx.in_exec(|ctx| check_block(ctx, &h.body));
                ctx.pop_scope();
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
            ctx.statement_match = matches!(s.expr, Expr::MatchExpr { .. });
            ctx.in_pure(|ctx| {
                infer::infer(ctx, &s.expr);
            });
            ctx.statement_match = false;
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
            // Taken from the per-FILE table, not the scope: the flat scope keeps
            // one symbol per name, so an alias shadowed by a same-named one in
            // the importer would be invisible here -- which is exactly the case
            // this frame exists to serve.
            let member_file = ns.decls.first().map(|d| d.range().file.clone());
            let traveling: Vec<(String, SymbolInfo)> = match &member_file {
                Some(file) => ctx
                    .namespaces_by_file
                    .get(file)
                    .map(|m| m.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|n| {
                        let info = SymbolInfo {
                            kind: SymbolKind::Namespace,
                            name: n.clone(),
                            ty: Type::Any,
                            decl_range: SourceRange {
                                file: file.clone(),
                                ..Default::default()
                            },
                            signature: None,
                            event_data: None,
                        };
                        (n, info)
                    })
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

/// A bound a `WS003` in `c`'s body suggests narrowing (see
/// `infer::report_record_as_one_value`) names the members whose check of the
/// body reported no error, so following it compiles: where none did, it
/// suggests none.
fn narrow_to_clean_members(
    c: &ChipDecl,
    combos: &[Vec<Type>],
    clean: &[bool],
    diags: &mut [Diagnostic],
) {
    let marker = crate::diagnostic::NARROW_THE_BOUND;
    for (k, tp) in c.type_params.iter().enumerate() {
        let written = format!("{marker}`<{}: ", tp.name);
        if !diags.iter().any(|d| d.message.contains(&written)) {
            continue;
        }
        let mut members: Vec<String> = Vec::new();
        for (combo, ok) in combos.iter().zip(clean) {
            let Some(t) = combo.get(k).filter(|_| *ok) else { continue };
            let t = crate::analysis::types::type_str(t);
            if !members.contains(&t) {
                members.push(t);
            }
        }
        for d in diags.iter_mut() {
            let Some(at) = d.message.find(&written) else { continue };
            let Some(end) = d.message[at..].find(">`").map(|e| at + e + 2) else { continue };
            let narrowed = if members.is_empty() {
                String::new()
            } else {
                format!("{written}{}>`", members.join(" | "))
            };
            let start = if members.is_empty() { d.message[..at].trim_end_matches("; ").len() } else { at };
            d.message.replace_range(start..end, &narrowed);
        }
    }
}

/// Check `c`'s body once, with its type parameters bound to `combo` (empty for
/// a non-generic `c`), under the enclosing body's substitution.
pub(super) fn check_chip_body(ctx: &mut TypeCheckCtx, c: &ChipDecl, combo: &[Type]) {
    // Concatenate onto the ENCLOSING substitution rather than
    // replacing it: a nested generic decl (`Stmt::ChipDecl`,
    // body-checked once per outer combo) must key its own body's
    // resolutions by the outer combo too, or every outer pass
    // would overwrite the same `(range, combo)` key with the
    // last-checked outer type. `combo` is empty for a non-generic
    // decl, so this leaves the key unchanged (inherits whatever
    // the enclosing decl set, `[]` at the top level).
    let saved_resolution_subst = std::mem::take(&mut ctx.resolution_subst);
    ctx.resolution_subst = saved_resolution_subst
        .iter()
        .cloned()
        .chain(combo.iter().cloned())
        .collect();
    let checked = (c.range.file.clone(), c.range.start.offset, c.range.end.offset, ctx.resolution_subst.clone());
    ctx.resolutions.checked_bodies.insert(checked);
    // The body reads the top level as its declaration sees it, wherever it
    // is checked from.
    let top_view = ctx.top_view(crate::scope::top::ReadAt::body(&c.range));
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
    let saved_unbounded = ctx.unbounded_type_params.len();
    let saved_unbounded_params = ctx.unbounded_typed_params.len();
    let saved_bounded = ctx.bounded_type_params.len();
    for (tp, ty) in c.type_params.iter().zip(combo.iter()) {
        if tp.bound.is_some() {
            let mask = super::resolve::type_param_mask(ctx, tp);
            ctx.bounded_type_params.push((tp.name.clone(), mask, ty.clone()));
        }
        if tp.bound.is_none() {
            ctx.unbounded_type_params.push(tp.name.clone());
            for p in &c.inputs {
                if super::enums::type_expr_mentions(&p.typ, &tp.name) {
                    ctx.unbounded_typed_params.push((p.range.clone(), tp.name.clone()));
                }
            }
        }
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
    for p in &c.inputs {
        let pt = resolve_type_expr(ctx, &p.typ);
        if !c.inline {
            check_port_record(ctx, &p.name, &pt, &p.range, false);
        }
        // `name: const T` means "inside the body the parameter
        // reads as [a compile-time constant]" (ast.rs's own doc
        // comment on `Param::is_const`) — a mod is inlined per
        // call site with the real argument value, so any call
        // site's actual constant is always literal by the time it
        // reaches a config port (see `lower/call/inline.rs`). This
        // one-time, non-generic body-check can't know that value,
        // so it records a type-shaped PLACEHOLDER in the body
        // frame's constants instead, just enough for
        // `expr_to_literal_in`/`ctx.const_lookup()`-based checks
        // (e.g. `validate_scalar_config_arg`'s WS028) to see the
        // param as "some constant" and accept it structurally.
        // Its VALUE must never be used for anything beyond that
        // presence check; lowering discards it entirely in favor
        // of the real argument. Recording it AS a placeholder
        // is what ENFORCES that: the `Stmt::If` const-elision arm evaluates against
        // an environment with placeholders removed, so this zero
        // can never select a branch (which lowering, holding the
        // real argument, would then pick the other way — shipping
        // a block this pass never checked).
        // `name: const T` IS a `const` declaration, recorded as one (though
        // being a placeholder already excludes it from the `if`-condition
        // environment either way).
        if p.is_const {
            let marks = crate::const_scope::Marks { declared: true, placeholder: true, uncertified: false };
            ctx.consts.record(&p.name, const_param_placeholder(&pt), marks);
        }
        // Decided on the type the parameter names as well as its spelling, so
        // an alias of a ref or an array binds as the type it stands for.
        let kind = if matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. })
            || matches!(pt, Type::Ref(_) | Type::Array(_))
        {
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
    // after the input-param loop, so this only drops the frame build's
    // own diagnostics; the caller's dedup window (which starts before
    // this body) keeps every real input-param and body diagnostic.
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
    if !c.inline {
        for (o, f) in c.outputs.iter().zip(&chip_outs) {
            check_port_record(ctx, &o.name, &f.ty, &o.range, false);
        }
    }
    ctx.out_ctx.push(chip_outs);
    let saved_inline = ctx.inline_body_depth;
    ctx.inline_body_depth = if c.inline { saved_inline + 1 } else { 0 };
    ctx.in_exec(|ctx| check_block(ctx, &c.body));
    ctx.inline_body_depth = saved_inline;
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
                    format!("output '{}' is never assigned; use `out {} = expr`, `emit {}`, or `return expr`", out.name, out.name, out.name),
                    out.range.clone(),
                );
            }
        }
    }
    ctx.out_ctx.pop();
    ctx.pop_scope();
    ctx.restore_top_view(top_view);
    ctx.unbounded_type_params.truncate(saved_unbounded);
    ctx.unbounded_typed_params.truncate(saved_unbounded_params);
    ctx.bounded_type_params.truncate(saved_bounded);
    ctx.resolution_subst = saved_resolution_subst;
}

/// WS090: a `mod`/`chip` with a value output that uses `return`, where some
/// path ends (a bare `return`, or the end of the body) before every value
/// output has a value on that path. With one `return <value>` in the body its
/// value is wired to the output unconditionally, and with several the output
/// keeps whatever the last call stored, so such a path outputs a value from
/// another path. An output is set on a path by `return <value>` or
/// `emit r = ...`; a continuous `out r = ...` (and any assignment inside a
/// nested handler, anonymous chip or loop body) counts as set on every path.
fn check_every_path_returns(ctx: &mut TypeCheckCtx, c: &ChipDecl) {
    // Written `exec`, or through a `type` alias of it.
    let is_exec = |t: &TypeExpr| match t {
        TypeExpr::Name { name, .. } => {
            name == "exec"
                || ctx
                    .scope
                    .lookup(name)
                    .is_some_and(|s| matches!(s.kind, SymbolKind::Type) && s.ty == Type::Exec)
        }
        _ => false,
    };
    let values: Vec<&str> = c
        .outputs
        .iter()
        .filter(|o| !is_exec(&o.typ))
        .map(|o| o.name.as_str())
        .collect();
    let Some(first_value) = c.outputs.iter().find(|o| !is_exec(&o.typ)) else {
        return;
    };
    if !block_has_return(&c.body) {
        return;
    }
    let mut set: crate::collections::HashSet<String> = crate::collections::HashSet::default();
    collect_off_path_assignments(&c.body, &values, &mut set);
    let mut bad: Option<SourceRange> = None;
    let open = walk_paths(&c.body, set, &values, &mut bad);
    if let Some(end) = open
        && !values.iter().all(|v| end.contains(*v))
    {
        bad.get_or_insert_with(|| first_value.range.clone());
    }
    if let Some(range) = bad {
        ctx.emit(
            "WS090",
            format!(
                "not every path through `{}` returns a value: a path that ends here \
                 (a bare `return`, or the end of the body) before `return <value>` or \
                 `emit` sets every value output would output a value from another path. \
                 Return a value on that path, or give the `if` an `else` that returns",
                c.name
            ),
            range,
        );
    }
}

/// Whether a `return` is reached on the body's own path, through branches and
/// `match` arms (not an anonymous chip, loop, nested handler or declaration).
fn block_has_return(block: &Block) -> bool {
    walk::any_below(
        walk::Node::Block(block),
        |f| matches!(f, walk::Flow::Seq | walk::Flow::Branch),
        &mut |n| matches!(n, walk::Node::Stmt(Stmt::Return { .. })),
    )
}

/// Output names set independently of any one path: every `out r = ...`, and
/// every `emit` inside a nested handler, anonymous chip or loop body.
fn collect_off_path_assignments(block: &Block, values: &[&str], set: &mut crate::collections::HashSet<String>) {
    walk::walk(walk::Node::Block(block), walk::Ctx::default(), &mut |n, cx| {
        match n {
            walk::Node::Stmt(Stmt::OutBinding(o)) if values.contains(&o.name.as_str()) => {
                set.insert(o.name.clone());
            }
            walk::Node::Stmt(Stmt::Emit(e))
                if (cx.handler || cx.anon_chip || cx.in_loop) && values.contains(&e.name.as_str()) =>
            {
                set.insert(e.name.clone());
            }
            walk::Node::Stmt(Stmt::ChipDecl(_)) => return false,
            _ => {}
        }
        true
    });
}

/// Walk `block` from `set` (the value outputs set so far on this path).
/// `None` when every path through it has ended; otherwise the outputs set on
/// every path that falls through. A bare `return` reached before every value
/// output is set records its range in `bad`.
fn walk_paths(
    block: &Block,
    mut set: crate::collections::HashSet<String>,
    values: &[&str],
    bad: &mut Option<SourceRange>,
) -> Option<crate::collections::HashSet<String>> {
    fn merge(a: Option<crate::collections::HashSet<String>>, b: Option<crate::collections::HashSet<String>>) -> Option<crate::collections::HashSet<String>> {
        match (a, b) {
            (None, x) | (x, None) => x,
            (Some(a), Some(b)) => Some(a.intersection(&b).cloned().collect()),
        }
    }
    for s in &block.stmts {
        // The block arms of a value `match` in the statement's own
        // expressions run before the statement does, each on its own path,
        // and leave through a `return` (WS095). A statement `match`'s arms
        // are its branches, walked below.
        let value_arms = match s {
            Stmt::ExprStmt(ExprStmt { expr: Expr::MatchExpr { scrutinee, .. }, .. }) => {
                walk::expr_arm_blocks(walk::Node::Expr(scrutinee))
            }
            _ => walk::expr_arm_blocks(walk::Node::Stmt(s)),
        };
        for b in value_arms {
            walk_paths(b, set.clone(), values, bad);
        }
        match s {
            Stmt::Return { value: Some(_), .. } => return None,
            Stmt::Return { value: None, range } => {
                if !values.iter().all(|v| set.contains(*v)) {
                    bad.get_or_insert_with(|| range.clone());
                }
                return None;
            }
            Stmt::Emit(e) => {
                if values.contains(&e.name.as_str()) {
                    set.insert(e.name.clone());
                }
                // A back-edge closes the exec chain.
                if e.buffer.is_some() {
                    return None;
                }
            }
            Stmt::If(i) => {
                let a = walk_paths(&i.then_block, set.clone(), values, bad);
                let b = match &i.else_block {
                    Some(eb) => walk_paths(eb, set.clone(), values, bad),
                    None => Some(set.clone()),
                };
                set = merge(a, b)?;
            }
            Stmt::IfLet(i) => {
                let a = walk_paths(&i.then_block, set.clone(), values, bad);
                let b = match &i.else_block {
                    Some(eb) => walk_paths(eb, set.clone(), values, bad),
                    None => Some(set.clone()),
                };
                set = merge(a, b)?;
            }
            Stmt::LetElse(l) => {
                walk_paths(&l.else_block, set.clone(), values, bad);
            }
            Stmt::ExprStmt(es) => {
                if let Expr::MatchExpr { arms, .. } = &es.expr
                    && !arms.is_empty()
                {
                    let mut out: Option<crate::collections::HashSet<String>> = None;
                    let mut first = true;
                    for a in arms {
                        let r = match &a.body {
                            MatchBody::Block(b) => walk_paths(b, set.clone(), values, bad),
                            MatchBody::Expr(_) => Some(set.clone()),
                        };
                        out = if first { r } else { merge(out, r) };
                        first = false;
                    }
                    set = out?;
                }
            }
            _ => {}
        }
    }
    Some(set)
}

/// The enum a port of type `ty` would carry that no pin can: any user or game
/// enum inside a record or tuple, or anywhere in a chip parameter or output. A
/// top-level (`boundary`) port of a payload-free enum is one wire, its tag, and
/// an array or map of them one column of tags; `*E` refs each slot.
fn enum_crossing(ctx: &TypeCheckCtx, ty: &Type, boundary: bool) -> Option<String> {
    match ty {
        Type::Ref(_) => None,
        Type::Enum { name, .. } => {
            let unit = ctx.enum_defs.of(ty).is_some_and(|d| {
                d.variants.iter().all(|v| matches!(v.payload, crate::typecheck::enums::Payload::Unit))
            });
            (!(boundary && unit)).then(|| name.clone())
        }
        Type::Record(fields) => fields.iter().find_map(|(_, t)| enum_crossing(ctx, t, false)),
        Type::Tuple(elems) => elems.iter().find_map(|t| enum_crossing(ctx, t, false)),
        // A top-level port array or map of payload-free enums is one column
        // of tags, which a pin carries as a container ref.
        Type::Array(e) => enum_crossing(ctx, e, boundary),
        Type::Map(k, v) => enum_crossing(ctx, k, false).or_else(|| enum_crossing(ctx, v, boundary)),
        _ => None,
    }
}
