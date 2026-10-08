//! Lowering `value as T`: one `Convert` gate per scalar that changes type.
//!
//! Typecheck's `typecheck::convert::conversion` decided what the result is; a
//! scalar already of its target type passes through untouched, and a record or
//! tuple converts field by field. Every gate made here carries
//! `sym::AS_CONVERT`, so one whose result nothing reads is pruned again
//! (`(1, 2) as (bool, int)` costs one gate, or none if the result goes unread).

use super::*;
use crate::types::mono::unwrap_ref;

/// The scalar `value as T`, read where one value goes.
pub(super) fn lower_as(ctx: &mut LowerCtx, value: &Expr, e: &Expr) -> PortRef {
    let dst = ctx.type_of(e);
    // A constant whose conversion is measured is its result, with no gate.
    if let Some(lit) = crate::lower::predeclare::expr_to_literal_in(e, &ctx.const_lookup()) {
        return literal_node(ctx, e, dst, lit);
    }
    let src = unwrap_ref(&ctx.type_of(value));
    let port = lower_expr(ctx, value);
    convert_port(ctx, port, &src, &dst, e.range())
}

/// `port` (a `src`) as a `dst`: itself when the types already agree, else the
/// output of a new Convert gate it feeds.
pub(super) fn convert_port(ctx: &mut LowerCtx, port: PortRef, src: &Type, dst: &Type, range: &SourceRange) -> PortRef {
    if src == dst || matches!(dst, Type::Any | Type::Opaque) {
        return port;
    }
    let mut properties = HashMap::default();
    properties.insert(*sym::AS_CONVERT, Literal::Bool(true));
    let node = ctx.add_gate(AddNodeOpts {
        gate_class: gc::CONVERT,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![PortSpec::new(*sym::INPUT, src.clone())],
            outputs: vec![PortSpec::new(*sym::OUTPUT, dst.clone())],
        },
        properties,
        note: Some("as"),
        ..Default::default()
    });
    ctx.connect(port, node.port(WirePort::Input));
    node.port(WirePort::Output)
}

/// The fields of a record or tuple `value as T`, each converted to its target
/// field type. `None` when the value has no fields to convert.
pub(super) fn as_record_fields(
    ctx: &mut LowerCtx,
    value: &Expr,
    e: &Expr,
) -> Option<HashMap<crate::intern::Sym, Binding>> {
    let src = unwrap_ref(&ctx.type_of(value));
    let dst = ctx.type_of(e);
    let fields = crate::lower::stmt::value_record_fields(ctx, value)?;
    Some(convert_fields(ctx, fields, &src, &dst, e.range()))
}

/// Field name and type of each field of a record or tuple type, tuples by
/// index as their bindings are keyed.
fn field_types(t: &Type) -> Vec<(String, Type)> {
    match t {
        Type::Record(fs) => fs.clone(),
        Type::Tuple(ts) => ts.iter().enumerate().map(|(i, t)| (i.to_string(), t.clone())).collect(),
        _ => Vec::new(),
    }
}

fn convert_fields(
    ctx: &mut LowerCtx,
    mut fields: HashMap<crate::intern::Sym, Binding>,
    src: &Type,
    dst: &Type,
    range: &SourceRange,
) -> HashMap<crate::intern::Sym, Binding> {
    let src_fields = field_types(src);
    for (name, d) in field_types(dst) {
        let Some(s) = src_fields.iter().find(|(n, _)| *n == name).map(|(_, t)| unwrap_ref(t)) else {
            continue;
        };
        if s == d {
            continue;
        }
        let key = crate::intern::intern(&name);
        let Some(binding) = fields.get(&key).cloned() else { continue };
        let converted = match binding {
            Binding::Record(inner) => Binding::Record(convert_fields(ctx, inner, &s, &d, range)),
            other => match binding_to_port(ctx, &other, range) {
                Some(port) => Binding::Local(LocalRecord { port: convert_port(ctx, port, &s, &d, range) }),
                None => other,
            },
        };
        fields.insert(key, converted);
    }
    fields
}

/// Drop every Convert gate an `as` made whose output nothing reads, along
/// with the wire into it, until none is left (a dropped conversion can leave
/// the one feeding it unread).
pub(super) fn prune_unread_conversions(module: &mut Module) {
    loop {
        let mut read: HashSet<NodeId> = HashSet::default();
        collect_sources(module, &mut read);
        let mut dropped: HashSet<NodeId> = HashSet::default();
        drop_unread(module, &read, &mut dropped);
        if dropped.is_empty() {
            return;
        }
        drop_wires_into(module, &dropped);
    }
}

fn collect_sources(module: &Module, read: &mut HashSet<NodeId>) {
    read.extend(module.wires.iter().map(|w| w.source.node_id));
    read.extend(module.dynamic_labels.values().map(|p| p.node_id));
    read.extend(module.root_dynamic_label.iter().map(|p| p.node_id));
    for child in module.chips.values() {
        collect_sources(child, read);
    }
}

fn drop_unread(module: &mut Module, read: &HashSet<NodeId>, dropped: &mut HashSet<NodeId>) {
    module.nodes.retain(|id, n| {
        let unread = n.gate_class == gc::CONVERT && n.properties.contains_key(&*sym::AS_CONVERT) && !read.contains(id);
        if unread {
            dropped.insert(*id);
        }
        !unread
    });
    for child in module.chips.values_mut() {
        drop_unread(child, read, dropped);
    }
}

fn drop_wires_into(module: &mut Module, dropped: &HashSet<NodeId>) {
    module.wires.retain(|w| !dropped.contains(&w.target.node_id));
    for child in module.chips.values_mut() {
        drop_wires_into(child, dropped);
    }
}
