//! Generic instantiation: read an argument's lowered type, build a mono
//! frame, and key it.

use super::*;

/// Type of `port` as declared on its node (searching this module and any
/// already-built chip children): an argument's actual lowered type.
pub(in crate::lower) fn arg_port_type(ctx: &LowerCtx<'_>, port: PortRef) -> Option<Type> {
    let n = ctx.find_node(port.node_id)?;
    let sym = port.port.sym();
    n.ports
        .find_output(sym)
        .or_else(|| n.ports.find_input(sym))
        .map(|p| p.ty.clone())
}

/// The type an argument lowers to here: a name's, or a bound record field's,
/// from its binding (see [`binding_lowered_type`]), anything else its type in
/// this instantiation. A template build (`compile_chip_template`) lowers a
/// generic body with no substitution and has no instantiation's types to
/// read, so its frames and container fallbacks type the bindings it built.
pub(in crate::lower) fn lowered_arg_type(ctx: &LowerCtx<'_>, e: &Expr) -> Type {
    let recorded = || ctx.type_of(e);
    let binding = match e {
        Expr::Ident { name, .. } => ctx.scope.get(name),
        Expr::FieldAccess { .. } | Expr::TuplePick { .. } if !ctx.mono_stack.is_empty() => {
            resolve_field_chain(ctx, e)
        }
        _ => None,
    };
    binding
        .and_then(|b| binding_lowered_type(ctx, b, recorded))
        .unwrap_or_else(recorded)
}

/// The type a name's binding lowers to (an array binding rebuilt as
/// `Array(elem)`, what a `T[]` param unifies against). It replaces `recorded`
/// (typecheck's type) only when the two have the same shape; a multi-output
/// result bound by its first port refines a record `recorded` from its output
/// ports instead. Inside a generic body a record binding's fields refine
/// `recorded`.
fn binding_lowered_type(
    ctx: &LowerCtx<'_>,
    b: &Binding,
    recorded: impl Fn() -> Type,
) -> Option<Type> {
    let storage_type = |storage: VarStorage, inner: &Type| match storage {
        VarStorage::Array => Type::Array(Box::new(inner.clone())),
        VarStorage::Var | VarStorage::Buffer | VarStorage::Map => inner.clone(),
    };
    let own = match b {
        Binding::Local(l) => arg_port_type(ctx, l.port)?,
        Binding::Var(v) => storage_type(v.storage, &v.inner_type),
        Binding::DynRef(d) => storage_type(d.storage, &d.inner_type),
        Binding::Input(i) => i.ty.clone(),
        Binding::Buffer(b) => b.ty.clone(),
        Binding::Record(fields) if !ctx.mono_stack.is_empty() => {
            return Some(refine_record(ctx, &recorded(), fields));
        }
        _ => return None,
    };
    let recorded = recorded();
    if same_shape(&own, &recorded) {
        return Some(own);
    }
    if let Binding::Local(l) = b {
        return Some(refine_fields(&recorded, |name, _| {
            let port = resolve_output_field_port(ctx, l.port.node_id, name)?;
            arg_port_type(ctx, port)
        }));
    }
    Some(recorded)
}

/// Whether `a` and `b` are the same kind of type: both records, both tuples,
/// both arrays, both maps, the same enum, or both scalars. A type param,
/// `any` or a union says nothing about shape and matches anything.
fn same_shape(a: &Type, b: &Type) -> bool {
    #[derive(PartialEq)]
    enum Shape<'t> {
        Record,
        Tuple,
        Array,
        Map,
        Enum(&'t str),
        Scalar,
    }
    fn shape(t: &Type) -> Option<Shape<'_>> {
        Some(match t {
            Type::Ref(inner) => return shape(inner),
            Type::Record(_) => Shape::Record,
            Type::Tuple(_) => Shape::Tuple,
            Type::Array(_) => Shape::Array,
            Type::Map(..) => Shape::Map,
            Type::Enum { name, .. } => Shape::Enum(name),
            Type::Param(_) | Type::Any | Type::Union(_) => return None,
            _ => Shape::Scalar,
        })
    }
    match (shape(a), shape(b)) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// `template` with each record field (or tuple element, by index) that
/// `field` can type replaced by that type. The rest keep their template type.
fn refine_fields(template: &Type, field: impl Fn(&str, &Type) -> Option<Type>) -> Type {
    match template {
        Type::Record(tf) => Type::Record(
            tf.iter()
                .map(|(name, ft)| (name.clone(), field(name, ft).unwrap_or_else(|| ft.clone())))
                .collect(),
        ),
        Type::Tuple(ts) => Type::Tuple(
            ts.iter()
                .enumerate()
                .map(|(i, ft)| field(&i.to_string(), ft).unwrap_or_else(|| ft.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `template` with each field a record binding holds replaced by the type
/// that field's binding lowers to. A field the binding cannot type (an enum
/// payload record, a record array's columns) keeps its template type.
fn refine_record(ctx: &LowerCtx<'_>, template: &Type, fields: &Fields) -> Type {
    refine_fields(template, |name, ft| match fields.get(&intern(name))? {
        Binding::Record(inner) => Some(refine_record(ctx, ft, inner)),
        other => binding_lowered_type(ctx, other, || ft.clone()),
    })
}

/// The [`MonoFrame`] `call` instantiates generic `chip_decl` with: the type
/// arguments typecheck recorded for it under the caller's substitution, so
/// the body's key is the one typecheck checked. `None` when it recorded
/// none, which it does only for a call it rejected. A template build lowers a
/// generic body with no substitution, so its calls have no record; it infers
/// them from the arguments' lowered types instead (see [`inferred_frame`]).
pub(super) fn mono_frame(
    ctx: &LowerCtx<'_>,
    chip_decl: &ChipDecl,
    call: &SourceRange,
    positional_args: &[&Expr],
    type_args: &[TypeExpr],
) -> Option<MonoFrame> {
    let key = (
        std::sync::Arc::clone(&call.file),
        call.start.offset,
        call.end.offset,
        std::sync::Arc::clone(&ctx.resolution_subst),
    );
    let params: Vec<String> = chip_decl
        .type_params
        .iter()
        .map(|tp| tp.name.clone())
        .collect();
    match ctx.resolutions.call_type_args.get(&key) {
        Some(args) => {
            let subst = params.iter().cloned().zip(args.iter().cloned()).collect();
            Some(MonoFrame::of(chip_decl, params, subst))
        }
        None if ctx.template_build => {
            Some(inferred_frame(ctx, chip_decl, positional_args, type_args))
        }
        None => None,
    }
}

/// Whether typecheck checked `chip_decl`'s body under `body_subst`. A body
/// under the empty key is checked once with no instantiation to miss, and a
/// template build lowers generic bodies typecheck checked under other keys.
pub(super) fn instantiation_checked(
    ctx: &LowerCtx<'_>,
    chip_decl: &ChipDecl,
    body_subst: &Arc<[Type]>,
) -> bool {
    let r = &chip_decl.range;
    body_subst.is_empty()
        || ctx.template_build
        || ctx.resolutions.checked_bodies.contains(&(
            std::sync::Arc::clone(&r.file),
            r.start.offset,
            r.end.offset,
            std::sync::Arc::clone(body_subst),
        ))
}

/// An internal error at `call` for an instantiation of `chip_decl` typecheck
/// never checked, whose types lowering has no record of. Typecheck checks
/// every instantiation a call it accepted demands, so this is a compiler bug.
pub(super) fn report_unchecked_instantiation(
    ctx: &mut LowerCtx<'_>,
    chip_decl: &ChipDecl,
    call: &SourceRange,
) -> PortRef {
    ctx.diagnostics.push(Diagnostic::error(
        crate::diagnostic::INTERNAL_ERROR,
        format!(
            "internal compiler error: this call instantiates `{}` with type arguments the type \
             checker never checked its body with, so the types inside it are unknown",
            chip_decl.name,
        ),
        call.clone(),
    ));
    synthesise_unsupported_no_warn(ctx, call)
}

/// A template build's frame: the call's substitution rebuilt from the
/// arguments' lowered types (see [`lowered_arg_type`]) by the inference
/// typecheck runs at a call (`types::mono::infer_call_subst`), or from
/// explicit type arguments.
fn inferred_frame(
    ctx: &LowerCtx<'_>,
    chip_decl: &ChipDecl,
    positional_args: &[&Expr],
    type_args: &[TypeExpr],
) -> MonoFrame {
    let param_names: Vec<String> = chip_decl
        .type_params
        .iter()
        .map(|tp| tp.name.clone())
        .collect();
    // Explicit type arguments (`pick<int>(...)`) bind each type param directly
    // (typecheck validated arity and mask). `resolve_local_type` resolves an
    // argument naming an outer `T` (`inner<T>(...)` inside `outer<T>`).
    if !type_args.is_empty() {
        let mut subst = crate::types::infer::Subst::new();
        for (tp, te) in chip_decl.type_params.iter().zip(type_args.iter()) {
            subst.insert(tp.name.clone(), ctx.resolve_local_type(te));
        }
        return MonoFrame::of(chip_decl, param_names, subst);
    }
    // Aliases resolve so a `Box<T>` param is the record it names and its `T`
    // unifies with the argument's field.
    let own_param = |name: &str, _: &SourceRange| {
        param_names
            .iter()
            .any(|p| p == name)
            .then(|| Type::Param(name.to_string()))
    };
    let mut param_types = Vec::new();
    let mut arg_types = Vec::new();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };
        param_types.push(
            ctx.declared_types_at(param.typ.range())
                .resolve_scoped(&param.typ, &own_param),
        );
        arg_types.push(lowered_arg_type(ctx, arg_expr));
    }
    let masks: Vec<(String, Vec<Type>)> = chip_decl
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                crate::types::mono::mask_for_param(tp.bound.as_ref(), &|te| {
                    ctx.declared_types_at(te.range()).resolve(te)
                }),
            )
        })
        .collect();
    let subst = crate::types::mono::infer_call_subst(&param_types, &arg_types, &masks);
    MonoFrame::of(chip_decl, param_names, subst)
}

/// Cache/template key for one generic-chip instantiation: the chip name plus
/// each type param's concrete type (`Boxed<int>`). `{:?}` on a resolved `Type`
/// is stable and unique per type, so two instantiations share a key exactly
/// when they pick the same concrete types.
pub(super) fn mono_key(chip_decl: &ChipDecl, frame: &MonoFrame) -> String {
    let args: Vec<String> = frame
        .params
        .iter()
        .map(|p| {
            format!(
                "{:?}",
                crate::types::mono::substitute(&Type::Param(p.clone()), &frame.subst)
            )
        })
        .collect();
    format!("{}<{}>", chip_decl.name, args.join(","))
}
