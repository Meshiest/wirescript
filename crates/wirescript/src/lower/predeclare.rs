use super::*;

// ---------- pre-declaration pass ----------

pub(super) fn pre_declare_decl(ctx: &mut LowerCtx, d: &TopDecl) {
    match d {
        TopDecl::Var(v) => pre_declare_var(ctx, v),
        TopDecl::Array(a) => pre_declare_array(ctx, a),
        TopDecl::Buffer(b) => pre_declare_buffer(ctx, b),
        TopDecl::In(i) => pre_declare_input(ctx, i),
        TopDecl::Out(o) => pre_declare_output(
            ctx,
            &o.name,
            o.value.as_ref(),
            o.typ.as_ref(),
            o.side,
            o.label.as_deref(),
            &o.range,
        ),
        TopDecl::Let(l) => pre_declare_exec_signal(ctx, l),
        TopDecl::AnonChip(ac) => {
            let chip_node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::MICROCHIP_ALT,
                source_range: ac.range.clone(),
                ports: GateIO::default(),
                ..Default::default()
            });
            if let Some(node) = ctx.builder.module.nodes.get_mut(&chip_node_id) {
                node.kind = NodeKind::Chip;
                let props = std::sync::Arc::make_mut(&mut node.properties);
                if ac.closed {
                    props.insert(*sym::CHIP_CLOSED, Literal::Bool(true));
                }
                if let Some(label) = &ac.label {
                    props.insert(*sym::NAME_LABEL, Literal::String(label.clone()));
                }
                if let Some(doc) = ctx.doc_comments.get(&ac.range.start.offset) {
                    props.insert(*sym::DOC_TEXT, Literal::String(doc.clone()));
                }
            }
            // Tag pre-declared nodes with chip_id.
            let saved = ctx.current_anon_chip.take();
            ctx.current_anon_chip = Some(chip_node_id);
            for s in &ac.body.stmts {
                match s {
                    Stmt::Var(v) => pre_declare_var(ctx, v),
                    Stmt::Buffer(b) => pre_declare_buffer(ctx, b),
                    Stmt::Array(a) => pre_declare_array(ctx, a),
                    Stmt::In(i) => pre_declare_input(ctx, i),
                    Stmt::OutBinding(o) if o.side.is_some() => {
                        report_non_root_side(ctx, &o.range);
                    }
                    _ => {}
                }
            }
            ctx.current_anon_chip = saved;
        }
        _ => {}
    }
}

/// Pre-declare a top-level `let x: exec` local signal: create a stable Union
/// "hub" gate, bind `x` to its `ExecOut` (so `on x` can trigger off it), and
/// register the emit target. `flush_pending_emits` later wires the union of all
/// `emit x` paths into the hub's `ExecA`. Non-`exec` lets are ignored here (they
/// lower normally in pass 2).
pub(super) fn pre_declare_exec_signal(ctx: &mut LowerCtx, l: &LetDecl) {
    let Some(TypeExpr::Name {
        name: type_name, ..
    }) = &l.typ
    else {
        return;
    };
    if type_name != "exec" {
        return;
    }
    let LetBinding::Ident { name, .. } = &l.binding else {
        return;
    };
    build_exec_signal_hub(ctx, name, &l.range);
}

/// Create the stable `Union` "hub" for a local `let x: exec` signal: bind `x`
/// to its `ExecOut` (so `await x` / `on x` / reads resolve to it) and register
/// the emit target. `flush_pending_emits` later wires the union of all `emit x`
/// paths into the hub's `ExecA`. Used for both top-level signals (this
/// pre-declare pass) and body-level signals (from `lower_let_decl`).
pub(super) fn build_exec_signal_hub(ctx: &mut LowerCtx, name: &str, range: &SourceRange) {
    let hub = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNION,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC_A,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::EXEC_B,
                    ty: Type::Exec,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::EXEC_OUT,
                ty: Type::Exec,
            }],
        },
        ..Default::default()
    });
    ctx.scope.insert(
        &name,
        Binding::Local(LocalRecord {
            port: hub.port(WirePort::ExecOut),
        }),
    );
    // Key the signal per-declaration (`name#hubId`), not by bare name: two
    // bodies declaring the same signal name are distinct signals. Emit/await
    // sites resolve the key through the scope binding (`LowerCtx::signal_key`).
    let key = format!("{name}#{hub}");
    ctx.exec_signal_hubs.insert(key.clone(), hub);
    ctx.exec_signal_keys.insert(hub, key.clone());
    ctx.pending_emits.entry(key).or_default();
}

pub(super) fn type_of_type_expr(t: &TypeExpr) -> Type {
    match t {
        TypeExpr::Name { name, .. } => match name.as_str() {
            "bool" => Type::Bool,
            "int" => Type::Int,
            "float" => Type::Float,
            "string" => Type::String,
            "vector" => Type::Vector,
            "rotator" => Type::Rotator,
            "quat" => Type::Quat,
            "color" => Type::Color,
            "entity" => Type::Entity,
            "character" => Type::Character,
            "controller" => Type::Controller,
            "brick" => Type::Brick,
            "prefab" => Type::Prefab,
            "exec" => Type::Exec,
            "any" => Type::Any,
            _ => Type::Any,
        },
        TypeExpr::Ref { inner, .. } => Type::Ref(Box::new(type_of_type_expr(inner))),
        TypeExpr::Array { inner, .. } => Type::Array(Box::new(type_of_type_expr(inner))),
        TypeExpr::Tuple { fields, .. } => {
            Type::Tuple(fields.iter().map(type_of_type_expr).collect())
        }
        TypeExpr::Union { options, .. } => {
            Type::Union(options.iter().map(type_of_type_expr).collect())
        }
        TypeExpr::Record { fields, .. } => Type::Record(
            fields
                .iter()
                .map(|f| (f.name.clone(), type_of_type_expr(&f.typ)))
                .collect(),
        ),
    }
}

#[allow(dead_code)]
pub(super) fn is_entity_family(t: &Type) -> bool {
    matches!(
        t,
        Type::Controller | Type::Character | Type::Entity | Type::Brick | Type::Prefab
    )
}

pub(super) fn unwrap_ref(t: &Type) -> Type {
    match t {
        Type::Ref(inner) => inner.as_ref().clone(),
        other => other.clone(),
    }
}

/// Default initial literal for Pseudo_Var data structs. Only covers
/// primitive types that have a clean wire_graph_variant mapping.
/// Object/entity types are omitted — the game defaults them correctly.
/// Default initial literal for Pseudo_Var data structs so the game knows
/// the variable's wire_graph_variant type. Every Var must have one.
pub(super) fn default_literal_for_var_type(t: &Type) -> Option<Literal> {
    match t {
        Type::Bool => Some(Literal::Bool(false)),
        Type::Int => Some(Literal::Int(0)),
        Type::String => Some(Literal::String(String::new())),
        Type::Vector => Some(Literal::Vector {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }),
        Type::Rotator => Some(Literal::Rotator {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        }),
        Type::Quat => Some(Literal::Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }),
        Type::Color => Some(Literal::LinearColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }),
        Type::Controller | Type::Character | Type::Entity | Type::Brick | Type::Prefab => {
            Some(Literal::Object)
        }
        _ => Some(Literal::Float(0.0)),
    }
}

/// Fold a constant-literal expression to a [`Literal`] (used for var/array
/// initial values). Returns `None` for anything that isn't a compile-time
/// constant. Shared with the type checker so both agree on what's a literal.
pub fn expr_to_literal(e: &Expr) -> Option<Literal> {
    match e {
        Expr::IntLit { value, .. } => Some(Literal::Int(*value)),
        Expr::FloatLit { value, .. } => Some(Literal::Float(*value)),
        Expr::BoolLit { value, .. } => Some(Literal::Bool(*value)),
        Expr::StringLit { value, .. } => Some(Literal::String(value.clone())),
        // Negative numeric literals: `-5`, `-1.0`.
        Expr::UnOp { op, operand, .. } if op == "-" => match expr_to_literal(operand)? {
            Literal::Int(n) => Some(Literal::Int(-n)),
            Literal::Float(f) => Some(Literal::Float(-f)),
            _ => None,
        },
        // Constructor calls on constant numeric args fold to literals, so
        // `var v = Vec(1.0, 2.0, 3.0)` (and Rotation/Color) bakes into the
        // gate's initial value instead of being dropped.
        Expr::Call { callee, args, .. } => {
            let Expr::Ident { name, .. } = callee.as_ref() else {
                return None;
            };
            let mut nums = Vec::with_capacity(args.len());
            for a in args {
                let CallArg::Positional(arg) = a else {
                    return None;
                };
                match expr_to_literal(arg) {
                    Some(Literal::Int(n)) => nums.push(n as f64),
                    Some(Literal::Float(f)) => nums.push(f),
                    _ => return None,
                }
            }
            match (name.as_str(), nums.as_slice()) {
                ("Vec", &[x, y, z]) => Some(Literal::Vector { x, y, z }),
                ("Rotation", &[pitch, yaw, roll]) => Some(Literal::Rotator { pitch, yaw, roll }),
                // Color is linear RGBA 0–1; alpha defaults to opaque.
                ("Color", &[r, g, b]) => Some(Literal::LinearColor { r, g, b, a: 1.0 }),
                ("Color", &[r, g, b, a]) => Some(Literal::LinearColor { r, g, b, a }),
                _ => None,
            }
        }
        // Asset reference `$Type/Name` — inlined into the gate's component data.
        Expr::AssetRef {
            asset_type,
            asset_name,
            ..
        } => Some(Literal::Asset {
            asset_type: asset_type.clone(),
            asset_name: asset_name.clone(),
        }),
        // Prefab file reference `$./file.brz` — inlined; resolved + embedded
        // at emit into the gate's `bundle_path_ref` property.
        Expr::PrefabRef { path, .. } => Some(Literal::PrefabRef { path: path.clone() }),
        _ => None,
    }
}

/// Fold a single array-literal element to a constant [`Literal`]. Spreads have
/// no constant form (they're only valid in exec-context assignments), so they
/// fold to `None` — which makes the all-literal length check fail and the
/// initializer is left empty (the type checker has already reported the error).
fn array_elem_literal(el: &ArrayElem) -> Option<Literal> {
    match el {
        ArrayElem::Item(e) => expr_to_literal(e),
        ArrayElem::Spread(_) => None,
    }
}

/// A `var` initializer that can't bake into the gate as a constant: returns it
/// for diagnosis. `None` = no initializer, or it bakes fine.
fn var_init_unbaked(v: &VarDecl) -> Option<&Expr> {
    let init = v.init.as_ref()?;
    let unbaked = match init {
        Expr::Array { elements, .. } => elements.iter().any(|el| array_elem_literal(el).is_none()),
        e => expr_to_literal(e).is_none(),
    };
    unbaked.then_some(init)
}

/// Warn when a `var` initializer is silently dropped: it can't bake into the
/// Variable gate as a constant, and no exec-context reset will apply it (the
/// var is in pure position, or is `static`, which skips the per-entry reset) —
/// so the var starts at its type default. `skip_array_inits` avoids
/// double-reporting top-level array literals the type checker already errors
/// on.
pub(super) fn warn_unbaked_var_init(ctx: &mut LowerCtx, v: &VarDecl, skip_array_inits: bool) {
    let Some(init) = var_init_unbaked(v) else {
        return;
    };
    if skip_array_inits && matches!(init, Expr::Array { .. }) {
        return;
    }
    let msg = if v.is_static {
        format!(
            "'static var {}' initializer must be a compile-time constant — this value is dropped and the var starts at its type default",
            v.name
        )
    } else {
        format!(
            "'var {}' initializer is not a compile-time constant — outside an exec context it is dropped and the var starts at its type default; assign the value inside an exec handler instead",
            v.name
        )
    };
    ctx.warn(msg, init.range());
}

pub(super) fn pre_declare_var(ctx: &mut LowerCtx, d: &VarDecl) {
    let inner_type = d
        .typ
        .as_ref()
        .map(type_of_type_expr)
        .or_else(|| d.init.as_ref().map(|e| ctx.type_of(e)))
        .unwrap_or(Type::Any);

    // `var foo: T[]` is an array — desugar to an ArrayVar gate (same as an
    // `array foo: T[]` declaration) so the array methods actually work. A
    // `= [..]` initializer carries its constant literals like `array` does.
    if let Type::Array(elem) = &inner_type {
        let elem_type = elem.as_ref().clone();
        let mut properties = HashMap::default();
        properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
        if let Some(Expr::Array { elements, .. }) = &d.init {
            let lits: Vec<Literal> = elements.iter().filter_map(array_elem_literal).collect();
            if lits.len() == elements.len() {
                properties.insert(intern_static("InitialValue"), Literal::Array(lits));
            }
        }
        let node_id = ctx.add_gate(AddNodeOpts {
            gate_class: gc::PSEUDO_ARRAY_VAR,
            source_range: d.range.clone(),
            ports: GateIO {
                inputs: vec![],
                outputs: vec![PortSpec {
                    name: *sym::ARRAY_VAR_REF,
                    ty: Type::Ref(Box::new(Type::Array(Box::new(elem_type.clone())))),
                }],
            },
            properties,
            note: None,
            ..Default::default()
        });
        ctx.scope.insert(
            &d.name,
            Binding::Var(VarRecord {
                node_id,
                inner_type: elem_type,
                get_node_for_handler: None,
                storage: VarStorage::Array,
            }),
        );
        return;
    }

    let init_lit = d
        .init
        .as_ref()
        .and_then(expr_to_literal)
        .or_else(|| default_literal_for_var_type(&inner_type));
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    if let Some(lit) = init_lit {
        properties.insert(*sym::INITIAL_VALUE, lit);
    }

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_VAR,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::VAR_REF,
                    ty: Type::Ref(Box::new(inner_type.clone())),
                },
            ],
        },
        properties,
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord {
            node_id,
            inner_type,
            get_node_for_handler: None,
            storage: VarStorage::Var,
        }),
    );
}

pub(super) fn pre_declare_buffer(ctx: &mut LowerCtx, d: &BufferDecl) {
    let annotated = d.typ.as_ref().map(type_of_type_expr);
    let rhs_type = ctx.type_of(&d.init);
    let inner_type = annotated.unwrap_or_else(|| unwrap_ref(&rhs_type));

    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::BUFFER_TICKS,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::INPUT,
                    ty: inner_type.clone(),
                },
                PortSpec {
                    name: *sym::TICKS_TO_WAIT,
                    ty: Type::Int,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: inner_type.clone(),
            }],
        },
        properties: [(*sym::TICKS_TO_WAIT, Literal::Int(1))].into_iter().collect(),
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Buffer(NodeRecord {
            node_id,
            ty: inner_type,
        }),
    );
}

pub(super) fn pre_declare_array(ctx: &mut LowerCtx, d: &ArrayDecl) {
    let elem_type = type_of_type_expr(&d.element_type);
    // Constant initializer (`array foo: int[] = [1, 2, 3]`): every element must
    // be a literal. Carry the values as an `InitialValue` property the emitter
    // writes straight into the ArrayVar's array variant (no runtime gates).
    let mut properties = HashMap::default();
    properties.insert(*sym::NAME_LABEL, Literal::String(d.name.clone()));
    if !d.init.is_empty() {
        let lits: Vec<Literal> = d.init.iter().filter_map(array_elem_literal).collect();
        if lits.len() == d.init.len() {
            properties.insert(intern_static("InitialValue"), Literal::Array(lits));
        }
    }
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_ARRAY_VAR,
        source_range: d.range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::ARRAY_VAR_REF,
                ty: Type::Ref(Box::new(Type::Array(Box::new(elem_type.clone())))),
            }],
        },
        properties,
        note: None,
        ..Default::default()
    });
    ctx.scope.insert(
        &d.name,
        Binding::Var(VarRecord {
            node_id,
            inner_type: elem_type,
            get_node_for_handler: None,
            storage: VarStorage::Array,
        }),
    );
}

/// Push the WS023 "annotation on a non-root port" diagnostic. Shared so the
/// message text has a single source (apply_port_side and the anon-chip output
/// path both use it).
fn report_non_root_side(ctx: &mut LowerCtx, range: &SourceRange) {
    ctx.diagnostics.push(Diagnostic::error(
        "WS023",
        "side annotations only apply to top-level ports of the compiled file",
        range.clone(),
    ));
}

/// Attach a `@side` annotation to a freshly created I/O node, or reject it
/// with WS023 when the port doesn't belong to the root module (chip/mod
/// bodies, anonymous chips).
fn apply_port_side(
    ctx: &mut LowerCtx,
    node_id: NodeId,
    side: Option<crate::ast::PortSide>,
    range: &SourceRange,
) {
    let Some(side) = side else { return };
    if !ctx.is_root_module || ctx.current_anon_chip.is_some() {
        report_non_root_side(ctx, range);
        return;
    }
    if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
        std::sync::Arc::make_mut(&mut node.properties).insert(
            *crate::intern::sym::REROUTE_SIDE,
            Literal::String(side.as_str().to_string()),
        );
    }
}

pub(super) fn pre_declare_input(ctx: &mut LowerCtx, d: &InDecl) {
    let t = type_of_type_expr(&d.typ);
    let node_id = ctx
        .builder
        .add_input(&mut ctx.ids, &d.name, t.clone(), d.range.clone());
    apply_port_side(ctx, node_id, d.side, &d.range);
    if let Some(label) = &d.label {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label.clone()));
        }
    }
    ctx.scope.insert(
        &d.name,
        Binding::Input(NodeRecord { node_id, ty: t }),
    );
}

pub(super) fn pre_declare_output(
    ctx: &mut LowerCtx,
    name: &str,
    value: Option<&Expr>,
    typ: Option<&TypeExpr>,
    side: Option<crate::ast::PortSide>,
    label: Option<&str>,
    range: &SourceRange,
) {
    let t = if let Some(v) = value {
        unwrap_ref(&ctx.type_of(v))
    } else if let Some(te) = typ {
        type_of_type_expr(te)
    } else {
        Type::Any
    };
    let node_id = ctx
        .builder
        .add_output(&mut ctx.ids, name, t.clone(), range.clone());
    apply_port_side(ctx, node_id, side, range);
    if let Some(label) = label {
        if let Some(node) = ctx.builder.module.nodes.get_mut(&node_id) {
            std::sync::Arc::make_mut(&mut node.properties)
                .insert(*sym::NAME_LABEL, Literal::String(label.to_string()));
        }
    }
    ctx.scope.insert(
        &crate::lower::context::output_scope_key(name),
        Binding::Output(NodeRecord { node_id, ty: t }),
    );
}
