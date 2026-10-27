use super::*;
use crate::catalog::methods::{ArgKind, Lowering, MethodArg};
use crate::lower::ordered::lower_in_order;
use crate::typecheck::ContainerKind;

/// Flatten a projection chain into its base and the named segments applied to
/// it, so an `unsafe` access can pick out its variant and field. `.0` on a
/// positional payload parses as a `TuplePick`, hence both forms.
fn flatten_segments(e: &Expr) -> (&Expr, Vec<String>) {
    match e {
        Expr::FieldAccess { obj, field, .. } => {
            let (base, mut segs) = flatten_segments(obj);
            segs.push(field.clone());
            (base, segs)
        }
        Expr::TuplePick { obj, index, .. } => {
            let (base, mut segs) = flatten_segments(obj);
            segs.push(index.to_string());
            (base, segs)
        }
        _ => (e, Vec::new()),
    }
}

/// The binding behind `unsafe <value>.<Variant>.<field>...`: the enum's own
/// `__{Variant}_{field}` slot, with any further segments walked into a
/// record-typed payload. The assertion costs no gate and touches only the slot.
pub(super) fn resolve_unsafe_slot<'a>(ctx: &'a LowerCtx<'_>, inner: &Expr) -> Option<&'a Binding> {
    let (base, segs) = flatten_segments(inner);
    let (variant, field) = (segs.first()?, segs.get(1)?);
    let Binding::Record(fields) = resolve_field_chain(ctx, base)? else {
        return None;
    };
    let mut binding = fields.get(&intern(&slot_name(variant, field)))?;
    for seg in &segs[2..] {
        let Binding::Record(inner_fields) = binding else {
            return None;
        };
        binding = inner_fields.get(&intern(seg))?;
    }
    Some(binding)
}

/// Walk a chain of `Ident` / `FieldAccess` nodes, resolving through
/// `Binding::Record` maps (and one `Binding::Namespace` hop). Returns `None`
/// when the chain is not entirely record-based (a non-record root, a missing field).
pub(super) fn resolve_field_chain<'a>(ctx: &'a LowerCtx<'_>, expr: &Expr) -> Option<&'a Binding> {
    match expr {
        Expr::Unsafe { inner, .. } => resolve_unsafe_slot(ctx, inner),
        Expr::Ident { name, .. } => ctx.scope.get(name),
        Expr::FieldAccess { obj, field, .. } => {
            let parent = resolve_field_chain(ctx, obj)?;
            match parent {
                Binding::Record(fields) => fields
                    .get(&intern(field))
                    // `.Value`/`.prev` opens the variable: a var needing more
                    // than one storage gate binds here, never as a single-port
                    // `Binding::Var`. After the field lookup, so a real `Value`
                    // field (`a.pop()`) still wins.
                    .or_else(|| {
                        (crate::catalog::is_var_pseudo_field(field)
                            && record_is_variable_backed(fields))
                        .then_some(parent)
                    }),
                // `ns.member` from `import * as ns` resolves through the
                // namespace's own members, so two namespaces exporting the same
                // name do not collide in the bare scope (which keeps only the last).
                // The bare-scope fallback keeps an uncaptured member off `_Unsupported`.
                Binding::Namespace(members) => members.get(field).or_else(|| ctx.scope.get(field)),
                _ => None,
            }
        }
        Expr::TuplePick { obj, index, .. } => {
            let parent = resolve_field_chain(ctx, obj)?;
            if let Binding::Record(fields) = parent {
                fields.get(&intern(&index.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A `Binding` that names an ARRAY container -> its ref port and element type:
/// a `var` array (`ArrayVarRef`), an `in`/`out X: T[]` (the rerouter's
/// `RER_Output`), an array-valued `let` (its wire), or a dynamic ref. A record
/// field holding one of these resolves the same way.
///
/// `obj_ty` types the port-only `Local` binding; it is `Any` on an assignment
/// target, so a `Local` resolves to `None` there.
pub(super) fn binding_array_ref(
    ctx: &LowerCtx<'_>,
    binding: &Binding,
    obj_ty: &Type,
) -> Option<(PortRef, Type)> {
    fn elem(t: &Type) -> Option<Type> {
        match unwrap_ref(t) {
            Type::Array(e) => Some(e.as_ref().clone()),
            _ => None,
        }
    }
    match binding {
        Binding::Var(var_rec) if var_rec.storage == VarStorage::Array => Some((
            var_rec.node_id.port(WirePort::ArrayVarRef),
            var_rec.inner_type.clone(),
        )),
        Binding::Input(inp) => elem(&inp.ty).map(|e| (inp.node_id.port(WirePort::RerOutput), e)),
        Binding::Output(out) => elem(&out.ty).map(|e| (out.node_id.port(WirePort::RerOutput), e)),
        Binding::Local(local) if !is_placeholder_port(ctx, local.port) => {
            elem(obj_ty).map(|e| (local.port, e))
        }
        Binding::DynRef(d) if d.storage == VarStorage::Array => {
            Some((d.port, d.inner_type.clone()))
        }
        _ => None,
    }
}

/// Does this port come off an `_Unsupported` placeholder? A container ref taken
/// from one would wire a real gate to a dead source without a warning; declining
/// leaves the caller on its own placeholder path, which warns.
fn is_placeholder_port(ctx: &LowerCtx<'_>, port: PortRef) -> bool {
    ctx.builder
        .module
        .nodes
        .get(&port.node_id)
        .is_some_and(|n| n.gate_class == gc::UNSUPPORTED)
}

/// The map analogue of [`binding_array_ref`]: a `Binding` that names a MAP
/// container -> its ref port and whole `Type::Map(K, V)`.
pub(super) fn binding_map_ref(
    ctx: &LowerCtx<'_>,
    binding: &Binding,
    obj_ty: &Type,
) -> Option<(PortRef, Type)> {
    fn map_ty(t: &Type) -> Option<Type> {
        match unwrap_ref(t) {
            m @ Type::Map(_, _) => Some(m),
            _ => None,
        }
    }
    match binding {
        Binding::Var(var_rec) if var_rec.storage == VarStorage::Map => Some((
            var_rec.node_id.port(WirePort::MapVarRef),
            var_rec.inner_type.clone(),
        )),
        Binding::Input(inp) => map_ty(&inp.ty).map(|t| (inp.node_id.port(WirePort::RerOutput), t)),
        Binding::Output(out) => map_ty(&out.ty).map(|t| (out.node_id.port(WirePort::RerOutput), t)),
        Binding::Local(local) if !is_placeholder_port(ctx, local.port) => {
            map_ty(obj_ty).map(|t| (local.port, t))
        }
        Binding::DynRef(d) if d.storage == VarStorage::Map => Some((d.port, d.inner_type.clone())),
        _ => None,
    }
}

/// Whether this record IS variable storage (every leaf a var/buffer gate)
/// rather than a compile-time record VALUE whose fields resolve to locals.
fn record_is_variable_backed(fields: &Fields) -> bool {
    !fields.is_empty()
        && fields.values().all(|b| match b {
            Binding::Var(_) | Binding::Buffer(_) => true,
            Binding::Record(inner) => record_is_variable_backed(inner),
            _ => false,
        })
}

/// Convert a resolved `Binding` into a `PortRef` for use in expressions.
/// For `Var` bindings in exec context, emits a `Var_Get` node.
pub(super) fn binding_to_port(
    ctx: &mut LowerCtx<'_>,
    binding: &Binding,
    range: &SourceRange,
) -> Option<PortRef> {
    match binding {
        Binding::Local(local) => Some(local.port),
        Binding::DynRef(d) => Some(read_dyn_ref(ctx, d, range)),
        Binding::EventParam(p) => Some(*p),
        Binding::Buffer(buf) => Some(buf.node_id.port(WirePort::Output)),
        Binding::Input(inp) => Some(inp.node_id.port(WirePort::RerOutput)),
        Binding::Var(var_rec) => {
            // An output of a pure mod call whose writes could not run.
            if let Some((name, call_range, call)) =
                ctx.unsequenced_reads.get(&var_rec.node_id).cloned()
            {
                ctx.unsequenced_reads.retain(|_, v| v.2 != call);
                report_unsequenced_output_write(ctx, &name, &call_range);
                return Some(synthesise_unsupported_no_warn(ctx, range));
            }
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
                let props = var_get_seed_props(&var_rec.inner_type);
                let get_id = chain_var_get(ctx, var_rec, exec, range, props, Some("get rec_field"));
                return Some(get_id.port(WirePort::Value));
            }
            Some(var_rec.node_id.port(WirePort::Value))
        }
        // Reading an output's value (a namespaced `L.count`) sources its
        // rerouter. Writes go through `lookup_output`/`emit`, never here.
        Binding::Output(out) => Some(out.node_id.port(WirePort::RerOutput)),
        Binding::Record(_) | Binding::Chip(_) | Binding::Namespace(_) => None,
    }
}

/// The caller-side field->port record for a call that lowered to a MULTI-output
/// chip instance. Pins are matched positionally against the signature's declared
/// field order, the order `predeclare` created them in; a record- or tuple-typed
/// output takes one pin per leaf (`record_output_pins`) as a nested record.
/// `pending_inline_record` and `resolve_output_field_port` cannot see these pins.
pub(super) fn multi_output_chip_record(
    ctx: &LowerCtx<'_>,
    call_port: PortRef,
    fields: &[(String, Type)],
) -> Option<Fields> {
    let child = ctx
        .builder
        .module
        .chips
        .values()
        .find(|child| child.outputs.contains(&call_port.node_id))?;
    fn take_pins(fields: &[(String, Type)], outputs: &[NodeId], idx: &mut usize) -> Fields {
        let mut out = HashMap::default();
        for (name, ty) in fields {
            let sub = match unwrap_ref(ty) {
                Type::Record(f) => Some(f),
                Type::Tuple(elems) => Some(
                    elems
                        .into_iter()
                        .enumerate()
                        .map(|(i, t)| (i.to_string(), t))
                        .collect(),
                ),
                _ => None,
            };
            if let Some(sub) = sub {
                out.insert(intern(name), Binding::Record(take_pins(&sub, outputs, idx)));
                continue;
            }
            let Some(&out_id) = outputs.get(*idx) else {
                break;
            };
            *idx += 1;
            out.insert(
                intern(name),
                Binding::Local(LocalRecord {
                    port: out_id.port(WirePort::RerOutput),
                }),
            );
        }
        out
    }
    let outputs = child.outputs.clone();
    let mut idx = 0usize;
    Some(take_pins(fields, &outputs, &mut idx))
}

/// Map a short field name (`.Forward`, `.Jump`) to the full gate port name it
/// stands for. `InputSplitter` exposes a few arbitrarily-named ports whose
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
    ctx: &LowerCtx<'_>,
    node_id: NodeId,
    field: &str,
) -> Option<PortRef> {
    let aliased = alias_output_field(field);
    let node = ctx.builder.module.nodes.get(&node_id)?;
    let pname = node.ports.outputs.iter().find_map(|p| {
        let pname = resolve(p.name);
        // Swizzle fields match a sibling port case-insensitively, so `.x` on a
        // multi-output result like `v.SplitVec()` reads its existing `X` port
        // rather than re-splitting the first field.
        (pname == aliased
            || crate::catalog::arrays::field_name_ref(pname) == field
            || (is_swizzle_field(field) && pname.eq_ignore_ascii_case(field)))
        .then_some(pname)
    })?;
    Some(port_ref(node_id, pname))
}

/// The result port a subscript's field names (`arr[i].value`,
/// `arr[i].bOutOfBounds`), or `None` for a field that names neither.
///
/// Matched case-insensitively with the `b` prefix optional, as typecheck
/// accepts every capitalisation. A spelling missed here type-checks, then
/// lowers to an unspawned `_Unsupported`, silently deleting the read.
fn index_result_port(field: &str) -> Option<WirePort> {
    if field.eq_ignore_ascii_case("value") {
        Some(WirePort::Value)
    } else if field.eq_ignore_ascii_case("bOutOfBounds")
        || field.eq_ignore_ascii_case("OutOfBounds")
    {
        Some(WirePort::BOutOfBounds)
    } else if field.eq_ignore_ascii_case("bFound") || field.eq_ignore_ascii_case("Found") {
        Some(WirePort::BFound)
    } else {
        None
    }
}

/// A vector, color, rotator or quat component name
/// ([`crate::catalog::value_fields`]). These name no gate port; they desugar
/// to a Split gate, so access through a local must fall through to that logic.
fn is_swizzle_field(field: &str) -> bool {
    crate::catalog::value_fields::is_field_name(field)
}

/// A scalar can never be swizzled: a chip output named `x` binds its result to
/// a local, and splitting that `int` would read a garbage component. Only known
/// scalars skip the split; vector, color and unknown/`any` types still split.
fn is_known_scalar(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Exec
    )
}

/// The gate that splits a value into its components.
#[derive(Clone, Copy)]
enum SplitKind {
    Quaternion,
    Rotation,
    Vector,
    Color,
}

impl SplitKind {
    fn gate_class(self) -> &'static str {
        match self {
            Self::Quaternion => gc::SPLIT_QUATERNION,
            Self::Rotation => gc::SPLIT_ROTATION,
            Self::Vector => gc::SPLIT_VECTOR,
            Self::Color => gc::SPLIT_COLOR,
        }
    }

    fn input_type(self) -> Type {
        match self {
            Self::Quaternion => Type::Quat,
            Self::Rotation => Type::Rotator,
            Self::Vector => Type::Vector,
            Self::Color => Type::Color,
        }
    }

    /// The gate's float output names, in port order.
    fn outputs(self) -> &'static [&'static str] {
        match self {
            Self::Quaternion => &["X", "Y", "Z", "W"],
            Self::Rotation => &["Pitch", "Yaw", "Roll"],
            Self::Vector => &["X", "Y", "Z"],
            Self::Color => &["R", "G", "B", "A"],
        }
    }
}

/// Read one component, `out_name`, of `obj_port` through a Split gate of `kind`.
/// A quaternion's X/Y/Z/W are addressed only by splitting it, so `q.x` costs one
/// gate; repeated components of one value share it through CSE.
fn split_component(
    ctx: &mut LowerCtx<'_>,
    obj_port: PortRef,
    kind: SplitKind,
    out_name: &str,
    range: &SourceRange,
) -> PortRef {
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: kind.gate_class(),
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![PortSpec::new(*sym::INPUT, kind.input_type())],
            outputs: kind
                .outputs()
                .iter()
                .map(|n| PortSpec::new(intern_static(n), Type::Float))
                .collect(),
        },
        ..Default::default()
    });
    ctx.connect(obj_port, node_id.port(WirePort::Input));
    port_ref(node_id, out_name)
}

/// Whether `choice`, an `if` or `match` expression whose value is a record,
/// tuple or enum, runs on an exec chain with an arm (one of `arms`) that does
/// more than give its value: WS097, which a field read of it reports in its
/// own words.
fn acting_aggregate_choice<'a>(
    ctx: &LowerCtx<'_>,
    choice: &Expr,
    arms: impl IntoIterator<Item = &'a Expr>,
) -> bool {
    ctx.current_exec.is_some()
        && matches!(
            unwrap_ref(&ctx.type_of(choice)),
            Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }
        )
        && arms.into_iter().any(|x| has_effects(ctx, x))
}

/// A field access on an `if` or `match` expression distributes over the
/// branches: `(if c then a else b).f` becomes `if c then a.f else b.f`, since an
/// aggregate branch carries no single wire for the field to read. `None` for any
/// other object, and for a `match` with a block-bodied arm (no value, left to
/// the statement form).
fn distribute_field_over_choice(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    field: &str,
    range: &SourceRange,
) -> Option<PortRef> {
    let field_of = |value: &Expr| Expr::FieldAccess {
        obj: Box::new(value.clone()),
        field: field.to_string(),
        range: range.clone(),
    };
    match obj {
        Expr::IfExpr {
            cond,
            then_branch,
            else_branch,
            range: if_range,
        } => {
            if acting_aggregate_choice(ctx, obj, [then_branch.as_ref(), else_branch.as_ref()]) {
                report_effectful_aggregate_field_read(ctx, field, range);
                return Some(synthesise_unsupported_no_warn(ctx, range));
            }
            let distributed = Expr::IfExpr {
                cond: cond.clone(),
                then_branch: Box::new(field_of(then_branch)),
                else_branch: Box::new(field_of(else_branch)),
                range: if_range.clone(),
            };
            Some(lower_expr(ctx, &distributed))
        }
        // `(match s { A => p, B => q }).f` becomes `match s { A => p.f, B => q.f }`,
        // one Select for the field.
        Expr::MatchExpr {
            scrutinee,
            arms,
            range: m_range,
        } if arms.iter().all(|a| matches!(a.body, MatchBody::Expr(_))) => {
            let values = arms.iter().filter_map(|a| match &a.body {
                MatchBody::Expr(x) => Some(x),
                MatchBody::Block(_) => None,
            });
            if acting_aggregate_choice(ctx, obj, values) {
                report_effectful_aggregate_field_read(ctx, field, range);
                return Some(synthesise_unsupported_no_warn(ctx, range));
            }
            let distributed = Expr::MatchExpr {
                scrutinee: scrutinee.clone(),
                arms: arms
                    .iter()
                    .map(|a| {
                        let MatchBody::Expr(body) = &a.body else {
                            unreachable!("guarded above")
                        };
                        MatchArm {
                            pattern: a.pattern.clone(),
                            guard: a.guard.clone(),
                            body: MatchBody::Expr(field_of(body)),
                            range: a.range.clone(),
                        }
                    })
                    .collect(),
                range: m_range.clone(),
            };
            Some(lower_expr(ctx, &distributed))
        }
        _ => None,
    }
}

/// A field of a record output read straight off the call (`f(c).pp.x`): walk the
/// call's own record, as `let t = f(c)` then `t.pp.x` does. `None` when `obj` is
/// not a field path rooted at a call that carries it.
fn lower_call_record_field(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    field: &str,
    range: &SourceRange,
) -> Option<PortRef> {
    let Expr::FieldAccess {
        obj: base,
        field: first,
        ..
    } = obj
    else {
        return None;
    };
    let mut root: &Expr = base;
    let mut path = vec![first.clone()];
    while let Expr::FieldAccess {
        obj: o, field: f, ..
    } = root
    {
        path.insert(0, f.clone());
        root = o;
    }
    let decl = called_chip_decl(ctx, root);
    let by_output = decl
        .as_ref()
        .is_some_and(|d| d.outputs.iter().any(|o| o.name == path[0]));
    // A field of a call's one record output, reached through a nested
    // record field (`f(p).inner.y`).
    let by_field = !by_output
        && decl.as_ref().is_some_and(|d| d.outputs.len() == 1)
        && matches!(unwrap_ref(&ctx.type_of(obj)), Type::Record(_));
    if !(by_output || by_field) {
        return None;
    }
    ctx.pending_inline_record = None;
    let port = lower_expr(ctx, root);
    let mut rec =
        ctx.pending_inline_record
            .take()
            .or_else(|| match unwrap_ref(&ctx.type_of(root)) {
                Type::Record(fields) => multi_output_chip_record(ctx, port, &fields),
                _ => None,
            });
    if by_field {
        rec = rec.map(|r| unwrap_call_record(ctx, root, r));
    }
    for step in &path {
        rec = match rec.as_ref().and_then(|r| r.get(&intern(step))) {
            Some(Binding::Record(inner)) => Some(inner.clone()),
            _ => None,
        };
    }
    if let Some(binding) = rec.as_ref().and_then(|r| r.get(&intern(field))).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return Some(port);
    }
    ctx.error(
        "WS072",
        format!("`{field}` has no value to read here: the call's output does not carry it"),
        range,
    );
    Some(synthesise_unsupported_no_warn(ctx, range))
}

/// How a field read on a call object resolved.
enum CallField {
    /// The read is answered; this is its port.
    Done(PortRef),
    /// Nothing named the field: the lowered call's primary port, for a
    /// component split to read.
    Lowered(PortRef),
}

/// A call object's field may name an output its own gate carries
/// (`v.SplitVec().y`); resolve it before a Split would split the call's primary
/// output again.
fn lower_call_field(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    field: &str,
    obj_ty: &Type,
    e: &Expr,
    range: &SourceRange,
) -> CallField {
    // Where the caller's chain stands before the call runs. `.exec` names
    // the call's own exec output and puts the chain back here, so the
    // statement after it takes the same source the call did: one exec
    // output driving two exec inputs runs both, which is the fork.
    let entry_exec = ctx.current_exec;
    // A named output of the call is read from its record below, so the
    // call need not also read its first output as a single value.
    let names_output =
        called_chip_decl(ctx, obj).is_some_and(|d| d.outputs.iter().any(|o| o.name == field));
    let saved_record_call = ctx.record_result_call.clone();
    if names_output {
        ctx.record_result_call = Some(obj.range().clone());
    }
    // Only this call's own record is read below, never one an earlier
    // call left (an argument read as one value keeps its stash).
    ctx.pending_inline_record = None;
    let obj_port = lower_expr(ctx, obj);
    ctx.record_result_call = saved_record_call;
    if field == "exec"
        && let Some(done) = ctx.current_exec
        && Some(done) != entry_exec
    {
        // A multi-output mod stashed its record; nothing here reads it, and
        // leaving it would hand the stash to an unrelated later field access.
        ctx.pending_inline_record.take();
        ctx.current_exec = entry_exec;
        return CallField::Done(done);
    }
    // An inline mod stashes its multi-output record (field -> binding)
    // rather than exposing ports; project the field from it, as
    // `let r = f()` then `r.field` does. Gate calls resolve ports by name.
    if let Some(record) = ctx.pending_inline_record.take()
        && let Some(binding) = record.get(&intern(field))
        // A payload-free enum field is its tag.
        && let binding = match binding {
            Binding::Record(rec) => enum_tag(ctx, rec, &ctx.type_of(e))
                .cloned()
                .or_else(|| held_output(ctx, rec, e.range(), OutputPick::First))
                .unwrap_or_else(|| binding.clone()),
            _ => binding.clone(),
        }
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return CallField::Done(port);
    }
    // An event call's field is the data output it names
    // (`CharacterSpawned().character` reads the `Character` port).
    if let Expr::Call { callee, .. } = obj
        && let Expr::Ident { name, .. } = callee.as_ref()
        && let Some(evt) = crate::catalog::events::find_event(name)
        && ctx
            .builder
            .module
            .nodes
            .get(&obj_port.node_id)
            .is_some_and(|n| n.gate_class == evt.gate_class)
        && let Some(data) = evt.data.iter().find(|d| d.name == field)
    {
        return CallField::Done(port_ref(obj_port.node_id, data.port));
    }
    if let Some(port) = resolve_output_field_port(ctx, obj_port.node_id, field) {
        return CallField::Done(port);
    }
    // A multi-output chip (`-> (a: ..., b: ...)`) stashes no inline record and
    // its outputs are separate rerouter nodes, so neither path above sees
    // them.
    if let Type::Record(rec_fields) = obj_ty
        && let Some(record) = multi_output_chip_record(ctx, obj_port, rec_fields)
        && let Some(binding) = record.get(&intern(field)).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return CallField::Done(port);
    }
    // A named output of a multi-output result that none of the above could
    // find has no wire; the component fallbacks would split the
    // call's primary output (possibly its exec) as a vector or color.
    if let Some(decl) = called_chip_decl(ctx, obj)
        && decl.outputs.iter().any(|o| o.name == field)
    {
        // The only output IS the call's value (`f(c).r` on `-> (r: int)`),
        // unless it is a record, which the stash above already served.
        if decl.outputs.len() == 1 && ctx.record_or_tuple_fields(&decl.outputs[0].typ).is_none() {
            return CallField::Done(obj_port);
        }
        ctx.error(
            "WS072",
            format!(
                "output `{field}` of this call has no value to read: nothing in the \
                 callee sets it (with `return` or `emit {field} = ...`) where this \
                 call can see it"
            ),
            range,
        );
        return CallField::Done(synthesise_unsupported_no_warn(ctx, range));
    }
    // A call that took no step on an exec chain (an inline `mod` whose
    // body is all values) has no completion for `.exec` to name.
    if field == "exec" {
        ctx.error(
            "WS043",
            "this call exposes no exec output for `.exec` to name: it runs no exec step. \
             `.exec` names the completion of a `fn` call, a `chip` call, or a `mod` whose \
             body runs exec statements"
                .to_string(),
            range,
        );
        return CallField::Done(synthesise_unsupported_no_warn(ctx, range));
    }
    CallField::Lowered(obj_port)
}

/// Read a vector, color, rotator or quaternion component, or a subscript's
/// result field, off `obj`. The field name picks the Split gate; the type is
/// consulted only to choose between vector and quaternion for `.x`/`.y`/`.z`,
/// since it may be unrecorded. `call_port` is `obj` already lowered.
fn lower_component_split(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    field: &str,
    obj_ty: &Type,
    call_port: Option<PortRef>,
    e: &Expr,
    range: &SourceRange,
) -> PortRef {
    let (kind, out_name) = match field {
        // A quaternion's X/Y/Z share the vector's names; an unknown type keeps
        // the vector reading.
        "x" | "X" | "y" | "Y" | "z" | "Z" if matches!(obj_ty, Type::Quat) => {
            (SplitKind::Quaternion, field[..1].to_uppercase())
        }
        "w" | "W" => (SplitKind::Quaternion, "W".to_string()),
        "pitch" | "Pitch" | "yaw" | "Yaw" | "roll" | "Roll" => {
            let mut out_name = field.to_string();
            out_name[..1].make_ascii_uppercase();
            (SplitKind::Rotation, out_name)
        }
        "x" | "X" | "y" | "Y" | "z" | "Z" => (SplitKind::Vector, field[..1].to_uppercase()),
        "r" | "R" | "g" | "G" | "b" | "B" | "a" | "A" => {
            (SplitKind::Color, field[..1].to_uppercase())
        }
        // Array index result fields: arr[i].value / arr[i].bOutOfBounds
        f if index_result_port(f).is_some() => {
            let port_id = index_result_port(f).expect("guarded by the arm");
            let obj_port = call_port.unwrap_or_else(|| lower_expr(ctx, obj));
            // The name alone does not prove the gate carries the port (`x.Value`
            // on any call result reaches here), and emit silently drops a wire
            // to an undeclared port.
            return match resolve_output_field_port(ctx, obj_port.node_id, port_id.as_str()) {
                Some(port) => port,
                None => synthesise_unsupported(ctx, e),
            };
        }
        // A field naming neither an inline-record entry nor a sibling output
        // port (both resolved earlier, for a call object) has nothing to read:
        // it degrades to an `_Unsupported` placeholder.
        _ => return synthesise_unsupported(ctx, e),
    };
    let obj_port = call_port.unwrap_or_else(|| lower_expr(ctx, obj));
    split_component(ctx, obj_port, kind, &out_name, range)
}

pub(super) fn lower_field_access(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    field: &str,
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    if let Some(port) = props::lower_environment_read(ctx, e) {
        return port;
    }
    if let Some(port) = props::lower_component_read(ctx, obj, field, e) {
        return port;
    }
    // A call's one enum output, named (`f(c).r` on `-> (r: Hue)`), is the
    // call's own value, as a scalar output is: read it, or a field after it,
    // off the call.
    if let Some(call) = named_enum_output_call(ctx, e) {
        return lower_expr(ctx, call);
    }
    if let Some(call) = named_enum_output_call(ctx, obj) {
        let on_call = Expr::FieldAccess {
            obj: Box::new(call.clone()),
            field: field.to_string(),
            range: range.clone(),
        };
        return lower_field_access(ctx, call, field, range, &on_call);
    }
    if let Some(port) = distribute_field_over_choice(ctx, obj, field, range) {
        return port;
    }
    // `<value>.Discriminant` projects to the integer discriminant (see
    // `LowerCtx::enum_defs`). Checked ahead of the record-field resolution,
    // which knows only the `__disc` sub-binding name. A variant path bakes a
    // literal; a stored enum value reads its `__disc` sub-binding.
    if field == DISCRIMINANT_FIELD
        && let Some(port) = lower_discriminant(ctx, obj, range)
    {
        return port;
    }
    // Resolve the whole chain `e` (possibly nested, `a.b.c`) through record
    // bindings first.
    if let Some(binding) = resolve_field_chain(ctx, e).cloned()
        && let binding = ref_fields::as_typed(ctx, obj, e, binding, range)
        // A payload-free enum field is its tag.
        && let binding = match &binding {
            Binding::Record(rec) => enum_tag(ctx, rec, &ctx.type_of(e))
                .cloned()
                .or_else(|| held_output(ctx, rec, e.range(), OutputPick::First))
                .unwrap_or(binding),
            _ => binding,
        }
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }
    // A nested record has no single port, so it falls through.

    // A field read on a record literal (`({...}).field`), which
    // `resolve_field_chain` cannot walk. Read before the swizzle fallback
    // claims `.x`/`.y`.
    if matches!(obj, Expr::RecordLit { .. })
        && let Some(fields) = value_record_fields(ctx, obj)
        && let Some(binding) = fields.get(&intern(field)).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }

    // `pts[i].f1.f2...`: a (possibly nested) field of a record array's element,
    // read from its parallel array. Must run before the swizzle fallback, which
    // would claim a record field named `.x` or `.a` as a component.
    if let Some(port) = lower_record_field_path(ctx, Columns::Array, e, range) {
        return port;
    }
    // `m[k].f1.f2...` / `m.get(k).f1.f2...`: the same for a record map's value.
    if let Some(port) = lower_record_field_path(ctx, Columns::Map, e, range) {
        return port;
    }

    // `rec.field.Value` / `.prev`: the explicit read of a var-backed record
    // field, resolved through the chain as the `Ident` arm below reads `x.Value`.
    if (field == "Value" || field == "prev")
        && matches!(obj, Expr::FieldAccess { .. } | Expr::TuplePick { .. })
        && let Some(binding) = resolve_field_chain(ctx, obj).cloned()
        && let Some(port) = binding_to_port(ctx, &binding, range)
    {
        return port;
    }

    if let Expr::Ident { name, .. } = obj {
        if (field == "Value" || field == "prev")
            && let Some(var_rec) = ctx.lookup_var(name).cloned()
        {
            return var_rec.node_id.port(WirePort::Value);
        }
        // Gate output port access: `input.Forward` resolves to the named
        // port on the gate node the local refers to.
        if let Some(local) = ctx.lookup_local(name).cloned() {
            if let Some(port) = resolve_output_field_port(ctx, local.port.node_id, field) {
                return port;
            }
            // A component (`v.x`, `c.r`) falls through to the Split logic
            // below, unless the local is a known scalar (a chip output named `y`),
            // which is the single-output result itself.
            if !is_swizzle_field(field) || is_known_scalar(&ctx.type_of(obj)) {
                return local.port;
            }
        }
    }
    // `.exec` names the exec output of an exec value (identity) or of a
    // data-carrying event, typed as a record with the exec first. Such an event
    // lowers with its exec as the primary output, so lowering the object yields it.
    if field == "exec" {
        let ot = ctx.type_of(obj);
        if matches!(ot, Type::Exec)
            || matches!(&ot, Type::Record(fs) if matches!(fs.first(), Some((_, Type::Exec))))
        {
            return lower_expr(ctx, obj);
        }
    }

    if let Some(port) = lower_call_record_field(ctx, obj, field, range) {
        return port;
    }
    // Read the object's type before lowering it: lowering writes the same
    // range-keyed map, so a nested expression can overwrite this entry.
    let obj_ty = ctx.type_of(obj);

    let call_port = if let Expr::Call { .. } = obj {
        match lower_call_field(ctx, obj, field, &obj_ty, e, range) {
            CallField::Done(port) => return port,
            CallField::Lowered(port) => Some(port),
        }
    } else {
        None
    };
    lower_component_split(ctx, obj, field, &obj_ty, call_port, e, range)
}

/// `<obj>.Discriminant` for a variant path or an enum value. `None` for
/// anything else, which the caller lowers as an ordinary field access.
pub(super) fn lower_discriminant(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    // A variant path (`Shape.Circle`) bakes the registry discriminant as a
    // literal. `enum_head` declines a value shadowing the enum's name (as
    // typecheck's `resolve_variant_for_construction` does), leaving it to the value branches.
    if let Expr::FieldAccess {
        obj: enum_obj,
        field: variant,
        ..
    } = obj
        && let Some(enum_name) = ctx.enum_head(enum_obj)
        && let Some(def) = ctx.enum_defs.expression_path(enum_name)
        && let Some(vdef) = def.variants.iter().find(|v| &v.name == variant)
    {
        return Some(literal_node_range(
            ctx,
            range,
            Type::Int,
            Literal::Int(vdef.discriminant),
        ));
    }
    // A gate read whose first output is an enum reads that output's tag.
    if let Some(fields) = gate_read_enum_value(ctx, obj)
        && let Some(disc_binding) = fields.get(&*DISC).cloned()
    {
        return binding_to_port(ctx, &disc_binding, range);
    }
    // A stored or selected enum value reads its record's `__disc` like any field.
    if let Some(Binding::Record(fields)) = resolve_field_chain(ctx, obj).cloned()
        && let Some(disc_binding) = fields.get(&*DISC).cloned()
    {
        return binding_to_port(ctx, &disc_binding, range);
    }
    // Any other enum value, read as a record value (or, off a port's column of
    // tags, as its one tag): an element of a container of enums, a call's
    // result or its one output by name (`f(c)`, `f(c).r`), an `if` or `match`
    // choosing one.
    if matches!(unwrap_ref(&ctx.type_of(obj)), Type::Enum { .. })
        && let Some(fields) = value_record_fields(ctx, obj)
        && let Some(disc_binding) = fields.get(&*DISC).cloned()
    {
        return binding_to_port(ctx, &disc_binding, range);
    }
    None
}

/// `t[i]` / `m[k]` with no runtime lowering but a compile-time-constant value,
/// as a literal source gate (the indexed analogue of `lower_ident`'s constant
/// fallback, sharing `wire_type_of_literal` and `literal_node_range`).
///
/// Must run only after [`lower_index_access_runtime`] declines: a `const t`
/// shadowed by a runtime `var t: int[]` must read the var.
fn const_fold_index_access(
    ctx: &mut LowerCtx<'_>,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let lit = ctx.const_eval(e, ConstScope::Plain).ok()?;
    let ty = wire_type_of_literal(&lit)?;
    Some(literal_node_range(ctx, range, ty, lit))
}

pub(super) fn lower_index_access(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    index: &Expr,
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // An index on an if-expression distributes over the branches:
    // `(if c then a else b)[i]` becomes `if c then a[i] else b[i]`. A
    // record-element array still hits the record-value limit in a branch.
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
    // `s[i]` on a string is a single-character Substring, not a container read.
    // Try the constant fold first so a known character costs no gate, then emit
    // the gate. Both work in pure context, since Substring is an `Expr_` gate.
    if matches!(unwrap_ref(&ctx.type_of(obj)), Type::String) {
        if let Some(port) = const_fold_index_access(ctx, e, range) {
            return port;
        }
        return lower_string_index(ctx, obj, index, range);
    }
    if let Some(port) = lower_index_access_runtime(ctx, obj, index, range) {
        return port;
    }
    if let Some(port) = const_fold_index_access(ctx, e, range) {
        return port;
    }
    // A `const` container read at a runtime index (`xs[i]`): give the
    // container its runtime form and retry. Must follow the fold, so
    // `const z = t[1]` stays a literal; the retry cannot duplicate gates.
    if materialize_const_container(ctx, obj).is_some()
        && let Some(port) = lower_index_access_runtime(ctx, obj, index, range)
    {
        return port;
    }
    // An array literal at a constant index is just that element, as long as
    // skipping the others skips no call.
    if let Expr::Array { elements, .. } = obj
        && elements.iter().all(|el| matches!(el, ArrayElem::Item(_)))
        && let Ok(Literal::Int(k)) = ctx.const_eval(index, ConstScope::Plain)
        && let Some(ArrayElem::Item(chosen)) = usize::try_from(k).ok().and_then(|k| elements.get(k))
        && elements.iter().all(|el| {
            std::ptr::eq(el.expr(), chosen) || !crate::analysis::visit::contains_call(el.expr())
        })
    {
        return lower_expr(ctx, chosen);
    }
    // An array literal read at a runtime index: its value is an immutable
    // container (`lower_expr` reports one with a runtime element).
    if let Expr::Array { .. } = obj
        && let Type::Array(elem) = unwrap_ref(&ctx.type_of(obj))
        && ctx.current_exec.is_some()
    {
        let array_ref = lower_expr(ctx, obj);
        if is_placeholder_port(ctx, array_ref) {
            return array_ref;
        }
        if let Some(port) = array_get(ctx, array_ref, *elem, index, range) {
            return port;
        }
    }
    // An element of a container of payload-free enums, read as one value: its
    // tag, as a name bound to it (`let x = hs[k]`) reads.
    if unit_enum_port(ctx, &unwrap_ref(&ctx.type_of(e)))
        && let Some(fields) = value_record_fields(ctx, e)
        && let Some(tag) = enum_tag(ctx, &fields, &ctx.type_of(e)).cloned()
        && let Some(port) = binding_to_port(ctx, &tag, range)
    {
        return port;
    }
    synthesise_unsupported(ctx, e)
}

/// `s[i]`: one Unicode character starting at `i`, via the game's Substring
/// gate with a baked `Length` of 1. Negative and out-of-range indices are the
/// gate's own business, which is why the constant fold declines them.
fn lower_string_index(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    index: &Expr,
    range: &SourceRange,
) -> PortRef {
    let text = lower_expr(ctx, obj);
    let start = lower_expr(ctx, index);
    let mut properties = HashMap::default();
    properties.insert(intern_static("Length"), Literal::Int(1));
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::STRING_SUBSTRING,
        source_range: range.clone(),
        properties,
        ports: GateIO {
            inputs: vec![
                PortSpec::new(*sym::INPUT, Type::String),
                PortSpec::new(intern_static("Start"), Type::Int),
            ],
            outputs: vec![PortSpec::new(*sym::OUTPUT, Type::String)],
        },
        ..Default::default()
    });
    ctx.connect(text, node_id.port(WirePort::Input));
    ctx.connect(
        start,
        PortRef {
            node_id,
            port: WirePort::Start,
        },
    );
    node_id.port(WirePort::Output)
}

/// The gate-emitting lowering of `obj[index]`, or `None` for no runtime form.
/// Every `None` exit precedes any lowering ([`container_ref`] emits only on
/// success), so declining leaves no half-built gates for the fold.
fn lower_index_access_runtime(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    index: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    ctx.current_exec?;
    let kind = ctx.container_kind(obj)?;
    let ContainerSource::Ref(container) = container_ref(ctx, obj, &kind)? else {
        return None;
    };
    // Map subscript `m[k]` is a MapVar_Get (as `m.get(k)`) read at its Value.
    // The Key/Value ports carry the map's concrete types; see
    // `lower_map_method` on why an `any` Key bakes a bad default.
    if let ContainerKind::Map { key: k, value: v } = &kind {
        let key = lower_expr(ctx, index);
        return Some(map_exec_op(
            ctx,
            range,
            container,
            gc::MAP_GET,
            vec![(WirePort::Key, k.clone(), key)],
            vec![(WirePort::Value, v.clone()), (WirePort::BFound, Type::Bool)],
            WirePort::Value,
        ));
    }
    let ContainerKind::Array { elem } = kind else {
        unreachable!()
    };
    array_get(ctx, container, elem, index, range)
}

/// An `ArrayVar_Get` of `array_ref` at `index` on the current exec chain,
/// returning its `Value`. `None` with no exec chain to sequence it on.
fn array_get(
    ctx: &mut LowerCtx<'_>,
    array_ref: PortRef,
    elem: Type,
    index: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let current_exec = ctx.current_exec?;
    let index_port = lower_expr(ctx, index);
    // Lowering the index may advance the exec chain (a Var_Get), so chain
    // from the updated head.
    let current_exec = ctx.current_exec.unwrap_or(current_exec);
    let elem_ty = elem;
    let node_id = ctx.add_gate(AddNodeOpts {
        gate_class: gc::ARRAY_GET,
        source_range: range.clone(),
        ports: GateIO {
            inputs: vec![
                PortSpec::exec(*sym::EXEC),
                PortSpec::new(*sym::ARRAY_VAR_REF, Type::Ref(Box::new(elem_ty.clone()))),
                PortSpec::new(*sym::INDEX, Type::Int),
            ],
            outputs: vec![
                PortSpec::new(*sym::VALUE, elem_ty),
                PortSpec::new(*sym::B_OUT_OF_BOUNDS, Type::Bool),
                PortSpec::exec(*sym::EXEC_OUT),
            ],
        },
        note: Some("array get"),
        ..Default::default()
    });
    ctx.connect(current_exec, node_id.port(WirePort::Exec));
    ctx.connect(array_ref, node_id.port(WirePort::ArrayVarRef));
    ctx.connect(index_port, node_id.port(WirePort::Index));
    ctx.current_exec = Some(node_id.port(WirePort::ExecOut));
    Some(node_id.port(WirePort::Value))
}

/// How a record container stores its fields: an array of records keeps one
/// parallel array per field, read and written at an index; a map of records
/// keeps one parallel map per field, at a key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Columns {
    Array,
    Map,
}

impl Columns {
    fn storage(self) -> VarStorage {
        match self {
            Self::Array => VarStorage::Array,
            Self::Map => VarStorage::Map,
        }
    }

    /// The port a column's ref arrives on, and the type its gates declare it.
    fn ref_port(self) -> (WirePort, Type) {
        match self {
            Self::Array => (WirePort::ArrayVarRef, Type::Array(Box::new(Type::Any))),
            Self::Map => (WirePort::MapVarRef, Type::Ref(Box::new(Type::Any))),
        }
    }

    /// The port that picks one position of column `v`, and its type: the
    /// index, or the key.
    fn position(self, v: &VarRecord) -> (WirePort, Type) {
        match self {
            Self::Array => (WirePort::Index, Type::Int),
            Self::Map => (WirePort::Key, map_kv(v).0),
        }
    }

    /// The type one position of column `v` holds.
    fn value_type(self, v: &VarRecord) -> Type {
        match self {
            Self::Array => v.inner_type.clone(),
            Self::Map => map_kv(v).1,
        }
    }

    /// The get gate, and the flag it reports beside the value.
    fn get(self) -> (&'static str, WirePort) {
        match self {
            Self::Array => (gc::ARRAY_GET, WirePort::BOutOfBounds),
            Self::Map => (gc::MAP_GET, WirePort::BFound),
        }
    }

    fn set(self) -> &'static str {
        match self {
            Self::Array => gc::ARRAY_SET_AT_INDEX,
            Self::Map => gc::MAP_SET,
        }
    }

    /// What a record written into one position is called in a message.
    fn written(self) -> &'static str {
        match self {
            Self::Array => "the element written here",
            Self::Map => "the value written here",
        }
    }

    /// What a read with no exec chain is called in a message.
    fn read(self) -> &'static str {
        match self {
            Self::Array => "an array read with no exec chain to run on",
            Self::Map => "a map read with no exec chain to run on",
        }
    }
}

/// An exec op on one array or map: wires the current exec, the container's
/// ref `container` and `extra_in`, exposes `extra_out`, advances the exec
/// chain, and returns the `ret` port.
///
/// With no exec chain the op is reported rather than returning the ref, which
/// would put a container reference where a value is expected. A `const` map
/// read reaches here when its fold declines (`container_call_exec_exempt`).
#[expect(
    clippy::too_many_arguments,
    reason = "each input is a separate part of the gate's shape"
)]
pub(super) fn column_exec_op(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    range: &SourceRange,
    container: PortRef,
    gate_class: &'static str,
    extra_in: Vec<(WirePort, Type, PortRef)>,
    extra_out: Vec<(WirePort, Type)>,
    ret: WirePort,
    note: Option<&'static str>,
) -> PortRef {
    let Some(exec_in) = ctx.current_exec else {
        return synthesise_unsupported_range(ctx, cols.read(), range);
    };
    let (ref_port, ref_type) = cols.ref_port();
    let mut inputs = vec![
        PortSpec::exec(*sym::EXEC),
        PortSpec::new(ref_port.sym(), ref_type),
    ];
    for (port, ty, _) in &extra_in {
        inputs.push(PortSpec::new(port.sym(), ty.clone()));
    }
    let mut outputs = vec![PortSpec::exec(*sym::EXEC_OUT)];
    for (port, ty) in extra_out {
        outputs.push(PortSpec::new(port.sym(), ty));
    }
    let mut wired = vec![(container, ref_port)];
    wired.extend(extra_in.into_iter().map(|(port, _, src)| (src, port)));
    let opts = AddNodeOpts {
        gate_class,
        source_range: range.clone(),
        ports: GateIO { inputs, outputs },
        note,
        ..Default::default()
    };
    chain_gate(ctx, opts, exec_in, &wired).port(ret)
}

/// [`column_exec_op`] on an array.
pub(super) fn array_exec_op(
    ctx: &mut LowerCtx<'_>,
    range: &SourceRange,
    array_ref: PortRef,
    gate_class: &'static str,
    extra_in: Vec<(WirePort, Type, PortRef)>,
    extra_out: Vec<(WirePort, Type)>,
    ret: WirePort,
) -> PortRef {
    column_exec_op(
        ctx,
        Columns::Array,
        range,
        array_ref,
        gate_class,
        extra_in,
        extra_out,
        ret,
        None,
    )
}

/// [`column_exec_op`] on a map; also the `m[k] = v` write in `lower_assign`.
pub(super) fn map_exec_op(
    ctx: &mut LowerCtx<'_>,
    range: &SourceRange,
    map_ref: PortRef,
    gate_class: &'static str,
    extra_in: Vec<(WirePort, Type, PortRef)>,
    extra_out: Vec<(WirePort, Type)>,
    ret: WirePort,
) -> PortRef {
    column_exec_op(
        ctx,
        Columns::Map,
        range,
        map_ref,
        gate_class,
        extra_in,
        extra_out,
        ret,
        None,
    )
}

/// The leaf columns of a record container's field map, name-sorted for a
/// stable fan-out order, recursing through nested record fields: one parallel
/// array (or map) per record field.
pub(super) fn leaf_columns(cols: Columns, fields: &Fields) -> Vec<VarRecord> {
    let names = sorted_field_names(fields);
    let mut out = Vec::new();
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => out.extend(leaf_columns(cols, sub)),
            Some(Binding::Var(v)) if v.storage == cols.storage() => out.push(v.clone()),
            _ => {}
        }
    }
    out
}

/// Resolve `base` (an identifier or a record field chain) to a record
/// container's field map: a `Binding::Record` with at least one leaf column.
pub(super) fn resolve_record_columns(
    ctx: &LowerCtx<'_>,
    cols: Columns,
    base: &Expr,
) -> Option<Fields> {
    let binding = match base {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        _ => resolve_field_chain(ctx, base).cloned(),
    }?;
    match binding {
        Binding::Record(f) if !leaf_columns(cols, &f).is_empty() => Some(f),
        _ => None,
    }
}

/// Decompose `c[p].f1.f2...` into the container base, the position expr and the
/// field path in source order (`.inner` and `.0` both add a segment). A map's
/// position is `m[k]` or `m.get(k)` ([`record_map_key_source`]). `None` unless
/// field/tuple accesses wrap a position access.
fn split_record_access(cols: Columns, e: &Expr) -> Option<(&Expr, &Expr, Vec<String>)> {
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
                let (base, position) = match (cols, cur) {
                    (Columns::Array, Expr::IndexAccess { obj, index, .. }) => {
                        (obj.as_ref(), index.as_ref())
                    }
                    (Columns::Map, _) => record_map_key_source(cur)?,
                    _ => return None,
                };
                path.reverse();
                return Some((base, position, path));
            }
        }
    }
}

/// Read column `v` at `position`: the get gate's `Value` port.
fn column_get(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    v: &VarRecord,
    position: PortRef,
    range: &SourceRange,
) -> PortRef {
    let (class, flag) = cols.get();
    let (pos_port, pos_type) = cols.position(v);
    let value_type = cols.value_type(v);
    column_exec_op(
        ctx,
        cols,
        range,
        v.node_id.port(cols.ref_port().0),
        class,
        vec![(pos_port, pos_type, position)],
        vec![(WirePort::Value, value_type), (flag, Type::Bool)],
        WirePort::Value,
        None,
    )
}

/// Write `value` into column `v` at `position`.
fn column_set(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    v: &VarRecord,
    position: PortRef,
    value: PortRef,
    range: &SourceRange,
) {
    let (pos_port, pos_type) = cols.position(v);
    let value_type = cols.value_type(v);
    column_exec_op(
        ctx,
        cols,
        range,
        v.node_id.port(cols.ref_port().0),
        cols.set(),
        vec![
            (pos_port, pos_type, position),
            (WirePort::Value, value_type, value),
        ],
        vec![],
        WirePort::ExecOut,
        None,
    );
}

/// Read every leaf column at `position` into a `Binding::Record` of `Local`
/// ports, recursing through nested records.
pub(super) fn record_column_read(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    fields: &Fields,
    position: PortRef,
    range: &SourceRange,
) -> Fields {
    let mut out = HashMap::default();
    let names = sorted_field_names(fields);
    for k in names {
        match fields.get(&k) {
            Some(Binding::Record(sub)) => {
                out.insert(
                    k,
                    Binding::Record(record_column_read(ctx, cols, sub, position, range)),
                );
            }
            Some(Binding::Var(v)) if v.storage == cols.storage() => {
                let port = column_get(ctx, cols, v, position, range);
                out.insert(k, Binding::Local(LocalRecord { port }));
            }
            _ => {}
        }
    }
    out
}

/// `pts[i] = rec` / `m[k] = rec` / `m.set(k, rec)`: write a whole record by
/// fanning the record value across the leaf columns at the shared position.
/// Returns `true` when it handled the assignment.
pub(super) fn lower_record_position_set(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    fields: &Fields,
    position: &Expr,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    if ctx.current_exec.is_none() {
        return true;
    }
    let Some((position_port, src)) = keyed_record(ctx, position, value) else {
        return false;
    };
    if let Some(src) = fit_record_value(ctx, cols.written(), value, fields, src) {
        record_set_walk(ctx, cols, fields, &src, position_port, range);
    }
    true
}

/// Write the record value `src` field by field into the leaf columns at
/// `position`, recursing through nested records.
pub(super) fn record_set_walk(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    fields: &Fields,
    src: &Fields,
    position: PortRef,
    range: &SourceRange,
) {
    let names = sorted_field_names(fields);
    for k in names {
        let Some(sbind) = src.get(&k).cloned() else {
            continue;
        };
        match fields.get(&k) {
            Some(Binding::Record(tsub)) => {
                if let Binding::Record(ssub) = &sbind {
                    record_set_walk(ctx, cols, tsub, ssub, position, range);
                }
            }
            Some(Binding::Var(v)) if v.storage == cols.storage() => {
                let Some(val_port) = binding_to_port(ctx, &sbind, range) else {
                    continue;
                };
                column_set(ctx, cols, v, position, val_port, range);
            }
            _ => {}
        }
    }
}

/// `c[p].f1.f2...`: read a field of a record container's entry. A scalar leaf
/// yields its column's `Value`; a nested record reads every leaf into
/// `pending_inline_record` and returns its first leaf port. `None` unless the
/// path resolves and there is an exec context.
pub(super) fn lower_record_field_path(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    e: &Expr,
    range: &SourceRange,
) -> Option<PortRef> {
    let (base, position, path) = split_record_access(cols, e)?;
    if path.is_empty() {
        return None;
    }
    let fields = resolve_record_columns(ctx, cols, base)?;
    let binding = navigate_record_fields(&fields, &path)?.clone();
    ctx.current_exec?;
    let position_port = lower_expr(ctx, position);
    match binding {
        Binding::Var(v) if v.storage == cols.storage() => {
            Some(column_get(ctx, cols, &v, position_port, range))
        }
        Binding::Record(sub) => {
            let rec = record_column_read(ctx, cols, &sub, position_port, range);
            let primary = first_leaf_port(&rec)?;
            ctx.pending_inline_record = Some(rec);
            Some(primary)
        }
        _ => None,
    }
}

/// `pts[i].inner` / `m[k].inner` as a record VALUE: a record-container field
/// path landing on a nested record. `None` otherwise, or with no exec context
/// to read on.
pub(super) fn lower_record_field_path_value(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    e: &Expr,
    range: &SourceRange,
) -> Option<Fields> {
    let (base, position, path) = split_record_access(cols, e)?;
    if path.is_empty() {
        return None;
    }
    let fields = resolve_record_columns(ctx, cols, base)?;
    let Binding::Record(sub) = navigate_record_fields(&fields, &path)?.clone() else {
        return None;
    };
    ctx.current_exec?;
    let position_port = lower_expr(ctx, position);
    Some(record_column_read(ctx, cols, &sub, position_port, range))
}

/// `c[p].f1.f2... = value`: write a field of a record container's entry. A
/// scalar leaf writes its column; a nested record fans the value across the
/// sub-record's columns at the shared position.
/// Returns `true` when it handled the assignment.
pub(super) fn lower_record_field_path_set(
    ctx: &mut LowerCtx<'_>,
    cols: Columns,
    target: &Expr,
    value: &Expr,
    range: &SourceRange,
) -> bool {
    let Some((base, position, path)) = split_record_access(cols, target) else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    let Some(fields) = resolve_record_columns(ctx, cols, base) else {
        return false;
    };
    let Some(binding) = navigate_record_fields(&fields, &path).cloned() else {
        return false;
    };
    if ctx.current_exec.is_none() {
        return true;
    }
    match binding {
        Binding::Var(v) if v.storage == cols.storage() => {
            let position_port = lower_expr(ctx, position);
            let value_port = lower_expr(ctx, value);
            column_set(ctx, cols, &v, position_port, value_port, range);
            true
        }
        Binding::Record(sub) => {
            let Some(src) = value_record_fields(ctx, value) else {
                return false;
            };
            let Some(src) = fit_record_value(ctx, cols.written(), value, &sub, src) else {
                return true;
            };
            let position_port = lower_expr(ctx, position);
            record_set_walk(ctx, cols, &sub, &src, position_port, range);
            true
        }
        _ => false,
    }
}

/// Resolve an argument naming another array (append/copyFrom/slice, a map's
/// keys/values destination, sortMultiple's arrays) to its ref port. A constant
/// array argument (a `const`, a `let` of one, a literal) resolves to its materialized gate.
pub(super) fn resolve_array_ref_arg(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
    materialize_const_container(ctx, e);
    if let Some(rec) = materialize_literal_container(ctx, e, "literal") {
        return Some(rec.node_id.port(WirePort::ArrayVarRef));
    }
    let kind @ ContainerKind::Array { .. } = ctx.container_kind(e)? else {
        return None;
    };
    match container_ref(ctx, e, &kind)? {
        ContainerSource::Ref(p) => Some(p),
        ContainerSource::Columns(_) => None,
    }
}

/// Resolve an argument naming another map (`copyFrom`) to its ref port; a
/// constant map argument resolves to its materialized gate.
fn resolve_map_ref_arg(ctx: &mut LowerCtx<'_>, e: &Expr) -> Option<PortRef> {
    materialize_const_container(ctx, e);
    let kind @ ContainerKind::Map { .. } = ctx.container_kind(e)? else {
        return None;
    };
    match container_ref(ctx, e, &kind)? {
        ContainerSource::Ref(p) => Some(p),
        ContainerSource::Columns(_) => None,
    }
}

/// Whether `obj`'s value is a container ref that exists only at runtime (an
/// element `rows[y]`, event data, a call result). Declines record/enum elements
/// (no single ref) and chains rooted in a `const` or an `_Unsupported`
/// placeholder, so those keep their WS044 or placeholder diagnostic.
fn is_runtime_container(ctx: &LowerCtx<'_>, obj: &Expr, elem: &Type) -> bool {
    if matches!(elem, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. }) {
        return false;
    }
    let mut chain = vec![obj];
    while let Expr::IndexAccess { obj, .. }
    | Expr::FieldAccess { obj, .. }
    | Expr::TuplePick { obj, .. } = chain[chain.len() - 1]
    {
        chain.push(obj);
    }
    if let Expr::Ident { name, .. } = chain[chain.len() - 1] {
        // Resolve up the field chain from the name (`V.rows` in `V.rows[0]`)
        // for as long as it names bindings. A field of any other binding
        // (`e.Value` on a `let e = m.get(k)`) is a runtime value, not a miss.
        let Some(mut binding) = ctx.scope.get(name) else {
            return false;
        };
        for step in chain.iter().rev().skip(1) {
            if !matches!(step, Expr::FieldAccess { .. } | Expr::TuplePick { .. }) {
                break;
            }
            let from_namespace = matches!(binding, Binding::Namespace(_));
            match resolve_field_chain(ctx, step) {
                Some(b) => binding = b,
                None if from_namespace => return false,
                None => break,
            }
        }
        if let Binding::Local(local) = binding
            && is_placeholder_port(ctx, local.port)
        {
            return false;
        }
    }
    match obj {
        Expr::Ident { name, .. } => !matches!(
            ctx.scope.get(name),
            Some(
                Binding::Record(_)
                    | Binding::Namespace(_)
                    | Binding::Chip(_)
                    | Binding::Var(VarRecord {
                        storage: VarStorage::Array | VarStorage::Map,
                        ..
                    })
            )
        ),
        // A literal names no container; a constant one is materialized by the
        // caller (`materialize_literal_container`).
        Expr::Array { .. } | Expr::MapLit { .. } => false,
        _ => true,
    }
}

/// Where a container receiver's ref comes from.
pub(super) enum ContainerSource {
    /// One `ArrayVarRef` / `MapVarRef` port.
    Ref(PortRef),
    /// A record (or tuple or enum) container stored as parallel per-field columns.
    Columns(Fields),
}

/// The one receiver resolver. `kind` is typecheck's answer for what `obj` is;
/// this only finds where its ref comes from: record columns, then a static
/// binding (var, `in` port, record field, runtime-bound parameter, non-placeholder
/// `let`), then the receiver's runtime value. A `const` receiver must already be
/// materialized by the caller, which owns the fold-versus-materialize order.
pub(super) fn container_ref(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    kind: &ContainerKind,
) -> Option<ContainerSource> {
    let columnar = |t: &Type| matches!(t, Type::Record(_) | Type::Tuple(_) | Type::Enum { .. });
    // An `in`/`out` port of columnar elements is one ref, not columns, so a
    // miss here falls through to the static binding.
    let columns = match kind {
        ContainerKind::Array { elem } if columnar(elem) => {
            resolve_record_columns(ctx, Columns::Array, obj)
        }
        ContainerKind::Map { value, .. } if columnar(value) => {
            resolve_record_columns(ctx, Columns::Map, obj)
        }
        _ => None,
    };
    if let Some(fields) = columns {
        return Some(ContainerSource::Columns(fields));
    }
    let binding = match obj {
        Expr::Ident { name, .. } => ctx.scope.get(name).cloned(),
        _ => resolve_field_chain(ctx, obj).cloned(),
    };
    if let Some(b) = &binding {
        let ty = kind.as_type();
        let hit = match kind {
            ContainerKind::Array { .. } => binding_array_ref(ctx, b, &ty).map(|(p, _)| p),
            ContainerKind::Map { .. } => binding_map_ref(ctx, b, &ty).map(|(p, _)| p),
        };
        if let Some(p) = hit {
            return Some(ContainerSource::Ref(p));
        }
    }
    let inner = match kind {
        ContainerKind::Array { elem } => elem,
        ContainerKind::Map { value, .. } => value,
    };
    if !is_runtime_container(ctx, obj, inner) {
        return None;
    }
    Some(ContainerSource::Ref(lower_expr(ctx, obj)))
}

/// The container backstop: an array/map operation whose receiver resolved to no
/// container ref here. `what` names the operation and leads the message.
pub(super) fn report_unresolved_container(ctx: &mut LowerCtx<'_>, what: &str, range: &SourceRange) {
    ctx.error(
        "WS044",
        format!(
            "{what}, but its receiver did not resolve to an array or map here (e.g. a \
             container in an unsupported position), so the operation would be silently dropped"
        ),
        range,
    );
}

/// The WS044 for a container ARGUMENT (a source or destination array/map) that
/// resolved to no container ref here, so the operation has nothing to act on.
pub(super) fn report_unresolved_container_arg(
    ctx: &mut LowerCtx<'_>,
    method: &str,
    range: &SourceRange,
) {
    ctx.error(
        "WS044",
        format!(
            "an array/map argument of `.{method}()` did not resolve to an array or map \
             here (e.g. a computed value, or a literal with non-constant elements), so \
             the operation would be silently dropped. Pass an array or map `var`"
        ),
        range,
    );
}

/// Whether `node` is an immutable container gate, or a `Select` or buffer in
/// this module that can pass one through (`if c then T else xs`, `buffer b = T`).
pub(super) fn is_immutable_container(ctx: &LowerCtx<'_>, node: NodeId) -> bool {
    if ctx.immutable_containers.is_empty() {
        return false;
    }
    let mut stack = vec![node];
    let mut seen: HashSet<NodeId> = HashSet::default();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if ctx.immutable_containers.contains(&id) {
            return true;
        }
        let Some(class) = ctx.builder.module.nodes.get(&id).map(|n| n.gate_class) else {
            continue;
        };
        let passes = |port: WirePort| match class {
            gc::SELECT => matches!(port, WirePort::InputA | WirePort::InputB),
            gc::BUFFER_TICKS | gc::BUFFER_SECONDS => port == WirePort::Input,
            _ => false,
        };
        if !matches!(class, gc::SELECT | gc::BUFFER_TICKS | gc::BUFFER_SECONDS) {
            continue;
        }
        let feeds = ordered::feeds_of(ctx, id);
        stack.extend(
            feeds
                .into_iter()
                .filter(|(port, _)| passes(*port))
                .map(|(_, src)| src.node_id),
        );
    }
    false
}

/// Reject (WS044) a method that would change a `const` container's contents,
/// which would make its compile-time value and its runtime gate disagree.
/// Keyed on the gate node, so it holds through aliasing (a `const` passed to a
/// `ys: T[]` parameter). Returns `true` when it reported; the caller must not emit the op.
pub(super) fn reject_const_container_mutation(
    ctx: &mut LowerCtx<'_>,
    target: NodeId,
    mutates: bool,
    op: &str,
    range: &SourceRange,
) -> bool {
    if !mutates || !is_immutable_container(ctx, target) {
        return false;
    }
    ctx.error(
        "WS044",
        format!(
            "{op} would change a `const` array/map. A constant container (a \
             `const`, or a `let` or literal holding a constant array/map) is \
             immutable, so its compile-time value and its runtime contents can \
             never disagree. Declare it `var` to make it mutable"
        ),
        range,
    );
    true
}

/// A record binding's field names, sorted by string. Walks emit gates, so hash
/// or intern order would make node ids (and output bytes) vary between compiles.
fn sorted_field_names(fields: &Fields) -> Vec<Sym> {
    let mut names: Vec<Sym> = fields.keys().copied().collect();
    names.sort_by_key(|s| resolve(*s));
    names
}

pub(super) fn map_kv(v: &VarRecord) -> (Type, Type) {
    match &v.inner_type {
        Type::Map(k, val) => (k.as_ref().clone(), val.as_ref().clone()),
        _ => (Type::Any, Type::Any),
    }
}

/// The map base expr and key expr of `m[key]` or `m.get(key)`, so both
/// spellings route through the same record-map paths.
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

/// `m.get(k)` / `m[k]` as a record value: every parallel field map read at the
/// shared key into a `Binding::Record` of `Local` ports, recursing nested
/// records. Mirrors [`lower_record_array_index_value`].
pub(super) fn lower_record_map_key_value(
    ctx: &mut LowerCtx<'_>,
    expr: &Expr,
    range: &SourceRange,
) -> Option<Fields> {
    let (base, key) = record_map_key_source(expr)?;
    let fields = resolve_record_columns(ctx, Columns::Map, base)?;
    ctx.current_exec?;
    let key_port = lower_expr(ctx, key);
    Some(record_column_read(
        ctx,
        Columns::Map,
        &fields,
        key_port,
        range,
    ))
}

/// Fan a record-map method across the parallel per-field maps. `set` decomposes
/// the record value; `has`/`length` read the first field (all fields share the
/// same keys); `clear`/`remove` fan out; `get` builds the record value and
/// stashes it in `pending_inline_record` (a let/out consumer picks it up).
pub(super) fn lower_record_map_method(
    ctx: &mut LowerCtx<'_>,
    fields: &Fields,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let leaves = leaf_columns(Columns::Map, fields);
    if leaves.is_empty() {
        return synthesise_unsupported(ctx, e);
    }
    let first_ref = leaves[0].node_id.port(WirePort::MapVarRef);
    match method {
        "length" | "has" => lower_map_method(
            ctx,
            first_ref,
            &leaves[0].inner_type,
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
                    &leaf.inner_type,
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
            match element_fields(ctx, fields, v) {
                Some(src) => {
                    let key_port = lower_expr(ctx, k);
                    record_set_walk(ctx, Columns::Map, fields, &src, key_port, range);
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
                    // A record auto-unwraps to its first field for a scalar
                    // consumer; record consumers take the pending record.
                    let primary = in_declared_order(ctx, &rec, e.range())
                        .into_iter()
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
        // Every parallel field map has the same keys, so `keys(dest)` reads
        // the first field's.
        "keys" => lower_map_method(
            ctx,
            first_ref,
            &leaves[0].inner_type,
            "keys",
            args,
            range,
            e,
        ),
        // `values` (a per-field split into a record array) and `copyFrom` (a
        // matching record map) have no record-map form.
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

fn nth_positional(args: &[CallArg], i: usize) -> Option<&Expr> {
    match args.get(i) {
        Some(CallArg::Positional(e)) => Some(e),
        _ => None,
    }
}

/// Apply a value-carrying array op (push / insert / fill) to each parallel field
/// array, reading the source record value field by field and recursing through
/// nested records. `scalar` (the index for `insert`) fans the same extra input
/// into every field.
pub(super) fn record_array_value_op(
    ctx: &mut LowerCtx<'_>,
    fields: &Fields,
    src: &Fields,
    range: &SourceRange,
    gate_class: &'static str,
    scalar: Option<(WirePort, PortRef)>,
) -> Option<PortRef> {
    let names = sorted_field_names(fields);
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

/// Walk a record field map along `path`, returning the binding the LAST segment
/// names. Every earlier segment must name a nested record to descend through;
/// a non-record earlier segment (or any missing name) yields `None`. `path`
/// must be non-empty.
fn navigate_record_fields<'a>(fields: &'a Fields, path: &[String]) -> Option<&'a Binding> {
    let mut cur = fields;
    for (i, seg) in path.iter().enumerate() {
        let b = cur.get(&intern(seg))?;
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
fn first_leaf_port(rec: &Fields) -> Option<PortRef> {
    let names = sorted_field_names(rec);
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

/// The array var backing the leaf column `path` names in a record array
/// (`pts.x` is `["x"]`, `qs.p.x` is `["p", "x"]`). `None` if the path does not
/// end on a scalar parallel array.
fn record_array_field(fields: &Fields, path: &[String]) -> Option<VarRecord> {
    match navigate_record_fields(fields, path) {
        Some(Binding::Var(v)) if v.storage == VarStorage::Array => Some(v.clone()),
        _ => None,
    }
}

/// `value`'s fields as an element of the record or enum container whose
/// columns are `columns`, fitted to them (see `stmt::fit_record_value`).
fn element_fields(ctx: &mut LowerCtx<'_>, columns: &Fields, value: &Expr) -> Option<Fields> {
    let src = value_record_fields(ctx, value)?;
    fit_record_value(ctx, "the element written here", value, columns, src)
}

/// `pts[i]` as a record value: every parallel field array read at the shared
/// index into a `Binding::Record` of `Local` ports, recursing nested records.
/// `None` if `base` is not a record array or there is no exec context.
pub(super) fn lower_record_array_index_value(
    ctx: &mut LowerCtx<'_>,
    base: &Expr,
    index: &Expr,
    range: &SourceRange,
) -> Option<Fields> {
    let fields = resolve_record_columns(ctx, Columns::Array, base)?;
    ctx.current_exec?;
    let index_port = lower_expr(ctx, index);
    Some(record_column_read(
        ctx,
        Columns::Array,
        &fields,
        index_port,
        range,
    ))
}

/// A fresh, unbound `Pseudo_ArrayVar` scratch gate of element type `elem` (for
/// the multi-pass record sort's key copies). Not bound in scope; only its ref
/// port is wired.
fn make_scratch_array(ctx: &mut LowerCtx<'_>, elem: &Type, range: &SourceRange) -> PortRef {
    make_array_var_gate(ctx, elem, HashMap::default(), range, Some("sort key copy"))
        .port(WirePort::ArrayVarRef)
}

/// `soa.field.sort(descending?)` on a record array: sort whole rows by the
/// column `path` names (`qs.p.x` is `["p", "x"]`). `sortMultiple` carries a key
/// plus 7 columns, so wider records sort in groups of 7, each later group against
/// a pre-sort copy of the key; the deterministic sort gives every group the same
/// permutation. `None` if `path` names no scalar column.
pub(super) fn lower_record_array_field_sort(
    ctx: &mut LowerCtx<'_>,
    fields: &Fields,
    path: &[String],
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
    let key = record_array_field(fields, path)?;
    let key_ref = key.node_id.port(WirePort::ArrayVarRef);
    let key_elem = key.inner_type.clone();
    let parallels: Vec<VarRecord> = leaf_columns(Columns::Array, fields)
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
        return Some(array_exec_op(
            ctx,
            range,
            key_ref,
            gc::ARRAY_SORT,
            extra,
            vec![],
            WirePort::ExecOut,
        ));
    }

    let groups: Vec<&[VarRecord]> = parallels.chunks(REFS.len()).collect();
    // Copy the key for every group past the first before any sort runs, so
    // each copy holds the pre-sort values.
    let mut sort_keys: Vec<PortRef> = vec![key_ref];
    for _ in 1..groups.len() {
        let scratch = make_scratch_array(ctx, &key_elem, range);
        array_exec_op(
            ctx,
            range,
            scratch,
            gc::ARRAY_COPY_FROM,
            vec![(
                WirePort::SourceRef,
                Type::Array(Box::new(Type::Any)),
                key_ref,
            )],
            vec![],
            WirePort::ExecOut,
        );
        sort_keys.push(scratch);
    }
    let mut ret = ctx.current_exec;
    for (gi, group) in groups.iter().enumerate() {
        let mut extra: Vec<(WirePort, Type, PortRef)> = Vec::new();
        for (i, p) in group.iter().enumerate() {
            extra.push((
                REFS[i],
                Type::Array(Box::new(Type::Any)),
                p.node_id.port(WirePort::ArrayVarRef),
            ));
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
/// the record array's `Binding::Record`. Ops that keep the arrays in lockstep
/// fan out; `length` reads the first field. Ops that would desync rows if run
/// per field (sort, shuffle, find, the aggregates) are WS050.
pub(super) fn lower_record_array_method(
    ctx: &mut LowerCtx<'_>,
    fields: &Fields,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let leaves = leaf_columns(Columns::Array, fields);
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
        "push" => match nth_positional(args, 0).and_then(|v| element_fields(ctx, fields, v)) {
            Some(src) => record_array_value_op(ctx, fields, &src, range, gc::ARRAY_PUSH, None)
                .unwrap_or_else(|| synthesise_unsupported(ctx, e)),
            None => synthesise_unsupported(ctx, e),
        },
        "fill" => match nth_positional(args, 0).and_then(|v| element_fields(ctx, fields, v)) {
            Some(src) => record_array_value_op(ctx, fields, &src, range, gc::ARRAY_FILL, None)
                .unwrap_or_else(|| synthesise_unsupported(ctx, e)),
            None => synthesise_unsupported(ctx, e),
        },
        "insert" => {
            let (Some(i), Some(v)) = (nth_positional(args, 0), nth_positional(args, 1)) else {
                return synthesise_unsupported(ctx, e);
            };
            let idx = lower_expr(ctx, i);
            match element_fields(ctx, fields, v) {
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
            match element_fields(ctx, fields, v) {
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
        // `swap(a, b)` on every column moves whole rows, keeping lockstep.
        "swap" => {
            let (Some(a), Some(b)) = (nth_positional(args, 0), nth_positional(args, 1)) else {
                return synthesise_unsupported(ctx, e);
            };
            let (pa, pb) = lower_pair(ctx, a, b);
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
        // `pop` removes the last row and stashes it in `pending_inline_record`
        // for a let/assign/out consumer.
        "pop" => {
            let record = record_array_pop(ctx, fields, range);
            let primary = in_declared_order(ctx, &record, e.range())
                .into_iter()
                .find_map(|b| match b {
                    Binding::Local(l) => Some(l.port),
                    _ => None,
                })
                .unwrap_or_else(|| synthesise_unsupported(ctx, e));
            ctx.pending_inline_record = Some(record);
            primary
        }
        // Everything else has no per-column meaning (sort/shuffle desync rows,
        // aggregates and `find` need whole records, dual-array ops need a
        // matching record array), so it is WS050.
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
fn record_array_pop(ctx: &mut LowerCtx<'_>, fields: &Fields, range: &SourceRange) -> Fields {
    let mut out = HashMap::default();
    let names = sorted_field_names(fields);
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

/// Lower `arr.<method>(...)` for an array value whose element type is
/// `elem_ty`. Every method in [`crate::catalog::arrays::ARRAY_METHODS`] lowers
/// through its entry's [`Lowering`].
pub(super) fn lower_array_method(
    ctx: &mut LowerCtx<'_>,
    array_ref: PortRef,
    elem_ty: Type,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let kind = ContainerKind::Array { elem: elem_ty };
    lower_container_method(ctx, array_ref, &kind, method, args, range, e)
}

/// Lower `m.<method>(...)` for a `map` value. `map_type` is the whole
/// `Type::Map(K, V)`. Every method in [`crate::catalog::maps::MAP_METHODS`]
/// lowers through its entry's [`Lowering`].
pub(super) fn lower_map_method(
    ctx: &mut LowerCtx<'_>,
    map_ref: PortRef,
    map_type: &Type,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let kind = match ContainerKind::of(map_type) {
        Some(k @ ContainerKind::Map { .. }) => k,
        _ => ContainerKind::Map {
            key: Type::Any,
            value: Type::Any,
        },
    };
    lower_container_method(ctx, map_ref, &kind, method, args, range, e)
}

/// The table entry for `method` on a receiver of `kind`: its gate, whether it
/// changes the receiver, and how it lowers.
fn container_method_entry(
    kind: &ContainerKind,
    method: &str,
) -> Option<(&'static str, bool, &'static Lowering)> {
    match kind {
        ContainerKind::Array { .. } => {
            crate::catalog::arrays::array_method(method).map(|m| (m.gate, m.mutates, &m.lowering))
        }
        ContainerKind::Map { .. } => {
            crate::catalog::maps::map_method(method).map(|m| (m.gate, m.mutates, &m.lowering))
        }
    }
}

fn lower_container_method(
    ctx: &mut LowerCtx<'_>,
    recv: PortRef,
    kind: &ContainerKind,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    // `exec = <trigger>` makes the op a leaf: restore the caller's exec context
    // around the whole call so no early return leaves later statements on the trigger.
    let uses_exec_arg = args.has_exec_arg();
    let saved_exec = ctx.current_exec;
    let result = lower_container_method_inner(ctx, recv, kind, method, args, range, e);
    if uses_exec_arg {
        ctx.current_exec = saved_exec;
    }
    result
}

fn lower_container_method_inner(
    ctx: &mut LowerCtx<'_>,
    recv: PortRef,
    kind: &ContainerKind,
    method: &str,
    args: &[CallArg],
    range: &SourceRange,
    e: &Expr,
) -> PortRef {
    let Some((gate, mutates, lowering)) = container_method_entry(kind, method) else {
        return synthesise_unsupported(ctx, e);
    };
    if reject_const_container_mutation(ctx, recv.node_id, mutates, &format!("`.{method}()`"), range)
    {
        return synthesise_unsupported(ctx, e);
    }
    // `exec = <trigger>` drives the op off an explicit trigger, so a read like
    // `lut.get(i, exec = i + 1)` works in a pure context.
    if let Some(exec_expr) = args.exec_arg() {
        let src = lower_expr(ctx, exec_expr);
        ctx.current_exec = Some(src);
    }
    // An array op with no exec chain has nothing to run on; a map op reports
    // that itself (`map_exec_op`).
    if matches!(kind, ContainerKind::Array { .. }) && ctx.current_exec.is_none() {
        return synthesise_unsupported(ctx, e);
    }
    let specs = match lowering {
        Lowering::Generic { args: specs, .. } => *specs,
        Lowering::Custom => {
            return match method {
                "sortMultiple" => lower_sort_multiple(ctx, recv, args, range),
                _ => synthesise_unsupported(ctx, e),
            };
        }
    };
    let Some(bound) = bind_method_args(specs, args) else {
        return synthesise_unsupported(ctx, e);
    };
    let mut inputs: Vec<(WirePort, PortRef)> = Vec::new();
    // Container arguments resolve to their refs first, so an unresolved one
    // reports before any value is lowered.
    for &(i, arg) in &bound {
        let spec = &specs[i];
        let resolved = match spec.kind {
            ArgKind::Value(_) => continue,
            ArgKind::ArrayRef(_) | ArgKind::DestArray(_) => resolve_array_ref_arg(ctx, arg),
            ArgKind::MapRef => resolve_map_ref_arg(ctx, arg),
        };
        let Some(port) = resolved else {
            report_unresolved_container_arg(ctx, method, range);
            return synthesise_unsupported_no_warn(ctx, range);
        };
        // A destination array is refilled.
        if matches!(spec.kind, ArgKind::DestArray(_))
            && reject_const_container_mutation(
                ctx,
                port.node_id,
                true,
                &format!("`.{method}()`"),
                range,
            )
        {
            return synthesise_unsupported_no_warn(ctx, range);
        }
        inputs.push((spec.port, port));
    }
    let values: Vec<(usize, &Expr)> = bound
        .iter()
        .copied()
        .filter(|&(i, _)| matches!(specs[i].kind, ArgKind::Value(_)))
        .collect();
    let exprs: Vec<&Expr> = values.iter().map(|&(_, x)| x).collect();
    let ports = lower_in_order(ctx, &exprs);
    inputs.extend(
        values
            .iter()
            .zip(ports)
            .map(|(&(i, _), port)| (specs[i].port, port)),
    );
    emit_container_op(ctx, range, recv, kind, gate, lowering, inputs)
}

/// Bind a call's arguments to a method's `specs`: positional arguments in
/// order, then named ones by name (`exec =` is the trigger, not an argument).
/// Returns `(spec index, expr)` in source order, or `None` when a required
/// argument is missing.
fn bind_method_args<'a>(
    specs: &[MethodArg],
    args: &'a [CallArg],
) -> Option<Vec<(usize, &'a Expr)>> {
    let mut taken = vec![false; specs.len()];
    let mut bound = Vec::new();
    let mut next = 0;
    for a in args {
        let slot = match a {
            CallArg::Positional(_) => {
                next += 1;
                Some(next - 1).filter(|&i| i < specs.len())
            }
            CallArg::Named { name, .. } => specs.iter().position(|s| s.name == name.as_str()),
            CallArg::Spread(_) => None,
        };
        let value = match a {
            CallArg::Positional(x) => x,
            CallArg::Named { value, .. } => value,
            CallArg::Spread(_) => continue,
        };
        if let Some(i) = slot
            && !taken[i]
        {
            taken[i] = true;
            bound.push((i, value));
        }
    }
    specs
        .iter()
        .zip(&taken)
        .all(|(s, &t)| t || s.optional)
        .then_some(bound)
}

/// Build the gate of a `Generic` container method on `recv` from already
/// lowered `inputs` (gate input, source). Inputs are declared with the type
/// their argument gives them, outputs with the type the gate catalog gives
/// them, either overridden by the entry's `port_types`. Advances the exec chain
/// and returns the entry's result port.
pub(super) fn emit_container_op(
    ctx: &mut LowerCtx<'_>,
    range: &SourceRange,
    recv: PortRef,
    kind: &ContainerKind,
    gate: &'static str,
    lowering: &Lowering,
    mut inputs: Vec<(WirePort, PortRef)>,
) -> PortRef {
    let Lowering::Generic {
        args: specs,
        result,
        port_types,
    } = lowering
    else {
        unreachable!("a Custom container method has no generic gate")
    };
    let spec_index = |port: WirePort| {
        specs
            .iter()
            .position(|s| s.port == port)
            .unwrap_or(usize::MAX)
    };
    inputs.sort_by_key(|&(port, _)| spec_index(port));
    let declared = |port: WirePort| -> Type {
        if let Some((_, t)) = port_types.iter().find(|(p, _)| *p == port) {
            return t.clone();
        }
        specs
            .iter()
            .find(|s| s.port == port)
            .map_or(Type::Any, |s| s.port_type(kind))
    };
    let extra_in: Vec<(WirePort, Type, PortRef)> = inputs
        .into_iter()
        .map(|(port, src)| (port, declared(port), src))
        .collect();
    let extra_out = crate::catalog::methods::gate_outputs(gate, kind, *result, port_types);
    match kind {
        ContainerKind::Array { .. } => {
            array_exec_op(ctx, range, recv, gate, extra_in, extra_out, *result)
        }
        ContainerKind::Map { .. } => {
            map_exec_op(ctx, range, recv, gate, extra_in, extra_out, *result)
        }
    }
}

/// `arr.sortMultiple(other, ..., descending?)`: parallel arrays fill
/// `ArrayVarRef2..8` (this array is the sort key); a `descending` bool (named,
/// or a trailing bool positional) sets `bDescending`. A record array argument
/// is sorted along as every one of its columns. Variadic, so it has no
/// argument list in the table.
fn lower_sort_multiple(
    ctx: &mut LowerCtx<'_>,
    array_ref: PortRef,
    args: &[CallArg],
    range: &SourceRange,
) -> PortRef {
    const REFS: [WirePort; 7] = [
        WirePort::ArrayVarRef2,
        WirePort::ArrayVarRef3,
        WirePort::ArrayVarRef4,
        WirePort::ArrayVarRef5,
        WirePort::ArrayVarRef6,
        WirePort::ArrayVarRef7,
        WirePort::ArrayVarRef8,
    ];
    let mut ports: Vec<PortRef> = Vec::new();
    let mut array_args = 0;
    let mut descending: Option<&Expr> = None;
    let positional = args
        .iter()
        .filter(|a| matches!(a, CallArg::Positional(_)))
        .count();
    let mut seen = 0;
    for a in args {
        match a {
            CallArg::Named { name, value, .. } if name == "descending" => {
                descending = Some(value);
            }
            CallArg::Positional(expr) => {
                seen += 1;
                // Only a trailing bool is `descending` (typecheck's rule);
                // every other positional is a parallel array.
                if seen == positional && matches!(unwrap_ref(&ctx.type_of(expr)), Type::Bool) {
                    descending = Some(expr);
                    continue;
                }
                let arg_ports: Vec<PortRef> = match resolve_array_ref_arg(ctx, expr) {
                    Some(port) => vec![port],
                    None => resolve_record_columns(ctx, Columns::Array, expr)
                        .map(|fields| {
                            leaf_columns(Columns::Array, &fields)
                                .iter()
                                .map(|v| v.node_id.port(WirePort::ArrayVarRef))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                if arg_ports.is_empty() {
                    report_unresolved_container_arg(ctx, "sortMultiple", expr.range());
                    return synthesise_unsupported_no_warn(ctx, range);
                }
                // The gate reorders every parallel array along with the key.
                for port in &arg_ports {
                    if reject_const_container_mutation(
                        ctx,
                        port.node_id,
                        true,
                        "`.sortMultiple()`",
                        expr.range(),
                    ) {
                        return synthesise_unsupported_no_warn(ctx, range);
                    }
                }
                array_args += 1;
                ports.extend(arg_ports);
            }
            _ => {}
        }
    }
    // Typecheck counts each argument as one array (WS011 past seven), so
    // only record-array columns can overflow the gate here.
    if ports.len() > REFS.len() {
        if array_args <= REFS.len() {
            ctx.error(
                "WS011",
                format!(
                    "'sortMultiple' sorts at most {} parallel arrays along with this one, \
                     and these arguments are {} columns (a record array counts one per \
                     field)",
                    REFS.len(),
                    ports.len()
                ),
                range,
            );
        }
        return synthesise_unsupported_no_warn(ctx, range);
    }
    let mut extra: Vec<(WirePort, Type, PortRef)> = ports
        .into_iter()
        .zip(REFS)
        .map(|(port, slot)| (slot, Type::Array(Box::new(Type::Any)), port))
        .collect();
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

pub(super) fn lower_array_set(
    ctx: &mut LowerCtx<'_>,
    obj: &Expr,
    index: &Expr,
    value: &Expr,
    range: &SourceRange,
) {
    let Some(current_exec) = ctx.current_exec else {
        return;
    };
    // `pts[i] = rec` on a record array fans the record value across the field
    // arrays; checked first, as the single-array resolution misses the record.
    if let Some(fields) = resolve_record_columns(ctx, Columns::Array, obj)
        && lower_record_position_set(ctx, Columns::Array, &fields, index, value, range)
    {
        return;
    }
    // The `Value` port takes typecheck's element type; a `Type::Any` port
    // would hide the string -> bool coercion from `ctx.connect`. A target with
    // no single container ref would drop the write, so it is reported.
    let unresolved = "`[..] =` is an array/map index write";
    let Some(ContainerKind::Array { elem: elem_ty }) = ctx.container_kind(obj) else {
        report_unresolved_container(ctx, unresolved, range);
        return;
    };
    let Some(ContainerSource::Ref(array_ref)) = container_ref(
        ctx,
        obj,
        &ContainerKind::Array {
            elem: elem_ty.clone(),
        },
    ) else {
        report_unresolved_container(ctx, unresolved, range);
        return;
    };
    // Only an array var's own cache is reset; a scalar var holding a ref
    // (`cur[0] = v` on `var cur: *int[]`) still holds the same ref.
    let var_name = match obj {
        Expr::Ident { name, .. }
            if ctx
                .lookup_var(name)
                .is_some_and(|v| v.storage == VarStorage::Array) =>
        {
            name.clone()
        }
        _ => String::new(),
    };
    // `xs[i] = v` mutates, so the `const` container rule applies. Typecheck
    // rejects the direct spelling; this catches aliases (a `const` passed to a
    // `ys: T[]` parameter), which share the gate node.
    if reject_const_container_mutation(ctx, array_ref.node_id, true, "an index write", range) {
        return;
    }
    let (index_port, value_port) = lower_pair(ctx, index, value);
    ctx.current_exec.get_or_insert(current_exec);
    column_exec_op(
        ctx,
        Columns::Array,
        range,
        array_ref,
        gc::ARRAY_SET_AT_INDEX,
        vec![
            (WirePort::Index, Type::Int, index_port),
            (WirePort::Value, elem_ty, value_port),
        ],
        vec![],
        WirePort::ExecOut,
        Some("array set"),
    );
    if let Some(id) = ctx.lookup_var(&var_name).map(|v| v.node_id) {
        ctx.var_gets.remove(&id);
    }
}

/// The call `e` reads when `e` names that call's one output and the output
/// is an enum (`f(c).r` on `-> (r: Hue)`).
pub(super) fn named_enum_output_call<'e>(ctx: &LowerCtx<'_>, e: &'e Expr) -> Option<&'e Expr> {
    let Expr::FieldAccess { obj, field, .. } = e else {
        return None;
    };
    let decl = called_chip_decl(ctx, obj)?;
    (decl.outputs.len() == 1
        && decl.outputs[0].name == *field
        && matches!(unwrap_ref(&ctx.type_of(e)), Type::Enum { .. }))
    .then_some(obj.as_ref())
}

/// The user `mod`/`chip` a call expression invokes, in any callee form: a bare
/// name, a namespace member (`u.f(x)`), or a `self`-receiver method (`v.f()`).
pub(super) fn called_chip_decl(
    ctx: &LowerCtx<'_>,
    call: &Expr,
) -> Option<std::sync::Arc<ChipDecl>> {
    let Expr::Call { callee, .. } = call else {
        return None;
    };
    match callee.as_ref() {
        Expr::Ident { name, range } => ctx.call_target_chip(name, &range.file).cloned(),
        Expr::FieldAccess { obj, field, range } => {
            if let Expr::Ident { name: ns, .. } = obj.as_ref()
                && let Some(c) = ctx.lookup_ns_chip(ns, field)
            {
                return Some(std::sync::Arc::clone(c));
            }
            ctx.self_mod_chip(field, &range.file).cloned()
        }
        _ => None,
    }
}

/// The index or key `key` and the record `value` written at it, lowered in
/// that order, the key held before the value runs code that may change what
/// it reads. `None` when `value` has no record form.
fn keyed_record(ctx: &mut LowerCtx<'_>, key: &Expr, value: &Expr) -> Option<(PortRef, Fields)> {
    let mut ord = ordered::Ordered::default();
    let k = ord.port(ctx, key);
    let v = ord.source(ctx, "", None, value.range(), |ctx| {
        value_record_fields(ctx, value).map(Binding::Record)
    });
    let key_port = ord.port_of(ctx, k);
    match v.and_then(|v| ord.value(ctx, v)) {
        Some(Binding::Record(src)) => Some((key_port, src)),
        _ => None,
    }
}

/// Two values lowered in order, the first held before the second runs code
/// that may change what it reads.
fn lower_pair(ctx: &mut LowerCtx<'_>, a: &Expr, b: &Expr) -> (PortRef, PortRef) {
    let ports = lower_in_order(ctx, &[a, b]);
    (ports[0], ports[1])
}
