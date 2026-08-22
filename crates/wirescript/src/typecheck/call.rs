//! Call-result typing: output types, generic inference, and the
//! `CallSignature` adapters.

use super::*;
use crate::types::mono::{substitute, unwrap_ref};

/// The result type of a call whose declared output is a union.
///
/// The math-variant gates (`Blend`/`lerp`/`Easing`) carry whichever variant
/// their inputs do, so a union output resolves to the **widening join**
/// (`crate::types::coerce::widening_join`) of every union-typed param's
/// argument type — the least upper bound, not just "first arg wins". Left as
/// the union, the result would satisfy no operator overload and every use of
/// it would fail. When only one operand is concrete, the join is just that
/// type. When two operands have no common
/// widening (e.g. `Blend(vector, 1, t)`), that's a genuine incompatibility —
/// emit `WS033` (the same code the generic-mod inference solver uses for an
/// unwidenable conflict; this is the same kind of failure, just for a
/// builtin's dynamically-typed param instead of a user type parameter) and
/// fall back to the declared union. Any other output type is returned
/// unchanged.
fn union_output_type(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    out_index: usize,
    range: &SourceRange,
) -> Type {
    let declared = c.outputs[out_index].ty.clone();
    // Generic passthrough builtins (`Sleep`/`SleepTicks`/`Select`/`Swap`): a
    // `Type::Param(name)` output — bare or as a Record field — resolves to the
    // widening join of the args at that same param's positions, so
    // `Select(cond, a: T, b: T) -> T` yields the args' type instead of `any`.
    if type_has_param(&declared) {
        return resolve_param_output(ctx, c, args, &declared, range);
    }
    // The output rides the input variant when it is a union directly (Blend /
    // lerp / Easing) OR contains one as a Record FIELD (a stateful gate like
    // `Tween`, whose `{ Value: <variant>, Arrived: exec }` should give a float
    // `Value` for a float target, not the full `float|int|vector|…` union). Grab
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
                                "'{}': incompatible operand types {} and {} — no common \
                                 widening (the math-variant params must all agree, up to \
                                 numeric/rotation widening)",
                                c.name,
                                crate::analysis::types::type_str(&prev),
                                crate::analysis::types::type_str(&t),
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
        // Bool never appears as a math-variant on its own (the mask is
        // Float/Int/Vector/Rotator/Quat/Color) — an all-bool fold widens one
        // step further to Int, the mask's narrowest numeric member.
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
                    let nt = if matches!(ft, Type::Union(_)) { resolved.clone() } else { ft };
                    (k, nt)
                })
                .collect(),
        ),
        other => other,
    }
}

/// Resolve a `Type::Param(name)` output (bare or nested in a Record/Array/Tuple)
/// to the widening join of the arguments passed to the same-named `Param` params
/// — the monomorphization for a generic builtin (`Sleep`/`Select`/`Swap`). An
/// unresolved param (no concrete positional arg) falls back to `Any`, never
/// `Param` — a `Param` must never reach emit. Args with no common widening
/// (e.g. `Select(c, 5, "hello")`) are a genuine conflict — the same failure
/// `union_output_type` reports for math-variant params and the generic-mod
/// solver reports for a user type parameter — so it's reported the same way:
/// emit `WS033` and fall back to `Any` so the conflict doesn't cascade.
/// Delegates to `resolve_param_output_inner`, which memoizes each param name
/// in `resolved` so a name referenced more than once in the output type
/// (`Swap`'s `Output`/`OutputB` both share `T`) is only joined — and any
/// conflict only reported — once.
fn resolve_param_output(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    ty: &Type,
    range: &SourceRange,
) -> Type {
    let mut resolved = HashMap::default();
    resolve_param_output_inner(ctx, c, args, ty, range, &mut resolved)
}

fn resolve_param_output_inner(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
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
                                         another — all '{name}' arguments must be the same type",
                                        c.name,
                                        crate::analysis::types::type_str(&prev),
                                        crate::analysis::types::type_str(&t),
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
            let result = if conflict { Type::Any } else { joined.unwrap_or(Type::Any) };
            resolved.insert(name.clone(), result.clone());
            result
        }
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, ft)| {
                    (k.clone(), resolve_param_output_inner(ctx, c, args, ft, range, resolved))
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

/// The result type of a call with at least one declared output — shared by
/// the builtin and receiver call arms. A single output widens directly via
/// `union_output_type`; multiple outputs each widen independently (per their
/// own `out_index`) and assemble into a field-keyed record, so a multi-output
/// gate whose field rides the math variant (like a single-output `Blend`)
/// still resolves to the argument type instead of the declared union. Callers
/// with a zero-output `CallSpec` handle that fallback themselves (the
/// builtin and receiver arms differ there) and never call this helper.
pub(super) fn output_record_type(
    ctx: &mut TypeCheckCtx,
    c: &crate::catalog::calls::CallSpec,
    args: &[CallArg],
    range: &SourceRange,
) -> Type {
    if c.outputs.len() == 1 {
        return union_output_type(ctx, c, args, 0, range);
    }
    Type::Record(
        c.outputs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                (
                    o.field.unwrap_or(o.port.as_str()).to_string(),
                    union_output_type(ctx, c, args, i, range),
                )
            })
            .collect(),
    )
}

/// Type a resolved user `mod`/`chip` call from its already-inferred positional
/// argument types. Shared by the plain-identifier call path and the
/// `self`-receiver method path (which prepends the receiver as positional
/// arg 0). Emits WS021 (use-before-declaration), WS022 (argument count) and
/// WS033 (generic inference), then returns the call's result type — the
/// (possibly monomorphized) single output, a record of the outputs, or `any`.
///
/// `positional_count` and `positional_arg_types` must already include the
/// receiver for a method call. `name_range` anchors the count / decl-order
/// diagnostics; `call_range` anchors the generic-inference one. `args` feeds
/// the `sig::check_args` arg-coercion pass — it must be the FULL `CallArg`
/// list, receiver included as its own leading `CallArg::Positional` for a
/// method call (mirroring `positional_arg_types`).
#[allow(clippy::too_many_arguments)]
pub(super) fn type_user_symbol_call(
    ctx: &mut TypeCheckCtx,
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
    // Use-before-declaration. Chips/mods are registered in source order during
    // lowering, so a call whose declaration lexically follows the call site
    // cannot resolve — it would synthesise an `_Unsupported` gate that silently
    // reads 0 at runtime. Only applies to same-file chip/mod decls (imports
    // live elsewhere and are always available).
    if sym.kind == SymbolKind::Chip
        && sym.signature.is_some()
        && sym.decl_range.file == name_range.file
        && (name_range.start.line, name_range.start.col)
            < (sym.decl_range.start.line, sym.decl_range.start.col)
    {
        ctx.emit(
            "WS021",
            format!(
                "call to `{name}` before its declaration — chips and \
                 mods must be declared before the point where they \
                 are used (move the declaration above its first caller)"
            ),
            name_range.clone(),
        );
    }
    // Argument-count check. User chips/mods/fns have no default parameters, so
    // the positional-argument count must equal the parameter count — each param
    // (including a whole-record or destructured one, and the `self` receiver)
    // takes exactly one positional arg. A spread makes the count dynamic, so
    // skip the check then. A mismatch would otherwise leave a param unbound,
    // silently reading 0 / an empty value.
    if let Some(sig) = &sym.signature
        && !has_spread
    {
        let expected = sig.params.len();
        if positional_count != expected {
            ctx.emit(
                "WS022",
                format!(
                    "`{name}` expects {expected} argument{} but {positional_count} {} given",
                    if expected == 1 { "" } else { "s" },
                    if positional_count == 1 { "was" } else { "were" },
                ),
                name_range.clone(),
            );
        }
    }
    let Some(sig) = &sym.signature else {
        // The callee resolved to a non-callable symbol — a var / let / array /
        // buffer / input / param, not a mod, chip, or function. Without this the
        // call typed as `any` with no diagnostic and lowering emitted an
        // `_Unsupported` gate that reads 0 — a silent miscompile. Common causes:
        // an index typo (`xs(i)` for `xs[i]`) and a chained comparison the
        // parser reads as an explicit-type-argument call (`a < b > (c)`).
        ctx.emit(
            "WS038",
            format!("`{name}` is not callable — only mods, chips, and functions can be called"),
            name_range.clone(),
        );
        return Type::Any;
    };
    // Arg-driven inference for a generic mod/chip call: collect an equality
    // constraint from every (declared param type, inferred arg type) pair, solve
    // for each type param, and substitute the result into the output types
    // below. Guarded on `type_params` being non-empty so a non-generic call
    // takes exactly the pre-generics path — `subst` stays `None` and `out_ty`
    // is a plain clone.
    let subst: Option<crate::types::infer::Subst> = if sig.type_params.is_empty() {
        if !type_args.is_empty() {
            ctx.emit(
                "WS033",
                format!("`{name}` is not generic — it takes no type arguments"),
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
            let mut s = crate::types::infer::Subst::new();
            for ((pname, mask), te) in sig.type_params.iter().zip(type_args.iter()) {
                let ty = resolve_type_expr(ctx, te);
                if !crate::types::classes::mask_contains(mask, &ty) {
                    ctx.emit(
                        "WS033",
                        format!(
                            "`{pname}` = {}, which isn't allowed by its bound",
                            crate::analysis::types::type_str(&ty),
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
        // as `Param(T)` vs `int` → `Eq(T, int)`, exactly like a value `T`.
        let param_types: Vec<Type> = sig.params.iter().map(|p| p.ty.clone()).collect();
        let constraints = crate::types::mono::call_constraints(&param_types, positional_arg_types);
        match crate::types::infer::solve(&constraints, &sig.type_params) {
            Ok(s) => Some(s),
            Err(e) => {
                let msg = match &e {
                    crate::types::infer::InferError::Conflict { var, a, b } => format!(
                        "cannot infer '{var}': it's {} from one argument but {} from another — all '{var}' arguments must be the same type",
                        crate::analysis::types::type_str(a),
                        crate::analysis::types::type_str(b),
                    ),
                    crate::types::infer::InferError::Unpinnable(var) => format!(
                        "cannot infer type parameter '{var}' — annotate the argument(s)"
                    ),
                    crate::types::infer::InferError::OutOfMask { var, ty, .. } => format!(
                        "'{var}' = {}, which isn't allowed by its bound",
                        crate::analysis::types::type_str(ty),
                    ),
                };
                ctx.emit("WS033", msg, call_range.clone());
                None
            }
        }
    };
    // Validate each argument against its (substituted) parameter type — the
    // same coercion the wire layer applies (`PortsAreCompatible`), routed
    // through the shared `sig::check_args` (arity already checked above as
    // WS022, so `check_arity = false` here). Skipping this would let
    // `f(int)` on a `vector` param — or a receiver call `x.m()` whose `x`'s
    // type doesn't match `self` — pass clean and then miscompile at the wire
    // level. Skipped only for a spread (variable positional count, nothing
    // to line up positionally); `check_args`'s own `Wire`-arm coerce already
    // treats `Any`/`Opaque` args as always-`Same` and skips a still-generic
    // (`Type::Param`-carrying) param — the latter left to the WS033
    // inference diagnostics above.
    if !has_spread {
        check_args(
            ctx,
            &sig_of_fnchip(name, sig, subst.as_ref()),
            args,
            0,
            /* check_arity */ false,
            // A user mod/chip DOES know its full param list, so flag an unknown
            // named arg (`g(1, bogus = 5)`) as WS041 — the arity check is off
            // only to avoid double-reporting count against WS022 above.
            /* check_named */ true,
            call_range,
        );
    }
    let out_ty = |t: &Type| match &subst {
        Some(s) => substitute(t, s),
        None => t.clone(),
    };
    // A call with an `exec =` trigger also returns the chip's completion exec as
    // an `exec` field (unless the chip declares its own `exec` output).
    if sig.outputs.len() == 1 && !has_exec_arg {
        return out_ty(&sig.outputs[0].ty);
    }
    if !sig.outputs.is_empty() {
        let mut fields: Vec<(String, Type)> = sig
            .outputs
            .iter()
            .map(|o| (o.name.clone(), out_ty(&o.ty)))
            .collect();
        if has_exec_arg && !fields.iter().any(|(n, _)| n == "exec") {
            fields.push(("exec".into(), Type::Exec));
        }
        return Type::Record(fields);
    }
    Type::Any
}

/// Reference-only types: like a variable ref, these wire and reroute but are not
/// values — they can't be selected, stored, or operated on. Covers the explicit
/// `ref T` var ref plus the opaque `zone`/`teleport` component references and
/// the compile-time-constant `prefab` reference.
pub(super) fn is_reference_type(t: &Type) -> bool {
    matches!(t, Type::Ref(_) | Type::Zone | Type::Teleport | Type::PrefabRef)
}

// ---------- generic mod/chip call-site inference ----------
//
// Arg-driven inference at a call to a generic `mod`/`chip`: `mono::call_constraints`
// walks each (declared param type, inferred arg type) pair in lockstep and
// records an equality `Constraint` everywhere the param side names a
// `Type::Param`. `types::infer::solve` turns the collected constraints into a
// `Subst`; `mono::substitute` then replaces every `Type::Param` in the
// signature's output type with its solved binding. The
// `call_constraints`/`substitute`/`mask_for_param` helpers live in the shared
// `types::mono` module so the lowering-side monomorphizer reuses the
// exact same inference at each generic-mod inline site.

/// Operand type for operator-overload resolution: unwrap a `ref`, then collapse
/// a multi-output gate result (`Record`) to its PRIMARY (first) field. A gate
/// like `ParseInt`/`GetDamage` exposes `{ Value, Success }` / `{ Damage,
/// DamageLimit }`, whose first field is the value the call "is" — so `ParseInt(s)
/// == n` compares against that value, mirroring the record auto-unwrap the
/// coercion layer already does for assignments and call arguments.
pub(super) fn op_operand_type(t: &Type) -> Type {
    match unwrap_ref(t) {
        Type::Record(fields) if !fields.is_empty() => op_operand_type(&fields[0].1),
        other => other,
    }
}

/// Adapt a builtin/receiver `CallSpec` into the generic `CallSignature` shape
/// `sig::check_args` validates against — the bridge that lets both call forms
/// route through the one arg checker. Each `CallParam` maps to a `Param` whose
/// `ParamKind` mirrors exactly the per-param branch the arg checker takes (enum
/// config / composite config / scalar config / ordinary wire), keyed off the
/// gate PORT name so the config validators' schema lookups are unchanged.
pub(super) fn sig_of_callspec(spec: &crate::catalog::calls::CallSpec) -> CallSignature {
    let params = spec
        .params
        .iter()
        .map(|p| {
            let kind = if let Some(et) = call_param_config_enum(spec, p) {
                ParamKind::ConfigEnum(et)
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

/// Adapt a user mod/chip's `FnOrChipSig` into a `CallSignature` for
/// `sig::check_args` — the `type_user_symbol_call` analog of
/// `sig_of_callspec`. Every param is `ParamKind::Wire`, except a `const`
/// parameter (`name: const T`, or any parameter of a `const mod`) which
/// becomes `ParamKind::Const` (user mods have no config-menu params) and
/// non-optional (user mods have no default params either). `subst` — the
/// generic-inference result computed by the caller, `None` for a non-generic
/// call — is applied to each param's type first; a param whose
/// (possibly-substituted) type still carries a `Type::Param` is left alone
/// here too — `check_args`'s
/// own `type_has_param` guard skips it. `config_gate` is `None`: a named arg
/// that matches no declared param has no data-driven fallback to dispatch to
/// for a user call.
fn sig_of_fnchip(
    name: &str,
    sig: &FnOrChipSig,
    subst: Option<&crate::types::infer::Subst>,
) -> CallSignature {
    let params = sig
        .params
        .iter()
        .map(|p| {
            let ty = match subst {
                Some(s) => substitute(&p.ty, s),
                None => p.ty.clone(),
            };
            Param {
                name: p.name.clone(),
                ty,
                optional: false,
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
