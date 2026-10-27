//! Lower a builtin call: `CallSpec` to gate node.

use super::*;

/// The integer discriminant of a built-in enum value passed as gate config
/// (`function = EasingFunction.Bounce`, or a constant that folds to an enum
/// value), read as `__disc` off its const-folded `{ __disc: N }` record, so it
/// matches the int the bare member name bakes. `None` when the argument does
/// not fold to an enum value or its discriminant is not in `enum_type`.
fn enum_config_discriminant(ctx: &LowerCtx<'_>, arg_expr: &Expr, enum_type: &str) -> Option<i64> {
    let Literal::Record(fields) = ctx.const_eval(arg_expr, ConstScope::Plain).ok()? else {
        return None;
    };
    let (_, disc) = fields.into_iter().find(|(n, _)| n == DISC_FIELD)?;
    let Literal::Int(v) = disc else { return None };
    crate::catalog::enum_has_value(enum_type, v).then_some(v)
}

/// Replace any `Type::Param` left in `t` with `Type::Any`, as
/// `typecheck::call::resolve_param_output` does for an unpinned param. Keeps
/// the invariant emit's `var_type_to_wire_value` asserts: no `Type::Param`
/// reaches emit.
fn scrub_param_type(t: Type) -> Type {
    match t {
        Type::Param(_) => Type::Any,
        Type::Array(inner) => Type::Array(Box::new(scrub_param_type(*inner))),
        Type::Ref(inner) => Type::Ref(Box::new(scrub_param_type(*inner))),
        Type::Map(k, v) => Type::Map(
            Box::new(scrub_param_type(*k)),
            Box::new(scrub_param_type(*v)),
        ),
        Type::Union(opts) => Type::Union(opts.into_iter().map(scrub_param_type).collect()),
        Type::Tuple(fs) => Type::Tuple(fs.into_iter().map(scrub_param_type).collect()),
        Type::Record(fs) => Type::Record(
            fs.into_iter()
                .map(|(n, ft)| (n, scrub_param_type(ft)))
                .collect(),
        ),
        Type::Enum { name, args } => Type::Enum {
            name,
            args: args.into_iter().map(scrub_param_type).collect(),
        },
        other => other,
    }
}

/// The value type an `abs`/`min`/`max`/`clamp`/`Dampen` call resolved to,
/// which each of its math-variant ports carries.
fn math_variant(
    ctx: &LowerCtx<'_>,
    spec: &crate::catalog::calls::CallSpec,
    e: &Expr,
) -> Option<Type> {
    if !matches!(
        spec.gate_class,
        gc::MATH_ABS | gc::MATH_MIN | gc::MATH_MAX | gc::MATH_CLAMP | gc::DAMPEN
    ) {
        return None;
    }
    match ctx.type_at(e.range()).map(|t| unwrap_ref(&t)) {
        Some(
            t @ (Type::Int | Type::Float | Type::Vector | Type::Rotator | Type::Quat | Type::Color),
        ) => Some(t),
        _ => Some(Type::Float),
    }
}

/// Whether a `Type::Param` is left anywhere in `t`.
fn has_param(t: &Type) -> bool {
    match t {
        Type::Param(_) => true,
        Type::Array(inner) | Type::Ref(inner) => has_param(inner),
        Type::Map(k, v) => has_param(k) || has_param(v),
        Type::Union(ts) | Type::Tuple(ts) => ts.iter().any(has_param),
        Type::Record(fs) => fs.iter().any(|(_, t)| has_param(t)),
        Type::Enum { args, .. } => args.iter().any(has_param),
        _ => false,
    }
}

/// `Select(c, a, b)` of two records or enum values: the gate carries one wire,
/// so each leaf is chosen by its own `Select` on the one condition, as
/// `if c then a else b` chooses a record. The chosen value is stashed in
/// `pending_inline_record` as a record-returning call's is; the port returned
/// is its tag (a payload-free enum where one value is expected) or a leaf.
fn select_of_records(
    ctx: &mut LowerCtx<'_>,
    spec: &crate::catalog::calls::CallSpec,
    bound: &HashMap<&str, &Expr>,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    if spec.name != "Select"
        || !matches!(
            unwrap_ref(&ctx.type_of(e)),
            Type::Record(_) | Type::Enum { .. }
        )
    {
        return None;
    }
    let [cond, a, b] =
        [0, 1, 2].map(|i| spec.params.get(i).and_then(|p| bound.get(p.name).copied()));
    let (cond, a, b) = (cond?, a?, b?);
    let cond_port = lower_expr(ctx, cond);
    let a_fields = choice_branch_fields(ctx, a, e.range())?;
    let b_fields = choice_branch_fields(ctx, b, e.range())?;
    // `b` when the condition is true, `a` when it is false.
    let rec = select_record_fields(ctx, cond_port, &b_fields, &a_fields, range);
    if let Some(outputs) = ctx.gate_outputs(e.range()) {
        mark_gate_read_choice(ctx, &rec, outputs);
    }
    let port = in_declared_order(ctx, &rec, e.range())
        .into_iter()
        .find_map(|f| binding_to_port(ctx, f, range))?;
    ctx.pending_inline_record = Some(rec);
    Some(port)
}

/// Whether `e` is a `Select` builtin call, whose stashed record is the chosen
/// value itself rather than a record of named outputs.
pub(in crate::lower) fn is_builtin_select(ctx: &LowerCtx<'_>, e: &Expr) -> bool {
    matches!(e, Expr::Call { callee, .. } if matches!(callee.as_ref(), Expr::Ident { name, .. } if name == "Select"))
        && called_chip_decl(ctx, e).is_none()
}

/// One value wired into a builtin gate's port.
struct WireEntry {
    port: WirePort,
    val_port: PortRef,
}

/// Constant axes of `Vector2D` composite layout ports (`positionX` ->
/// "Position.X"), keyed by parent (e.g. "Position"), baked into the parent
/// `Vector2D` data field once every argument is seen. A runtime axis wires the
/// "Position.X" sub-port instead.
type Vec2Axes = HashMap<&'static str, (Option<f64>, Option<f64>)>;

/// What `bake_or_wire_param` did with one argument.
enum ParamOutcome {
    /// Baked into the gate's data, or dropped with a diagnostic.
    Handled,
    /// Left to wire into the port.
    Wire,
}

pub(in crate::lower) fn lower_builtin_call(
    ctx: &mut LowerCtx<'_>,
    spec: &crate::catalog::calls::CallSpec,
    receiver: Option<&Expr>,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // Expand `...tuple` spreads into per-element positional args before binding.
    let expanded = expand_spread_args(ctx, args);
    let args = &expanded[..];
    // An explicit `exec =` named arg allows an exec gate in a pure context.
    let explicit_exec = args.exec_arg();
    if spec.exec && ctx.current_exec.is_none() && explicit_exec.is_none() {
        return synthesise_unsupported(ctx, e);
    }

    let bound = bind_builtin_args(spec, receiver, args);

    if let Some(port) = select_of_records(ctx, spec, &bound, e, range) {
        return port;
    }

    // Arguments lower before the gate is added (adapters may advance exec).
    // The arguments wired rather than baked, lowered after this loop in source
    // order.
    let mut wired: Vec<(WirePort, &Expr)> = Vec::new();
    let mut arg_types: Vec<(WirePort, Option<Type>)> = Vec::new();
    let mut properties: HashMap<Sym, Literal> = HashMap::default();
    let mut vec2_axes = Vec2Axes::default();

    for p in &spec.params {
        let Some(&arg_expr) = bound.get(p.name) else {
            continue;
        };
        if let ParamOutcome::Wire =
            bake_or_wire_param(ctx, spec, p, arg_expr, &mut properties, &mut vec2_axes)
        {
            // Character and controller wire directly into each other's ports in
            // Brickadia, so no adapter gate is inserted between them (one would
            // need the admin-only `GetFromEntity`, blocked on paste for non-admins).
            // The argument's typechecked type types an `any` port better than the
            // wired port's type, which a mod return or loose catalog output leaves
            // as `any`.
            let arg_ty = ctx.one_value_type_of(arg_expr);
            arg_types.push((p.port, arg_ty));
            wired.push((p.port, arg_expr));
        }
    }
    let wires = lower_wired_args(ctx, &wired);

    bake_config_attrs(ctx, spec, args, &mut properties);

    // Constant layout axes bake into their parent Vector2D field, an unset
    // axis taking the gate's registered default (`anchorY = 0.25` gives
    // Anchor {X: 0.5, Y: 0.25}). A runtime axis wired above overrides its own.
    for (parent, (px, py)) in vec2_axes {
        let x = px.unwrap_or_else(|| vector2d_default_axis(spec.gate_class, parent, "X"));
        let y = py.unwrap_or_else(|| vector2d_default_axis(spec.gate_class, parent, "Y"));
        properties.insert(intern(parent), Literal::Vector { x, y, z: 0.0 });
    }

    // Most exec gates take exec on `Exec`, but a few pseudo gates
    // (Send[Global]CustomEvent, QueueTicks/QueueSeconds) name it `ExecIn`. The
    // game rejects a wire into a port the component lacks at load.
    let exec_in_port =
        if spec.exec && crate::catalog::is_wire_input(spec.gate_class, WirePort::ExecIn.as_str()) {
            WirePort::ExecIn
        } else {
            WirePort::Exec
        };

    let mut ports = builtin_port_types(ctx, spec, e, exec_in_port, &wires, &arg_types);
    fill_catalog_ports(spec, &mut ports);

    // `GetMicrochipBrick()` is a var emit fills with the program's own microchip brick.
    if spec.name == "GetMicrochipBrick" {
        properties.insert(intern(function::MICROCHIP_BRICK), Literal::Bool(true));
    }
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: spec.gate_class,
        source_range: range.clone(),
        ports,
        properties,
        note: Some(spec.name),
        ..Default::default()
    });

    if spec.exec {
        let exec_source = ctx
            .current_exec
            .or_else(|| explicit_exec.map(|e| lower_expr(ctx, e)));
        if let Some(exec) = exec_source {
            ctx.connect(exec, node_id.port(exec_in_port));
            if ctx.current_exec.is_some() {
                ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
            }
        }
    }
    for w in wires {
        ctx.connect(w.val_port, node_id.port(w.port));
    }

    // Named record outputs (Edge's rising/falling) stash a field->port record
    // so a `let` binding resolves fields through the spec. Always set for this
    // call, so a nested record-returning argument call cannot leak its record.
    ctx.pending_inline_record = if spec.outputs.iter().any(|o| o.field.is_some()) {
        let mut record: Fields = HashMap::default();
        for out in &spec.outputs {
            if let Some(field) = out.field {
                record.insert(
                    intern(field),
                    Binding::Local(LocalRecord {
                        port: node_id.port(out.port),
                    }),
                );
            }
        }
        Some(record)
    } else {
        None
    };

    if !spec.outputs.is_empty() {
        return node_id.port(spec.outputs[0].port);
    }
    if spec.exec {
        return node_id.port(WirePort::ExecOut);
    }
    if let Some(p) = spec.params.first() {
        return node_id.port(p.port);
    }
    node_id.port(WirePort::Output)
}

/// The argument expression each of the spec's params takes, by param name. A
/// method-call receiver is the first positional argument, and an argument past
/// the last param or a named one that names no param binds nothing.
fn bind_builtin_args<'a>(
    spec: &crate::catalog::calls::CallSpec,
    receiver: Option<&'a Expr>,
    args: &'a [CallArg],
) -> HashMap<&'a str, &'a Expr> {
    let mut bound: HashMap<&str, &Expr> = HashMap::default();
    let mut next_pos = 0usize;
    if let Some(recv) = receiver {
        if let Some(p) = spec.params.first() {
            bound.insert(p.name, recv);
        }
        next_pos = 1;
    }
    for a in args {
        match a {
            CallArg::Named { name, value, .. } => {
                if spec.params.iter().any(|p| p.name == name) {
                    bound.insert(name, value);
                }
            }
            CallArg::Positional(value) => {
                if let Some(p) = spec.params.get(next_pos) {
                    bound.insert(p.name, value);
                }
                next_pos += 1;
            }
            // `expand_spread_args` has replaced every spread by its elements.
            CallArg::Spread(_) => {}
        }
    }
    bound
}

/// Bake the constant argument `arg_expr` of param `p` into the gate's data, or
/// report that it takes a wire.
fn bake_or_wire_param(
    ctx: &mut LowerCtx<'_>,
    spec: &crate::catalog::calls::CallSpec,
    p: &crate::catalog::calls::CallParam,
    arg_expr: &Expr,
    properties: &mut HashMap<Sym, Literal>,
    vec2_axes: &mut Vec2Axes,
) -> ParamOutcome {
    // A dotted port is a Vector2D composite sub-port: a constant axis is
    // baked below, a runtime axis wires the float sub-port.
    if let Some((parent, axis)) = p.port.as_str().split_once('.') {
        let axis_val = match literal_for_property_port(ctx, arg_expr, &Type::Float, false) {
            Some(Literal::Float(f)) => Some(f),
            Some(Literal::Int(i)) => Some(i as f64),
            _ => None,
        };
        if let Some(f) = axis_val {
            let e = vec2_axes.entry(parent).or_insert((None, None));
            match axis {
                "X" => e.0 = Some(f),
                "Y" => e.1 = Some(f),
                _ => {}
            }
            return ParamOutcome::Handled;
        }
        // Runtime: fall through to the wire path below.
    }
    // Enum-typed config bakes the enum's int: a bare member name first,
    // then a folded enum value, then a constant naming a member or value
    // (as `typecheck::config::validate_enum_config_arg` resolves it). On a
    // wireable enum port (DisplayText's `justify`), anything else is a
    // runtime value and wires.
    if let Some(enum_type) =
        crate::catalog::config_field_enum_type(spec.gate_class, p.port.as_str())
    {
        if let Expr::Ident { name, .. } = arg_expr
            && let Some(v) = crate::catalog::enum_member_value(enum_type, name)
        {
            properties.insert(p.port.sym(), Literal::Int(v));
            return ParamOutcome::Handled;
        }
        if let Some(v) = enum_config_discriminant(ctx, arg_expr, enum_type) {
            properties.insert(p.port.sym(), Literal::Int(v));
            return ParamOutcome::Handled;
        }
        let resolved = match expr_to_literal_in(arg_expr, &ctx.const_lookup()) {
            Some(Literal::String(s)) => crate::catalog::enum_member_value(enum_type, &s),
            Some(Literal::Int(v)) if crate::catalog::enum_has_value(enum_type, v) => Some(v),
            _ => None,
        };
        if let Some(v) = resolved {
            properties.insert(p.port.sym(), Literal::Int(v));
            return ParamOutcome::Handled;
        }
    }
    // Composite constant-only config (meshColors: Color[], ammoOverride:
    // WeaponAmmoOverride) targets non-wire data fields: it folds into gate
    // data or reports WS028, never wires (a wire into a non-input port is
    // a silently broken gate).
    if !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str())
        && matches!(p.port, WirePort::MeshColors | WirePort::WeaponAmmoOverride)
    {
        let folded = {
            let consts = ctx.const_lookup();
            match p.port {
                WirePort::MeshColors => fold_mesh_colors(arg_expr, &consts),
                _ => fold_ammo_override(arg_expr, &consts),
            }
        };
        match folded {
            Some(lit) => {
                properties.insert(intern(p.port.as_str()), lit);
            }
            // Reported here too in case typecheck and lowering diverge.
            None => {
                ctx.error(
                    "WS028",
                    format!(
                        "'{}' is a constant-only config field, and its value did not resolve \
                         to a constant during lowering - the setting would be dropped and the \
                         gate would use its default",
                        p.port.as_str()
                    ),
                    arg_expr.range(),
                );
            }
        }
        return ParamOutcome::Handled;
    }
    // A constant argument is inlined as a property in the data struct. A
    // port with no wire pin takes the full const evaluator, as typecheck's
    // `config::validate_scalar_config_arg` does; a wire-capable port keeps
    // the narrow fold (see `literal_for_property_port`).
    let config_only = !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str());
    if let Some(lit) = literal_for_property_port(ctx, arg_expr, &p.ty, config_only) {
        // Struct-valued constants (Vec/Rotation/Color) inline only into a
        // wire-variant data field; other gates get a wired Make* gate
        // from the materialize pass. A rerouter has no data struct, so
        // an inlined property would be dropped at emit: it always wires.
        let inlinable = spec.gate_class != gc::REROUTER
            && (!matches!(
                lit,
                Literal::Vector { .. }
                    | Literal::Rotator { .. }
                    | Literal::Quat { .. }
                    | Literal::LinearColor { .. }
            ) || crate::emit::port_accepts_inline_variant(spec.gate_class, p.port, &lit));
        if inlinable {
            properties.insert(intern(p.port.as_str()), lit);
            return ParamOutcome::Handled;
        }
    }
    // A data-only config port has no pin: a wire into it loads as a silent
    // "Failed to connect wire". Its value did not fold, so report WS028;
    // typecheck folds constants in a separate environment and may have
    // accepted it, leaving the gate on its default (an empty event channel).
    if !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str()) {
        ctx.error(
            "WS028",
            format!(
                "'{}' is a constant-only config field, and its value did not resolve to a \
                 constant during lowering, so the setting would be dropped and the gate would \
                 use its default",
                p.port.as_str()
            ),
            arg_expr.range(),
        );
        return ParamOutcome::Handled;
    }
    ParamOutcome::Wire
}

/// Lower the wired arguments in source order, each fixed at its value before a
/// later one runs code that may change what it reads.
fn lower_wired_args(ctx: &mut LowerCtx<'_>, wired: &[(WirePort, &Expr)]) -> Vec<WireEntry> {
    let mut order: Vec<usize> = (0..wired.len()).collect();
    order.sort_by_key(|&i| wired[i].1.range().start.offset);
    let exprs: Vec<&Expr> = order.iter().map(|&i| wired[i].1).collect();
    let mut ports = vec![None; wired.len()];
    for (i, port) in order.into_iter().zip(ordered::lower_in_order(ctx, &exprs)) {
        ports[i] = Some(port);
    }
    wired
        .iter()
        .zip(ports)
        .map(|((port, _), val_port)| WireEntry {
            port: *port,
            val_port: val_port.expect("every argument lowered"),
        })
        .collect()
}

/// Named args that name a settings-menu config field
/// (`bOnlyHitPlayerBodyParts = true`) rather than a declared param bake
/// into the gate's data-struct field by raw name; enum members bake as int.
fn bake_config_attrs(
    ctx: &mut LowerCtx<'_>,
    spec: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    properties: &mut HashMap<Sym, Literal>,
) {
    for a in args {
        if let CallArg::Named { name, value, .. } = a {
            if spec.params.iter().any(|p| p.name == name) {
                continue;
            }
            let Some(cfg) = crate::catalog::scalar_config_field(spec.gate_class, name) else {
                continue;
            };
            let lit = match cfg.ty.as_str() {
                // A bare member name or quoted name resolves to the enum's int;
                // a raw int literal passes straight through.
                "enum" => {
                    let et = crate::catalog::config_field_enum_type(spec.gate_class, &cfg.name);
                    match value {
                        Expr::Ident { name: member, .. }
                        | Expr::StringLit { value: member, .. } => et
                            .and_then(|e| crate::catalog::enum_member_value(e, member))
                            .map(Literal::Int),
                        // A qualified enum value (`EasingFunction.Bounce`)
                        // bakes its discriminant, as in the param loop above;
                        // an int literal takes the narrow property fold.
                        _ => et
                            .and_then(|e| enum_config_discriminant(ctx, value, e).map(Literal::Int))
                            .or_else(|| literal_for_property_port(ctx, value, &Type::Int, false)),
                    }
                }
                "bool" => literal_for_property_port(ctx, value, &Type::Bool, false),
                "int" => literal_for_property_port(ctx, value, &Type::Int, false),
                "float" => literal_for_property_port(ctx, value, &Type::Float, false),
                "string" => literal_for_property_port(ctx, value, &Type::String, false),
                _ => None,
            };
            match lit {
                Some(lit) => {
                    properties.insert(intern(cfg.name.as_str()), lit);
                }
                // A scalar config field that did not fold reports WS028, as in
                // the param loop above, rather than ship the gate's default. A
                // non-scalar cfg type is not bakeable here.
                None if matches!(
                    cfg.ty.as_str(),
                    "bool" | "int" | "float" | "string" | "enum"
                ) =>
                {
                    ctx.error(
                        "WS028",
                        format!(
                            "'{}' is a constant-only config field, and its value did not resolve \
                             to a constant during lowering - the setting would be dropped and the \
                             gate would use its default",
                            cfg.name
                        ),
                        value.range(),
                    );
                }
                None => {}
            }
        }
    }
}

/// The gate's port list as a call site types it: exec ports for an exec gate,
/// then each param and output, a generic port typed from its argument.
fn builtin_port_types(
    ctx: &mut LowerCtx<'_>,
    spec: &crate::catalog::calls::CallSpec,
    e: &Expr,
    exec_in_port: WirePort,
    wires: &[WireEntry],
    arg_types: &[(WirePort, Option<Type>)],
) -> GateIO {
    let mut ports = GateIO::default();
    if spec.exec {
        ports
            .inputs
            .push(PortSpec::exec(intern(exec_in_port.as_str())));
        ports.outputs.push(PortSpec::exec(*sym::EXEC_OUT));
    }
    // Generic passthrough builtins (`Sleep`, `Select`, `Swap` and so on) declare
    // value ports as `Type::Param("T")`. The substitution is recovered by
    // unifying each param with its argument's typechecked type and each
    // output with the call's (or its record field), since emit cannot type a
    // bare `Type::Param`.
    let param_subst: crate::types::infer::Subst = {
        let mut constraints = Vec::new();
        for (port, arg_ty) in arg_types {
            if let (Some(p), Some(t)) = (spec.params.iter().find(|p| p.port == *port), arg_ty) {
                crate::types::mono::collect(&p.ty, &unwrap_ref(t), &mut constraints);
            }
        }
        if let Some(call_ty) = ctx.type_at(e.range()) {
            match (spec.outputs.as_slice(), unwrap_ref(&call_ty)) {
                ([out], t) => crate::types::mono::collect(&out.ty, &t, &mut constraints),
                (outs, Type::Record(fields)) => {
                    for out in outs {
                        let name = crate::catalog::arrays::field_name_ref(out.port.as_str());
                        if let Some((_, t)) = fields
                            .iter()
                            .find(|(n, _)| n == name || n == out.port.as_str())
                        {
                            crate::types::mono::collect(&out.ty, t, &mut constraints);
                        }
                    }
                }
                _ => {}
            }
        }
        constraints
            .into_iter()
            .map(|crate::types::infer::Constraint::Eq(n, t)| (n, t))
            .collect()
    };
    // A type parameter no port pins is a compiler fault, not a program's: the
    // port would ship as `any` (the float variant) whatever the value is.
    let mut unpinned = false;
    let variant = math_variant(ctx, spec, e);
    let mut concrete = |t: &Type| {
        if let (Type::Union(_), Some(v)) = (t, &variant) {
            return v.clone();
        }
        let t = crate::types::mono::substitute(t, &param_subst);
        unpinned |= has_param(&t);
        scrub_param_type(t)
    };

    // A custom-event send declares its eight data params `any`, but the
    // receiver types its DataOut ports from the handler's annotation, and an
    // `any` port emits as float. So a wired data port takes its value's type;
    // an unwired one keeps `any` (float), matching the receiver's unused slots.
    // `SetCustomProperty`'s value is typed the same way.
    let types_any_from_wire = spec.gate_class == gc::SEND_CUSTOM_EVENT
        || spec.gate_class == gc::SEND_CUSTOM_EVENT_GLOBAL
        || spec.gate_class == gc::ENTITY_SET_CUSTOM_PROPERTY;
    for p in &spec.params {
        let mut ty = concrete(&p.ty);
        if types_any_from_wire
            && matches!(ty, Type::Any)
            && let Some(w) = wires.iter().find(|w| w.port == p.port)
        {
            // A wired ref keeps its ref type (a numeric variant turns a ref into
            // 1); else typecheck's argument type, unwrapped since a `*T` place
            // sends its target's value; else the wired port's. Both untyped
            // stays `any`, which typecheck reports as WS045.
            let concrete = |t: &Type| !matches!(t, Type::Any | Type::Opaque | Type::Exec);
            let wired = ctx.source_port_type(w.val_port);
            let from_ref = wired
                .clone()
                .filter(|t| matches!(t, Type::Ref(inner) if !matches!(**inner, Type::Array(_) | Type::Map(..))));
            let from_arg = arg_types
                .iter()
                .find(|(port, _)| *port == p.port)
                .and_then(|(_, t)| t.clone())
                .map(|t| unwrap_ref(&t))
                .filter(concrete);
            if let Some(t) = from_ref.or(from_arg).or_else(|| wired.filter(concrete)) {
                ty = t;
            }
        }
        ports
            .inputs
            .push(PortSpec::new(intern(p.port.as_str()), ty));
    }
    for out in &spec.outputs {
        let ty = concrete(&out.ty);
        ports
            .outputs
            .push(PortSpec::new(intern(out.port.as_str()), ty));
    }
    if unpinned && !ctx.template_build {
        ctx.error(
            crate::diagnostic::INTERNAL_ERROR,
            format!(
                "internal: no port of `{}` pins its type parameter, so it would be typed `any`",
                spec.name
            ),
            e.range(),
        );
    }
    ports
}

/// Add every catalog port of the gate that `ports` lacks, including ones the
/// `CallSpec` does not declare (`InputSplitter` has several outputs, its `CallSpec`
/// one).
fn fill_catalog_ports(spec: &crate::catalog::calls::CallSpec, ports: &mut GateIO) {
    if let Some(gate) = crate::catalog::default_catalog().find_by_class(spec.gate_class) {
        let existing: HashSet<Sym> = ports.all_port_names().collect();
        for p in &gate.component.inputs {
            let sym = intern(&p.name);
            if !existing.contains(&sym) {
                ports.inputs.push(PortSpec::new(sym, Type::Any));
            }
        }
        for p in &gate.component.outputs {
            let sym = intern(&p.name);
            if !existing.contains(&sym) {
                ports.outputs.push(PortSpec::new(sym, Type::Any));
            }
        }
    }
}

/// The registered default for one axis (`"X"`/`"Y"`) of a gate's `Vector2D` data
/// field, read from brdb's `STRUCT_DEFAULTS` as emit reads it. `0.0` when the
/// gate, field or axis has no registered default.
fn vector2d_default_axis(gate_class: &str, field: &str, axis: &str) -> f64 {
    let Some(strct) = brdb::component_db::COMPONENT_TYPE_STRUCT_PAIRS
        .iter()
        .find(|(c, _)| *c == gate_class)
        .map(|(_, s)| *s)
    else {
        return 0.0;
    };
    let Some(value) = brdb::component_db::STRUCT_DEFAULTS
        .iter()
        .find(|(s, _)| *s == strct)
        .and_then(|(_, fs)| fs.iter().find(|(n, _)| *n == field))
        .map(|(_, v)| v.as_ref())
    else {
        return 0.0;
    };
    let schema = brdb::schemas::bricks_components_schema_max();
    let Some(id) = schema.intern.get(axis) else {
        return 0.0;
    };
    value
        .as_brdb_struct_prop_value(schema, id, id)
        .ok()
        .and_then(|p| p.as_brdb_f64().ok())
        .unwrap_or(0.0)
}
