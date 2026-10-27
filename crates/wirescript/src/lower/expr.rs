use super::*;
use crate::ast::walk;
use crate::lower::matchtree::{Decision, PathStep, ValueCheck};

// Expressions

pub(super) fn lower_expr(ctx: &mut LowerCtx<'_>, e: &Expr) -> PortRef {
    // Enum construction (unit, positional or named) has no single wire: like a
    // record literal it lowers to a `Binding::Record` (see `try_lower_enum_ctor`).
    // Checked once here so the `FieldAccess`/`Call`/`VariantCtor` shapes share
    // one enum-name, shadow and registry guard.
    if let Some(port) = try_lower_enum_ctor(ctx, e) {
        return port;
    }
    // `Enum.FromInt(n)` (tag-only construction) and `<enum value>.ToInt()`
    // (`.Discriminant` alias) go ahead of ordinary call lowering, which would
    // treat `FromInt`/`ToInt` as unknown methods and emit `_Unsupported`.
    if let Some(port) = try_lower_enum_from_int(ctx, e) {
        return port;
    }
    if let Some(port) = try_lower_enum_to_int(ctx, e) {
        return port;
    }
    // `EnumToInt(value)` and `IntToEnum(value, wrap?)`, the gate-backed twins of
    // `.ToInt()` and `Enum.FromInt(n)`: a compile-time-known value folds with no
    // gate, a runtime value goes through the real game gate.
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
        // `null` lowers to its resolved type's zero value (typecheck recorded
        // the type via `check_null`): an unset object, `0`, `false`, `""` and so on.
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
        // `$Type/Name` as a value (`weapon == $Item/Foo`) becomes the matching
        // `*Reference` gate, which holds the asset in its `Asset` field and
        // outputs it as an `entity` wire: assets cannot be inlined into
        // wire-variant ports such as Compare `InputB`. No gate takes a component
        // type; a save stores one as the external asset `BrickComponentTypeBase/<class>`.
        Expr::AssetRef {
            asset_type, range, ..
        } if asset_type == crate::analysis::assets::COMPONENT => synthesise_unsupported_range(
            ctx,
            "a component type reference, which no gate takes yet",
            range,
        ),
        Expr::AssetRef {
            asset_type,
            asset_name,
            range,
        } => {
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
            if name == "_"
                && let Some(port) = ctx.await_armed_port
            {
                return port;
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
                    format!("'*{name}' deref requires exec context; use .Value for pure reads"),
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
                        held_output(ctx, rec, range, OutputPick::First)
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
            match resolve_unsafe_slot(ctx, inner)
                .cloned()
                .and_then(|b| binding_to_port(ctx, &b, range))
            {
                Some(port) => port,
                None => synthesise_unsupported_range(ctx, "an `unsafe` read", range),
            }
        }
        Expr::Is { value, path, range } => {
            let compare = Expr::variant_test_desugared(value, path, range);
            lower_expr(ctx, &compare)
        }
        Expr::As { value, .. } => convert::lower_as(ctx, value, e),
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
            if ctx.current_exec.is_some()
                && (has_effects(ctx, then_branch) || has_effects(ctx, else_branch))
            {
                return lower_if_value(ctx, cond, then_branch, else_branch, range, e);
            }
            lower_if_expr(ctx, cond, then_branch, else_branch, range)
        }
        Expr::MatchExpr {
            scrutinee,
            arms,
            range,
        } => {
            // A value `match` with a block arm (one that leaves through a
            // `return`, typecheck's WS095), or an arm that does more than give
            // its value, threads exec through the statement form's Branch/Union
            // tree, so each arm runs only when it matches; its value arms store
            // into a scratch var. A `match` statement reaches `lower_match_stmt`
            // from `Stmt::ExprStmt`.
            if ctx.current_exec.is_some() {
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(x) => {
                            warn_handlers_ungated_by_arm(ctx, walk::Node::Expr(x));
                        }
                        MatchBody::Block(b) => {
                            warn_handlers_ungated_by_arm(ctx, walk::Node::Block(b));
                        }
                    }
                }
            }
            if ctx.current_exec.is_some()
                && arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(|g| has_effects(ctx, g))
                        || match &a.body {
                            MatchBody::Block(_) => true,
                            MatchBody::Expr(x) => has_effects(ctx, x),
                        }
                })
            {
                return lower_match_value(ctx, scrutinee, arms, range, e);
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
        Expr::MapLit { range, .. } => {
            synthesise_unsupported_range(ctx, "a map literal where one value goes", range)
        }
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
        // A well-formed `VariantCtor` is intercepted by `try_lower_enum_ctor`
        // above, so here `path` is not a resolvable `Enum.Variant` and typecheck
        // has reported an error. Lowering still runs on such a program, so this
        // is a placeholder rather than a panic.
        Expr::VariantCtor { range, .. } => {
            synthesise_unsupported_range(ctx, "an enum construction", range)
        }
        _ => synthesise_unsupported(ctx, e),
    }
}

/// Bare-name variant resolution through the shared
/// `typecheck::enums::resolve_bare_variant_enum`, with lower's shadow predicate:
/// a name in `scope` (value bindings and `mod`/`chip` symbols), an enum name, a
/// `const` parameter or a `resolve_mod` hit shadows the variant. Type-only
/// shadows (a `type` alias, a namespace) are typecheck errors in value position.
fn resolve_bare_variant_enum(ctx: &LowerCtx<'_>, name: &str) -> Option<String> {
    crate::typecheck::enums::resolve_bare_variant_enum(&ctx.enum_defs, name, |n| {
        ctx.scope.get(n).is_some()
            || ctx.constant(n, crate::const_scope::Admit::All).is_some()
            || ctx.enum_defs.expression_path(n).is_some()
            || ctx.resolve_mod(n).is_some()
    })
    .map(|def| def.name.clone())
}

/// Recognizes `e` as an enum construction (unit, positional or named, qualified
/// or bare) of a known, unshadowed enum; a bare `Some(x)` resolves like
/// `Option.Some(x)`. Builds the `Binding::Record` via [`lower_enum_ctor`],
/// stashes it in `ctx.pending_inline_record` for a record-shaped consumer (as a
/// record-returning call does), and returns the `__disc` port. `None` otherwise.
fn try_lower_enum_ctor(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
    // `enum_head` rejects a head a value symbol shadows (lower's `scope` holds
    // no type names, so any hit is a shadow); a shadowed head is no construction.
    let qualified = |obj: &Expr, field: &str| -> Option<(String, String)> {
        let name = ctx.enum_head(obj)?;
        Some((name.to_string(), field.to_string()))
    };
    let (enum_name, variant) = match e {
        Expr::FieldAccess { obj, field, .. } => qualified(obj, field)?,
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::FieldAccess { obj, field, .. } => qualified(obj, field)?,
            // Bare positional construction (`Some(42)`): `name` is the variant,
            // resolved against the whole registry with its own shadow guard.
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
    // A generic enum constructs like a non-generic one: the runtime layout
    // (`__disc` plus per-variant payload slots) is the same once typecheck has
    // inferred its type arguments. An unknown variant falls through to ordinary
    // field/call lowering.
    let def = ctx.enum_defs.expression_path(&enum_name)?;
    if !def.variants.iter().any(|v| v.name == variant) {
        return None;
    }

    // Each payload value is fixed at its value before a later one runs code
    // that may change what it reads.
    let mut ord = ordered::Ordered::default();
    let mut payload_value = |ctx: &mut LowerCtx<'_>, name: String, v: &Expr| {
        let slot = ord.source(ctx, &name, None, v.range(), |ctx| {
            Some(lower_payload_value(ctx, v))
        });
        (name, slot)
    };
    let payload: Vec<(String, Option<usize>)> = match e {
        // Keyed by index among positional args, the scheme `static_enum_ctor`
        // in predeclare.rs uses, so both producers of the `__{V}_{i}` slot key
        // agree.
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
                RecordLitField::Named { name, value, .. } => {
                    Some(payload_value(ctx, name.clone(), value))
                }
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
    let disc_port = match fields.get(&*DISC) {
        Some(Binding::Local(l)) => Some(l.port),
        _ => None,
    };
    ctx.pending_inline_record = Some(fields);
    disc_port
}

/// Lowers one enum payload value to the binding its slot needs. A record-typed
/// or nested-enum slot spans several ports (`enum_payload_slot`), so it takes a
/// `Binding::Record` from `value_record_fields`; a single port matches no arm in
/// `assign_record_fields` and the write is silently dropped. Only a non-scalar
/// value is probed, and a call the probe lowered is kept, so a payload runs once
/// (`S.B(a.pop().Value)` pops once).
fn lower_payload_value(ctx: &mut LowerCtx<'_>, value: &Expr) -> Binding {
    if record_valued(ctx, value) {
        if let Some(fields) = value_record_fields(ctx, value) {
            return Binding::Record(fields);
        }
        if let Some(port) = take_value_record_port(ctx, value) {
            return Binding::Local(LocalRecord { port });
        }
    }
    Binding::Local(LocalRecord {
        port: lower_expr(ctx, value),
    })
}

/// `Enum.FromInt(n)`: a tag-only construction whose `__disc` is the runtime int
/// `n`, with every variant's payload slots at their zero value (via
/// [`build_enum_fields`]), stashed in `pending_inline_record`; returns the
/// `__disc` port. Recognized only for a known, unshadowed enum with no variant
/// named `FromInt`, matching `typecheck::infer` and `const_eval::expr`.
fn try_lower_enum_from_int(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
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
/// `None` when the receiver is not an enum value or for any other shape, and
/// the caller falls through to ordinary call lowering.
fn try_lower_enum_to_int(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
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
    lower_discriminant(ctx, obj, range)
}

/// `EnumToInt(value)`: the gate-backed twin of `<enum value>.ToInt()`. A
/// compile-time-known enum (a variant literal such as `Shape.Circle(1.0)`, or a
/// `const` enum) folds to its discriminant literal with no gate. A runtime value
/// emits the `EXPR_ENUM_TO_INTEGER` gate fed by its `__disc` port, so game and
/// native enums route through the real gate. `None` for any other shape or when
/// a user chip/mod shadows the name.
fn try_lower_enum_to_integer(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident {
        name,
        range: name_range,
    } = callee.as_ref()
    else {
        return None;
    };
    if name != "EnumToInt" {
        return None;
    }
    // A user chip/mod of the same name shadows the builtin, as in `lower_call`.
    if ctx.call_target_chip(name, &name_range.file).is_some() {
        return None;
    }
    let Some(CallArg::Positional(value)) = args.first() else {
        return None;
    };
    // Fold path: const-eval folds the whole `EnumToInt(<const enum>)` call to
    // its discriminant, baked as a literal with no gate.
    if let Ok(Literal::Int(n)) = ctx.const_eval(e, ConstScope::Plain) {
        return Some(literal_node_range(
            ctx,
            e.range(),
            Type::Int,
            Literal::Int(n),
        ));
    }
    // Runtime path: the value's `__disc` port feeds `EXPR_ENUM_TO_INTEGER`. A
    // value with no record decomposition (an already-errored program) feeds its
    // own single port instead.
    let disc_port =
        lower_discriminant(ctx, value, e.range()).unwrap_or_else(|| lower_expr(ctx, value));
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

/// `IntToEnum(value, wrap?)`: the gate-backed twin of `Enum.FromInt(n)`, building
/// the same record for the enum type typecheck pinned at the use site. A
/// constant `value` with no `wrap` binds `__disc` to the literal with no gate;
/// otherwise `__disc` is the output of an `EXPR_INTEGER_TO_ENUM` gate.
///
/// `None` for any other shape, a name a user chip/mod shadows, or an expected
/// type that is not a known enum (an already-errored program).
fn try_lower_integer_to_enum(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
    let Expr::Call { callee, args, .. } = e else {
        return None;
    };
    let Expr::Ident {
        name,
        range: name_range,
    } = callee.as_ref()
    else {
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
    // An explicit `wrap` clamps an out-of-range tag, which only the game gate
    // does, so a `wrap` call routes through the gate even with a constant int.
    let has_wrap = args.iter().enumerate().any(|(i, a)| match a {
        CallArg::Named { name, .. } => name == "wrap",
        CallArg::Positional(_) => i >= 1,
        CallArg::Spread(_) => false,
    });
    // Fold path: a compile-time-constant int (and no `wrap`) sources `__disc`
    // directly, no gate.
    let const_int = if has_wrap {
        None
    } else {
        match ctx.const_eval(value, ConstScope::Plain) {
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
        let wrap_port = args.iter().enumerate().find_map(|(i, a)| match a {
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

/// The `Binding::Record` for constructing `variant` of `enum_name`: `__disc` is
/// the registry discriminant as a literal, and every variant's slots are laid
/// out (defaulted via [`build_enum_fields`]) with the constructed variant's
/// slots overwritten by `payload`. The full superset lets a later `match`,
/// `if let` or `let else` read any arm's payload even when the value is never
/// stored. An unknown `enum_name` yields `__disc` 0 plus the given slots.
pub(super) fn lower_enum_ctor(
    ctx: &mut LowerCtx<'_>,
    enum_name: &str,
    enum_args: &[Type],
    variant: &str,
    payload: Vec<(String, Binding)>,
    range: &SourceRange,
) -> Binding {
    let Some(def) = ctx.enum_defs.expression_path(enum_name).cloned() else {
        let mut fields = HashMap::default();
        let disc_port = literal_node_range(ctx, range, Type::Int, Literal::Int(0));
        fields.insert(*DISC, Binding::Local(LocalRecord { port: disc_port }));
        for (slot_key, binding) in payload {
            fields.insert(intern(&slot_name(variant, slot_key)), binding);
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
        fields.insert(intern(&slot_name(variant, slot_key)), binding);
    }
    Binding::Record(fields)
}

/// The `*Reference` gate class that sources an asset of `asset_type` as an
/// `entity` wire: it holds the asset in its class/object `Asset` field and
/// outputs `Value: entity`.
fn asset_reference_gate(asset_type: &str) -> &'static str {
    match asset_type {
        "BRItemBase" => "BrickComponentType_ItemReference",
        "BRPickupBase" => "BrickComponentType_PickupReference",
        "BRWeaponProjectile" => "BrickComponentType_ProjectileReference",
        "BrickAudioDescriptor" => "BrickComponentType_AudioReference",
        "BrickFontDescriptor" => "BrickComponentType_FontReference",
        "BrickOneShotAudioDescriptor" => "BrickComponentType_OneShotAudioReference",
        "BrickWheelEngineAudioDescriptor" => "BrickComponentType_WheelEngineAudioReference",
        // Unknown/other categories -> the generic entity-type reference.
        _ => "BrickComponentType_EntityTypeReference",
    }
}

/// A number constant `e` as the literal it lowers to under the type typecheck
/// recorded for it (`const_eval::coerce_const_to_type`): an integer is the
/// float it names in a `float` slot, a float the integer it truncates to in
/// an `int` slot, and each is itself anywhere else.
pub(in crate::lower) fn constant_literal(
    ctx: &LowerCtx<'_>,
    e: &Expr,
    lit: Literal,
) -> (Type, Literal) {
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
/// Names resolve through the scope (`certified_const_ctx`), so a local named
/// like an outer constant keeps its own wire. An integer operation with no
/// certified fold result anywhere in the evaluation leaves the value to the gate.
fn int_constant_expr_as_float(ctx: &LowerCtx<'_>, e: &Expr) -> Option<Literal> {
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
    match ctx.const_eval_budget(e, ConstScope::Certified) {
        (Ok(lit @ Literal::Int(_)), budget) if !budget.uncertified => Some(
            crate::const_eval::coerce_const_to_type(lit, &Type::Float, ctx.consts.types()),
        ),
        _ => None,
    }
}

/// A float constant expression (a literal, a name bound to one, or an operator,
/// call, index or field read of constants) that typecheck typed `int` because
/// it flows into an `int` slot: the integer it truncates to, as
/// `const_eval::coerce_const_to_type` holds it.
fn float_constant_expr_as_int(ctx: &LowerCtx<'_>, e: &Expr) -> Option<Literal> {
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
    match ctx.const_eval_budget(e, ConstScope::Certified) {
        (Ok(lit @ Literal::Float(_)), budget) if !budget.uncertified => Some(
            crate::const_eval::coerce_const_to_type(lit, &Type::Int, ctx.consts.types()),
        )
        .filter(|l| matches!(l, Literal::Int(_))),
        _ => None,
    }
}

/// A name bound to an integer constant, read where typecheck typed it
/// `float` (an integer constant passed to a `float` parameter): the float
/// literal it names, beside the shared integer gate.
fn retype_int_constant(ctx: &mut LowerCtx<'_>, e: &Expr, port: PortRef) -> PortRef {
    match int_constant_as_float(ctx, e, port) {
        Some(lit) => literal_node(ctx, e, Type::Float, lit),
        None => port,
    }
}

/// The float literal a read `e` of the integer constant at `port` names,
/// when typecheck typed the read `float`.
fn int_constant_as_float(ctx: &LowerCtx<'_>, e: &Expr, port: PortRef) -> Option<Literal> {
    let n = ctx
        .builder
        .module
        .nodes
        .get(&port.node_id)
        .filter(|n| n.gate_class == gc::LITERAL)?;
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
pub(in crate::lower) fn reads_constant_as_other_number(ctx: &LowerCtx<'_>, e: &Expr) -> bool {
    if int_constant_expr_as_float(ctx, e).is_some() || float_constant_expr_as_int(ctx, e).is_some()
    {
        return true;
    }
    let Expr::Ident { name, .. } = e else {
        return false;
    };
    match ctx.scope.get(name) {
        Some(Binding::Local(l)) => int_constant_as_float(ctx, e, l.port).is_some(),
        _ => false,
    }
}

pub(super) fn literal_node(ctx: &mut LowerCtx<'_>, e: &Expr, ty: Type, lit: Literal) -> PortRef {
    literal_node_range(ctx, e.range(), ty, lit)
}

/// The wire type a constant [`Literal`] has when materialized as a literal
/// source gate with no annotation to take the type from. Exhaustive with no
/// catch-all, so a new `Literal` variant forces a decision here.
///
/// `None` means no wire form: the caller falls back to the WSP001 placeholder
/// rather than baking a `Type::Any` literal gate. A `const` mod/chip parameter
/// of any kind can reach here bare, since the body frame records whatever its
/// argument const-evaluates to.
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
        // A constant prefab reference: never a runtime wire value (reference-only,
        // like `Type::Zone`), but a real singular type, reachable as a top-level
        // `const p = $./f.brz` and as a `const` parameter read bare.
        Literal::PrefabRef { .. } | Literal::NestedPrefab { .. } | Literal::PrefabBytes(_) => {
            Some(Type::PrefabRef)
        }
        // Collections, a compile-time record and an external asset reference
        // are not values a single wire carries. A top-level `let`/`const` binds
        // them separately (`lower_let_decl`); a `const` parameter reaches here.
        Literal::Array(_) | Literal::Map(_) | Literal::Record(_) | Literal::Asset { .. } => None,
        // No expression folds to `Object`; it is only the default value of an
        // object `Var` gate. Treated as no wire form rather than `unreachable!`,
        // so a violation gives a WSP001 placeholder, not a panic. Invariant test:
        // `lower::tests::const_init::literal_object_is_never_produced_by_const_eval`.
        Literal::Object => None,
    }
}

pub(super) fn literal_node_range(
    ctx: &mut LowerCtx<'_>,
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
        props.insert(*sym::SEPARATOR, Literal::String(String::new()));
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::STRING_CONCATENATE,
            source_range: range.clone(),
            ports: GateIO::source(Type::String),
            properties: props,
            ..Default::default()
        });
        return node_id.port(WirePort::Output);
    }
    raw_literal(ctx, range, ty, lit, None)
}

/// A `_Literal` gate holding `lit` as a `ty`: its `Output` port.
pub(super) fn raw_literal(
    ctx: &mut LowerCtx<'_>,
    range: &SourceRange,
    ty: Type,
    lit: Literal,
    note: Option<&'static str>,
) -> PortRef {
    let mut props = HashMap::default();
    props.insert(*sym::VALUE, lit);
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::LITERAL,
        source_range: range.clone(),
        ports: GateIO::source(ty),
        properties: props,
        note,
        ..Default::default()
    });
    node_id.port(WirePort::Output)
}

/// `&x` in a value position: the port carrying `x`'s ref. A var's `VarRef`,
/// an array's `ArrayVarRef`, a map's `MapVarRef`, an `in` port's `RER_Output`,
/// or, for a name already bound to a runtime ref, that wire. An operand that
/// already evaluates to a ref (`&rows[0]`, a var holding an array ref) is its
/// own ref.
fn lower_ref_of(ctx: &mut LowerCtx<'_>, operand: &Expr, range: &SourceRange) -> PortRef {
    let binding = match operand {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        Expr::FieldAccess { .. } | Expr::TuplePick { .. } => {
            resolve_field_chain(ctx, operand).cloned()
        }
        _ => None,
    };
    match binding {
        Some(Binding::Var(v))
            if matches!(v.inner_type, Type::Ref(_)) && v.storage == VarStorage::Var =>
        {
            lower_expr(ctx, operand)
        }
        Some(Binding::Var(v)) => match v.storage {
            VarStorage::Var => v.node_id.port(WirePort::VarRef),
            VarStorage::Array => v.node_id.port(WirePort::ArrayVarRef),
            VarStorage::Map => v.node_id.port(WirePort::MapVarRef),
            VarStorage::Buffer => {
                ctx.error(
                    "WS008",
                    "a buffer has no ref to take: it is a wire, not a var",
                    range,
                );
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
pub(super) fn read_dyn_ref(
    ctx: &mut LowerCtx<'_>,
    d: &DynRefRecord,
    range: &SourceRange,
) -> PortRef {
    if d.storage != VarStorage::Var {
        return d.port;
    }
    match ctx.current_exec {
        Some(exec) => {
            chain_var_get_through(ctx, d.port, &d.inner_type, exec, range).port(WirePort::Value)
        }
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
fn ident_type(ctx: &LowerCtx<'_>, name: &str, range: &SourceRange) -> Type {
    ctx.type_of(&Expr::Ident {
        name: name.to_string(),
        range: range.clone(),
    })
}

pub(super) fn lower_ident(ctx: &mut LowerCtx<'_>, name: &str, range: &SourceRange) -> PortRef {
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
                if let Some(cached) = ctx.var_gets.get(&var_rec.node_id) {
                    return cached.port(WirePort::Value);
                }
                let props = var_get_seed_props(&var_rec.inner_type);
                let get_id = chain_var_get(ctx, &var_rec, exec, range, props, None);
                ctx.var_gets.insert(var_rec.node_id, get_id);
                return get_id.port(WirePort::Value);
            }
            var_rec.node_id.port(WirePort::Value)
        }
        Some(Binding::Buffer(buf)) => buf.node_id.port(WirePort::Output),
        Some(Binding::Input(inp)) => inp.node_id.port(WirePort::RerOutput),
        Some(Binding::EventParam(p)) => p,
        Some(Binding::Local(local)) => local.port,
        Some(Binding::DynRef(d)) => read_dyn_ref(ctx, &d, range),
        Some(Binding::Record(ref rec))
            if enum_tag(ctx, rec, &ident_type(ctx, name, range)).is_some() =>
        {
            let tag = enum_tag(ctx, rec, &ident_type(ctx, name, range))
                .cloned()
                .expect("checked by the guard");
            binding_to_port(ctx, &tag, range)
                .unwrap_or_else(|| synthesise_unsupported_no_warn(ctx, range))
        }
        Some(Binding::Record(rec)) => {
            // A record is a bundle of per-field ports with no single wire, and
            // whole-record consumers go through `value_record_fields`, so here
            // it was asked for as one value. Typecheck's `op_operand_type` cannot
            // tell this from a multi-output gate result (`ParseInt(s) == 42`),
            // which does have a port; only the binding can, so WS071 is raised
            // here. An enum's slots are internal, so its message names the tag.
            let declared = unwrap_ref(&ident_type(ctx, name, range));
            let enum_value = matches!(declared, Type::Enum { .. });
            let mut names: Vec<&str> = rec
                .keys()
                .map(|k| resolve(*k))
                .filter(|n| !n.starts_with("__"))
                .collect();
            names.sort_unstable();
            // The first field as the type declares it, when the type names one.
            if let Some(first) = declared
                .fields()
                .and_then(|fs| fs.iter().find(|(n, _)| !n.starts_with("__")))
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
                let field = names
                    .first()
                    .map_or(String::new(), |f| format!(" (`{name}.{f}`)"));
                format!(
                    "`{name}` is a record - several wires, not one value - so \
                     it can't be used where a single value is expected. Read \
                     one of its fields{field}; to build a string, concatenate \
                     the fields individually."
                )
            };
            // A gate read held as its outputs gives one value its first.
            if let Some(b) = held_output(ctx, &rec, range, OutputPick::First)
                && let Some(port) = binding_to_port(ctx, &b, range)
            {
                return port;
            }
            ctx.error("WS071", message, range);
            synthesise_unsupported_no_warn(ctx, range)
        }
        Some(Binding::Output(_) | Binding::Chip(_) | Binding::Namespace(_)) => {
            synthesise_unsupported_range(ctx, &format!("a read of `{name}`"), range)
        }
        // Not bound to a wire. A compile-time constant (`ctx.const_lookup()`, as
        // the scope resolves it) becomes a literal source gate, which literal
        // inlining in `lower/mod.rs` folds into its consumers. A `const`
        // parameter needs this: its value lives in the body frame's constants,
        // not in `scope`. Composite constants are included, since the
        // alternative is `_Unsupported`; one with no wire form falls through.
        None => {
            if let Some(lit) = ctx.const_lookup().get(name).cloned()
                && let Some(ty) = wire_type_of_literal(&lit)
            {
                return literal_node_range(ctx, range, ty, lit);
            }
            // A constant array/map read as a VALUE (an `if` branch, a record
            // field, a tuple element, a `return`, an `out`, a chip pin): its
            // runtime form is the container gate, and the value is its ref.
            let ident = Expr::Ident {
                name: name.to_string(),
                range: range.clone(),
            };
            if let Some(rec) = super::predeclare::materialize_const_container(ctx, &ident) {
                let port = match rec.storage {
                    VarStorage::Map => WirePort::MapVarRef,
                    _ => WirePort::ArrayVarRef,
                };
                return rec.node_id.port(port);
            }
            // Fallback tier: an output port reads as its rerouter's `RER_Output`,
            // as a namespaced `L.count` read does. Keyed under the `out:` prefix
            // and checked after constants, the order typecheck resolves in.
            if let Some(Binding::Output(out)) = ctx.scope.get(&output_scope_key(name)).cloned() {
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
    ctx: &mut LowerCtx<'_>,
    cond: &Expr,
    then_br: &Expr,
    else_br: &Expr,
    range: &SourceRange,
) -> PortRef {
    let ports = ordered::lower_in_order(ctx, &[cond, then_br, else_br]);
    let (cond_port, then_port, else_port) = (ports[0], ports[1], ports[2]);
    // The Select carries the branches' widening join, as typecheck types the
    // `if`: `if c then 1 else 2.0` is a Float Select fed the int wire directly
    // (ports coerce natively). With no join (WS003) the else type is used.
    // Inside a generic mod `type_of` holds the last mask member's type, so the
    // branches' lowered port types are read instead. A gate-read branch gives
    // its first output.
    let branch_ty =
        |ctx: &LowerCtx<'_>, e: &Expr| ctx.one_value_type_at(e.range()).unwrap_or(Type::Any);
    let (then_ty, else_ty) = if ctx.mono_stack.is_empty() || !ctx.in_generic_instance(range) {
        (
            unwrap_ref(&branch_ty(ctx, then_br)),
            unwrap_ref(&branch_ty(ctx, else_br)),
        )
    } else {
        let then_ty =
            super::call::arg_port_type(ctx, then_port).unwrap_or_else(|| branch_ty(ctx, then_br));
        let else_ty =
            super::call::arg_port_type(ctx, else_port).unwrap_or_else(|| branch_ty(ctx, else_br));
        (unwrap_ref(&then_ty), unwrap_ref(&else_ty))
    };
    let result_ty = crate::types::coerce::widening_join(&then_ty, &else_ty).unwrap_or(else_ty);
    emit_select(
        ctx,
        cond_port,
        then_port,
        else_port,
        &result_ty,
        "if-expr select",
        range,
    )
}

/// Lowers a value `match` to a pure nested `Select` tree (the statement form
/// uses `Branch`). Walks the decision tree: a `Switch` reads the `__disc` at its
/// path and folds one `Select` per case on `disc == case`; a `Leaf` lowers its
/// arm body with the payload captures bound to the scrutinee's payload slots.
pub(super) fn lower_match_expr(
    ctx: &mut LowerCtx<'_>,
    scrutinee: &Expr,
    arms: &[MatchArm],
    range: &SourceRange,
    match_expr: &Expr,
) -> PortRef {
    // The result type is the arms' widening join that typecheck recorded for
    // the match; for gate reads, the first output, as in `lower_if_expr`.
    let result_ty = unwrap_ref(
        &ctx.one_value_type_at(match_expr.range())
            .unwrap_or(Type::Any),
    );
    let scrut_ty = scrutinee_type(ctx, scrutinee);
    let decision = build_arms(ctx, &scrut_ty, arms);

    // The scrutinee's `__disc` plus payload record: a named scrutinee resolves
    // through the scope, an inline construction is lowered to get it. One with
    // no decomposition (an enum input port, an errored program) gets the
    // `_Unsupported` placeholder. A single value sits alone in its root.
    let Some(root) = match_root(ctx, scrutinee, &scrut_ty) else {
        return synthesise_unsupported_range(ctx, "a `match` on a value with no enum tag", range);
    };
    let outer_concats = std::mem::take(&mut ctx.concat_cache);

    // Const elision, as in `lower_if`: a compile-time-known `__disc` resolves to
    // the taken leaf, and only that leaf lowers. Off under `@nofold`, and only
    // names spelled `const` count (`if_cond_const_ctx`), never a foldable `let`.
    let decision = match ctx.nofold_depth {
        0 => try_const_decision(ctx, &decision, scrutinee).unwrap_or(decision),
        _ => decision,
    };
    let walk = MatchWalk {
        root: &root,
        arms,
        range,
    };
    let port = walk.decision(ctx, &decision, &result_ty);
    ctx.concat_cache = outer_concats;
    port
}

/// What a `match` walk over a decision tree keeps fixed while it recurses: the
/// scrutinee record, the arms, and the range of the whole match.
pub(super) struct MatchWalk<'a> {
    pub(super) root: &'a Fields,
    pub(super) arms: &'a [MatchArm],
    pub(super) range: &'a SourceRange,
}

impl MatchWalk<'_> {
    /// Walk one [`Decision`] node into wires, returning the
    /// port carrying that sub-match's value.
    fn decision(&self, ctx: &mut LowerCtx<'_>, decision: &Decision, result_ty: &Type) -> PortRef {
        let Self { root, arms, range } = *self;
        match decision {
            Decision::Leaf(i) => lower_match_arm(ctx, &arms[*i], root, result_ty, range),
            // No arm matched (only reachable for a non-exhaustive match, already a
            // WS054): the zero/default of the result type on the innermost else path.
            Decision::Fail => {
                let lit = default_literal_for_var_type(result_ty).unwrap_or(Literal::Float(0.0));
                literal_node_range(ctx, range, result_ty.clone(), lit)
            }
            Decision::Switch {
                path,
                cases,
                default,
            } => {
                let disc_port = read_disc_at_path(ctx, root, path, range).unwrap_or_else(|| {
                    synthesise_unsupported_range(ctx, "an enum tag read", range)
                });
                // Fold RIGHT: the final default (or `Fail` zero) supplies the
                // innermost `Select`'s `InputA`, and each case wraps another
                // `Select(disc == k, sub, acc)` around it.
                let mut acc = match default {
                    Some(d) => self.decision(ctx, d, result_ty),
                    None => self.decision(ctx, &Decision::Fail, result_ty),
                };
                for (keys, sub) in cases.iter().rev() {
                    let sub_port = self.decision(ctx, sub, result_ty);
                    let cond_port = emit_disc_in(ctx, disc_port, keys, range);
                    acc = emit_select(
                        ctx,
                        cond_port,
                        sub_port,
                        acc,
                        result_ty,
                        "match-expr select",
                        range,
                    );
                }
                acc
            }
            Decision::Check {
                tests,
                arm,
                guarded,
                otherwise,
            } => {
                let else_port = self.decision(ctx, otherwise, result_ty);
                let arm = &arms[*arm];
                let (cond, then_port) =
                    checked_arm(ctx, arm, root, tests, *guarded, range, |ctx| {
                        arm_value(ctx, arm, result_ty, range)
                    });
                match cond {
                    Some(cond) => emit_select(
                        ctx,
                        cond,
                        then_port,
                        else_port,
                        result_ty,
                        "match-expr select",
                        range,
                    ),
                    None => then_port,
                }
            }
        }
    }

    /// [`MatchWalk::decision`]'s per-field twin: walk one decision node into a field map
    /// rather than a single port.
    fn decision_record(&self, ctx: &mut LowerCtx<'_>, decision: &Decision) -> Option<Fields> {
        let Self { root, arms, range } = *self;
        match decision {
            Decision::Leaf(i) => lower_match_arm_record(ctx, &arms[*i], root, range),
            // No arm matched (a non-exhaustive match, already WS054). A record has
            // no single zero literal, so this contributes no fields and the fold
            // below makes the last case the innermost value.
            Decision::Fail => None,
            Decision::Switch {
                path,
                cases,
                default,
            } => {
                // One `__disc` read per Switch, like the scalar path. A Switch with
                // a single case and no default needs no test at all (nothing else
                // is reachable), so it reads no disc either.
                let disc_port = if cases.len() > 1 || default.is_some() {
                    Some(
                        read_disc_at_path(ctx, root, path, range).unwrap_or_else(|| {
                            synthesise_unsupported_range(ctx, "an enum tag read", range)
                        }),
                    )
                } else {
                    None
                };
                // Fold RIGHT exactly as `MatchWalk::decision` does, so the arms are
                // tested in the same order.
                let mut acc = default.as_ref().and_then(|d| self.decision_record(ctx, d));
                for (keys, sub) in cases.iter().rev() {
                    let sub_fields = self.decision_record(ctx, sub)?;
                    acc = Some(match acc {
                        Some(else_fields) => {
                            // `disc_port` is read whenever a test can be needed,
                            // which is exactly when `acc` is already `Some`.
                            let dp = disc_port?;
                            let cond = emit_disc_in(ctx, dp, keys, range);
                            select_record_fields(ctx, cond, &sub_fields, &else_fields, range)
                        }
                        // The innermost fallback: no Select, no `disc == k` compare.
                        None => sub_fields,
                    });
                }
                acc
            }
            Decision::Check {
                tests,
                arm,
                guarded,
                otherwise,
            } => {
                let else_fields = self.decision_record(ctx, otherwise);
                let arm = &arms[*arm];
                let (cond, then_fields) =
                    checked_arm(ctx, arm, root, tests, *guarded, range, |ctx| {
                        match &arm.body {
                            MatchBody::Expr(expr) => choice_branch_fields(ctx, expr, range),
                            MatchBody::Block(_) => None,
                        }
                    });
                let then_fields = then_fields?;
                Some(match (cond, else_fields) {
                    (Some(cond), Some(else_fields)) => {
                        select_record_fields(ctx, cond, &then_fields, &else_fields, range)
                    }
                    // Nothing to fall back on: only reachable for a non-exhaustive
                    // match, already a WS054 error.
                    _ => then_fields,
                })
            }
        }
    }
}

/// The decision tree for `arms` matched against a `scrut_ty` value, each
/// guarded arm marked.
pub(super) fn build_arms(ctx: &LowerCtx<'_>, scrut_ty: &Type, arms: &[MatchArm]) -> Decision {
    let patterns: Vec<Pattern> = arms.iter().map(|a| a.pattern.clone()).collect();
    let guarded: Vec<bool> = arms.iter().map(|a| a.guard.is_some()).collect();
    matchtree::build_guarded(ctx.enums(), scrut_ty, &patterns, &guarded)
}

/// The record-valued twin of [`lower_match_expr`]: one `Select` per leaf field
/// over the same decision tree, as a record-valued if-expr builds in
/// `value_record_fields` (sharing `select_record_fields`).
///
/// `None` (not a record source) for a block-bodied arm, checked before anything
/// lowers; a scrutinee with no record decomposition; or an arm with no field map.
pub(super) fn lower_match_expr_record(
    ctx: &mut LowerCtx<'_>,
    scrutinee: &Expr,
    arms: &[MatchArm],
    range: &SourceRange,
) -> Option<Fields> {
    // A block-bodied arm carries no value at all (the statement form builds an
    // exec chain instead), so there is no per-field choice to make.
    if arms.iter().any(|a| matches!(a.body, MatchBody::Block(_))) {
        return None;
    }
    // Nor is there a store to confine an arm that acts to its own branch.
    if ctx.current_exec.is_some()
        && arms
            .iter()
            .any(|a| matches!(&a.body, MatchBody::Expr(x) if has_effects(ctx, x)))
    {
        report_effectful_aggregate_choice(ctx, range);
        return None;
    }
    let scrut_ty = scrutinee_type(ctx, scrutinee);
    let decision = build_arms(ctx, &scrut_ty, arms);
    let root = match_root(ctx, scrutinee, &scrut_ty)?;
    // Const elision, as in `lower_match_expr`.
    let decision = match ctx.nofold_depth {
        0 => try_const_decision(ctx, &decision, scrutinee).unwrap_or(decision),
        _ => decision,
    };
    let outer_concats = std::mem::take(&mut ctx.concat_cache);
    let walk = MatchWalk {
        root: &root,
        arms,
        range,
    };
    let fields = walk.decision_record(ctx, &decision);
    ctx.concat_cache = outer_concats;
    fields
}

/// The condition a `Check` runs `arm` on (its value tests and guard, `None`
/// for neither), and what `body` makes of the arm, both lowered with the
/// arm's captures bound.
fn checked_arm<R>(
    ctx: &mut LowerCtx<'_>,
    arm: &MatchArm,
    root: &Fields,
    tests: &[ValueCheck],
    guarded: bool,
    range: &SourceRange,
    body: impl FnOnce(&mut LowerCtx<'_>) -> R,
) -> (Option<PortRef>, R) {
    with_arm_captures(ctx, arm, root, |ctx| {
        let cond = arm_condition(ctx, arm, root, tests, guarded, range);
        (cond, body(ctx))
    })
}

/// A `Check`'s value tests and, when `guarded`, the arm's guard, all of
/// which must hold; lowered where the arm's captures are bound.
pub(super) fn arm_condition(
    ctx: &mut LowerCtx<'_>,
    arm: &MatchArm,
    root: &Fields,
    tests: &[ValueCheck],
    guarded: bool,
    range: &SourceRange,
) -> Option<PortRef> {
    let mut conds: Vec<PortRef> = lower_value_checks(ctx, root, tests, range)
        .into_iter()
        .collect();
    if guarded && let Some(guard) = &arm.guard {
        conds.push(lower_expr(ctx, guard));
    }
    all_of(ctx, conds, range)
}

/// [`lower_match_arm`]'s record twin: bind the arm's payload captures the same
/// way, then resolve its body as a per-field record instead of a single port.
fn lower_match_arm_record(
    ctx: &mut LowerCtx<'_>,
    arm: &MatchArm,
    root: &Fields,
    range: &SourceRange,
) -> Option<Fields> {
    let MatchBody::Expr(expr) = &arm.body else {
        return None;
    };
    with_arm_captures(ctx, arm, root, |ctx| choice_branch_fields(ctx, expr, range))
}

/// The scrutinee's `__disc` plus payload-slot record. A named scrutinee resolves
/// through the scope (`resolve_field_chain`); any other shape that carries a
/// record (an inline construction, a container element, a field path, a
/// record-valued conditional) goes through `value_record_fields`. `None` when
/// there is no record (an enum input port, an errored program); the caller
/// emits `_Unsupported`.
pub(super) fn match_scrutinee_record(ctx: &mut LowerCtx<'_>, scrutinee: &Expr) -> Option<Fields> {
    // A gate read is its first output, the enum it reads.
    if enum_gate_read(ctx, scrutinee).is_some() {
        return gate_read_enum_value(ctx, scrutinee);
    }
    if let Some(Binding::Record(root)) = resolve_field_chain(ctx, scrutinee).cloned() {
        return Some(root);
    }
    value_record_fields(ctx, scrutinee)
}

/// The enum value `e`, a gate read whose first output is an enum, gives a
/// place for one value: that output's tag and payload slots.
pub(super) fn gate_read_enum_value(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<Fields> {
    enum_gate_read(ctx, e)?;
    let read = value_record_fields(ctx, e)?;
    match held_output(ctx, &read, e.range(), OutputPick::First) {
        Some(Binding::Record(value)) => Some(value),
        Some(_) => None,
        // An enum container's read lowers to the enum value itself.
        None => enum_disc(&read).is_some().then_some(read),
    }
}

/// A gate read whose first output is an enum (`hs.pop()`, `m.get(k)`, a
/// call of a `mod` with several outputs, a name holding one) as a
/// scrutinee: that enum.
fn enum_gate_read(ctx: &LowerCtx<'_>, scrutinee: &Expr) -> Option<Type> {
    let (_, first) = ctx.gate_outputs(scrutinee.range())?.first()?;
    let first = unwrap_ref(first);
    matches!(first, Type::Enum { .. }).then(|| first.as_record())
}

/// The enum a `match`, `if let` or `let ... else` scrutinee is: its checked
/// type, or the enum a gate read reads first.
pub(super) fn scrutinee_type(ctx: &LowerCtx<'_>, scrutinee: &Expr) -> Type {
    enum_gate_read(ctx, scrutinee).unwrap_or_else(|| unwrap_ref(&ctx.type_of(scrutinee)))
}

/// The taken terminal (`Leaf`/`Fail`) node of `decision` for a compile-time-known
/// `scrutinee`, or `None` for a runtime value. Uses the same constant scope as
/// `lower_if`'s elision (`ConstScope::IfCond`), so both agree on what is constant.
pub(super) fn try_const_decision(
    ctx: &LowerCtx<'_>,
    decision: &Decision,
    scrutinee: &Expr,
) -> Option<Decision> {
    let lit = ctx.const_eval(scrutinee, ConstScope::IfCond).ok()?;
    matchtree::resolve_const_leaf(decision, &lit)
}

/// Lower one arm body with its payload captures bound as compile-time moves of
/// the scrutinee's matching payload slots (a scoped bind, no gate).
fn lower_match_arm(
    ctx: &mut LowerCtx<'_>,
    arm: &MatchArm,
    root: &Fields,
    result_ty: &Type,
    range: &SourceRange,
) -> PortRef {
    with_arm_captures(ctx, arm, root, |ctx| arm_value(ctx, arm, result_ty, range))
}

/// One arm's value, its captures already bound.
fn arm_value(
    ctx: &mut LowerCtx<'_>,
    arm: &MatchArm,
    result_ty: &Type,
    range: &SourceRange,
) -> PortRef {
    match &arm.body {
        MatchBody::Expr(expr) => lower_expr(ctx, expr),
        // A block-bodied arm carries no value in expression position (typecheck
        // omits it from the arm-type join): yield the result type's zero rather
        // than an exec chain (the statement form builds one).
        MatchBody::Block(_) => {
            let lit = default_literal_for_var_type(result_ty).unwrap_or(Literal::Float(0.0));
            literal_node_range(ctx, range, result_ty.clone(), lit)
        }
    }
}

/// Runs `f` in a block scope with the arm's captures bound: payload captures as
/// compile-time moves of the scrutinee's slots (no gate), a string pattern's
/// holes as the substrings its test cut. The scope is popped afterwards, so no
/// arm leaks its captures into the next.
pub(super) fn with_arm_captures<R>(
    ctx: &mut LowerCtx<'_>,
    arm: &MatchArm,
    root: &Fields,
    f: impl FnOnce(&mut LowerCtx<'_>) -> R,
) -> R {
    ctx.push_scope(crate::scope::ScopeTag::BLOCK);
    bind_pattern_captures(ctx, &arm.pattern, root, &arm.range);
    let out = f(ctx);
    ctx.pop_scope();
    out
}

/// Bind every name `pattern` captures into the current scope, read off the
/// match `root`. Returns the names bound.
pub(super) fn bind_pattern_captures(
    ctx: &mut LowerCtx<'_>,
    pattern: &Pattern,
    root: &Fields,
    range: &SourceRange,
) -> Vec<String> {
    let mut captures = Vec::new();
    matchtree::collect_captures(pattern, &mut Vec::new(), &mut captures);
    let mut names = Vec::with_capacity(captures.len());
    for capture in captures {
        if let Some(binding) = bind_capture(ctx, root, &capture, range) {
            ctx.scope.insert(&capture.name, binding);
        }
        names.push(capture.name);
    }
    names
}

/// Navigate the scrutinee record along `path` to the captured slot's binding,
/// cloned for a compile-time move into the arm scope. An empty path binds the
/// whole scrutinee: the record, or a single value's own binding; a missing or
/// non-record intermediate yields `None`.
pub(super) fn navigate_capture(root: &Fields, path: &[String]) -> Option<Binding> {
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        let binding = cur.get(&intern(seg))?;
        if i + 1 == path.len() {
            return Some(binding.clone());
        }
        match binding {
            Binding::Record(sub) => cur = sub,
            _ => return None,
        }
    }
    if let Some(value) = root.get(&intern(matchtree::VALUE_ROOT)) {
        return Some(value.clone());
    }
    Some(Binding::Record(root.clone()))
}

/// Read the `__disc` of the sub-record reached by `path` (navigate the record
/// via each `PathStep::Field`, then its `__disc`) as a value port.
pub(super) fn read_disc_at_path(
    ctx: &mut LowerCtx<'_>,
    root: &Fields,
    path: &[PathStep],
    range: &SourceRange,
) -> Option<PortRef> {
    let mut cur = root;
    for PathStep::Field(f) in path {
        match cur.get(&intern(f)) {
            Some(Binding::Record(sub)) => cur = sub,
            _ => return None,
        }
    }
    let disc = cur.get(&*DISC)?.clone();
    binding_to_port(ctx, &disc, range)
}

/// A `disc == k` equality gate (`CompareEqual`), with `k` a literal int source.
pub(super) fn emit_disc_eq(
    ctx: &mut LowerCtx<'_>,
    disc_port: PortRef,
    k: i64,
    range: &SourceRange,
) -> PortRef {
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
        note: Some("match disc compare"),
        ..Default::default()
    });
    ctx.connect(disc_port, cmp.port(WirePort::InputA));
    ctx.connect(k_port, cmp.port(WirePort::InputB));
    cmp.port(WirePort::BOutput)
}

/// Compile-time string to bool coercion: a String literal baked into a Bool
/// destination becomes `Literal::Bool(!s.is_empty())`. It must match the
/// `CompareNotEqual(s, "")` gate `LowerCtx::wrap_string_to_bool` inserts on the
/// wire path, so "0" and "false" are true. A raw String on a Bool field would
/// read through the game's content-aware truthiness ("0" falsy) or fail emit
/// with `UnimplementedCast`. Other pairs pass through.
pub(super) fn bake_string_bool(lit: Literal, ty: &Type) -> Literal {
    match (&lit, ty) {
        (Literal::String(s), Type::Bool) => Literal::Bool(!s.is_empty()),
        _ => lit,
    }
}

/// The literal `e` bakes into a data field of type `port_ty`, if it is constant.
///
/// `config_only` marks a port with no wire pin (a settings-menu field such as
/// `SendCustomEvent`'s `EventName`): it adds a last fallback through the full
/// const evaluator, matching typecheck's `validate_scalar_config_arg`, or the
/// argument is silently dropped. It must stay off for wire-capable ports, where
/// folding operators would delete gates (`Rotation(0.0 + 0.0, ...)` keeps its
/// `MathAdd`). Composite and data-driven config keep the narrow evaluator on
/// both sides (`validate_composite_config_arg`, `validate_data_driven_config`).
pub(super) fn literal_for_property_port(
    ctx: &LowerCtx<'_>,
    e: &Expr,
    port_ty: &Type,
    config_only: bool,
) -> Option<Literal> {
    // No type promotion (emit takes the native type from the data struct
    // schema), except string to bool, which emit cannot cast (`bake_string_bool`).
    // First the env-less fold: literals, negated literals and constructor calls
    // on constant args, never operators or names.
    let lit = expr_to_literal(e)
        .or_else(|| {
            // A bare name bound to a constant (`let pf = $./foo.brz` then
            // `SpawnPrefab(prefab = pf)`), via `ctx.const_lookup()`. Only a plain
            // `Ident`, and not a Vector/Rotator/Quat/Color: those keep the wire
            // path, since most consumers need the wired `Make*` gate and skipping
            // it can prune the producer as a wireless orphan.
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
            // Config-only port: typecheck's acceptance (see the doc comment).
            // Last, so it only adds a literal where the narrow paths found none.
            if !config_only {
                return None;
            }
            ctx.const_eval(e, ConstScope::Plain).ok()
        });
    // A number constant typecheck held in the other number type (event data
    // a receiver declares) is that number.
    let lit = lit.map(|lit| match lit {
        Literal::Int(_) | Literal::Float(_) => constant_literal(ctx, e, lit).1,
        other => other,
    });
    lit.map(|lit| bake_string_bool(lit, port_ty))
}

pub(super) fn synthesise_unsupported(ctx: &mut LowerCtx<'_>, e: &Expr) -> PortRef {
    let port = synthesise_unsupported_no_warn(ctx, e.range());
    ctx.warn(
        format!(
            "{}: {}",
            crate::diagnostic::PLACEHOLDER,
            unsupported_what(e)
        ),
        e.range(),
    );
    port
}

pub(super) fn synthesise_unsupported_range(
    ctx: &mut LowerCtx<'_>,
    what: &str,
    range: &SourceRange,
) -> PortRef {
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
    ctx: &mut LowerCtx<'_>,
    range: &SourceRange,
) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNSUPPORTED,
        source_range: range.clone(),
        ports: GateIO::source(Type::Any),
        note: Some("unsupported expression"),
        ..Default::default()
    });
    node_id.port(WirePort::Output)
}

/// Whether lowering `e` on an exec chain does anything beyond producing its
/// value: runs a statement or a `match` block arm (which leaves through a
/// `return`), takes the chain point (`emit` as a value), runs an action
/// builtin (`CallSpec::has_effect`) or a container mutation, or calls a
/// `mod`/`chip` whose body writes outside itself (an output it does not own, a
/// variable it did not declare, a `static var`, a container, an `await`).
pub(super) fn has_effects(ctx: &LowerCtx<'_>, e: &Expr) -> bool {
    effects_below(ctx, walk::Node::Expr(e), None, &mut HashSet::default())
}

/// What a `mod`/`chip` body may write without an effect outside it: the names
/// it declares and its own outputs.
struct BodyScope<'a> {
    locals: HashSet<&'a str>,
    outputs: Vec<&'a str>,
}

fn effects_below<'a>(
    ctx: &LowerCtx<'_>,
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
    ctx: &LowerCtx<'_>,
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
                None => crate::catalog::calls::find_call(name)
                    .is_some_and(super::super::catalog::calls::CallSpec::has_effect),
            },
            Expr::FieldAccess { obj, field, range } => {
                let mutates = crate::catalog::arrays::array_method(field)
                    .is_some_and(|m| m.mutates)
                    || crate::catalog::maps::map_method(field).is_some_and(|m| m.mutates);
                if mutates {
                    return !matches!(crate::typecheck::infer::flatten_access(obj).0, Expr::Ident { name, .. } if local(name));
                }
                if crate::catalog::calls::find_call(field)
                    .is_some_and(|c| c.receiver.is_some() && c.has_effect())
                {
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

fn body_has_effects(ctx: &LowerCtx<'_>, decl: &ChipDecl, seen: &mut HashSet<usize>) -> bool {
    if !seen.insert(std::ptr::from_ref::<ChipDecl>(decl) as usize) {
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
pub(super) fn warn_handlers_ungated_by_arm(ctx: &mut LowerCtx<'_>, arm: walk::Node<'_>) {
    let mut found: Vec<(String, String, SourceRange)> = Vec::new();
    let mut visit = |n: walk::Node<'_>| {
        let walk::Node::Expr(Expr::Call { callee, range, .. }) = n else {
            return;
        };
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
    walk::any_below(
        arm,
        |f| matches!(f, walk::Flow::Seq | walk::Flow::AnonChip),
        &mut |n| {
            visit(n);
            false
        },
    );
    for (name, trigger, range) in found {
        if ctx
            .diagnostics
            .iter()
            .any(|d| d.code == "WS102" && d.range == range)
        {
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
fn acting_handler(ctx: &LowerCtx<'_>, decl: &ChipDecl) -> Option<String> {
    let key = std::ptr::from_ref::<ChipDecl>(decl) as usize;
    if let Some(known) = ctx.acting_handler_memo.borrow().get(&key) {
        return known.clone();
    }
    let mut trigger = None;
    walk::any_below(
        walk::Node::Block(&decl.body),
        walk::Flow::on_path,
        &mut |m| {
            let walk::Node::Stmt(Stmt::Handler(h)) = m else {
                return false;
            };
            let mut locals = HashSet::default();
            collect_locals(&h.body, &mut locals);
            let scope = BodyScope {
                locals,
                outputs: decl.outputs.iter().map(|o| o.name.as_str()).collect(),
            };
            if effects_below(
                ctx,
                walk::Node::Block(&h.body),
                Some(&scope),
                &mut HashSet::default(),
            ) {
                trigger = Some(trigger_text(&h.trigger));
                return true;
            }
            false
        },
    );
    ctx.acting_handler_memo
        .borrow_mut()
        .insert(key, trigger.clone());
    trigger
}

/// How a handler's trigger is written.
fn trigger_text(t: &Trigger) -> String {
    use crate::ast::Trigger;
    match t {
        Trigger::Ident { name, .. } => name.clone(),
        Trigger::Field { obj, field, .. } => format!("{obj}.{field}"),
        Trigger::Not { inner, .. } => format!("!{}", trigger_text(inner)),
        Trigger::Union { parts, .. } => parts
            .iter()
            .map(trigger_text)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

/// What `decl`'s body, and the nested handler bodies in `handlers`, may
/// write without an effect outside it.
fn body_scope<'a>(decl: &'a ChipDecl, handlers: &[&'a Block]) -> BodyScope<'a> {
    let mut locals: HashSet<&str> = HashSet::default();
    for block in std::iter::once(&decl.body).chain(handlers.iter().copied()) {
        collect_locals(block, &mut locals);
    }
    BodyScope {
        locals,
        outputs: decl.outputs.iter().map(|o| o.name.as_str()).collect(),
    }
}

/// The names `block` declares, outside its nested handlers and declarations.
fn collect_locals<'a>(block: &'a Block, locals: &mut HashSet<&'a str>) {
    walk::walk(
        walk::Node::Block(block),
        walk::Ctx::default(),
        &mut |n, _| {
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
        },
    );
}
