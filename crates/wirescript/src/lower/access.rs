use super::*;

/// Walk a chain of `Ident` / `FieldAccess` nodes, resolving through
/// `Binding::Record` maps. Returns the final `Binding` if every step
/// resolved to a record field, or `None` when the chain isn't entirely
/// record-based (e.g. the root ident isn't a record, or a field is
/// missing).
pub(super) fn resolve_field_chain<'a>(ctx: &'a LowerCtx, expr: &Expr) -> Option<&'a Binding> {
    match expr {
        Expr::Ident { name, .. } => ctx.scope.get(name),
        Expr::FieldAccess { obj, field, .. } => {
            let parent = resolve_field_chain(ctx, obj)?;
            if let Binding::Record(fields) = parent {
                fields.get(&crate::intern::intern(field))
            } else {
                None
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
        Binding::Record(_) | Binding::Output(_) | Binding::Chip(_) | Binding::Namespace(_) => None,
    }
}

/// Map a short field name (`.Forward`, `.Jump`) to the full gate port name it
/// stands for. InputSplitter exposes a few arbitrarily-named ports whose
/// surface field names differ from the underlying port.
pub(super) fn alias_output_field(field: &str) -> &str {
    match field {
        "Forward" => "InputForward",
        "Right" => "InputRight",
        "Jump" => "bPressedJump",
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
    // Try resolving through record bindings first.
    // The full expression `e` is `obj.field`, so resolve_field_chain on `e`
    // walks the entire chain (potentially nested: `a.b.c`).
    if let Some(binding) = resolve_field_chain(ctx, e).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }
    // If it's a nested record, we can't return a single port — fall through.

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
            // `field` may name an output port on the gate an inline call
            // lowers to — e.g. `.Found` / `.Index` on `arr.find(x)` used
            // directly, without first binding the result to a `let`. Lower the
            // call (emitting its gate) and resolve the field to a sibling
            // output port, exactly as the bound-ident path above does. Without
            // this, `obj` is never lowered and the field access degrades to an
            // `_Unsupported` placeholder — silently dropping the call.
            if let Expr::Call { .. } = obj {
                let obj_port = lower_expr(ctx, obj);
                if let Some(port) = resolve_output_field_port(ctx, obj_port.node_id, field) {
                    return port;
                }
            }
            synthesise_unsupported(ctx, e)
        }
    }
}

pub(super) fn lower_index_access(
    ctx: &mut LowerCtx,
    obj: &Expr,
    index: &Expr,
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let current_exec = match ctx.current_exec {
        Some(e) => e,
        None => return synthesise_unsupported(ctx, e),
    };
    let array_ref = if let Expr::Ident { name, .. } = obj {
        if let Some(var_rec) = ctx.lookup_var(name).cloned() {
            if var_rec.storage == VarStorage::Array {
                var_rec.node_id.port(WirePort::ArrayVarRef)
            } else {
                return synthesise_unsupported(ctx, e);
            }
        } else if let Some(inp) = ctx.lookup_input(name).cloned() {
            inp.node_id.port(WirePort::RerOutput)
        } else {
            return synthesise_unsupported(ctx, e);
        }
    } else if let Some(binding) = resolve_field_chain(ctx, obj).cloned() {
        // obj is a record field chain that resolves to an array var
        if let Binding::Var(var_rec) = &binding {
            if var_rec.storage == VarStorage::Array {
                var_rec.node_id.port(WirePort::ArrayVarRef)
            } else {
                return synthesise_unsupported(ctx, e);
            }
        } else {
            return synthesise_unsupported(ctx, e);
        }
    } else {
        return synthesise_unsupported(ctx, e);
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
    node_id.port(WirePort::Value)
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

/// Resolve a positional argument that names another array variable to its
/// `ArrayVarRef` port (for the dual-array ops append/copyFrom/slice).
fn resolve_array_ref_arg(ctx: &LowerCtx, arg: Option<&CallArg>) -> Option<PortRef> {
    if let Some(CallArg::Positional(Expr::Ident { name, .. })) = arg {
        let vr = ctx.lookup_var(name)?;
        if vr.storage == VarStorage::Array {
            return Some(vr.node_id.port(WirePort::ArrayVarRef));
        }
    }
    None
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
    let current_exec = match ctx.current_exec {
        Some(e) => e,
        None => return synthesise_unsupported(ctx, e),
    };
    // Every method handled here must also appear in the canonical
    // `catalog::arrays::ARRAY_METHODS` table (which drives editor completion /
    // hover); the `every_canonical_array_method_lowers` test enforces it.
    match method {
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
        _ => synthesise_unsupported(ctx, e),
    }
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
    let (array_ref, var_name) = if let Expr::Ident { name, .. } = obj {
        if let Some(var_rec) = ctx.lookup_var(name).cloned() {
            if var_rec.storage == VarStorage::Array {
                (var_rec.node_id.port(WirePort::ArrayVarRef), name.clone())
            } else {
                return;
            }
        } else if let Some(inp) = ctx.lookup_input(name).cloned() {
            (inp.node_id.port(WirePort::RerOutput), name.clone())
        } else {
            return;
        }
    } else if let Some(binding) = resolve_field_chain(ctx, obj).cloned() {
        // obj is a record field chain resolving to an array var
        if let Binding::Var(var_rec) = &binding {
            if var_rec.storage == VarStorage::Array {
                (var_rec.node_id.port(WirePort::ArrayVarRef), "rec_arr".to_string())
            } else {
                return;
            }
        } else {
            return;
        }
    } else {
        return;
    };
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
