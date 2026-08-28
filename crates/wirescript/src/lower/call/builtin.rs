//! Lower a builtin call: `CallSpec` to gate node.

use super::*;

/// The integer discriminant of a built-in enum value passed as gate config
/// (`function = EasingFunction.Bounce`, or a constant that folds to an enum
/// value). Reuses the compiler's `.Discriminant` variant-path fold: const
/// evaluation folds the value to its `{ __disc: N }` record and this reads
/// `__disc` back, so the baked int is the same one the bare member name bakes.
/// `None` when the argument does not fold to an enum value (an int or a
/// member-name string falls through to the caller's other resolution paths),
/// or its discriminant is not a member of `enum_type` (typecheck's WS003
/// already rejects a mismatched enum, so a well-typed program never hits that).
fn enum_config_discriminant(ctx: &LowerCtx, arg_expr: &Expr, enum_type: &str) -> Option<i64> {
    let lookup = |n: &str| ctx.resolve_mod(n);
    let mut budget = crate::const_eval::Budget::default();
    let lit = crate::const_eval::eval_expr(arg_expr, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()?;
    let Literal::Record(fields) = lit else {
        return None;
    };
    let (_, disc) = fields.into_iter().find(|(n, _)| n == "__disc")?;
    let Literal::Int(v) = disc else { return None };
    crate::catalog::enum_has_value(enum_type, v).then_some(v)
}

pub(in crate::lower) fn lower_builtin_call(
    ctx: &mut LowerCtx,
    spec: &crate::catalog::calls::CallSpec,
    receiver: Option<&Expr>,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // Expand `...tuple` spreads into per-element positional args before binding.
    let expanded = expand_spread_args(ctx, args);
    let args = &expanded[..];
    // Check for explicit `exec` named arg — allows exec gates in pure contexts
    let explicit_exec = args.iter().find_map(|a| match a {
        CallArg::Named { name, value, .. } if name == "exec" => Some(value),
        _ => None,
    });
    if spec.exec && ctx.current_exec.is_none() && explicit_exec.is_none() {
        return synthesise_unsupported(ctx, e);
    }

    let mut bound: HashMap<&str, &Expr> = HashMap::default();
    let mut next_pos = 0usize;
    // A method-call receiver is the first positional argument.
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
            CallArg::Spread(_) => {
                // TODO: handle spread in call lowering
            }
        }
    }

    // Lower args first (adapters may advance exec)
    struct WireEntry {
        port: WirePort,
        val_port: PortRef,
    }
    let mut wires: Vec<WireEntry> = Vec::new();
    let mut arg_types: Vec<(WirePort, Option<Type>)> = Vec::new();
    let mut properties: HashMap<crate::intern::Sym, Literal> = HashMap::default();
    // Vector2D composite layout ports (`positionX` -> "Position.X"): constant
    // axes accumulate here (keyed by parent, e.g. "Position") and bake the parent
    // Vector2D data field after the loop; runtime axes fall through to wire the
    // "Position.X" sub-port.
    let mut vec2_axes: HashMap<&'static str, (Option<f64>, Option<f64>)> = HashMap::default();

    for p in spec.params.iter() {
        let Some(&arg_expr) = bound.get(p.name) else {
            continue;
        };
        // A dotted port is a Vector2D composite sub-port. Accumulate a constant
        // axis (baked below); let a runtime axis fall through to the wire path
        // (is_wire_input("Position.X") is true, so it wires the float sub-port).
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
                continue;
            }
            // Runtime: fall through to the wire path below.
        }
        // Enum-typed config: resolve to the enum's integer value and inline as
        // gate data. A bare member name (`function = Bounce`) resolves against
        // the schema; a qualified built-in enum value (`function =
        // EasingFunction.Bounce`) or a constant that folds to an enum value
        // reads its discriminant off the folded `{ __disc: N }` record, reusing
        // the same `.Discriminant` variant-path fold const evaluation performs,
        // so the baked int matches the bare member name exactly. Int and
        // quoted-name forms fall through to the literal path below (the emitter
        // resolves those). Typecheck (WS028/WS003) already validated membership
        // and enum identity.
        //
        // When a bare identifier is NOT a member name, mirror
        // `typecheck::config::validate_enum_config_arg`'s fallback: resolve it
        // through the constant environment (`function = EASE` for `const EASE =
        // "Bounce"`) instead. Member-name interpretation stays first, so no
        // program that relies on it changes meaning.
        if !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str())
            && let Some(enum_type) =
                crate::catalog::config_field_enum_type(spec.gate_class, p.port.as_str())
        {
            if let Expr::Ident { name, .. } = arg_expr
                && let Some(v) = crate::catalog::enum_member_value(enum_type, name)
            {
                properties.insert(intern(p.port.as_str()), Literal::Int(v));
                continue;
            }
            if let Some(v) = enum_config_discriminant(ctx, arg_expr, enum_type) {
                properties.insert(intern(p.port.as_str()), Literal::Int(v));
                continue;
            }
            let resolved = match expr_to_literal_in(arg_expr, &ctx.const_lookup()) {
                Some(Literal::String(s)) => crate::catalog::enum_member_value(enum_type, &s),
                Some(Literal::Int(v)) if crate::catalog::enum_has_value(enum_type, v) => Some(v),
                _ => None,
            };
            if let Some(v) = resolved {
                properties.insert(intern(p.port.as_str()), Literal::Int(v));
                continue;
            }
        }
        // Composite constant-only config (meshColors: Color[], ammoOverride:
        // WeaponAmmoOverride nested struct). These target NON-wire data fields,
        // so a value that can't fold to a constant must never fall through to
        // the wire path below (a wire into a non-input port is a silent broken
        // gate). Fold it into gate data, or drop it — typecheck (WS028) has
        // already flagged the non-constant case.
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
                // Never silently drop: a composite config value that did not
                // fold to a constant is the same WS028 violation the scalar
                // paths report, surfaced here if typecheck and lowering diverge.
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
            continue;
        }
        // Literal check — inline constant arguments as properties so they
        // go into the data struct. With negative literal folding in the
        // parser, all constant args (positive and negative) are consistent.
        // A port with no wire pin is constant-only config: allow the full
        // const evaluator, matching what typecheck's
        // `config::validate_scalar_config_arg` accepts for it. A wire-capable
        // port keeps the narrow fold (see `literal_for_property_port`).
        let config_only = !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str());
        if let Some(lit) = literal_for_property_port(ctx, arg_expr, &p.ty, config_only) {
            // Struct-valued constants (folded Vec/Rotation/Color) only
            // inline when the gate's data field is a wire variant; other
            // gates (entity Set*, Split*) need a wired Make* gate, which
            // the fallthrough + materialize pass provides.
            //
            // Rerouter (`Opaque`) has no data struct at all — an inlined
            // property would just be silently dropped at emit time, so it
            // must always keep a real wired literal source instead.
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
                continue;
            }
        }
        // A data-only config port (settings-menu field, not a wire input) has
        // no pin to wire into. Reaching here means the value wasn't a foldable
        // constant — drop it rather than emit a wire into a nonexistent pin
        // (which loads as a silent "Failed to connect wire", with the config
        // never applied).
        //
        // Typecheck's WS028 normally reports the non-constant value before this
        // point, but typecheck and lowering resolve constants through separate
        // environments, so a name typecheck can fold and lowering cannot reaches
        // here with nothing reported by anybody, and the config would silently
        // ship as the type's default — an empty custom-event channel name, for
        // instance, which is a gate that quietly never fires. Report it here too,
        // under the same WS028 code, since it's the same rule being violated,
        // just detected a stage later.
        if !crate::catalog::is_wire_input(spec.gate_class, p.port.as_str()) {
            ctx.error(
                "WS028",
                format!(
                    "'{}' is a constant-only config field, and its value did not resolve to a \
                     constant during lowering — the setting would be dropped and the gate would \
                     use its default",
                    p.port.as_str()
                ),
                arg_expr.range(),
            );
            continue;
        }
        let val_port = lower_expr(ctx, arg_expr);
        // character and controller wire directly into each other's ports in
        // Brickadia, so no adapter gate is inserted for a character passed to
        // a controller param (or vice versa) — an adapter would need
        // `GetFromEntity` ("Get Player (Persistent)"), an admin-only gate that
        // gets blocked on paste for non-admins.
        // Remember the argument's typechecked type. It is strictly better than the
        // wired node's port type for typing an `any` port: a mod return (`aimHit()
        // -> entity`) or a gate output the catalog declares loosely both reach emit
        // as `any`, while typecheck has already resolved them to the real type.
        let arg_ty = {
            let r = arg_expr.range();
            ctx.type_of_expr
                .get(&(r.file.clone(), r.start.offset, r.end.offset))
                .cloned()
        };
        arg_types.push((p.port, arg_ty));
        wires.push(WireEntry {
            port: p.port,
            val_port,
        });
    }

    // Data-driven config attributes: named args that name a settings-menu config
    // field (`bOnlyHitPlayerBodyParts = true`) rather than a declared param. Bake
    // each constant into the gate's data-struct field, keyed by the raw field
    // name; enum members resolve to their int. Typecheck (WS028) already
    // validated constant-ness / membership.
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
                        // A qualified built-in enum value (`Function =
                        // EasingFunction.Bounce`) reads its discriminant off the
                        // folded `{ __disc: N }` record, the same integer the
                        // bare member name bakes; the declared-param loop above
                        // does the identical thing. Falls back to the narrow
                        // property evaluator for an int literal.
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
                // Same rule the declared-param loop enforces above: a scalar
                // config field we tried to bake but could not resolve to a
                // constant is never silently dropped (the gate would ship its
                // default), so report WS028 when typecheck's acceptance and
                // lowering's fold diverge. A non-scalar cfg type is not bakeable
                // on this path and is left alone.
                None if matches!(cfg.ty.as_str(), "bool" | "int" | "float" | "string" | "enum") => {
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

    // Bake accumulated constant layout axes into their parent Vector2D data
    // field, filling any axis the caller left unset from the gate's registered
    // default (so `anchorY = 0.25` -> Anchor {X: 0.5 default, Y: 0.25}). A runtime
    // axis wired above overrides its own sub-port at load time.
    for (parent, (px, py)) in vec2_axes {
        let x = px.unwrap_or_else(|| vector2d_default_axis(spec.gate_class, parent, "X"));
        let y = py.unwrap_or_else(|| vector2d_default_axis(spec.gate_class, parent, "Y"));
        properties.insert(intern(parent), Literal::Vector { x, y, z: 0.0 });
    }

    // Most exec gates take their incoming exec on an `Exec` input, but a few
    // pseudo gates (Send[Global]CustomEvent, QueueTicks/QueueSeconds) name it
    // `ExecIn`. Wire into whichever the real component actually exposes, or the
    // game rejects the connection ("port Exec does not exist in target
    // component") when the save loads.
    let exec_in_port = if spec.exec
        && crate::catalog::is_wire_input(spec.gate_class, WirePort::ExecIn.as_str())
    {
        WirePort::ExecIn
    } else {
        WirePort::Exec
    };

    let mut ports = GateIO::default();
    if spec.exec {
        ports.inputs.push(PortSpec {
            name: intern(exec_in_port.as_str()),
            ty: Type::Exec,
        });
        ports.outputs.push(PortSpec {
            name: *sym::EXEC_OUT,
            ty: Type::Exec,
        });
    }
    // A custom-event send declares its eight data params as `any` (the channel
    // accepts anything), but the RECEIVER types its DataOut ports from the
    // handler's annotation. If the send keeps `any`, emit falls through to the
    // float variant and the two gates disagree on every payload that is not a
    // float — invisible for strings, fatal when an object reference arrives as
    // a number.
    //
    // Take the port's type from the value actually wired into it. Only a port
    // that HAS a wire is retyped: an unused data slot must keep `any` so it
    // still emits float, which is what the receiver's unused slots default to —
    // retyping those would just move the mismatch to slots n+1..8.
    //
    // When the wired value has NO concrete type of its own (`any`, or an
    // `Opaque(...)` that erased it) there is nothing to propagate. Warn rather
    // than guess: inventing a type here would silently pick a variant the
    // receiver may not declare, which is the same class of bug in a new coat.
    let is_event_send = spec.gate_class == gc::PSEUDO_SEND_CUSTOM_EVENT
        || spec.gate_class == gc::PSEUDO_SEND_CUSTOM_EVENT_GLOBAL;
    for p in spec.params.iter() {
        let mut ty = p.ty.clone();
        if is_event_send
            && matches!(ty, Type::Any)
            && let Some(w) = wires.iter().find(|w| w.port == p.port)
        {
            // Prefer typecheck's view of the argument, falling back to the wired
            // node's port type. Nothing to propagate when BOTH are untyped: leave
            // `any` (float) and let typecheck's WS045 report it, rather than
            // guessing a variant the receiver may not declare.
            let concrete = |t: &Type| !matches!(t, Type::Any | Type::Opaque | Type::Exec);
            let from_arg = arg_types
                .iter()
                .find(|(port, _)| *port == p.port)
                .and_then(|(_, t)| t.clone())
                .map(|t| unwrap_ref(&t))
                .filter(concrete);
            if let Some(t) = from_arg.or_else(|| {
                ctx.source_port_type(w.val_port).filter(concrete)
            }) {
                ty = t;
            }
        }
        ports.inputs.push(PortSpec {
            name: intern(p.port.as_str()),
            ty,
        });
    }
    for out in &spec.outputs {
        ports.outputs.push(PortSpec {
            name: intern(out.port.as_str()),
            ty: out.ty.clone(),
        });
    }

    // Ensure all gate ports are present — the catalog may define ports
    // not covered by the CallSpec params/output (e.g. InputSplitter has
    // multiple outputs but CallSpec only declares one).
    if let Some(gate) = crate::catalog::default_catalog().find_by_class(spec.gate_class) {
        let existing: std::collections::HashSet<crate::intern::Sym> =
            ports.all_port_names().collect();
        for p in &gate.component.inputs {
            let sym = intern(&p.name);
            if !existing.contains(&sym) {
                ports.inputs.push(PortSpec {
                    name: sym,
                    ty: Type::Any,
                });
            }
        }
        for p in &gate.component.outputs {
            let sym = intern(&p.name);
            if !existing.contains(&sym) {
                ports.outputs.push(PortSpec {
                    name: sym,
                    ty: Type::Any,
                });
            }
        }
    }

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: spec.gate_class,
        source_range: range.clone(),
        ports,
        properties,
        note: Some(spec.name.into()),
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

    // Named record outputs (e.g. Edge's rising/falling): stash a field->port
    // record so a `let` binding resolves fields through the spec instead of
    // port-name matching. Set definitively for THIS call — `None` otherwise —
    // so a nested record-returning arg call doesn't leak into the outer let.
    ctx.pending_inline_record = if spec.outputs.iter().any(|o| o.field.is_some()) {
        let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::default();
        for out in &spec.outputs {
            if let Some(field) = out.field {
                record.insert(
                    crate::intern::intern(field),
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

    if spec.outputs.len() == 1 {
        return node_id.port(spec.outputs[0].port);
    }
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

/// The registered default for one axis (`"X"`/`"Y"`) of a gate's `Vector2D` data
/// field, read from brdb's `STRUCT_DEFAULTS` — the same source emit uses. Fills
/// the axis a per-axis layout call left unset (e.g. `anchorY = 0.25` needs the
/// default `Anchor.X`). `0.0` when the gate/field/axis has no registered default.
#[cfg(feature = "brdb-full")]
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

#[cfg(not(feature = "brdb-full"))]
fn vector2d_default_axis(_gate_class: &str, _field: &str, _axis: &str) -> f64 {
    0.0
}
