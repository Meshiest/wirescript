use super::*;

/// Walk a chain of `Ident` / `FieldAccess` nodes, resolving through
/// `Binding::Record` maps (and one `Binding::Namespace` hop). Returns the final
/// `Binding` if every step resolved, or `None` when the chain isn't entirely
/// record-based (e.g. the root ident isn't a record, or a field is missing).
pub(super) fn resolve_field_chain<'a>(ctx: &'a LowerCtx, expr: &Expr) -> Option<&'a Binding> {
    match expr {
        Expr::Ident { name, .. } => ctx.scope.get(name),
        Expr::FieldAccess { obj, field, .. } => {
            let parent = resolve_field_chain(ctx, obj)?;
            match parent {
                Binding::Record(fields) => fields.get(&crate::intern::intern(field)),
                // `ns.member` where `ns` came from `import * as ns`. The
                // Namespace binding carries this module's own members, keyed by
                // name — resolve through it rather than the shared bare-name
                // scope, so two namespaces that export the same member name do
                // not collide (the bare scope keeps only the last import's). A
                // chained `ns.rec.field` continues through the Record arm on the
                // record binding found here. The bare-scope fallback keeps
                // anything not captured in the map (there should be none) from
                // silently dropping to an `_Unsupported` placeholder.
                Binding::Namespace(members) => members.get(field).or_else(|| ctx.scope.get(field)),
                _ => None,
            }
        }
        Expr::TuplePick { obj, index, .. } => {
            let parent = resolve_field_chain(ctx, obj)?;
            if let Binding::Record(fields) = parent {
                fields.get(&crate::intern::intern(&index.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Convert a resolved `Binding` into a `PortRef` for use in expressions.
/// For `Var` bindings in exec context, emits a `Var_Get` node.
pub(super) fn binding_to_port(
    ctx: &mut LowerCtx,
    binding: &Binding,
    range: &SourceRange,
) -> Option<PortRef> {
    match binding {
        Binding::Local(local) => Some(local.port),
        Binding::EventParam(p) => Some(*p),
        Binding::Buffer(buf) => Some(buf.node_id.port(WirePort::Output)),
        Binding::Input(inp) => Some(inp.node_id.port(WirePort::RerOutput)),
        Binding::Var(var_rec) => {
            if var_rec.storage == VarStorage::Buffer {
                return Some(var_rec.node_id.port(WirePort::Output));
            }
            if var_rec.storage == VarStorage::Array {
                return Some(var_rec.node_id.port(WirePort::ArrayVarRef));
            }
            if var_rec.storage == VarStorage::Map {
                return Some(var_rec.node_id.port(WirePort::MapVarRef));
            }
            if let Some(exec) = ctx.current_exec {
                if let Some(cached) = var_rec.get_node_for_handler {
                    return Some(cached.port(WirePort::Value));
                }
                let inner = var_rec.inner_type.clone();
                let mut get_props = HashMap::default();
                if let Some(lit) = default_literal_for_var_type(&inner) {
                    get_props.insert(*sym::VALUE, lit);
                }
                let get_id = ctx.add_gate(AddNodeOpts {
                    gate_class: gc::VAR_GET,
                    source_range: range.clone(),
                    properties: get_props,
                    ports: GateIO {
                        inputs: vec![
                            PortSpec {
                                name: *sym::EXEC,
                                ty: Type::Exec,
                            },
                            PortSpec {
                                name: *sym::VAR_REF,
                                ty: Type::Ref(Box::new(inner.clone())),
                            },
                        ],
                        outputs: vec![
                            PortSpec {
                                name: *sym::VALUE,
                                ty: inner.clone(),
                            },
                            PortSpec {
                                name: *sym::EXEC_OUT,
                                ty: Type::Exec,
                            },
                        ],
                    },
                    note: Some("get rec_field".into()),
                    ..Default::default()
                });
                ctx.connect(exec, get_id.port(WirePort::Exec));
                ctx.connect(
                    var_rec.node_id.port(WirePort::VarRef),
                    get_id.port(WirePort::VarRef),
                );
                ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
                return Some(get_id.port(WirePort::Value));
            }
            Some(var_rec.node_id.port(WirePort::Value))
        }
        // Reading an output's VALUE (a namespaced `L.count`): source its
        // rerouter, which mirrors whatever drives the port from inside its
        // module. Writing an output goes through `lookup_output`/`emit`, a
        // separate path, so this only ever fires on a genuine read.
        Binding::Output(out) => Some(out.node_id.port(WirePort::RerOutput)),
        Binding::Record(_) | Binding::Chip(_) | Binding::Namespace(_) => None,
    }
}

/// Map a short field name (`.Forward`, `.Jump`) to the full gate port name it
/// stands for. InputSplitter exposes a few arbitrarily-named ports whose
/// surface field names differ from the underlying port.
pub(super) fn alias_output_field(field: &str) -> &str {
    match field {
        "Forward" => "InputForward",
        "Right" => "InputRight",
        "Up" => "InputUp",
        "Pitch" => "InputPitch",
        "Yaw" => "InputYaw",
        "Roll" => "InputRoll",
        "MouseWheel" => "InputMouseWheel",
        other => other,
    }
}

/// Resolve a field name to a real output port on `node_id`: an exact/aliased
/// match, or the port whose cleaned name matches (e.g. `bFound` for `.Found`).
/// Returns `None` when no output port corresponds to the field.
pub(super) fn resolve_output_field_port(
    ctx: &LowerCtx,
    node_id: crate::ir::NodeId,
    field: &str,
) -> Option<PortRef> {
    let aliased = alias_output_field(field);
    let node = ctx.builder.module.nodes.get(&node_id)?;
    let pname = node.ports.outputs.iter().find_map(|p| {
        let pname = crate::intern::resolve(p.name);
        (pname == aliased || crate::catalog::arrays::field_name_ref(pname) == field)
            .then_some(pname)
    })?;
    Some(port_ref(node_id, pname))
}

/// A vector (`x`/`y`/`z`) or color (`r`/`g`/`b`/`a`) component name, in either
/// case. These don't name gate ports — they desugar to a SplitVector /
/// SplitColor gate, so access through a local must fall through to that logic.
fn is_swizzle_field(field: &str) -> bool {
    matches!(
        field,
        "x" | "X" | "y" | "Y" | "z" | "Z" | "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A"
    )
}

/// A scalar can never be swizzled. A chip whose output is named `x`/`y`/`z`/
/// `r`/`g`/`b`/`a` binds its (auto-unwrapped) result to a local, and splitting
/// that `int` as if it were a vector silently reads a garbage component instead
/// of the chip's output. Only KNOWN scalars short-circuit the split — vector,
/// color and unknown/`any` types keep the existing Split* behaviour.
fn is_known_scalar(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Exec
    )
}

pub(super) fn lower_field_access(
    ctx: &mut LowerCtx,
    obj: &Expr,
    field: &str,
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // A field access on an if-expression distributes over the branches:
    // `(if c then a else b).f` becomes `if c then a.f else b.f`. An aggregate
    // if-expr's branches carry no single wire, so without this a scalar field
    // (`.u`) fell to an `_Unsupported` placeholder and a swizzle (`.x`) to a
    // Split* over two placeholders. The rewritten branches are scalar, so the
    // Select the if-expr lowers to picks between the two field values.
    if let Expr::IfExpr {
        cond,
        then_branch,
        else_branch,
        range: if_range,
    } = obj
    {
        let branch_field = |br: &Expr| Expr::FieldAccess {
            obj: Box::new(br.clone()),
            field: field.to_string(),
            range: range.clone(),
        };
        let distributed = Expr::IfExpr {
            cond: cond.clone(),
            then_branch: Box::new(branch_field(then_branch)),
            else_branch: Box::new(branch_field(else_branch)),
            range: if_range.clone(),
        };
        return lower_expr(ctx, &distributed);
    }
    // `<value>.Discriminant` (an enum value, or a bare variant path like
    // `Shape.Circle`) always projects to its integer discriminant - see the
    // "Enum value layout" doc on `LowerCtx::enum_defs`. Checked directly on
    // `field` (ahead of the general record-field resolution below, which only
    // knows the LITERAL `__disc` sub-binding name, not the surface
    // `Discriminant` spelling) so both forms resolve: a variant PATH bakes the
    // registry discriminant as a literal (no gate for `obj` itself - a bare
    // `Enum.Variant` has no storage of its own); a stored/merged enum VALUE
    // reads through its `Binding::Record`'s `__disc` sub-binding (a `Var_Get`
    // for a stored var, matching any other field read).
    if field == "Discriminant"
        && let Some(port) = lower_discriminant(ctx, obj, range)
    {
        return port;
    }
    // Try resolving through record bindings first.
    // The full expression `e` is `obj.field`, so resolve_field_chain on `e`
    // walks the entire chain (potentially nested: `a.b.c`).
    if let Some(binding) = resolve_field_chain(ctx, e).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }
    // If it's a nested record, we can't return a single port — fall through.

    // A field read on a record LITERAL such as `({...}).field`. Because
    // `resolve_field_chain` walks only names and chains, read the field from the
    // literal's lowered per-field bindings before the swizzle / SplitVector
    // fallback claims `.x`/`.y`.
    if matches!(obj, Expr::RecordLit { .. })
        && let Some(fields) = crate::lower::stmt::value_record_fields(ctx, obj)
        && let Some(binding) = fields.get(&crate::intern::intern(field)).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }

    // `pts[i].f1.f2…` — a field (possibly nested) of a record array's element.
    // The chain resolve above can't see through the index subscript, so read
    // the field's parallel array here before the swizzle / SplitVector fallback
    // claims `.x`/`.y` (and before it claims a genuine `.a`/`.b`/`.r`/`.g` record
    // field as a colour component). Uses the whole expr `e`, so a nested path
    // like `pts[i].inner.a` resolves rather than degrading to a placeholder.
    if let Some(port) = lower_record_array_field_path(ctx, e, range) {
        return port;
    }
    // `m[k].f1.f2…` / `m.get(k).f1.f2…` — a field (possibly nested) of a record
    // map's value. Uses the whole expr `e`, so a nested path resolves rather than
    // degrading to a placeholder (and before the swizzle fallback claims a
    // genuine `.a`/`.b`/`.x`/`.y` record field as a colour/vector component).
    if let Some(port) = lower_record_map_field_path(ctx, e, range) {
        return port;
    }

    if let Expr::Ident { name, .. } = obj {
        if (field == "Value" || field == "prev")
            && let Some(var_rec) = ctx.lookup_var(name).cloned()
        {
            return var_rec.node_id.port(WirePort::Value);
        }
        // Gate output port access: `input.Forward` resolves to the
        // named port on the gate node referenced by the local.
        // Short field names map to full port names for known components.
        if let Some(local) = ctx.lookup_local(name).cloned() {
            // InputReader exposes a few arbitrarily-named ports.
            let aliased = alias_output_field(field);
            // Resolve the field to a real output port on the node: an exact /
            // aliased match, or the port whose cleaned name matches (e.g. the
            // `bFound` port for `.Found`, derived via the same rule the return
            // type uses). Falls back to the port directly for a single-output
            // auto-unwrapped result.
            if let Some(node) = ctx.builder.module.nodes.get(&local.port.node_id) {
                let resolved = node.ports.outputs.iter().find_map(|p| {
                    let pname = crate::intern::resolve(p.name);
                    // Swizzle fields match a sibling port case-insensitively, so
                    // `.x`/`.y`/`.z` (and `.r`/`.g`/`.b`/`.a`) on a multi-output
                    // result like `v.SplitVec()` / `c.SplitColor()` read its
                    // existing `X`/`Y`/`Z` / `R`/`G`/`B`/`A` port instead of
                    // re-splitting the first field.
                    (pname == aliased
                        || crate::catalog::arrays::field_name_ref(pname) == field
                        || (is_swizzle_field(field) && pname.eq_ignore_ascii_case(field)))
                    .then_some(pname)
                });
                if let Some(pname) = resolved {
                    return port_ref(local.port.node_id, pname);
                }
            }
            // A vector/color component (`v.x`, `c.r`) on a local doesn't name a
            // gate output port — fall through to the SplitVector / SplitColor
            // logic below, which feeds this local's value in as the split input.
            // Only when the local really holds a vector/color, though: a chip
            // output named `y` binds an `int` here, and splitting it would read
            // a garbage component instead of the chip's output.
            if !is_swizzle_field(field) || is_known_scalar(&ctx.type_of(obj)) {
                return local.port;
            }
        }
    }
    // Note: can't rely on ctx.type_of(obj) because nested exprs sharing
    // the same start offset overwrite each other in the type_of_expr map.
    // Instead, match on field name directly — these names are unambiguous.
    match field {
        "x" | "X" | "y" | "Y" | "z" | "Z" => {
            let obj_port = lower_expr(ctx, obj);
            let out_name = field[..1].to_uppercase();
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::SPLIT_VECTOR,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::INPUT,
                            ty: Type::Vector,
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: intern_static("X"),
                            ty: Type::Float,
                        },
                        PortSpec {
                            name: intern_static("Y"),
                            ty: Type::Float,
                        },
                        PortSpec {
                            name: intern_static("Z"),
                            ty: Type::Float,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(obj_port, node_id.port(WirePort::Input));
            port_ref(node_id, &out_name)
        }
        "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A" => {
            let obj_port = lower_expr(ctx, obj);
            let out_name = field[..1].to_uppercase();
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::SPLIT_COLOR,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::INPUT,
                            ty: Type::Color,
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: intern_static("R"),
                            ty: Type::Float,
                        },
                        PortSpec {
                            name: intern_static("G"),
                            ty: Type::Float,
                        },
                        PortSpec {
                            name: intern_static("B"),
                            ty: Type::Float,
                        },
                        PortSpec {
                            name: intern_static("A"),
                            ty: Type::Float,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(obj_port, node_id.port(WirePort::Input));
            port_ref(node_id, &out_name)
        }
        // Array index result fields: arr[i].value / arr[i].bOutOfBounds
        "value" | "bOutOfBounds" | "OutOfBounds" => {
            let obj_port = lower_expr(ctx, obj);
            let port_id = if field == "value" {
                WirePort::Value
            } else {
                WirePort::BOutOfBounds
            };
            obj_port.node_id.port(port_id)
        }
        _ => {
            // `.exec` names the exec output of an exec-producing expression: a
            // bare exec value (identity), or an event/call that carries data
            // alongside its exec (`GlobalCustomEvent(c) -> (n)`, typed as a
            // record with the exec output FIRST). A data-carrying event lowers
            // with its exec port as the primary output (see the event-call
            // dispatch), so lowering the object yields that exec directly. Lets
            // a data-carrying event compose into `Union(...)` explicitly.
            if field == "exec" {
                let ot = ctx.type_of(obj);
                if matches!(ot, Type::Exec)
                    || matches!(&ot, Type::Record(fs) if matches!(fs.first(), Some((_, Type::Exec))))
                {
                    return lower_expr(ctx, obj);
                }
            }
            // `field` may name an output on the gate an inline call lowers to.
            // An inline mod stashes its multi-output record (field -> source
            // binding) instead of exposing named ports, so prefer that stash and
            // project the field out of it — exactly as `lower_let_decl` binds
            // `let r = f()` then reads `r.field`. A chip / builtin / event call
            // instead lowers to a real gate with sibling output ports (`.Found`
            // / `.Index` on `arr.find(x)`); resolve those by name. Without
            // either, `obj` is never lowered and the field access degrades to an
            // `_Unsupported` placeholder — silently dropping the call.
            if let Expr::Call { .. } = obj {
                let obj_port = lower_expr(ctx, obj);
                if let Some(record) = ctx.pending_inline_record.take()
                    && let Some(binding) = record.get(&crate::intern::intern(field))
                    && let Some(port) = binding_to_port(ctx, binding, range)
                {
                    return port;
                }
                if let Some(port) = resolve_output_field_port(ctx, obj_port.node_id, field) {
                    return port;
                }
            }
            synthesise_unsupported(ctx, e)
        }
    }
}

/// `.Discriminant`'s two forms - see the call site in `lower_field_access`.
/// Returns `None` for anything that is neither a known variant path nor a
/// value resolving to an enum's `Binding::Record` (the caller falls through
/// to the ordinary field-access handling, e.g. for a typecheck-error program).
pub(super) fn lower_discriminant(
    ctx: &mut LowerCtx,
    obj: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    // A variant PATH (`Shape.Circle`) is statically known: bake the registry
    // discriminant directly as a literal, no gate for `obj` itself. Guarded
    // (mirrors `resolve_variant_for_construction`'s shadow guard in
    // typecheck) so a value symbol shadowing the enum's name falls through to
    // the value branch below instead of misreading it as a type name.
    if let Expr::FieldAccess {
        obj: enum_obj,
        field: variant,
        ..
    } = obj
        && let Expr::Ident { name: enum_name, .. } = enum_obj.as_ref()
        && ctx.scope.get(enum_name).is_none()
        && let Some(def) = ctx.enum_defs.get(enum_name)
        && let Some(vdef) = def.variants.iter().find(|v| &v.name == variant)
    {
        return Some(literal_node_range(ctx, range, Type::Int, Literal::Int(vdef.discriminant)));
    }
    // A stored/merged enum VALUE: resolve to its `Binding::Record`, index
    // `__disc`, and read it like any other field.
    if let Some(Binding::Record(fields)) = resolve_field_chain(ctx, obj).cloned()
        && let Some(disc_binding) = fields.get(&crate::intern::intern("__disc")).cloned()
    {
        return binding_to_port(ctx, &disc_binding, range);
    }
    None
}

/// `t[i]` / `m[k]` whose RUNTIME lowering is unavailable but whose value is a
/// compile-time constant: materialize it as a literal source gate, exactly as
/// `lower_ident`'s constant fallback does for a bare constant name (see its
/// comment — this is the indexed analogue, and shares its `wire_type_of_literal`
/// + `literal_node_range` pair so both agree on which literals have a wire form).
///
/// Called ONLY after [`lower_index_access_runtime`] has declined, so this is
/// strictly a NARROWING of the `_Unsupported` case: an index that resolves to a
/// real array/map var never reaches here, and no program that already lowered
/// correctly can change. That ordering is load-bearing rather than stylistic —
/// a `const t = [...]` shadowed by a same-named runtime `var t: int[]` must read
/// the VAR, so the fold can never run before the ordinary resolution.
///
/// This closes a real silent miscompile: typecheck accepts `const z = t[1]`
/// (the read emits no gate, so its exec-context rule does not apply), but the
/// ordinary lowering had no form for it and synthesised an `_Unsupported`
/// placeholder that emit never writes a component for — so `z`'s wire died and
/// every runtime consumer read the type default (0) instead of the real value.
fn const_fold_index_access(ctx: &mut LowerCtx, e: &Expr, range: &SourceRange) -> Option<PortRef> {
    let lit = {
        let lookup = |n: &str| ctx.resolve_mod(n);
        let mut budget = crate::const_eval::Budget::default();
        crate::const_eval::eval_expr(e, &ctx.const_ctx(Some(&lookup)), &mut budget).ok()?
    };
    let ty = wire_type_of_literal(&lit)?;
    Some(literal_node_range(ctx, range, ty, lit))
}

pub(super) fn lower_index_access(
    ctx: &mut LowerCtx,
    obj: &Expr,
    index: &Expr,
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // An index on an if-expression distributes over the branches:
    // `(if c then a else b)[i]` becomes `if c then a[i] else b[i]`, so each
    // branch indexes its own container instead of the if-expr failing to
    // resolve as a container (scalar-element arrays; a record-element array
    // still hits the record-value limit inside a branch).
    if let Expr::IfExpr {
        cond,
        then_branch,
        else_branch,
        range: if_range,
    } = obj
    {
        let branch_index = |br: &Expr| Expr::IndexAccess {
            obj: Box::new(br.clone()),
            index: Box::new(index.clone()),
            range: range.clone(),
        };
        let distributed = Expr::IfExpr {
            cond: cond.clone(),
            then_branch: Box::new(branch_index(then_branch)),
            else_branch: Box::new(branch_index(else_branch)),
            range: if_range.clone(),
        };
        return lower_expr(ctx, &distributed);
    }
    if let Some(port) = lower_index_access_runtime(ctx, obj, index, range) {
        return port;
    }
    if let Some(port) = const_fold_index_access(ctx, e, range) {
        return port;
    }
    // Neither a real container nor a compile-time-constant read: the last case
    // is a `const` container read at a RUNTIME index (`xs[i]`), which would
    // otherwise land on the placeholder below and silently read 0. Give the
    // container its runtime form and retry the ordinary lowering.
    //
    // Placed THIRD, after the fold, and that order is load-bearing: it keeps
    // `const z = t[1]` folding to a literal with no gate at all. Like the fold
    // above it, this is strictly a NARROWING of the `_Unsupported` case —
    // `lower_index_access_runtime` reaches every `None` exit before lowering
    // the index expression (see its doc comment), so the retry cannot
    // duplicate gates.
    if materialize_const_container(ctx, obj).is_some()
        && let Some(port) = lower_index_access_runtime(ctx, obj, index, range)
    {
        return port;
    }
    synthesise_unsupported(ctx, e)
}

/// The ordinary (gate-emitting) lowering of `obj[index]`. `None` means "no
/// runtime form here", letting [`lower_index_access`] try a compile-time fold
/// before falling back to the `_Unsupported` placeholder.
///
/// Every `None` exit is reached BEFORE the index expression is lowered (the
/// only container resolution helpers used above that point, `resolve_map_target`
/// and `resolve_field_chain`, both take `&LowerCtx` and emit nothing), so
/// declining here leaves no half-built gates behind for the fold to trip over.
fn lower_index_access_runtime(
    ctx: &mut LowerCtx,
    obj: &Expr,
    index: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let current_exec = ctx.current_exec?;
    // Map subscript `m[k]` desugars to a MapVar_Get (the same gate `m.get(k)`
    // lowers to), auto-unwrapping to the Value port. The Key/Value ports
    // carry the map's CONCRETE k/v types (not `any`) — see the comment at
    // `lower_map_method` on why a generic `any` Key would bake a bad default.
    if let Some((map_ref, Type::Map(k, v))) = resolve_map_target(ctx, obj) {
        let key = lower_expr(ctx, index);
        return Some(map_exec_op(
            ctx,
            range,
            map_ref,
            gc::MAP_GET,
            vec![(WirePort::Key, k.as_ref().clone(), key)],
            vec![
                (WirePort::Value, v.as_ref().clone()),
                (WirePort::BFound, Type::Bool),
            ],
            WirePort::Value,
        ));
    }
    let array_ref = if let Expr::Ident { name, .. } = obj {
        if let Some(var_rec) = ctx.lookup_var(name).cloned() {
            if var_rec.storage == VarStorage::Array {
                var_rec.node_id.port(WirePort::ArrayVarRef)
            } else {
                return None;
            }
        } else if let Some(inp) = ctx.lookup_input(name).cloned() {
            inp.node_id.port(WirePort::RerOutput)
        } else {
            return None;
        }
    } else if let Some(binding) = resolve_field_chain(ctx, obj).cloned() {
        // obj is a record field chain that resolves to an array var
        if let Binding::Var(var_rec) = &binding {
            if var_rec.storage == VarStorage::Array {
                var_rec.node_id.port(WirePort::ArrayVarRef)
            } else {
                return None;
            }
        } else {
            return None;
        }
    } else {
        return None;
    };
    let index_port = lower_expr(ctx, index);
    // lower_expr for the index may have advanced the exec chain via
    // Var_Get etc.; use the updated head, not the entry-time capture.
    let current_exec = ctx.current_exec.unwrap_or(current_exec);
    let elem_ty = match &ctx.type_of(obj) {
        Type::Array(inner) => inner.as_ref().clone(),
        Type::Ref(inner) => match inner.as_ref() {
            Type::Array(inner) => inner.as_ref().clone(),
            _ => Type::Any,
        },
        _ => Type::Any,
    };
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::ARRAY_GET,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::ARRAY_VAR_REF,
                    ty: Type::Ref(Box::new(elem_ty.clone())),
                },
                PortSpec {
                    name: *sym::INDEX,
                    ty: Type::Int,
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::VALUE,
                    ty: elem_ty,
                },
                PortSpec {
                    name: *sym::B_OUT_OF_BOUNDS,
                    ty: Type::Bool,
                },
                PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                },
            ],
        },
        note: Some("array get".into()),
        ..Default::default()
    });
    ctx.connect(current_exec, node_id.port(WirePort::Exec));
    ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
    ctx.connect(index_port, node_id.port(WirePort::Index));
    ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
    Some(node_id.port(WirePort::Value))
}

/// Build an ArrayVar exec gate with the standard `Exec` + `ArrayVarRef` inputs
/// and `ExecOut` output, plus the supplied extra (already-lowered) inputs and
/// extra outputs. Advances the exec chain and returns the `ret` port.
fn array_exec_op(
    ctx: &mut LowerCtx,
    range: &SourceRange,
    array_ref: PortRef,
    gate_class: &'static str,
    extra_in: Vec<(WirePort, Type, PortRef)>,
    extra_out: Vec<(WirePort, Type)>,
    ret: WirePort,
) -> PortRef {
    let exec_in = match ctx.current_exec {
        Some(e) => e,
        None => return array_ref,
    };
    let mut inputs = vec![
        PortSpec {
            name: *sym::EXEC,
            ty: Type::Exec,
        },
        PortSpec {
            name: *sym::ARRAY_VAR_REF,
            ty: Type::Array(Box::new(Type::Any)),
        },
    ];
    for (port, ty, _) in &extra_in {
        inputs.push(PortSpec {
            name: intern(port.as_str()),
            ty: ty.clone(),
        });
    }
    let mut outputs = vec![PortSpec {
        name: *sym::EXEC_OUT,
        ty: Type::Exec,
    }];
    for (port, ty) in &extra_out {
        outputs.push(PortSpec {
            name: intern(port.as_str()),
            ty: ty.clone(),
        });
    }
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class,
        source_range: range.clone(),
        ports: GateIO { inputs, outputs },
        ..Default::default()
    });
    ctx.connect(exec_in, node_id.port(WirePort::Exec));
    ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
    for (port, _, src) in extra_in {
        ctx.connect(src, node_id.port(port));
    }
    ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
    node_id.port(ret)
}

/// Resolve a positional argument that names another array — a `var` array or an
/// `in X: T[]` input — to its ref port (for the dual-array ops
/// append/copyFrom/slice). A var array exposes `ArrayVarRef`; an input array's
/// ref rides its `RER_Output`.
fn resolve_array_ref_arg(ctx: &LowerCtx, arg: Option<&CallArg>) -> Option<PortRef> {
    if let Some(CallArg::Positional(Expr::Ident { name, .. })) = arg {
        if let Some(vr) = ctx.lookup_var(name)
            && vr.storage == VarStorage::Array
        {
            return Some(vr.node_id.port(WirePort::ArrayVarRef));
        }
        if let Some(Binding::Input(inp)) = ctx.scope.get(name)
            && matches!(inp.ty, Type::Array(_))
        {
            return Some(inp.node_id.port(WirePort::RerOutput));
        }
    }
    None
}

/// Resolve a positional argument that names another map — a `var` map or an
/// `in X: Map<K,V>` input — to its ref port (for `copyFrom`). A var map exposes
/// `MapVarRef`; an input map's ref rides its `RER_Output` (mirrors the array
/// case).
fn resolve_map_ref_arg(ctx: &LowerCtx, arg: Option<&CallArg>) -> Option<PortRef> {
    if let Some(CallArg::Positional(Expr::Ident { name, .. })) = arg {
        if let Some(vr) = ctx.lookup_var(name)
            && vr.storage == VarStorage::Map
        {
            return Some(vr.node_id.port(WirePort::MapVarRef));
        }
        if let Some(Binding::Input(inp)) = ctx.scope.get(name)
            && matches!(inp.ty, Type::Map(_, _))
        {
            return Some(inp.node_id.port(WirePort::RerOutput));
        }
    }
    None
}

/// Resolve `obj` to a map's ref port AND its whole `Type::Map(K, V)` — the
/// shared dispatch used by both the `m[k]` read (`lower_index_access`) and
/// `m[k] = v` write (`lower_assign` in `stmt.rs`) subscript desugars.
/// Dispatches on the var/input's OWN storage — mirroring the array-ref
/// resolution in `lower_index_access`'s array path / `lower_array_set` —
/// rather than on typecheck's `type_of_expr`: unlike a read position,
/// `infer_assign_target` (the assignment-target typer in `typecheck.rs`)
/// never records a `type_of_expr` entry for an assignment target's object,
/// so `ctx.type_of(obj)` would always come back `Any` on the write side.
/// A `var` map exposes `MapVarRef` and carries the whole map type as
/// `inner_type`; an `in m: Map<K,V>` input rides its `RER_Output` and carries
/// the map type directly in `.ty`; a record field chain resolving to a map
/// var also exposes `MapVarRef`. `None` when `obj` doesn't resolve to a map
/// var/input.
pub(super) fn resolve_map_target(ctx: &LowerCtx, obj: &Expr) -> Option<(PortRef, Type)> {
    if let Expr::Ident { name, .. } = obj {
        if let Some(var_rec) = ctx.lookup_var(name) {
            if var_rec.storage == VarStorage::Map {
                return Some((
                    var_rec.node_id.port(WirePort::MapVarRef),
                    var_rec.inner_type.clone(),
                ));
            }
            return None;
        }
        if let Some(inp) = ctx.lookup_input(name) {
            if matches!(inp.ty, Type::Map(_, _)) {
                return Some((inp.node_id.port(WirePort::RerOutput), inp.ty.clone()));
            }
            return None;
        }
        return None;
    }
    if let Some(Binding::Var(var_rec)) = resolve_field_chain(ctx, obj)
        && var_rec.storage == VarStorage::Map
    {
        return Some((
            var_rec.node_id.port(WirePort::MapVarRef),
            var_rec.inner_type.clone(),
        ));
    }
    None
}

/// A `MapVar_*` exec op: wires the current exec + the map's `MapVarRef` plus
/// `extra_in`, exposes `extra_out`, advances exec, and returns the `ret` port.
/// The map analogue of [`array_exec_op`]. `pub(super)` so the `m[k] = v`
/// write desugar in `stmt.rs::lower_assign` can call it directly, mirroring
/// the read desugar in `lower_index_access` below.
pub(super) fn map_exec_op(
    ctx: &mut LowerCtx,
    range: &SourceRange,
    map_ref: PortRef,
    gate_class: &'static str,
    extra_in: Vec<(WirePort, Type, PortRef)>,
    extra_out: Vec<(WirePort, Type)>,
    ret: WirePort,
) -> PortRef {
    let exec_in = match ctx.current_exec {
        Some(e) => e,
        None => return map_ref,
    };
    let mut inputs = vec![
        PortSpec { name: *sym::EXEC, ty: Type::Exec },
        PortSpec { name: *sym::MAP_VAR_REF, ty: Type::Ref(Box::new(Type::Any)) },
    ];
    for (port, ty, _) in &extra_in {
        inputs.push(PortSpec { name: intern(port.as_str()), ty: ty.clone() });
    }
    let mut outputs = vec![PortSpec { name: *sym::EXEC_OUT, ty: Type::Exec }];
    for (port, ty) in &extra_out {
        outputs.push(PortSpec { name: intern(port.as_str()), ty: ty.clone() });
    }
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class,
        source_range: range.clone(),
        ports: GateIO { inputs, outputs },
        ..Default::default()
    });
    ctx.connect(exec_in, node_id.port(WirePort::Exec));
    ctx.connect(map_ref, node_id.port(WirePort::MapVarRef));
    for (port, _, src) in extra_in {
        ctx.connect(src, node_id.port(port));
    }
    ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
    node_id.port(ret)
}

/// Assign a whole map literal to a map var at runtime: clear it, then `set`
/// each entry in order. Mirrors `lower_array_literal_assign` — there's no
/// single "set map" gate, so the contents are rebuilt via Clear + one Set per
/// entry on the current exec chain. No-op outside exec context (a constant
/// initializer bakes instead; see `bake_map_init`).
pub(super) fn lower_map_literal_assign(
    ctx: &mut LowerCtx,
    map_ref: PortRef,
    entries: &[crate::ast::MapLitEntry],
    range: &SourceRange,
) {
    if ctx.current_exec.is_none() {
        return;
    }
    map_exec_op(ctx, range, map_ref, gc::MAP_CLEAR, vec![], vec![], WirePort::ExecOut);
    for e in entries {
        let key = lower_expr(ctx, &e.key);
        let val = lower_expr(ctx, &e.value);
        map_exec_op(
            ctx,
            range,
            map_ref,
            gc::MAP_SET,
            vec![
                (WirePort::Key, Type::Any, key),
                (WirePort::Value, Type::Any, val),
            ],
            vec![],
            WirePort::ExecOut,
        );
    }
}

/// Reject a method that would CHANGE a `const` container's contents.
///
/// A `const` array/map is two things at once — a compile-time value the const
/// environment answers questions about, and (once something reads it at
/// runtime) a real container gate. Mutation is what makes those two disagree:
/// after `xs.push(40)`, `const n = xs.length()` and the gate would both be
/// "right" with different answers, and the const environment would stop being a
/// single source of truth. So it stays an error.
///
/// Reuses WS044 (the container-method diagnostic) with a message naming the
/// real cause: the backstop's generic "did not resolve to an array or map"
/// text would be actively wrong here, since the receiver resolved fine.
///
/// Keyed on the gate NODE, so it holds through aliasing — a `const` table
/// passed to a `ys: T[]` parameter binds `ys` to this same node, and
/// `ys.push(…)` inside the callee is rejected exactly like `xs.push(…)` outside
/// it. Returns `true` when it reported (the caller must not emit the op).
pub(super) fn reject_const_container_mutation(
    ctx: &mut LowerCtx,
    target: NodeId,
    mutates: bool,
    op: &str,
    range: &SourceRange,
) -> bool {
    if !mutates || !ctx.immutable_containers.contains(&target) {
        return false;
    }
    ctx.error(
        "WS044",
        format!(
            "{op} would change a `const` array/map — a `const` container is \
             immutable, so its compile-time value and its runtime contents can \
             never disagree. Declare it `var` to make it mutable"
        ),
        range,
    );
    true
}

/// The leaf map-backed vars of a record MAP's field map, name-sorted, recursing
/// through nested record fields. Each leaf is one `VarStorage::Map` parallel map
/// (`Map<K, fieldType>`) — a record map is stored as one map per record field.
fn leaf_maps(fields: &HashMap<crate::intern::Sym, Binding>) -> Vec<VarRecord> {
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    let mut out = Vec::new();
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => out.extend(leaf_maps(sub)),
            Some(Binding::Var(v)) if v.storage == VarStorage::Map => out.push(v.clone()),
            _ => {}
        }
    }
    out
}

fn map_kv(v: &VarRecord) -> (Type, Type) {
    match &v.inner_type {
        Type::Map(k, val) => (k.as_ref().clone(), val.as_ref().clone()),
        _ => (Type::Any, Type::Any),
    }
}

/// Resolve `base` to a record MAP's field map (a `Binding::Record` with at least
/// one parallel map leaf). Mirrors [`resolve_record_array`].
pub(super) fn resolve_record_map(
    ctx: &LowerCtx,
    base: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let binding = match base {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        _ => resolve_field_chain(ctx, base).cloned(),
    }?;
    match binding {
        Binding::Record(f) if !leaf_maps(&f).is_empty() => Some(f),
        _ => None,
    }
}

/// `m[key]` / `m.get(key)` — recover the map base expr and the key expr from
/// either spelling, so both route through the same record-map read/field paths.
fn record_map_key_source(expr: &Expr) -> Option<(&Expr, &Expr)> {
    match expr {
        Expr::IndexAccess { obj, index, .. } => Some((obj, index)),
        Expr::Call { callee, args, .. } => {
            if let Expr::FieldAccess { obj, field, .. } = callee.as_ref()
                && field == "get"
                && let Some(CallArg::Positional(k)) = args.first()
            {
                Some((obj, k))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `m.get(k)` / `m[k]` as a record VALUE — read every parallel field map at the
/// shared key into a `Binding::Record` of `Local` ports (recursing nested
/// records). Mirrors [`lower_record_array_index_value`].
pub(super) fn lower_record_map_key_value(
    ctx: &mut LowerCtx,
    expr: &Expr,
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let (base, key) = record_map_key_source(expr)?;
    let fields = resolve_record_map(ctx, base)?;
    ctx.current_exec?;
    let key_port = lower_expr(ctx, key);
    Some(record_map_key_read(ctx, &fields, key_port, range))
}

fn record_map_key_read(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    key_port: PortRef,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut out = HashMap::default();
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => {
                out.insert(k, Binding::Record(record_map_key_read(ctx, sub, key_port, range)));
            }
            Some(Binding::Var(v)) if v.storage == VarStorage::Map => {
                let (key_ty, val_ty) = map_kv(v);
                let mref = v.node_id.port(WirePort::MapVarRef);
                let port = map_exec_op(
                    ctx,
                    range,
                    mref,
                    gc::MAP_GET,
                    vec![(WirePort::Key, key_ty, key_port)],
                    vec![(WirePort::Value, val_ty), (WirePort::BFound, Type::Bool)],
                    WirePort::Value,
                );
                out.insert(k, Binding::Local(LocalRecord { port }));
            }
            _ => {}
        }
    }
    out
}

/// `m[k] = rec` / `m.set(k, rec)` — write a whole record value by fanning it
/// across the parallel field maps at the shared key. Returns `true` when handled.
pub(super) fn lower_record_map_key_set(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    key: &Expr,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    if ctx.current_exec.is_none() {
        return true;
    }
    let Some(src) = crate::lower::stmt::value_record_fields(ctx, value) else {
        return false;
    };
    let key_port = lower_expr(ctx, key);
    record_map_set_walk(ctx, fields, &src, key_port, range);
    true
}

fn record_map_set_walk(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    src: &HashMap<crate::intern::Sym, Binding>,
    key_port: PortRef,
    range: &SourceRange,
) {
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    for k in names {
        let Some(sbind) = src.get(&k).cloned() else {
            continue;
        };
        match fields.get(&k) {
            Some(Binding::Record(tsub)) => {
                if let Binding::Record(ssub) = &sbind {
                    record_map_set_walk(ctx, tsub, ssub, key_port, range);
                }
            }
            Some(Binding::Var(v)) if v.storage == VarStorage::Map => {
                let (key_ty, val_ty) = map_kv(v);
                let mref = v.node_id.port(WirePort::MapVarRef);
                let Some(val_port) = binding_to_port(ctx, &sbind, range) else {
                    continue;
                };
                map_exec_op(
                    ctx,
                    range,
                    mref,
                    gc::MAP_SET,
                    vec![
                        (WirePort::Key, key_ty, key_port),
                        (WirePort::Value, val_ty, val_port),
                    ],
                    vec![],
                    WirePort::ExecOut,
                );
            }
            _ => {}
        }
    }
}

/// Fan a record-map method across the parallel per-field maps. `set` decomposes
/// the record value; `has`/`length` read the first field (all fields share the
/// same keys); `clear`/`remove` fan out; `get` builds the record value and
/// stashes it in `pending_inline_record` (a let/out consumer picks it up).
pub(super) fn lower_record_map_method(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let leaves = leaf_maps(fields);
    if leaves.is_empty() {
        return synthesise_unsupported(ctx, e);
    }
    let first_ref = leaves[0].node_id.port(WirePort::MapVarRef);
    match method {
        "length" | "has" => lower_map_method(
            ctx,
            first_ref,
            leaves[0].inner_type.clone(),
            method,
            args,
            range,
            e,
        ),
        "clear" | "remove" => {
            let mut ret = ctx.current_exec;
            for leaf in &leaves {
                ret = Some(lower_map_method(
                    ctx,
                    leaf.node_id.port(WirePort::MapVarRef),
                    leaf.inner_type.clone(),
                    method,
                    args,
                    range,
                    e,
                ));
            }
            ret.unwrap_or_else(|| synthesise_unsupported(ctx, e))
        }
        "set" => {
            let (Some(k), Some(v)) = (
                match args.first() {
                    Some(CallArg::Positional(x)) => Some(x),
                    _ => None,
                },
                match args.get(1) {
                    Some(CallArg::Positional(x)) => Some(x),
                    _ => None,
                },
            ) else {
                return synthesise_unsupported(ctx, e);
            };
            match crate::lower::stmt::value_record_fields(ctx, v) {
                Some(src) => {
                    let key_port = lower_expr(ctx, k);
                    record_map_set_walk(ctx, fields, &src, key_port, range);
                    ctx.current_exec
                        .unwrap_or_else(|| synthesise_unsupported(ctx, e))
                }
                None => synthesise_unsupported(ctx, e),
            }
        }
        "get" => {
            let record = lower_record_map_key_value(ctx, e, range);
            match record {
                Some(rec) => {
                    // Primary port (a record auto-unwraps to its first field) so a
                    // scalar misuse still has something; record consumers use the
                    // pending record instead.
                    let primary = rec
                        .values()
                        .find_map(|b| match b {
                            Binding::Local(l) => Some(l.port),
                            _ => None,
                        })
                        .unwrap_or_else(|| synthesise_unsupported(ctx, e));
                    ctx.pending_inline_record = Some(rec);
                    primary
                }
                None => synthesise_unsupported(ctx, e),
            }
        }
        // Keys are identical across every parallel field map, so `keys(dest)`
        // reads them from the FIRST field into the (scalar-keyed) destination.
        "keys" => lower_map_method(
            ctx,
            first_ref,
            leaves[0].inner_type.clone(),
            "keys",
            args,
            range,
            e,
        ),
        // `values` would fill a RECORD array (per-field split needed) and
        // `copyFrom` needs a matching record map - neither is implemented yet.
        _ => {
            ctx.error(
                "WS050",
                format!(
                    "`.{method}()` is not supported on a record map - the values are \
                     records, which `{method}` has no per-field form for yet. Use \
                     `m.get(k)` / `m[k]` for value access"
                ),
                range,
            );
            synthesise_unsupported(ctx, e)
        }
    }
}

/// Lower `m.<method>(...)` for a `map` value. `map_type` is the whole
/// `Type::Map(K, V)`. Mirrors [`lower_array_method`]; every method in
/// [`crate::catalog::maps::MAP_METHODS`] must be handled here.
pub(super) fn lower_map_method(
    ctx: &mut LowerCtx,
    map_ref: PortRef,
    map_type: Type,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // The Key/Value ports MUST carry the map's concrete key/value types, not
    // `any`: a port's declared type drives the baked DATA-field default in the
    // component (the "last value"). A generic `any` Key bakes a float `0.0`,
    // which for a string-keyed map is a type the game rejects at load — the
    // whole component (and every wire into it, including Exec) fails to connect.
    // Mirror how `Value` already keys off `value_ty`.
    let (key_ty, value_ty) = match &map_type {
        Type::Map(k, v) => (k.as_ref().clone(), v.as_ref().clone()),
        _ => (Type::Any, Type::Any),
    };

    let mutates = crate::catalog::maps::map_method(method).is_some_and(|m| m.mutates);
    if reject_const_container_mutation(
        ctx,
        map_ref.node_id,
        mutates,
        &format!("`.{method}()`"),
        range,
    ) {
        return synthesise_unsupported(ctx, e);
    }

    // `exec = <trigger>` named arg: drive the op off an explicit trigger instead
    // of the surrounding exec chain, so a read like
    // `m.get(k, exec = Change(k).exec)` works in a PURE context (e.g. an output
    // binding). Mirrors the array-method path. The op is a leaf then, so restore
    // the caller's exec afterward rather than advancing it.
    let exec_arg: Option<&Expr> = args.iter().find_map(|a| match a {
        CallArg::Named { name, value, .. } if name == "exec" => Some(value),
        _ => None,
    });
    let saved_exec = ctx.current_exec;
    if let Some(exec_expr) = exec_arg {
        let src = lower_expr(ctx, exec_expr);
        ctx.current_exec = Some(src);
    }

    // Positional args only — the `exec =` named arg is handled above, so the key
    // is the first POSITIONAL arg even when an `exec =` precedes it.
    let pos_args: Vec<&CallArg> = args
        .iter()
        .filter(|a| matches!(a, CallArg::Positional(_)))
        .collect();
    let key_at = |ctx: &mut LowerCtx, i: usize| match pos_args.get(i).copied() {
        Some(CallArg::Positional(k)) => Some(lower_expr(ctx, k)),
        _ => None,
    };

    let method_result = match method {
        "get" => match key_at(ctx, 0) {
            Some(key) => map_exec_op(
                ctx,
                range,
                map_ref,
                gc::MAP_GET,
                vec![(WirePort::Key, key_ty.clone(), key)],
                vec![(WirePort::Value, value_ty), (WirePort::BFound, Type::Bool)],
                WirePort::Value,
            ),
            None => synthesise_unsupported(ctx, e),
        },
        "set" => match (key_at(ctx, 0), key_at(ctx, 1)) {
            (Some(key), Some(val)) => map_exec_op(
                ctx,
                range,
                map_ref,
                gc::MAP_SET,
                vec![(WirePort::Key, key_ty.clone(), key), (WirePort::Value, value_ty, val)],
                vec![],
                WirePort::ExecOut,
            ),
            _ => synthesise_unsupported(ctx, e),
        },
        "has" | "remove" => match key_at(ctx, 0) {
            Some(key) => {
                let gate = if method == "has" { gc::MAP_HAS } else { gc::MAP_REMOVE };
                map_exec_op(
                    ctx,
                    range,
                    map_ref,
                    gate,
                    vec![(WirePort::Key, key_ty.clone(), key)],
                    vec![(WirePort::BFound, Type::Bool)],
                    WirePort::BFound,
                )
            }
            None => synthesise_unsupported(ctx, e),
        },
        "clear" => map_exec_op(ctx, range, map_ref, gc::MAP_CLEAR, vec![], vec![], WirePort::ExecOut),
        "length" => map_exec_op(
            ctx,
            range,
            map_ref,
            gc::MAP_GET_LENGTH,
            vec![],
            vec![(WirePort::Length, Type::Int)],
            WirePort::Length,
        ),
        "copyFrom" => match resolve_map_ref_arg(ctx, pos_args.first().copied()) {
            Some(src) => map_exec_op(
                ctx,
                range,
                map_ref,
                gc::MAP_COPY_FROM,
                vec![(WirePort::SourceRef, Type::Ref(Box::new(Type::Any)), src)],
                vec![],
                WirePort::ExecOut,
            ),
            None => synthesise_unsupported(ctx, e),
        },
        "keys" | "values" => match resolve_array_ref_arg(ctx, pos_args.first().copied()) {
            Some(dest) => {
                let gate = if method == "keys" { gc::MAP_GET_KEYS } else { gc::MAP_GET_VALUES };
                map_exec_op(
                    ctx,
                    range,
                    map_ref,
                    gate,
                    vec![(WirePort::ArrayVarRef, Type::Array(Box::new(Type::Any)), dest)],
                    vec![],
                    WirePort::ExecOut,
                )
            }
            None => synthesise_unsupported(ctx, e),
        },
        _ => synthesise_unsupported(ctx, e),
    };

    // An explicit `exec =` trigger makes this op a leaf: restore the caller's exec
    // context so the surrounding (possibly pure) lowering is unaffected.
    if exec_arg.is_some() {
        ctx.current_exec = saved_exec;
    }
    method_result
}

/// The leaf array-backed vars of a record container's field map, name-sorted for
/// a stable fan-out order, recursing through nested record fields. Each leaf is
/// one `VarStorage::Array` parallel array — a record ARRAY is stored as one
/// array per record field.
fn leaf_arrays(fields: &HashMap<crate::intern::Sym, Binding>) -> Vec<VarRecord> {
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    let mut out = Vec::new();
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => out.extend(leaf_arrays(sub)),
            Some(Binding::Var(v)) if v.storage == VarStorage::Array => out.push(v.clone()),
            _ => {}
        }
    }
    out
}

fn nth_positional<'a>(args: &'a [CallArg], i: usize) -> Option<&'a Expr> {
    match args.get(i) {
        Some(CallArg::Positional(e)) => Some(e),
        _ => None,
    }
}

/// Apply a value-carrying array op (push / insert / fill) to each parallel field
/// array, reading the source record VALUE field by field (`binding_to_port`
/// yields the scalar the field's array should store). `idx` (Some for `insert`)
/// fans the same index into every field. Recurses through nested record fields.
fn record_array_value_op(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    src: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
    gate_class: &'static str,
    scalar: Option<(WirePort, PortRef)>,
) -> Option<PortRef> {
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    let mut ret = ctx.current_exec;
    for k in names {
        let Some(sbind) = src.get(&k).cloned() else {
            continue;
        };
        match fields.get(&k) {
            Some(Binding::Record(tsub)) => {
                if let Binding::Record(ssub) = &sbind {
                    ret = record_array_value_op(ctx, tsub, ssub, range, gate_class, scalar);
                }
            }
            Some(Binding::Var(v)) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let aref = v.node_id.port(WirePort::ArrayVarRef);
                let Some(val_port) = binding_to_port(ctx, &sbind, range) else {
                    continue;
                };
                let mut extra = vec![(WirePort::Value, elem, val_port)];
                if let Some((port, p)) = scalar {
                    extra.push((port, Type::Int, p));
                }
                let out_extra = if gate_class == gc::ARRAY_INSERT {
                    vec![(WirePort::BOutOfBounds, Type::Bool)]
                } else {
                    vec![]
                };
                ret = Some(array_exec_op(
                    ctx,
                    range,
                    aref,
                    gate_class,
                    extra,
                    out_extra,
                    WirePort::ExecOut,
                ));
            }
            _ => {}
        }
    }
    ret
}

/// Resolve `base` (an identifier or a record field chain) to a record array's
/// field map — a `Binding::Record` with at least one parallel array leaf. Used
/// to fan `base[i]` / `base[i].field` across the per-field arrays.
pub(super) fn resolve_record_array(
    ctx: &LowerCtx,
    base: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let binding = match base {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        _ => resolve_field_chain(ctx, base).cloned(),
    }?;
    match binding {
        Binding::Record(f) if !leaf_arrays(&f).is_empty() => Some(f),
        _ => None,
    }
}

/// Decompose `arr[i].f1.f2…` into the array base, the index expr, and the
/// trailing field path (source order). Both named fields (`.inner`) and tuple
/// picks (`.0`) contribute a path segment, so a record whose field is itself a
/// tuple decomposes the same way. `None` unless `e` is field/tuple accesses
/// wrapping an index access.
fn split_record_array_access(e: &Expr) -> Option<(&Expr, &Expr, Vec<String>)> {
    let mut path: Vec<String> = Vec::new();
    let mut cur = e;
    loop {
        match cur {
            Expr::FieldAccess { obj, field, .. } => {
                path.push(field.clone());
                cur = obj;
            }
            Expr::TuplePick { obj, index, .. } => {
                path.push(index.to_string());
                cur = obj;
            }
            Expr::IndexAccess { obj, index, .. } => {
                path.reverse();
                return Some((obj, index, path));
            }
            _ => return None,
        }
    }
}

/// Walk a record field map along `path`, returning the binding the LAST segment
/// names. Every earlier segment must name a nested record to descend through;
/// a non-record earlier segment (or any missing name) yields `None`. `path`
/// must be non-empty.
fn navigate_record_fields<'a>(
    fields: &'a HashMap<crate::intern::Sym, Binding>,
    path: &[String],
) -> Option<&'a Binding> {
    let mut cur = fields;
    for (i, seg) in path.iter().enumerate() {
        let b = cur.get(&crate::intern::intern(seg))?;
        if i + 1 == path.len() {
            return Some(b);
        }
        match b {
            Binding::Record(sub) => cur = sub,
            _ => return None,
        }
    }
    None
}

/// The first leaf `Local` port of a lowered record value, in name-sorted order,
/// descending through nested records. A record auto-unwraps to this port when
/// used where a scalar is expected. `None` for an empty record.
fn first_leaf_port(rec: &HashMap<crate::intern::Sym, Binding>) -> Option<PortRef> {
    let mut names: Vec<crate::intern::Sym> = rec.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    for k in names {
        match rec.get(&k) {
            Some(Binding::Local(l)) => return Some(l.port),
            Some(Binding::Record(sub)) => {
                if let Some(p) = first_leaf_port(sub) {
                    return Some(p);
                }
            }
            _ => {}
        }
    }
    None
}

/// The array var backing one field of a record array (`pts[i].field` /
/// `pts[i].field = v`). `None` if the field is not a scalar parallel array
/// (e.g. a nested record — a deeper case left for a follow-up).
fn record_array_field(
    fields: &HashMap<crate::intern::Sym, Binding>,
    field: &str,
) -> Option<VarRecord> {
    match fields.get(&crate::intern::intern(field)) {
        Some(Binding::Var(v)) if v.storage == VarStorage::Array => Some(v.clone()),
        _ => None,
    }
}

/// `pts[i].f1.f2…` — read a field of a record array's element by indexing the
/// parallel arrays. A path landing on a SCALAR leaf yields that element's
/// `Value` port; a path landing on a NESTED RECORD reads every leaf of that
/// sub-record at the shared index into a `Binding::Record` value (stashed in
/// `pending_inline_record` for a record consumer) and returns its first leaf
/// port so a scalar consumer still auto-unwraps. `None` unless `e` is a
/// record-array field path that resolves, with an exec context to read on.
pub(super) fn lower_record_array_field_path(
    ctx: &mut LowerCtx,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let (base, index, path) = split_record_array_access(e)?;
    if path.is_empty() {
        return None;
    }
    let fields = resolve_record_array(ctx, base)?;
    let binding = navigate_record_fields(&fields, &path)?.clone();
    ctx.current_exec?;
    let index_port = lower_expr(ctx, index);
    match binding {
        Binding::Var(v) if v.storage == VarStorage::Array => Some(array_exec_op(
            ctx,
            range,
            v.node_id.port(WirePort::ArrayVarRef),
            gc::ARRAY_GET,
            vec![(WirePort::Index, Type::Int, index_port)],
            vec![
                (WirePort::Value, v.inner_type.clone()),
                (WirePort::BOutOfBounds, Type::Bool),
            ],
            WirePort::Value,
        )),
        Binding::Record(sub) => {
            let rec = record_array_index_read(ctx, &sub, index_port, range);
            let primary = first_leaf_port(&rec)?;
            ctx.pending_inline_record = Some(rec);
            Some(primary)
        }
        _ => None,
    }
}

/// `pts[i].inner` as a record VALUE — the nested-record analogue of
/// [`lower_record_array_index_value`]. `None` unless `e` is a record-array field
/// path landing on a nested record, with an exec context to read on.
pub(super) fn lower_record_array_field_path_value(
    ctx: &mut LowerCtx,
    e: &Expr,
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let (base, index, path) = split_record_array_access(e)?;
    if path.is_empty() {
        return None;
    }
    let fields = resolve_record_array(ctx, base)?;
    let Binding::Record(sub) = navigate_record_fields(&fields, &path)?.clone() else {
        return None;
    };
    ctx.current_exec?;
    let index_port = lower_expr(ctx, index);
    Some(record_array_index_read(ctx, &sub, index_port, range))
}

/// `pts[i].f1.f2… = value` — write a field of a record array's element. A
/// SCALAR-leaf path writes that field's parallel array; a NESTED-RECORD path
/// fans the record value across the sub-record's parallel arrays at the shared
/// index. Returns `true` when it handled the assignment.
pub(super) fn lower_record_array_field_path_set(
    ctx: &mut LowerCtx,
    target: &Expr,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    let Some((base, index, path)) = split_record_array_access(target) else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    let Some(fields) = resolve_record_array(ctx, base) else {
        return false;
    };
    let Some(binding) = navigate_record_fields(&fields, &path).cloned() else {
        return false;
    };
    if ctx.current_exec.is_none() {
        return true;
    }
    match binding {
        Binding::Var(v) if v.storage == VarStorage::Array => {
            let index_port = lower_expr(ctx, index);
            let value_port = lower_expr(ctx, value);
            array_exec_op(
                ctx,
                range,
                v.node_id.port(WirePort::ArrayVarRef),
                gc::ARRAY_SET_AT_INDEX,
                vec![
                    (WirePort::Index, Type::Int, index_port),
                    (WirePort::Value, v.inner_type.clone(), value_port),
                ],
                vec![],
                WirePort::ExecOut,
            );
            true
        }
        Binding::Record(sub) => {
            let Some(src) = crate::lower::stmt::value_record_fields(ctx, value) else {
                return false;
            };
            let index_port = lower_expr(ctx, index);
            record_array_set_walk(ctx, &sub, &src, index_port, range);
            true
        }
        _ => false,
    }
}

/// Decompose `m[k].f1.f2…` / `m.get(k).f1.f2…` into the map base, the key expr,
/// and the trailing field path. The map counterpart of
/// [`split_record_array_access`]; the base recognises BOTH key-access spellings
/// via [`record_map_key_source`]. `None` unless field/tuple accesses wrap a map
/// key access.
fn split_record_map_access(e: &Expr) -> Option<(&Expr, &Expr, Vec<String>)> {
    let mut path: Vec<String> = Vec::new();
    let mut cur = e;
    loop {
        match cur {
            Expr::FieldAccess { obj, field, .. } => {
                path.push(field.clone());
                cur = obj;
            }
            Expr::TuplePick { obj, index, .. } => {
                path.push(index.to_string());
                cur = obj;
            }
            _ => {
                let (base, key) = record_map_key_source(cur)?;
                path.reverse();
                return Some((base, key, path));
            }
        }
    }
}

/// `m[k].f1.f2…` — read a field of a record map's value by keying the parallel
/// maps. The map analogue of [`lower_record_array_field_path`]: a SCALAR-leaf
/// path yields that field's `MapVar_Get` value; a NESTED-RECORD path reads every
/// leaf of the sub-record at the shared key into a `Binding::Record` value
/// (stashed in `pending_inline_record`) and returns its first leaf port.
pub(super) fn lower_record_map_field_path(
    ctx: &mut LowerCtx,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let (base, key, path) = split_record_map_access(e)?;
    if path.is_empty() {
        return None;
    }
    let fields = resolve_record_map(ctx, base)?;
    let binding = navigate_record_fields(&fields, &path)?.clone();
    ctx.current_exec?;
    let key_port = lower_expr(ctx, key);
    match binding {
        Binding::Var(v) if v.storage == VarStorage::Map => {
            let (key_ty, val_ty) = map_kv(&v);
            Some(map_exec_op(
                ctx,
                range,
                v.node_id.port(WirePort::MapVarRef),
                gc::MAP_GET,
                vec![(WirePort::Key, key_ty, key_port)],
                vec![(WirePort::Value, val_ty), (WirePort::BFound, Type::Bool)],
                WirePort::Value,
            ))
        }
        Binding::Record(sub) => {
            let rec = record_map_key_read(ctx, &sub, key_port, range);
            let primary = first_leaf_port(&rec)?;
            ctx.pending_inline_record = Some(rec);
            Some(primary)
        }
        _ => None,
    }
}

/// `m[k].inner` as a record VALUE — the nested-record analogue of
/// [`lower_record_map_key_value`]. `None` unless `e` is a record-map field path
/// landing on a nested record, with an exec context to read on.
pub(super) fn lower_record_map_field_path_value(
    ctx: &mut LowerCtx,
    e: &Expr,
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let (base, key, path) = split_record_map_access(e)?;
    if path.is_empty() {
        return None;
    }
    let fields = resolve_record_map(ctx, base)?;
    let Binding::Record(sub) = navigate_record_fields(&fields, &path)?.clone() else {
        return None;
    };
    ctx.current_exec?;
    let key_port = lower_expr(ctx, key);
    Some(record_map_key_read(ctx, &sub, key_port, range))
}

/// `m[k].f1.f2… = value` — write a field of a record map's value. A SCALAR-leaf
/// path writes that field's parallel map; a NESTED-RECORD path fans the record
/// value across the sub-record's parallel maps at the shared key. Returns `true`
/// when it handled the assignment.
pub(super) fn lower_record_map_field_path_set(
    ctx: &mut LowerCtx,
    target: &Expr,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    let Some((base, key, path)) = split_record_map_access(target) else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    let Some(fields) = resolve_record_map(ctx, base) else {
        return false;
    };
    let Some(binding) = navigate_record_fields(&fields, &path).cloned() else {
        return false;
    };
    if ctx.current_exec.is_none() {
        return true;
    }
    match binding {
        Binding::Var(v) if v.storage == VarStorage::Map => {
            let (key_ty, val_ty) = map_kv(&v);
            let key_port = lower_expr(ctx, key);
            let value_port = lower_expr(ctx, value);
            map_exec_op(
                ctx,
                range,
                v.node_id.port(WirePort::MapVarRef),
                gc::MAP_SET,
                vec![
                    (WirePort::Key, key_ty, key_port),
                    (WirePort::Value, val_ty, value_port),
                ],
                vec![],
                WirePort::ExecOut,
            );
            true
        }
        Binding::Record(sub) => {
            let Some(src) = crate::lower::stmt::value_record_fields(ctx, value) else {
                return false;
            };
            let key_port = lower_expr(ctx, key);
            record_map_set_walk(ctx, &sub, &src, key_port, range);
            true
        }
        _ => false,
    }
}

/// `pts[i]` as a record VALUE — read every parallel field array at the shared
/// index into a `Binding::Record` of `Local` ports (recursing for nested record
/// fields). Lets `p = pts[i]`, `pts.push(other[i])`, `out o = pts[i]`, etc. flow
/// through the ordinary record-value machinery. `None` if `base` isn't a record
/// array or there is no exec context to read on.
pub(super) fn lower_record_array_index_value(
    ctx: &mut LowerCtx,
    base: &Expr,
    index: &Expr,
    range: &SourceRange,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let fields = resolve_record_array(ctx, base)?;
    ctx.current_exec?;
    let index_port = lower_expr(ctx, index);
    Some(record_array_index_read(ctx, &fields, index_port, range))
}

fn record_array_index_read(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    index_port: PortRef,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut out = HashMap::default();
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => {
                let inner = record_array_index_read(ctx, sub, index_port, range);
                out.insert(k, Binding::Record(inner));
            }
            Some(Binding::Var(v)) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let aref = v.node_id.port(WirePort::ArrayVarRef);
                let port = array_exec_op(
                    ctx,
                    range,
                    aref,
                    gc::ARRAY_GET,
                    vec![(WirePort::Index, Type::Int, index_port)],
                    vec![
                        (WirePort::Value, elem),
                        (WirePort::BOutOfBounds, Type::Bool),
                    ],
                    WirePort::Value,
                );
                out.insert(k, Binding::Local(LocalRecord { port }));
            }
            _ => {}
        }
    }
    out
}

/// `pts[i] = rec` — write a whole record element by fanning the record VALUE
/// across the parallel field arrays at the shared index. Returns `true` when it
/// handled the assignment.
pub(super) fn lower_record_array_index_set(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    index: &Expr,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    if ctx.current_exec.is_none() {
        return true;
    }
    let Some(src) = crate::lower::stmt::value_record_fields(ctx, value) else {
        return false;
    };
    let index_port = lower_expr(ctx, index);
    record_array_set_walk(ctx, fields, &src, index_port, range);
    true
}

fn record_array_set_walk(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    src: &HashMap<crate::intern::Sym, Binding>,
    index_port: PortRef,
    range: &SourceRange,
) {
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    for k in names {
        let Some(sbind) = src.get(&k).cloned() else {
            continue;
        };
        match fields.get(&k) {
            Some(Binding::Record(tsub)) => {
                if let Binding::Record(ssub) = &sbind {
                    record_array_set_walk(ctx, tsub, ssub, index_port, range);
                }
            }
            Some(Binding::Var(v)) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let aref = v.node_id.port(WirePort::ArrayVarRef);
                let Some(val_port) = binding_to_port(ctx, &sbind, range) else {
                    continue;
                };
                array_exec_op(
                    ctx,
                    range,
                    aref,
                    gc::ARRAY_SET_AT_INDEX,
                    vec![
                        (WirePort::Index, Type::Int, index_port),
                        (WirePort::Value, elem, val_port),
                    ],
                    vec![],
                    WirePort::ExecOut,
                );
            }
            _ => {}
        }
    }
}

/// A fresh, unbound `Pseudo_ArrayVar` scratch gate of element type `elem` (for
/// the multi-pass record sort's key copies). Not bound in scope — only its ref
/// port is wired.
fn make_scratch_array(ctx: &mut LowerCtx, elem: &Type, range: &SourceRange) -> PortRef {
    let node = ctx.add_gate(AddNodeOpts {
        gate_class: gc::PSEUDO_ARRAY_VAR,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::ARRAY_VAR_REF,
                ty: Type::Ref(Box::new(Type::Array(Box::new(elem.clone())))),
            }],
        },
        note: Some("sort key copy".into()),
        ..Default::default()
    });
    node.port(WirePort::ArrayVarRef)
}

/// `soa.field.sort(descending?)` on a record array: sort the WHOLE record by
/// `field`, reordering every sibling field array to match, so rows stay intact.
/// The `sortMultiple` gate carries a sort key plus 7 parallel columns, so a
/// record wider than 8 fields is sorted in GROUPS of 7: each later group is
/// sorted against a COPY of the original key taken before any sort, and a
/// deterministic sort applies the identical permutation to every group. `None`
/// if `field` isn't a scalar parallel array (a nested-record field can't be a
/// sort key).
pub(super) fn lower_record_array_field_sort(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    field: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> Option<PortRef> {
    const REFS: [WirePort; 7] = [
        WirePort::ArrayVarRef2,
        WirePort::ArrayVarRef3,
        WirePort::ArrayVarRef4,
        WirePort::ArrayVarRef5,
        WirePort::ArrayVarRef6,
        WirePort::ArrayVarRef7,
        WirePort::ArrayVarRef8,
    ];
    let key = record_array_field(fields, field)?;
    let key_ref = key.node_id.port(WirePort::ArrayVarRef);
    let key_elem = key.inner_type.clone();
    let parallels: Vec<VarRecord> = leaf_arrays(fields)
        .into_iter()
        .filter(|v| v.node_id != key.node_id)
        .collect();
    let desc_port = args
        .iter()
        .find_map(|a| match a {
            CallArg::Positional(d) => Some(d),
            CallArg::Named { name, value, .. } if name == "descending" => Some(value),
            _ => None,
        })
        .map(|d| lower_expr(ctx, d));

    // A single-field record: just sort the key column.
    if parallels.is_empty() {
        let mut extra = Vec::new();
        if let Some(dp) = desc_port {
            extra.push((WirePort::BDescending, Type::Bool, dp));
        }
        return Some(array_exec_op(ctx, range, key_ref, gc::ARRAY_SORT, extra, vec![], WirePort::ExecOut));
    }

    let groups: Vec<&[VarRecord]> = parallels.chunks(REFS.len()).collect();
    // Copy the ORIGINAL key for every group past the first, BEFORE any sort runs
    // (so each copy holds the pre-sort values). One `sortMultiple` per group then
    // reorders that group's ≤7 columns by the key's (identical) permutation.
    let mut sort_keys: Vec<PortRef> = vec![key_ref];
    for _ in 1..groups.len() {
        let scratch = make_scratch_array(ctx, &key_elem, range);
        array_exec_op(
            ctx,
            range,
            scratch,
            gc::ARRAY_COPY_FROM,
            vec![(WirePort::SourceRef, Type::Array(Box::new(Type::Any)), key_ref)],
            vec![],
            WirePort::ExecOut,
        );
        sort_keys.push(scratch);
    }
    let mut ret = ctx.current_exec;
    for (gi, group) in groups.iter().enumerate() {
        let mut extra: Vec<(WirePort, Type, PortRef)> = Vec::new();
        for (i, p) in group.iter().enumerate() {
            extra.push((REFS[i], Type::Array(Box::new(Type::Any)), p.node_id.port(WirePort::ArrayVarRef)));
        }
        if let Some(dp) = desc_port {
            extra.push((WirePort::BDescending, Type::Bool, dp));
        }
        ret = Some(array_exec_op(
            ctx,
            range,
            sort_keys[gi],
            gc::ARRAY_SORT_MULTIPLE,
            extra,
            vec![(WirePort::BSuccess, Type::Bool)],
            WirePort::ExecOut,
        ));
    }
    ret.or_else(|| Some(synthesise_unsupported(ctx, e)))
}

/// Fan a record-array method across the parallel per-field arrays. `fields` is
/// the record array's `Binding::Record`. Ops that keep the parallel arrays in
/// lockstep — push/insert/remove/fill/clear/reverse — fan out; `length` reads
/// the first field (all fields share one count). Ops that would DESYNC the
/// arrays if applied per field (sort/shuffle/find and the aggregates) are left
/// unsupported here rather than silently corrupting row correspondence.
pub(super) fn lower_record_array_method(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let leaves = leaf_arrays(fields);
    if leaves.is_empty() {
        return synthesise_unsupported(ctx, e);
    }
    match method {
        "length" => lower_array_method(
            ctx,
            leaves[0].node_id.port(WirePort::ArrayVarRef),
            leaves[0].inner_type.clone(),
            "length",
            &[],
            range,
            e,
        ),
        // Deterministic value-free (or scalar-arg) mutations run identically on
        // every parallel array, keeping them in lockstep.
        "clear" | "reverse" | "remove" => {
            let mut ret = ctx.current_exec;
            for leaf in &leaves {
                ret = Some(lower_array_method(
                    ctx,
                    leaf.node_id.port(WirePort::ArrayVarRef),
                    leaf.inner_type.clone(),
                    method,
                    args,
                    range,
                    e,
                ));
            }
            ret.unwrap_or_else(|| synthesise_unsupported(ctx, e))
        }
        "push" => match nth_positional(args, 0)
            .and_then(|v| crate::lower::stmt::value_record_fields(ctx, v))
        {
            Some(src) => record_array_value_op(ctx, fields, &src, range, gc::ARRAY_PUSH, None)
                .unwrap_or_else(|| synthesise_unsupported(ctx, e)),
            None => synthesise_unsupported(ctx, e),
        },
        "fill" => match nth_positional(args, 0)
            .and_then(|v| crate::lower::stmt::value_record_fields(ctx, v))
        {
            Some(src) => record_array_value_op(ctx, fields, &src, range, gc::ARRAY_FILL, None)
                .unwrap_or_else(|| synthesise_unsupported(ctx, e)),
            None => synthesise_unsupported(ctx, e),
        },
        "insert" => {
            let (Some(i), Some(v)) = (nth_positional(args, 0), nth_positional(args, 1)) else {
                return synthesise_unsupported(ctx, e);
            };
            let idx = lower_expr(ctx, i);
            match crate::lower::stmt::value_record_fields(ctx, v) {
                Some(src) => record_array_value_op(
                    ctx,
                    fields,
                    &src,
                    range,
                    gc::ARRAY_INSERT,
                    Some((WirePort::Index, idx)),
                )
                .unwrap_or_else(|| synthesise_unsupported(ctx, e)),
                None => synthesise_unsupported(ctx, e),
            }
        }
        "resize" => {
            let (Some(s), Some(v)) = (nth_positional(args, 0), nth_positional(args, 1)) else {
                return synthesise_unsupported(ctx, e);
            };
            let size = lower_expr(ctx, s);
            match crate::lower::stmt::value_record_fields(ctx, v) {
                Some(src) => record_array_value_op(
                    ctx,
                    fields,
                    &src,
                    range,
                    gc::ARRAY_RESIZE,
                    Some((WirePort::Size, size)),
                )
                .unwrap_or_else(|| synthesise_unsupported(ctx, e)),
                None => synthesise_unsupported(ctx, e),
            }
        }
        // Index-scalar mutations that reorder ROWS as a whole keep the parallel
        // arrays in lockstep, so they fan out: `swap(a, b)` moves both endpoints'
        // full records.
        "swap" => {
            let (Some(a), Some(b)) = (nth_positional(args, 0), nth_positional(args, 1)) else {
                return synthesise_unsupported(ctx, e);
            };
            let (pa, pb) = (lower_expr(ctx, a), lower_expr(ctx, b));
            let mut ret = ctx.current_exec;
            for leaf in &leaves {
                ret = Some(array_exec_op(
                    ctx,
                    range,
                    leaf.node_id.port(WirePort::ArrayVarRef),
                    gc::ARRAY_SWAP,
                    vec![
                        (WirePort::IndexA, Type::Int, pa),
                        (WirePort::IndexB, Type::Int, pb),
                    ],
                    vec![(WirePort::BOutOfBounds, Type::Bool)],
                    WirePort::ExecOut,
                ));
            }
            ret.unwrap_or_else(|| synthesise_unsupported(ctx, e))
        }
        // `pop` removes the last ROW and returns it as a record value (per-field
        // pop), stashed in `pending_inline_record` for a let/assign/out consumer.
        "pop" => {
            let record = record_array_pop(ctx, fields, range);
            let primary = record
                .values()
                .find_map(|b| match b {
                    Binding::Local(l) => Some(l.port),
                    _ => None,
                })
                .unwrap_or_else(|| synthesise_unsupported(ctx, e));
            ctx.pending_inline_record = Some(record);
            primary
        }
        // Everything else has no per-field-parallel-array meaning: `sort`/`shuffle`
        // reorder by value/randomly (desyncs the fields), the aggregates
        // (`sum`/`min`/`max`/`average`) fold over whole records, `find` needs an
        // all-fields match, `get(i)` duplicates `pts[i]`, and the dual-array /
        // fill-from-* ops need matching record arrays. Reject cleanly (WS050)
        // rather than silently lowering to a no-op placeholder.
        _ => {
            ctx.error(
                "WS050",
                format!(
                    "`.{method}()` is not supported on a record array - it has no \
                     per-field meaning (sorting/shuffling would desync the fields, and \
                     the aggregate/dual-array forms are not implemented). Use `pts[i]` \
                     for element access, or store a scalar array"
                ),
                range,
            );
            synthesise_unsupported(ctx, e)
        }
    }
}

/// Pop the last row of a record array: pop every parallel field array (lockstep)
/// and collect the popped values into a record. Recurses for nested records.
fn record_array_pop(
    ctx: &mut LowerCtx,
    fields: &HashMap<crate::intern::Sym, Binding>,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let mut out = HashMap::default();
    let mut names: Vec<crate::intern::Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| crate::intern::resolve(*s));
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => {
                out.insert(k, Binding::Record(record_array_pop(ctx, sub, range)));
            }
            Some(Binding::Var(v)) if v.storage == VarStorage::Array => {
                let elem = v.inner_type.clone();
                let aref = v.node_id.port(WirePort::ArrayVarRef);
                let port = array_exec_op(
                    ctx,
                    range,
                    aref,
                    gc::ARRAY_POP,
                    vec![],
                    vec![(WirePort::Value, elem), (WirePort::BIsEmpty, Type::Bool)],
                    WirePort::Value,
                );
                out.insert(k, Binding::Local(LocalRecord { port }));
            }
            _ => {}
        }
    }
    out
}

pub(super) fn lower_array_method(
    ctx: &mut LowerCtx,
    array_ref: PortRef,
    elem_ty: Type,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let mutates = crate::catalog::arrays::array_method(method).is_some_and(|m| m.mutates);
    if reject_const_container_mutation(
        ctx,
        array_ref.node_id,
        mutates,
        &format!("`.{method}()`"),
        range,
    ) {
        return synthesise_unsupported(ctx, e);
    }
    // `exec = <trigger>` named arg: drive the op off an explicit trigger instead
    // of the surrounding exec chain, so a read like `lut.get(i, exec = i + 1)`
    // works in a PURE context (e.g. an output binding). The get self-fires
    // whenever the index changes (i + 1 is never the no-fire value 0). The op is a
    // leaf in that case, so restore the caller's exec context afterward rather than
    // advancing it.
    let exec_arg: Option<&Expr> = args.iter().find_map(|a| match a {
        CallArg::Named { name, value, .. } if name == "exec" => Some(value),
        _ => None,
    });
    let saved_exec = ctx.current_exec;
    if let Some(exec_expr) = exec_arg {
        let src = lower_expr(ctx, exec_expr);
        ctx.current_exec = Some(src);
    }
    let current_exec = match ctx.current_exec {
        Some(e) => e,
        None => return synthesise_unsupported(ctx, e),
    };
    // Every method handled here must also appear in the canonical
    // `catalog::arrays::ARRAY_METHODS` table (which drives editor completion /
    // hover); the `every_canonical_array_method_lowers` test enforces it.
    let method_result = match method {
        "push" => {
            let val = match args.first() {
                Some(CallArg::Positional(v)) => lower_expr(ctx, v),
                _ => return synthesise_unsupported(ctx, e),
            };
            let exec_in = ctx.current_exec.unwrap_or(current_exec);
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::ARRAY_PUSH,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::EXEC,
                            ty: Type::Exec,
                        },
                        PortSpec {
                            name: *sym::ARRAY_VAR_REF,
                            ty: Type::Array(Box::new(Type::Any)),
                        },
                        PortSpec {
                            name: *sym::VALUE,
                            ty: Type::Any,
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: *sym::EXEC_OUT,
                            ty: Type::Exec,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(exec_in, node_id.port(WirePort::Exec));
            ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
            ctx.connect(val, node_id.port(WirePort::Value));
            ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
            node_id.port(WirePort::ExecOut)
        }
        // Result is a record { Value, IsEmpty } accessed via `.Value` /
        // `.IsEmpty`, or used bare as the popped element (its default). Both
        // outputs MUST be declared - otherwise `.IsEmpty` silently falls back
        // to the `Value` port, and the emitted gate's `Value` output binds to
        // the wrong schema slot (returning bIsEmpty = 0 for a non-empty pop).
        "pop" => array_exec_op(
            ctx,
            range,
            array_ref,
            gc::ARRAY_POP,
            vec![],
            vec![
                (WirePort::Value, elem_ty.clone()),
                (WirePort::BIsEmpty, Type::Bool),
            ],
            WirePort::Value,
        ),
        // `arr[i]` gives the element and drops the bounds flag. `get` exposes
        // both as a record { Value, OutOfBounds }, so a read can be checked
        // rather than silently reading 0 past the end. Bare use is the element,
        // matching `pop`.
        "get" => {
            let index = match args.first() {
                Some(CallArg::Positional(v)) => lower_expr(ctx, v),
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_GET,
                vec![(WirePort::Index, Type::Int, index)],
                vec![
                    (WirePort::Value, elem_ty.clone()),
                    (WirePort::BOutOfBounds, Type::Bool),
                ],
                WirePort::Value,
            )
        }
        "clear" | "shuffle" => {
            let exec_in = ctx.current_exec.unwrap_or(current_exec);
            let gate_class = if method == "clear" {
                gc::ARRAY_CLEAR
            } else {
                gc::ARRAY_SHUFFLE
            };
            let _base = if method == "clear" {
                "arrClear"
            } else {
                "arrShuffle"
            };
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::EXEC,
                            ty: Type::Exec,
                        },
                        PortSpec {
                            name: *sym::ARRAY_VAR_REF,
                            ty: Type::Array(Box::new(Type::Any)),
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: *sym::EXEC_OUT,
                            ty: Type::Exec,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(exec_in, node_id.port(WirePort::Exec));
            ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
            ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
            node_id.port(WirePort::ExecOut)
        }
        "remove" => {
            let idx = match args.first() {
                Some(CallArg::Positional(v)) => lower_expr(ctx, v),
                _ => return synthesise_unsupported(ctx, e),
            };
            let exec_in = ctx.current_exec.unwrap_or(current_exec);
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::ARRAY_REMOVE_AT_INDEX,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::EXEC,
                            ty: Type::Exec,
                        },
                        PortSpec {
                            name: *sym::ARRAY_VAR_REF,
                            ty: Type::Array(Box::new(Type::Any)),
                        },
                        PortSpec {
                            name: *sym::INDEX,
                            ty: Type::Int,
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: *sym::B_OUT_OF_BOUNDS,
                            ty: Type::Bool,
                        },
                        PortSpec {
                            name: *sym::EXEC_OUT,
                            ty: Type::Exec,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(exec_in, node_id.port(WirePort::Exec));
            ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
            ctx.connect(idx, node_id.port(WirePort::Index));
            ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
            node_id.port(WirePort::ExecOut)
        }
        "length" => {
            let exec_in = ctx.current_exec.unwrap_or(current_exec);
            let node_id = ctx.add_gate(AddNodeOpts {
                gate_class: gc::ARRAY_GET_LENGTH,
                source_range: range.clone(),
                ports: GateIO {
                    inputs: vec![
                        PortSpec {
                            name: *sym::EXEC,
                            ty: Type::Exec,
                        },
                        PortSpec {
                            name: *sym::ARRAY_VAR_REF,
                            ty: Type::Array(Box::new(Type::Any)),
                        },
                    ],
                    outputs: vec![
                        PortSpec {
                            name: intern_static("Length"),
                            ty: Type::Int,
                        },
                        PortSpec {
                            name: *sym::EXEC_OUT,
                            ty: Type::Exec,
                        },
                    ],
                },
                ..Default::default()
            });
            ctx.connect(exec_in, node_id.port(WirePort::Exec));
            ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
            ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
            node_id.port(WirePort::Length)
        }
        "insert" => {
            let (idx, val) = match (args.first(), args.get(1)) {
                (Some(CallArg::Positional(i)), Some(CallArg::Positional(v))) => {
                    (lower_expr(ctx, i), lower_expr(ctx, v))
                }
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_INSERT,
                vec![
                    (WirePort::Value, Type::Any, val),
                    (WirePort::Index, Type::Int, idx),
                ],
                vec![(WirePort::BOutOfBounds, Type::Bool)],
                WirePort::ExecOut,
            )
        }
        "find" => {
            let val = match args.first() {
                Some(CallArg::Positional(v)) => lower_expr(ctx, v),
                _ => return synthesise_unsupported(ctx, e),
            };
            // Result is a record { Index, Found } accessed via `.Index` /
            // `.Found`, or used bare as the index (its default). The gate's
            // `Value` output is the search arg passed through, so it isn't
            // exposed (it would collide with the `Value` input wire).
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_FIND,
                vec![(WirePort::Value, Type::Any, val)],
                vec![(WirePort::Index, Type::Int), (WirePort::BFound, Type::Bool)],
                WirePort::Index,
            )
        }
        "sort" => {
            let mut extra = vec![];
            if let Some(CallArg::Positional(d)) = args.first() {
                extra.push((WirePort::BDescending, Type::Bool, lower_expr(ctx, d)));
            }
            array_exec_op(ctx, range, array_ref, gc::ARRAY_SORT, extra, vec![], WirePort::ExecOut)
        }
        "reverse" => {
            array_exec_op(ctx, range, array_ref, gc::ARRAY_REVERSE, vec![], vec![], WirePort::ExecOut)
        }
        "sum" => array_exec_op(
            ctx,
            range,
            array_ref,
            gc::ARRAY_SUM,
            vec![],
            vec![(WirePort::Value, elem_ty.clone())],
            WirePort::Value,
        ),
        "min" | "max" => array_exec_op(
            ctx,
            range,
            array_ref,
            if method == "min" { gc::ARRAY_MIN } else { gc::ARRAY_MAX },
            vec![],
            vec![
                (WirePort::Value, elem_ty.clone()),
                (WirePort::BIsEmpty, Type::Bool),
            ],
            WirePort::Value,
        ),
        "average" => array_exec_op(
            ctx,
            range,
            array_ref,
            gc::ARRAY_AVERAGE,
            vec![],
            vec![(WirePort::Value, Type::Float), (WirePort::BIsEmpty, Type::Bool)],
            WirePort::Value,
        ),
        "swap" => {
            let (a, b) = match (args.first(), args.get(1)) {
                (Some(CallArg::Positional(a)), Some(CallArg::Positional(b))) => {
                    (lower_expr(ctx, a), lower_expr(ctx, b))
                }
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_SWAP,
                vec![
                    (WirePort::IndexA, Type::Int, a),
                    (WirePort::IndexB, Type::Int, b),
                ],
                vec![(WirePort::BOutOfBounds, Type::Bool)],
                WirePort::ExecOut,
            )
        }
        "fill" => {
            let val = match args.first() {
                Some(CallArg::Positional(v)) => lower_expr(ctx, v),
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_FILL,
                vec![(WirePort::Value, Type::Any, val)],
                vec![],
                WirePort::ExecOut,
            )
        }
        "resize" => {
            let (size, val) = match (args.first(), args.get(1)) {
                (Some(CallArg::Positional(s)), Some(CallArg::Positional(v))) => {
                    (lower_expr(ctx, s), lower_expr(ctx, v))
                }
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_RESIZE,
                vec![
                    (WirePort::Value, Type::Any, val),
                    (WirePort::Size, Type::Int, size),
                ],
                vec![],
                WirePort::ExecOut,
            )
        }
        "append" | "copyFrom" => {
            let Some(src) = resolve_array_ref_arg(ctx, args.first()) else {
                return synthesise_unsupported(ctx, e);
            };
            let gate = if method == "append" { gc::ARRAY_APPEND } else { gc::ARRAY_COPY_FROM };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gate,
                vec![(WirePort::SourceRef, Type::Array(Box::new(Type::Any)), src)],
                vec![],
                WirePort::ExecOut,
            )
        }
        "fillFromPlayers" => array_exec_op(
            ctx,
            range,
            array_ref,
            gc::GAMEMODE_FILL_FROM_PLAYERS,
            vec![],
            vec![],
            WirePort::ExecOut,
        ),
        "fillFromTeam" => {
            let team = match args.first() {
                Some(CallArg::Positional(t)) => lower_expr(ctx, t),
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::GAMEMODE_FILL_FROM_TEAM,
                vec![(WirePort::Team, Type::Entity, team)],
                vec![],
                WirePort::ExecOut,
            )
        }
        "slice" => {
            // dest.slice(source, start, count): copy source[start..start+count]
            // into this array.
            let Some(src) = resolve_array_ref_arg(ctx, args.first()) else {
                return synthesise_unsupported(ctx, e);
            };
            let (start, count) = match (args.get(1), args.get(2)) {
                (Some(CallArg::Positional(s)), Some(CallArg::Positional(c))) => {
                    (lower_expr(ctx, s), lower_expr(ctx, c))
                }
                _ => return synthesise_unsupported(ctx, e),
            };
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_SLICE,
                vec![
                    (WirePort::Start, Type::Int, start),
                    (WirePort::Count, Type::Int, count),
                    (WirePort::SourceRef, Type::Array(Box::new(Type::Any)), src),
                ],
                vec![(WirePort::BOutOfBounds, Type::Bool)],
                WirePort::ExecOut,
            )
        }
        "fillFromZoneEntities" | "fillFromZonePlayers" => {
            let gate = if method == "fillFromZoneEntities" {
                gc::ZONE_GET_ENTITIES
            } else {
                gc::ZONE_GET_PLAYERS
            };
            let zone = match args.first() {
                Some(CallArg::Positional(z)) => lower_expr(ctx, z),
                _ => return synthesise_unsupported(ctx, e),
            };
            let mut extra = vec![(WirePort::Zone, Type::Entity, zone)];
            // Optional tag filter: `tagFilter = <v>` or a second positional arg.
            let tag = args
                .iter()
                .find_map(|a| match a {
                    CallArg::Named { name, value, .. } if name == "tagFilter" => Some(value),
                    _ => None,
                })
                .or_else(|| match args.get(1) {
                    Some(CallArg::Positional(t)) => Some(t),
                    _ => None,
                });
            if let Some(t) = tag {
                let tp = lower_expr(ctx, t);
                extra.push((WirePort::TagFilter, Type::Any, tp));
            }
            array_exec_op(ctx, range, array_ref, gate, extra, vec![], WirePort::ExecOut)
        }
        "sortMultiple" => {
            // Parallel arrays fill ArrayVarRef2..8 (this array is the sort key);
            // a `descending` bool (named or a trailing non-array positional) sets
            // bDescending.
            const REFS: [WirePort; 7] = [
                WirePort::ArrayVarRef2,
                WirePort::ArrayVarRef3,
                WirePort::ArrayVarRef4,
                WirePort::ArrayVarRef5,
                WirePort::ArrayVarRef6,
                WirePort::ArrayVarRef7,
                WirePort::ArrayVarRef8,
            ];
            let mut extra: Vec<(WirePort, Type, PortRef)> = Vec::new();
            let mut descending: Option<&Expr> = None;
            for a in args {
                match a {
                    CallArg::Named { name, value, .. } if name == "descending" => {
                        descending = Some(value);
                    }
                    CallArg::Positional(expr) => {
                        if let Some(port) = resolve_array_ref_arg(ctx, Some(a)) {
                            if extra.len() < REFS.len() {
                                extra.push((
                                    REFS[extra.len()],
                                    Type::Array(Box::new(Type::Any)),
                                    port,
                                ));
                            }
                        } else {
                            descending = Some(expr);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(d) = descending {
                let dp = lower_expr(ctx, d);
                extra.push((WirePort::BDescending, Type::Bool, dp));
            }
            array_exec_op(
                ctx,
                range,
                array_ref,
                gc::ARRAY_SORT_MULTIPLE,
                extra,
                vec![(WirePort::BSuccess, Type::Bool)],
                WirePort::ExecOut,
            )
        }
        _ => synthesise_unsupported(ctx, e),
    };
    // An explicit `exec =` trigger makes this op a leaf: restore the caller's exec
    // context so the surrounding (possibly pure) lowering is unaffected.
    if exec_arg.is_some() {
        ctx.current_exec = saved_exec;
    }
    method_result
}

pub(super) fn lower_array_set(
    ctx: &mut LowerCtx,
    obj: &Expr,
    index: &Expr,
    value: &Expr,
    range: &SourceRange,
) {
    let current_exec = match ctx.current_exec {
        Some(e) => e,
        None => return,
    };
    // `pts[i] = rec` where `pts` is a record array: fan the record VALUE across
    // the parallel field arrays at the shared index. Checked before the single-
    // array resolution below, which would otherwise miss the `Binding::Record`.
    if let Some(fields) = resolve_record_array(ctx, obj)
        && lower_record_array_index_set(ctx, &fields, index, value, range)
    {
        return;
    }
    // Element type rides along so the `Value` port below is declared with
    // it (a `VarRecord.inner_type` for arrays IS the element type) — a
    // `Type::Any` Value port would hide the string → bool coercion from the
    // `ctx.connect` choke point, silently writing a raw String into a
    // `bool[]` slot.
    let (array_ref, var_name, elem_ty) = if let Expr::Ident { name, .. } = obj {
        if let Some(var_rec) = ctx.lookup_var(name).cloned() {
            if var_rec.storage == VarStorage::Array {
                (
                    var_rec.node_id.port(WirePort::ArrayVarRef),
                    name.clone(),
                    var_rec.inner_type.clone(),
                )
            } else {
                return;
            }
        } else if let Some(inp) = ctx.lookup_input(name).cloned() {
            let elem = match &inp.ty {
                Type::Array(e) => e.as_ref().clone(),
                _ => Type::Any,
            };
            (inp.node_id.port(WirePort::RerOutput), name.clone(), elem)
        } else {
            return;
        }
    } else if let Some(binding) = resolve_field_chain(ctx, obj).cloned() {
        // obj is a record field chain resolving to an array var
        if let Binding::Var(var_rec) = &binding {
            if var_rec.storage == VarStorage::Array {
                (
                    var_rec.node_id.port(WirePort::ArrayVarRef),
                    "rec_arr".to_string(),
                    var_rec.inner_type.clone(),
                )
            } else {
                return;
            }
        } else {
            return;
        }
    } else {
        return;
    };
    // `xs[i] = v` is a mutation, so the `const` container rule applies to it
    // exactly as it does to `xs.fill(v)`. Typecheck rejects the direct spelling
    // by name; this catches the aliased ones (a `const` table reaching a
    // `ys: T[]` parameter), which resolve to the same gate node.
    if reject_const_container_mutation(ctx, array_ref.node_id, true, "an index write", range) {
        return;
    }
    let index_port = lower_expr(ctx, index);
    let value_port = lower_expr(ctx, value);
    let exec_in = ctx.current_exec.unwrap_or(current_exec);
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::ARRAY_SET_AT_INDEX,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::EXEC,
                    ty: Type::Exec,
                },
                PortSpec {
                    name: *sym::ARRAY_VAR_REF,
                    ty: Type::Array(Box::new(Type::Any)),
                },
                PortSpec {
                    name: *sym::INDEX,
                    ty: Type::Int,
                },
                PortSpec {
                    name: *sym::VALUE,
                    ty: elem_ty,
                },
            ],
            outputs: vec![
                PortSpec {
                    name: *sym::EXEC_OUT,
                    ty: Type::Exec,
                },
            ],
        },
        note: Some("array set".into()),
        ..Default::default()
    });
    ctx.connect(exec_in, node_id.port(WirePort::Exec));
    ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
    ctx.connect(index_port, node_id.port(WirePort::Index));
    ctx.connect(value_port, node_id.port(WirePort::Value));
    ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
    if let Some(v) = ctx.lookup_var_mut(&var_name) {
        v.get_node_for_handler = None;
    }
}
