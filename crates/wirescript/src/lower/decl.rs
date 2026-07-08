use super::*;

// ---------- declaration body pass ----------

pub(super) fn lower_decl(ctx: &mut LowerCtx, d: &TopDecl) {
    match d {
        TopDecl::Out(b) => lower_out_binding(ctx, &b.name, b.value.as_ref(), &b.range),
        TopDecl::Handler(h) => lower_handler(ctx, h),
        TopDecl::Event(e) => lower_event_decl(ctx, e),
        TopDecl::Let(l) => lower_let_decl(ctx, l),
        TopDecl::Buffer(b) => lower_buffer_body(ctx, b),
        TopDecl::Var(_) | TopDecl::Array(_) | TopDecl::In(_) => {} // handled in pre-pass
        TopDecl::Chip(c) => lower_chip_decl(ctx, c),
        TopDecl::AnonChip(ac) => lower_anon_chip(ctx, ac),
        TopDecl::Assign(a) => lower_assign(ctx, a),
        TopDecl::If(i) => lower_if(ctx, i),
        TopDecl::ExprStmt(es) => {
            lower_expr(ctx, &es.expr);
        }
        TopDecl::Fn(f) => {
            // Deprecated: convert fn to inline mod with return value
            ctx.diagnostics.push(Diagnostic {
                severity: crate::diagnostic::Severity::Warning,
                code: "WS015".into(),
                message: format!(
                    "'fn {}' is deprecated — use 'mod {}({}) -> {} {{ return <body> }}'",
                    f.name,
                    f.name,
                    f.params
                        .iter()
                        .map(|p| format!(
                            "{}: {}",
                            p.name,
                            crate::analysis::types::type_expr_str(&p.typ)
                        ))
                        .collect::<Vec<_>>()
                        .join(", "),
                    f.return_type
                        .as_ref()
                        .map(crate::analysis::types::type_expr_str)
                        .unwrap_or_else(|| "auto".into()),
                ),
                range: f.range.clone(),
            });
            // Synthesize a ChipDecl from the FnDecl
            let outputs = if let Some(ref ret_type) = f.return_type {
                vec![NamedOutput {
                    name: "_".into(),
                    typ: ret_type.clone(),
                    range: f.range.clone(),
                }]
            } else {
                Vec::new()
            };
            let chip = ChipDecl {
                name: f.name.clone(),
                inputs: f.params.clone(),
                outputs,
                body: Block {
                    stmts: vec![Stmt::Return {
                        value: Some(f.body.clone()),
                        range: f.range.clone(),
                    }],
                    range: f.range.clone(),
                },
                range: f.range.clone(),
                inline: true,
            };
            lower_chip_decl(ctx, &chip);
        }
        TopDecl::Import(_) | TopDecl::TypeAlias(_) | TopDecl::Await(_) => {}
        TopDecl::Namespace(ns) => {
            let mut ns_decls = HashMap::new();
            for d in &ns.decls {
                if let TopDecl::Chip(c) = d {
                    ns_decls.insert(c.name.clone(), c.clone());
                }
            }
            ctx.scope
                .insert(ns.name.clone(), Binding::Namespace(ns_decls));
        }
    }
}

pub(super) fn lower_buffer_body(ctx: &mut LowerCtx, d: &BufferDecl) {
    let rec = match ctx.lookup_buffer(&d.name) {
        Some(r) => r.clone(),
        None => return,
    };
    let saved_chain = ctx.builder.current_chain_id;
    let chain = ctx.alloc_chain();
    ctx.builder.current_chain_id = Some(chain);
    if let Some(node) = ctx.builder.module.nodes.get_mut(&rec.node_id) {
        node.chain_id = Some(chain);
    }
    let rhs_port = lower_expr(ctx, &d.init);
    ctx.builder
        .connect(rhs_port, rec.node_id.port(WirePort::Input));
    ctx.builder.current_chain_id = saved_chain;
}

/// Anonymous chip: reuses the Chip node created during pre-declare.
/// Processes the body in the PARENT scope with chip_id set so nodes
/// get tagged for the emitter to route into a child grid.
pub(super) fn lower_anon_chip(ctx: &mut LowerCtx, d: &AnonChipDecl) {
    // Find the chip node that was created during pre_declare_decl.
    let chip_node_id = ctx
        .builder
        .module
        .nodes
        .iter()
        .find(|(_, n)| {
            n.kind == NodeKind::Chip
                && n.source_range == d.range
                && n.chip_id == ctx.current_anon_chip
        })
        .map(|(id, _)| *id);
    let Some(chip_node_id) = chip_node_id else {
        return;
    };

    let saved_chip = ctx.current_anon_chip.take();
    ctx.current_anon_chip = Some(chip_node_id);

    lower_block(ctx, &d.body);

    ctx.current_anon_chip = saved_chip;
}

pub(super) fn lower_chip_decl(ctx: &mut LowerCtx, d: &ChipDecl) {
    // Inline declarations (mod keyword or ref params) are stored for
    // expansion at call sites, not compiled as standalone microchips.
    let has_ref_params = d
        .inputs
        .iter()
        .any(|p| matches!(&p.typ, TypeExpr::Ref { .. } | TypeExpr::Array { .. }));
    if d.inline || has_ref_params {
        ctx.scope
            .insert(d.name.clone(), Binding::Chip(Box::new(d.clone())));
        return;
    }

    // Standalone chips: register for instantiation at call sites.
    ctx.scope
        .insert(d.name.clone(), Binding::Chip(Box::new(d.clone())));
}

pub(super) fn lower_let_decl(ctx: &mut LowerCtx, d: &LetDecl) {
    // Clear any leftover inline-mod record so only THIS statement's call can set
    // it (an inline mod call within `d.value` sets it definitively at its end).
    ctx.pending_inline_record = None;
    // `let name: exec` — local exec signal, register as emit target
    if let Some(TypeExpr::Name {
        name: ref type_name,
        ..
    }) = d.typ
    {
        if type_name == "exec" {
            if let LetBinding::Ident { name, .. } = &d.binding {
                // Top-level signals were already hubbed by the pre-declare
                // pass; skip the pass-2 revisit (no exec context at top level).
                // Body-level declarations always build a fresh hub — each
                // mod/handler instance is its own signal, even under the same
                // name (shadowing any outer signal).
                if ctx.current_exec.is_none() && ctx.signal_key(name).is_some() {
                    return;
                }
                build_exec_signal_hub(ctx, name, &d.range);
            }
            return;
        }
    }
    // Handle record literals specially — they produce a Binding::Record,
    // not a single PortRef.
    if let Expr::RecordLit { fields, .. } = &d.value {
        let record = lower_record_lit(ctx, fields);
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(name.clone(), Binding::Record(record));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &record, destruct_fields);
                return;
            }
            _ => {
                // Tuple/Record name destructuring on a record lit — fall through
                // to normal handling (unlikely but safe).
            }
        }
    }

    // Handle RHS that is an ident referencing a record binding.
    if let Expr::Ident { name: rhs_name, .. } = &d.value
        && let Some(Binding::Record(src)) = ctx.scope.get(rhs_name).cloned()
    {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(name.clone(), Binding::Record(src));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &src, destruct_fields);
                return;
            }
            _ => {}
        }
    }

    // Handle RHS that is a field-chain resolving to a record binding.
    if let Some(Binding::Record(src)) = resolve_field_chain(ctx, &d.value).cloned() {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(name.clone(), Binding::Record(src));
                return;
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &src, destruct_fields);
                return;
            }
            _ => {}
        }
    }

    let rhs_port = lower_expr(ctx, &d.value);
    let rhs_type = ctx.type_of(&d.value);

    // Multi-output inline mod: the call stashed a field→source-port record (its
    // output nodes are internal and were removed). Bind the record directly.
    if matches!(&d.value, Expr::Call { .. })
        && let Some(record) = ctx.pending_inline_record.take()
    {
        match &d.binding {
            LetBinding::Ident { name, .. } => {
                ctx.scope.insert(name.clone(), Binding::Record(record));
            }
            LetBinding::RecordDestruct {
                fields: destruct_fields,
                ..
            } => {
                install_record_destruct(ctx, &record, destruct_fields);
            }
            _ => {}
        }
        return;
    }

    // Multi-output chip/call: the rhs_port points to the first output's
    // MicrochipOutput node. If the type is a Record, look up the chip node
    // that owns these outputs and build field→port bindings.
    if let Type::Record(ref fields) = rhs_type {
        if let Expr::Call { .. } = &d.value {
            // Find the chip node whose outputs include rhs_port.node_id
            let chip_entry = ctx
                .builder
                .module
                .chips
                .iter()
                .find(|(_, child)| child.outputs.contains(&rhs_port.node_id));
            if let Some((_, child)) = chip_entry {
                let outputs = child.outputs.clone();
                let mut record: HashMap<crate::intern::Sym, Binding> = HashMap::new();
                for (i, (field_name, _ty)) in fields.iter().enumerate() {
                    if let Some(&out_id) = outputs.get(i) {
                        record.insert(
                            crate::intern::intern(field_name),
                            Binding::Local(LocalRecord {
                                port: out_id.port(WirePort::RerOutput),
                            }),
                        );
                    }
                }
                match &d.binding {
                    LetBinding::Ident { name, .. } => {
                        ctx.scope.insert(name.clone(), Binding::Record(record));
                    }
                    LetBinding::RecordDestruct {
                        fields: destruct_fields,
                        ..
                    } => {
                        install_record_destruct(ctx, &record, destruct_fields);
                    }
                    _ => {}
                }
                return;
            }
        }
    }

    match &d.binding {
        LetBinding::Ident { name, .. } => {
            ctx.scope
                .insert(name.clone(), Binding::Local(LocalRecord { port: rhs_port }));
        }
        LetBinding::Tuple { names, .. } | LetBinding::Record { names, .. } => {
            let source_node = ctx.builder.module.nodes.get(&rhs_port.node_id).cloned();
            if let Some(node) = source_node {
                for (i, name) in names.iter().enumerate() {
                    if let Some(port) = node.ports.outputs.get(i) {
                        ctx.scope.insert(
                            name.clone(),
                            Binding::Local(LocalRecord {
                                port: port_ref(node.id, crate::intern::resolve(port.name)),
                            }),
                        );
                    }
                }
            }
        }
        LetBinding::RecordDestruct { .. } => {
            // Record destructuring of non-record RHS — nothing to do.
            // TODO: this is an error..?
        }
    }
}

/// Lower a record literal into a `HashMap<Sym, Binding>`.
fn lower_record_lit(
    ctx: &mut LowerCtx,
    fields: &[RecordLitField],
) -> HashMap<crate::intern::Sym, Binding> {
    let mut map = HashMap::new();
    for field in fields {
        match field {
            RecordLitField::Named { name, value, .. } => {
                // Check if value is itself a record literal (nested records).
                if let Expr::RecordLit {
                    fields: inner_fields,
                    ..
                } = value
                {
                    let inner = lower_record_lit(ctx, inner_fields);
                    map.insert(crate::intern::intern(name), Binding::Record(inner));
                } else if let Some(binding) = resolve_field_chain(ctx, value).cloned() {
                    // Value references something in scope (possibly through a record chain).
                    map.insert(crate::intern::intern(name), binding);
                } else {
                    // Otherwise evaluate as expression and store as Local.
                    let port = lower_expr(ctx, value);
                    map.insert(
                        crate::intern::intern(name),
                        Binding::Local(LocalRecord { port }),
                    );
                }
            }
            RecordLitField::Shorthand { name, .. } => {
                // { foo } means { foo: foo } — look up foo in scope.
                if let Some(binding) = ctx.scope.get(name).cloned() {
                    map.insert(crate::intern::intern(name), binding);
                }
            }
            RecordLitField::Spread { value, .. } => {
                // ...expr — expr must resolve to a Binding::Record.
                if let Some(Binding::Record(src_fields)) = resolve_field_chain(ctx, value).cloned()
                {
                    for (k, v) in src_fields {
                        map.insert(k, v); // later fields override
                    }
                }
            }
        }
    }
    map
}

/// Install record destructure bindings from a source record into the scope.
pub(super) fn install_record_destruct(
    ctx: &mut LowerCtx,
    src: &HashMap<crate::intern::Sym, Binding>,
    destruct_fields: &[RecordDestructField],
) {
    let mut remaining = src.clone();
    for field in destruct_fields {
        match field {
            RecordDestructField::Named { name, alias, .. } => {
                let key = crate::intern::intern(name);
                if let Some(binding) = remaining.remove(&key) {
                    let bind_name = alias.as_deref().unwrap_or(name);
                    ctx.scope.insert(bind_name.to_string(), binding);
                }
            }
            RecordDestructField::Rest { name, .. } => {
                ctx.scope
                    .insert(name.clone(), Binding::Record(remaining.clone()));
            }
        }
    }
}
