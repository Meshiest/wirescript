use super::*;

// ---------- expressions ----------

pub(super) fn lower_expr(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    match e {
        Expr::IntLit { value, .. } => literal_node(ctx, e, Type::Int, Literal::Int(*value)),
        Expr::AtomLit { value, .. } => literal_node(ctx, e, Type::Int, Literal::Int(*value)),
        Expr::FloatLit { value, .. } => literal_node(ctx, e, Type::Float, Literal::Float(*value)),
        Expr::BoolLit { value, .. } => literal_node(ctx, e, Type::Bool, Literal::Bool(*value)),
        // `null` lowers to its resolved type's zero/default (typecheck recorded
        // the type via `check_null`): an unset object, `0`, `false`, `""`, …
        Expr::NullLit { .. } => {
            let t = unwrap_ref(&ctx.type_of(e));
            let lit = default_literal_for_var_type(&t).unwrap_or(Literal::Float(0.0));
            literal_node(ctx, e, t, lit)
        }
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
                    outputs: vec![PortSpec {
                        name: *sym::VALUE,
                        ty: Type::Entity,
                    }],
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
            lower_ident(ctx, name, range)
        }
        Expr::BinOp { .. } => lower_binop(ctx, e),
        Expr::UnOp { .. } => lower_unop(ctx, e),
        Expr::Deref { operand, range } => {
            if let Expr::Ident { name, .. } = operand.as_ref()
                && let Some(var_rec) = ctx.lookup_var(name).cloned()
            {
                let inner = var_rec.inner_type.clone();
                if let Some(exec) = ctx.current_exec {
                    let get_id = ctx.add_gate(AddNodeOpts {
                        gate_class: gc::VAR_GET,
                        source_range: range.clone(),
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
                        note: None,
                        ..Default::default()
                    });
                    ctx.connect(exec, get_id.port(WirePort::Exec));
                    ctx.connect(
                        var_rec.node_id.port(WirePort::VarRef),
                        get_id.port(WirePort::VarRef),
                    );
                    ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
                    return get_id.port(WirePort::Value);
                }
                ctx.warn(
                    format!(
                        "'*{}' deref requires exec context — use .Value for pure reads",
                        name
                    ),
                    range,
                );
                return var_rec.node_id.port(WirePort::Value);
            }
            lower_expr(ctx, operand)
        }
        Expr::TuplePick { range, .. } => {
            if let Some(binding) = resolve_field_chain(ctx, e).cloned()
                && let Some(port) = binding_to_port(ctx, &binding, range)
            {
                return port;
            }
            synthesise_unsupported(ctx, e)
        }
        Expr::FieldAccess { obj, field, range } => lower_field_access(ctx, obj, field, range, e),
        Expr::IndexAccess { obj, index, range } => lower_index_access(ctx, obj, index, range, e),
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range,
        } => lower_if_expr(ctx, cond, then_branch, else_branch, range),
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
            synthesise_unsupported_range(ctx, range)
        }
        // A map literal reaching the generic expression lowerer is in an
        // unsupported position (not a map-var initializer). Lowering intercepts
        // `MapLit` in the assignment/initializer path before it reaches here.
        Expr::MapLit { range, .. } => synthesise_unsupported_range(ctx, range),
        _ => synthesise_unsupported(ctx, e),
    }
}

/// The `*Reference` gate class that sources an asset of `asset_type` as an
/// `entity` wire — it holds the asset in its class/object `Asset` field and
/// outputs `Value: entity`.
fn asset_reference_gate(asset_type: &str) -> &'static str {
    match asset_type {
        "BRItemBase" => "BrickComponentType_WireGraph_ItemReference",
        "BRPickupBase" => "BrickComponentType_WireGraph_PickupReference",
        "BRWeaponProjectile" => "BrickComponentType_WireGraph_ProjectileReference",
        "BrickAudioDescriptor" => "BrickComponentType_WireGraph_AudioReference",
        "BrickFontDescriptor" => "BrickComponentType_WireGraph_FontReference",
        "BrickOneShotAudioDescriptor" => "BrickComponentType_WireGraph_OneShotAudioReference",
        "BrickWheelEngineAudioDescriptor" => {
            "BrickComponentType_WireGraph_WheelEngineAudioReference"
        }
        // Unknown/other categories → the generic entity-type reference.
        _ => "BrickComponentType_WireGraph_EntityTypeReference",
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
/// mod/chip parameter — `lower_chip_call_inline`'s `is_const` branch stashes
/// whatever `const_eval::eval_expr` returns for the call argument into
/// `scoped_consts` with NO filter on the literal's kind, unlike a top-level
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
pub(super) fn wire_type_of_literal(lit: &Literal) -> Option<Type> {
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
            ports: GateIO {
                inputs: vec![],
                outputs: vec![PortSpec {
                    name: *sym::OUTPUT,
                    ty: Type::String,
                }],
            },
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
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty,
            }],
        },
        properties: props,
        ..Default::default()
    });
    node_id.port(WirePort::Output)
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
                    note: None,
                    ..Default::default()
                });
                ctx.connect(exec, get_id.port(WirePort::Exec));
                ctx.connect(
                    var_rec.node_id.port(WirePort::VarRef),
                    get_id.port(WirePort::VarRef),
                );
                ctx.current_exec = Some(get_id.port(WirePort::ExecOut));
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
        Some(Binding::Record(_)) => {
            // Records are compile-time bundles; they don't produce a single port.
            // Field access on records is handled in lower_field_access.
            synthesise_unsupported_range(ctx, range)
        }
        Some(Binding::Output(_) | Binding::Chip(_) | Binding::Namespace(_)) => {
            synthesise_unsupported_range(ctx, range)
        }
        // Not bound to any wire at all. Before giving up, check whether the
        // name is a compile-time CONSTANT (`ctx.const_lookup()`: the top-level
        // `const_env` overlaid by every open `scoped_consts` frame) and, if so,
        // materialize it as a literal source gate — the existing
        // literal-inlining pass (`lower/mod.rs`) then folds that gate's value
        // straight into whatever consumes it and prunes the gate, so a constant
        // read in a WIRE position costs nothing while still producing a real
        // operand.
        //
        // This is what a `const` PARAMETER needs: `lower_chip_call_inline`
        // records its value in `scoped_consts` (no wire, so a const-only use
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
        None => match ctx.const_lookup().get(name).cloned() {
            Some(lit) => match wire_type_of_literal(&lit) {
                Some(ty) => literal_node_range(ctx, range, ty, lit),
                None => synthesise_unsupported_range(ctx, range),
            },
            None => synthesise_unsupported_range(ctx, range),
        },
    }
}

pub(super) fn lower_if_expr(
    ctx: &mut LowerCtx,
    cond: &Expr,
    then_br: &Expr,
    else_br: &Expr,
    range: &SourceRange,
) -> PortRef {
    let cond_port = lower_expr(ctx, cond);
    let then_port = lower_expr(ctx, then_br);
    let else_port = lower_expr(ctx, else_br);
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
    // Non-generic lowering keeps the byte-identical `type_of` path.
    let (then_ty, else_ty) = if ctx.mono_stack.is_empty() {
        (unwrap_ref(&ctx.type_of(then_br)), unwrap_ref(&ctx.type_of(else_br)))
    } else {
        let then_ty = super::call::arg_port_type(ctx, then_port).unwrap_or_else(|| ctx.type_of(then_br));
        let else_ty = super::call::arg_port_type(ctx, else_port).unwrap_or_else(|| ctx.type_of(else_br));
        (unwrap_ref(&then_ty), unwrap_ref(&else_ty))
    };
    let result_ty =
        crate::types::coerce::widening_join(&then_ty, &else_ty).unwrap_or(else_ty);
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::SELECT,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec {
                    name: *sym::INPUT_A,
                    ty: result_ty.clone(),
                },
                PortSpec {
                    name: *sym::INPUT_B,
                    ty: result_ty.clone(),
                },
                PortSpec {
                    name: *sym::B_SELECT_B,
                    ty: Type::Bool,
                },
            ],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: result_ty.clone(),
            }],
        },
        note: Some("if-expr select".into()),
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
        // `ctx.const_lookup()` (top-level `const_env` overlaid by every open
        // `scoped_consts` frame — see `LowerCtx::push_scope`/`pop_scope`).
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
        match ctx.const_lookup().get(name).cloned() {
            Some(Literal::Vector { .. } | Literal::Rotator { .. } | Literal::Quat { .. } | Literal::LinearColor { .. }) => None,
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
    lit.map(|lit| bake_string_bool(lit, port_ty))
}

pub(super) fn synthesise_unsupported(ctx: &mut LowerCtx, e: &Expr) -> PortRef {
    synthesise_unsupported_range(ctx, e.range())
}

pub(super) fn synthesise_unsupported_range(ctx: &mut LowerCtx, range: &SourceRange) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::UNSUPPORTED,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![],
            outputs: vec![PortSpec {
                name: *sym::OUTPUT,
                ty: Type::Any,
            }],
        },
        note: Some("unsupported expression".into()),
        ..Default::default()
    });
    ctx.warn(
        "IR lowering not yet supported for this expression — emitted placeholder",
        range,
    );
    node_id.port(WirePort::Output)
}
