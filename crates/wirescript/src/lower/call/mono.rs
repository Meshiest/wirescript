//! Generic instantiation: read an argument's lowered type, build a mono
//! frame, and key it.

use super::*;

/// Type of `port` as declared on its node (searching this module and any
/// already-built chip children). Used to read an argument's ACTUAL lowered type.
pub(in crate::lower) fn arg_port_type(ctx: &LowerCtx, port: PortRef) -> Option<Type> {
    fn find(m: &Module, id: NodeId) -> Option<&crate::ir::Node> {
        m.nodes
            .get(&id)
            .or_else(|| m.chips.values().find_map(|c| find(c, id)))
    }
    let n = find(&ctx.builder.module, port.node_id)?;
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
pub(in crate::lower) fn lowered_arg_type(ctx: &LowerCtx, e: &Expr) -> Type {
    let recorded = || ctx.type_of(e);
    let binding = match e {
        Expr::Ident { name, .. } => ctx.scope.get(name),
        Expr::FieldAccess { .. } | Expr::TuplePick { .. } if !ctx.mono_stack.is_empty() => resolve_field_chain(ctx, e),
        _ => None,
    };
    binding.and_then(|b| binding_lowered_type(ctx, b, recorded)).unwrap_or_else(recorded)
}

/// The type a name's binding lowers to. A container binding records its
/// element type (an array) or its whole `Map(K, V)`, so an array is rebuilt as
/// `Array(elem)`: that is what a `T[]` param has to unify against.
///
/// The binding's type replaces `recorded` (typecheck's type, which in a
/// template build is the last combination checked) only when the two have the
/// same shape. A multi-output result bound by its first port (`ss.pop()`)
/// has that port's scalar type but a record's type, so a record `recorded`
/// is refined from the result's output ports instead.
///
/// A record binding has no type of its own, so inside a generic body its
/// fields refine `recorded`. Outside one `recorded` is already the monomorph
/// and is returned as is.
fn binding_lowered_type(ctx: &LowerCtx, b: &Binding, recorded: impl Fn() -> Type) -> Option<Type> {
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
        Binding::Record(fields) if !ctx.mono_stack.is_empty() => return Some(refine_record(ctx, &recorded(), fields)),
        _ => return None,
    };
    let recorded = recorded();
    if same_shape(&own, &recorded) {
        return Some(own);
    }
    if let Binding::Local(l) = b {
        return Some(refine_fields(&recorded, |name, _| {
            let port = crate::lower::access::resolve_output_field_port(ctx, l.port.node_id, name)?;
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
fn refine_record(
    ctx: &LowerCtx,
    template: &Type,
    fields: &HashMap<crate::intern::Sym, Binding>,
) -> Type {
    refine_fields(template, |name, ft| match fields.get(&crate::intern::intern(name))? {
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
    ctx: &LowerCtx,
    chip_decl: &ChipDecl,
    call: &SourceRange,
    positional_args: &[&Expr],
    type_args: &[TypeExpr],
) -> Option<MonoFrame> {
    let key = (call.file.clone(), call.start.offset, call.end.offset, ctx.resolution_subst.clone());
    let params: Vec<String> = chip_decl.type_params.iter().map(|tp| tp.name.clone()).collect();
    match ctx.resolutions.call_type_args.get(&key) {
        Some(args) => {
            let subst = params.iter().cloned().zip(args.iter().cloned()).collect();
            Some(MonoFrame::of(chip_decl, params, subst))
        }
        None if ctx.template_build => Some(inferred_frame(ctx, chip_decl, positional_args, type_args)),
        None => None,
    }
}

/// Whether typecheck checked `chip_decl`'s body under `body_subst`. A body
/// under the empty key is checked once with no instantiation to miss, and a
/// template build lowers generic bodies typecheck checked under other keys.
pub(super) fn instantiation_checked(ctx: &LowerCtx, chip_decl: &ChipDecl, body_subst: &Arc<[Type]>) -> bool {
    let r = &chip_decl.range;
    body_subst.is_empty()
        || ctx.template_build
        || ctx
            .resolutions
            .checked_bodies
            .contains(&(r.file.clone(), r.start.offset, r.end.offset, body_subst.clone()))
}

/// An internal error at `call` for an instantiation of `chip_decl` typecheck
/// never checked, whose types lowering has no record of. Typecheck checks
/// every instantiation a call it accepted demands, so this is a compiler bug.
pub(super) fn report_unchecked_instantiation(ctx: &mut LowerCtx, chip_decl: &ChipDecl, call: &SourceRange) -> PortRef {
    ctx.diagnostics.push(Diagnostic::error(
        crate::diagnostic::INTERNAL_ERROR,
        format!(
            "internal compiler error: this call instantiates `{}` with type arguments the type \
             checker never checked its body with, so the types inside it are unknown",
            chip_decl.name,
        ),
        call.clone(),
    ));
    crate::lower::expr::synthesise_unsupported_no_warn(ctx, call)
}

/// A template build's frame: the call's substitution rebuilt from the
/// arguments' lowered types (see [`lowered_arg_type`]) by the inference
/// typecheck runs at a call (`types::mono::infer_call_subst`), or from
/// explicit type arguments.
fn inferred_frame(
    ctx: &LowerCtx,
    chip_decl: &ChipDecl,
    positional_args: &[&Expr],
    type_args: &[TypeExpr],
) -> MonoFrame {
    let param_names: Vec<String> = chip_decl
        .type_params
        .iter()
        .map(|tp| tp.name.clone())
        .collect();
    // Explicit type arguments (`pick<int>(...)`): the caller pinned each type
    // param. Typecheck already validated arity + mask, so bind each `T_i` to its
    // resolved type arg directly — `resolve_local_type` monomorphizes a type
    // argument that itself references an outer `T` (`inner<T>(...)` inside
    // `outer<T>`'s body).
    if !type_args.is_empty() {
        let mut subst = crate::types::infer::Subst::new();
        for (tp, te) in chip_decl.type_params.iter().zip(type_args.iter()) {
            subst.insert(tp.name.clone(), ctx.resolve_local_type(te));
        }
        return MonoFrame::of(chip_decl, param_names, subst);
    }
    // Aliases resolve so a `Box<T>` param is the record it names and its `T`
    // unifies with the argument's field.
    let own_param = |name: &str, _: &SourceRange| param_names.iter().any(|p| p == name).then(|| Type::Param(name.to_string()));
    let mut param_types = Vec::new();
    let mut arg_types = Vec::new();
    for (i, param) in chip_decl.inputs.iter().enumerate() {
        let Some(arg_expr) = positional_args.get(i) else {
            continue;
        };
        param_types.push(ctx.declared_types_at(param.typ.range()).resolve_scoped(&param.typ, &own_param));
        arg_types.push(lowered_arg_type(ctx, arg_expr));
    }
    let masks: Vec<(String, Vec<Type>)> = chip_decl
        .type_params
        .iter()
        .map(|tp| {
            (
                tp.name.clone(),
                crate::types::mono::mask_for_param(tp.bound.as_ref(), &|te| ctx.declared_types_at(te.range()).resolve(te)),
            )
        })
        .collect();
    let subst = crate::types::mono::infer_call_subst(&param_types, &arg_types, &masks);
    MonoFrame::of(chip_decl, param_names, subst)
}


/// Canonical cache/template key for one generic-chip instantiation: the chip
/// name plus the concrete type each type param resolves to under this call's
/// substitution (`Boxed<int>`, `Boxed<vector>`). `{:?}` on a resolved `Type` is
/// a stable, unique-per-type string, so two instantiations collide iff they
/// pick the same concrete types — exactly when their emitted grids are
/// interchangeable and safe to dedup. A non-generic chip never calls this: its
/// key stays the bare name.
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
