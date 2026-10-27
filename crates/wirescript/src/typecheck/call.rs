//! Call-result typing: output types, generic inference, and the
//! `CallSignature` adapters.

use super::*;
use crate::analysis::types::type_str;
use crate::catalog::calls::CallSpec;
use crate::ir::gate_class;
use crate::types::infer::{InferError, Subst};
use crate::types::mono::{substitute, unwrap_ref};

/// The result type of a call whose declared output is a union. The math-variant
/// gates (`Blend`, `lerp`, `Easing`) carry the variant their inputs carry, so
/// the output is the widening join (`crate::types::coerce::widening_join`) of
/// the union-typed parameters' argument types; left a union, it would match no
/// operator overload. Arguments with no common widening (`Blend(vector, 1, t)`)
/// are WS033, as for the generic solver, falling back to the declared union.
/// Any other output type is returned unchanged.
fn union_output_type(
    ctx: &mut TypeCheckCtx<'_>,
    c: &CallSpec,
    args: &[CallArg],
    out_index: usize,
    range: &SourceRange,
) -> Type {
    let declared = c.outputs[out_index].ty.clone();
    // Get/Add Component give a reference of the type their `$Component/...` names.
    if matches!(
        c.gate_class,
        gate_class::GET_COMPONENT | gate_class::ADD_COMPONENT
    ) {
        let pos = c.params.iter().position(|p| p.name == "componentType");
        let arg = args
            .named("componentType")
            .or_else(|| args.positionals().nth(pos?));
        return match arg.and_then(super::props::component_type_arg) {
            Some(class) => Type::Component(class.to_string()),
            None => {
                // A `$Component/...` that names nothing has its own WS110.
                if !matches!(arg, Some(Expr::AssetRef { asset_type, .. }) if asset_type == crate::analysis::assets::COMPONENT)
                {
                    ctx.emit(
                        "WS110",
                        format!(
                            "`{}` takes the component type as `$Component/<name>`",
                            c.name
                        ),
                        arg.map_or(range, |a| a.range()).clone(),
                    );
                }
                Type::Any
            }
        };
    }
    // `GetBrickProperty` reads whatever its constant `property` holds.
    if c.gate_class == gate_class::GET_BRICK_PROPERTY {
        let pos = c.params.iter().position(|p| p.name == "property");
        let property = args
            .named("property")
            .or_else(|| args.positionals().nth(pos?));
        return property
            .and_then(super::sig::brick_property_member)
            .and_then(super::sig::brick_property_type)
            .unwrap_or(Type::Any);
    }
    // Generic passthrough builtins (`Sleep`, `SleepTicks`, `Select`, `Swap`): a
    // `Type::Param(name)` output, bare or as a record field, is the widening join
    // of the arguments at that parameter's positions, so
    // `Select(cond, a: T, b: T) -> T` gives the arguments' type, not `any`.
    if type_has_param(&declared) {
        return resolve_param_output(ctx, c, args, &declared, range);
    }
    // The output rides the input variant when it is a union directly (Blend /
    // lerp / Easing) OR contains one as a Record FIELD (a stateful gate like
    // `Tween`, whose `{ Value: <variant>, Arrived: exec }` should give a float
    // `Value` for a float target, not the full `float|int|vector|...` union). Grab
    // that union's mask; nothing to resolve if there is no union.
    let mask: Vec<Type> = match &declared {
        Type::Union(m) => m.clone(),
        Type::Record(fs) => match fs.iter().find_map(|(_, t)| match t {
            Type::Union(m) => Some(m.clone()),
            _ => None,
        }) {
            Some(m) => m,
            None => return declared,
        },
        _ => return declared,
    };
    let mut joined: Option<Type> = None;
    for (i, p) in c.params.iter().enumerate() {
        if matches!(p.ty, Type::Union(_))
            && let Some(CallArg::Positional(e)) = args.get(i)
        {
            let t = unwrap_ref(&infer::infer(ctx, e));
            if matches!(t, Type::Any) {
                continue;
            }
            joined = Some(match joined {
                None => t,
                Some(prev) => match widening_join_all([prev.clone(), t.clone()]) {
                    Some(j) => j,
                    None => {
                        ctx.emit(
                            "WS033",
                            format!(
                                "'{}': incompatible operand types {} and {} with no common \
                                 widening (the math-variant params must all agree, up to \
                                 numeric/rotation widening)",
                                c.name,
                                type_str(&prev),
                                type_str(&t),
                            ),
                            range.clone(),
                        );
                        return declared;
                    }
                },
            });
        }
    }
    let resolved = match joined {
        // Bool is never a math variant on its own (the mask is Float, Int, Vector,
        // Rotator, Quat, Color), so an all-bool join widens one more step to Int.
        Some(Type::Bool) if !mask_contains(&mask, &Type::Bool) => Type::Int,
        Some(t) => t,
        None => return declared,
    };
    // Apply the resolved variant: replace the bare union, or each union-typed
    // field of the Record output (leaving `Arrived: exec` and the like intact).
    match declared {
        Type::Union(_) => resolved,
        Type::Record(fields) => Type::Record(
            fields
                .into_iter()
                .map(|(k, ft)| {
                    let nt = if matches!(ft, Type::Union(_)) {
                        resolved.clone()
                    } else {
                        ft
                    };
                    (k, nt)
                })
                .collect(),
        ),
        other => other,
    }
}

/// Resolve a `Type::Param(name)` output (bare or inside a record, array or
/// tuple) to the widening join of the arguments passed for the same-named
/// parameters: the monomorphization of a generic builtin (`Sleep`, `Select`,
/// `Swap`). No concrete argument gives `Any`, never a `Param`, which must not
/// reach emit. No common widening (`Select(c, 5, "hello")`) is WS033, falling
/// back to `Any`. `resolve_param_output_inner` memoizes per name, so a name used
/// twice (`Swap`'s `Output` and `OutputB`) is joined and reported once.
fn resolve_param_output(
    ctx: &mut TypeCheckCtx<'_>,
    c: &CallSpec,
    args: &[CallArg],
    ty: &Type,
    range: &SourceRange,
) -> Type {
    let mut resolved = HashMap::default();
    resolve_param_output_inner(ctx, c, args, ty, range, &mut resolved)
}

fn resolve_param_output_inner(
    ctx: &mut TypeCheckCtx<'_>,
    c: &CallSpec,
    args: &[CallArg],
    ty: &Type,
    range: &SourceRange,
    resolved: &mut HashMap<String, Type>,
) -> Type {
    match ty {
        Type::Param(name) => {
            if let Some(t) = resolved.get(name) {
                return t.clone();
            }
            let mut joined: Option<Type> = None;
            let mut conflict = false;
            for (i, p) in c.params.iter().enumerate() {
                if let Type::Param(pn) = &p.ty
                    && pn == name
                    && let Some(CallArg::Positional(e)) = args.get(i)
                {
                    let t = unwrap_ref(&infer::infer(ctx, e));
                    if matches!(t, Type::Any) {
                        continue;
                    }
                    match joined.take() {
                        None => joined = Some(t),
                        Some(prev) => match widening_join_all([prev.clone(), t.clone()]) {
                            Some(j) => joined = Some(j),
                            None => {
                                ctx.emit(
                                    "WS033",
                                    format!(
                                        "'{}': '{name}' is {} from one argument but {} from \
                                         another, and all '{name}' arguments must be the same type",
                                        c.name,
                                        type_str(&prev),
                                        type_str(&t),
                                    ),
                                    range.clone(),
                                );
                                conflict = true;
                                break;
                            }
                        },
                    }
                }
            }
            let result = if conflict {
                Type::Any
            } else {
                joined.unwrap_or(Type::Any)
            };
            resolved.insert(name.clone(), result.clone());
            result
        }
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, ft)| {
                    (
                        k.clone(),
                        resolve_param_output_inner(ctx, c, args, ft, range, resolved),
                    )
                })
                .collect(),
        ),
        Type::Array(inner) => Type::Array(Box::new(resolve_param_output_inner(
            ctx, c, args, inner, range, resolved,
        ))),
        Type::Tuple(elems) => Type::Tuple(
            elems
                .iter()
                .map(|t| resolve_param_output_inner(ctx, c, args, t, range, resolved))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The result type of a call with at least one declared output, for the
/// builtin and receiver arms. One output widens through `union_output_type`;
/// several each widen on their own `out_index` and form a field-keyed record.
/// A zero-output `CallSpec` is the callers' own fallback.
pub(super) fn output_record_type(
    ctx: &mut TypeCheckCtx<'_>,
    c: &CallSpec,
    args: &[CallArg],
    range: &SourceRange,
) -> Type {
    let name =
        |o: &crate::catalog::calls::CallOutput| o.field.unwrap_or(o.port.as_str()).to_string();
    let outputs = if c.outputs.len() == 1 {
        // One declared output of several ports (`SplitVec`) gives each; a
        // type argument that is a record (`Select` of two records) is one
        // value.
        match union_output_type(ctx, c, args, 0, range) {
            Type::Record(fields) if matches!(c.outputs[0].ty, Type::Record(_)) => fields,
            t => vec![(name(&c.outputs[0]), t)],
        }
    } else {
        c.outputs
            .iter()
            .enumerate()
            .map(|(i, o)| (name(o), union_output_type(ctx, c, args, i, range)))
            .collect()
    };
    Type::call_result(outputs)
}

/// The number of positional arguments a `...t` spread contributes, or `None`
/// when its arity is unknown (a non-tuple or unresolved type keeps the call
/// arity-dynamic). A tuple or record literal counts its fields; a bound one uses
/// its recorded type.
fn spread_arity(ctx: &TypeCheckCtx<'_>, t: &Expr) -> Option<usize> {
    if let Expr::RecordLit { fields, .. } = t {
        return Some(
            fields
                .iter()
                .filter(|f| matches!(f, RecordLitField::Named { .. }))
                .count(),
        );
    }
    match unwrap_ref(ctx.recorded_type(t.range())?) {
        Type::Tuple(e) => Some(e.len()),
        Type::Record(f) | Type::Outputs(f) => Some(f.len()),
        _ => None,
    }
}

/// The type of each parameter slot `args` fills, in order: a positional
/// argument's own (from `positional_arg_types`, which lists the positional
/// arguments' types in order), and each element of a `...tuple` spread. Ends
/// at a spread whose elements are unknown, past which the slots don't line up.
fn spread_arg_types(
    ctx: &TypeCheckCtx<'_>,
    args: &[CallArg],
    positional_arg_types: &[Type],
) -> Vec<Type> {
    let recorded = |e: &Expr| ctx.recorded_type(e.range()).cloned();
    let mut positional = positional_arg_types.iter();
    let mut out = Vec::new();
    for a in args {
        match a {
            CallArg::Positional(_) => out.extend(positional.next().cloned()),
            CallArg::Named { .. } => {}
            CallArg::Spread(Expr::RecordLit { fields, .. }) => {
                for f in fields {
                    match f {
                        RecordLitField::Named { value, .. } => {
                            out.push(recorded(value).unwrap_or(Type::Any));
                        }
                        _ => return out,
                    }
                }
            }
            CallArg::Spread(t) => match recorded(t).map(|t| unwrap_ref(&t)) {
                Some(Type::Tuple(elems)) => out.extend(elems),
                Some(Type::Record(fields) | Type::Outputs(fields)) => {
                    out.extend(fields.into_iter().map(|(_, t)| t));
                }
                _ => return out,
            },
        }
    }
    out
}

/// Type a resolved user `mod` or `chip` call from its inferred positional
/// argument types, for the plain call and the `self` method path (which
/// prepends the receiver as argument 0). Reports WS021 (use before
/// declaration), WS022 (argument count) and WS033 (generic inference) and
/// returns the result type: the single output, a record of the outputs, or
/// `any`. For a method call, `positional_count`, `positional_arg_types` and
/// `args` (the full list, for `sig::check_args`) include the receiver.
/// `name_range` anchors the count and order diagnostics, `call_range` the
/// inference one.
#[expect(
    clippy::too_many_arguments,
    reason = "each input is a separate part of the call site"
)]
pub(super) fn type_user_symbol_call(
    ctx: &mut TypeCheckCtx<'_>,
    name: &str,
    sym: &SymbolInfo,
    positional_arg_types: &[Type],
    args: &[CallArg],
    type_args: &[TypeExpr],
    positional_count: usize,
    has_spread: bool,
    has_exec_arg: bool,
    call_range: &SourceRange,
    name_range: &SourceRange,
) -> Type {
    // Use before declaration: chips and mods register in source order during
    // lowering, so a call above its same-file declaration cannot resolve and would
    // become an `_Unsupported` gate reading 0. Imports are always available.
    if sym.kind == SymbolKind::Chip
        && sym.signature.is_some()
        && sym.decl_range.file == name_range.file
        && (name_range.start.line, name_range.start.col)
            < (sym.decl_range.start.line, sym.decl_range.start.col)
    {
        ctx.emit(
            "WS021",
            format!(
                "call to `{name}` before its declaration: chips and \
                 mods must be declared before the point where they \
                 are used (move the declaration above its first caller)"
            ),
            name_range.clone(),
        );
    }
    // The positional count must lie between the required count and the
    // parameter count (defaulted trailing parameters may be omitted); every
    // parameter, a record or destructured one and the `self` receiver included,
    // takes one positional argument. A `...tuple` spread contributes its element
    // count, and the check is skipped only for a spread of unknown arity.
    if let Some(sig) = &sym.signature {
        let expected = sig.params.len();
        let mut effective = positional_count;
        let mut all_known = true;
        for a in args {
            if let CallArg::Spread(t) = a {
                match spread_arity(ctx, t) {
                    Some(n) => effective += n,
                    None => all_known = false,
                }
            }
        }
        // A variadic mod (`...rest`) fixes only its leading parameters, so the count
        // only has to reach the fixed count; defaulted ones may still be omitted.
        let required = sig.required.min(expected);
        let count_wrong = if sig.variadic {
            effective < expected
        } else {
            effective < required || effective > expected
        };
        if all_known && count_wrong {
            let expectation = if sig.variadic {
                format!(
                    "at least {expected} argument{}",
                    if expected == 1 { "" } else { "s" }
                )
            } else if required < expected {
                format!("{required} to {expected} arguments")
            } else {
                format!(
                    "{expected} argument{}",
                    if expected == 1 { "" } else { "s" }
                )
            };
            ctx.emit(
                "WS022",
                format!(
                    "`{name}` expects {expectation} but {effective} {} given",
                    if effective == 1 { "was" } else { "were" },
                ),
                name_range.clone(),
            );
        }
    }
    if sym.kind == SymbolKind::Chip
        && let Some(decl) = ctx.resolve_mod(name)
        && decl.range.file == sym.decl_range.file
        && decl.range.start.offset == sym.decl_range.start.offset
    {
        super::shadow::note_call(ctx, name, &decl);
    }
    let Some(sig) = &sym.signature else {
        // The callee resolved to a non-callable symbol (a var, let, array, buffer,
        // input or parameter). Untyped, it would lower to an `_Unsupported` gate
        // reading 0. Common causes: an index typo (`xs(i)` for `xs[i]`) and a chained
        // comparison read as an explicit-type-argument call (`a < b > (c)`).
        ctx.emit(
            "WS038",
            format!("`{name}` is not callable: only mods, chips, and functions can be called"),
            name_range.clone(),
        );
        return Type::Any;
    };
    // An exec-requiring mod called from a pure position: its container ops, whose
    // WS007 is deferred inside the body, would inline into pure context, where a
    // container read yields the reference instead of a value. Reported as the
    // builtin exec-call WS007 is, unless the call has its own `exec =` trigger.
    // Only mods and chips (`SymbolKind::Chip`) are scanned through `resolve_mod`.
    if ctx.exec_mode() != ExecMode::Exec
        && !has_exec_arg
        && sym.kind == SymbolKind::Chip
        && infer::mod_is_exec_requiring(ctx, name)
    {
        ctx.emit(
            "WS007",
            format!(
                "exec call '{name}' outside an exec context (its body reads or writes a \
                 container; pass exec = ... or call it from an exec handler)"
            ),
            name_range.clone(),
        );
    }
    // Inference for a generic call: an equality constraint from every (declared
    // parameter type, inferred argument type) pair, solved per type parameter and
    // substituted into the output types below. A non-generic call skips it
    // (`subst` stays `None`).
    let subst: Option<Subst> = if sig.type_params.is_empty() {
        if !type_args.is_empty() {
            ctx.emit(
                "WS033",
                format!("`{name}` is not generic, so it takes no type arguments"),
                call_range.clone(),
            );
        }
        None
    } else if !type_args.is_empty() {
        // Explicit type arguments `f<int>(...)`: the caller pinned each type
        // param. Bind them directly (skipping arg-driven inference), validating
        // arity and each arg against its param's bound mask. This is the ONLY
        // way to pin a `T` that appears only in the return type (`make<int>()`),
        // which inference can't derive from the arguments.
        if type_args.len() != sig.type_params.len() {
            ctx.emit(
                "WS033",
                format!(
                    "`{name}` expects {} type argument{}, but {} {} given",
                    sig.type_params.len(),
                    if sig.type_params.len() == 1 { "" } else { "s" },
                    type_args.len(),
                    if type_args.len() == 1 { "was" } else { "were" },
                ),
                call_range.clone(),
            );
            None
        } else {
            let mut s = Subst::new();
            for ((pname, mask), te) in sig.type_params.iter().zip(type_args.iter()) {
                let ty = resolve_type_expr(ctx, te);
                if !crate::types::classes::mask_contains(mask, &ty) {
                    ctx.emit(
                        "WS033",
                        format!(
                            "`{pname}` = {}, which isn't allowed by its bound",
                            type_str(&ty),
                        ),
                        te.range().clone(),
                    );
                }
                s.insert(pname.clone(), ty);
            }
            Some(s)
        }
    } else {
        // Ref-align param and arg before collecting. A `*T` param resolves to
        // `Ref(Param(T))`, but a var passed to it infers to its already-auto-
        // derefed inner type (`Int`, not `Ref(Int)`), so the Ref layers are
        // asymmetric: strip a leading Ref off BOTH so `*T` vs `int` collects
        // as `Param(T)` vs `int` -> `Eq(T, int)`, exactly like a value `T`.
        let param_types: Vec<Type> = sig.params.iter().map(|p| p.ty.clone()).collect();
        let arg_types = if has_spread {
            std::borrow::Cow::Owned(spread_arg_types(ctx, args, positional_arg_types))
        } else {
            std::borrow::Cow::Borrowed(positional_arg_types)
        };
        let constraints = crate::types::mono::call_constraints(&param_types, &arg_types);
        match crate::types::infer::solve(&constraints, &sig.type_params) {
            Ok(s) => Some(s),
            Err(e) => {
                let msg = match &e {
                    InferError::Conflict { var, a, b } => format!(
                        "cannot infer '{var}': it's {} from one argument but {} from another, and all '{var}' arguments must be the same type",
                        type_str(a),
                        type_str(b),
                    ),
                    InferError::Unpinnable(var) => {
                        format!("cannot infer type parameter '{var}'; annotate the argument(s)")
                    }
                    InferError::OutOfMask { var, ty, .. } => format!(
                        "'{var}' = {}, which isn't allowed by its bound",
                        type_str(ty),
                    ),
                };
                ctx.emit("WS033", msg, call_range.clone());
                None
            }
        }
    };
    // The instantiation this call builds: lowering takes these type arguments as
    // they are, and its body is checked for exactly them.
    if !sig.type_params.is_empty()
        && let Some(s) = &subst
    {
        let type_args: Vec<Type> = sig
            .type_params
            .iter()
            .map(|(p, _)| s.get(p).cloned().unwrap_or(Type::Any))
            .collect();
        let named = sig
            .type_params
            .iter()
            .map(|(p, _)| p.clone())
            .zip(type_args.iter().cloned())
            .collect();
        let key = sym.decl_range.start_key();
        ctx.generic_call_args.entry(key).or_default().push(named);
        super::instances::demand(ctx, &sym.decl_range, type_args, call_range);
    }
    // Each argument against its substituted parameter type, the coercion the
    // wire layer applies, through `sig::check_args` (arity was WS022 above).
    // A `...tuple` spread is checked per element; one of unknown arity adds no
    // slots, and a still-generic parameter is left to WS033.
    let checked = sig_of_fnchip(name, sig, subst.as_ref());
    check_args(
        ctx, &checked, args, /* check_arity */ false,
        // A user mod or chip knows its full parameter list, so an unknown named
        // argument (`g(1, bogus = 5)`) is WS041; the arity check is off only because
        // WS022 above already reported the count.
        /* check_named */
        true, call_range,
    );
    infer::args_fill_float_params(ctx, &checked.params, args);
    let out_ty = |t: &Type| match &subst {
        Some(s) => substitute(t, s),
        None => t.clone(),
    };
    // A call with an `exec =` trigger also returns the chip's completion exec as
    // an `exec` field (unless the chip declares its own `exec` output).
    if !sig.outputs.is_empty() {
        let mut fields: Vec<(String, Type)> = sig
            .outputs
            .iter()
            .map(|o| (o.name.clone(), out_ty(&o.ty)))
            .collect();
        if has_exec_arg && !fields.iter().any(|(n, _)| n == "exec") {
            fields.push(("exec".into(), Type::Exec));
        }
        return Type::call_result(fields);
    }
    // No outputs: the call has no value to give. `Never` is the "no value" type (a
    // void container mutation such as `a.push(x)` carries it), so the shared
    // rejections (`let_binding::reject_never_value`, `infer::coerce_or_emit`)
    // catch a consumer while a statement-position call stays clean. As `Any`,
    // `o = noret(1)` would wire an exec pin into the store's `Value`. An
    // `exec =` trigger keeps `Any`, since that spelling returns the chain's
    // completion as a real port. A `fn` call returns its `Completed` exec as its
    // one field.
    if has_exec_arg && sig.is_fn {
        return Type::outputs(vec![("exec".into(), Type::Exec)]);
    }
    if has_exec_arg {
        return Type::Any;
    }
    Type::Never
}

/// Reference-only types: like a variable ref they wire and reroute, but they
/// are not values and cannot be selected, stored or operated on. The explicit
/// `ref T`, the opaque `zone` and `teleport` component references, and the
/// compile-time `prefab` reference.
pub(super) fn is_reference_type(t: &Type) -> bool {
    matches!(
        t,
        Type::Ref(_) | Type::Zone | Type::Teleport | Type::PrefabRef
    )
}

// Generic mod and chip call-site inference
//
// `mono::call_constraints` walks each (declared parameter type, inferred
// argument type) pair and records an equality `Constraint` wherever the
// parameter side names a `Type::Param`; `types::infer::solve` turns them into a
// `Subst`, and `mono::substitute` applies it to the output type. These live in
// `types::mono`, so lowering's monomorphizer infers the same way.

/// Operand type for operator-overload resolution: unwrap a `ref`, then take a
/// gate read's first output. A gate like `ParseInt`/`GetDamage` exposes
/// `{ Value, Success }` / `{ Damage, DamageLimit }`, whose first output is the
/// value the call "is", so `ParseInt(s) == n` compares against that value, as
/// an assignment or an argument reads it. A record of the program's own has
/// no first output (`infer::record_read_as_one_value`).
pub(super) fn op_operand_type(t: &Type) -> Type {
    match unwrap_ref(t) {
        Type::Outputs(fields) if !fields.is_empty() => unwrap_ref(&fields[0].1),
        other => other,
    }
}

/// A builtin or receiver `CallSpec` as the `CallSignature` `sig::check_args`
/// validates, so both call forms share one argument checker. Each `CallParam`
/// maps to a `Param` whose `ParamKind` is the branch the checker takes (enum,
/// composite or scalar config, or ordinary wire), keyed by the gate port name
/// the config validators' schema lookups use.
pub(super) fn sig_of_callspec(spec: &CallSpec) -> CallSignature {
    let params = spec
        .params
        .iter()
        .map(|p| {
            let kind = if let Some(et) = call_param_config_enum(spec, p) {
                ParamKind::ConfigEnum(et)
            } else if let Some(et) = call_param_wire_enum(spec, p) {
                ParamKind::WireEnum(et)
            } else if is_composite_config_param(spec, p) {
                ParamKind::ConfigComposite(p.port.as_str())
            } else if is_scalar_config_param(spec, p) {
                ParamKind::ConfigScalar(p.port.as_str())
            } else {
                ParamKind::Wire
            };
            Param {
                name: p.name.to_string(),
                ty: p.ty.clone(),
                optional: p.optional,
                kind,
            }
        })
        .collect();
    CallSignature {
        name: spec.name.to_string(),
        params,
        config_gate: Some(spec.gate_class),
    }
}

/// A user mod or chip's `FnOrChipSig` as a `CallSignature` for
/// `sig::check_args`, the counterpart of `sig_of_callspec`. Every parameter is
/// `ParamKind::Wire` except a `const` one (`name: const T`, or any parameter of
/// a `const mod`), which is `ParamKind::Const`; parameters past `required` (the
/// defaulted ones) are optional. `subst`, the caller's generic inference
/// (`None` for a non-generic call), applies to each type first; a type still
/// carrying a `Type::Param` is skipped by `check_args`. `config_gate` is
/// `None`: an unmatched named argument has no data-driven fallback.
fn sig_of_fnchip(name: &str, sig: &FnOrChipSig, subst: Option<&Subst>) -> CallSignature {
    let params = sig
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = match subst {
                Some(s) => substitute(&p.ty, s),
                None => p.ty.clone(),
            };
            Param {
                name: p.name.clone(),
                ty,
                optional: i >= sig.required,
                kind: if p.is_const {
                    ParamKind::Const
                } else {
                    ParamKind::Wire
                },
            }
        })
        .collect();
    CallSignature {
        name: name.to_string(),
        params,
        config_gate: None,
    }
}
