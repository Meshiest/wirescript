use super::*;
use crate::ast::walk;

// ---------- expressions ----------

pub(super) fn lower_expr(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    // Enum CONSTRUCTION (unit `Enum.Variant`, positional `Enum.Variant(args)`,
    // or named `Enum.Variant { fields }`) has no single wire - like a record
    // literal, it lowers to a `Binding::Record` (see `try_lower_enum_ctor`),
    // not a `PortRef`. Checked once, ahead of the match below, so all three
    // AST shapes (`FieldAccess`/`Call`/`VariantCtor`) route through the same
    // check instead of duplicating the enum-name/shadow/registry guard in
    // each arm.
    if let Some(port) = try_lower_enum_ctor(ctx, e) {
        return port;
    }
    // `Enum.FromInt(n)` (tag-only construction) and `<enum value>.ToInt()`
    // (`.Discriminant` alias) are checked next, ahead of the ordinary call
    // lowering that would treat their `FromInt`/`ToInt` method names as unknown
    // and synthesise an `_Unsupported` placeholder.
    if let Some(port) = try_lower_enum_from_int(ctx, e) {
        return port;
    }
    if let Some(port) = try_lower_enum_to_int(ctx, e) {
        return port;
    }
    // `EnumToInt(value)` / `IntToEnum(value, wrap?)`: the gate-backed
    // twins of `.ToInt()` / `Enum.FromInt(n)`. Checked ahead of the ordinary
    // call lowering so a compile-time-known value folds gate-free while a
    // runtime value routes through the real game gate (see each function's own
    // doc comment).
    if let Some(port) = try_lower_enum_to_integer(ctx, e) {
        return port;
    }
    if let Some(port) = try_lower_integer_to_enum(ctx, e) {
        return port;
    }
    if let Some(lit) = int_constant_expr_as_float(ctx, e) {
        return literal_node(ctx, e, Type::Float, lit);
    }
    if let Some(lit) = float_constant_expr_as_int(ctx, e) {
        return literal_node(ctx, e, Type::Int, lit);
    }
    match e {
        Expr::IntLit { value, .. } => {
            let (ty, lit) = constant_literal(ctx, e, Literal::Int(*value));
            literal_node(ctx, e, ty, lit)
        }
        Expr::AtomLit { value, .. } => literal_node(ctx, e, Type::Int, Literal::Int(*value)),
        Expr::FloatLit { value, .. } => literal_node(ctx, e, Type::Float, Literal::Float(*value)),
        Expr::BoolLit { value, .. } if ctx.type_of(e) == Type::Int => {
            literal_node(ctx, e, Type::Int, Literal::Int(i64::from(*value)))
        }
        Expr::BoolLit { value, .. } => literal_node(ctx, e, Type::Bool, Literal::Bool(*value)),
        // `null` lowers to its resolved type's zero/default (typecheck recorded
        // the type via `check_null`): an unset object, `0`, `false`, `""`, …
        Expr::NullLit { .. } => {
            let t = unwrap_ref(&ctx.type_of(e));
            let lit = default_literal_for_var_type(&t).unwrap_or(Literal::Float(0.0));
            literal_node(ctx, e, t, lit)
        }
        // `emit` in value position is the exec chain point the handler has
        // reached. Pure context has none, which typecheck already reported as
        // WS007; the placeholder only gives that program's consumer a port to
        // wire, without a second (generic) diagnostic on top.
        Expr::CurrentExec { range } => match ctx.current_exec {
            Some(port) => port,
            None => synthesise_unsupported_no_warn(ctx, range),
        },
        Expr::StringLit { value, .. } => {
            literal_node(ctx, e, Type::String, Literal::String(value.clone()))
        }
        Expr::InterpLit { parts, range } => lower_interp(ctx, parts, range),
        // `$Type/Name` as a value (e.g. a compare operand `weapon == $Item/Foo`)
        // materializes into the matching `*Reference` gate: the asset is held in
        // that gate's class/object `Asset` field (where an asset CAN be inlined)
        // and the gate outputs it as an `entity` wire. Assets can't be inlined
        // into arbitrary wire-variant ports (Compare `InputB`), so this wire is
        // how they reach such consumers. Typed `entity` (see typecheck).
        // No gate takes a component type until the component surface exists.
        // A save stores one as the external asset `BrickComponentTypeBase/<class>`.
        Expr::AssetRef { asset_type, range, .. } if asset_type == crate::analysis::assets::COMPONENT => {
            synthesise_unsupported_range(ctx, "a component type reference, which no gate takes yet", range)
        }
        Expr::AssetRef { asset_type, asset_name, range } => {
            let mut props = HashMap::default();
            props.insert(
                intern_static("Asset"),
                Literal::Asset {
                    asset_type: asset_type.clone(),
                    asset_name: asset_name.clone(),
                },
            );
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class: asset_reference_gate(asset_type),
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![],
                    outputs: vec![PortSpec::new(*sym::VALUE, Type::Entity)],
                },
                properties: props,
                ..Default::default()
            });
            node_id.port(WirePort::Value)
        }
        Expr::Ident { name, range } => {
            if name == "_" {
                if let Some(port) = ctx.await_armed_port {
                    return port;
                }
            }
            let port = lower_ident(ctx, name, range);
            retype_int_constant(ctx, e, port)
        }
        Expr::BinOp { .. } => lower_binop(ctx, e),
        Expr::UnOp { .. } => lower_unop(ctx, e),
        Expr::Deref { operand, range } => {
            // `*e` on a ref VALUE: read through the wire that carries it.
            if matches!(ctx.type_of(operand), Type::Ref(_)) {
                let target = ctx.type_of(e);
                let ref_port = lower_expr(ctx, operand);
                let Some(exec) = ctx.current_exec else {
                    ctx.error(
                        "WS006",
                        "reading through a ref needs an exec context: `*` is a Var_Get",
                        range,
                    );
                    return synthesise_unsupported_no_warn(ctx, range);
                };
                return chain_var_get_through(ctx, ref_port, &target, exec, range)
                    .port(WirePort::Value);
            }
            if let Expr::Ident { name, .. } = operand.as_ref()
                && let Some(var_rec) = ctx.lookup_var(name).cloned()
            {
                if let Some(exec) = ctx.current_exec {
                    let get_id =
                        chain_var_get(ctx, &var_rec, exec, range, HashMap::default(), None);
                    return get_id.port(WirePort::Value);
                }
                ctx.warn(
                    format!(
                        "'*{}' deref requires exec context; use .Value for pure reads",
                        name
                    ),
                    range,
                );
                return var_rec.node_id.port(WirePort::Value);
            }
            lower_expr(ctx, operand)
        }
        Expr::RefOf { operand, range } => lower_ref_of(ctx, operand, range),
        Expr::TuplePick { range, .. } => {
            if let Some(binding) = resolve_field_chain(ctx, e).cloned()
                // A gate read held as its outputs is its first output here.
                && let binding = match &binding {
                    Binding::Record(rec) => {
                        crate::lower::context::held_output(ctx, rec, range, crate::lower::context::OutputPick::First)
                            .unwrap_or(binding)
                    }
                    _ => binding,
                }
                && let Some(port) = binding_to_port(ctx, &binding, range)
            {
                return port;
            }
            synthesise_unsupported(ctx, e)
        }
        Expr::Unsafe { inner, range } => {
            // The slot binding IS the value; no tag test, no gate of its own.
            match crate::lower::access::resolve_unsafe_slot(ctx, inner)
                .cloned()
                .and_then(|b| crate::lower::access::binding_to_port(ctx, &b, range))
            {
                Some(port) => port,
                None => synthesise_unsupported_range(ctx, "an `unsafe` read", range),
            }
        }
        Expr::Is { value, path, range } => {
            let compare = Expr::variant_test_desugared(value, path, range);
            lower_expr(ctx, &compare)
        }
        Expr::As { value, .. } => crate::lower::convert::lower_as(ctx, value, e),
        Expr::FieldAccess { obj, field, range } => lower_field_access(ctx, obj, field, range, e),
        Expr::IndexAccess { obj, index, range } => lower_index_access(ctx, obj, index, range, e),
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range,
        } => {
            if ctx.current_exec.is_some() {
                warn_handlers_ungated_by_arm(ctx, walk::Node::Expr(then_branch));
                warn_handlers_ungated_by_arm(ctx, walk::Node::Expr(else_branch));
            }
            if ctx.current_exec.is_some() && (has_effects(ctx, then_branch) || has_effects(ctx, else_branch)) {
                return crate::lower::stmt::lower_if_value(ctx, cond, then_branch, else_branch, range, e);
            }
            lower_if_expr(ctx, cond, then_branch, else_branch, range)
        }
        Expr::MatchExpr { scrutinee, arms, range } => {
            // A value `match` with a block arm (one that leaves through a
            // `return`, typecheck's WS095), or an arm that does more than give
            // its value, threads exec through the statement form's Branch/Union
            // tree, so each arm runs only when it matches; its value arms store
            // into a scratch var. A `match` statement reaches `lower_match_stmt`
            // from `Stmt::ExprStmt`.
            if ctx.current_exec.is_some() {
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(x) => warn_handlers_ungated_by_arm(ctx, walk::Node::Expr(x)),
                        MatchBody::Block(b) => warn_handlers_ungated_by_arm(ctx, walk::Node::Block(b)),
                    }
                }
            }
            if ctx.current_exec.is_some()
                && arms.iter().any(|a| match &a.body {
                    MatchBody::Block(_) => true,
                    MatchBody::Expr(x) => has_effects(ctx, x),
                })
            {
                return crate::lower::stmt::lower_match_value(ctx, scrutinee, arms, range, e);
            }
            lower_match_expr(ctx, scrutinee, arms, range, e)
        }
        Expr::BlockExpr { stmts, value, .. } => {
            ctx.push_scope(crate::scope::ScopeTag::BLOCK);
            for s in stmts {
                lower_stmt(ctx, s);
            }
            let result = lower_expr(ctx, value);
            ctx.pop_scope();
            result
        }
        Expr::Call { .. } => {
            // Constant constructor calls (`Vec/Rotation/Color` on literal
            // args) lower to a _Literal so consumers inline them as component
            // data; `materialize_unfoldable_constants` re-creates the Make*
            // gate for any consumer that can't absorb an inlined value.
            if let Some(lit) = expr_to_literal(e) {
                let ty = match &lit {
                    Literal::Vector { .. } => Some(Type::Vector),
                    Literal::Rotator { .. } => Some(Type::Rotator),
                    Literal::LinearColor { .. } => Some(Type::Color),
                    _ => None,
                };
                if let Some(ty) = ty {
                    return literal_node(ctx, e, ty, lit);
                }
            }
            lower_call(ctx, e)
        }
        Expr::RecordLit { range, .. } => {
            // Record literals are handled in lower_let_decl, not as standalone expressions.
            synthesise_unsupported_range(ctx, crate::diagnostic::LITERAL_AS_ONE_VALUE, range)
        }
        // A map literal reaching the generic expression lowerer is in an
        // unsupported position (not a map-var initializer). Lowering intercepts
        // `MapLit` in the assignment/initializer path before it reaches here.
        Expr::MapLit { range, .. } => synthesise_unsupported_range(ctx, "a map literal where one value goes", range),
        // An array literal read as a value: a constant one is an immutable
        // container, like a `const` table; one with a runtime element has no
        // storage to hand out.
        Expr::Array { range, .. } => {
            if let Some(rec) = super::predeclare::materialize_literal_container(ctx, e, "literal") {
                return rec.node_id.port(WirePort::ArrayVarRef);
            }
            ctx.error(
                "WS044",
                "this array literal has a non-constant element, so it names no storage \
                 for its value to come from. Build it in a `var` (`var xs: int[]` and \
                 `xs.push(..)`) and use that",
                range,
            );
            synthesise_unsupported_no_warn(ctx, range)
        }
        // A well-formed `Expr::VariantCtor` (named enum-payload construction,
        // `Enum.Variant { f: v, ... }`) is always intercepted by
        // `try_lower_enum_ctor` above, before this match runs. Reaching this
        // arm means `path` is NOT a resolvable `Enum.Variant` - a program with
        // a typecheck ERROR here (the common case, since `compile()` runs
        // `lower` before checking for errors: a shadowed/unknown enum name, a
        // non-`FieldAccess` path, ...). Placeholder, not a panic, so such a
        // program still returns its diagnostics instead of crashing.
        Expr::VariantCtor { range, .. } => synthesise_unsupported_range(ctx, "an enum construction", range),
        _ => synthesise_unsupported(ctx, e),
    }
}

/// Bare-name variant resolution - the lowering-side call into the
/// single-sourced `enums::resolve_bare_variant_enum`, supplying lower's OWN
/// shadow predicate so the lookup + uniqueness rule can never drift from
/// typecheck's (they call the same helper).
///
/// Lower's `scope` holds every NON-type binding - `var`/param/let VALUE
/// symbols AND `mod`/`chip` `Binding::Chip` symbols (that is exactly what
/// `lookup_chip`/`resolve_mod` read out of it) - but no type-name entry. So
/// `scope.get` already shadows a user `mod`/`chip` named after a variant
/// (`mod Ok`), and it ORs in `enum_defs.contains_key` to ALSO treat a user
/// enum whose NAME collides with a variant (`enum Some { .. }`) as a shadow -
/// the one type-name collision that is reachable-and-clean, which typecheck
/// shadows too (every enum registers a type-name scope symbol). `resolve_mod`
/// is ORed in as well: it is a strict subset of `scope.get` today (both read
/// `ctx.scope`), so it is belt-and-suspenders rather than a behavior change,
/// but it makes the `mod`/`chip` shadow EXPLICIT and structurally parallel to
/// const-eval's `lookup_mod` term and typecheck's full-scope lookup, so the
/// three predicates read alike and none can silently drift if `resolve_mod`'s
/// backing store or the scope-population order later changes. The other
/// type-only shadows typecheck has (a `type Some = ..` alias, a namespace) are
/// unreachable here - using such a name in value/call position is itself a
/// typecheck error, so no clean program reaches this with one. A `const`
/// parameter is bound by its constant alone (no scope binding), so a constant
/// of the name shadows too.
fn resolve_bare_variant_enum(ctx: &LowerCtx, name: &str) -> Option<String> {
    crate::typecheck::enums::resolve_bare_variant_enum(&ctx.enum_defs, name, |n| {
        ctx.scope.get(n).is_some()
            || ctx.constant(n, crate::const_scope::Admit::All).is_some()
            || ctx.enum_defs.expression_path(n).is_some()
            || ctx.resolve_mod(n).is_some()
    })
    .map(|def| def.name.clone())
}

/// Recognize `e` as an enum CONSTRUCTION - unit `Enum.Variant`/bare `Variant`,
/// positional `Enum.Variant(args)`/bare `Variant(args)`, or named
/// `Enum.Variant { fields }` - whose enum name is a known, non-generic,
/// unshadowed enum. Mirrors the shadow-guard + registry lookup
/// `typecheck::infer::resolve_variant_for_construction`/
/// `resolve_bare_variant_enum` share across their own construction sites, so
/// a bare `Some(x)` resolves to the identical `(enum_name, variant)` pair a
/// qualified `Option.Some(x)` would, and everything below this point (the
/// `Binding::Record` build) runs unchanged for either spelling. On a match:
/// lowers each payload arg, builds the `Binding::Record` via
/// [`lower_enum_ctor`], stashes it in `ctx.pending_inline_record` for a
/// record-shaped consumer (assignment, `let`, return, ...) exactly like a
/// record-returning CALL does (see `call/builtin.rs`), and returns the
/// `__disc` port as the fallback single-port value. `None` for anything else
/// - ordinary `FieldAccess`/`Call`/`Ident` lowering is untouched.
fn try_lower_enum_ctor(ctx: &mut LowerCtx, e: &Expr) -> Option<PortRef> {
    // A value symbol shadowing the enum's name (lower's `scope` never holds a
    // type-name entry, so ANY hit here is a shadow) means the qualified
    // `Enum.Variant`/`Enum.Variant(args)`/`Enum.Variant { .. }` forms below
    // aren't a construction at all.
    let qualified = |obj: &Expr, field: &str| -> Option<(String, String)> {
        let name = ctx.enum_head(obj)?;
        Some((name.to_string(), field.to_string()))
    };
    let (enum_name, variant) = match e {
        Expr::FieldAccess { obj, field, .. } => qualified(obj, field)?,
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::FieldAccess { obj, field, .. } => qualified(obj, field)?,
            // Bare positional construction (`Some(42)` for
            // `Option.Some(42)`): `name` here IS the variant, not the enum -
            // resolved against the full registry via
            // `resolve_bare_variant_enum` (its own shadow guard checks
            // `name` itself, not an enum name, so it is not routed through
            // `qualified` above).
            Expr::Ident { name, .. } => (resolve_bare_variant_enum(ctx, name)?, name.clone()),
            _ => return None,
        },
        Expr::VariantCtor { path, .. } => {
            let Expr::FieldAccess { obj, field, .. } = path.as_ref() else {
                return None;
            };
            qualified(obj, field)?
        }
        // Bare unit-variant reference (`None` for `Option.None`).
        Expr::Ident { name, .. } => (resolve_bare_variant_enum(ctx, name)?, name.clone()),
        _ => return None,
    };
    // A generic enum constructs exactly like a non-generic one - the runtime
    // value layout (`__disc` + per-variant payload slots) is the same, its type
    // arguments having been erased once typecheck inferred them. Only an unknown
    // variant falls through to ordinary field/call lowering.
    let def = ctx.enum_defs.expression_path(&enum_name)?;
    if !def.variants.iter().any(|v| v.name == variant) {
        return None;
    }

    // Each payload value is fixed at its value before a later one runs code
    // that may change what it reads.
    let mut ord = crate::lower::ordered::Ordered::default();
    let mut payload_value = |ctx: &mut LowerCtx, name: String, v: &Expr| {
        let slot = ord.source(ctx, &name, None, v.range(), |ctx| Some(lower_payload_value(ctx, v)));
        (name, slot)
    };
    let payload: Vec<(String, Option<usize>)> = match e {
        // Key each positional arg by its index AMONG POSITIONAL ARGS (not its
        // index among all args), matching `static_enum_ctor` in predeclare.rs
        // exactly so the two producers of the `__{V}_{i}` slot key can never
        // disagree. (Equivalent for a well-formed all-positional construction,
        // which typecheck enforces, but unified so no future mixed-arg shape
        // splits the schemes.)
        Expr::Call { args, .. } => {
            let mut out = Vec::new();
            let mut i = 0usize;
            for a in args {
                if let CallArg::Positional(v) = a {
                    out.push(payload_value(ctx, i.to_string(), v));
                    i += 1;
                }
            }
            out
        }
        Expr::VariantCtor { fields, .. } => fields
            .iter()
            .filter_map(|f| match f {
                RecordLitField::Named { name, value, .. } => Some(payload_value(ctx, name.clone(), value)),
                RecordLitField::Shorthand { name, range } => {
                    let ident = Expr::Ident {
                        name: name.clone(),
                        range: range.clone(),
                    };
                    Some(payload_value(ctx, name.clone(), &ident))
                }
                RecordLitField::Spread { .. } => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let payload: Vec<(String, Binding)> = payload
        .into_iter()
        .filter_map(|(name, slot)| Some((name, ord.value(ctx, slot?)?)))
        .collect();

    // The concrete type arguments (for a generic enum) come from the use-site
    // type typecheck pinned, so `build_enum_fields` lays each payload slot out at
    // its instantiated type.
    let enum_args = match ctx.type_of(e) {
        Type::Enum { args, .. } => args,
        _ => Vec::new(),
    };
    let Binding::Record(fields) =
        lower_enum_ctor(ctx, &enum_name, &enum_args, &variant, payload, e.range())
    else {
        unreachable!("lower_enum_ctor always returns Binding::Record")
    };
    let disc_port = match fields.get(&crate::intern::intern("__disc")) {
        Some(Binding::Local(l)) => Some(l.port),
        _ => None,
    };
    ctx.pending_inline_record = Some(fields);
    disc_port
}

/// Lower ONE enum payload value to the binding its slot needs.
///
/// A payload slot is NOT always a single wire: `enum_payload_slot` lays a
/// record-typed field out per-leaf-field and a nested-enum field out as its own
/// `__disc` + slots superset, so both need a `Binding::Record` here. Lowering
/// every field with `lower_expr` (which returns exactly one `PortRef`) dropped
/// all but one of those ports - a record literal fell through to the
/// `_Unsupported`/WSP001 placeholder, and a nested enum construction quietly
/// yielded only its `__disc` while its `pending_inline_record` was discarded.
/// Either way `assign_record_fields` then found a `Binding::Local` against a
/// `Binding::Record` target, matched no arm, and silently wrote nothing.
///
/// `value_record_fields` is the single source of truth for "what are this
/// value's fields": it resolves record literals, record vars/lets/inputs, field
/// chains, record array/map elements AND enum constructions (through
/// `pending_inline_record`), so a record and a nested enum both land here by the
/// same path the ordinary record-assignment lowering uses. Scalars have no field
/// map, so they fall back to the one-port form unchanged.
///
/// Only a value typecheck did not type as a scalar is probed, and a call the
/// probe lowered is kept, so a payload runs once (`S.B(a.pop().Value)` pops
/// once), as a record-literal field does.
fn lower_payload_value(ctx: &mut LowerCtx, value: &Expr) -> Binding {
    if crate::lower::decl::record_valued(ctx, value) {
        if let Some(fields) = crate::lower::stmt::value_record_fields(ctx, value) {
            return Binding::Record(fields);
        }
        if let Some(port) = crate::lower::stmt::take_value_record_port(ctx, value) {
            return Binding::Local(LocalRecord { port });
        }
    }
    Binding::Local(LocalRecord {
        port: lower_expr(ctx, value),
    })
}

/// `Enum.FromInt(n)`: a tag-only enum construction whose `__disc` is the RUNTIME
/// int `n` (not a baked constant) and whose every payload slot is defaulted to
/// its zero value. Reuses [`build_enum_fields`]'s superset-slot construction
/// (so a match on the value can read any variant's defaulted payload) with the
/// `__disc` override wired to the lowered `n` port, stashes the record in
/// `pending_inline_record` for a let/assign/out consumer exactly like an
/// ordinary construction, and returns the `__disc` port. `None` for anything
/// that is not this shape - the caller falls through to ordinary lowering.
///
/// Recognized only when `enum_name` is a known, unshadowed enum with NO variant
/// literally named `FromInt` (a real variant of that name is an ordinary
/// construction, handled by [`try_lower_enum_ctor`] above). Mirrors the
/// typecheck resolution in `typecheck::infer` and the const fold in
/// `const_eval::expr`.
fn try_lower_enum_from_int(ctx: &mut LowerCtx, e: &Expr) -> Option<PortRef> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::FieldAccess { obj, field, .. } = callee.as_ref() else {
        return None;
    };
    if field != "FromInt" {
        return None;
    }
    // A value symbol shadowing the enum's name means this is not a construction.
    let enum_name = ctx.enum_head(obj)?;
    let def = ctx.enum_defs.expression_path(enum_name).cloned()?;
    if def.variants.iter().any(|v| v.name == "FromInt") {
        return None;
    }
    // The single positional argument is the runtime tag. A well-formed call
    // (typecheck enforces exactly one int arg) has it as the first positional;
    // an ill-formed one falls back to a literal 0 so lowering never panics.
    let disc_port = args
        .iter()
        .find_map(|a| match a {
            CallArg::Positional(v) => Some(lower_expr(ctx, v)),
            _ => None,
        })
        .unwrap_or_else(|| literal_node_range(ctx, e.range(), Type::Int, Literal::Int(0)));
    // The enum's concrete generic arguments (empty for a non-generic enum) come
    // from the call's inferred type, so a generic payload slot lays out against
    // the right instantiation.
    let enum_args = match ctx.type_of(e) {
        Type::Enum { args, .. } => args,
        _ => Vec::new(),
    };
    let fields = crate::lower::predeclare::build_enum_fields(
        ctx,
        &def,
        &enum_args,
        enum_name,
        None,
        Some(disc_port),
        e.range(),
    );
    ctx.pending_inline_record = Some(fields);
    Some(disc_port)
}

/// `<enum value>.ToInt()`: an exact alias for `.Discriminant`, resolved through
/// the same [`lower_discriminant`] helper (a variant path bakes the registry
/// discriminant as a literal; a stored/merged value reads its `__disc` slot).
/// `None` for a no-args call whose receiver is not an enum value, or any other
/// shape - the caller falls through to ordinary call lowering.
fn try_lower_enum_to_int(ctx: &mut LowerCtx, e: &Expr) -> Option<PortRef> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let Expr::FieldAccess { obj, field, range } = callee.as_ref() else {
        return None;
    };
    if field != "ToInt" {
        return None;
    }
    crate::lower::access::lower_discriminant(ctx, obj, range)
}

/// `EnumToInt(value)`: the gate-backed twin of `<enum value>.ToInt()`. A
/// compile-time-known enum (a variant literal such as `Shape.Circle(1.0)`, or a
/// `const` enum value) folds straight to its discriminant literal with NO gate,
/// reusing the same const evaluation `.ToInt()` and `const_eval` use. A runtime
/// enum value instead EMITS the real `EXPR_ENUM_TO_INTEGER` gate, fed by the
/// value's `__disc` port (for record enums that tag IS the integer, but the
/// gate is emitted per the design so game/native enums route through it). `None`
/// for anything that is not this call shape, or when the name is shadowed by a
/// user chip/mod - the caller falls through to ordinary lowering.
fn try_lower_enum_to_integer(ctx: &mut LowerCtx, e: &Expr) -> Option<PortRef> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident { name, range: name_range } = callee.as_ref() else {
        return None;
    };
    if name != "EnumToInt" {
        return None;
    }
    // A user chip/mod of the same name shadows the builtin (matches the
    // `lookup_chip`-first dispatch in `lower::call::dispatch::lower_call`).
    if ctx.call_target_chip(name, &name_range.file).is_some() {
        return None;
    }
    let Some(CallArg::Positional(value)) = args.first() else {
        return None;
    };
    // Fold path: const-evaluate the WHOLE call (the const-eval mirror folds
    // `EnumToInt(<const enum>)` to its discriminant int). A successful fold
    // bakes a literal and emits no gate.
    let folded = {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()
    };
    if let Some(Literal::Int(n)) = folded {
        return Some(literal_node_range(ctx, e.range(), Type::Int, Literal::Int(n)));
    }
    // Runtime path: read the value's `__disc` port and feed it into the real
    // `EXPR_ENUM_TO_INTEGER` gate. `lower_discriminant` yields the tag port for
    // a stored enum value; if the value has no record decomposition (an
    // already-errored program) fall back to its own single port so the gate is
    // still wired to something rather than left dangling.
    let disc_port = crate::lower::access::lower_discriminant(ctx, value, e.range())
        .unwrap_or_else(|| lower_expr(ctx, value));
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::EXPR_ENUM_TO_INTEGER,
        source_range: e.range().clone(),
        ports: GateIO {
            inputs: vec![PortSpec::new(WirePort::Input.sym(), Type::Any)],
            outputs: vec![PortSpec::new(*sym::OUTPUT, Type::Int)],
        },
        ..Default::default()
    });
    ctx.connect(disc_port, node_id.port(WirePort::Input));
    Some(node_id.port(WirePort::Output))
}

/// `IntToEnum(value, wrap?)`: the gate-backed twin of `Enum.FromInt(n)`.
/// The result enum's concrete type comes from the use-site expected type
/// (`ctx.type_of(e)`, pinned by typecheck), so this builds the same superset
/// `__disc` + defaulted-payload record `build_enum_fields` lays down for
/// `FromInt`, differing only in where `__disc` is sourced:
///   - a compile-time-constant `value` binds `__disc` to that literal int, with
///     NO `EXPR_INTEGER_TO_ENUM` gate (the const mirror of the fold path);
///   - a runtime `value` EMITS the `EXPR_INTEGER_TO_ENUM` gate (wired to the
///     lowered int, plus `wrap` when given) and binds `__disc` to the gate's
///     output, so a later `match` routes on the runtime tag.
/// `None` when this is not that call shape, the name is shadowed by a user
/// chip/mod, or the expected type is not a known enum (an already-errored
/// program) - the caller falls through to ordinary lowering.
fn try_lower_integer_to_enum(ctx: &mut LowerCtx, e: &Expr) -> Option<PortRef> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident { name, range: name_range } = callee.as_ref() else {
        return None;
    };
    if name != "IntToEnum" {
        return None;
    }
    if ctx.call_target_chip(name, &name_range.file).is_some() {
        return None;
    }
    // The concrete enum type is pinned by the use site (typecheck recorded it).
    let enum_ty = ctx.type_of(e);
    let (enum_name, enum_args) = match &enum_ty {
        Type::Enum { name, args } => (name.clone(), args.clone()),
        _ => return None,
    };
    let def = ctx.enum_defs.of(&enum_ty).cloned()?;
    // The first positional arg is the integer; a later `wrap` may be positional
    // or named.
    let value = args.iter().find_map(|a| match a {
        CallArg::Positional(v) => Some(v),
        _ => None,
    })?;
    // An explicit `wrap` (positional or named) clamps an out-of-range tag - a
    // runtime-gate behavior the tag-only fold cannot reproduce, so a `wrap` call
    // always routes through the gate even with a constant int.
    let has_wrap = args.iter().enumerate().any(|(i, a)| match a {
        CallArg::Named { name, .. } => name == "wrap",
        CallArg::Positional(_) => i >= 1,
        _ => false,
    });
    // Fold path: a compile-time-constant int (and no `wrap`) sources `__disc`
    // directly, no gate.
    let const_int = if has_wrap {
        None
    } else {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        match crate::const_eval::eval_expr(value, &ctx.const_ctx(Some(&lookup)), &mut budget) {
            Ok(Literal::Int(n)) => Some(n),
            _ => None,
        }
    };
    let disc_port = if let Some(n) = const_int {
        literal_node_range(ctx, e.range(), Type::Int, Literal::Int(n))
    } else {
        // Runtime path: wire the int (and optional `wrap`) into the real gate;
        // its output feeds `__disc`.
        let value_port = lower_expr(ctx, value);
        let wrap_port = args
            .iter()
            .enumerate()
            .find_map(|(i, a)| match a {
                CallArg::Named { name, value, .. } if name == "wrap" => Some(lower_expr(ctx, value)),
                // A second positional arg is `wrap`.
                CallArg::Positional(v) if i >= 1 => Some(lower_expr(ctx, v)),
                _ => None,
            });
        let mut inputs = vec![PortSpec::new(WirePort::Input.sym(), Type::Int)];
        if wrap_port.is_some() {
            inputs.push(PortSpec::new(WirePort::BWrap.sym(), Type::Bool));
        }
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::EXPR_INTEGER_TO_ENUM,
            source_range: e.range().clone(),
            ports: GateIO {
                inputs,
                outputs: vec![PortSpec::new(*sym::OUTPUT, Type::Int)],
            },
            ..Default::default()
        });
        ctx.connect(value_port, node_id.port(WirePort::Input));
        if let Some(wp) = wrap_port {
            ctx.connect(wp, node_id.port(WirePort::BWrap));
        }
        node_id.port(WirePort::Output)
    };
    let fields = crate::lower::predeclare::build_enum_fields(
        ctx,
        &def,
        &enum_args,
        &enum_name,
        None,
        Some(disc_port),
        e.range(),
    );
    ctx.pending_inline_record = Some(fields);
    Some(disc_port)
}

/// Build the `Binding::Record` for a construction of `variant` on `enum_name`
/// (with concrete `enum_args` for a generic enum): `__disc` bound to a
/// `Literal::Int` of the registry discriminant (a fresh value's tag is always
/// statically known), and the FULL payload superset - every variant's slots,
/// defaulted via [`build_enum_fields`] - with the constructed variant's own
/// slots then overwritten by its runtime payload ports. Building the superset
/// (rather than only the constructed variant's slots) is what lets a later
/// `match`/`if let`/`let else` on this fresh value read ANY arm's payload; a
/// value that never reaches a stored `var` (a `let`/`const` binding, a call
/// argument, a mod return) would otherwise carry only its own variant's slots
/// and lower every other arm's payload read to `_Unsupported`. An unknown
/// `enum_name` (shouldn't happen post-typecheck) falls back to a minimal
/// `__disc(0)` + provided-ports record rather than panicking.
pub(super) fn lower_enum_ctor(
    ctx: &mut LowerCtx,
    enum_name: &str,
    enum_args: &[Type],
    variant: &str,
    payload: Vec<(String, Binding)>,
    range: &SourceRange,
) -> Binding {
    let Some(def) = ctx.enum_defs.expression_path(enum_name).cloned() else {
        let mut fields = HashMap::default();
        let disc_port = literal_node_range(ctx, range, Type::Int, Literal::Int(0));
        fields.insert(
            crate::intern::intern("__disc"),
            Binding::Local(LocalRecord { port: disc_port }),
        );
        for (slot_key, binding) in payload {
            fields.insert(
                crate::intern::intern(&format!("__{variant}_{slot_key}")),
                binding,
            );
        }
        return Binding::Record(fields);
    };
    let disc = def
        .variants
        .iter()
        .find(|v| v.name == variant)
        .map(|v| v.discriminant)
        .unwrap_or(0);
    let disc_port = literal_node_range(ctx, range, Type::Int, Literal::Int(disc));
    let mut fields = crate::lower::predeclare::build_enum_fields(
        ctx,
        &def,
        enum_args,
        enum_name,
        None,
        Some(disc_port),
        range,
    );
    for (slot_key, binding) in payload {
        fields.insert(
            crate::intern::intern(&format!("__{variant}_{slot_key}")),
            binding,
        );
    }
    Binding::Record(fields)
}

/// The `*Reference` gate class that sources an asset of `asset_type` as an
/// `entity` wire — it holds the asset in its class/object `Asset` field and
/// outputs `Value: entity`.
fn asset_reference_gate(asset_type: &str) -> &'static str {
    match asset_type {
        "BRItemBase" => "BrickComponentType_ItemReference",
        "BRPickupBase" => "BrickComponentType_PickupReference",
        "BRWeaponProjectile" => "BrickComponentType_ProjectileReference",
        "BrickAudioDescriptor" => "BrickComponentType_AudioReference",
        "BrickFontDescriptor" => "BrickComponentType_FontReference",
        "BrickOneShotAudioDescriptor" => "BrickComponentType_OneShotAudioReference",
        "BrickWheelEngineAudioDescriptor" => {
            "BrickComponentType_WheelEngineAudioReference"
        }
        // Unknown/other categories → the generic entity-type reference.
        _ => "BrickComponentType_EntityTypeReference",
    }
}

/// A number constant `e` as the literal it lowers to under the type typecheck
/// recorded for it (`const_eval::coerce_const_to_type`): an integer is the
/// float it names in a `float` slot, a float the integer it truncates to in
/// an `int` slot, and each is itself anywhere else.
pub(in crate::lower) fn constant_literal(ctx: &LowerCtx, e: &Expr, lit: Literal) -> (Type, Literal) {
    let ty = ctx.type_of(e);
    if let (Type::Bool, Literal::Int(n)) = (&ty, &lit) {
        return (Type::Bool, Literal::Bool(*n != 0));
    }
    match crate::const_eval::coerce_const_to_type(lit, &ty, ctx.consts.types()) {
        Literal::Float(f) => (Type::Float, Literal::Float(f)),
        lit => (Type::Int, lit),
    }
}

/// An integer constant expression (a name bound to one, or an operator, call,
/// index or field read of constants) that typecheck typed `float` because it
/// flows into a `float` slot: the float it names. Each read is typed on its
/// own, so the same constant read into an `int` slot stays an integer.
///
/// Every name `e` reads resolves through the scope (`certified_const_ctx`),
/// so a parameter or local named like an outer constant keeps its own wire.
/// An integer operation the certified fold table has no result for, in `e`,
/// in a `const mod` it calls or behind a name it reads, leaves the value to
/// the gate.
fn int_constant_expr_as_float(ctx: &LowerCtx, e: &Expr) -> Option<Literal> {
    if !matches!(
        e,
        Expr::Ident { .. }
            | Expr::BinOp { .. }
            | Expr::UnOp { .. }
            | Expr::Call { .. }
            | Expr::IndexAccess { .. }
            | Expr::FieldAccess { .. }
            | Expr::TuplePick { .. }
    ) || !matches!(ctx.type_of(e), Type::Float)
    {
        return None;
    }
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    match crate::const_eval::eval_expr(e, &ctx.certified_const_ctx(Some(&lookup)), &mut budget) {
        Ok(lit @ Literal::Int(_)) if !budget.uncertified => {
            Some(crate::const_eval::coerce_const_to_type(lit, &Type::Float, ctx.consts.types()))
        }
        _ => None,
    }
}

/// A float constant expression (a literal, a name bound to one, or an operator,
/// call, index or field read of constants) that typecheck typed `int` because
/// it flows into an `int` slot: the integer it truncates to, as
/// `const_eval::coerce_const_to_type` holds it.
fn float_constant_expr_as_int(ctx: &LowerCtx, e: &Expr) -> Option<Literal> {
    if !matches!(
        e,
        Expr::FloatLit { .. }
            | Expr::Ident { .. }
            | Expr::BinOp { .. }
            | Expr::UnOp { .. }
            | Expr::Call { .. }
            | Expr::IndexAccess { .. }
            | Expr::FieldAccess { .. }
            | Expr::TuplePick { .. }
    ) || !matches!(ctx.type_of(e), Type::Int)
    {
        return None;
    }
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    match crate::const_eval::eval_expr(e, &ctx.certified_const_ctx(Some(&lookup)), &mut budget) {
        Ok(lit @ Literal::Float(_)) if !budget.uncertified => {
            Some(crate::const_eval::coerce_const_to_type(lit, &Type::Int, ctx.consts.types()))
                .filter(|l| matches!(l, Literal::Int(_)))
        }
        _ => None,
    }
}

/// A name bound to an integer constant, read where typecheck typed it
/// `float` (an integer constant passed to a `float` parameter): the float
/// literal it names, beside the shared integer gate.
fn retype_int_constant(ctx: &mut LowerCtx, e: &Expr, port: PortRef) -> PortRef {
    match int_constant_as_float(ctx, e, port) {
        Some(lit) => literal_node(ctx, e, Type::Float, lit),
        None => port,
    }
}

/// The float literal a read `e` of the integer constant at `port` names,
/// when typecheck typed the read `float`.
fn int_constant_as_float(ctx: &LowerCtx, e: &Expr, port: PortRef) -> Option<Literal> {
    let n = ctx.builder.module.nodes.get(&port.node_id).filter(|n| n.gate_class == gc::LITERAL)?;
    let lit @ Literal::Int(_) = n.properties.get(&*sym::VALUE)?.clone() else {
        return None;
    };
    match constant_literal(ctx, e, lit) {
        (Type::Float, lit) => Some(lit),
        _ => None,
    }
}

/// Whether `e` names a number constant typecheck typed as the other number
/// type (an integer typed `float`, a float typed `int`), whose read is a
/// literal of that type rather than the binding's shared gate.
pub(in crate::lower) fn reads_constant_as_other_number(ctx: &LowerCtx, e: &Expr) -> bool {
    if int_constant_expr_as_float(ctx, e).is_some() || float_constant_expr_as_int(ctx, e).is_some() {
        return true;
    }
    let Expr::Ident { name, .. } = e else { return false };
    match ctx.scope.get(name) {
        Some(Binding::Local(l)) => int_constant_as_float(ctx, e, l.port).is_some(),
        _ => false,
    }
}

pub(super) fn literal_node(ctx: &mut LowerCtx, e: &Expr, ty: Type, lit: Literal) -> PortRef {
    literal_node_range(ctx, e.range(), ty, lit)
}

/// The wire type a bare [`Literal`] produces when it is materialized as a
/// literal source gate. Only needed where the literal arrives WITHOUT a
/// declared annotation to take the type from (see `lower_ident`'s constant
/// fallback); every other `literal_node` caller passes the type its
/// expression already typechecked to.
///
/// Exhaustive over every `Literal` variant on purpose — no catch-all — so
/// adding a new one forces a decision here instead of silently landing on
/// `Type::Any`, the same discipline every other exhaustive match over
/// `Literal` already applies. Reaching this function at all means a `const`
/// value is being read bare: a top-level `const`/`let`, OR a `const`
/// mod/chip parameter: `lower_chip_call_inline`'s `is_const` branch records
/// whatever `const_eval::eval_expr` returns for the call argument in the
/// body frame's constants with NO filter on the literal's kind, unlike a top-level
/// `let` (which special-cases record/array/map/prefab initializers into
/// their own bindings, mostly bypassing this fallback — see `lower_let_decl`).
/// So a `const` PARAMETER of any of these "no wire form" types, read bare
/// where lowering doesn't otherwise resolve it (e.g. inside a record-literal
/// field), lands here even when the same value as a top-level constant would
/// not have.
///
/// `None` means "no wire form" — the caller must fall back to the same
/// WSP001 "unsupported expression" placeholder this arm bypassed before the
/// `const` fallback existed, not bake a `Type::Any` literal gate whose value
/// has nowhere honest to go (`emit::variants::literal_to_wire_variant`
/// returns `None` for these same kinds too — there is no wire-variant form
/// to inline into).
pub(crate) fn wire_type_of_literal(lit: &Literal) -> Option<Type> {
    match lit {
        Literal::Bool(_) => Some(Type::Bool),
        Literal::Int(_) => Some(Type::Int),
        Literal::Float(_) => Some(Type::Float),
        Literal::String(_) => Some(Type::String),
        Literal::Vector { .. } => Some(Type::Vector),
        Literal::Rotator { .. } => Some(Type::Rotator),
        Literal::Quat { .. } => Some(Type::Quat),
        Literal::Color { .. } | Literal::LinearColor { .. } => Some(Type::Color),
        // A compile-time-constant prefab reference. Never a runtime wire
        // value (see `Type::PrefabRef`'s doc comment — reference-only, like
        // `Type::Zone`/`Type::Teleport`), but it IS a real, singular type, so
        // it gets one rather than falling into the no-wire-form group below.
        // Reachable both as a top-level `const p = $./f.brz` (predeclare.rs's
        // `build_const_env` folds it independently of `lower_let_decl`'s own
        // no-scope-binding early return for the very same decl) and as a
        // `const` parameter read bare.
        Literal::PrefabRef { .. } | Literal::NestedPrefab { .. } => Some(Type::PrefabRef),
        // No SCALAR wire type — collections, a compile-time record, and an
        // external asset reference, not values a single wire pin carries.
        // All four are reachable through the `const`-PARAMETER path described
        // above (a top-level `let`/`const` shields Array/Map/Record/Asset
        // from ever reaching here — see `lower_let_decl`'s record/array/map/
        // asset-specific handling — but nothing shields a `const` parameter).
        Literal::Array(_) | Literal::Map(_) | Literal::Record(_) | Literal::Asset { .. } => None,
        // Never produced by any source expression — no `Expr` folds to it.
        // It exists solely as `default_literal_for_var_type`'s placeholder
        // for a `Var` gate's initial value (Controller/Character/Entity),
        // which never touches `const_eval`/`ConstEnv`, so it can never reach
        // `const_lookup()` — the only way anything reaches this function.
        // Treated the same as the no-wire-form group just above rather than
        // `unreachable!()`: if this invariant is ever wrong (a future
        // `const_eval` change starts producing one), the caller already
        // falls back to the same `_Unsupported`/WSP001 placeholder a
        // no-wire-form constant gets — a diagnosable gap, not a process
        // abort. See `lower::tests::const_init::literal_object_is_never_produced_by_const_eval`
        // for the invariant this leans on.
        Literal::Object => None,
    }
}

pub(super) fn literal_node_range(
    ctx: &mut LowerCtx,
    range: &SourceRange,
    ty: Type,
    lit: Literal,
) -> PortRef {
    // String literals can't be inlined as wire_graph_variant immediate values
    // on consumer gates (e.g. Select). Emit them as String_Concatenate gates
    // whose str-typed fields accept inline strings, producing a wire signal.
    if let Literal::String(ref s) = lit {
        let mut props = HashMap::default();
        props.insert(*sym::INPUT_A, Literal::String(s.clone()));
        props.insert(*sym::INPUT_B, Literal::String(String::new()));
        props.insert(intern_static("Separator"), Literal::String(String::new()));
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::STRING_CONCATENATE,
            source_range: range.clone(),
            ports: GateIO::source(Type::String),
            properties: props,
            ..Default::default()
        });
        return node_id.port(WirePort::Output);
    }
    let mut props = HashMap::default();
    props.insert(*sym::VALUE, lit);
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: range.clone(),
        ports: GateIO::source(ty),
        properties: props,
        ..Default::default()
    });
    node_id.port(WirePort::Output)
}

/// `&x` in a value position: the port carrying `x`'s ref. A var's `VarRef`,
/// an array's `ArrayVarRef`, a map's `MapVarRef`, an `in` port's `RER_Output`,
/// or, for a name already bound to a runtime ref, that wire. An operand that
/// already evaluates to a ref (`&rows[0]`, a var holding an array ref) is its
/// own ref.
fn lower_ref_of(ctx: &mut LowerCtx, operand: &Expr, range: &SourceRange) -> PortRef {
    let binding = match operand {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        Expr::FieldAccess { .. } | Expr::TuplePick { .. } => resolve_field_chain(ctx, operand).cloned(),
        _ => None,
    };
    match binding {
        Some(Binding::Var(v)) if matches!(v.inner_type, Type::Ref(_)) && v.storage == VarStorage::Var => {
            lower_expr(ctx, operand)
        }
        Some(Binding::Var(v)) => match v.storage {
            VarStorage::Var => v.node_id.port(WirePort::VarRef),
            VarStorage::Array => v.node_id.port(WirePort::ArrayVarRef),
            VarStorage::Map => v.node_id.port(WirePort::MapVarRef),
            VarStorage::Buffer => {
                ctx.error("WS008", "a buffer has no ref to take: it is a wire, not a var", range);
                synthesise_unsupported_no_warn(ctx, range)
            }
        },
        Some(Binding::DynRef(d)) => d.port,
        Some(Binding::Input(inp)) => inp.node_id.port(WirePort::RerOutput),
        _ => lower_expr(ctx, operand),
    }
}

/// The value of a name bound to a runtime ref: an array or map ref is its own
/// wire; a `*T` is read through with a `Var_Get`, which needs an exec chain.
pub(super) fn read_dyn_ref(ctx: &mut LowerCtx, d: &DynRefRecord, range: &SourceRange) -> PortRef {
    if d.storage != VarStorage::Var {
        return d.port;
    }
    match ctx.current_exec {
        Some(exec) => chain_var_get_through(ctx, d.port, &d.inner_type, exec, range)
            .port(WirePort::Value),
        None => {
            ctx.error(
                "WS006",
                "reading through a runtime ref needs an exec context: the read is a Var_Get, \
                 which only runs on an exec chain",
                range,
            );
            synthesise_unsupported_no_warn(ctx, range)
        }
    }
}

/// The type typecheck gave the identifier `name` read at `range`, in the
/// instantiation being lowered.
fn ident_type(ctx: &LowerCtx, name: &str, range: &SourceRange) -> Type {
    ctx.type_of(&Expr::Ident { name: name.to_string(), range: range.clone() })
}

pub(super) fn lower_ident(ctx: &mut LowerCtx, name: &str, range: &SourceRange) -> PortRef {
    let binding = ctx.scope.get(name).cloned();
    match binding {
        Some(Binding::Var(var_rec)) => {
            if var_rec.storage == VarStorage::Buffer {
                return var_rec.node_id.port(WirePort::Output);
            }
            if var_rec.storage == VarStorage::Array {
                return var_rec.node_id.port(WirePort::ArrayVarRef);
            }
            if var_rec.storage == VarStorage::Map {
                return var_rec.node_id.port(WirePort::MapVarRef);
            }
            if let Some(exec) = ctx.current_exec {
                if let Some(cached) = var_rec.get_node_for_handler {
                    return cached.port(WirePort::Value);
                }
                let props = var_get_seed_props(&var_rec.inner_type);
                let get_id = chain_var_get(ctx, &var_rec, exec, range, props, None);
                if let Some(Binding::Var(v)) = ctx.scope.get_mut(name) {
                    v.get_node_for_handler = Some(get_id);
                }
                return get_id.port(WirePort::Value);
            }
            var_rec.node_id.port(WirePort::Value)
        }
        Some(Binding::Buffer(buf)) => buf.node_id.port(WirePort::Output),
        Some(Binding::Input(inp)) => inp.node_id.port(WirePort::RerOutput),
        Some(Binding::EventParam(p)) => p,
        Some(Binding::Local(local)) => local.port,
        Some(Binding::DynRef(d)) => read_dyn_ref(ctx, &d, range),
        Some(Binding::Record(ref rec)) if enum_tag(ctx, rec, &ident_type(ctx, name, range)).is_some() => {
            let tag = enum_tag(ctx, rec, &ident_type(ctx, name, range)).cloned().expect("checked by the guard");
            crate::lower::access::binding_to_port(ctx, &tag, range)
                .unwrap_or_else(|| synthesise_unsupported_no_warn(ctx, range))
        }
        Some(Binding::Record(rec)) => {
            // Records are compile-time bundles of per-field ports; they don't
            // produce a single port. Field access reaches its fields through
            // `lower_field_access`, and a whole-record consumer (an argument,
            // an `out`, an assignment) goes through `value_record_fields`, so
            // arriving HERE means the record was asked for as ONE value, which
            // it has no wire to give.
            //
            // Typecheck cannot catch this: `op_operand_type` collapses a record
            // operand to its first field, which is the legitimate auto-unwrap
            // for a multi-output GATE result (`ParseInt(s) == 42`) and for a
            // call written inline, both of which DO lower to a real port and so
            // never reach this arm. Only the binding tells the two apart, so the
            // diagnostic belongs here rather than in typecheck.
            // An enum's slots are internal: name its tag instead.
            let declared = unwrap_ref(&ident_type(ctx, name, range));
            let enum_value = matches!(declared, Type::Enum { .. });
            let mut names: Vec<&str> =
                rec.keys().map(|k| crate::intern::resolve(*k)).filter(|n| !n.starts_with("__")).collect();
            names.sort_unstable();
            // The first field as the type declares it, when the type names one.
            if let Some(first) = declared.fields().and_then(|fs| fs.iter().find(|(n, _)| !n.starts_with("__")))
                && let Some(i) = names.iter().position(|n| *n == first.0)
            {
                names.swap(0, i);
            }
            let message = if enum_value {
                format!(
                    "`{name}` holds an enum value, so it can't be used where a single value is \
                     expected. Read its tag (`{name}.Discriminant`) or `match` it."
                )
            } else {
                let field = names.first().map_or(String::new(), |f| format!(" (`{name}.{f}`)"));
                format!(
                    "`{name}` is a record - several wires, not one value - so \
                     it can't be used where a single value is expected. Read \
                     one of its fields{field}; to build a string, concatenate \
                     the fields individually."
                )
            };
            // A gate read held as its outputs gives one value its first.
            if let Some(b) =
                crate::lower::context::held_output(ctx, &rec, range, crate::lower::context::OutputPick::First)
                && let Some(port) = crate::lower::access::binding_to_port(ctx, &b, range)
            {
                return port;
            }
            ctx.error("WS071", message, range);
            synthesise_unsupported_no_warn(ctx, range)
        }
        Some(Binding::Output(_) | Binding::Chip(_) | Binding::Namespace(_)) => {
            synthesise_unsupported_range(ctx, &format!("a read of `{name}`"), range)
        }
        // Not bound to any wire at all. Before giving up, check whether the
        // name is a compile-time CONSTANT (`ctx.const_lookup()`: the module's
        // constants and those of every open frame, as the scope resolves them) and, if so,
        // materialize it as a literal source gate — the existing
        // literal-inlining pass (`lower/mod.rs`) then folds that gate's value
        // straight into whatever consumes it and prunes the gate, so a constant
        // read in a WIRE position costs nothing while still producing a real
        // operand.
        //
        // This is what a `const` PARAMETER needs: `lower_chip_call_inline`
        // records its value in the body frame's constants (no wire, so a const-only use
        // emits no gates at all), which means the name is deliberately absent
        // from `scope` — without this arm, `mod addk(n: const int, m: int) { out
        // r = n + m }` lowered `n` to `_Unsupported`, a WSP001 warning and a
        // silently dead gate rather than the literal `5`.
        //
        // Strictly a NARROWING of the `_Unsupported` case: a name that IS bound
        // in scope never reaches here, so no program that already lowered
        // correctly can change. Composite constants (Vector/Rotator/Quat/Color)
        // are deliberately included — unlike `literal_for_property_port`'s
        // bare-Ident path (which excludes them so a named vector keeps its wired
        // `Make*` producer), the alternative here is not a wire but
        // `_Unsupported`, so a real literal source is unambiguously better.
        //
        // `wire_type_of_literal` returns `None` for a constant with no wire
        // form (a collection, a compile-time record, an external asset
        // reference — see its doc comment) — reachable through a `const`
        // parameter of one of those types read bare. That is exactly the
        // pre-fallback situation this whole arm narrows: fall back to the
        // same `_Unsupported`/WSP001 placeholder, rather than baking a
        // `Type::Any` literal gate around a value with nowhere honest to go.
        None => {
            if let Some(lit) = ctx.const_lookup().get(name).cloned()
                && let Some(ty) = wire_type_of_literal(&lit)
            {
                return literal_node_range(ctx, range, ty, lit);
            }
            // A constant array/map read as a VALUE (an `if` branch, a record
            // field, a tuple element, a `return`, an `out`, a chip pin): its
            // runtime form is the container gate, and the value is its ref.
            let ident = Expr::Ident { name: name.to_string(), range: range.clone() };
            if let Some(rec) = super::predeclare::materialize_const_container(ctx, &ident) {
                let port = match rec.storage {
                    VarStorage::Map => WirePort::MapVarRef,
                    _ => WirePort::ArrayVarRef,
                };
                return rec.node_id.port(port);
            }
            // Fallback tier: an output port reads as its rerouter's
            // `RER_Output`, the same port a namespaced `L.count` read already
            // resolves to. Keyed under the `out:` prefix, so it is reached only
            // when nothing in the ordinary tier owns the name. Placed after the
            // constant lookup so a `const` parameter keeps winning, which is
            // also the order typecheck resolves in.
            if let Some(Binding::Output(out)) = ctx
                .scope
                .get(&crate::lower::context::output_scope_key(name))
                .cloned()
            {
                return out.node_id.port(WirePort::RerOutput);
            }
            // A mod or chip body reads a top-level `let` where it is called,
            // so a call above the declaration finds nothing bound yet. Outside
            // every body, the read itself is above the declaration, which the
            // type check reports (WS002).
            if ctx.known_let_names.contains(name) && ctx.chip_call_stack.is_empty() {
                return synthesise_unsupported_no_warn(ctx, range);
            }
            if ctx.known_let_names.contains(name) {
                ctx.error(
                    "WS021",
                    format!(
                        "`{name}` is read before its declaration: a mod or chip body reads a \
                         top-level `let` where it is called, so declare `{name}` above the \
                         first call"
                    ),
                    range,
                );
                return synthesise_unsupported_no_warn(ctx, range);
            }
            synthesise_unsupported_range(ctx, &format!("a read of `{name}`"), range)
        }
    }
}

pub(super) fn lower_if_expr(
    ctx: &mut LowerCtx,
    cond: &Expr,
    then_br: &Expr,
    else_br: &Expr,
    range: &SourceRange,
) -> PortRef {
    let ports = crate::lower::ordered::lower_in_order(ctx, &[cond, then_br, else_br]);
    let (cond_port, then_port, else_port) = (ports[0], ports[1], ports[2]);
    // Widen to the branches' least upper bound (matches the typecheck result
    // for `Expr::IfExpr`) so the Select gate's ports carry the joined type —
    // e.g. `if c then 1 else 2.0` emits a Float Select, with the int branch's
    // wire relying on native port-type compatibility to flow into it (no
    // cast gate is inserted for numeric coercion). Falls back to the else
    // branch's type if there's no common widening (typecheck already raised
    // WS003 for that case; this just picks something to keep lowering going).
    // Inside a generic mod body `type_of` holds the stale last-mask-member type
    // (the per-mask-member body check overwrote it), so read the branches' ACTUAL
    // lowered port types — the concrete monomorph — falling back to `type_of`.
    // Non-generic lowering keeps the byte-identical `type_of` path, as does a
    // non-generic body inlined from a generic caller.
    // A gate read branch gives its first output.
    let branch_ty = |ctx: &LowerCtx, e: &Expr| ctx.one_value_type_at(e.range()).unwrap_or(Type::Any);
    let (then_ty, else_ty) = if ctx.mono_stack.is_empty() || !ctx.in_generic_instance(range) {
        (unwrap_ref(&branch_ty(ctx, then_br)), unwrap_ref(&branch_ty(ctx, else_br)))
    } else {
        let then_ty = super::call::arg_port_type(ctx, then_port).unwrap_or_else(|| branch_ty(ctx, then_br));
        let else_ty = super::call::arg_port_type(ctx, else_port).unwrap_or_else(|| branch_ty(ctx, else_br));
        (unwrap_ref(&then_ty), unwrap_ref(&else_ty))
    };
    let result_ty =
        crate::types::coerce::widening_join(&then_ty, &else_ty).unwrap_or(else_ty);
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::SELECT,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::new(*sym::INPUT_A, result_ty.clone()),
                PortSpec::new(*sym::INPUT_B, result_ty.clone()),
                PortSpec::new(*sym::B_SELECT_B, Type::Bool),
            ],
            outputs: vec![PortSpec::new(*sym::OUTPUT, result_ty.clone())],
        },
        note: Some("if-expr select".into()),
        ..Default::default()
    });
    ctx.connect(cond_port, node_id.port(WirePort::BSelectB));
    ctx.connect(then_port, node_id.port(WirePort::InputB));
    ctx.connect(else_port, node_id.port(WirePort::InputA));
    node_id.port(WirePort::Output)
}

/// Lower a `match` used as a VALUE (expression position) to a pure nested
/// `Select` tree - no exec, no `Branch` (that is the statement form). Builds the
/// decision tree from the scrutinee's enum type and arm patterns, then
/// walks it: a `Switch` reads the `__disc` at its path and folds one `Select` per
/// case keyed on `disc == case`; a `Leaf` lowers its arm body with the arm's
/// payload captures bound as compile-time moves of the scrutinee's payload slots.
pub(super) fn lower_match_expr(
    ctx: &mut LowerCtx,
    scrutinee: &Expr,
    arms: &[MatchArm],
    range: &SourceRange,
    match_expr: &Expr,
) -> PortRef {
    // The result type is the widening join over the arm bodies - exactly the
    // type typecheck recorded for the whole match expression; of a join of
    // gate reads, its first output, as `lower_if_expr` takes it.
    let result_ty = unwrap_ref(&ctx.one_value_type_at(match_expr.range()).unwrap_or(Type::Any));
    let scrut_ty = scrutinee_type(ctx, scrutinee);
    let arm_patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
    let decision = crate::lower::matchtree::build(ctx.enums(), &scrut_ty, &arm_patterns);

    // An enum value lowers to a `Binding::Record` of `__disc` + payload slots.
    // A NAMED scrutinee (`match s`) resolves to that record through
    // the scope; an INLINE-construction scrutinee (`match Shape.Circle(3.0)
    // {...}`) resolves through no name, so lower the construction to obtain it
    // (see `inline_scrutinee_record`). Any other non-record scrutinee (an enum
    // INPUT port, a typecheck-error program) has no record decomposition - the
    // loud `_Unsupported` placeholder.
    let Some(root) = match_scrutinee_record(ctx, scrutinee) else {
        return synthesise_unsupported_range(ctx, "a `match` on a value with no enum tag", range);
    };

    // Const-elision fast path: when the scrutinee's `__disc` is a
    // compile-time-known constant, resolve straight to the taken leaf and
    // lower ONLY that leaf via `lower_decision` - no Select tree, no
    // `disc == case` compares for the untaken cases. Mirrors `lower_if`'s own
    // const-elision (`stmt.rs`) exactly: gated on `nofold_depth == 0` (a
    // `@nofold`-scoped match must still build the real tree below) and reads
    // the scrutinee through `if_cond_const_ctx` (names actually spelled
    // `const`, never a plain `let` that merely happens to fold - see that
    // method's own doc comment), so a program using no `const` keyword
    // lowers identically to before. `try_const_decision` returning `Some`
    // means the scrutinee resolved; `lower_decision` on an already-terminal
    // `Leaf`/`Fail` node builds no Switch/Select at all.
    if ctx.nofold_depth == 0
        && let Some(leaf) = try_const_decision(ctx, &decision, scrutinee)
    {
        return lower_decision(ctx, &leaf, &root, arms, &result_ty, range);
    }

    lower_decision(ctx, &decision, &root, arms, &result_ty, range)
}

/// The record/enum-VALUE twin of [`lower_match_expr`]. A `match` whose arms are
/// records or enum values has no single wire to hand back, so it lowers to one
/// `Select` per LEAF FIELD instead of one per match - the same shape the
/// record-valued if-expr takes in `value_record_fields`, over the same decision
/// tree, in the same first-match-wins order, sharing `select_record_fields`.
///
/// `None` means "this match carries no record", which every caller treats as
/// "not a record source" and falls through on: a block-bodied arm (checked
/// BEFORE anything is lowered, so a match that cannot produce a value emits
/// nothing rather than half a Select tree), a scrutinee with no record
/// decomposition, or an arm body that resolves to no field map.
pub(super) fn lower_match_expr_record(
    ctx: &mut LowerCtx,
    scrutinee: &Expr,
    arms: &[MatchArm],
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    // A block-bodied arm carries no value at all (the statement form builds an
    // exec chain instead), so there is no per-field choice to make.
    if arms.iter().any(|a| matches!(a.body, MatchBody::Block(_))) {
        return None;
    }
    // Nor is there a store to confine an arm that acts to its own branch.
    if ctx.current_exec.is_some()
        && arms.iter().any(|a| matches!(&a.body, MatchBody::Expr(x) if has_effects(ctx, x)))
    {
        crate::lower::stmt::report_effectful_aggregate_choice(ctx, range);
        return None;
    }
    let scrut_ty = scrutinee_type(ctx, scrutinee);
    let arm_patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
    let decision = crate::lower::matchtree::build(ctx.enums(), &scrut_ty, &arm_patterns);
    let root = match_scrutinee_record(ctx, scrutinee)?;
    // Const-elision fast path, identical to `lower_match_expr`'s: a
    // compile-time-known `__disc` resolves straight to the taken leaf, so only
    // that arm lowers and no Select/compare is built for the rest.
    if ctx.nofold_depth == 0
        && let Some(leaf) = try_const_decision(ctx, &decision, scrutinee)
    {
        return lower_decision_record(ctx, &leaf, &root, arms, range);
    }
    lower_decision_record(ctx, &decision, &root, arms, range)
}

/// [`lower_decision`]'s per-field twin: walk one decision node into a field map
/// rather than a single port.
fn lower_decision_record(
    ctx: &mut LowerCtx,
    decision: &crate::lower::matchtree::Decision,
    root: &HashMap<crate::intern::Sym, Binding>,
    arms: &[MatchArm],
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    use crate::lower::matchtree::Decision;
    match decision {
        Decision::Leaf(i) => lower_match_arm_record(ctx, &arms[*i], root, range),
        // No arm matched. Unlike the scalar path there is no zero LITERAL to
        // fall back on - a record is several wires - and this node is only
        // reachable for a non-exhaustive match, already a WS054 error. It
        // contributes no fields, and the fold below makes the last case the
        // innermost value instead.
        Decision::Fail => None,
        Decision::Switch { path, cases, default } => {
            // One `__disc` read per Switch, like the scalar path. A Switch with
            // a single case and no default needs no test at all (nothing else
            // is reachable), so it reads no disc either.
            let disc_port = if cases.len() > 1 || default.is_some() {
                Some(
                    read_disc_at_path(ctx, root, path, range)
                        .unwrap_or_else(|| synthesise_unsupported_range(ctx, "an enum tag read", range)),
                )
            } else {
                None
            };
            // Fold RIGHT exactly as `lower_decision` does, so the arms are
            // tested in the same order.
            let mut acc = default
                .as_ref()
                .and_then(|d| lower_decision_record(ctx, d, root, arms, range));
            for (k, sub) in cases.iter().rev() {
                let sub_fields = lower_decision_record(ctx, sub, root, arms, range)?;
                acc = Some(match acc {
                    Some(else_fields) => {
                        // `disc_port` is read whenever a test can be needed,
                        // which is exactly when `acc` is already `Some`.
                        let dp = disc_port?;
                        let cond = emit_disc_eq(ctx, dp, *k, range);
                        crate::lower::stmt::select_record_fields(
                            ctx,
                            cond,
                            &sub_fields,
                            &else_fields,
                            range,
                        )
                    }
                    // The innermost fallback: no Select, no `disc == k` compare.
                    None => sub_fields,
                });
            }
            acc
        }
    }
}

/// [`lower_match_arm`]'s record twin: bind the arm's payload captures the same
/// way, then resolve its body as a per-field record instead of a single port.
fn lower_match_arm_record(
    ctx: &mut LowerCtx,
    arm: &MatchArm,
    root: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let MatchBody::Expr(expr) = &arm.body else {
        return None;
    };
    with_arm_captures(ctx, arm, root, |ctx| {
        crate::lower::stmt::choice_branch_fields(ctx, expr, range)
    })
}

/// The scrutinee's `__disc` + payload-slot `Binding::Record`, shared by
/// `lower_match_expr` and `lower_match_stmt`. A NAMED scrutinee resolves
/// through the scope (`resolve_field_chain`); an INLINE enum construction
/// (`match Shape.Circle(3.0) {...}`) resolves through no name, so its
/// construction is lowered to obtain the record (an enum ctor stashes it in
/// `pending_inline_record`, the exact path a `const s = Shape.Circle(3.0)`
/// binding takes - so `match <inline-ctor>` lowers to real gates instead of
/// the `_Unsupported` placeholder the bare `resolve_field_chain` guard emitted
/// for it). `None` for any scrutinee whose lowering yields no such record (an
/// enum INPUT port, whose lowering produces a scalar not a record; a
/// typecheck-error program) - the caller emits the loud `_Unsupported`.
pub(super) fn match_scrutinee_record(
    ctx: &mut LowerCtx,
    scrutinee: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    // A gate read is its first output, the enum it reads.
    if enum_gate_read(ctx, scrutinee).is_some() {
        return gate_read_enum_value(ctx, scrutinee);
    }
    if let Some(Binding::Record(root)) = resolve_field_chain(ctx, scrutinee).cloned() {
        return Some(root);
    }
    // Not a name resolving to a record. The shared record resolver covers every
    // other shape that carries one: an enum construction (through
    // `pending_inline_record`), a container element (`match m[k]`,
    // `match arr[i]`), a field path, a record-valued conditional. Binding the
    // same read to a name first always worked, so an inline scrutinee reaching
    // the placeholder was a gap in the resolution, not a limit of the layout.
    crate::lower::stmt::value_record_fields(ctx, scrutinee)
}

/// The enum value `e`, a gate read whose first output is an enum, gives a
/// place for one value: that output's tag and payload slots.
pub(super) fn gate_read_enum_value(ctx: &mut LowerCtx, e: &Expr) -> Option<HashMap<crate::intern::Sym, Binding>> {
    enum_gate_read(ctx, e)?;
    let read = crate::lower::stmt::value_record_fields(ctx, e)?;
    match crate::lower::context::held_output(ctx, &read, e.range(), OutputPick::First) {
        Some(Binding::Record(value)) => Some(value),
        Some(_) => None,
        // An enum container's read lowers to the enum value itself.
        None => crate::lower::context::enum_disc(&read).is_some().then_some(read),
    }
}

/// A gate read whose first output is an enum (`hs.pop()`, `m.get(k)`, a
/// call of a `mod` with several outputs, a name holding one) as a
/// scrutinee: that enum.
fn enum_gate_read(ctx: &LowerCtx, scrutinee: &Expr) -> Option<Type> {
    let (_, first) = ctx.gate_outputs(scrutinee.range())?.first()?;
    let first = unwrap_ref(first);
    matches!(first, Type::Enum { .. }).then(|| first.as_record())
}

/// The enum a `match`, `if let` or `let ... else` scrutinee is: its checked
/// type, or the enum a gate read reads first.
pub(super) fn scrutinee_type(ctx: &LowerCtx, scrutinee: &Expr) -> Type {
    enum_gate_read(ctx, scrutinee).unwrap_or_else(|| unwrap_ref(&ctx.type_of(scrutinee)))
}

/// Try to resolve `decision` to its taken terminal (`Leaf`/`Fail`) node using
/// a compile-time-known `scrutinee`, or `None` when the scrutinee isn't
/// const-evaluable at all (an ordinary runtime value) - the caller falls back
/// to the general Select/Branch tree. Shares `ctx.if_cond_const_ctx` with
/// `lower_if`'s elision, so the two agree on exactly what counts as constant.
pub(super) fn try_const_decision(
    ctx: &LowerCtx,
    decision: &crate::lower::matchtree::Decision,
    scrutinee: &Expr,
) -> Option<crate::lower::matchtree::Decision> {
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    let cx = ctx.if_cond_const_ctx(Some(&lookup));
    let lit = crate::const_eval::eval_expr(scrutinee, &cx, &mut budget).ok()?;
    Some(crate::lower::matchtree::resolve_const_leaf(decision, &lit))
}

/// Walk one [`crate::lower::matchtree::Decision`] node into wires, returning the
/// port carrying that sub-match's value.
fn lower_decision(
    ctx: &mut LowerCtx,
    decision: &crate::lower::matchtree::Decision,
    root: &HashMap<crate::intern::Sym, Binding>,
    arms: &[MatchArm],
    result_ty: &Type,
    range: &SourceRange,
) -> PortRef {
    use crate::lower::matchtree::Decision;
    match decision {
        Decision::Leaf(i) => lower_match_arm(ctx, &arms[*i], root, result_ty, range),
        // No arm matched (only reachable for a non-exhaustive match, already a
        // WS054): the zero/default of the result type on the innermost else path.
        Decision::Fail => {
            let lit = default_literal_for_var_type(result_ty).unwrap_or(Literal::Float(0.0));
            literal_node_range(ctx, range, result_ty.clone(), lit)
        }
        Decision::Switch { path, cases, default } => {
            let disc_port = read_disc_at_path(ctx, root, path, range)
                .unwrap_or_else(|| synthesise_unsupported_range(ctx, "an enum tag read", range));
            // Fold RIGHT: the final default (or `Fail` zero) supplies the
            // innermost `Select`'s `InputA`, and each case wraps another
            // `Select(disc == k, sub, acc)` around it.
            let mut acc = match default {
                Some(d) => lower_decision(ctx, d, root, arms, result_ty, range),
                None => lower_decision(ctx, &Decision::Fail, root, arms, result_ty, range),
            };
            for (k, sub) in cases.iter().rev() {
                let sub_port = lower_decision(ctx, sub, root, arms, result_ty, range);
                let cond_port = emit_disc_eq(ctx, disc_port, *k, range);
                acc = emit_select(ctx, cond_port, sub_port, acc, result_ty, range);
            }
            acc
        }
    }
}

/// Lower one arm body with its payload captures bound as compile-time moves of
/// the scrutinee's matching payload slots (a scoped bind, no gate).
fn lower_match_arm(
    ctx: &mut LowerCtx,
    arm: &MatchArm,
    root: &HashMap<crate::intern::Sym, Binding>,
    result_ty: &Type,
    range: &SourceRange,
) -> PortRef {
    with_arm_captures(ctx, arm, root, |ctx| match &arm.body {
        MatchBody::Expr(expr) => lower_expr(ctx, expr),
        // A block-bodied arm carries no value in expression position (typecheck
        // omits it from the arm-type join): yield the result type's zero rather
        // than an exec chain (the statement form builds one).
        MatchBody::Block(_) => {
            let lit = default_literal_for_var_type(result_ty).unwrap_or(Literal::Float(0.0));
            literal_node_range(ctx, range, result_ty.clone(), lit)
        }
    })
}

/// Run `f` in a block scope with the arm's payload captures bound as
/// compile-time moves of the scrutinee's matching slots (a scoped bind, no
/// gate). The scope is popped on the way out, so no arm can leak its captures
/// into the next one.
pub(super) fn with_arm_captures<R>(
    ctx: &mut LowerCtx,
    arm: &MatchArm,
    root: &HashMap<crate::intern::Sym, Binding>,
    f: impl FnOnce(&mut LowerCtx) -> R,
) -> R {
    ctx.push_scope(crate::scope::ScopeTag::BLOCK);
    let mut captures = Vec::new();
    collect_pattern_captures(&arm.pattern, &mut Vec::new(), &mut captures);
    for (name, slot_path) in captures {
        if let Some(binding) = navigate_capture(root, &slot_path) {
            ctx.scope.insert(&name, binding);
        }
    }
    let out = f(ctx);
    ctx.pop_scope();
    out
}

/// Collect `(capture name, slot path)` for every binding in `pattern`. Each path
/// is the sequence of payload-slot keys (`__{Variant}_{i}` / `__{Variant}_{f}`,
/// matching the keys `build_enum_fields` lays down) from the scrutinee root to
/// the captured slot. An empty path names the whole scrutinee value.
///
/// `pub(crate)` (not `pub(super)`) so `const_eval::expr`'s own `MatchExpr` arm
/// can bind the same captures against a compile-time LITERAL scrutinee
/// (`matchtree::navigate_capture_literal`) that this function's other callers
/// bind against a lowered `Binding::Record` (`navigate_capture` below) -- one
/// path-collection rule for both.
pub(crate) fn collect_pattern_captures(
    pattern: &Pattern,
    prefix: &mut Vec<String>,
    out: &mut Vec<(String, Vec<String>)>,
) {
    match pattern {
        Pattern::Wildcard(_) => {}
        Pattern::Binding { name, .. } => out.push((name.clone(), prefix.clone())),
        Pattern::Variant { variant, sub, .. } => match sub {
            VariantPattern::Unit => {}
            VariantPattern::Positional(subs) => {
                for (i, sp) in subs.iter().enumerate() {
                    prefix.push(format!("__{variant}_{i}"));
                    collect_pattern_captures(sp, prefix, out);
                    prefix.pop();
                }
            }
            VariantPattern::Named { fields, .. } => {
                for (fname, sp) in fields {
                    prefix.push(format!("__{variant}_{fname}"));
                    collect_pattern_captures(sp, prefix, out);
                    prefix.pop();
                }
            }
        },
    }
}

/// Navigate the scrutinee record along `path` to the captured slot's binding,
/// cloned for a compile-time move into the arm scope. An empty path binds the
/// whole scrutinee record; a missing or non-record intermediate yields `None`.
pub(super) fn navigate_capture(root: &HashMap<crate::intern::Sym, Binding>, path: &[String]) -> Option<Binding> {
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        let binding = cur.get(&crate::intern::intern(seg))?;
        if i + 1 == path.len() {
            return Some(binding.clone());
        }
        match binding {
            Binding::Record(sub) => cur = sub,
            _ => return None,
        }
    }
    Some(Binding::Record(root.clone()))
}

/// Read the `__disc` of the sub-record reached by `path` (navigate the record
/// via each `PathStep::Field`, then its `__disc`) as a value port.
pub(super) fn read_disc_at_path(
    ctx: &mut LowerCtx,
    root: &HashMap<crate::intern::Sym, Binding>,
    path: &[crate::lower::matchtree::PathStep],
    range: &SourceRange,
) -> Option<PortRef> {
    let mut cur = root;
    for crate::lower::matchtree::PathStep::Field(f) in path {
        match cur.get(&crate::intern::intern(f)) {
            Some(Binding::Record(sub)) => cur = sub,
            _ => return None,
        }
    }
    let disc = cur.get(&crate::intern::intern("__disc"))?.clone();
    binding_to_port(ctx, &disc, range)
}

/// A `disc == k` equality gate (`CompareEqual`), with `k` a literal int source.
pub(super) fn emit_disc_eq(ctx: &mut LowerCtx, disc_port: PortRef, k: i64, range: &SourceRange) -> PortRef {
    let k_port = literal_node_range(ctx, range, Type::Int, Literal::Int(k));
    let cmp = ctx.add_gate(AddNodeOpts {
        gate_class: gc::COMPARE_EQUAL,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::new(*sym::INPUT_A, Type::Int),
                PortSpec::new(*sym::INPUT_B, Type::Int),
            ],
            outputs: vec![PortSpec::new(*sym::B_OUTPUT, Type::Bool)],
        },
        note: Some("match disc compare".into()),
        ..Default::default()
    });
    ctx.connect(disc_port, cmp.port(WirePort::InputA));
    ctx.connect(k_port, cmp.port(WirePort::InputB));
    cmp.port(WirePort::BOutput)
}

/// One `Select` gate wired exactly like [`lower_if_expr`]: `then`->`InputB`,
/// `else`->`InputA`, `cond`->`bSelectB` (true picks `InputB`).
fn emit_select(
    ctx: &mut LowerCtx,
    cond_port: PortRef,
    then_port: PortRef,
    else_port: PortRef,
    result_ty: &Type,
    range: &SourceRange,
) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::SELECT,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::new(*sym::INPUT_A, result_ty.clone()),
                PortSpec::new(*sym::INPUT_B, result_ty.clone()),
                PortSpec::new(*sym::B_SELECT_B, Type::Bool),
            ],
            outputs: vec![PortSpec::new(*sym::OUTPUT, result_ty.clone())],
        },
        note: Some("match-expr select".into()),
        ..Default::default()
    });
    ctx.connect(cond_port, node_id.port(WirePort::BSelectB));
    ctx.connect(then_port, node_id.port(WirePort::InputB));
    ctx.connect(else_port, node_id.port(WirePort::InputA));
    node_id.port(WirePort::Output)
}

/// Compile-time arm of the string → bool coercion: a String literal baked
/// into a Bool destination converts to `Literal::Bool(!s.is_empty())`.
/// CONSISTENCY LAW: this must equal the runtime semantics of the
/// `CompareNotEqual(s, "")` gate that `LowerCtx::wrap_string_to_bool`
/// inserts on the WIRE path — both are exactly `s != ""` (empty false,
/// everything else — including "0" and "false" — true). A String literal
/// left raw on a Bool destination would either miscompile (a Bool
/// `InitialValue` read through the gate's NATIVE content-aware truthiness,
/// where "0"/"false" are falsy) or crash emit (UnimplementedCast on a Bool
/// data field). Non-(String → Bool) pairs pass through untouched.
pub(super) fn bake_string_bool(lit: Literal, ty: &Type) -> Literal {
    match (&lit, ty) {
        (Literal::String(s), Type::Bool) => Literal::Bool(!s.is_empty()),
        _ => lit,
    }
}

/// Bake a scalar `InitialValue` literal in its DECLARED type's shape, for the
/// storage slots whose wire-variant member is chosen by the literal's own kind
/// (a `Pseudo_Var`'s `WireVariant` — see `emit::module`'s `Pseudo_Var` arm,
/// which only falls back to the port type when there is NO literal). Without
/// this, `var x: float = 0` bakes `Int(0)` and ships an INTEGER variable for a
/// `float` declaration; the annotation is silently lost.
///
/// bool/int/float coerce bidirectionally in the type system
/// (`types::coerce::is_prim_number`), so all six ordered pairs reach here.
/// The conversions match the element-wise table `emit::variants`'s
/// `wire_array_variant_from_literals` already applies to ARRAY initializers,
/// so `var a: int[] = [1.5]` and `var s: int = 1.5` agree: float → int
/// truncates, bool → number is 1/0, and number → bool is `!= 0`. String → bool
/// keeps its `!= ""` law via [`bake_string_bool`]. Every other pair — and every
/// non-scalar type — passes through untouched.
pub(super) fn bake_literal_for_type(lit: Literal, ty: &Type) -> Literal {
    match (&lit, ty) {
        (Literal::Int(_), Type::Float) | (Literal::Float(_), Type::Int) => crate::const_eval::number_in_type(lit, ty),
        (Literal::Bool(b), Type::Float) => Literal::Float(if *b { 1.0 } else { 0.0 }),
        (Literal::Bool(b), Type::Int) => Literal::Int(*b as i64),
        (Literal::Int(n), Type::Bool) => Literal::Bool(*n != 0),
        (Literal::Float(f), Type::Bool) => Literal::Bool(*f != 0.0),
        _ => bake_string_bool(lit, ty),
    }
}

/// `config_only` marks a port that has NO wire pin at all — a settings-menu
/// data field (`crate::catalog::is_wire_input(...) == false`), such as
/// `SendCustomEvent`'s `EventName`. It unlocks a final fallback through the
/// full const evaluator (`const_eval::eval_expr`), which is what typecheck's
/// `config::validate_scalar_config_arg` accepts for exactly these ports —
/// including a `const mod` call (`evtName("died")`) and a certified method
/// call (`"died".ToUpper()`). Without it the two disagree: typecheck says the
/// value is constant, lowering folds nothing, and the gate ships with the
/// field unset while `builtin.rs`'s non-wire-port guard silently drops the
/// argument.
///
/// It must stay OFF for every wire-capable port. `eval_expr` evaluates
/// operators, and folding them here would delete real gates a program
/// depends on — `Rotation(0.0 + 0.0, …)` must keep its MathAdd (see the
/// env-less-fold note below). A config-only port has no such hazard: there is
/// no wire for a gate to feed, so the value either bakes or is dropped.
///
/// Deliberately NOT extended to the composite (`MeshColors`/
/// `WeaponAmmoOverride`) or data-driven config paths, which keep the narrow
/// evaluator on BOTH sides — their typecheck validators
/// (`validate_composite_config_arg` / `validate_data_driven_config`) were
/// left on `expr_to_literal_in`, so widening only the lowering half would
/// re-open this same accept-but-drop gap in the opposite direction.
pub(super) fn literal_for_property_port(
    ctx: &LowerCtx,
    e: &Expr,
    port_ty: &Type,
    config_only: bool,
) -> Option<Literal> {
    // Return the literal without type promotion — the emit layer handles the
    // native type (i32/f64/str) from the data struct schema — EXCEPT the
    // string → bool coercion, which must apply its `!= ""` law at compile
    // time here (see `bake_string_bool`): emit has no String→bool cast, and
    // a raw String on a Bool data field is an UnimplementedCast crash.
    //
    // The env-less fold first: bare literals, negated literals, and constructor calls on constant
    // args. Deliberately does not evaluate operators (`expr_to_literal`'s own
    // doc comment: folding `0.0 + 0.0` here would delete the real MathAdd
    // gate a program like `Rotation(0.0 + 0.0, ...)` must keep) and does not
    // resolve names (an env is required for that — see below).
    let lit = expr_to_literal(e).or_else(|| {
        // A bare name referencing a scoped-or-top-level `let` constant (e.g.
        // `let pf = $./foo.brz` ... `SpawnPrefab(prefab = pf)`), resolved via
        // `ctx.const_lookup()` (the module's constants and those of every open
        // frame, as the scope resolves them; see `crate::const_scope`).
        // Restricted to a plain `Ident` (never a compound expression — that
        // would reintroduce the operator-folding hazard above) and to
        // non-composite literal kinds: a named Vector/Rotator/Quat/Color
        // constant still takes the WIRE path exactly as before. Those four
        // are the ones `inlinable` below treats specially (only inlining
        // when `port_accepts_inline_variant` allows it) precisely because
        // most consumers need a real wired Make* gate; resolving them here
        // would let a receiver like `dir.RotationByAngle(...)` (`let dir =
        // Vec(...)`) skip that wire entirely, silently pruning the producing
        // gate as a "wireless orphan" whenever its own result also goes
        // unused. Scalars (Int/Float/Bool/String) and prefab references
        // (`PrefabRef`/`NestedPrefab`) have no such wired-producer nuance —
        // `literal_for_property_port` already inlines them unconditionally
        // when they arrive as bare literals, so resolving them by name here
        // is consistent, not a new capability class.
        let Expr::Ident { name, .. } = e else {
            return None;
        };
        // A constant array/map is a container, not a data value: it takes
        // the wire path, which reaches it through its materialized gate.
        match ctx.const_lookup().get(name).cloned() {
            Some(
                Literal::Vector { .. }
                | Literal::Rotator { .. }
                | Literal::Quat { .. }
                | Literal::LinearColor { .. }
                | Literal::Array(_)
                | Literal::Map(_),
            ) => None,
            other => other,
        }
    })
    .or_else(|| {
        // Config-only port: match typecheck's acceptance exactly (see this
        // function's doc comment). Runs LAST, so every expression the two
        // narrow paths above already handled keeps its existing result
        // byte-for-byte — this can only add a literal where lowering
        // previously produced none.
        if !config_only {
            return None;
        }
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()
    });
    // A number constant typecheck held in the other number type (event data
    // a receiver declares) is that number.
    let lit = lit.map(|lit| match lit {
        Literal::Int(_) | Literal::Float(_) => constant_literal(ctx, e, lit).1,
        other => other,
    });
    lit.map(|lit| bake_string_bool(lit, port_ty))
}

pub(super) fn synthesise_unsupported(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    let port = synthesise_unsupported_no_warn(ctx, e.range());
    ctx.warn(format!("{}: {}", crate::diagnostic::PLACEHOLDER, unsupported_what(e)), e.range());
    port
}

pub(super) fn synthesise_unsupported_range(ctx: &mut LowerCtx, what: &str, range: &SourceRange) -> PortRef {
    let port = synthesise_unsupported_no_warn(ctx, range);
    ctx.warn(format!("{}: {what}", crate::diagnostic::PLACEHOLDER), range);
    port
}

/// What a placeholder stands for, as its diagnostic names it.
fn unsupported_what(e: &Expr) -> String {
    match e {
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::FieldAccess { field, .. } => format!("the method call `.{field}()`"),
            Expr::Ident { name, .. } => format!("the call `{name}(..)`"),
            _ => "a call".to_string(),
        },
        Expr::Ident { name, .. } => format!("a read of `{name}`"),
        Expr::FieldAccess { field, .. } => format!("the field read `.{field}`"),
        Expr::TuplePick { index, .. } => format!("the tuple element read `.{index}`"),
        Expr::IndexAccess { .. } => "an index read".to_string(),
        Expr::BinOp { op, .. } | Expr::UnOp { op, .. } => format!("the `{op}` operation"),
        Expr::IfExpr { .. } => "an `if` expression".to_string(),
        Expr::MatchExpr { .. } => "a `match` expression".to_string(),
        Expr::InterpLit { .. } => "a string interpolation".to_string(),
        Expr::RecordLit { .. } => crate::diagnostic::LITERAL_AS_ONE_VALUE.to_string(),
        Expr::Array { .. } => "an array literal".to_string(),
        Expr::MapLit { .. } => "a map literal".to_string(),
        _ => "this expression".to_string(),
    }
}

/// The `_Unsupported` placeholder gate WITHOUT the generic WSP001 warning, for
/// callers that report a specific diagnostic of their own. The node still has to
/// exist: its consumer already holds a `PortRef` to wire.
pub(super) fn synthesise_unsupported_no_warn(
    ctx: &mut LowerCtx,
    range: &SourceRange,
) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNSUPPORTED,
        source_range: range.clone(),
        ports: GateIO::source(Type::Any),
        note: Some("unsupported expression".into()),
        ..Default::default()
    });
    node_id.port(WirePort::Output)
}

/// Whether lowering `e` on an exec chain does anything beyond producing its
/// value: runs a statement or a `match` block arm (which leaves through a
/// `return`), takes the chain point (`emit` as a value), runs an action
/// builtin (`CallSpec::has_effect`) or a container mutation, or calls a `mod`/`chip` whose body writes outside
/// itself (an output it does not own, a variable it did not declare, a
/// `static var`, a container, an `await`).
pub(super) fn has_effects(ctx: &LowerCtx, e: &Expr) -> bool {
    effects_below(ctx, walk::Node::Expr(e), None, &mut HashSet::default())
}

/// What a `mod`/`chip` body may write without an effect outside it: the names
/// it declares and its own outputs.
struct BodyScope<'a> {
    locals: HashSet<&'a str>,
    outputs: Vec<&'a str>,
}

fn effects_below<'a>(
    ctx: &LowerCtx,
    node: walk::Node<'a>,
    body: Option<&BodyScope<'a>>,
    seen: &mut HashSet<usize>,
) -> bool {
    if node_has_effect(ctx, node, body, seen) {
        return true;
    }
    let mut found = false;
    walk::children(node, &mut |child, flow| {
        // A nested handler or declaration does not run when the call does.
        if found || matches!(flow, walk::Flow::Handler | walk::Flow::Decl) {
            return;
        }
        found = effects_below(ctx, child, body, seen);
    });
    found
}

fn node_has_effect<'a>(
    ctx: &LowerCtx,
    node: walk::Node<'a>,
    body: Option<&BodyScope<'a>>,
    seen: &mut HashSet<usize>,
) -> bool {
    let local = |name: &str| body.is_some_and(|b| b.locals.contains(name));
    match (node, body) {
        // In an expression, any statement runs on the chain: a `match` block
        // arm, or a block expression's statement other than a `let`.
        (walk::Node::Block(_), None) => true,
        (walk::Node::Stmt(s), None) => !matches!(s, Stmt::Let(_)),
        (walk::Node::Expr(Expr::CurrentExec { .. }), None) => true,
        (walk::Node::Stmt(s), Some(b)) => match s {
            Stmt::Emit(Emit { name, .. }) | Stmt::OutBinding(OutBinding { name, .. }) => {
                !b.outputs.contains(&name.as_str())
            }
            Stmt::Assign(a) => match crate::typecheck::infer::flatten_access(&a.target).0 {
                Expr::Ident { name, .. } => !local(name),
                _ => true,
            },
            Stmt::Var(v) => v.is_static,
            Stmt::Await(_) => true,
            _ => false,
        },
        (walk::Node::Expr(Expr::Call { callee, .. }), _) => match callee.as_ref() {
            Expr::Ident { name, range } => match ctx.call_target_chip(name, &range.file) {
                Some(decl) => body_has_effects(ctx, decl, seen),
                None => crate::catalog::calls::find_call(name).is_some_and(|c| c.has_effect()),
            },
            Expr::FieldAccess { obj, field, range } => {
                let mutates = crate::catalog::arrays::array_method(field).is_some_and(|m| m.mutates)
                    || crate::catalog::maps::map_method(field).is_some_and(|m| m.mutates);
                if mutates {
                    return !matches!(crate::typecheck::infer::flatten_access(obj).0, Expr::Ident { name, .. } if local(name));
                }
                if crate::catalog::calls::find_call(field).is_some_and(|c| c.receiver.is_some() && c.has_effect()) {
                    return true;
                }
                let decl = match obj.as_ref() {
                    Expr::Ident { name, .. } => ctx.lookup_ns_chip(name, field),
                    _ => None,
                }
                .or_else(|| ctx.self_mod_chip(field, &range.file));
                decl.is_some_and(|d| body_has_effects(ctx, d, seen))
            }
            _ => false,
        },
        _ => false,
    }
}

fn body_has_effects(ctx: &LowerCtx, decl: &ChipDecl, seen: &mut HashSet<usize>) -> bool {
    if !seen.insert(decl as *const ChipDecl as usize) {
        return false;
    }
    let scope = body_scope(decl, &[]);
    effects_below(ctx, walk::Node::Block(&decl.body), Some(&scope), seen)
}

/// A call in an arm of an `if` or `match` expression on an exec chain to a
/// `mod` or `chip` with an `on` handler of its own that acts (writes outside
/// the body, sends, mutates a container): that handler runs on its own
/// trigger whether or not the arm is taken, and the arm cannot gate it.
/// Warned once per call (`WS102`). A nested `if` or `match` in the arm warns
/// for its own arms when it is lowered.
pub(super) fn warn_handlers_ungated_by_arm(ctx: &mut LowerCtx, arm: walk::Node<'_>) {
    let mut found: Vec<(String, String, SourceRange)> = Vec::new();
    let mut visit = |n: walk::Node<'_>| {
        let walk::Node::Expr(Expr::Call { callee, range, .. }) = n else { return };
        let decl = match callee.as_ref() {
            Expr::Ident { name, range } => ctx.call_target_chip(name, &range.file),
            Expr::FieldAccess { obj, field, .. } => match obj.as_ref() {
                Expr::Ident { name, .. } => ctx.lookup_ns_chip(name, field),
                _ => None,
            },
            _ => None,
        };
        let Some(decl) = decl else { return };
        if let Some(trigger) = acting_handler(ctx, decl) {
            found.push((decl.name.clone(), trigger, range.clone()));
        }
    };
    visit(arm);
    walk::any_below(arm, |f| matches!(f, walk::Flow::Seq | walk::Flow::AnonChip), &mut |n| {
        visit(n);
        false
    });
    for (name, trigger, range) in found {
        if ctx.diagnostics.iter().any(|d| d.code == "WS102" && d.range == range) {
            continue;
        }
        ctx.warn_code(
            "WS102",
            format!(
                "`{name}` runs its `on {trigger}` handler whenever `{trigger}` fires, whether or not this \
                 branch is taken: the condition only chooses which value is read. To run it only on this \
                 branch, call it from an `if` statement whose branch fires its trigger"
            ),
            &range,
        );
    }
}

/// The trigger of an `on` handler in `decl`'s body that acts, if any. Only the
/// handler's own locals and `decl`'s outputs are its to write: a `var` or
/// container the body declares outside the handler keeps what the handler
/// writes for the next trigger, so writing one acts.
fn acting_handler(ctx: &LowerCtx, decl: &ChipDecl) -> Option<String> {
    let key = decl as *const ChipDecl as usize;
    if let Some(known) = ctx.acting_handler_memo.borrow().get(&key) {
        return known.clone();
    }
    let mut trigger = None;
    walk::any_below(walk::Node::Block(&decl.body), walk::Flow::on_path, &mut |m| {
        let walk::Node::Stmt(Stmt::Handler(h)) = m else { return false };
        let mut locals = HashSet::default();
        collect_locals(&h.body, &mut locals);
        let scope = BodyScope { locals, outputs: decl.outputs.iter().map(|o| o.name.as_str()).collect() };
        if effects_below(ctx, walk::Node::Block(&h.body), Some(&scope), &mut HashSet::default()) {
            trigger = Some(trigger_text(&h.trigger));
            return true;
        }
        false
    });
    ctx.acting_handler_memo.borrow_mut().insert(key, trigger.clone());
    trigger
}

/// How a handler's trigger is written.
fn trigger_text(t: &crate::ast::Trigger) -> String {
    use crate::ast::Trigger;
    match t {
        Trigger::Ident { name, .. } => name.clone(),
        Trigger::Field { obj, field, .. } => format!("{obj}.{field}"),
        Trigger::Not { inner, .. } => format!("!{}", trigger_text(inner)),
        Trigger::Union { parts, .. } => parts.iter().map(trigger_text).collect::<Vec<_>>().join(" | "),
    }
}

/// What `decl`'s body, and the nested handler bodies in `handlers`, may
/// write without an effect outside it.
fn body_scope<'a>(decl: &'a ChipDecl, handlers: &[&'a Block]) -> BodyScope<'a> {
    let mut locals: HashSet<&str> = HashSet::default();
    for block in std::iter::once(&decl.body).chain(handlers.iter().copied()) {
        collect_locals(block, &mut locals);
    }
    BodyScope { locals, outputs: decl.outputs.iter().map(|o| o.name.as_str()).collect() }
}

/// The names `block` declares, outside its nested handlers and declarations.
fn collect_locals<'a>(block: &'a Block, locals: &mut HashSet<&'a str>) {
    walk::walk(walk::Node::Block(block), walk::Ctx::default(), &mut |n, _| {
        match n {
            walk::Node::Stmt(Stmt::Var(v)) if !v.is_static => {
                locals.insert(v.name.as_str());
            }
            walk::Node::Stmt(Stmt::Array(a)) => {
                locals.insert(a.name.as_str());
            }
            walk::Node::Stmt(Stmt::Map(m)) => {
                locals.insert(m.name.as_str());
            }
            walk::Node::Stmt(Stmt::Let(l)) => match &l.binding {
                LetBinding::Ident { name, .. } => {
                    locals.insert(name.as_str());
                }
                LetBinding::Tuple { names, rest, .. } => {
                    locals.extend(names.iter().map(String::as_str));
                    locals.extend(rest.as_deref());
                }
                LetBinding::Record { names, .. } => locals.extend(names.iter().map(String::as_str)),
                LetBinding::RecordDestruct { fields, .. } => {
                    for f in fields {
                        match f {
                            RecordDestructField::Named { name, alias, .. } => {
                                locals.insert(alias.as_deref().unwrap_or(name));
                            }
                            RecordDestructField::Rest { name, .. } => {
                                locals.insert(name.as_str());
                            }
                        }
                    }
                }
            },
            _ => {}
        }
        !matches!(n, walk::Node::Stmt(Stmt::Handler(_) | Stmt::ChipDecl(_)))
    });
}
