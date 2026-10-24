//! The two kinds a scalar `*T` record field's binding can have, and the
//! boundaries where a record changes which kind its fields must be.
//!
//! A field bound to a scalar var, or to a `*T` parameter bound to one, names
//! that storage: a PLACE, which reads its var's value and passes its var's
//! `VarRef`. Any other binding (a ref-holding var, `&x`, a pin, a map read)
//! carries a ref on a wire: a VALUE, which reads as the ref itself. Typecheck
//! classifies every route the same way (`typecheck::let_binding::ref_values_at`):
//! a record parameter's fields are places, and a call's result and the record
//! an `if` or `match` chooses are values. A record argument, result or choice
//! built from the other kind is rebound here, so a field's binding is always
//! the kind typecheck typed it as.

use super::*;
use crate::intern::Sym;

/// Whether `b`, bound to a scalar `*T` field, names storage.
pub(super) fn binds_place(b: &Binding) -> bool {
    match b {
        Binding::Var(v) => v.storage == VarStorage::Var && !matches!(v.inner_type, Type::Ref(_)),
        Binding::DynRef(d) => d.storage == VarStorage::Var,
        _ => false,
    }
}

/// The ref a scalar `*T` field holds: a place's own ref, or the ref a value
/// carries.
pub(super) fn ref_port_of(ctx: &mut LowerCtx, b: &Binding, range: &SourceRange) -> Option<PortRef> {
    match b {
        Binding::Var(v) if binds_place(b) => Some(v.node_id.port(WirePort::VarRef)),
        Binding::DynRef(d) if d.storage == VarStorage::Var => Some(d.port),
        _ => access::binding_to_port(ctx, b, range),
    }
}

/// Which kind a record's scalar `*T` fields must have.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldKind {
    /// A record parameter's: a ref value binds as the place it points at, read
    /// and written through the ref.
    Place,
    /// A call's result: a place leaves as its ref.
    Value,
}

/// Rebind the scalar `*T` fields of `rec`, a record of type `typ`, to `kind`.
pub(super) fn rebind_ref_fields(
    ctx: &mut LowerCtx,
    typ: &crate::ast::TypeExpr,
    rec: &mut HashMap<Sym, Binding>,
    kind: FieldKind,
    range: &SourceRange,
) {
    if let Some(decl) = ctx.record_or_tuple_fields(typ) {
        rebind_fields(ctx, &decl, rec, kind, range);
    }
}

/// `rebind_ref_fields` over an explicit field list (a call's outputs).
pub(super) fn rebind_fields(
    ctx: &mut LowerCtx,
    decl: &[crate::ast::RecordTypeField],
    rec: &mut HashMap<Sym, Binding>,
    kind: FieldKind,
    range: &SourceRange,
) {
    for f in decl {
        let key = crate::intern::intern(&f.name);
        let Some(b) = rec.get(&key).cloned() else {
            continue;
        };
        if let Some(sub) = ctx.record_or_tuple_fields(&f.typ) {
            if let Binding::Record(mut inner) = b {
                rebind_fields(ctx, &sub, &mut inner, kind, range);
                rec.insert(key, Binding::Record(inner));
            }
            continue;
        }
        let storage = ctx.ref_storage(None, &f.typ);
        if storage != Some(VarStorage::Var) {
            continue;
        }
        let t = ctx.resolve_local_type(&f.typ);
        let Some((_, inner)) = context::container_binding(storage, &t) else {
            continue;
        };
        if let Some(rebound) = rebind_leaf(ctx, &b, kind, inner, range) {
            rec.insert(key, rebound);
        }
    }
}

/// `rebind_fields` for a record known by its resolved type: a record-valued
/// `if` or `match` branch, whose chosen value is a value like a call's result.
pub(super) fn rebind_typed(
    ctx: &mut LowerCtx,
    ty: &Type,
    rec: &mut HashMap<Sym, Binding>,
    kind: FieldKind,
    range: &SourceRange,
) {
    let fields: Vec<(String, Type)> = match unwrap_ref(ty) {
        Type::Record(fields) => fields,
        Type::Tuple(elems) => elems
            .into_iter()
            .enumerate()
            .map(|(i, t)| (i.to_string(), t))
            .collect(),
        _ => return,
    };
    for (name, ft) in fields {
        let key = crate::intern::intern(&name);
        let Some(b) = rec.get(&key).cloned() else {
            continue;
        };
        let rebound = match (&ft, b) {
            (Type::Record(_) | Type::Tuple(_), Binding::Record(mut inner)) => {
                rebind_typed(ctx, &ft, &mut inner, kind, range);
                Some(Binding::Record(inner))
            }
            (Type::Ref(inner), b) if ft.ref_kind() == Some(crate::ir::RefKind::Var) => {
                rebind_leaf(ctx, &b, kind, (**inner).clone(), range)
            }
            _ => None,
        };
        if let Some(rebound) = rebound {
            rec.insert(key, rebound);
        }
    }
}

/// The binding of `e`, the field read `obj.field`, as typecheck typed that
/// read: `*T` for a value, `T` for a place. The boundaries above give every
/// binding the kind typecheck does, so this rebinds nothing unless a route
/// was missed; the read then still means what its type says.
pub(super) fn as_typed(
    ctx: &mut LowerCtx,
    obj: &Expr,
    e: &Expr,
    b: Binding,
    range: &SourceRange,
) -> Binding {
    let Expr::FieldAccess { field, .. } = e else {
        return b;
    };
    let Type::Record(fields) = unwrap_ref(&ctx.type_of(obj)) else {
        return b;
    };
    let Some(Type::Ref(inner)) = fields
        .iter()
        .find(|(n, _)| n == field)
        .map(|(_, t)| t.clone())
    else {
        return b;
    };
    if inner.ref_kind().is_some() || matches!(*inner, Type::Record(_) | Type::Tuple(_)) {
        return b;
    }
    let kind = match ctx.type_of(e) {
        Type::Ref(_) => FieldKind::Value,
        t if t == *inner => FieldKind::Place,
        _ => return b,
    };
    debug_assert!(
        binds_place(&b) == (kind == FieldKind::Place)
            || !matches!(b, Binding::Var(_) | Binding::DynRef(_) | Binding::Local(_)),
        "`{field}` is bound as the other kind of `*T` field than its read is typed"
    );
    rebind_leaf(ctx, &b, kind, *inner, range).unwrap_or(b)
}

/// One scalar `*T` field's binding as `kind`, or `None` when it already is.
fn rebind_leaf(
    ctx: &mut LowerCtx,
    b: &Binding,
    kind: FieldKind,
    inner: Type,
    range: &SourceRange,
) -> Option<Binding> {
    if binds_place(b) == (kind == FieldKind::Place) {
        return None;
    }
    let port = match kind {
        FieldKind::Value => ref_port_of(ctx, b, range)?,
        FieldKind::Place => access::binding_to_port(ctx, b, range)?,
    };
    Some(match kind {
        FieldKind::Value => Binding::Local(LocalRecord { port }),
        FieldKind::Place => Binding::DynRef(DynRefRecord {
            port,
            storage: VarStorage::Var,
            inner_type: inner,
        }),
    })
}

/// `&r` for a record `r`: each field's ref, as `*` on a record distributes over
/// its fields. An array or map field is its own ref.
pub(super) fn refs_of(
    ctx: &mut LowerCtx,
    fields: &HashMap<Sym, Binding>,
    range: &SourceRange,
) -> HashMap<Sym, Binding> {
    let mut out = HashMap::default();
    for (k, b) in fields {
        let b = match b {
            Binding::Record(inner) => Binding::Record(refs_of(ctx, inner, range)),
            b if binds_place(b) => match ref_port_of(ctx, b, range) {
                Some(port) => Binding::Local(LocalRecord { port }),
                None => b.clone(),
            },
            b => b.clone(),
        };
        out.insert(*k, b);
    }
    out
}
